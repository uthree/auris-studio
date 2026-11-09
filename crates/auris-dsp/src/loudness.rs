//! Programme loudness, to ITU-R BS.1770-4.
//!
//! A number for *how loud this sounds*, which is a question neither peak nor RMS answers. A peak
//! is one sample and hears nothing about the rest; an RMS weights 40 Hz the same as 3 kHz, where
//! the ear is twenty decibels more sensitive. A balance struck by either comes out wrong in the
//! same direction every time — the kick too quiet because it is peaky, the pad too loud because
//! it is not.
//!
//! The standard is three steps: filter every channel with the K-weighting, take the mean square
//! of 400 ms blocks, and average the blocks that survive two gates. The gates are what make it a
//! measure of the *programme* rather than of the tape — a piece with a quiet introduction reads
//! as loud as the same piece without one, which is the whole reason a level set from this holds
//! up across eight pieces that are not built alike.
//!
//! `auris_gpu::analysis` measures the other two, peak and true peak, over a whole render. Neither
//! replaces this and this replaces neither: a mix is *balanced* by loudness and *clipped* by
//! peaks, and a master needs both numbers before it can be moved.

use std::f64::consts::PI;

use auris_core::AudioBuffer;

use crate::biquad::{Biquad, BiquadCoefficients};
use crate::true_peak::{DETECTION_DELAY, TAPS, interval_peak};

/// How long one measurement block is, in seconds.
const HOPS_PER_BLOCK: usize = 4;

/// How far one block starts after the last, in seconds: the standard's 75 per cent overlap.
///
/// It is also the width of the running sums this is accumulated in, which is what makes the
/// overlap free — four consecutive sums are one block, and the next block drops the oldest and
/// takes the next. Summing each block from its own samples would filter the signal four times.
const HOP_SECONDS: f64 = 0.100;

/// The gate below which a block is silence rather than quiet, in LUFS.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;

/// How far below the ungated average the second gate sits, in LU.
///
/// This is what stops a rest counting as programme. Ten, and it is the one number in the standard
/// that was arrived at by listening rather than by derivation.
const RELATIVE_GATE_LU: f64 = 10.0;

/// The offset that puts a 1 kHz sine at the level it is written down as.
///
/// Not a correction for anything physical: the K-weighting has about 0.691 dB of gain at 1 kHz,
/// and this takes it back out so that a sine of amplitude 0.1 in both channels — which everybody
/// calls -20 dBFS — measures -20 LUFS. Without it the whole scale would sit two thirds of a
/// decibel away from the numbers anyone quotes.
const OFFSET_DB: f64 = -0.691;

/// The two sections of the K-weighting filter, designed for `sample_rate`.
///
/// A high shelf of +4 dB above about 1.7 kHz, which is the head and torso a microphone does not
/// have, and a high-pass at 38 Hz, which is where level stops being something the ear hears as
/// loudness. The standard tabulates the coefficients at 48 kHz only and derives them from an
/// analogue prototype; they are re-derived here because a project renders at whatever rate its
/// device runs at, and a filter designed for the wrong one is not this filter at all.
///
/// The bilinear transform is written out rather than taken from
/// [`BiquadCoefficients::high_shelf`]. The cookbook parameterises a shelf by its Q at the midpoint
/// and the standard by the shelf's own `Vh` and `Vb`, and the two disagree by a few per cent of
/// every coefficient — which is enough to miss the reference numbers the standard prints, and
/// hitting those exactly is the only evidence that this is the measure it claims to be.
pub fn k_weighting(sample_rate: f64) -> [BiquadCoefficients; 2] {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return [BiquadCoefficients::identity(); 2];
    }
    // The standard's own numbers, at the precision it prints them. The exponent on `Vb` is one of
    // them: it sets the gain at the shelf's own corner, and is near enough to a half that reading
    // it as one would look like an obvious simplification and would miss the reference.
    let shelf = {
        let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196f64);
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        BiquadCoefficients {
            b0: ((vh + vb * k / q + k * k) / a0) as f32,
            b1: (2.0 * (k * k - vh) / a0) as f32,
            b2: ((vh - vb * k / q + k * k) / a0) as f32,
            a1: (2.0 * (k * k - 1.0) / a0) as f32,
            a2: ((1.0 - k / q + k * k) / a0) as f32,
        }
    };
    let highpass = {
        let (f0, q) = (38.13547087602444, 0.5003270373238773f64);
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        BiquadCoefficients {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: (2.0 * (k * k - 1.0) / a0) as f32,
            a2: ((1.0 - k / q + k * k) / a0) as f32,
        }
    };
    [shelf, highpass]
}

/// What a block of mean square `power` measures, in LUFS.
fn block_lufs(power: f64) -> f64 {
    if power <= 0.0 {
        return f64::NEG_INFINITY;
    }
    OFFSET_DB + 10.0 * power.log10()
}

/// The mean square of every whole 400 ms block of `buffer`, K-weighted and summed over channels.
///
/// Every channel is weighted 1. The standard weights the two surround channels 1.41 and everything
/// else 1, and a project renders in stereo, so there is nothing here to weight differently — a
/// buffer with more channels than that has not come from this program.
fn block_powers(buffer: &AudioBuffer) -> Vec<f64> {
    let rate = buffer.sample_rate();
    if !rate.is_finite() || rate <= 0.0 || buffer.channel_count() == 0 {
        return Vec::new();
    }
    let hop_frames = (rate * HOP_SECONDS).round() as usize;
    let block_frames = hop_frames.saturating_mul(HOPS_PER_BLOCK);
    let per_block = HOPS_PER_BLOCK;
    if hop_frames == 0 || buffer.frame_count() < block_frames {
        return Vec::new();
    }

    // One running sum per hop, shared by every channel: the sum a block wants is four of these
    // added together, and a channel is another set of squares into the same bins.
    let hops = buffer.frame_count() / hop_frames;
    let mut sums = vec![0.0f64; hops];
    let coefficients = k_weighting(rate);
    for channel in buffer.iter_channels() {
        let mut sections = [Biquad::new(coefficients[0]), Biquad::new(coefficients[1])];
        for (hop, samples) in channel.chunks_exact(hop_frames).take(hops).enumerate() {
            let mut sum = 0.0;
            for &sample in samples {
                // A NaN or an infinity — a plugin that blew up — would poison both filters for
                // the rest of the channel and every block after it. It is counted as silence:
                // one bad sample must not decide whether a whole mix measures at all.
                let sample = if sample.is_finite() { sample } else { 0.0 };
                let filtered = sections
                    .iter_mut()
                    .fold(sample, |signal, section| section.process_sample(signal));
                sum += f64::from(filtered) * f64::from(filtered);
            }
            sums[hop] += sum;
        }
    }

    (0..hops.saturating_sub(per_block - 1))
        .map(|start| sums[start..start + per_block].iter().sum::<f64>() / block_frames as f64)
        .collect()
}

/// The average of the blocks that are programme, or `None` when none of them are.
///
/// Two gates, and they are not the same idea twice. The absolute one throws away digital silence,
/// which would otherwise drag the average of a piece down by however much of it is a rest; the
/// relative one throws away everything more than 10 LU under what is left, which is what keeps a
/// quiet introduction from being averaged in with the piece it introduces.
fn gated_mean(blocks: &[f64]) -> Option<f64> {
    let mean = |kept: &[f64]| kept.iter().sum::<f64>() / kept.len() as f64;
    let loud: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|&power| block_lufs(power) > ABSOLUTE_GATE_LUFS)
        .collect();
    if loud.is_empty() {
        return None;
    }
    let relative = block_lufs(mean(&loud)) - RELATIVE_GATE_LU;
    let programme: Vec<f64> = loud
        .into_iter()
        .filter(|&power| block_lufs(power) > relative)
        .collect();
    // The relative gate cannot empty the set — it sits 10 LU under the average of these very
    // blocks, so the loudest of them is always above it — but saying so in code is cheaper than
    // trusting it.
    (!programme.is_empty()).then(|| mean(&programme))
}

/// The integrated loudness of `buffer` in LUFS, or `None` when none of it is programme.
///
/// `None` rather than a very small number, because silence has no loudness and every caller has
/// to decide what to do about that for itself. A fader set from a measurement that does not exist
/// would turn a silent track up by however far the target happened to be.
///
/// A buffer shorter than one 400 ms block also measures nothing. That is the standard's own
/// shape — it averages whole blocks and there is no such thing as a partial one — and it is the
/// honest answer besides: a fifth of a second is a sound rather than a programme.
pub fn integrated_lufs(buffer: &AudioBuffer) -> Option<f32> {
    gated_mean(&block_powers(buffer)).map(|power| block_lufs(power) as f32)
}

/// The loudness of the `quantile` loudest of `buffer`'s blocks, in LUFS.
pub fn loudness_quantile(buffer: &AudioBuffer, quantile: f64) -> Option<f32> {
    let mut blocks = block_powers(buffer);
    if blocks.is_empty() {
        return None;
    }
    blocks.sort_by(f64::total_cmp);
    let at = ((blocks.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    Some(block_lufs(blocks[at]) as f32)
}

/// How many hops the momentary loudness spans: 400 ms.
const MOMENTARY_HOPS: usize = HOPS_PER_BLOCK;

/// How many hops the short-term loudness spans: 3 s.
const SHORT_TERM_HOPS: usize = 30;

/// How far below the short-term average the loudness range's relative gate sits, in LU.
///
/// EBU Tech 3342's number, and not the integrated measure's ten: the range is a statement about
/// how far the music moves, and a gate that close would throw away the quiet passages it is
/// supposed to be measuring.
const RANGE_RELATIVE_GATE_LU: f64 = 20.0;

/// The live loudness of a stereo stream, to EBU R128: momentary, short-term, integrated, the
/// loudness range and the true peak.
///
/// [`integrated_lufs`] answers the question for a buffer that is all there; this answers it for a
/// signal arriving a few milliseconds at a time, which is what a meter on a playing mix is. The
/// arithmetic is the same — the same K-weighting, the same 100 ms running sums, the same two
/// gates — kept as it goes rather than computed at the end, so a piece played through this and
/// the same piece rendered and measured agree.
///
/// It allocates as the integrated history grows (one number per 100 ms of programme), so it
/// belongs on a normal thread reading from a scope, never in an audio callback.
#[derive(Clone, Debug)]
pub struct LoudnessMeter {
    sample_rate: f64,
    hop_frames: usize,
    sections: [[Biquad; 2]; 2],
    /// The hop being filled: the sum of squares so far and how many frames it holds.
    hop_sum: f64,
    hop_filled: usize,
    /// The most recent complete hops' sums, newest last, as many as the short-term window needs.
    recent: std::collections::VecDeque<f64>,
    /// Every 400 ms block's mean square since the meter was reset, for the integrated loudness.
    blocks: Vec<f64>,
    /// Every 3 s window's mean square since the meter was reset, for the loudness range.
    short_terms: Vec<f64>,
    /// The last [`TAPS`] samples of each channel, for the oversampled peak between them.
    peak_windows: [[f32; TAPS]; 2],
    true_peak: f32,
}

impl LoudnessMeter {
    /// A meter for a stream at `sample_rate`, having heard nothing.
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let coefficients = k_weighting(rate);
        let channel = [Biquad::new(coefficients[0]), Biquad::new(coefficients[1])];
        Self {
            sample_rate: rate,
            hop_frames: ((rate * HOP_SECONDS).round() as usize).max(1),
            sections: [channel, channel],
            hop_sum: 0.0,
            hop_filled: 0,
            recent: std::collections::VecDeque::with_capacity(SHORT_TERM_HOPS + 1),
            blocks: Vec::new(),
            short_terms: Vec::new(),
            peak_windows: [[0.0; TAPS]; 2],
            true_peak: 0.0,
        }
    }

    /// The rate the meter was built for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Forgets everything heard: the integrated loudness, the range and the peak start again.
    pub fn reset(&mut self) {
        *self = Self::new(self.sample_rate);
    }

    /// Measures the next stretch of the stream. The two channels are paired frame by frame; a
    /// longer one has its excess ignored.
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        for (&left, &right) in left.iter().zip(right) {
            let mut sum = 0.0;
            for (channel, sample) in [left, right].into_iter().enumerate() {
                // A plugin that blew up must not poison the filters for the rest of the take.
                let sample = if sample.is_finite() { sample } else { 0.0 };
                let window = &mut self.peak_windows[channel];
                window.copy_within(1.., 0);
                window[TAPS - 1] = sample;
                self.true_peak = self
                    .true_peak
                    .max(window[DETECTION_DELAY - 1].abs())
                    .max(interval_peak(window));
                let filtered = self.sections[channel]
                    .iter_mut()
                    .fold(sample, |signal, section| section.process_sample(signal));
                sum += f64::from(filtered) * f64::from(filtered);
            }
            self.hop_sum += sum;
            self.hop_filled += 1;
            if self.hop_filled == self.hop_frames {
                self.close_hop();
            }
        }
    }

    fn close_hop(&mut self) {
        self.recent.push_back(self.hop_sum);
        if self.recent.len() > SHORT_TERM_HOPS {
            self.recent.pop_front();
        }
        self.hop_sum = 0.0;
        self.hop_filled = 0;
        if let Some(block) = self.mean_square(MOMENTARY_HOPS) {
            self.blocks.push(block);
        }
        if let Some(window) = self.mean_square(SHORT_TERM_HOPS) {
            self.short_terms.push(window);
        }
    }

    /// The mean square of the newest `hops` complete hops, once that many have been heard.
    fn mean_square(&self, hops: usize) -> Option<f64> {
        (self.recent.len() >= hops).then(|| {
            self.recent.iter().rev().take(hops).sum::<f64>() / (hops * self.hop_frames) as f64
        })
    }

    /// The loudness of `power`, or nothing for silence under the absolute gate.
    fn reading(power: f64) -> Option<f32> {
        let lufs = block_lufs(power);
        (lufs > ABSOLUTE_GATE_LUFS).then_some(lufs as f32)
    }

    /// The last 400 ms, in LUFS; nothing until that much has been heard or while it is silent.
    pub fn momentary(&self) -> Option<f32> {
        self.mean_square(MOMENTARY_HOPS).and_then(Self::reading)
    }

    /// The last 3 s, in LUFS; nothing until that much has been heard or while it is silent.
    pub fn short_term(&self) -> Option<f32> {
        self.mean_square(SHORT_TERM_HOPS).and_then(Self::reading)
    }

    /// Everything since the last reset, gated as [`integrated_lufs`] gates it, in LUFS.
    pub fn integrated(&self) -> Option<f32> {
        gated_mean(&self.blocks).map(|power| block_lufs(power) as f32)
    }

    /// The loudness range since the last reset, in LU: the spread between the 10th and the 95th
    /// percentile of the gated short-term loudness, as EBU Tech 3342 defines it.
    pub fn range(&self) -> Option<f32> {
        let loud: Vec<f64> = self
            .short_terms
            .iter()
            .copied()
            .filter(|&power| block_lufs(power) > ABSOLUTE_GATE_LUFS)
            .collect();
        if loud.is_empty() {
            return None;
        }
        let mean = loud.iter().sum::<f64>() / loud.len() as f64;
        let gate = block_lufs(mean) - RANGE_RELATIVE_GATE_LU;
        let mut kept: Vec<f64> = loud
            .into_iter()
            .map(block_lufs)
            .filter(|&lufs| lufs > gate)
            .collect();
        if kept.is_empty() {
            return None;
        }
        kept.sort_by(f64::total_cmp);
        let at = |quantile: f64| kept[((kept.len() - 1) as f64 * quantile).round() as usize];
        Some((at(0.95) - at(0.10)) as f32)
    }

    /// The highest four-times oversampled peak since the last reset, in dBTP; nothing before any
    /// signal has been heard.
    pub fn true_peak_db(&self) -> Option<f32> {
        (self.true_peak > 0.0).then(|| 20.0 * self.true_peak.log10())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer of `channels` identical sine channels at `hz`, `seconds` long.
    fn sine(hz: f64, amplitude: f32, seconds: f64, rate: f64, channels: usize) -> AudioBuffer {
        let frames = (rate * seconds) as usize;
        let mut buffer = AudioBuffer::new(channels, frames, rate);
        for channel in buffer.iter_channels_mut() {
            for (frame, sample) in channel.iter_mut().enumerate() {
                *sample = amplitude * (2.0 * PI * hz * frame as f64 / rate).sin() as f32;
            }
        }
        buffer
    }

    #[test]
    fn a_few_non_finite_samples_neither_silence_the_measure_nor_panic_it() {
        let clean = sine(1_000.0, 0.5, 3.0, 48_000.0, 2);
        let mut broken = clean.clone();
        broken.channel_mut(0)[10_000] = f32::NAN;
        broken.channel_mut(1)[50_000] = f32::INFINITY;
        broken.channel_mut(0)[90_000] = f32::NEG_INFINITY;

        let expected = integrated_lufs(&clean).unwrap();
        let measured = integrated_lufs(&broken).expect("a loud mix still measures");
        assert!(
            (measured - expected).abs() < 0.1,
            "{measured} vs {expected}"
        );
        let quantile = loudness_quantile(&broken, 0.9).expect("and does not panic");
        assert!(quantile.is_finite());
    }

    #[test]
    fn the_filter_is_the_one_the_standard_prints() {
        // BS.1770-4, table 1 and table 2: the coefficients at 48 kHz, which is the only rate it
        // tabulates. Everything else here is a derivation from the same prototype, and this is
        // the one place it can be checked against a number somebody else wrote down.
        let [shelf, highpass] = k_weighting(48_000.0);
        for (measured, wanted, name) in [
            (shelf.b0, 1.53512485958697, "shelf b0"),
            (shelf.b1, -2.69169618940638, "shelf b1"),
            (shelf.b2, 1.19839281085285, "shelf b2"),
            (shelf.a1, -1.69065929318241, "shelf a1"),
            (shelf.a2, 0.73248077421585, "shelf a2"),
            (highpass.a1, -1.99004745483398, "highpass a1"),
            (highpass.a2, 0.99007225036621, "highpass a2"),
        ] {
            assert!(
                (f64::from(measured) - wanted).abs() < 1.0e-6,
                "{name} came out {measured} against the standard's {wanted}"
            );
        }
        assert_eq!((highpass.b0, highpass.b1, highpass.b2), (1.0, -2.0, 1.0));
    }

    #[test]
    fn a_non_standard_rate_still_builds_each_block_from_four_whole_hops() {
        let rate = 44_055.0;
        let hop = (rate * HOP_SECONDS).round() as usize;
        let buffer = AudioBuffer::new(1, hop * HOPS_PER_BLOCK, rate);

        assert_eq!(block_powers(&buffer).len(), 1);
    }

    #[test]
    fn a_kilohertz_sine_reads_the_level_it_is_written_as() {
        // The standard's own calibration, and the reason for the -0.691: a 1 kHz sine of amplitude
        // 0.1 in both channels of a stereo pair is what everybody calls -20 dBFS, and it has to
        // measure -20 LUFS. A tenth of a decibel, which is finer than any two meters agree to.
        let measured =
            integrated_lufs(&sine(1000.0, 0.1, 5.0, 48_000.0, 2)).expect("a sine is loud");
        assert!(
            (measured - -20.0).abs() < 0.1,
            "a -20 dBFS sine measured {measured:.2} LUFS"
        );
    }

    #[test]
    fn twice_the_amplitude_is_six_more() {
        let quiet = integrated_lufs(&sine(1000.0, 0.1, 5.0, 48_000.0, 2)).expect("a sine is loud");
        let loud = integrated_lufs(&sine(1000.0, 0.2, 5.0, 48_000.0, 2)).expect("a sine is loud");
        assert!(
            (loud - quiet - 6.02).abs() < 0.05,
            "doubling the amplitude moved the measurement by {:.2} LU",
            loud - quiet
        );
    }

    #[test]
    fn the_same_signal_measures_the_same_at_any_rate() {
        // The filter is re-derived per rate for exactly this, and it is the failure the tabulated
        // 48 kHz coefficients would produce silently: a project rendered at 44.1 would be measured
        // through a filter designed for something else and would come out a little too loud.
        let at = |rate| integrated_lufs(&sine(1000.0, 0.1, 5.0, rate, 2)).expect("a sine is loud");
        let (low, high) = (at(44_100.0), at(96_000.0));
        assert!(
            (low - high).abs() < 0.05,
            "44.1 kHz measured {low:.2} LUFS against 96 kHz at {high:.2}"
        );
    }

    #[test]
    fn silence_has_no_loudness() {
        assert_eq!(
            integrated_lufs(&AudioBuffer::stereo(48_000, 48_000.0)),
            None
        );
        // And so does a buffer with nothing in it to make one block from.
        assert_eq!(integrated_lufs(&sine(1000.0, 0.5, 0.2, 48_000.0, 2)), None);
    }

    #[test]
    fn a_quiet_introduction_does_not_drag_the_piece_down() {
        // Ten seconds at -20 and ten at -50, which is what a piece that fades in out of nothing
        // looks like to a meter. Averaged flat it would read about 3 LU low; gated, the quiet half
        // is 30 LU under the loud one and falls outside the relative gate entirely.
        let mut piece = sine(1000.0, 0.1, 20.0, 48_000.0, 2);
        for channel in piece.iter_channels_mut() {
            for sample in channel.iter_mut().take(10 * 48_000) {
                *sample /= 31.6;
            }
        }
        let measured = integrated_lufs(&piece).expect("half of it is loud");
        assert!(
            (measured - -20.0).abs() < 0.2,
            "the introduction pulled the measurement to {measured:.2} LUFS"
        );
    }

    /// Feeds `buffer` to a meter in blocks of `block` frames, as a scope would deliver it.
    fn streamed(buffer: &AudioBuffer, block: usize) -> LoudnessMeter {
        let mut meter = LoudnessMeter::new(buffer.sample_rate());
        let (left, right) = (buffer.channel(0), buffer.channel(1));
        for start in (0..left.len()).step_by(block) {
            let end = (start + block).min(left.len());
            meter.push(&left[start..end], &right[start..end]);
        }
        meter
    }

    #[test]
    fn the_live_meter_reads_a_calibration_sine_on_every_scale() {
        let meter = streamed(&sine(1000.0, 0.1, 5.0, 48_000.0, 2), 512);
        for (name, value) in [
            ("momentary", meter.momentary()),
            ("short-term", meter.short_term()),
            ("integrated", meter.integrated()),
        ] {
            let value = value.expect("a sine is loud");
            assert!((value - -20.0).abs() < 0.1, "{name} read {value:.2} LUFS");
        }
        let peak = meter.true_peak_db().unwrap();
        assert!((peak - -20.0).abs() < 0.1, "true peak read {peak:.2} dBTP");
        let range = meter.range().unwrap();
        assert!(
            range.abs() < 0.1,
            "a steady tone has no range, read {range:.2} LU"
        );
    }

    #[test]
    fn the_live_meter_agrees_with_the_offline_measure_whatever_the_block_size() {
        let mut piece = sine(1000.0, 0.1, 20.0, 44_100.0, 2);
        for channel in piece.iter_channels_mut() {
            for sample in channel.iter_mut().take(10 * 44_100) {
                *sample /= 31.6;
            }
        }
        let offline = integrated_lufs(&piece).unwrap();
        for block in [64, 441, 1000, 4096] {
            let live = streamed(&piece, block).integrated().unwrap();
            assert!(
                (live - offline).abs() < 0.01,
                "blocks of {block}: {live:.3} against {offline:.3}"
            );
        }
    }

    #[test]
    fn the_range_spans_the_loud_and_quiet_halves() {
        // Ten seconds at -20 and ten at -30: the range is the ten decibels between them, which is
        // what Tech 3342's own first test signal is built to read.
        let mut piece = sine(1000.0, 0.1, 20.0, 48_000.0, 2);
        for channel in piece.iter_channels_mut() {
            for sample in channel.iter_mut().take(10 * 48_000) {
                *sample /= 10f32.powf(10.0 / 20.0);
            }
        }
        let range = streamed(&piece, 1024).range().unwrap();
        assert!((range - 10.0).abs() < 0.2, "range read {range:.2} LU");
    }

    #[test]
    fn silence_reads_nothing_and_a_reset_forgets() {
        let mut meter = LoudnessMeter::new(48_000.0);
        meter.push(&[0.0; 48_000], &[0.0; 48_000]);
        assert_eq!(meter.momentary(), None);
        assert_eq!(meter.integrated(), None);
        assert_eq!(meter.true_peak_db(), None);
        let loud = sine(1000.0, 0.5, 1.0, 48_000.0, 2);
        meter.push(loud.channel(0), loud.channel(1));
        assert!(meter.integrated().is_some());
        meter.reset();
        assert_eq!(meter.integrated(), None);
        assert_eq!(meter.true_peak_db(), None);
        assert_eq!(meter.sample_rate(), 48_000.0);
    }

    #[test]
    fn the_true_peak_finds_the_crest_between_samples() {
        // A sine at a quarter of the rate, phased so every sample lands 3 dB under its crest:
        // a sample peak meter reads -3 dBFS of a signal that will clip a converter at 0.
        let samples: Vec<f32> = (0..4800)
            .map(|i| (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin())
            .collect();
        let mut meter = LoudnessMeter::new(48_000.0);
        meter.push(&samples, &samples);
        let peak = meter.true_peak_db().unwrap();
        assert!((-0.1..0.5).contains(&peak), "true peak read {peak:.2} dBTP");
    }
}
