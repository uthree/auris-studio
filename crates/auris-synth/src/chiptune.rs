//! The `auris.synth.chiptune` instrument.

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, ParamValueCurve,
    Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::lfo::Lfo;
use crate::oscillator::{Oscillator, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;
use auris_dsp::Adsr;
use auris_dsp::adsr::SEGMENT_TIME_CONSTANTS;

/// Voices in the pool. Eight is well past the four channels a chiptune would really have had,
/// which leaves room for held chords plus the tails of the notes before them.
const VOICE_COUNT: usize = 8;

/// Detuned copies per voice.
const MAX_UNISON: usize = 4;

/// Parameter ids, matching the order the descriptors are built in.
const P_WAVEFORM: u32 = 0;
const P_PULSE_WIDTH: u32 = 1;
const P_OCTAVE: u32 = 2;
const P_DETUNE: u32 = 3;
const P_ATTACK: u32 = 4;
const P_DECAY: u32 = 5;
const P_SUSTAIN: u32 = 6;
const P_RELEASE: u32 = 7;
const P_GLIDE: u32 = 8;
const P_VIBRATO_RATE: u32 = 9;
const P_VIBRATO: u32 = 10;
const P_MOD_DEPTH: u32 = 11;
const P_UNISON: u32 = 12;
const P_SPREAD: u32 = 13;
const P_BITS: u32 = 14;
const P_LEVEL: u32 = 15;

/// Widest pitch bend the instrument will follow, in semitones. Two octaves covers every
/// sensible bend range without letting a runaway controller push a voice past Nyquist.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// How far the modulation wheel bends the pitch at the top of its travel, by default.
///
/// Half a semitone, which is what a mod wheel does on almost every synthesiser ever sold: enough
/// to be unmistakably vibrato and short of the quarter-tone where it starts sounding out of tune.
/// A patch that wants none sets it to zero — but the default is *not* zero, because a wheel that
/// does nothing until a parameter is found is a wheel nobody discovers.
pub const DEFAULT_MOD_DEPTH: f32 = 0.5;

/// One voice's oscillators and envelope.
#[derive(Clone, Debug)]
struct ChiptuneVoice {
    oscillators: [Oscillator; MAX_UNISON],
    envelope: Adsr,
    /// The vibrato, restarted at every note on so a chord wobbles together.
    vibrato: Lfo,
    /// MIDI note the voice was started with, before octave, detune and bend.
    pitch: f32,
    /// Frequency the voice is sounding at right now; chases `target` when glide is on.
    frequency: f32,
    target: f32,
    velocity: f32,
}

impl ChiptuneVoice {
    fn new(index: usize, sample_rate: f32) -> Self {
        let oscillators = std::array::from_fn(|slot| {
            let mut oscillator =
                Oscillator::with_seed(Oscillator::seed_for_voice(index * MAX_UNISON + slot));
            oscillator.set_sample_rate(sample_rate);
            oscillator
        });
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(sample_rate);
        let mut vibrato = Lfo::new();
        vibrato.set_sample_rate(sample_rate);
        Self {
            oscillators,
            envelope,
            vibrato,
            pitch: 69.0,
            frequency: 440.0,
            target: 440.0,
            velocity: 0.0,
        }
    }
}

/// A polyphonic pulse/saw/triangle/noise synth with unison and a bit crusher.
///
/// This is the instrument the rest of the application is built against: five waveforms, a
/// normal ADSR, portamento, up to four detuned copies per voice, and a sample quantiser that
/// gives back the 4-bit grain of the hardware the sound comes from.
#[derive(Clone, Debug)]
pub struct Chiptune {
    params: ParamBank,
    voices: Vec<ChiptuneVoice>,
    allocator: VoiceAllocator,
    sample_rate: f32,

    waveform: Waveform,
    /// Octave, detune and pitch bend folded into one offset in semitones.
    pitch_offset: f32,
    bend: f32,
    /// Most recent modulation, 0 to 1.
    modulation: f32,
    /// How far the vibrato swings right now, in semitones: the patch's own depth plus whatever
    /// the modulation is asking for.
    vibrato_depth: f32,
    /// How fast it swings, in hertz.
    vibrato_rate: f32,
    glide_coeff: f32,
    unison: usize,
    unison_ratios: [f32; MAX_UNISON],
    unison_gain: f32,
    crush_levels: f32,
    output_gain: f32,
    /// Frequency the last note started on, so a new note can glide from where the last one was.
    last_frequency: Option<f32>,
}

impl Default for Chiptune {
    fn default() -> Self {
        Chiptune::new()
    }
}

impl Chiptune {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.synth.chiptune";

    /// Builds the instrument with its default patch: a 50 % square with a short attack.
    pub fn new() -> Self {
        let mut synth = Self {
            params: ParamBank::new(descriptors()),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            waveform: Waveform::Square,
            pitch_offset: 0.0,
            bend: 0.0,
            modulation: 0.0,
            vibrato_depth: 0.0,
            vibrato_rate: 0.0,
            glide_coeff: 1.0,
            unison: 1,
            unison_ratios: [1.0; MAX_UNISON],
            unison_gain: 1.0,
            crush_levels: 0.0,
            output_gain: 1.0,
            last_frequency: None,
        };
        synth.refresh();
        synth
    }

    /// Recomputes everything derived from the parameters.
    ///
    /// Cheap enough to run on every parameter write — a few dozen operations plus a push of the
    /// envelope times into the voice pool — which keeps a moving knob in sync with sounding
    /// notes without any per-sample parameter lookups.
    fn refresh(&mut self) {
        self.waveform = Waveform::from_param(self.params.at(P_WAVEFORM));
        self.pitch_offset = self.params.at(P_OCTAVE) * 12.0 + self.params.at(P_DETUNE) + self.bend;
        self.output_gain = db_to_gain(self.params.at(P_LEVEL));

        // The patch's own vibrato plus whatever the wheel is asking for. Added rather than
        // blended, because they are two people's decisions: the patch says how much this *sound*
        // wobbles and the wheel says how much more is wanted right now.
        self.vibrato_rate = self.params.at(P_VIBRATO_RATE);
        self.vibrato_depth =
            self.params.at(P_VIBRATO) + self.params.at(P_MOD_DEPTH) * self.modulation;
        for voice in &mut self.voices {
            voice.vibrato.set_rate(self.vibrato_rate);
        }

        let bits = self.params.at(P_BITS).round().clamp(1.0, 16.0);
        // A depth of 16 bits is finer than the f32 mix path resolves, so it means "off".
        self.crush_levels = if bits >= 16.0 {
            0.0
        } else {
            // Signed quantisation: n bits give 2^(n-1) steps either side of zero.
            (bits - 1.0).exp2()
        };

        let glide = self.params.at(P_GLIDE);
        self.glide_coeff = if glide <= 0.0 {
            1.0
        } else {
            // Same convention as the envelope segments: the pitch is within -80 dB of the
            // target — a fraction of a cent — after the stated time.
            1.0 - (-SEGMENT_TIME_CONSTANTS / (glide * self.sample_rate).max(1.0)).exp()
        };

        self.unison = (self
            .params
            .at(P_UNISON)
            .round()
            .clamp(1.0, MAX_UNISON as f32)) as usize;
        let spread = self.params.at(P_SPREAD);
        for (slot, ratio) in self.unison_ratios.iter_mut().enumerate() {
            let offset = if self.unison > 1 {
                // Spread the copies evenly over -spread..+spread semitones.
                (2.0 * slot as f32 / (self.unison - 1) as f32 - 1.0) * spread
            } else {
                0.0
            };
            *ratio = (offset / 12.0).exp2();
        }
        // Detuned copies are only partly correlated, so power sums rather than amplitude:
        // 1/sqrt(n) keeps the level roughly constant as copies are added.
        self.unison_gain = 1.0 / (self.unison as f32).sqrt();

        let pulse_width = self.params.at(P_PULSE_WIDTH);
        let (attack, decay) = (self.params.at(P_ATTACK), self.params.at(P_DECAY));
        let (sustain, release) = (self.params.at(P_SUSTAIN), self.params.at(P_RELEASE));
        for voice in &mut self.voices {
            voice.envelope.set_adsr(attack, decay, sustain, release);
            for oscillator in &mut voice.oscillators {
                oscillator.set_pulse_width(pulse_width);
            }
        }
    }

    fn frequency_for(&self, pitch: f32) -> f32 {
        pitch_to_hz(pitch + self.pitch_offset)
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let target = self.frequency_for(f32::from(pitch));
        let start = if self.glide_coeff >= 1.0 {
            target
        } else {
            self.last_frequency.unwrap_or(target)
        };
        let stolen = assignment.stolen;
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.pitch = f32::from(pitch);
        voice.velocity = velocity;
        voice.target = target;
        voice.frequency = start;
        if !stolen {
            // Restarting the phase gives every note the same attack transient, and it is only
            // safe on a silent voice: doing it to one that is still sounding would step the
            // waveform and click.
            for oscillator in &mut voice.oscillators {
                oscillator.reset();
            }
            // And the vibrato with them, so a chord struck together wobbles together. A voice
            // stolen mid-note keeps its phase for the same reason its oscillators do.
            voice.vibrato.reset();
        }
        voice.envelope.trigger();
        self.last_frequency = Some(target);
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
        ParamDescriptor::new(P_WAVEFORM, "waveform", "Waveform", 0.0, 4.0, 1.0)
            .with_choices(Waveform::CHOICES),
        ParamDescriptor::new(P_PULSE_WIDTH, "pulse_width", "Pulse Width", 0.05, 0.95, 0.5)
            .with_unit(ParamUnit::Percent),
        ParamDescriptor::new(P_OCTAVE, "octave", "Octave", -3.0, 3.0, 0.0).with_steps(7),
        ParamDescriptor::new(P_DETUNE, "detune", "Detune", -1.0, 1.0, 0.0)
            .with_unit(ParamUnit::Semitones),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.0, 2.0, 0.005)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_DECAY, "decay", "Decay", 0.0, 4.0, 0.12)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::percent(P_SUSTAIN, "sustain", "Sustain", 0.7),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.0, 4.0, 0.15)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_GLIDE, "glide", "Glide", 0.0, 1.0, 0.0)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        // Beside Glide, because these are the other three controls that move the pitch.
        ParamDescriptor::new(
            P_VIBRATO_RATE,
            "vibrato_rate",
            "Vibrato Rate",
            0.1,
            12.0,
            5.5,
        )
        .with_unit(ParamUnit::Hertz),
        ParamDescriptor::new(P_VIBRATO, "vibrato", "Vibrato", 0.0, 2.0, 0.0)
            .with_unit(ParamUnit::Semitones)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(
            P_MOD_DEPTH,
            "mod_depth",
            "Mod Depth",
            0.0,
            2.0,
            DEFAULT_MOD_DEPTH,
        )
        .with_unit(ParamUnit::Semitones)
        .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(P_UNISON, "unison", "Unison", 1.0, MAX_UNISON as f32, 1.0)
            .with_steps(MAX_UNISON as u32),
        ParamDescriptor::new(P_SPREAD, "spread", "Unison Spread", 0.0, 0.5, 0.08)
            .with_unit(ParamUnit::Semitones),
        ParamDescriptor::new(P_BITS, "bits", "Bit Depth", 1.0, 16.0, 16.0).with_steps(16),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -6.0),
    ]
}

impl Parameterized for Chiptune {
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

impl SegmentRenderer for Chiptune {
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
                        // The de-click ramp, not a hard cut: an instant stop on a sounding
                        // voice is a step edge, and that is audible as a click.
                        voice.envelope.kill();
                    }
                }
                self.last_frequency = None;
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
                self.refresh();
            }
            NoteEvent::Modulation { amount, .. } => {
                self.modulation = finite_or(amount, 0.0).clamp(0.0, 1.0);
                self.refresh();
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

        let waveform = self.waveform;
        let glide_coeff = self.glide_coeff;
        let unison = self.unison;
        let ratios = self.unison_ratios;
        let voice_gain = self.unison_gain;
        let pitch_offset = self.pitch_offset;
        let vibrato_depth = self.vibrato_depth;

        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            voice.target = pitch_to_hz(voice.pitch + pitch_offset);
            let gain = voice_gain * voice.velocity;

            for sample in dst.iter_mut() {
                voice.frequency += (voice.target - voice.frequency) * glide_coeff;
                // The vibrato multiplies the glided frequency rather than joining the pitch
                // offset, so it is a wobble *around* wherever the note has glided to rather than
                // something the glide would chase and flatten out.
                let swing = match vibrato_depth > 0.0 {
                    true => (voice.vibrato.next() * vibrato_depth / 12.0).exp2(),
                    // Still advanced when it is silent, so turning the wheel up mid-note picks
                    // the cycle up where it would have been rather than jumping.
                    false => {
                        voice.vibrato.next();
                        1.0
                    }
                };
                let frequency = voice.frequency * swing;
                let envelope = voice.envelope.process();
                let mut mix = 0.0;
                for (oscillator, ratio) in
                    voice.oscillators.iter_mut().zip(ratios.iter()).take(unison)
                {
                    oscillator.set_frequency(frequency * ratio);
                    mix += oscillator.next(waveform);
                }
                *sample += mix * gain * envelope;
            }

            self.allocator
                .set_level(index, voice.envelope.level() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(index);
            }
        }

        // The crusher runs on the summed voices, the way the hardware's single output stage
        // did, so stacked notes quantise together instead of each keeping its own steps.
        if self.crush_levels > 0.0 {
            let levels = self.crush_levels;
            let inverse = 1.0 / levels;
            for sample in dst.iter_mut() {
                *sample = (*sample * levels).round() * inverse;
            }
        }

        let output_gain = self.output_gain;
        for sample in dst.iter_mut() {
            *sample *= output_gain;
        }
    }
}

impl Instrument for Chiptune {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Chiptune",
            "Band-limited pulse, saw, triangle and LFSR noise with unison and bit crushing",
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = if ctx.sample_rate > 0.0 {
            ctx.sample_rate as f32
        } else {
            48_000.0
        };
        self.voices.clear();
        self.voices.reserve(VOICE_COUNT);
        for index in 0..VOICE_COUNT {
            self.voices
                .push(ChiptuneVoice::new(index, self.sample_rate));
        }
        self.allocator.prepare(VOICE_COUNT);
        self.bend = 0.0;
        self.modulation = 0.0;
        self.last_frequency = None;
        self.refresh();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            for oscillator in &mut voice.oscillators {
                oscillator.reset();
            }
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.modulation = 0.0;
        self.last_frequency = None;
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
    use crate::test_support::{Rig, goertzel, peak, peak_db, zero_crossing_hz};
    use auris_core::param::gain_to_db;

    const SAMPLE_RATE: f64 = 48_000.0;

    fn sine_rig() -> Rig {
        let mut synth = Chiptune::new();
        synth.set_param_by_key("waveform", Waveform::Sine.index() as f32);
        synth.set_param_by_key("attack", 0.001);
        synth.set_param_by_key("decay", 0.001);
        synth.set_param_by_key("sustain", 1.0);
        synth.set_param_by_key("release", 0.05);
        synth.set_param_by_key("level", 0.0);
        Rig::new(Box::new(synth), SAMPLE_RATE, 512, 2)
    }

    fn note_on(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    #[test]
    fn descriptor_and_parameters_line_up() {
        let synth = Chiptune::new();
        assert_eq!(synth.descriptor().id, Chiptune::ID);
        assert_eq!(synth.descriptor().category, PluginCategory::Synth);
        for (index, descriptor) in synth.parameters().iter().enumerate() {
            assert_eq!(descriptor.id.index(), index, "id of `{}`", descriptor.key);
        }
        assert_eq!(synth.parameters().len(), 16);
    }

    #[test]
    fn a_note_at_midi_69_sounds_at_440_hz() {
        let mut rig = sine_rig();
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        // Skip the attack so the measurement runs on the steady part of the note.
        let steady = &rendered[2_048..];

        let measured = zero_crossing_hz(steady, SAMPLE_RATE);
        assert!(
            (measured / 440.0 - 1.0).abs() < 0.01,
            "zero crossings say {measured:.2} Hz"
        );

        // Cross-check with a DFT bin: 440 Hz must dominate the neighbouring semitones.
        let at_440 = goertzel(steady, SAMPLE_RATE, 440.0);
        let below = goertzel(steady, SAMPLE_RATE, 415.30);
        let above = goertzel(steady, SAMPLE_RATE, 466.16);
        assert!(
            at_440 > below * 10.0 && at_440 > above * 10.0,
            "440 Hz bin {at_440:.4}, neighbours {below:.4} / {above:.4}"
        );
    }

    #[test]
    fn the_wheel_opens_a_vibrato_and_nothing_opens_it_by_itself() {
        // Two claims, and both matter. A patch nobody has touched must sound exactly as it did
        // before any of this existed — otherwise every piece already written changed. And the
        // wheel must reach the pitch without a parameter being found first, or it is a control
        // nobody discovers.
        // Half a second of audio and a two-hertz swing, so the render covers a whole cycle —
        // eight slices of an eighth each, which is fine enough to catch both ends of it.
        let slice = 3_000;

        let mut still = sine_rig();
        let steady = still.render(24_000, &[note_on(0, 69)]);
        let plain: Vec<f64> = steady[512..]
            .chunks(slice)
            .map(|chunk| zero_crossing_hz(chunk, SAMPLE_RATE))
            .collect();
        for measured in &plain {
            assert!(
                (measured / 440.0 - 1.0).abs() < 0.02,
                "an untouched patch wobbled: {measured:.2} Hz"
            );
        }

        let mut wheeled = sine_rig();
        wheeled.set_param("vibrato_rate", 2.0);
        wheeled.set_param("mod_depth", 2.0);
        let rendered = wheeled.render(
            24_000,
            &[
                NoteEvent::Modulation {
                    frame: 0,
                    amount: 1.0,
                },
                note_on(0, 69),
            ],
        );
        let swung: Vec<f64> = rendered[512..]
            .chunks(slice)
            .map(|chunk| zero_crossing_hz(chunk, SAMPLE_RATE))
            .collect();
        let highest = swung.iter().cloned().fold(f64::MIN, f64::max);
        let lowest = swung.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            highest > 465.0 && lowest < 415.0,
            "the wheel should swing a whole tone either way: {swung:?}"
        );
        // And it comes back — the swing is *around* the note, not a detune away from it.
        let mean = swung.iter().sum::<f64>() / swung.len() as f64;
        assert!(
            (mean / 440.0 - 1.0).abs() < 0.03,
            "the vibrato is centred on {mean:.2} Hz rather than on the note"
        );
    }

    #[test]
    fn a_vibrato_written_into_the_patch_needs_no_wheel() {
        // The other half of the pair: `vibrato` is the depth this *sound* has, and it is there
        // whether or not anybody touches a controller.
        let mut rig = sine_rig();
        rig.set_param("vibrato_rate", 2.0);
        rig.set_param("vibrato", 2.0);
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        let swung: Vec<f64> = rendered[512..]
            .chunks(3_000)
            .map(|chunk| zero_crossing_hz(chunk, SAMPLE_RATE))
            .collect();
        let highest = swung.iter().cloned().fold(f64::MIN, f64::max);
        let lowest = swung.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            highest > 465.0 && lowest < 415.0,
            "no wobble without a wheel: {swung:?}"
        );
    }

    #[test]
    fn every_pitched_waveform_lands_on_the_same_fundamental() {
        // The 440 Hz check above runs on a sine, where a band-limiting mistake cannot show up.
        // Square, saw and triangle all have to land on the same note.
        for waveform in [Waveform::Square, Waveform::Saw, Waveform::Triangle] {
            let mut rig = sine_rig();
            rig.set_param("waveform", waveform.index() as f32);
            let rendered = rig.render(24_000, &[note_on(0, 69)]);
            let measured = zero_crossing_hz(&rendered[2_048..], SAMPLE_RATE);
            assert!(
                (measured / 440.0 - 1.0).abs() < 0.01,
                "{waveform:?} measured {measured:.2} Hz"
            );
        }
    }

    #[test]
    fn the_pulse_width_changes_the_harmonic_content() {
        // A 50 % square is odd-harmonic only; narrowing the pulse is what brings in the even
        // harmonics that give a chiptune lead its nasal character.
        fn second_harmonic(width: f32) -> f64 {
            let mut rig = sine_rig();
            rig.set_param("waveform", Waveform::Square.index() as f32);
            rig.set_param("pulse_width", width);
            let rendered = rig.render(24_000, &[note_on(0, 69)]);
            goertzel(&rendered[4_096..], SAMPLE_RATE, 880.0)
        }

        let symmetric = second_harmonic(0.5);
        let narrow = second_harmonic(0.25);
        assert!(
            symmetric < 0.01,
            "a 50 % square should have no second harmonic, found {symmetric:.4}"
        );
        assert!(
            narrow > symmetric * 10.0,
            "pulse width did not change the timbre: 50 % {symmetric:.4}, 25 % {narrow:.4}"
        );
    }

    #[test]
    fn every_octave_transposes_by_a_factor_of_two() {
        for (octave, expected) in [(-1.0, 220.0), (0.0, 440.0), (1.0, 880.0)] {
            let mut rig = sine_rig();
            rig.set_param("octave", octave);
            let rendered = rig.render(24_000, &[note_on(0, 69)]);
            let measured = zero_crossing_hz(&rendered[2_048..], SAMPLE_RATE);
            assert!(
                (measured / expected - 1.0).abs() < 0.01,
                "octave {octave} gave {measured:.2} Hz, wanted {expected}"
            );
        }
    }

    #[test]
    fn a_note_on_at_frame_100_leaves_the_head_of_the_block_silent() {
        let mut rig = sine_rig();
        let rendered = rig.render(512, &[note_on(100, 69)]);
        assert!(
            rendered[..100].iter().all(|s| *s == 0.0),
            "frames before the note on are not silent: peak {}",
            peak(&rendered[..100])
        );
        assert!(
            peak(&rendered[100..]) > 0.1,
            "the note did not start inside the block"
        );
        // The very first sample of the note is the start of the attack, not full level.
        assert!(rendered[100].abs() < 0.05, "attack was skipped");
    }

    #[test]
    fn events_land_on_their_own_frame_not_the_block_boundary() {
        // Two blocks' worth of frames with the note on late in the second block.
        let mut rig = sine_rig();
        let rendered = rig.render(1_024, &[note_on(700, 69)]);
        assert!(rendered[..700].iter().all(|s| *s == 0.0));
        assert!(peak(&rendered[700..]) > 0.1);
    }

    #[test]
    fn the_output_is_overwritten_not_added_to() {
        let mut rig = sine_rig();
        rig.prefill(1.0);
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0), "stale audio survived");

        // A silent block only proves the very first `fill`. Rendering real notes into a dirty
        // buffer has to come out bit-identical to rendering them into a clean one, which is what
        // catches a segment that forgets to clear the frames it is responsible for.
        let events = [note_on(0, 60), note_on(137, 67), note_on(400, 72)];
        let clean = sine_rig().render(512, &events);
        let mut dirty = sine_rig();
        dirty.prefill(0.9);
        assert_eq!(
            dirty.render(512, &events),
            clean,
            "the instrument added into the buffer instead of overwriting it"
        );
    }

    #[test]
    fn a_released_note_falls_below_minus_60_dbfs() {
        let mut rig = sine_rig();
        rig.set_param("release", 0.2);
        let events = [
            note_on(0, 69),
            NoteEvent::NoteOff {
                frame: 4_800,
                pitch: 69,
            },
        ];
        // 100 ms of note, then 200 ms of release, then look at what is left.
        let rendered = rig.render(4_800 + 9_600 + 4_800, &events);
        let tail = &rendered[4_800 + 9_600..];
        assert!(
            peak_db(tail) < -60.0,
            "tail peaked at {:.1} dBFS",
            peak_db(tail)
        );
        assert_eq!(rig.instrument.active_voices(), 0, "voice was not reclaimed");
    }

    #[test]
    fn active_voices_tracks_held_notes() {
        let mut rig = sine_rig();
        rig.render(64, &[note_on(0, 60), note_on(0, 64), note_on(0, 67)]);
        assert_eq!(rig.instrument.active_voices(), 3);
        rig.render(
            64,
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: 64,
            }],
        );
        // Still sounding out its release.
        assert_eq!(rig.instrument.active_voices(), 3);
        rig.render(48_000, &[]);
        assert_eq!(rig.instrument.active_voices(), 2);
    }

    #[test]
    fn one_of_two_overlapping_notes_on_a_pitch_survives_the_first_release() {
        // A pedal note with a second strike of the same pitch inside it. The release that ends
        // the inner note must leave the pedal sounding, which it only does if each note-off is
        // paired with one voice rather than with every voice at that pitch.
        let mut rig = sine_rig();
        rig.render(512, &[note_on(0, 60)]);
        rig.render(512, &[note_on(0, 60)]);
        assert_eq!(rig.instrument.active_voices(), 2);

        rig.render(
            512,
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: 60,
            }],
        );
        // Half a second is ten times the 50 ms release, so the voice that was let go has retired
        // and anything still audible belongs to the note that was not.
        let remaining = rig.render(24_000, &[]);
        assert_eq!(rig.instrument.active_voices(), 1);
        assert!(
            peak_db(&remaining[12_000..]) > -12.0,
            "the pedal note was cut short: {:.1} dBFS",
            peak_db(&remaining[12_000..])
        );

        // The second off ends it, and a third has nothing left to pair with.
        rig.render(
            512,
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: 60,
            }],
        );
        rig.render(24_000, &[]);
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn more_notes_than_the_pool_holds_stay_finite() {
        let mut rig = sine_rig();
        let events: Vec<NoteEvent> = (0..32).map(|i| note_on(i * 4, 40 + i as u8)).collect();
        let rendered = rig.render(4_096, &events);
        assert!(rendered.iter().all(|s| s.is_finite()), "output went NaN");
        assert!(
            peak(&rendered) < 16.0,
            "peak {} is implausible",
            peak(&rendered)
        );
        assert!(rig.instrument.active_voices() <= VOICE_COUNT);
        // Everything still works afterwards.
        let after = rig.render(4_096, &[note_on(0, 69)]);
        assert!(peak(&after) > 0.01);
    }

    #[test]
    fn all_notes_off_releases_and_all_sound_off_mutes() {
        let mut rig = sine_rig();
        rig.set_param("release", 0.5);
        rig.render(2_048, &[note_on(0, 60), note_on(0, 64)]);

        let released = rig.render(2_048, &[NoteEvent::AllNotesOff { frame: 0 }]);
        assert!(
            peak(&released) > 0.05,
            "all notes off should let the release ring, not cut it"
        );

        let muted = rig.render(2_048, &[NoteEvent::AllSoundOff { frame: 0 }]);
        // The de-click ramp is 2 ms = 96 frames at 48 kHz.
        assert!(
            peak(&muted[192..]) == 0.0,
            "all sound off left {} behind",
            peak(&muted[192..])
        );
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn all_sound_off_takes_effect_on_its_own_frame() {
        // Testing it only at frame 0 cannot tell a sample-accurate mute from one that silences
        // the whole block, which would swallow audio that was supposed to be heard.
        let mut rig = sine_rig();
        rig.render(2_048, &[note_on(0, 69)]);
        let rendered = rig.render(512, &[NoteEvent::AllSoundOff { frame: 200 }]);
        assert!(
            peak(&rendered[..200]) > 0.1,
            "the mute swallowed the frames before its own"
        );
        // The de-click ramp is 96 frames at 48 kHz, so it is over well inside the block.
        assert_eq!(
            peak(&rendered[400..]),
            0.0,
            "the mute did not reach silence"
        );
    }

    #[test]
    fn pitch_bend_transposes_sounding_notes() {
        let mut rig = sine_rig();
        rig.render(512, &[note_on(0, 69)]);
        let bent = rig.render(
            24_000,
            &[NoteEvent::PitchBend {
                frame: 0,
                semitones: 12.0,
            }],
        );
        let measured = zero_crossing_hz(&bent[512..], SAMPLE_RATE);
        assert!(
            (measured / 880.0 - 1.0).abs() < 0.01,
            "bent note measured {measured:.2} Hz"
        );
    }

    #[test]
    fn the_level_parameter_is_honoured_in_decibels() {
        let mut rig = sine_rig();
        rig.set_param("level", 0.0);
        let full = peak(&rig.render(9_600, &[note_on(0, 69)]));

        let mut rig = sine_rig();
        rig.set_param("level", -12.0);
        let quiet = peak(&rig.render(9_600, &[note_on(0, 69)]));

        let difference = gain_to_db(quiet / full);
        assert!(
            (difference + 12.0).abs() < 0.5,
            "level difference {difference:.2} dB"
        );
    }

    #[test]
    fn the_bit_crusher_quantises_the_output() {
        let mut rig = sine_rig();
        rig.set_param("level", 0.0);
        rig.set_param("bits", 3.0);
        let rendered = rig.render(9_600, &[note_on(0, 69)]);
        // Three bits give four steps either side of zero, i.e. multiples of 0.25.
        let distinct = rendered
            .iter()
            .filter(|s| s.is_finite())
            .map(|s| (s * 4.0).round() as i32)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(distinct.len() <= 9, "{} distinct levels", distinct.len());
        for sample in &rendered {
            let steps = sample * 4.0;
            assert!(
                (steps - steps.round()).abs() < 1e-4,
                "{sample} is not on the quantiser grid"
            );
        }
    }

    #[test]
    fn unison_detunes_without_changing_the_note() {
        let mut rig = sine_rig();
        rig.set_param("unison", 4.0);
        rig.set_param("spread", 0.2);
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        let steady = &rendered[4_096..];
        // The stack still centres on 440 Hz, and the detuned copies beat around it.
        let at_440 = goertzel(steady, SAMPLE_RATE, 440.0);
        let at_400 = goertzel(steady, SAMPLE_RATE, 400.0);
        assert!(at_440 > at_400 * 5.0, "440 {at_440:.4} vs 400 {at_400:.4}");
        assert!(rendered.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn glide_starts_the_new_note_at_the_old_pitch() {
        let mut rig = sine_rig();
        rig.set_param("glide", 1.0);
        // A short release keeps the note being left behind out of the measurement.
        rig.set_param("release", 0.001);
        rig.render(4_800, &[note_on(0, 69)]);
        rig.render(
            480,
            &[
                NoteEvent::NoteOff {
                    frame: 0,
                    pitch: 69,
                },
                note_on(1, 81),
            ],
        );
        // An octave up over a second: 10 to 30 ms in, the new note is still near 440 Hz and
        // nowhere near the 880 Hz it was asked for.
        let early = rig.render(1_024, &[]);
        let measured = zero_crossing_hz(&early, SAMPLE_RATE);
        assert!(
            (450.0..640.0).contains(&measured),
            "glide jumped straight to {measured:.1} Hz"
        );
        // Given time, it arrives.
        let late = rig.render(72_000, &[]);
        let arrived = zero_crossing_hz(&late[48_000..], SAMPLE_RATE);
        assert!(
            (arrived / 880.0 - 1.0).abs() < 0.02,
            "glide settled at {arrived:.1} Hz"
        );
    }

    #[test]
    fn no_parameter_extreme_produces_a_non_finite_sample() {
        let descriptors = descriptors();
        // The midpoint matters as much as the ends: it is the only one of the three that turns
        // the bit crusher on and stacks a partial unison.
        for extreme in [0.0f32, 0.5, 1.0] {
            for waveform in Waveform::ALL {
                let mut synth = Chiptune::new();
                for descriptor in &descriptors {
                    synth.set_param(descriptor.id, descriptor.denormalize(extreme));
                }
                synth.set_param(ParamId(P_WAVEFORM), waveform.index() as f32);
                let mut rig = Rig::new(Box::new(synth), SAMPLE_RATE, 512, 2);
                let events = [
                    note_on(0, 0),
                    note_on(1, 127),
                    note_on(2, 69),
                    NoteEvent::PitchBend {
                        frame: 3,
                        semitones: 240.0,
                    },
                    NoteEvent::NoteOff {
                        frame: 400,
                        pitch: 69,
                    },
                ];
                let rendered = rig.render(8_192, &events);
                assert!(
                    rendered.iter().all(|s| s.is_finite()),
                    "{waveform:?} at extreme {extreme} produced a non-finite sample"
                );
            }
        }
    }

    #[test]
    fn state_round_trips_through_keys() {
        let mut synth = Chiptune::new();
        synth.set_param_by_key("waveform", 2.0);
        synth.set_param_by_key("release", 1.25);
        synth.set_param_by_key("unison", 3.0);
        let state = synth.save_state();

        let mut restored = Chiptune::new();
        restored.load_state(&state);
        assert_eq!(restored.param_by_key("waveform"), Some(2.0));
        assert_eq!(restored.param_by_key("release"), Some(1.25));
        assert_eq!(restored.param_by_key("unison"), Some(3.0));
        assert_eq!(restored.waveform, Waveform::Saw);
    }

    #[test]
    fn reset_silences_everything() {
        let mut rig = sine_rig();
        rig.render(2_048, &[note_on(0, 69)]);
        rig.instrument.reset();
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0));
        assert_eq!(rig.instrument.active_voices(), 0);
    }
}
