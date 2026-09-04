//! The `auris.synth.vocal` instrument: the voice a singer track previews through.
//!
//! A voice model renders offline and does not exist yet; this is what lets a melody with words
//! on it be *heard* while it is written. Three fixed formants — the classic parallel-bandpass
//! "ah" — over a band-limited saw, which is the oldest trick in speech synthesis and instantly
//! reads as "a voice" without pretending to be one. It shares every primitive the other
//! built-ins use: the same oscillator, the same [`Adsr`], the same voice pool, plus
//! [`Biquad`] sections from `auris-dsp` for the formants.
//!
//! It answers the two controllers a singer track writes: the modulation wheel opens extra
//! vibrato, and the expression pedal — the same controller 11 the frame renderer reads as
//! energy — scales the level, so what the preview does and what the model will be told agree.

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, CC_EXPRESSION, CC_MODULATION, Instrument, NoteEvent, ParamDescriptor, ParamId,
    ParamUnit, ParamValueCurve, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};

use crate::lfo::Lfo;
use crate::oscillator::{Oscillator, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;
use auris_dsp::{Adsr, Biquad, BiquadCoefficients};

/// Voices in the pool. A singer sings one line, but a preview auditioned as a chord should not
/// steal itself silent.
const VOICE_COUNT: usize = 8;

const P_ATTACK: u32 = 0;
const P_RELEASE: u32 = 1;
const P_VIBRATO_RATE: u32 = 2;
const P_VIBRATO: u32 = 3;
const P_BREATH: u32 = 4;
const P_LEVEL: u32 = 5;

/// Widest pitch bend followed, in semitones.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// The formants of an open "ah", as `(centre Hz, gain)`.
///
/// Textbook values for an adult voice, one vowel only: the preview's job is to sound sung, not
/// to sound *spelt*, and per-phoneme formants belong to the voice model this stands in for.
/// The gains fall the way vowel spectra do, and the third peak is what keeps the tone from
/// reading as a filter sweep.
const FORMANTS: [(f32, f32); 3] = [(700.0, 1.0), (1_220.0, 0.5), (2_600.0, 0.25)];

/// Resonance of each formant section — narrow enough to peak, wide enough to keep the note's
/// own harmonics audible between the peaks.
const FORMANT_Q: f32 = 8.0;

/// Fixed decay into the sustain, seconds. Short: a sung note settles quickly, and the audible
/// shape belongs to the attack and release the parameters own.
const DECAY_SECONDS: f32 = 0.04;

/// Sustain level under the peak.
const SUSTAIN_LEVEL: f32 = 0.85;

/// One sung note: the source, its envelope, its wobble, and the three formant sections.
#[derive(Clone, Debug)]
struct VocalVoice {
    source: Oscillator,
    breath: Oscillator,
    envelope: Adsr,
    /// Restarted at every note on, so repeated notes wobble the same way.
    vibrato: Lfo,
    formants: [Biquad; FORMANTS.len()],
    pitch: f32,
    velocity: f32,
}

impl VocalVoice {
    fn new(sample_rate: f32) -> Self {
        let mut source = Oscillator::new();
        let mut breath = Oscillator::new();
        source.set_sample_rate(sample_rate);
        breath.set_sample_rate(sample_rate);
        // The noise register is clocked by the oscillator's frequency; run it high so the hiss
        // is broadband rather than pitched.
        breath.set_frequency(sample_rate / 4.0);
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(sample_rate);
        let mut vibrato = Lfo::new();
        vibrato.set_sample_rate(sample_rate);
        Self {
            source,
            breath,
            envelope,
            vibrato,
            formants: [Biquad::new(BiquadCoefficients::identity()); FORMANTS.len()],
            pitch: 69.0,
            velocity: 0.0,
        }
    }
}

/// A formant-filtered preview voice for singer tracks.
pub struct Vocal {
    params: ParamBank,
    voices: Vec<VocalVoice>,
    allocator: VoiceAllocator,
    sample_rate: f32,

    bend: f32,
    /// Most recent modulation wheel, 0 to 1 — extra vibrato.
    modulation: f32,
    /// Most recent expression pedal, 0 to 1 — the energy the frames also read.
    expression: f32,
    /// How far the vibrato swings right now, in semitones.
    vibrato_depth: f32,
    breath_gain: f32,
    output_gain: f32,
}

impl Default for Vocal {
    fn default() -> Self {
        Vocal::new()
    }
}

impl Vocal {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.synth.vocal";

    /// Builds the preview voice with a gentle attack and a light vibrato.
    pub fn new() -> Self {
        let mut synth = Self {
            params: ParamBank::new(descriptors()),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            bend: 0.0,
            modulation: 0.0,
            expression: 1.0,
            vibrato_depth: 0.0,
            breath_gain: 0.0,
            output_gain: 1.0,
        };
        synth.refresh();
        synth
    }

    fn refresh(&mut self) {
        self.output_gain = db_to_gain(self.params.at(P_LEVEL));
        self.breath_gain = self.params.at(P_BREATH);
        // The wheel opens up to half a semitone on top of whatever the dial asks for.
        self.vibrato_depth = self.params.at(P_VIBRATO) + 0.5 * self.modulation;
        let vibrato_rate = self.params.at(P_VIBRATO_RATE);
        let (attack, release) = (self.params.at(P_ATTACK), self.params.at(P_RELEASE));
        for voice in &mut self.voices {
            voice
                .envelope
                .set_adsr(attack, DECAY_SECONDS, SUSTAIN_LEVEL, release);
            voice.vibrato.set_rate(vibrato_rate);
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let stolen = assignment.stolen;
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.pitch = f32::from(pitch);
        voice.velocity = velocity;
        if !stolen {
            // A silent voice can start from phase zero; a stolen one keeps its phase and its
            // filter state, because resetting either mid-flight clicks.
            voice.source.reset();
            voice.vibrato.reset();
            for filter in &mut voice.formants {
                filter.reset();
            }
        }
        voice.envelope.trigger();
    }

    fn note_off(&mut self, pitch: u8) {
        for index in self.allocator.note_off(pitch) {
            if let Some(voice) = self.voices.get_mut(index) {
                voice.envelope.release();
            }
        }
    }
}

fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.0, 2.0, 0.02)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.0, 4.0, 0.18)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(
            P_VIBRATO_RATE,
            "vibrato_rate",
            "Vibrato Rate",
            0.1,
            12.0,
            5.2,
        )
        .with_unit(ParamUnit::Hertz),
        ParamDescriptor::new(P_VIBRATO, "vibrato", "Vibrato", 0.0, 2.0, 0.12)
            .with_unit(ParamUnit::Semitones)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::percent(P_BREATH, "breath", "Breath", 0.06),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -6.0),
    ]
}

impl Parameterized for Vocal {
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

impl SegmentRenderer for Vocal {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => self.note_off(pitch),
            NoteEvent::AllNotesOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.release();
                    }
                }
            }
            NoteEvent::AllSoundOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.kill();
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
            }
            NoteEvent::Controller { number, value, .. } => {
                if number == CC_MODULATION {
                    self.modulation = finite_or(value, 0.0).clamp(0.0, 1.0);
                    self.refresh();
                }
                // The controller the frame renderer reads as energy; answering it here is what
                // keeps "what you hear" and "what the model is told" one story.
                if number == CC_EXPRESSION {
                    self.expression = finite_or(value, 0.0).clamp(0.0, 1.0);
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

        let bend = self.bend;
        let vibrato_depth = self.vibrato_depth;
        let breath_gain = self.breath_gain;

        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            let frequency = pitch_to_hz(voice.pitch + bend);
            let gain = voice.velocity;

            for sample in dst.iter_mut() {
                let swing = match vibrato_depth > 0.0 {
                    true => (voice.vibrato.next() * vibrato_depth / 12.0).exp2(),
                    false => {
                        voice.vibrato.next();
                        1.0
                    }
                };
                voice.source.set_frequency(frequency * swing);
                // Breath rides the envelope with the tone, so a released note does not leave
                // a hiss behind.
                let excitation = voice.source.next(Waveform::Saw)
                    + voice.breath.next(Waveform::Noise) * breath_gain;
                let mut shaped = 0.0;
                for (filter, (_, weight)) in voice.formants.iter_mut().zip(FORMANTS) {
                    shaped += filter.process_sample(excitation) * weight;
                }
                *sample += shaped * voice.envelope.process() * gain;
            }

            self.allocator
                .set_level(slot, voice.envelope.level() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(slot);
            }
        }

        let output_gain = self.output_gain * self.expression;
        for sample in dst.iter_mut() {
            *sample *= output_gain;
        }
    }
}

impl Instrument for Vocal {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Vocal",
            "A formant-filtered preview voice for singer tracks",
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        self.voices.clear();
        self.voices.reserve(VOICE_COUNT);
        for _ in 0..VOICE_COUNT {
            let mut voice = VocalVoice::new(self.sample_rate);
            for (filter, (centre, _)) in voice.formants.iter_mut().zip(FORMANTS) {
                filter.set_coefficients(BiquadCoefficients::bandpass(
                    f64::from(self.sample_rate),
                    centre,
                    FORMANT_Q,
                ));
            }
            self.voices.push(voice);
        }
        self.allocator.prepare(VOICE_COUNT);
        self.bend = 0.0;
        self.modulation = 0.0;
        self.expression = 1.0;
        self.refresh();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            voice.source.reset();
            for filter in &mut voice.formants {
                filter.reset();
            }
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.modulation = 0.0;
        self.expression = 1.0;
        self.refresh();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        spread_to_all_channels(out, frames);
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, band_amplitude, peak};

    const SAMPLE_RATE: f64 = 48_000.0;

    fn rig() -> Rig {
        let mut synth = Vocal::new();
        // Instant edges so a short render measures the sustained tone, not the ramps.
        synth.set_param_by_key("attack", 0.001);
        synth.set_param_by_key("release", 0.02);
        synth.set_param_by_key("vibrato", 0.0);
        Rig::new(Box::new(synth), SAMPLE_RATE, 512, 1)
    }

    fn note_on(pitch: u8, velocity: f32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity,
        }
    }

    #[test]
    fn the_formants_shape_the_spectrum() {
        let mut rig = rig();
        // A2: a 110 Hz saw puts harmonics through every band being compared.
        let samples = rig.render(48_000, &[note_on(45, 1.0)]);
        let settled = &samples[24_000..];
        let first = band_amplitude(settled, SAMPLE_RATE, 700.0);
        let gap = band_amplitude(settled, SAMPLE_RATE, 1_900.0);
        let third = band_amplitude(settled, SAMPLE_RATE, 2_600.0);
        assert!(
            first > gap * 2.0,
            "the first formant ({first:.4}) should stand over the gap ({gap:.4})"
        );
        assert!(
            third > gap,
            "the third formant ({third:.4}) should rise back out of the gap ({gap:.4})"
        );
    }

    #[test]
    fn a_released_note_falls_silent_and_frees_its_voice() {
        let mut rig = rig();
        let sounding = rig.render(4_800, &[note_on(60, 1.0)]);
        assert!(peak(&sounding) > 0.01, "the note should be audible");
        let off = NoteEvent::NoteOff {
            frame: 0,
            pitch: 60,
        };
        rig.render(4_800, &[off]);
        let tail = rig.render(4_800, &[]);
        assert!(
            peak(&tail) < 1.0e-4,
            "20 ms of release should be over, peak was {}",
            peak(&tail)
        );
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn velocity_and_expression_both_scale_the_level() {
        let mut rig = rig();
        let loud = peak(&rig.render(9_600, &[note_on(57, 1.0)])[4_800..]);
        rig.instrument.reset();
        let soft = peak(&rig.render(9_600, &[note_on(57, 0.25)])[4_800..]);
        assert!(
            soft < loud * 0.5,
            "velocity 0.25 ({soft:.4}) should sit well under velocity 1 ({loud:.4})"
        );

        rig.instrument.reset();
        let pedal = NoteEvent::Controller {
            frame: 0,
            number: CC_EXPRESSION,
            value: 0.25,
        };
        let ridden = peak(&rig.render(9_600, &[note_on(57, 1.0), pedal])[4_800..]);
        assert!(
            ridden < loud * 0.5,
            "the pedal at 0.25 ({ridden:.4}) should duck the same note ({loud:.4})"
        );
    }
}
