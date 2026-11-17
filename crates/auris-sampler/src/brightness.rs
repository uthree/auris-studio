//! The SoundFont sampler's answer to the brightness controller.
//!
//! A font's modulators are read once, at note-on, so nothing in a SoundFont can follow a
//! controller that moves while a note sounds — which is what a swell, or a phrase growing softer,
//! asks of the sound. The sampler therefore answers [`auris_core::CC_BRIGHTNESS`] itself, after
//! the font, with a one-pole low-pass blended into the dry signal.
//!
//! At the controller's resting middle the blend is zero and the output is the font's, bit for
//! bit; above the middle it stays there, since a filter can take brightness away but cannot add
//! what was never recorded. Below the middle the blend and the cutoff move together, from a
//! touch of an 8 kHz roll-off just under the middle to all of an 800 Hz one at zero.

/// Cutoff with the blend just above zero, in hertz.
const OPEN_HZ: f32 = 8000.0;
/// Octaves the cutoff falls between the middle and zero: 8 kHz down to about 800 Hz.
const OCTAVES: f32 = 3.3;
/// How long the blend takes to follow the controller, in seconds, so a step cannot click.
const SMOOTHING_SECONDS: f32 = 0.01;
/// Below this the blend counts as gone, and the filter is skipped.
const SILENT_DEPTH: f32 = 1e-4;

/// Per-channel darkening, smoothed towards what the controller last said.
#[derive(Clone, Debug)]
pub(crate) struct Brightness {
    sample_rate: f32,
    follow: f32,
    target: f32,
    depth: f32,
    state: [f32; 2],
    engaged: bool,
}

impl Default for Brightness {
    fn default() -> Self {
        let mut brightness = Self {
            sample_rate: 48_000.0,
            follow: 0.0,
            target: 0.0,
            depth: 0.0,
            state: [0.0; 2],
            engaged: false,
        };
        brightness.prepare(48_000.0);
        brightness
    }
}

impl Brightness {
    /// Sets the rate and returns to rest.
    pub(crate) fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.follow = 1.0 - (-1.0 / (SMOOTHING_SECONDS * self.sample_rate)).exp();
        self.reset();
    }

    /// Back to the resting middle at once, as a reset of the channel is.
    pub(crate) fn reset(&mut self) {
        self.target = 0.0;
        self.depth = 0.0;
        self.state = [0.0; 2];
        self.engaged = false;
    }

    /// The controller's new value, 0 to 1.
    pub(crate) fn set(&mut self, value: f32) {
        let value = if value.is_finite() { value } else { 0.5 };
        self.target = (1.0 - value.clamp(0.0, 1.0) / 0.5).max(0.0);
    }

    /// Darkens a stereo run in place. Allocates nothing.
    pub(crate) fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.engaged && self.target <= 0.0 && self.depth <= SILENT_DEPTH {
            return;
        }
        if !self.engaged {
            // Start the filter where the signal is, so engaging it cannot step.
            self.state = [
                left.first().copied().unwrap_or(0.0),
                right.first().copied().unwrap_or(0.0),
            ];
            self.engaged = true;
        }
        let tau = std::f32::consts::TAU;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.depth += (self.target - self.depth) * self.follow;
            let cutoff = OPEN_HZ * (-OCTAVES * self.depth).exp2();
            let coefficient = 1.0 - (-tau * cutoff / self.sample_rate).exp();
            for (sample, state) in [l, r].into_iter().zip(self.state.iter_mut()) {
                *state += coefficient * (*sample - *state);
                *sample += self.depth * (*state - *sample);
            }
        }
        if self.target <= 0.0 && self.depth <= SILENT_DEPTH {
            self.depth = 0.0;
            self.engaged = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect()
    }

    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt()
    }

    fn through(value: f32, hz: f32) -> f32 {
        let mut brightness = Brightness::default();
        brightness.set(value);
        let mut left = sine(hz, 48_000);
        let mut right = left.clone();
        brightness.process(&mut left, &mut right);
        assert_eq!(left, right);
        rms(&left[24_000..]) / rms(&sine(hz, 48_000)[24_000..])
    }

    #[test]
    fn the_middle_and_above_leave_the_font_untouched() {
        let mut brightness = Brightness::default();
        for value in [0.5, 0.8, 1.0] {
            brightness.set(value);
            let mut left = sine(5000.0, 4800);
            let mut right = left.clone();
            brightness.process(&mut left, &mut right);
            assert_eq!(left, sine(5000.0, 4800));
        }
    }

    #[test]
    fn lower_values_take_the_top_away_and_leave_the_bottom() {
        // At zero the cutoff is about 800 Hz with the filter fully blended in.
        let high = through(0.0, 5000.0);
        assert!(high < 0.2, "{high}");
        let low = through(0.0, 100.0);
        assert!(low > 0.95, "{low}");
        // Halfway down is a gentler roll-off.
        let half = through(0.25, 5000.0);
        assert!(half > high && half < 0.8, "{half}");
    }

    #[test]
    fn returning_to_the_middle_settles_back_to_the_dry_signal() {
        let mut brightness = Brightness::default();
        brightness.set(0.0);
        let mut left = sine(3000.0, 4800);
        let mut right = left.clone();
        brightness.process(&mut left, &mut right);
        brightness.set(0.5);
        let mut left = sine(3000.0, 48_000);
        let mut right = left.clone();
        brightness.process(&mut left, &mut right);
        assert!(!brightness.engaged);
        let dry = sine(3000.0, 48_000);
        assert_eq!(&left[40_000..], &dry[40_000..]);
    }
}
