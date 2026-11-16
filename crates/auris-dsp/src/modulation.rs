//! Modulation spectra: how strongly, and at what rate, the loudness of each register pulses.
//!
//! A spectrogram says which frequencies sound when. Transforming each frequency band's
//! *envelope* once more, along time, says how that band's loudness repeats: a kick on every
//! beat puts a peak at one cycle per beat in the low octaves, sixteenth-note hats put one at
//! four cycles per beat near 8 kHz, and a held pad puts almost nothing anywhere.
//!
//! The analysis is two halves so that the time axis can be the caller's. [`band_envelopes`]
//! measures each octave band's amplitude envelope at a fixed rate in *seconds*;
//! [`modulation_spectrum`] transforms any uniformly sampled series, whatever its unit. The
//! session resamples the envelopes onto the score's beat grid between the two, which is what
//! lets the answer be read in cycles per beat even across a tempo change.

use crate::biquad::{Biquad, BiquadCoefficients};
use auris_core::AudioBuffer;
use realfft::RealFftPlanner;
use std::sync::atomic::{AtomicBool, Ordering};

/// How finely [`band_envelopes`] divides the spectrum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BandResolution {
    /// Ten octave bands, 31.25 Hz to 16 kHz, with Linkwitz–Riley fourth-order edges.
    Octave,
    /// Thirty third-octave bands, 19.7 Hz to 16 kHz, with Linkwitz–Riley eighth-order edges: a
    /// tone at the neighbouring band's centre reads about 11 dB down, two bands away 35 dB.
    #[default]
    ThirdOctave,
}

/// Butterworth fourth-order section Qs; each edge of a third-octave band is two of these.
const BUTTERWORTH_4: [f32; 2] = [0.541_196_1, 1.306_563];

impl BandResolution {
    /// Band centres, low to high, in hertz, on the base-two series through 1 kHz.
    pub fn centres(self) -> Vec<f64> {
        match self {
            Self::Octave => (-5..=4).map(|k| 1000.0 * 2.0_f64.powi(k)).collect(),
            Self::ThirdOctave => (-17..=12)
                .map(|k| 1000.0 * 2.0_f64.powf(f64::from(k) / 3.0))
                .collect(),
        }
    }

    /// Ratio from a band's centre to each of its edges.
    fn half_width(self) -> f64 {
        match self {
            Self::Octave => std::f64::consts::SQRT_2,
            Self::ThirdOctave => 2.0_f64.powf(1.0 / 6.0),
        }
    }

    /// The Q of each second-order section at one edge; an edge is -6 dB.
    fn edge_sections(self) -> &'static [f32] {
        match self {
            Self::Octave => &[
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            Self::ThirdOctave => &[
                BUTTERWORTH_4[0],
                BUTTERWORTH_4[1],
                BUTTERWORTH_4[0],
                BUTTERWORTH_4[1],
            ],
        }
    }
}

/// Envelope frames per second that [`band_envelopes`] produces.
pub const ENVELOPE_RATE: f64 = 200.0;

/// Power at or below this (about -100 dBFS) counts as silence, never as a deep modulation.
const SILENT_POWER: f64 = 1.0e-10;

/// Amplitude envelopes of the bands, all sampled at [`ENVELOPE_RATE`].
pub struct BandEnvelopes {
    /// Band centres in hertz, low to high, as [`BandResolution::centres`] gives them.
    pub centres: Vec<f64>,
    /// One amplitude series per band: the root of the band's mean power over each frame,
    /// averaged across channels.
    pub amplitude: Vec<Vec<f32>>,
    /// Each band's mean power over the whole buffer in dB (power reference 1, so a full-scale
    /// sine is -3 dB), `None` for a silent band. The edges take about 3.9 dB (octave) or 5.8 dB
    /// (third octave) from a tone at a band's centre; the levels compare bands and regions, not
    /// absolute loudness.
    pub level_db: Vec<Option<f32>>,
}

/// Halve the rate of `signal` after a fourth-order low-pass at 0.2 of the old rate.
fn decimate(signal: &mut Vec<f32>, rate: f64) {
    for q in BUTTERWORTH_4 {
        Biquad::new(BiquadCoefficients::lowpass(rate, (rate * 0.2) as f32, q))
            .process_block(signal);
    }
    let kept: Vec<f32> = signal.iter().step_by(2).copied().collect();
    *signal = kept;
}

/// Measure every band's amplitude envelope.
///
/// Each band is a band-pass with Linkwitz–Riley edges (Butterworth high-passes at the lower edge
/// and low-passes at the upper one, each edge -6 dB), so a band's skirt does not leak much of a
/// neighbour's pulse. Bands are filtered from the top down, and the signal is halved in rate
/// whenever the next band's upper edge is below an eighth of it — a third-octave analysis of four
/// minutes then costs a few seconds rather than tens. Channel powers are averaged, as in
/// [`crate::mel`], so opposite-phase stereo does not cancel. A three-tap smoothing across frames
/// keeps a band's own carrier out of the envelope's rate.
/// Returns `None` on cancellation or for empty, non-finite or unusably slow audio.
pub fn band_envelopes(
    audio: &AudioBuffer,
    resolution: BandResolution,
    cancel: &AtomicBool,
) -> Option<BandEnvelopes> {
    let rate = audio.sample_rate();
    let frames = audio.frame_count();
    let channels = audio.channel_count();
    if frames == 0 || channels == 0 || !rate.is_finite() || rate < 4.0 * ENVELOPE_RATE {
        return None;
    }
    if audio
        .iter_channels()
        .any(|channel| channel.iter().any(|sample| !sample.is_finite()))
    {
        return None;
    }
    let centres = resolution.centres();
    let hops = ((frames as f64 * ENVELOPE_RATE / rate).ceil() as usize).max(1);
    let mut power = vec![vec![0.0_f64; hops]; centres.len()];
    for channel in audio.iter_channels() {
        let mut signal = channel.to_vec();
        let mut current = rate;
        for (band, centre) in centres.iter().enumerate().rev() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let low = centre / resolution.half_width();
            let wanted = centre * resolution.half_width();
            while current / 2.0 >= (4.0 * wanted).max(4.0 * ENVELOPE_RATE) {
                decimate(&mut signal, current);
                current /= 2.0;
            }
            let high = wanted.min(current * 0.45);
            if low >= high {
                continue;
            }
            let mut filtered = signal.clone();
            for &q in resolution.edge_sections() {
                Biquad::new(BiquadCoefficients::highpass(current, low as f32, q))
                    .process_block(&mut filtered);
                Biquad::new(BiquadCoefficients::lowpass(current, high as f32, q))
                    .process_block(&mut filtered);
            }
            let length = filtered.len();
            let boundary =
                |hop: usize| ((hop as f64 * current / ENVELOPE_RATE).round() as usize).min(length);
            for (hop, slot) in power[band].iter_mut().enumerate() {
                let block = &filtered[boundary(hop)..boundary(hop + 1)];
                if !block.is_empty() {
                    let energy: f64 = block.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
                    *slot += energy / block.len() as f64 / channels as f64;
                }
            }
        }
    }
    let mut amplitude = Vec::with_capacity(centres.len());
    let mut level_db = Vec::with_capacity(centres.len());
    for power in &power {
        let mean = power.iter().sum::<f64>() / hops as f64;
        level_db.push((mean > SILENT_POWER).then(|| (10.0 * mean.log10()) as f32));
        let smoothed = (0..hops).map(|hop| {
            let before = power[hop.saturating_sub(1)];
            let after = power[(hop + 1).min(hops - 1)];
            (0.25 * before + 0.5 * power[hop] + 0.25 * after).sqrt() as f32
        });
        amplitude.push(smoothed.collect());
    }
    Some(BandEnvelopes {
        centres,
        amplitude,
        level_db,
    })
}

/// The modulation spectrum of one envelope, in cycles per unit of the series' own rate.
#[derive(Clone, Debug)]
pub struct ModulationSpectrum {
    /// Width of one bin, in cycles per unit.
    pub resolution: f64,
    /// Length of one analysed segment, in units; the slowest repetition it can tell from a
    /// steady level is about two cycles per segment.
    pub segment_units: f64,
    /// Segments that carried sound and were averaged.
    pub segments: usize,
    /// Modulation depth per bin from 0 cycles upwards: the swing of the envelope at that rate
    /// as a fraction of its mean, so an envelope `m·(1 + d·cos)` reads `d`.
    pub depth: Vec<f32>,
}

/// Longest Welch segment, in units of the series (128 beats is 32 bars of 4/4).
pub const MAX_SEGMENT_UNITS: f64 = 128.0;

/// Transform one uniformly sampled envelope into modulation depth per rate.
///
/// The series is cut into Hann-windowed segments of at most [`MAX_SEGMENT_UNITS`] overlapping
/// by half (Welch's method), each segment's mean removed and its spectrum divided by that mean,
/// so a loud band and a quiet one pulsing alike read alike. Depths are averaged in power over
/// the segments that sound. Each segment is zero-padded fourfold, which interpolates the bins
/// without improving the resolution [`ModulationSpectrum::segment_units`] sets.
pub fn modulation_spectrum(series: &[f32], samples_per_unit: f64) -> ModulationSpectrum {
    let total = series.len();
    let segment = total
        .min((MAX_SEGMENT_UNITS * samples_per_unit).round() as usize)
        .max(2);
    let size = segment.next_power_of_two() * 4;
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(size);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let window: Vec<f64> = (0..segment)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / segment as f64).cos())
        .collect();
    let window_sum: f64 = window.iter().sum();
    let mut accumulated = vec![0.0_f64; size / 2 + 1];
    let mut segments = 0;
    let hop = (segment / 2).max(1);
    let mut start = 0;
    loop {
        let end = (start + segment).min(total);
        let slice = &series[start.min(total)..end];
        if slice.len() == segment {
            let weighted: f64 = slice
                .iter()
                .zip(&window)
                .map(|(&v, &w)| f64::from(v) * w)
                .sum();
            let mean = weighted / window_sum;
            if mean * mean > SILENT_POWER {
                input.fill(0.0);
                for ((slot, &value), &weight) in input.iter_mut().zip(slice).zip(&window) {
                    *slot = (f64::from(value) - mean) * weight;
                }
                if fft.process(&mut input, &mut output).is_ok() {
                    for (sum, bin) in accumulated.iter_mut().zip(&output) {
                        let depth = 2.0 * bin.norm() / window_sum / mean;
                        *sum += depth * depth;
                    }
                    segments += 1;
                }
            }
        }
        if end >= total {
            break;
        }
        start += hop;
        // A last partial segment would be dropped; align one to the end instead.
        if start + segment > total {
            start = total.saturating_sub(segment);
        }
    }
    let depth = accumulated
        .iter()
        .map(|sum| (sum / segments.max(1) as f64).sqrt() as f32)
        .collect();
    ModulationSpectrum {
        resolution: samples_per_unit / size as f64,
        segment_units: segment as f64 / samples_per_unit,
        segments,
        depth,
    }
}

impl ModulationSpectrum {
    /// Depth at `rate`, linearly interpolated between bins; zero beyond the last bin.
    pub fn depth_at(&self, rate: f64) -> f32 {
        let position = rate / self.resolution;
        let index = position.floor() as usize;
        let (Some(&a), b) = (self.depth.get(index), self.depth.get(index + 1)) else {
            return 0.0;
        };
        let b = b.copied().unwrap_or(a);
        let fraction = (position - index as f64) as f32;
        a + (b - a) * fraction
    }

    /// The largest depth between `rate / (1 + tolerance)` and `rate * (1 + tolerance)`,
    /// which absorbs a little tempo drift and the bin grid's own spacing.
    pub fn depth_near(&self, rate: f64, tolerance: f64) -> f32 {
        let low = rate / (1.0 + tolerance);
        let high = rate * (1.0 + tolerance);
        let first = (low / self.resolution).ceil() as usize;
        let last = (high / self.resolution).floor() as usize;
        (first..=last)
            .filter_map(|bin| self.depth.get(bin).copied())
            .fold(self.depth_at(rate), f32::max)
    }

    /// The strongest local maxima between `low` and `high`, deepest first, at least
    /// `min_depth` deep, none within a ratio of `1.08` of a deeper one.
    pub fn peaks(&self, low: f64, high: f64, count: usize, min_depth: f32) -> Vec<(f64, f32)> {
        let first = ((low / self.resolution).ceil() as usize).max(1);
        let last =
            ((high / self.resolution).floor() as usize).min(self.depth.len().saturating_sub(2));
        let mut candidates: Vec<(f64, f32)> = (first..=last)
            .filter(|&bin| {
                let d = self.depth[bin];
                d >= min_depth && d > self.depth[bin - 1] && d >= self.depth[bin + 1]
            })
            .map(|bin| (bin as f64 * self.resolution, self.depth[bin]))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut chosen: Vec<(f64, f32)> = Vec::new();
        for (rate, depth) in candidates {
            if chosen.len() == count {
                break;
            }
            if chosen
                .iter()
                .all(|(other, _)| (rate / other).max(other / rate) > 1.08)
            {
                chosen.push((rate, depth));
            }
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f64, seconds: f64, envelope: impl Fn(f64) -> f64) -> AudioBuffer {
        let rate = 48_000.0;
        let frames = (seconds * rate) as usize;
        let samples: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f64 / rate;
                (envelope(t) * (2.0 * std::f64::consts::PI * frequency * t).sin()) as f32
            })
            .collect();
        AudioBuffer::from_planar(vec![samples.clone(), samples], rate).unwrap()
    }

    #[test]
    fn a_pure_envelope_reads_its_own_depth_at_its_own_rate() {
        let rate = 32.0;
        let series: Vec<f32> = (0..(rate as usize * 64))
            .map(|i| (1.0 + 0.5 * (2.0 * std::f64::consts::PI * i as f64 / rate).cos()) as f32)
            .collect();
        let spectrum = modulation_spectrum(&series, rate);
        assert_eq!(spectrum.segments, 1);
        assert!((spectrum.segment_units - 64.0).abs() < 1e-9);
        let depth = spectrum.depth_near(1.0, 0.02);
        assert!((depth - 0.5).abs() < 0.01, "{depth}");
        assert!(spectrum.depth_near(2.0, 0.02) < 0.01);
        assert!(spectrum.depth_near(0.5, 0.02) < 0.01);
        let peaks = spectrum.peaks(0.1, 16.0, 3, 0.05);
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].0 - 1.0).abs() < 0.01, "{peaks:?}");
    }

    #[test]
    fn a_steady_or_silent_series_has_no_modulation() {
        let steady = modulation_spectrum(&[0.3; 1024], 32.0);
        assert!(steady.depth.iter().all(|d| *d < 1e-6));
        let silent = modulation_spectrum(&[0.0; 1024], 32.0);
        assert_eq!(silent.segments, 0);
        assert!(silent.depth.iter().all(|d| *d == 0.0));
    }

    #[test]
    fn long_series_are_averaged_over_overlapping_segments() {
        let rate = 32.0;
        let series: Vec<f32> = (0..(rate as usize * 300))
            .map(|i| {
                (1.0 + 0.25 * (2.0 * std::f64::consts::PI * 4.0 * i as f64 / rate).cos()) as f32
            })
            .collect();
        let spectrum = modulation_spectrum(&series, rate);
        assert!((spectrum.segment_units - MAX_SEGMENT_UNITS).abs() < 1e-9);
        assert!(spectrum.segments >= 4, "{}", spectrum.segments);
        let depth = spectrum.depth_near(4.0, 0.02);
        assert!((depth - 0.25).abs() < 0.01, "{depth}");
    }

    #[test]
    fn a_pulsing_low_tone_modulates_only_its_own_octave() {
        // 63 Hz gated at 2 Hz: the 63 Hz band pulses, the 4 kHz band holds nothing.
        let audio = tone(
            63.0,
            8.0,
            |t| {
                if (t * 2.0).fract() < 0.5 { 0.5 } else { 0.05 }
            },
        );
        let envelopes =
            band_envelopes(&audio, BandResolution::Octave, &AtomicBool::new(false)).unwrap();
        assert_eq!(envelopes.amplitude.len(), 10);
        let frames = (8.0 * ENVELOPE_RATE) as usize;
        assert!(envelopes.amplitude.iter().all(|band| band.len() == frames));
        let level = envelopes.level_db[1].unwrap();
        // A 0.5 sine is -9 dB in power; gated half the time at 0.5 and half at 0.05, about -12,
        // less the 3.9 dB both edges take from a tone at the band's centre.
        assert!((level + 15.9).abs() < 0.5, "{level}");
        let bass = modulation_spectrum(&envelopes.amplitude[1], ENVELOPE_RATE);
        assert!(
            bass.depth_near(2.0, 0.03) > 0.6,
            "{}",
            bass.depth_near(2.0, 0.03)
        );
        let high = envelopes.level_db[7];
        // Only the gate's clicks reach 4 kHz.
        assert!(high.is_none_or(|db| db < level - 35.0), "{high:?}");
    }

    #[test]
    fn third_octaves_tell_apart_two_tones_a_third_of_an_octave_apart() {
        // 1 kHz pulsing at 4 Hz under a steady tone a third of an octave higher.
        let rate = 48_000.0;
        let samples: Vec<f32> = (0..(4.0 * rate) as usize)
            .map(|i| {
                let t = i as f64 / rate;
                let gate = if (t * 4.0).fract() < 0.5 { 0.4 } else { 0.0 };
                let pulsing = gate * (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
                let steady = 0.4 * (2.0 * std::f64::consts::PI * 1259.9 * t).sin();
                (pulsing + steady) as f32
            })
            .collect();
        let audio = AudioBuffer::from_planar(vec![samples], rate).unwrap();
        let envelopes =
            band_envelopes(&audio, BandResolution::ThirdOctave, &AtomicBool::new(false)).unwrap();
        assert_eq!(envelopes.centres.len(), 30);
        let index = |hz: f64| {
            envelopes
                .centres
                .iter()
                .position(|centre| (centre / hz - 1.0).abs() < 0.01)
                .unwrap()
        };
        let depth = |band: usize| {
            modulation_spectrum(&envelopes.amplitude[band], ENVELOPE_RATE).depth_near(4.0, 0.03)
        };
        let (pulsing, steady) = (depth(index(1000.0)), depth(index(1259.9)));
        // The steady neighbour leaks in about 11 dB down and fills the gate's gaps a little.
        assert!(pulsing > 0.5, "{pulsing}");
        assert!(steady < 0.25, "{steady}");
        // The steady tone: -11 dB, less the edges' 5.8 dB, plus the gated 1 kHz leaking in.
        let level = |hz| envelopes.level_db[index(hz)].unwrap();
        assert!((level(1259.9) + 15.0).abs() < 0.7, "{}", level(1259.9));
        // Decimated low bands stay calibrated: nothing sounds down there.
        assert!(envelopes.level_db[index(62.996)].is_none_or(|db| db < -60.0));
    }

    #[test]
    fn decimated_low_bands_read_the_same_level_as_full_rate_ones() {
        let audio = tone(50.0, 2.0, |_| 0.5);
        let envelopes =
            band_envelopes(&audio, BandResolution::ThirdOctave, &AtomicBool::new(false)).unwrap();
        let band = envelopes
            .centres
            .iter()
            .position(|centre| (centre / 49.6 - 1.0).abs() < 0.01)
            .unwrap();
        // A 0.5 sine is -9 dB; the band's edges take 5.8 dB at its centre.
        let level = envelopes.level_db[band].unwrap();
        assert!((level + 14.8).abs() < 0.6, "{level}");
    }

    #[test]
    fn cancellation_and_unusable_audio_return_nothing() {
        let audio = tone(440.0, 0.5, |_| 0.5);
        let resolution = BandResolution::ThirdOctave;
        assert!(band_envelopes(&audio, resolution, &AtomicBool::new(true)).is_none());
        let empty = AudioBuffer::stereo(0, 48_000.0);
        assert!(band_envelopes(&empty, resolution, &AtomicBool::new(false)).is_none());
    }
}
