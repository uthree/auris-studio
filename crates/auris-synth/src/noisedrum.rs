//! The `auris.synth.noisedrum` instrument.

use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, ParamValueCurve,
    Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::envelope::Adsr;
use crate::oscillator::{Oscillator, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

/// Voices in the pool. Drum parts overlap far less than melodic ones, but a ringing crash under
/// a fast hat pattern still needs a few.
const VOICE_COUNT: usize = 8;

const P_TONE: u32 = 0;
const P_DECAY: u32 = 1;
const P_SWEEP: u32 = 2;
const P_LEVEL: u32 = 3;

/// Octaves the band-pass sweeps down through at a sweep amount of 1.
///
/// Four octaves is what turns the same noise burst from a hat into a kick: the ear reads a fast
/// downward pitch move as a drum head losing tension.
const SWEEP_OCTAVES: f32 = 4.0;

/// The pitch sweep finishes in this fraction of the amplitude decay, so the thump lands early
/// and the rest of the note is body rather than a slide.
const SWEEP_DECAY_FRACTION: f32 = 0.35;

/// MIDI note the tone parameter is stated at; other notes transpose it.
const REFERENCE_PITCH: f32 = 60.0;

/// Widest pitch bend followed, in semitones.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// Attack of the amplitude envelope. A hard start would be a step edge with energy right across
/// the band; 1 ms is short enough to still read as a hit.
const ATTACK_SECONDS: f32 = 0.001;

/// Resonance of the band-pass, as `1/Q`. `Q = 2` is broad enough to keep the noise sounding
/// like noise while still giving the sweep an audible pitch.
const FILTER_DAMPING: f32 = 0.5;

/// Highest centre frequency as a fraction of the sample rate.
///
/// The Chamberlin topology below is stable while `2*sin(pi*f/fs) < 2 - damping`; a fifth of the
/// sample rate keeps a comfortable margin from that bound.
const MAX_CUTOFF_RATIO: f32 = 0.2;

/// A Chamberlin state-variable filter, used here for its band-pass output.
///
/// It is the right filter for a swept resonant band because its coefficient is a single sine of
/// the cutoff, so retuning it every sample costs one transcendental and needs no coefficient
/// history — a biquad would have to be recomputed and would zipper as it moved.
#[derive(Clone, Debug, Default)]
struct StateVariableFilter {
    low: f32,
    band: f32,
}

impl StateVariableFilter {
    fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }

    /// Runs one sample through the filter at `cutoff_ratio` (cutoff over sample rate) and
    /// returns the band-pass output, normalised so the peak gain is about 1.
    fn process(&mut self, input: f32, cutoff_ratio: f32) -> f32 {
        let ratio = if cutoff_ratio.is_finite() {
            cutoff_ratio.clamp(0.0, MAX_CUTOFF_RATIO)
        } else {
            0.0
        };
        let f = 2.0 * (std::f32::consts::PI * ratio).sin();
        self.low += f * self.band;
        let high = input - self.low - FILTER_DAMPING * self.band;
        self.band += f * high;
        self.band * FILTER_DAMPING
    }

    fn is_finite(&self) -> bool {
        self.low.is_finite() && self.band.is_finite()
    }
}

/// One drum hit.
#[derive(Clone, Debug)]
struct DrumVoice {
    oscillator: Oscillator,
    filter: StateVariableFilter,
    amplitude: Adsr,
    /// Drives the downward frequency sweep; decays much faster than the amplitude.
    sweep: Adsr,
    /// MIDI note the hit was played at, which transposes the tone.
    note: f32,
    velocity: f32,
}

impl DrumVoice {
    fn new(index: usize, sample_rate: f32) -> Self {
        let mut oscillator = Oscillator::with_seed(Oscillator::seed_for_voice(index));
        oscillator.set_sample_rate(sample_rate);
        let mut amplitude = Adsr::new();
        let mut sweep = Adsr::new();
        amplitude.set_sample_rate(sample_rate);
        sweep.set_sample_rate(sample_rate);
        Self {
            oscillator,
            filter: StateVariableFilter::default(),
            amplitude,
            sweep,
            note: REFERENCE_PITCH,
            velocity: 0.0,
        }
    }
}

/// A one-shot noise drum: LFSR noise through a pitch-swept band-pass.
///
/// Tuned low with a long sweep it is a kick; low with a short decay, a tom; high and short, a
/// hat; mid with a wide filter, a snare. That covers a usable drum kit without a sampler and
/// without a single audio file in the project.
///
/// Note offs deliberately do not stop a hit: a drum is a one-shot, and cutting it when the key
/// lifts would make the sound depend on how long a step-sequencer note happened to be.
#[derive(Clone, Debug)]
pub struct NoiseDrum {
    params: ParamBank,
    voices: Vec<DrumVoice>,
    allocator: VoiceAllocator,
    sample_rate: f32,
    inv_sample_rate: f32,

    tone: f32,
    sweep_octaves: f32,
    bend: f32,
    output_gain: f32,
}

impl Default for NoiseDrum {
    fn default() -> Self {
        NoiseDrum::new()
    }
}

impl NoiseDrum {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.synth.noisedrum";

    /// Builds the instrument with a mid-tuned, medium-decay hit.
    pub fn new() -> Self {
        let mut drum = Self {
            params: ParamBank::new(descriptors()),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            inv_sample_rate: 1.0 / 48_000.0,
            tone: 220.0,
            sweep_octaves: 0.0,
            bend: 0.0,
            output_gain: 1.0,
        };
        drum.refresh();
        drum
    }

    fn refresh(&mut self) {
        self.tone = self.params.at(P_TONE);
        self.sweep_octaves = self.params.at(P_SWEEP) * SWEEP_OCTAVES;
        self.output_gain = db_to_gain(self.params.at(P_LEVEL));

        let decay = self.params.at(P_DECAY);
        for voice in &mut self.voices {
            voice.amplitude.set_adsr(ATTACK_SECONDS, decay, 0.0, decay);
            voice
                .sweep
                .set_adsr(0.0, decay * SWEEP_DECAY_FRACTION, 0.0, decay);
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.note = f32::from(pitch);
        voice.velocity = velocity;
        voice.oscillator.reset();
        voice.filter.reset();
        voice.amplitude.trigger();
        voice.sweep.trigger();
    }
}

fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::hertz(P_TONE, "tone", "Tone", 40.0, 8_000.0, 220.0),
        ParamDescriptor::new(P_DECAY, "decay", "Decay", 0.01, 2.0, 0.25)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::percent(P_SWEEP, "sweep", "Pitch Sweep", 0.6),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -6.0),
    ]
}

impl Parameterized for NoiseDrum {
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

impl SegmentRenderer for NoiseDrum {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            // A hit rings for its decay whatever the key does; the allocator still marks the
            // voice released so it is the first one stolen.
            NoteEvent::NoteOff { pitch, .. } => {
                self.allocator.note_off(pitch);
            }
            NoteEvent::AllNotesOff { .. } => {
                self.allocator.release_all();
            }
            NoteEvent::AllSoundOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.amplitude.kill();
                        voice.sweep.silence();
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
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

        let sweep_octaves = self.sweep_octaves;
        let inv_sample_rate = self.inv_sample_rate;
        let pitch_offset = self.bend - REFERENCE_PITCH;
        let tone = self.tone;

        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if !voice.amplitude.is_active() {
                continue;
            }
            // The tone is stated at middle C; the note and any bend transpose the whole hit.
            let base = tone * ((voice.note + pitch_offset) / 12.0).exp2();
            let gain = voice.velocity;
            for sample in dst.iter_mut() {
                let sweep = voice.sweep.process();
                let centre = base * (sweep_octaves * sweep).exp2();
                // The noise clock tracks the band so a low hit rumbles and a high one hisses.
                voice.oscillator.set_frequency(centre);
                let noise = voice.oscillator.next(Waveform::Noise);
                let filtered = voice.filter.process(noise, centre * inv_sample_rate);
                *sample += filtered * voice.amplitude.process() * gain;
            }

            // Checked once per segment rather than per sample: the filter is unconditionally
            // stable at these coefficients, so this only catches a state that arrived broken.
            if !voice.filter.is_finite() {
                voice.filter.reset();
            }
            self.allocator
                .set_level(slot, voice.amplitude.level() * voice.velocity);
            if voice.amplitude.is_finished() {
                self.allocator.retire(slot);
            }
        }

        let output_gain = self.output_gain;
        for sample in dst.iter_mut() {
            *sample *= output_gain;
        }
    }
}

impl Instrument for NoiseDrum {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Noise Drum",
            "Pitch-swept band-passed LFSR noise: kicks, snares and hats without a sampler",
            PluginCategory::Drum,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = if ctx.sample_rate > 0.0 {
            ctx.sample_rate as f32
        } else {
            48_000.0
        };
        self.inv_sample_rate = 1.0 / self.sample_rate;
        self.voices.clear();
        self.voices.reserve(VOICE_COUNT);
        for index in 0..VOICE_COUNT {
            self.voices.push(DrumVoice::new(index, self.sample_rate));
        }
        self.allocator.prepare(VOICE_COUNT);
        self.bend = 0.0;
        self.refresh();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.amplitude.silence();
            voice.sweep.silence();
            voice.filter.reset();
            voice.oscillator.reset();
        }
        self.allocator.clear();
        self.bend = 0.0;
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
    use crate::test_support::{Rig, band_amplitude, peak, peak_db, rms};

    const SAMPLE_RATE: f64 = 48_000.0;

    fn rig() -> Rig {
        let mut drum = NoiseDrum::new();
        drum.set_param_by_key("level", 0.0);
        Rig::new(Box::new(drum), SAMPLE_RATE, 512, 2)
    }

    fn hit(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    #[test]
    fn descriptor_and_parameters_line_up() {
        let drum = NoiseDrum::new();
        assert_eq!(drum.descriptor().id, NoiseDrum::ID);
        assert_eq!(drum.descriptor().category, PluginCategory::Drum);
        for (index, descriptor) in drum.parameters().iter().enumerate() {
            assert_eq!(descriptor.id.index(), index, "id of `{}`", descriptor.key);
        }
        assert_eq!(drum.parameters().len(), 4);
    }

    #[test]
    fn a_note_on_at_frame_100_leaves_the_head_of_the_block_silent() {
        let mut rig = rig();
        let rendered = rig.render(512, &[hit(100, 60)]);
        assert!(
            rendered[..100].iter().all(|s| *s == 0.0),
            "frames before the hit are not silent: peak {}",
            peak(&rendered[..100])
        );
        assert!(peak(&rendered[100..]) > 0.01, "the hit did not sound");
    }

    #[test]
    fn the_output_is_overwritten_not_added_to() {
        let mut rig = rig();
        rig.prefill(0.75);
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0));

        // Hits rendered into a dirty buffer must match a clean render exactly, which proves
        // every segment clears the frames it owns and not just the first one.
        let events = [hit(0, 48), hit(137, 60)];
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
    fn a_hit_decays_below_minus_60_dbfs_within_its_decay_time() {
        let mut rig = rig();
        rig.set_param("decay", 0.2);
        // 200 ms of decay, then look at the following 100 ms.
        let rendered = rig.render(9_600 + 4_800, &[hit(0, 60)]);
        assert!(peak(&rendered[..9_600]) > 0.05, "the hit was too quiet");
        let tail = &rendered[9_600..];
        assert!(
            peak_db(tail) < -60.0,
            "tail peaked at {:.1} dBFS",
            peak_db(tail)
        );
        assert_eq!(rig.instrument.active_voices(), 0, "voice was not reclaimed");
    }

    #[test]
    fn the_band_pass_puts_the_energy_around_the_tone() {
        let mut rig = rig();
        rig.set_param("tone", 400.0);
        rig.set_param("sweep", 0.0);
        rig.set_param("decay", 1.0);
        let rendered = rig.render(24_000, &[hit(0, 60)]);
        let steady = &rendered[2_400..];
        let in_band = band_amplitude(steady, SAMPLE_RATE, 400.0);
        let above = band_amplitude(steady, SAMPLE_RATE, 4_000.0);
        let below = band_amplitude(steady, SAMPLE_RATE, 40.0);
        assert!(
            in_band > above * 4.0,
            "400 Hz {in_band:.5} vs 4 kHz {above:.5}"
        );
        assert!(
            in_band > below * 4.0,
            "400 Hz {in_band:.5} vs 40 Hz {below:.5}"
        );
    }

    #[test]
    fn the_pitch_sweep_starts_the_hit_higher_than_it_ends() {
        let mut rig = rig();
        rig.set_param("tone", 100.0);
        rig.set_param("sweep", 1.0);
        rig.set_param("decay", 0.5);
        let rendered = rig.render(24_000, &[hit(0, 60)]);
        // The sweep starts four octaves up, at 1600 Hz, and lands on 100 Hz.
        let early = &rendered[..1_024];
        let late = &rendered[12_000..];
        let early_high = band_amplitude(early, SAMPLE_RATE, 1_600.0);
        let late_high = band_amplitude(late, SAMPLE_RATE, 1_600.0);
        assert!(
            early_high > late_high * 4.0,
            "1.6 kHz energy: start {early_high:.5}, end {late_high:.5}"
        );

        // Without the sweep the same hit starts at 100 Hz and never reaches up there.
        let mut flat = super::tests::rig();
        flat.set_param("tone", 100.0);
        flat.set_param("sweep", 0.0);
        flat.set_param("decay", 0.5);
        let unswept = flat.render(1_024, &[hit(0, 60)]);
        assert!(
            early_high > band_amplitude(&unswept, SAMPLE_RATE, 1_600.0) * 2.0,
            "the sweep did not raise the start of the hit"
        );
    }

    #[test]
    fn the_note_transposes_the_hit() {
        let mut low = rig();
        low.set_param("tone", 200.0);
        low.set_param("sweep", 0.0);
        low.set_param("decay", 1.0);
        let low_hit = low.render(24_000, &[hit(0, 60)]);

        let mut high = rig();
        high.set_param("tone", 200.0);
        high.set_param("sweep", 0.0);
        high.set_param("decay", 1.0);
        // One octave up the same tone should sit at 400 Hz.
        let high_hit = high.render(24_000, &[hit(0, 72)]);

        let steady = 2_400..;
        let low_at_200 = band_amplitude(&low_hit[steady.clone()], SAMPLE_RATE, 200.0);
        let low_at_400 = band_amplitude(&low_hit[steady.clone()], SAMPLE_RATE, 400.0);
        let high_at_200 = band_amplitude(&high_hit[steady.clone()], SAMPLE_RATE, 200.0);
        let high_at_400 = band_amplitude(&high_hit[steady], SAMPLE_RATE, 400.0);
        assert!(
            low_at_200 > low_at_400 * 2.0,
            "the untransposed hit is not centred on its tone: {low_at_200:.5} / {low_at_400:.5}"
        );
        assert!(
            high_at_400 > high_at_200 * 2.0,
            "an octave up should move the band to 400 Hz: {high_at_400:.5} / {high_at_200:.5}"
        );
    }

    #[test]
    fn pitch_bend_transposes_a_sounding_hit() {
        let mut rig = rig();
        rig.set_param("tone", 200.0);
        rig.set_param("sweep", 0.0);
        rig.set_param("decay", 2.0);
        rig.render(2_400, &[hit(0, 60)]);
        let bent = rig.render(
            24_000,
            &[NoteEvent::PitchBend {
                frame: 0,
                semitones: 12.0,
            }],
        );
        let at_400 = band_amplitude(&bent, SAMPLE_RATE, 400.0);
        let at_200 = band_amplitude(&bent, SAMPLE_RATE, 200.0);
        assert!(
            at_400 > at_200 * 2.0,
            "bend did not move the band: 400 Hz {at_400:.5} vs 200 Hz {at_200:.5}"
        );
    }

    #[test]
    fn note_off_does_not_cut_the_hit_short() {
        let mut rig = rig();
        rig.set_param("decay", 0.5);
        let events = [
            hit(0, 60),
            NoteEvent::NoteOff {
                frame: 480,
                pitch: 60,
            },
        ];
        let rendered = rig.render(9_600, &events);
        // 100 ms after a note off that landed at 10 ms, the hit is still going.
        assert!(
            rms(&rendered[4_800..9_600]) > 0.001,
            "the hit stopped with the key: rms {}",
            rms(&rendered[4_800..9_600])
        );
    }

    #[test]
    fn all_notes_off_lets_the_hit_ring_but_still_frees_the_voice() {
        // A one-shot has nothing to release, so an all-notes-off must not cut it. The voice
        // still has to be reclaimed once the hit has decayed, or the pool would leak.
        let mut rig = rig();
        rig.set_param("decay", 0.5);
        rig.render(2_400, &[hit(0, 60)]);
        let ringing = rig.render(2_400, &[NoteEvent::AllNotesOff { frame: 0 }]);
        assert!(
            rms(&ringing) > 0.001,
            "all notes off cut the hit short: rms {}",
            rms(&ringing)
        );
        rig.render(48_000, &[]);
        assert_eq!(rig.instrument.active_voices(), 0, "the voice leaked");
    }

    #[test]
    fn more_hits_than_the_pool_holds_stay_finite() {
        let mut rig = rig();
        let events: Vec<NoteEvent> = (0..32).map(|i| hit(i * 8, 36 + i as u8)).collect();
        let rendered = rig.render(4_096, &events);
        assert!(rendered.iter().all(|s| s.is_finite()), "output went NaN");
        assert!(peak(&rendered) < 16.0, "peak {}", peak(&rendered));
        assert!(rig.instrument.active_voices() <= VOICE_COUNT);
    }

    #[test]
    fn all_sound_off_mutes_within_the_declick_ramp() {
        let mut rig = rig();
        rig.set_param("decay", 2.0);
        rig.render(2_048, &[hit(0, 60)]);
        let muted = rig.render(2_048, &[NoteEvent::AllSoundOff { frame: 0 }]);
        assert_eq!(peak(&muted[192..]), 0.0);
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn no_parameter_extreme_produces_a_non_finite_sample() {
        let descriptors = descriptors();
        for extreme in [0.0f32, 0.5, 1.0] {
            let mut drum = NoiseDrum::new();
            for descriptor in &descriptors {
                drum.set_param(descriptor.id, descriptor.denormalize(extreme));
            }
            let mut rig = Rig::new(Box::new(drum), SAMPLE_RATE, 512, 2);
            let events = [hit(0, 0), hit(1, 127), hit(2, 60)];
            let rendered = rig.render(16_384, &events);
            assert!(
                rendered.iter().all(|s| s.is_finite()),
                "extreme {extreme} produced a non-finite sample"
            );
            assert!(
                peak(&rendered) < 8.0,
                "extreme {extreme} peaked at {}",
                peak(&rendered)
            );
        }
    }

    #[test]
    fn the_filter_stays_stable_at_the_top_of_its_range() {
        let mut filter = StateVariableFilter::default();
        let mut oscillator = Oscillator::new();
        oscillator.set_sample_rate(48_000.0);
        oscillator.set_frequency(10_000.0);
        let mut worst = 0.0f32;
        for _ in 0..48_000 {
            // Ask for a cutoff well past the clamp to prove the clamp is what holds it.
            let out = filter.process(oscillator.next(Waveform::Noise), 0.9);
            worst = worst.max(out.abs());
        }
        assert!(filter.is_finite());
        assert!(worst < 8.0, "band-pass rang up to {worst}");
    }

    #[test]
    fn reset_silences_everything() {
        let mut rig = rig();
        rig.render(2_048, &[hit(0, 60)]);
        rig.instrument.reset();
        let rendered = rig.render(512, &[]);
        assert!(rendered.iter().all(|s| *s == 0.0));
        assert_eq!(rig.instrument.active_voices(), 0);
    }
}
