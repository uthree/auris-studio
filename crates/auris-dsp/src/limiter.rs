//! `auris.fx.limiter` — a look-ahead brickwall limiter.

use auris_core::AudioBuffer;
use auris_core::param::{
    ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::MILLISECONDS_PER_SECOND;
use crate::bank::ParamBank;
use crate::delay_line::DelayLine;
use crate::smooth::one_pole_coefficient;

const P_INPUT_DB: u32 = 0;
const P_CEILING_DB: u32 = 1;
const P_RELEASE_MS: u32 = 2;

/// Look-ahead, in milliseconds.
///
/// Fixed rather than exposed as a parameter because it *is* the plugin's reported latency, and
/// a latency that changes while the transport is rolling would desynchronise delay compensation.
/// Two milliseconds is long enough to round the gain curve over a kick drum transient without
/// pushing the mix bus into audible pre-echo.
const LOOKAHEAD_MS: f32 = 2.0;

/// Highest rate used to size the limiter's internal windows.
///
/// This is well above production audio formats while keeping a corrupt project value from
/// turning `prepare` into an unbounded allocation.
const MAX_PREPARE_SAMPLE_RATE: f32 = 768_000.0;

/// A brickwall limiter whose output provably cannot exceed its ceiling.
///
/// The gain is derived from the input in three steps:
///
/// 1. `required[n] = min(1, ceiling / |x[n]|)` — the gain each sample would need on its own.
/// 2. A **sliding minimum** over the look-ahead window, so the gain starts falling `L` samples
///    *before* a peak arrives rather than after it.
/// 3. A **moving average** of that, over the same window, which rounds the corners off the
///    resulting curve.
///
/// Averaging is what makes the guarantee work: every value inside the average's window is a
/// window minimum that already contains `required[n - L]`, so the mean cannot exceed it either.
/// Applying that mean to `x[n - L]` therefore always lands at or below the ceiling.
pub struct Limiter {
    params: ParamBank,
    sample_rate: f32,
    lookahead: usize,
    lines: Vec<DelayLine>,
    minimum: SlidingMinimum,
    average: MovingAverage,
    /// Gain after the release stage, always at or below the window minimum.
    hold: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    /// A new limiter at its default settings.
    pub fn new() -> Self {
        let descriptors = vec![
            ParamDescriptor::decibels(P_INPUT_DB, "input_db", "Input", 0.0, 24.0, 0.0),
            ParamDescriptor::decibels(P_CEILING_DB, "ceiling_db", "Ceiling", -24.0, 0.0, -0.3),
            ParamDescriptor::new(P_RELEASE_MS, "release_ms", "Release", 1.0, 1_000.0, 100.0)
                .with_unit(ParamUnit::Milliseconds)
                .with_curve(ParamValueCurve::Logarithmic),
        ];
        Self {
            params: ParamBank::new(descriptors),
            sample_rate: 48_000.0,
            lookahead: 1,
            lines: Vec::new(),
            minimum: SlidingMinimum::new(1),
            average: MovingAverage::new(1),
            hold: 1.0,
        }
    }

    /// Gain the limiter is currently applying, in dB; zero when it is not working.
    ///
    /// Read from the stage before the smoothing average, which is what a meter should show:
    /// it reaches the true reduction of the peak the limiter is reacting to.
    pub fn gain_reduction_db(&self) -> f32 {
        gain_to_db(self.hold.clamp(1.0e-6, 1.0))
    }
}

impl Parameterized for Limiter {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.params.set(id, value);
    }
}

impl Effect for Limiter {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.limiter",
            "Limiter",
            "Look-ahead brickwall limiter with a guaranteed output ceiling",
            PluginCategory::Dynamics,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        let requested_rate = ctx.sample_rate as f32;
        self.sample_rate = if requested_rate.is_finite() && requested_rate > 0.0 {
            requested_rate.min(MAX_PREPARE_SAMPLE_RATE)
        } else {
            48_000.0
        };
        self.lookahead =
            ((LOOKAHEAD_MS * self.sample_rate / MILLISECONDS_PER_SECOND).round() as usize).max(1);

        let window = self.lookahead + 1;
        self.minimum = SlidingMinimum::new(window);
        self.average = MovingAverage::new(window);

        let capacity = self.lookahead + 2;
        self.lines.clear();
        self.lines
            .resize_with(ctx.channel_count.max(1), || DelayLine::new(capacity));
        self.hold = 1.0;
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        self.minimum.reset();
        self.average.reset();
        self.hold = 1.0;
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        let channels = buffer.channel_count().min(self.lines.len());
        if frames == 0 || channels == 0 {
            return;
        }

        let input_gain = db_to_gain(self.params.at(P_INPUT_DB));
        let ceiling = db_to_gain(self.params.at(P_CEILING_DB));
        let release = one_pole_coefficient(
            self.params.at(P_RELEASE_MS) / MILLISECONDS_PER_SECOND,
            self.sample_rate,
        );
        let lookahead = self.lookahead as f32;

        for frame in 0..frames {
            let mut peak = 0.0f32;
            for channel in buffer.channels().iter().take(channels) {
                peak = peak.max((channel[frame] * input_gain).abs());
            }
            let required = if peak > ceiling { ceiling / peak } else { 1.0 };

            let window_minimum = self.minimum.push(required);
            // Release only ever moves the gain back up, and the window minimum caps it, so the
            // bound survives the smoothing.
            self.hold = (1.0 - (1.0 - self.hold) * release).min(window_minimum);
            let gain = self.average.push(self.hold);

            for (channel, line) in buffer
                .channels_mut()
                .iter_mut()
                .zip(self.lines.iter_mut())
                .take(channels)
            {
                let driven = crate::settled(channel[frame] * input_gain);
                let delayed = line.read(lookahead);
                line.write(driven);
                // The clamp is a backstop for f32 rounding in the division above; the gain
                // computation already keeps the product inside the ceiling.
                channel[frame] = (delayed * gain).clamp(-ceiling, ceiling);
            }
        }
    }

    fn tail_frames(&self) -> usize {
        self.lookahead
    }

    fn latency_frames(&self) -> usize {
        self.lookahead
    }
}

/// Minimum of the last `window` pushed values, in amortised constant time.
///
/// A monotonic deque: values that can never be the minimum again (because a smaller value
/// arrived later) are dropped on the way in, so the front is always the answer.
struct SlidingMinimum {
    values: Vec<f32>,
    positions: Vec<u64>,
    head: usize,
    tail: usize,
    mask: usize,
    window: u64,
    position: u64,
}

impl SlidingMinimum {
    fn new(window: usize) -> Self {
        let window = window.max(1);
        // A push can transiently hold `window + 1` entries before the front is evicted, so two
        // spare slots keep "full" from ever looking like "empty".
        let capacity = (window + 2).next_power_of_two();
        Self {
            values: vec![0.0; capacity],
            positions: vec![0; capacity],
            head: 0,
            tail: 0,
            mask: capacity - 1,
            window: window as u64,
            position: 0,
        }
    }

    fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.position = 0;
    }

    #[inline]
    fn push(&mut self, value: f32) -> f32 {
        while self.tail != self.head {
            let last = (self.tail + self.mask) & self.mask;
            if self.values[last] >= value {
                self.tail = last;
            } else {
                break;
            }
        }
        self.values[self.tail] = value;
        self.positions[self.tail] = self.position;
        self.tail = (self.tail + 1) & self.mask;

        // Drop the front while it is older than the window. The entry just pushed is never
        // older than the window, so the deque cannot empty here.
        while self.positions[self.head] + self.window <= self.position {
            self.head = (self.head + 1) & self.mask;
        }
        self.position += 1;
        self.values[self.head]
    }
}

/// Mean of the last `window` pushed values.
struct MovingAverage {
    buffer: Vec<f32>,
    index: usize,
    /// Accumulated in `f64`: an `f32` running sum would drift audibly over a long render.
    sum: f64,
    count: usize,
}

impl MovingAverage {
    fn new(window: usize) -> Self {
        Self {
            buffer: vec![0.0; window.max(1)],
            index: 0,
            sum: 0.0,
            count: 0,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.sum = 0.0;
        self.count = 0;
    }

    #[inline]
    fn push(&mut self, value: f32) -> f32 {
        let evicted = self.buffer[self.index];
        self.buffer[self.index] = value;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        self.sum += value as f64 - evicted as f64;
        if self.count < self.buffer.len() {
            self.count += 1;
        }
        (self.sum / self.count as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::param::gain_to_db;

    const SR: f64 = 48_000.0;

    fn prepared() -> Limiter {
        let mut plugin = Limiter::new();
        plugin.prepare(&PrepareContext::new(SR, 8_192, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    fn sine(frames: usize, frequency: f32, amplitude: f32) -> AudioBuffer {
        let step = std::f32::consts::TAU * frequency / SR as f32;
        let samples: Vec<f32> = (0..frames)
            .map(|n| (step * n as f32).sin() * amplitude)
            .collect();
        AudioBuffer::from_planar(vec![samples.clone(), samples], SR).unwrap()
    }

    #[test]
    fn latency_is_the_lookahead() {
        let plugin = prepared();
        // 2 ms at 48 kHz.
        assert_eq!(plugin.latency_frames(), 96);
        assert_eq!(plugin.tail_frames(), 96);

        let mut at_44k = Limiter::new();
        at_44k.prepare(&PrepareContext::new(44_100.0, 512, 2));
        assert_eq!(at_44k.latency_frames(), 88);
    }

    #[test]
    fn an_untrusted_sample_rate_cannot_request_an_unbounded_window() {
        let mut plugin = Limiter::new();
        plugin.prepare(&PrepareContext::new(1.0e12, 512, 2));

        assert_eq!(plugin.latency_frames(), 1_536);
        assert_eq!(plugin.lines.len(), 2);
    }

    #[test]
    fn an_overloud_sine_never_passes_the_ceiling() {
        for ceiling_db in [-0.3f32, -3.0, -12.0] {
            let mut plugin = prepared();
            plugin.set_param_by_key("ceiling_db", ceiling_db);
            plugin.set_param_by_key("release_ms", 50.0);
            let ceiling = db_to_gain(ceiling_db);

            let mut buffer = sine(48_000, 220.0, 2.0);
            plugin.process(&mut buffer, &context(48_000));
            assert!(
                buffer.peak() <= ceiling,
                "ceiling {ceiling_db} dB: peak {} exceeded {ceiling}",
                buffer.peak()
            );
            // It must actually reach the ceiling rather than over-duck the whole signal.
            let settled = buffer.slice(24_000, 24_000).peak();
            assert!(
                gain_to_db(settled) > ceiling_db - 1.0,
                "settled at {} dB, ceiling {ceiling_db} dB",
                gain_to_db(settled)
            );
        }
    }

    #[test]
    fn limiting_is_gain_reduction_rather_than_clipping() {
        // `an_overloud_sine_never_passes_the_ceiling` would also pass for a plain hard clipper,
        // so pin down *how* the ceiling is met: a limiter that reduces gain lets the waveform
        // touch the ceiling only at its peaks, while a clipper flattens the top of every cycle.
        let mut plugin = prepared();
        plugin.set_param_by_key("ceiling_db", -0.3);
        plugin.set_param_by_key("release_ms", 50.0);
        let ceiling = db_to_gain(-0.3);

        // 4x over the ceiling: a clipper would pin |sin| >= 0.24, i.e. 84 % of every cycle.
        let mut buffer = sine(48_000, 220.0, 4.0);
        plugin.process(&mut buffer, &context(48_000));

        let pinned = buffer.channel(0)[24_000..]
            .iter()
            .filter(|s| s.abs() >= ceiling)
            .count();
        // 220 Hz for half a second is 110 cycles, so a clean limiter touches the ceiling about
        // 220 times. A clipper would pin more than 20 000 of these 24 000 samples.
        assert!(
            pinned < 2_000,
            "{pinned} of 24 000 samples sat at the ceiling; the gain stage is clipping"
        );
        assert!(
            buffer.slice(24_000, 24_000).peak() > ceiling * 0.99,
            "the limiter never reached its ceiling at all"
        );
    }

    #[test]
    fn isolated_spikes_never_pass_the_ceiling() {
        let mut plugin = prepared();
        plugin.set_param_by_key("ceiling_db", -0.3);
        let ceiling = db_to_gain(-0.3);

        let mut buffer = AudioBuffer::stereo(16_384, SR);
        for channel in buffer.channels_mut() {
            for (n, sample) in channel.iter_mut().enumerate() {
                *sample = match n % 1_000 {
                    0 => 8.0,
                    500 => -6.0,
                    _ => 0.1,
                };
            }
        }
        plugin.process(&mut buffer, &context(16_384));
        assert!(
            buffer.peak() <= ceiling,
            "peak {} exceeded {ceiling}",
            buffer.peak()
        );
        assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_non_finite_input_cannot_cross_the_lookahead_line() {
        let mut plugin = prepared();
        let mut buffer = AudioBuffer::stereo(512, SR);
        buffer.channel_mut(0)[10] = f32::NAN;
        buffer.channel_mut(1)[20] = f32::INFINITY;

        plugin.process(&mut buffer, &context(512));

        assert!(
            buffer
                .iter_channels()
                .flatten()
                .all(|sample| sample.is_finite())
        );
    }

    #[test]
    fn the_gain_starts_falling_before_the_peak_arrives() {
        let mut plugin = prepared();
        plugin.set_param_by_key("ceiling_db", 0.0);
        let lookahead = plugin.latency_frames();

        let spike_at = 1_000;
        let mut buffer = AudioBuffer::stereo(4_096, SR);
        for channel in buffer.channels_mut() {
            channel.fill(0.5);
            channel[spike_at] = 4.0;
        }
        plugin.process(&mut buffer, &context(4_096));

        // Output index `spike_at` carries input index `spike_at - lookahead`, which is a plain
        // 0.5 sample. Look-ahead means it is already ducked.
        let early = buffer.channel(0)[spike_at];
        assert!(early < 0.5, "gain had not started falling: {early}");
        assert!(early > 0.0, "gain collapsed entirely: {early}");
        // The spike itself lands `lookahead` frames later and is fully controlled.
        assert!(buffer.channel(0)[spike_at + lookahead] <= 1.0);
        assert!(buffer.peak() <= 1.0, "peak was {}", buffer.peak());
    }

    #[test]
    fn quiet_material_passes_through_delayed_but_untouched() {
        let mut plugin = prepared();
        let lookahead = plugin.latency_frames();
        let input = sine(4_096, 220.0, 0.25);
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(4_096));

        for n in lookahead..4_096 {
            let expected = input.channel(0)[n - lookahead];
            assert!(
                (buffer.channel(0)[n] - expected).abs() < 1e-6,
                "sample {n}: {} != {expected}",
                buffer.channel(0)[n]
            );
        }
    }

    #[test]
    fn silence_stays_silent() {
        let mut plugin = prepared();
        plugin.set_param_by_key("input_db", 24.0);
        let mut buffer = AudioBuffer::stereo(4_096, SR);
        plugin.process(&mut buffer, &context(4_096));
        assert_eq!(buffer.peak(), 0.0);
    }

    #[test]
    fn sliding_minimum_tracks_a_moving_window() {
        let mut minimum = SlidingMinimum::new(3);
        assert_eq!(minimum.push(5.0), 5.0);
        assert_eq!(minimum.push(3.0), 3.0);
        assert_eq!(minimum.push(4.0), 3.0);
        // 5.0 has now left the window.
        assert_eq!(minimum.push(6.0), 3.0);
        // 3.0 has now left the window.
        assert_eq!(minimum.push(7.0), 4.0);
        assert_eq!(minimum.push(8.0), 6.0);
    }

    #[test]
    fn moving_average_returns_the_mean_of_its_window() {
        let mut average = MovingAverage::new(4);
        assert_eq!(average.push(1.0), 1.0);
        assert_eq!(average.push(3.0), 2.0);
        assert_eq!(average.push(2.0), 2.0);
        assert_eq!(average.push(2.0), 2.0);
        // The first 1.0 drops out: (3 + 2 + 2 + 5) / 4.
        assert_eq!(average.push(5.0), 3.0);
    }
}
