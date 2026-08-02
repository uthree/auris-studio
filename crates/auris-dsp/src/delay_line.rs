//! A fractional-delay ring buffer.

/// A ring buffer with linear interpolation on read.
///
/// The buffer length is rounded up to a power of two so the wrap-around is a bitmask instead of
/// a modulo — the read/write index arithmetic then costs one AND per access, which matters in
/// a reverb running twelve of these per channel.
///
/// # Read / write ordering
///
/// [`Self::read`] is meant to be called **before** [`Self::write`] for the current frame. Under
/// that ordering `read(d)` returns exactly the sample written `d` frames ago, so a `d` of
/// `1.0` is the previous frame's input. This is what a feedback loop needs:
///
/// ```
/// # use auris_dsp::DelayLine;
/// let mut line = DelayLine::new(16);
/// for n in 0..8 {
///     let delayed = line.read(4.0);
///     line.write(if n == 0 { 1.0 } else { 0.0 });
///     if n == 4 {
///         assert_eq!(delayed, 1.0);
///     }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    write_index: usize,
}

/// Shortest buffer allocated. Two slots of headroom are reserved above the largest usable
/// delay so the interpolator's second tap never wraps onto the sample about to be written.
const MIN_BUFFER_LEN: usize = 4;

impl DelayLine {
    /// A cleared line able to delay by at least `max_delay_samples` frames.
    pub fn new(max_delay_samples: usize) -> Self {
        let mut line = Self {
            buffer: Vec::new(),
            mask: 0,
            write_index: 0,
        };
        line.resize(max_delay_samples);
        line
    }

    /// Reallocates for a new maximum delay and clears the contents.
    ///
    /// Allocates — call from `prepare`, never from `process`.
    pub fn resize(&mut self, max_delay_samples: usize) {
        let len = max_delay_samples
            .saturating_add(2)
            .max(MIN_BUFFER_LEN)
            .next_power_of_two();
        self.buffer = vec![0.0; len];
        self.mask = len - 1;
        self.write_index = 0;
    }

    /// Largest delay this line can produce, in frames.
    pub fn max_delay_samples(&self) -> usize {
        self.buffer.len() - 2
    }

    /// Fills the line with silence and rewinds the write head.
    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_index = 0;
    }

    /// Pushes one sample in, overwriting the oldest.
    #[inline]
    pub fn write(&mut self, sample: f32) {
        self.buffer[self.write_index] = sample;
        self.write_index = (self.write_index + 1) & self.mask;
    }

    /// Reads `delay_samples` frames back, interpolating linearly between neighbours.
    ///
    /// The delay is clamped to `1.0 ..= max_delay_samples`; a non-finite request reads silence.
    #[inline]
    pub fn read(&self, delay_samples: f32) -> f32 {
        if !delay_samples.is_finite() {
            return 0.0;
        }
        let delay = delay_samples.clamp(1.0, self.max_delay_samples() as f32);
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        let len = self.buffer.len();
        // `whole` is at most len - 2, so neither index can reach the slot about to be written.
        let recent = (self.write_index + len - whole) & self.mask;
        let older = (recent + self.mask) & self.mask;
        self.buffer[recent] * (1.0 - fraction) + self.buffer[older] * fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        assert_eq!(DelayLine::new(0).max_delay_samples(), 2);
        assert_eq!(DelayLine::new(8).max_delay_samples(), 14);
        assert_eq!(DelayLine::new(1_000).max_delay_samples(), 1_022);
    }

    #[test]
    fn integer_delay_is_exact() {
        let mut line = DelayLine::new(64);
        line.write(1.0);
        line.write(0.0);
        line.write(0.0);
        assert_eq!(line.read(3.0), 1.0);
        assert_eq!(line.read(2.0), 0.0);
        assert_eq!(line.read(1.0), 0.0);
    }

    #[test]
    fn fractional_delay_interpolates_between_taps() {
        let mut line = DelayLine::new(64);
        line.write(1.0);
        line.write(0.0);
        line.write(0.0);
        // Half way between tap 2 (0.0) and tap 3 (1.0).
        assert_eq!(line.read(2.5), 0.5);
        assert!((line.read(2.25) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_full_lap_of_the_ring_still_lines_up() {
        let mut line = DelayLine::new(8);
        let capacity = line.max_delay_samples();
        for n in 0..1_000 {
            line.write(n as f32);
        }
        // The most recent write was 999.
        assert_eq!(line.read(1.0), 999.0);
        assert_eq!(line.read(capacity as f32), (1_000 - capacity) as f32);
    }

    #[test]
    fn out_of_range_requests_are_clamped_not_panics() {
        let mut line = DelayLine::new(8);
        line.write(0.5);
        assert_eq!(line.read(-5.0), 0.5);
        assert_eq!(line.read(1.0e9), 0.0);
        assert_eq!(line.read(f32::NAN), 0.0);
    }

    #[test]
    fn reset_clears_the_contents() {
        let mut line = DelayLine::new(8);
        line.write(1.0);
        line.reset();
        assert_eq!(line.read(1.0), 0.0);
    }
}
