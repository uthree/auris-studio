//! The pitched kick-drum basses behind `auris.synth.boom_bass`, `auris.synth.kick_bass` and
//! `auris.synth.sub_bass`.
//!
//! The kits in [`electro`](crate::electro) tune their kick once. These play it from the
//! keyboard: every note is the same formula as the kit's kick — a sine whose pitch falls
//! exponentially onto the note, a click, and soft clipping — so a bass line written with them is
//! a kick that follows the chords. That is the trap "808", the club "kick bass" whose every note
//! is a tuned kick, and, with the punch turned off, a plain sustained sub-bass.
//!
//! ```text
//!  sine at note · 2^(punch · e^(−t/τ))  + click ─ tanh drive ─ one-pole tone ─ ADSR ─ out
//! ```
//!
//! # One voice
//!
//! A sub-bass is monophonic on purpose. Two sines a semitone apart forty hertz up beat at a
//! couple of hertz, which is not a chord but a wobble in the level of the whole mix; every bass
//! of this kind therefore plays one note at a time. With `glide` above zero, a note played while
//! another is still held slides to it instead of striking again — the slide that trap basslines
//! are written around — and releasing it slides back to the key still down.
//!
//! # Realtime behaviour
//!
//! The held keys are a fixed array; nothing allocates after `prepare`.

use std::f32::consts::TAU;

use auris_core::motion::{MOTION_MODES, MOTION_POINTS, MotionFrame, MotionGeometry};
use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, ParamValueCurve,
    Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::{Adsr, Biquad, BiquadCoefficients};

use crate::observe::{Observer, harmonics};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};

const P_PUNCH: u32 = 0;
const P_PUNCH_TIME: u32 = 1;
const P_CLICK: u32 = 2;
const P_DRIVE: u32 = 3;
const P_TONE: u32 = 4;
const P_ATTACK: u32 = 5;
const P_DECAY: u32 = 6;
const P_SUSTAIN: u32 = 7;
const P_RELEASE: u32 = 8;
const P_GLIDE: u32 = 9;
const P_LEVEL: u32 = 10;

/// Octaves each note's sweep starts above it at a punch of 1.
const PUNCH_OCTAVES: f32 = 4.0;

/// Keys remembered for last-note priority. Ten fingers and a few to spare.
const HELD_KEYS: usize = 16;

/// Widest pitch bend followed, in semitones.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// How long the click lasts: a few cycles of the beater, not a note.
const CLICK_SECONDS: f32 = 0.012;

/// One of the patches, each its own instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoomPatch {
    /// The trap "808": a long, slightly distorted sine boom with a soft punch and a glide
    /// between held notes.
    Boom,
    /// The club kick bass: every note is a hard, clicky, heavily distorted kick tuned to the key.
    KickBass,
    /// A clean, sustained sine sub-bass under everything else.
    Sub,
}

impl BoomPatch {
    /// Every patch, in the order the browser lists them.
    pub const ALL: [Self; 3] = [Self::Boom, Self::KickBass, Self::Sub];

    /// Stable plugin id stored in a project.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Boom => "auris.synth.boom_bass",
            Self::KickBass => "auris.synth.kick_bass",
            Self::Sub => "auris.synth.sub_bass",
        }
    }

    /// The patch an id names, if it is one of these.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|patch| patch.id() == id)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Boom => "Boom Bass",
            Self::KickBass => "Kick Bass",
            Self::Sub => "Sub Bass",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Boom => {
                "Trap 808-style boom: a long distorted sine kick played as a bass, with glide"
            }
            Self::KickBass => {
                "Club kick bass: every note a hard, clicky, distorted kick tuned to the key"
            }
            Self::Sub => "Clean sustained sine sub-bass, felt more than heard",
        }
    }

    /// The patch's own settings, as the parameters' defaults.
    fn defaults(self) -> [f32; 11] {
        match self {
            //             punch ptime  click drive tone     att    dec   sus   rel   glide level
            Self::Boom => [
                0.3, 0.025, 0.1, 0.4, 3_200.0, 0.001, 1.6, 0.35, 0.3, 0.07, -6.0,
            ],
            Self::KickBass => [
                0.65, 0.012, 0.6, 0.75, 6_000.0, 0.001, 0.32, 0.0, 0.08, 0.0, -6.0,
            ],
            Self::Sub => [
                0.0, 0.02, 0.0, 0.04, 900.0, 0.006, 0.4, 0.85, 0.15, 0.05, -6.0,
            ],
        }
    }
}

/// A pitched kick-drum bass: one sine voice with a falling-pitch punch, click and drive.
#[derive(Clone, Debug)]
pub struct BoomBass {
    patch: BoomPatch,
    params: ParamBank,
    prepared: bool,
    sample_rate: f32,
    inv_sample_rate: f32,

    envelope: Adsr,
    click: f32,
    click_coef: f32,
    click_filter: Biquad,
    rng: u32,
    phase: f32,
    /// The note sounding, in semitones, and the one it is gliding to.
    pitch: f32,
    target: f32,
    glide_coef: f32,
    /// Octaves above the note the punch still has to fall.
    sweep: f32,
    sweep_coef: f32,
    velocity: f32,
    tone_coef: f32,
    tone_hz: f32,
    tone_state: f32,
    held: [u8; HELD_KEYS],
    held_count: usize,

    punch: f32,
    click_gain: f32,
    drive: f32,
    drive_norm: f32,
    glide: f32,
    bend: f32,
    output_gain: f32,
    channel_volume: f32,
    observer: Observer,
}

impl BoomBass {
    /// Builds `patch` with its own settings.
    pub fn new(patch: BoomPatch) -> Self {
        let mut bass = Self {
            patch,
            params: ParamBank::new(descriptors(patch)),
            prepared: false,
            sample_rate: 48_000.0,
            inv_sample_rate: 1.0 / 48_000.0,
            envelope: Adsr::new(),
            click: 0.0,
            click_coef: 0.0,
            click_filter: Biquad::new(BiquadCoefficients::identity()),
            rng: 1,
            phase: 0.0,
            pitch: 36.0,
            target: 36.0,
            glide_coef: 1.0,
            sweep: 0.0,
            sweep_coef: 0.0,
            velocity: 0.0,
            tone_coef: 1.0,
            tone_hz: 1_000.0,
            tone_state: 0.0,
            held: [0; HELD_KEYS],
            held_count: 0,
            punch: 0.0,
            click_gain: 0.0,
            drive: 0.0,
            drive_norm: 1.0,
            glide: 0.0,
            bend: 0.0,
            output_gain: 1.0,
            channel_volume: 1.0,
            observer: Observer::default(),
        };
        bass.refresh();
        bass
    }

    /// Which patch this is.
    pub fn patch(&self) -> BoomPatch {
        self.patch
    }

    fn refresh(&mut self) {
        let rate = self.sample_rate;
        let p = |index| self.params.at(index);
        self.punch = p(P_PUNCH);
        self.sweep_coef = (-1.0 / (p(P_PUNCH_TIME).max(0.0005) * rate)).exp();
        self.click_gain = p(P_CLICK);
        self.drive = p(P_DRIVE);
        self.drive_norm = 1.0 / (1.0 + self.drive * 9.0).tanh();
        let tone = p(P_TONE).min(rate * 0.45);
        self.tone_hz = tone;
        self.tone_coef = 1.0 - (-TAU * tone / rate).exp();
        self.envelope.set_sample_rate(rate);
        self.envelope
            .set_adsr(p(P_ATTACK), p(P_DECAY), p(P_SUSTAIN), p(P_RELEASE));
        self.glide = p(P_GLIDE);
        // A time constant of a third of the glide: the slide is all but over when it says.
        self.glide_coef = if self.glide > 0.0 {
            1.0 - (-3.0 / (self.glide * rate)).exp()
        } else {
            1.0
        };
        self.click_coef = (-9.0 / (CLICK_SECONDS * rate)).exp();
        self.click_filter
            .set_coefficients(BiquadCoefficients::bandpass(
                f64::from(rate),
                3_500.0f32.min(rate * 0.45),
                0.7,
            ));
        self.output_gain = db_to_gain(p(P_LEVEL));
    }

    fn strike(&mut self, key: u8, velocity: f32) {
        self.velocity = velocity;
        self.pitch = f32::from(key);
        self.target = self.pitch;
        // A harder note punches further, as a harder kick does.
        self.sweep = self.punch * PUNCH_OCTAVES * (0.8 + 0.2 * velocity);
        self.click = 1.0;
        self.rng = 0x2545_F491 ^ u32::from(key);
        // The phase carries on: restarting a sounding sine at zero is a step, and a retriggered
        // bass would click on every note. A silent voice starts at zero so every hit is alike.
        if !self.envelope.is_active() {
            self.phase = 0.0;
            self.tone_state = 0.0;
            self.click_filter.reset();
        }
        self.envelope.trigger();
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        if !self.prepared {
            return;
        }
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let legato = self.held_count > 0 && self.glide > 0.0 && self.envelope.is_active();
        self.remove_held(key);
        if self.held_count == HELD_KEYS {
            self.held.copy_within(1.., 0);
            self.held_count -= 1;
        }
        self.held[self.held_count] = key;
        self.held_count += 1;
        if legato {
            self.target = f32::from(key);
        } else {
            self.strike(key, velocity);
        }
    }

    fn note_off(&mut self, key: u8) {
        let was_top = self.held_count > 0 && self.held[self.held_count - 1] == key;
        self.remove_held(key);
        if !was_top {
            return;
        }
        match self.held_count {
            0 => self.envelope.release(),
            n => {
                // Back to the key still down. With a glide it slides there; without one it
                // jumps, as a mono synth's last-note priority does, without a new strike.
                self.target = f32::from(self.held[n - 1]);
                if self.glide <= 0.0 {
                    self.pitch = self.target;
                }
            }
        }
    }

    fn remove_held(&mut self, key: u8) {
        if let Some(index) = self.held[..self.held_count].iter().position(|k| *k == key) {
            self.held.copy_within(index + 1..self.held_count, index);
            self.held_count -= 1;
        }
    }

    /// The pitch sounding now, in hertz, punch included.
    fn frequency(&self) -> f32 {
        pitch_to_hz(self.pitch + self.bend) * self.sweep.exp2()
    }

    /// The note as one cycle of what leaves the voice.
    fn publish_motion(&mut self) {
        let sounding = self.envelope.is_active();
        let mut frame = MotionFrame {
            geometry: MotionGeometry::Oscillator,
            active: usize::from(sounding),
            expression: self.channel_volume,
            ..MotionFrame::default()
        };
        if sounding {
            let hz = self.frequency().max(1.0);
            let mut cycle = [0.0; MOTION_POINTS];
            boom_cycle(self.drive, self.tone_hz / hz, &mut cycle);
            let mut modes = [0.0; MOTION_MODES];
            harmonics(&cycle, &mut modes);
            let level = self.envelope.level() * self.velocity;
            let voice = &mut frame.voices[0];
            for (point, sample) in voice.points.iter_mut().zip(cycle) {
                *point = sample * level;
            }
            voice.modes = modes;
            voice.pitch = self.pitch + self.bend;
            voice.level = level;
            voice.contact = 0.0;
            voice.excitation = hz;
            voice.held = self.held_count > 0;
        }
        self.observer.publish(&frame);
    }

    #[inline]
    fn white(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// One steady cycle of the voice's sine after its drive and tone filter, at a tone of
/// `tone_ratio` times the note: what the display shows of a sounding note.
fn boom_cycle(drive: f32, tone_ratio: f32, out: &mut [f32]) {
    let length = out.len().max(1);
    let gain = 1.0 + drive.clamp(0.0, 1.0) * 9.0;
    let norm = 1.0 / gain.tanh();
    // The one-pole at `length` samples a cycle; three cycles first so it has settled.
    let coef = 1.0 - (-TAU * tone_ratio.max(0.0) / length as f32).exp();
    let mut state = 0.0_f32;
    for pass in 0..4 {
        for (index, point) in out.iter_mut().enumerate() {
            let mut x = (TAU * index as f32 / length as f32).sin();
            if drive > 0.0 {
                x = (x * gain).tanh() * norm;
            }
            state += (x - state) * coef;
            if pass == 3 {
                *point = state;
            }
        }
    }
}

fn descriptors(patch: BoomPatch) -> Vec<ParamDescriptor> {
    let d = patch.defaults();
    let seconds = |id, key: &'static str, name: &'static str, min, max, default| {
        ParamDescriptor::new(id, key, name, min, max, default)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0))
    };
    vec![
        ParamDescriptor::percent(P_PUNCH, "punch", "Punch", d[0]),
        seconds(P_PUNCH_TIME, "punch_time", "Punch Time", 0.002, 0.1, d[1]),
        ParamDescriptor::percent(P_CLICK, "click", "Click", d[2]),
        ParamDescriptor::percent(P_DRIVE, "drive", "Drive", d[3]),
        ParamDescriptor::hertz(P_TONE, "tone", "Tone", 150.0, 12_000.0, d[4]),
        seconds(P_ATTACK, "attack", "Attack", 0.001, 0.5, d[5]),
        seconds(P_DECAY, "decay", "Decay", 0.02, 4.0, d[6]),
        ParamDescriptor::percent(P_SUSTAIN, "sustain", "Sustain", d[7]),
        seconds(P_RELEASE, "release", "Release", 0.005, 2.0, d[8]),
        seconds(P_GLIDE, "glide", "Glide", 0.0, 0.5, d[9]),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, d[10]),
    ]
}

impl Parameterized for BoomBass {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) {
            self.refresh();
        }
    }
}

impl SegmentRenderer for BoomBass {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => self.note_off(pitch),
            NoteEvent::AllNotesOff { .. } => {
                self.held_count = 0;
                self.envelope.release();
            }
            NoteEvent::AllSoundOff { .. } => {
                self.held_count = 0;
                self.envelope.kill();
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
            }
            NoteEvent::Controller { number, value, .. } => {
                if number == 7 {
                    self.channel_volume = finite_or(value, 1.0).clamp(0.0, 1.0).powi(2);
                }
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((mono, _)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(dst) = mono.get_mut(start..end) else {
            return;
        };
        dst.fill(0.0);
        if !self.envelope.is_active() {
            return;
        }
        let gain =
            self.output_gain * self.channel_volume * self.velocity * (0.35 + 0.65 * self.velocity);
        let inv_rate = self.inv_sample_rate;
        for sample in dst.iter_mut() {
            self.pitch += (self.target - self.pitch) * self.glide_coef;
            let hz = self.frequency();
            self.sweep *= self.sweep_coef;
            let mut x = (TAU * self.phase).sin();
            self.phase += hz * inv_rate;
            self.phase -= self.phase.floor();
            if self.click > 1.0e-4 {
                let noise = self.white();
                x += self.click_filter.process_sample(noise) * self.click * self.click_gain * 2.0;
                self.click *= self.click_coef;
            }
            if self.drive > 0.0 {
                x = (x * (1.0 + self.drive * 9.0)).tanh() * self.drive_norm;
            }
            self.tone_state += (x - self.tone_state) * self.tone_coef;
            *sample = self.tone_state * self.envelope.process() * gain;
        }
        if !(self.phase.is_finite() && self.tone_state.is_finite() && self.pitch.is_finite()) {
            self.phase = 0.0;
            self.tone_state = 0.0;
            self.pitch = self.target;
            self.click_filter.reset();
        }
    }
}

impl Instrument for BoomBass {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            self.patch.id(),
            self.patch.name(),
            self.patch.description(),
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        self.inv_sample_rate = 1.0 / self.sample_rate;
        self.prepared = true;
        self.bend = 0.0;
        self.channel_volume = 1.0;
        self.refresh();
        self.reset();
    }

    fn reset(&mut self) {
        self.envelope.silence();
        self.held_count = 0;
        self.phase = 0.0;
        self.tone_state = 0.0;
        self.sweep = 0.0;
        self.click = 0.0;
        self.click_filter.reset();
        self.bend = 0.0;
        self.channel_volume = 1.0;
        self.observer.reset(MotionGeometry::Oscillator);
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        spread_to_all_channels(out, frames);
        if self.observer.due(frames, self.sample_rate) {
            self.publish_motion();
        }
    }

    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.observer.monitor())
    }

    fn active_voices(&self) -> usize {
        usize::from(self.envelope.is_active())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        Rig, band_amplitude, count_allocations, peak, peak_db, rms, zero_crossing_hz,
    };

    const RATE: f64 = 48_000.0;

    fn rig(patch: BoomPatch) -> Rig {
        let mut bass = BoomBass::new(patch);
        bass.set_param_by_key("level", 0.0);
        Rig::new(Box::new(bass), RATE, 512, 2)
    }

    fn on(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    fn off(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOff { frame, pitch }
    }

    fn ms(milliseconds: usize) -> usize {
        milliseconds * 48
    }

    #[test]
    fn every_patch_shares_one_parameter_layout() {
        let keys = |bass: &BoomBass| {
            bass.parameters()
                .iter()
                .map(|p| p.key.to_string())
                .collect::<Vec<_>>()
        };
        let reference = BoomBass::new(BoomPatch::Boom);
        for patch in BoomPatch::ALL {
            let bass = BoomBass::new(patch);
            assert_eq!(bass.descriptor().id, patch.id());
            assert_eq!(BoomPatch::from_id(patch.id()), Some(patch));
            assert_eq!(bass.descriptor().category, PluginCategory::Synth);
            assert_eq!(keys(&bass), keys(&reference));
            for (index, descriptor) in bass.parameters().iter().enumerate() {
                assert_eq!(descriptor.id.index(), index);
                assert!((descriptor.min..=descriptor.max).contains(&descriptor.default));
            }
        }
    }

    #[test]
    fn every_note_settles_on_its_own_pitch() {
        for patch in BoomPatch::ALL {
            for key in [28u8, 33, 40, 48] {
                let rendered = rig(patch).render(ms(250), &[on(0, key)]);
                let hz = zero_crossing_hz(&rendered[ms(120)..ms(250)], RATE);
                let want = f64::from(pitch_to_hz(f32::from(key)));
                assert!(
                    (hz - want).abs() < want * 0.01,
                    "{patch:?} key {key}: {hz:.2} Hz, want {want:.2}"
                );
            }
        }
    }

    #[test]
    fn the_kick_bass_punches_from_far_above_the_note_and_the_sub_does_not() {
        let start = |patch| {
            let rendered = rig(patch).render(ms(40), &[on(0, 33)]);
            zero_crossing_hz(&rendered[..ms(40)], RATE)
        };
        let note = f64::from(pitch_to_hz(33.0));
        assert!(start(BoomPatch::KickBass) > note * 1.4);
        assert!((start(BoomPatch::Sub) - note).abs() < note * 0.05);
    }

    #[test]
    fn the_kick_bass_is_distorted_and_the_sub_is_clean() {
        let third = |patch| {
            let rendered = rig(patch).render(ms(150), &[on(0, 33)]);
            let tail = &rendered[ms(50)..ms(150)];
            let note = f64::from(pitch_to_hz(33.0));
            band_amplitude(tail, RATE, note * 3.0) / band_amplitude(tail, RATE, note)
        };
        let (kick, sub) = (third(BoomPatch::KickBass), third(BoomPatch::Sub));
        assert!(kick > 0.15, "kick bass third harmonic {kick}");
        // The sub keeps a little warmth from its light drive, and no more.
        assert!(sub < kick / 3.0, "sub {sub} against kick {kick}");
    }

    #[test]
    fn the_sub_sustains_while_held_and_stops_after_its_release() {
        let mut rig = rig(BoomPatch::Sub);
        let rendered = rig.render(48_000 * 2, &[on(0, 36), off(48_000, 36)]);
        let held = rms(&rendered[ms(800)..ms(1_000)]);
        assert!(held > 0.4, "held at {held}");
        assert!(peak_db(&rendered[ms(1_200)..]) < -80.0);
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn the_kick_bass_is_over_on_its_own_like_a_drum() {
        let mut rig = rig(BoomPatch::KickBass);
        let rendered = rig.render(ms(600), &[on(0, 36)]);
        assert!(peak(&rendered) > 0.9);
        assert!(peak_db(&rendered[ms(400)..]) < -80.0);
        assert_eq!(
            rig.instrument.active_voices(),
            0,
            "the key is still down, but the hit is over"
        );
    }

    #[test]
    fn a_held_boom_glides_to_the_next_note_without_striking_again() {
        let mut rig = rig(BoomPatch::Boom);
        let rendered = rig.render(ms(600), &[on(0, 33), on(ms(200) as u32, 40)]);
        assert_eq!(rig.instrument.active_voices(), 1);
        let after = zero_crossing_hz(&rendered[ms(400)..ms(600)], RATE);
        let want = f64::from(pitch_to_hz(40.0));
        assert!((after - want).abs() < want * 0.01, "{after} vs {want}");
        // No new strike: the level carries on falling instead of jumping back to the top.
        let before = peak(&rendered[ms(150)..ms(200)]);
        let later = peak(&rendered[ms(200)..ms(260)]);
        assert!(later <= before * 1.05, "{later} after vs {before} before");
    }

    #[test]
    fn a_detached_note_strikes_again_and_one_voice_plays_at_a_time() {
        let mut rig = rig(BoomPatch::Boom);
        let rendered = rig.render(
            ms(700),
            &[
                on(0, 33),
                off(ms(100) as u32, 33),
                on(ms(400) as u32, 40),
                on(ms(400) as u32, 45),
            ],
        );
        assert_eq!(rig.instrument.active_voices(), 1);
        assert!(
            peak(&rendered[ms(400)..ms(450)]) > 0.9,
            "the new note did not strike"
        );
    }

    #[test]
    fn releasing_the_top_key_returns_to_the_one_still_held() {
        let mut rig = rig(BoomPatch::Sub);
        let rendered = rig.render(
            ms(800),
            &[on(0, 33), on(ms(100) as u32, 40), off(ms(300) as u32, 40)],
        );
        let back = zero_crossing_hz(&rendered[ms(550)..ms(800)], RATE);
        let want = f64::from(pitch_to_hz(33.0));
        assert!((back - want).abs() < want * 0.01, "{back} vs {want}");
    }

    #[test]
    fn the_display_shows_the_note_punching_down_onto_its_pitch() {
        let mut rig = rig(BoomPatch::KickBass);
        rig.set_param("punch_time", 0.05);
        let monitor = rig.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        // The first frame is due a thirtieth of a second in, in the 512-frame blocks drawn here.
        rig.render(ms(45), &[on(0, 36)]);
        let early = monitor.read().unwrap();
        assert_eq!(early.active, 1);
        assert_eq!(early.geometry, MotionGeometry::Oscillator);
        let voice = early.voices[0];
        assert_eq!(voice.pitch, 36.0);
        assert!(voice.held);
        // Forty milliseconds in, a slow punch is still well above the note's 65 Hz.
        assert!(voice.excitation > 100.0, "{}", voice.excitation);
        rig.render(ms(60), &[]);
        let later = monitor.read().unwrap().voices[0];
        let note = pitch_to_hz(36.0);
        assert!(
            later.excitation < voice.excitation && later.excitation >= note,
            "{} then {}",
            voice.excitation,
            later.excitation
        );
        // Driven hard, the cycle is squared off: its third harmonic is plainly there.
        assert!(later.modes[2] > 0.15, "{:?}", later.modes);
        assert!(later.points.iter().any(|point| point.abs() > 0.01));
        rig.instrument.reset();
        assert_eq!(monitor.read().unwrap().active, 0);
    }

    #[test]
    fn the_sub_is_drawn_as_a_clean_sine() {
        let mut rig = rig(BoomPatch::Sub);
        let monitor = rig.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        rig.render(ms(300), &[on(0, 28)]);
        let voice = monitor.read().unwrap().voices[0];
        // Barely driven: no even harmonics at all, and only a trace of the odd ones.
        assert!((voice.modes[0] - 1.0).abs() < 1e-3);
        assert!(
            voice.modes[1] < 1e-3 && voice.modes[2] < 0.15,
            "{:?}",
            voice.modes
        );
        assert!(
            voice.modes[3..].iter().all(|mode| *mode < 0.05),
            "{:?}",
            voice.modes
        );
    }

    #[test]
    fn watching_never_changes_the_sound() {
        let mut watched = rig(BoomPatch::Boom);
        let mut plain = rig(BoomPatch::Boom);
        watched.instrument.motion_monitor().unwrap().watch(true);
        let events = [on(0, 36), on(ms(100) as u32, 43), off(ms(300) as u32, 43)];
        assert_eq!(
            watched.render(ms(500), &events),
            plain.render(ms(500), &events)
        );
    }

    #[test]
    fn processing_allocates_nothing() {
        let mut bass = BoomBass::new(BoomPatch::Boom);
        bass.prepare(&PrepareContext::new(RATE, 512, 2));
        let mut output = AudioBuffer::stereo(512, RATE);
        let context = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let events: [NoteEvent; 40] = std::array::from_fn(|index| {
            if index % 2 == 0 {
                on(index as u32 * 10, 30 + (index % 24) as u8)
            } else {
                off(index as u32 * 10, 30 + ((index - 1) % 24) as u8)
            }
        });
        let count = count_allocations(|| {
            bass.process(&events, &mut output, &context);
            bass.set_param_by_key("glide", 0.2);
            bass.process(&[], &mut output, &context);
        });
        assert_eq!(count, 0);
    }
}
