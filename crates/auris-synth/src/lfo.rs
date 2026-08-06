//! The low-frequency oscillator the built-in instruments vibrato with.
//!
//! Separate from [`Oscillator`](crate::oscillator::Oscillator), which is an *audio* oscillator:
//! band-limited, seeded for noise, and carrying five waveforms none of which a vibrato wants. This
//! is a sine between −1 and 1 at a few hertz, and the only thing it has to be is cheap and
//! phase-continuous.
//!
//! # One per voice
//!
//! Each voice owns one and [`Lfo::reset`]s it at note on, so every note's vibrato starts from the
//! centre and swings the same way. A single instrument-wide LFO is the other reasonable choice and
//! it is the wrong one here: a chord struck together would have every note already somewhere
//! different in its cycle, so the chord would arrive detuned by however far the wheel was up, and
//! the amount would depend on when the *previous* note happened to have been played.
//!
//! It also fits how the instruments render. A voice is the outer loop and a sample is the inner
//! one, so a per-voice LFO advances exactly where it is read; an instrument-wide one would have to
//! be rendered into a scratch buffer first, and there is no allocation available to put one in.

use std::f32::consts::TAU;

/// A sine between −1 and 1, at a rate a note can be heard wobbling at.
#[derive(Copy, Clone, Debug)]
pub struct Lfo {
    /// Where in the cycle it is, from 0 to 1.
    phase: f32,
    /// How far the phase moves per sample.
    increment: f32,
    sample_rate: f32,
    rate: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Lfo::new()
    }
}

impl Lfo {
    /// Sample rate assumed until [`Lfo::set_sample_rate`] is called.
    pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

    /// A stopped oscillator at the centre of its swing.
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            increment: 0.0,
            sample_rate: Self::DEFAULT_SAMPLE_RATE,
            rate: 0.0,
        }
    }

    /// Sets the sample rate, keeping the rate in hertz it was set to.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            Self::DEFAULT_SAMPLE_RATE
        };
        self.set_rate(self.rate);
    }

    /// Sets how fast it swings, in hertz.
    ///
    /// A rate at or below zero stops it *at whatever phase it is on*, which is why [`Self::reset`]
    /// exists separately: turning the rate to nothing mid-note should not jump the pitch.
    pub fn set_rate(&mut self, hz: f32) {
        self.rate = if hz.is_finite() { hz.max(0.0) } else { 0.0 };
        self.increment = self.rate / self.sample_rate;
    }

    /// Puts the phase back at the centre of the swing, on the way up.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Advances one sample and returns the value there, from −1 to 1.
    ///
    /// Named `next` to match [`Oscillator::next`](crate::oscillator::Oscillator::next), which is
    /// the thing every reader will have in mind when they meet this. It is not an iterator and
    /// never could be — an infinite one is not something anybody should be able to `collect`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let value = (self.phase * TAU).sin();
        self.phase += self.increment;
        // Wrapped rather than left to grow: an `f32` phase counted up for an hour loses enough
        // precision that the sine visibly steps.
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many samples one cycle takes at `hz`.
    fn cycle(hz: f32, sample_rate: f32) -> usize {
        (sample_rate / hz).round() as usize
    }

    #[test]
    fn it_starts_at_the_centre_and_swings_up_first() {
        // Which is what makes a note's vibrato start *at the pitch that was played*: any other
        // phase would make the attack of every note land sharp or flat by the depth.
        let mut lfo = Lfo::new();
        lfo.set_sample_rate(48_000.0);
        lfo.set_rate(5.0);
        assert_eq!(lfo.next(), 0.0);
        assert!(lfo.next() > 0.0, "the first move is upwards");
    }

    #[test]
    fn one_cycle_takes_the_time_the_rate_says() {
        let sample_rate = 48_000.0;
        let hz = 6.0;
        let mut lfo = Lfo::new();
        lfo.set_sample_rate(sample_rate);
        lfo.set_rate(hz);

        let samples = cycle(hz, sample_rate);
        let mut peak_at = 0;
        let mut peak = f32::MIN;
        for index in 0..samples {
            let value = lfo.next();
            assert!((-1.0..=1.0).contains(&value), "{value} is off the range");
            if value > peak {
                peak = value;
                peak_at = index;
            }
        }
        // The top of the swing is a quarter of the way through, and the value there is 1.
        assert!((peak - 1.0).abs() < 0.001, "the peak is {peak}");
        let expected = samples / 4;
        assert!(
            peak_at.abs_diff(expected) <= 2,
            "the peak was at {peak_at}, expected about {expected}"
        );
    }

    #[test]
    fn a_rate_of_nothing_holds_still_rather_than_jumping() {
        // Turning the rate down mid-note must not move the pitch, so a stopped LFO stays where
        // it stopped rather than snapping back to the centre.
        let mut lfo = Lfo::new();
        lfo.set_sample_rate(48_000.0);
        lfo.set_rate(6.0);
        for _ in 0..2000 {
            lfo.next();
        }
        lfo.set_rate(0.0);
        let held = lfo.next();
        for _ in 0..100 {
            assert_eq!(lfo.next(), held, "a stopped LFO moved");
        }
        lfo.reset();
        assert_eq!(lfo.next(), 0.0, "and reset is what brings it back");
    }

    #[test]
    fn changing_the_sample_rate_keeps_the_rate_in_hertz() {
        // `prepare` sets the sample rate after the parameters have been read, so a rate set
        // first has to survive it — otherwise every vibrato would run at whatever 48 kHz made
        // of it, on hardware running at 96.
        let mut lfo = Lfo::new();
        lfo.set_rate(5.0);
        lfo.set_sample_rate(96_000.0);
        let samples = cycle(5.0, 96_000.0);
        let mut positive: usize = 0;
        for _ in 0..samples {
            if lfo.next() > 0.0 {
                positive += 1;
            }
        }
        // Half the cycle is above zero, whatever the rate the samples are counted at.
        assert!(
            positive.abs_diff(samples / 2) <= 2,
            "{positive} of {samples} were positive"
        );
    }

    #[test]
    fn a_nonsense_rate_is_stillness_rather_than_a_panic() {
        // Reached from a parameter, which is reached from a project file.
        let mut lfo = Lfo::new();
        lfo.set_sample_rate(48_000.0);
        for bad in [f32::NAN, f32::INFINITY, -3.0] {
            lfo.set_rate(bad);
            lfo.reset();
            assert_eq!(lfo.next(), 0.0);
            assert_eq!(lfo.next(), 0.0, "{bad} should hold it still");
        }
        lfo.set_sample_rate(0.0);
        lfo.set_rate(5.0);
        assert!(lfo.next().is_finite());
    }
}
