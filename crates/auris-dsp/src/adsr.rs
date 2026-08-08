//! Amplitude envelopes.
//!
//! [`Adsr`] is a sample-accurate attack/decay/sustain/release generator. The attack is a
//! straight line; decay and release are exponential, because a level that falls by a constant
//! *ratio* per second is what the ear hears as an even fade, while a straight line down sounds
//! like it stops abruptly at the end.
//!
//! An exponential never actually reaches its target, so a stated time has to mean something
//! else. Here "decay time" and "release time" are the time to come within -80 dBFS of
//! the target, at which point the envelope snaps to it. That definition is what makes the
//! release testable: one release time after a note off the envelope is at or below -80 dB of
//! the level it started from, and the voice is free.
//!
//! It sits here rather than with the instruments that started it because two crates now shape a
//! note with it: `auris-synth` gives every built-in voice one, and `auris-sampler` puts one over
//! whatever envelope a SoundFont brought with it. One definition of what an attack of five
//! milliseconds sounds like is the whole point of the move.

/// Level at or below which a fading envelope is treated as having arrived: -80 dBFS, quiet
/// enough to be inaudible under anything else in a mix.
pub const SILENCE_LEVEL: f32 = 1.0e-4;

/// Time constants an exponential segment runs for.
///
/// Reaching [`SILENCE_LEVEL`] takes `ln(1/1e-4) = 9.2103` of them; the extra 3 % here means the
/// arrival test has already fired by the time the stated duration is up, instead of landing
/// exactly on the boundary where rounding decides the outcome.
pub const SEGMENT_TIME_CONSTANTS: f32 = 9.4907;

/// Length of the de-click ramp used when a voice is force-silenced.
///
/// Cutting a sounding voice to zero in one sample injects a step whose spectrum covers the
/// whole band — an audible click. Ramping over 2 ms pushes that energy below about 500 Hz and
/// down by more than 40 dB, and still frees the voice inside a single 512-frame block at any
/// supported sample rate.
///
/// Voice *stealing* does not use this ramp: a stolen voice is re-triggered from its current
/// level by [`Adsr::trigger`], which is already continuous and does not delay the new note by
/// the length of a ramp.
const KILL_SECONDS: f32 = 0.002;

/// Which segment an [`Adsr`] is currently in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EnvelopeStage {
    /// Silent and available for reuse.
    #[default]
    Idle,
    /// Rising to full level.
    Attack,
    /// Falling from full level to the sustain level.
    Decay,
    /// Holding the sustain level until the note is released.
    Sustain,
    /// Falling to silence after a note off.
    Release,
    /// Falling to silence over the fixed de-click ramp after a force-silence.
    Kill,
}

/// A four-stage amplitude envelope.
#[derive(Clone, Debug)]
pub struct Adsr {
    stage: EnvelopeStage,
    level: f32,
    /// Distance the decay still has to cover, `level - sustain`.
    ///
    /// The decay steps this rather than the level itself. Iterating
    /// `level += (sustain - level) * coeff` in `f32` stalls: once the remaining gap is small
    /// next to the level, `level + gap * coeff` rounds back to `level` and the envelope freezes
    /// above the sustain level, so the arrival test never fires and a note reports `Decay`
    /// forever. At 48 kHz that starts at a decay of 0.7 s, which is inside the range every
    /// instrument exposes. Decaying the gap in its own variable keeps each step inside the gap's
    /// own exponent range, so it converges geometrically however long the decay is. This is the
    /// same trick, for the same reason, as `SmoothedValue` in `auris-dsp`.
    gap: f32,
    sample_rate: f32,
    attack_seconds: f32,
    decay_seconds: f32,
    sustain: f32,
    release_seconds: f32,
    attack_step: f32,
    decay_coeff: f32,
    release_coeff: f32,
    kill_step: f32,
}

impl Default for Adsr {
    fn default() -> Self {
        Adsr::new()
    }
}

impl Adsr {
    /// An envelope at 48 kHz with a short attack and a moderate release.
    pub fn new() -> Self {
        let mut envelope = Self {
            stage: EnvelopeStage::Idle,
            level: 0.0,
            gap: 0.0,
            sample_rate: 48_000.0,
            attack_seconds: 0.005,
            decay_seconds: 0.1,
            sustain: 0.7,
            release_seconds: 0.1,
            attack_step: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            kill_step: 0.0,
        };
        envelope.refresh();
        envelope
    }

    /// Sets the rate the envelope is stepped at and recomputes its coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate > 0.0 {
            self.sample_rate = sample_rate;
            self.refresh();
        }
    }

    /// Sets all four stages at once. Times are in seconds, `sustain` is a linear 0..1 level.
    pub fn set_adsr(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) {
        self.attack_seconds = non_negative(attack);
        self.decay_seconds = non_negative(decay);
        self.sustain = non_negative(sustain).min(1.0);
        self.release_seconds = non_negative(release);
        self.refresh();
    }

    /// Attack time in seconds.
    pub fn attack(&self) -> f32 {
        self.attack_seconds
    }

    /// Decay time in seconds.
    pub fn decay(&self) -> f32 {
        self.decay_seconds
    }

    /// Sustain level, linear.
    pub fn sustain(&self) -> f32 {
        self.sustain
    }

    /// Release time in seconds.
    pub fn release_time(&self) -> f32 {
        self.release_seconds
    }

    /// Starts the attack stage.
    ///
    /// The current level is kept rather than reset to zero, so retriggering a voice that is
    /// still sounding — the usual case when a voice gets stolen — stays continuous.
    pub fn trigger(&mut self) {
        self.stage = EnvelopeStage::Attack;
        self.gap = self.level - self.sustain;
    }

    /// Moves to the release stage. Envelopes that are already idle or being killed are left
    /// alone, so a stray note off cannot resurrect a finished voice.
    pub fn release(&mut self) {
        match self.stage {
            EnvelopeStage::Idle | EnvelopeStage::Kill | EnvelopeStage::Release => {}
            _ => self.stage = EnvelopeStage::Release,
        }
    }

    /// Starts the fixed de-click ramp to silence, used when an all-sound-off arrives.
    pub fn kill(&mut self) {
        if self.stage == EnvelopeStage::Idle {
            return;
        }
        let samples = (KILL_SECONDS * self.sample_rate).max(1.0);
        self.kill_step = self.level / samples;
        self.stage = EnvelopeStage::Kill;
    }

    /// Drops to silence immediately, without any ramp.
    pub fn silence(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.level = 0.0;
    }

    /// Produces the gain for the next sample and advances one step.
    pub fn process(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => return 0.0,
            EnvelopeStage::Attack => {
                self.level += self.attack_step;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.gap = self.level - self.sustain;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.gap *= 1.0 - self.decay_coeff;
                self.level = self.sustain + self.gap;
                if self.gap.abs() <= SILENCE_LEVEL {
                    self.gap = 0.0;
                    // A sustain of zero means the note is percussive: it is over here, and the
                    // voice is free without waiting for a note off.
                    if self.sustain <= SILENCE_LEVEL {
                        self.level = 0.0;
                        self.stage = EnvelopeStage::Idle;
                    } else {
                        self.level = self.sustain;
                        self.stage = EnvelopeStage::Sustain;
                    }
                }
            }
            EnvelopeStage::Sustain => {}
            EnvelopeStage::Release => {
                self.level -= self.level * self.release_coeff;
                if self.level <= SILENCE_LEVEL {
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
            EnvelopeStage::Kill => {
                self.level -= self.kill_step;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        self.level
    }

    /// Current gain, without advancing.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Stage the envelope is in.
    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    /// `true` once the envelope has reached silence and its voice can be reclaimed.
    pub fn is_finished(&self) -> bool {
        self.stage == EnvelopeStage::Idle
    }

    /// `true` while the envelope is producing sound.
    pub fn is_active(&self) -> bool {
        self.stage != EnvelopeStage::Idle
    }

    /// `true` once the note has been let go, whether by a note off or by a force-silence.
    pub fn is_releasing(&self) -> bool {
        matches!(self.stage, EnvelopeStage::Release | EnvelopeStage::Kill)
    }

    fn refresh(&mut self) {
        // Moving the sustain level under a decay that is already running changes how far it has
        // left to travel, so the tracked gap has to be restated against the new target.
        self.gap = self.level - self.sustain;
        self.attack_step = 1.0 / stage_samples(self.attack_seconds, self.sample_rate);
        self.decay_coeff = exponential_coeff(self.decay_seconds, self.sample_rate);
        self.release_coeff = exponential_coeff(self.release_seconds, self.sample_rate);
    }
}

/// Length of a stage in samples, never less than one so that a zero time is instantaneous
/// rather than a division by zero.
fn stage_samples(seconds: f32, sample_rate: f32) -> f32 {
    (non_negative(seconds) * sample_rate).max(1.0)
}

/// Per-sample coefficient of a one-pole fall that lands within -80 dB of its target after
/// `seconds`.
fn exponential_coeff(seconds: f32, sample_rate: f32) -> f32 {
    let samples = stage_samples(seconds, sample_rate);
    1.0 - (-SEGMENT_TIME_CONSTANTS / samples).exp()
}

/// Clamps a user-supplied time or level to something usable; NaN becomes zero.
fn non_negative(value: f32) -> f32 {
    // `f32::max` returns the non-NaN operand, so this also filters NaN out.
    value.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::param::gain_to_db;

    fn envelope(attack: f32, decay: f32, sustain: f32, release: f32) -> Adsr {
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(48_000.0);
        envelope.set_adsr(attack, decay, sustain, release);
        envelope
    }

    #[test]
    fn an_untriggered_envelope_stays_silent() {
        let mut env = envelope(0.01, 0.1, 1.0, 0.1);
        for _ in 0..1_000 {
            assert_eq!(env.process(), 0.0);
        }
        assert!(env.is_finished());
    }

    #[test]
    fn attack_reaches_full_level_after_the_attack_time() {
        let mut env = envelope(0.01, 0.1, 1.0, 0.1);
        env.trigger();
        // 10 ms at 48 kHz is 480 samples.
        for _ in 0..479 {
            env.process();
        }
        assert!(
            (env.level() - 479.0 / 480.0).abs() < 1e-3,
            "level {}",
            env.level()
        );
        env.process();
        assert!((env.level() - 1.0).abs() < 1e-6);
        assert_eq!(env.stage(), EnvelopeStage::Decay);
    }

    #[test]
    fn decay_settles_on_the_sustain_level() {
        let mut env = envelope(0.0, 0.05, 0.5, 0.1);
        env.trigger();
        // Attack is instantaneous, then 50 ms of decay: 2400 samples.
        for _ in 0..(1 + 2_400) {
            env.process();
        }
        assert_eq!(env.stage(), EnvelopeStage::Sustain);
        assert!((env.level() - 0.5).abs() < 1e-3, "level {}", env.level());
        // Sustain holds indefinitely.
        for _ in 0..10_000 {
            env.process();
        }
        assert!((env.level() - 0.5).abs() < 1e-3);

        // A long decay has to arrive too. Its per-sample step is tiny next to the level it is
        // subtracted from, so stepping the level itself rounds to a no-op and strands the
        // envelope in Decay for the life of the note. Both rates are checked because a higher
        // rate makes the step smaller still.
        for sample_rate in [48_000.0f32, 96_000.0] {
            let mut env = Adsr::new();
            env.set_sample_rate(sample_rate);
            env.set_adsr(0.0, 4.0, 0.5, 0.1);
            env.trigger();
            let steps = (4.0 * sample_rate) as usize + 1;
            for _ in 0..steps {
                env.process();
            }
            assert_eq!(
                env.stage(),
                EnvelopeStage::Sustain,
                "stuck at {} at {sample_rate} Hz",
                env.level()
            );
            assert_eq!(env.level(), 0.5);
        }
    }

    #[test]
    fn moving_the_sustain_level_mid_decay_keeps_the_output_continuous() {
        // The decay tracks its remaining distance separately from the level, so a sustain level
        // that moves under a running decay has to restate that distance. Leaving it stale would
        // shift the whole envelope by however far the sustain moved, in one sample — a click,
        // and one a user can trigger just by dragging the sustain slider on a held note.
        let mut env = envelope(0.0, 0.5, 0.8, 0.1);
        env.trigger();
        for _ in 0..2_000 {
            env.process();
        }
        let before = env.level();
        assert!(before > 0.85, "the decay is supposed to still be running");
        env.set_adsr(0.0, 0.5, 0.2, 0.1);
        let after = env.process();
        assert!(
            (after - before).abs() < 1e-3,
            "level jumped from {before} to {after}"
        );
        // And the decay still lands on the level it was moved to.
        for _ in 0..30_000 {
            env.process();
        }
        assert_eq!(env.stage(), EnvelopeStage::Sustain);
        assert!((env.level() - 0.2).abs() < 1e-6, "level {}", env.level());
    }

    #[test]
    fn release_falls_below_minus_60_db_within_the_release_time() {
        let mut env = envelope(0.0, 0.001, 1.0, 0.25);
        env.trigger();
        for _ in 0..100 {
            env.process();
        }
        env.release();
        // 250 ms at 48 kHz.
        let mut level = env.level();
        for _ in 0..12_000 {
            level = env.process();
        }
        assert!(
            gain_to_db(level) <= -60.0,
            "level {level} is {} dBFS",
            gain_to_db(level)
        );
        assert!(env.is_finished(), "voice should be reclaimable");
    }

    #[test]
    fn a_zero_sustain_ends_the_note_at_the_end_of_the_decay() {
        let mut env = envelope(0.0, 0.02, 0.0, 0.1);
        env.trigger();
        // 20 ms at 48 kHz is 960 samples, plus the one-sample instantaneous attack.
        for _ in 0..850 {
            env.process();
        }
        assert!(!env.is_finished(), "ended {} samples early", 960 - 850);
        for _ in 0..120 {
            env.process();
        }
        assert!(env.is_finished());
        assert_eq!(env.process(), 0.0);
    }

    #[test]
    fn kill_ramps_to_zero_over_two_milliseconds() {
        let mut env = envelope(0.0, 1.0, 1.0, 1.0);
        env.trigger();
        env.process();
        assert!((env.level() - 1.0).abs() < 1e-6);
        env.kill();
        // 2 ms at 48 kHz is 96 samples, so the ramp is half way down after 48.
        for _ in 0..48 {
            env.process();
        }
        assert!((env.level() - 0.5).abs() < 0.02, "level {}", env.level());
        for _ in 0..42 {
            env.process();
        }
        assert!(env.level() > 0.0, "the ramp finished early");
        for _ in 0..8 {
            env.process();
        }
        assert_eq!(env.level(), 0.0);
        assert!(env.is_finished());
    }

    #[test]
    fn kill_is_monotonic_so_it_cannot_click() {
        let mut env = envelope(0.001, 0.5, 0.8, 0.5);
        env.trigger();
        for _ in 0..200 {
            env.process();
        }
        env.kill();
        let mut previous = env.level();
        for _ in 0..96 {
            let level = env.process();
            assert!(level <= previous + 1e-7, "{level} rose above {previous}");
            previous = level;
        }
    }

    #[test]
    fn silence_is_immediate_and_release_cannot_revive_it() {
        let mut env = envelope(0.1, 0.1, 1.0, 0.1);
        env.trigger();
        for _ in 0..100 {
            env.process();
        }
        env.silence();
        assert_eq!(env.level(), 0.0);
        env.release();
        assert!(env.is_finished());
        assert_eq!(env.process(), 0.0);
    }

    #[test]
    fn degenerate_times_stay_finite() {
        for (attack, decay, sustain, release) in [
            (0.0, 0.0, 0.0, 0.0),
            (f32::NAN, f32::NAN, f32::NAN, f32::NAN),
            (-1.0, -1.0, -1.0, -1.0),
            (100.0, 100.0, 1.0, 100.0),
        ] {
            let mut env = envelope(attack, decay, sustain, release);
            env.trigger();
            for _ in 0..1_000 {
                let level = env.process();
                assert!(level.is_finite(), "level went non-finite");
                assert!((0.0..=1.0).contains(&level), "level {level} out of range");
            }
            env.release();
            for _ in 0..1_000 {
                assert!(env.process().is_finite());
            }
        }
    }
}
