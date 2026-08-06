//! The `auris.synth.fm2` instrument.

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, ParamValueCurve,
    Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::envelope::Adsr;
use crate::lfo::Lfo;
use crate::oscillator::{Oscillator, RADIANS_TO_CYCLES, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

/// Voices in the pool.
const VOICE_COUNT: usize = 8;

const P_RATIO: u32 = 0;
const P_INDEX: u32 = 1;
const P_MOD_DECAY: u32 = 2;
const P_ATTACK: u32 = 3;
const P_DECAY: u32 = 4;
const P_SUSTAIN: u32 = 5;
const P_RELEASE: u32 = 6;
const P_VIBRATO_RATE: u32 = 7;
const P_VIBRATO: u32 = 8;
const P_MOD_DEPTH: u32 = 9;
const P_LEVEL: u32 = 10;

/// Widest pitch bend followed, in semitones.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// Release time of the modulator envelope. It is only there so that a note off cannot leave the
/// modulator running; the decay is what shapes the timbre.
const MOD_RELEASE_SECONDS: f32 = 0.005;

/// Attack time of the modulator envelope.
///
/// It has to be non-zero. A stolen voice keeps its carrier phase — resetting it would click —
/// but its modulator envelope has usually already run down to Idle, and a zero attack makes the
/// envelope jump straight to full level on the first sample of the new note. That would move the
/// carrier's read phase by up to `index / TAU` cycles between two adjacent samples, which is the
/// same discontinuity the phase reset is skipped to avoid, arriving through the modulation path
/// instead. Ramping the depth over 2 ms spreads it out; that is well inside the bright transient
/// the modulator decay shapes, so the timbre is unchanged.
const MOD_ATTACK_SECONDS: f32 = 0.002;

/// Carrier and modulator for one note.
#[derive(Clone, Debug)]
struct Fm2Voice {
    carrier: Oscillator,
    modulator: Oscillator,
    envelope: Adsr,
    /// The vibrato, restarted at every note on so a chord wobbles together.
    vibrato: Lfo,
    /// Drives the modulation depth. Its sustain is zero, which is what gives 2-op FM its
    /// characteristic bright attack settling into a near-sine body.
    mod_envelope: Adsr,
    pitch: f32,
    velocity: f32,
}

impl Fm2Voice {
    fn new(sample_rate: f32) -> Self {
        let mut carrier = Oscillator::new();
        let mut modulator = Oscillator::new();
        carrier.set_sample_rate(sample_rate);
        modulator.set_sample_rate(sample_rate);
        let mut envelope = Adsr::new();
        let mut mod_envelope = Adsr::new();
        envelope.set_sample_rate(sample_rate);
        mod_envelope.set_sample_rate(sample_rate);
        let mut vibrato = Lfo::new();
        vibrato.set_sample_rate(sample_rate);
        Self {
            carrier,
            modulator,
            envelope,
            vibrato,
            mod_envelope,
            pitch: 69.0,
            velocity: 0.0,
        }
    }
}

/// A two-operator phase-modulation synth.
///
/// One sine modulates the phase of another. The modulator's frequency is a ratio of the
/// carrier's, so integer ratios give harmonic timbres — 1:1 is a brassy saw, 2:1 a hollow
/// square-ish tone — and non-integer ratios give bells and mallets. Modulation depth follows
/// its own decay, which is where the bright transient at the start of each note comes from.
///
/// It shares every primitive with [`Chiptune`](crate::Chiptune) — same oscillator, same
/// envelope, same voice pool — and none of its synthesis method, which is the point: a new
/// synthesis technique is a new `process`, not a new engine.
#[derive(Clone, Debug)]
pub struct Fm2 {
    params: ParamBank,
    voices: Vec<Fm2Voice>,
    allocator: VoiceAllocator,
    sample_rate: f32,

    ratio: f32,
    /// Modulation index converted from radians of peak phase deviation to cycles.
    index_cycles: f32,
    bend: f32,
    /// Most recent modulation, 0 to 1.
    modulation: f32,
    /// How far the vibrato swings right now, in semitones.
    vibrato_depth: f32,
    output_gain: f32,
}

impl Default for Fm2 {
    fn default() -> Self {
        Fm2::new()
    }
}

impl Fm2 {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.synth.fm2";

    /// Builds the instrument with a 2:1 ratio and a moderate modulation index.
    pub fn new() -> Self {
        let mut synth = Self {
            params: ParamBank::new(descriptors()),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            ratio: 2.0,
            index_cycles: 0.0,
            bend: 0.0,
            modulation: 0.0,
            vibrato_depth: 0.0,
            output_gain: 1.0,
        };
        synth.refresh();
        synth
    }

    fn refresh(&mut self) {
        self.ratio = self.params.at(P_RATIO);
        self.index_cycles = self.params.at(P_INDEX) * RADIANS_TO_CYCLES;
        self.output_gain = db_to_gain(self.params.at(P_LEVEL));
        self.vibrato_depth =
            self.params.at(P_VIBRATO) + self.params.at(P_MOD_DEPTH) * self.modulation;
        let vibrato_rate = self.params.at(P_VIBRATO_RATE);

        let (attack, decay) = (self.params.at(P_ATTACK), self.params.at(P_DECAY));
        let (sustain, release) = (self.params.at(P_SUSTAIN), self.params.at(P_RELEASE));
        let mod_decay = self.params.at(P_MOD_DECAY);
        for voice in &mut self.voices {
            voice.envelope.set_adsr(attack, decay, sustain, release);
            voice
                .mod_envelope
                .set_adsr(MOD_ATTACK_SECONDS, mod_decay, 0.0, MOD_RELEASE_SECONDS);
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
            // FM timbre depends on the phase relationship between the two operators, so a
            // reproducible note needs both to start together — safe only on a silent voice.
            voice.carrier.reset();
            voice.modulator.reset();
            voice.vibrato.reset();
        }
        voice.envelope.trigger();
        voice.mod_envelope.trigger();
    }

    fn note_off(&mut self, pitch: u8) {
        for index in self.allocator.note_off(pitch) {
            if let Some(voice) = self.voices.get_mut(index) {
                voice.envelope.release();
                voice.mod_envelope.release();
            }
        }
    }
}

fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_RATIO, "ratio", "Ratio", 0.25, 16.0, 2.0)
            .with_unit(ParamUnit::Ratio)
            .with_curve(ParamValueCurve::Logarithmic),
        ParamDescriptor::new(P_INDEX, "index", "Index", 0.0, 24.0, 5.0)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(P_MOD_DECAY, "mod_decay", "Mod Decay", 0.001, 4.0, 0.35)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.0, 2.0, 0.002)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_DECAY, "decay", "Decay", 0.0, 4.0, 0.35)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::percent(P_SUSTAIN, "sustain", "Sustain", 0.6),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.0, 4.0, 0.25)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
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
            crate::chiptune::DEFAULT_MOD_DEPTH,
        )
        .with_unit(ParamUnit::Semitones)
        .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -9.0),
    ]
}

impl Parameterized for Fm2 {
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

impl SegmentRenderer for Fm2 {
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
                        voice.mod_envelope.release();
                    }
                }
            }
            NoteEvent::AllSoundOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.kill();
                        voice.mod_envelope.silence();
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
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

        let ratio = self.ratio;
        let index_cycles = self.index_cycles;
        let bend = self.bend;
        let vibrato_depth = self.vibrato_depth;

        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            let frequency = pitch_to_hz(voice.pitch + bend);
            voice.carrier.set_frequency(frequency);
            voice.modulator.set_frequency(frequency * ratio);
            let gain = voice.velocity;

            for sample in dst.iter_mut() {
                // Both operators move together, so the ratio between them — which is the whole
                // of the timbre — is unchanged by the wobble.
                let swing = match vibrato_depth > 0.0 {
                    true => (voice.vibrato.next() * vibrato_depth / 12.0).exp2(),
                    false => {
                        voice.vibrato.next();
                        1.0
                    }
                };
                if swing != 1.0 {
                    voice.carrier.set_frequency(frequency * swing);
                    voice.modulator.set_frequency(frequency * swing * ratio);
                }
                let depth = voice.mod_envelope.process() * index_cycles;
                let offset = voice.modulator.next(Waveform::Sine) * depth;
                let carrier = voice.carrier.next_modulated(Waveform::Sine, offset);
                *sample += carrier * voice.envelope.process() * gain;
            }

            self.allocator
                .set_level(slot, voice.envelope.level() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(slot);
            }
        }

        let output_gain = self.output_gain;
        for sample in dst.iter_mut() {
            *sample *= output_gain;
        }
    }
}

impl Instrument for Fm2 {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "FM 2-Op",
            "Two-operator phase modulation: bells, basses and electric pianos",
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
        for _ in 0..VOICE_COUNT {
            self.voices.push(Fm2Voice::new(self.sample_rate));
        }
        self.allocator.prepare(VOICE_COUNT);
        self.bend = 0.0;
        self.modulation = 0.0;
        self.refresh();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            voice.mod_envelope.silence();
            voice.carrier.reset();
            voice.modulator.reset();
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.modulation = 0.0;
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
    use crate::test_support::{Rig, goertzel, max_step, peak, peak_db, zero_crossing_hz};

    const SAMPLE_RATE: f64 = 48_000.0;

    fn rig() -> Rig {
        let mut synth = Fm2::new();
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
        let synth = Fm2::new();
        assert_eq!(synth.descriptor().id, Fm2::ID);
        assert_eq!(synth.descriptor().category, PluginCategory::Synth);
        for (index, descriptor) in synth.parameters().iter().enumerate() {
            assert_eq!(descriptor.id.index(), index, "id of `{}`", descriptor.key);
        }
        assert_eq!(synth.parameters().len(), 11);
    }

    #[test]
    fn the_carrier_sounds_at_440_hz_for_midi_69() {
        let mut rig = rig();
        // With no modulation the carrier is a bare sine, so zero crossings are exact.
        rig.set_param("index", 0.0);
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        let measured = zero_crossing_hz(&rendered[2_048..], SAMPLE_RATE);
        assert!(
            (measured / 440.0 - 1.0).abs() < 0.01,
            "measured {measured:.2} Hz"
        );
        let at_440 = goertzel(&rendered[2_048..], SAMPLE_RATE, 440.0);
        assert!(at_440 > 0.5, "carrier amplitude {at_440:.3}");
    }

    #[test]
    fn modulation_puts_energy_in_the_sidebands() {
        // A 2:1 ratio at 440 Hz places sidebands at 440 +- n*880 Hz.
        let mut quiet = rig();
        quiet.set_param("index", 0.0);
        quiet.set_param("mod_decay", 4.0);
        let clean = quiet.render(24_000, &[note_on(0, 69)]);

        let mut loud = rig();
        loud.set_param("index", 2.0);
        loud.set_param("mod_decay", 4.0);
        let bright = loud.render(24_000, &[note_on(0, 69)]);

        let window = 4_096..20_000;
        let clean_sideband = goertzel(&clean[window.clone()], SAMPLE_RATE, 1_320.0);
        let bright_sideband = goertzel(&bright[window], SAMPLE_RATE, 1_320.0);
        assert!(
            clean_sideband < 0.01,
            "unmodulated sideband {clean_sideband:.4}"
        );
        assert!(
            bright_sideband > 0.1,
            "modulated sideband {bright_sideband:.4}"
        );
    }

    #[test]
    fn the_ratio_parameter_places_the_sidebands() {
        let mut rig = rig();
        rig.set_param("ratio", 3.0);
        rig.set_param("index", 2.0);
        rig.set_param("mod_decay", 4.0);
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        let steady = &rendered[4_096..];
        // A 3:1 ratio puts sidebands at 440 +- 1320, so 1760 Hz is populated and 1320 is not.
        let at_1760 = goertzel(steady, SAMPLE_RATE, 1_760.0);
        let at_1320 = goertzel(steady, SAMPLE_RATE, 1_320.0);
        assert!(
            at_1760 > at_1320 * 5.0,
            "1760 Hz {at_1760:.4} vs 1320 Hz {at_1320:.4}"
        );
    }

    #[test]
    fn the_modulator_envelope_darkens_the_tail() {
        let mut rig = rig();
        rig.set_param("index", 2.0);
        rig.set_param("mod_decay", 0.05);
        let rendered = rig.render(24_000, &[note_on(0, 69)]);
        let attack = goertzel(&rendered[..2_400], SAMPLE_RATE, 1_320.0);
        let tail = goertzel(&rendered[12_000..], SAMPLE_RATE, 1_320.0);
        assert!(
            tail < attack * 0.2,
            "sideband barely moved: attack {attack:.4}, tail {tail:.4}"
        );
    }

    #[test]
    fn a_note_on_at_frame_100_leaves_the_head_of_the_block_silent() {
        let mut rig = rig();
        let rendered = rig.render(512, &[note_on(100, 69)]);
        assert!(
            rendered[..100].iter().all(|s| *s == 0.0),
            "frames before the note on are not silent: peak {}",
            peak(&rendered[..100])
        );
        assert!(peak(&rendered[100..]) > 0.1);
    }

    #[test]
    fn the_output_is_overwritten_not_added_to() {
        let mut rig = rig();
        rig.prefill(-0.5);
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0));

        // Sounding notes into a dirty buffer must match a clean render exactly, so that every
        // segment is proved to clear the frames it owns rather than only the first one.
        let events = [note_on(0, 60), note_on(137, 67)];
        let clean = self::rig().render(512, &events);
        let mut dirty = self::rig();
        dirty.prefill(0.9);
        assert_eq!(
            dirty.render(512, &events),
            clean,
            "the instrument added into the buffer instead of overwriting it"
        );
    }

    #[test]
    fn a_released_note_falls_below_minus_60_dbfs() {
        let mut rig = rig();
        rig.set_param("release", 0.2);
        let events = [
            note_on(0, 69),
            NoteEvent::NoteOff {
                frame: 4_800,
                pitch: 69,
            },
        ];
        let rendered = rig.render(4_800 + 9_600 + 4_800, &events);
        let tail = &rendered[4_800 + 9_600..];
        assert!(
            peak_db(tail) < -60.0,
            "tail peaked at {:.1} dBFS",
            peak_db(tail)
        );
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn more_notes_than_the_pool_holds_stay_finite() {
        let mut rig = rig();
        let events: Vec<NoteEvent> = (0..32).map(|i| note_on(i * 4, 40 + i as u8)).collect();
        let rendered = rig.render(4_096, &events);
        assert!(rendered.iter().all(|s| s.is_finite()));
        assert!(peak(&rendered) < 16.0, "peak {}", peak(&rendered));
        assert!(rig.instrument.active_voices() <= VOICE_COUNT);
    }

    #[test]
    fn stealing_a_voice_does_not_step_the_modulation_depth() {
        let mut rig = rig();
        // The threshold below is calibrated against how steep a fully modulated carrier legally
        // is, which scales with the index, so the index is pinned rather than inherited from the
        // parameter default — moving that default must not silently retune this test.
        rig.set_param("index", 5.0);
        // Fill the pool so the next note on has to take a voice that is sounding at full level.
        let held: Vec<NoteEvent> = (0..VOICE_COUNT)
            .map(|slot| note_on(0, 48 + slot as u8 * 2))
            .collect();
        rig.render(512, &held);
        // Run every modulator envelope down to Idle, which is the state a steal re-triggers
        // from and the state in which a stepped depth is at its most violent.
        rig.render(48_000, &[]);

        let quiescent = rig.render(512, &[]);
        // A low note keeps the honest slope of a fully modulated voice — which rises with the
        // modulator frequency — near that of the quiescent block, so a discontinuity is the only
        // thing that can make the steal block measurably steeper.
        let stolen = rig.render(512, &[note_on(0, 24)]);

        // The steal takes effect on the first sample of the second block, so the step to look
        // for straddles the block boundary and the boundary has to be measured too.
        let mut across = Vec::with_capacity(stolen.len() + 1);
        across.push(*quiescent.last().expect("block is not empty"));
        across.extend_from_slice(&stolen);

        let before = max_step(&quiescent);
        let after = max_step(&across);
        assert!(
            after <= before * 2.0,
            "the steal stepped by {after} against a quiescent worst of {before}"
        );
    }

    #[test]
    fn a_stolen_voice_is_still_modulated() {
        // Silencing the modulator on a steal would remove the click too, so this pins the other
        // half of the contract: a stolen note has to sound like an FM note, not a bare sine.
        fn steal_onto_a_silent_pool(index: f32) -> (Vec<f32>, Vec<f32>) {
            let mut rig = rig();
            rig.set_param("index", index);
            // Velocity zero fills the pool without contributing anything to the output, so what
            // comes back after the steal is the stolen voice alone and can be measured directly.
            let held: Vec<NoteEvent> = (0..VOICE_COUNT)
                .map(|slot| NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 48 + slot as u8 * 2,
                    velocity: 0.0,
                })
                .collect();
            rig.render(512, &held);
            // Long enough for every modulator envelope to reach Idle, which is the state a steal
            // restarts from and the one in which a skipped trigger leaves the note unmodulated.
            let quiet = rig.render(24_000, &[]);
            (quiet, rig.render(2_048, &[note_on(0, 24)]))
        }

        let (quiet, modulated) = steal_onto_a_silent_pool(5.0);
        assert_eq!(peak(&quiet), 0.0, "the placeholder voices are not silent");
        let (_, bare) = steal_onto_a_silent_pool(0.0);
        // A modulated carrier swings its instantaneous frequency well above the note's own, so
        // it is far steeper sample to sample than the sine that note would be without it.
        let (modulated_step, bare_step) = (max_step(&modulated), max_step(&bare));
        assert!(
            modulated_step > bare_step * 4.0,
            "the stolen note is barely modulated: {modulated_step} against {bare_step} unmodulated"
        );
    }

    #[test]
    fn pitch_bend_transposes_sounding_notes() {
        let mut rig = rig();
        rig.set_param("index", 0.0);
        rig.render(512, &[note_on(0, 69)]);
        let bent = rig.render(
            24_000,
            &[NoteEvent::PitchBend {
                frame: 0,
                semitones: -12.0,
            }],
        );
        let measured = zero_crossing_hz(&bent[512..], SAMPLE_RATE);
        assert!(
            (measured / 220.0 - 1.0).abs() < 0.01,
            "bent note measured {measured:.2} Hz"
        );
    }

    #[test]
    fn all_sound_off_mutes_within_the_declick_ramp() {
        let mut rig = rig();
        rig.render(2_048, &[note_on(0, 60)]);
        let muted = rig.render(2_048, &[NoteEvent::AllSoundOff { frame: 0 }]);
        assert_eq!(peak(&muted[192..]), 0.0);
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn no_parameter_extreme_produces_a_non_finite_sample() {
        let descriptors = descriptors();
        for extreme in [0.0f32, 0.5, 1.0] {
            let mut synth = Fm2::new();
            for descriptor in &descriptors {
                synth.set_param(descriptor.id, descriptor.denormalize(extreme));
            }
            let mut rig = Rig::new(Box::new(synth), SAMPLE_RATE, 512, 2);
            let events = [
                note_on(0, 0),
                note_on(1, 127),
                NoteEvent::PitchBend {
                    frame: 2,
                    semitones: 240.0,
                },
                NoteEvent::NoteOff {
                    frame: 400,
                    pitch: 127,
                },
            ];
            let rendered = rig.render(8_192, &events);
            assert!(
                rendered.iter().all(|s| s.is_finite()),
                "extreme {extreme} produced a non-finite sample"
            );
        }
    }

    #[test]
    fn reset_silences_everything() {
        let mut rig = rig();
        rig.render(2_048, &[note_on(0, 69)]);
        rig.instrument.reset();
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0));
        assert_eq!(rig.instrument.active_voices(), 0);
    }
}
