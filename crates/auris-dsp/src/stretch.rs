//! Time stretching: making a recording longer or shorter without moving its pitch.
//!
//! What a tempo change needs. Playing a file faster is a resampling away and it takes the pitch
//! with it, which is a different instrument rather than the same one in a quicker piece — so the
//! material has to be cut into overlapping windows and laid down again at a different spacing.
//! Each window keeps its own waveform, so every period inside it is the length it always was, and
//! only the number of them per second changes.
//!
//! The method is **WSOLA**: overlap-add with a search. A naive overlap-add lays each window down
//! at a fixed distance and lets the phases fall where they may, which cancels — a stretched sine
//! comes out with holes in it. WSOLA instead looks a few milliseconds either side of where a
//! window would nominally be taken from and picks the position whose waveform best continues what
//! was written last, so the pieces are joined where they already agree.
//!
//! # Not a realtime path
//!
//! Nothing here may be called from `process`. It allocates the whole output, and the search costs
//! tens of operations per output sample. It is meant to be run once when a clip's stretch changes
//! and the result kept — see `auris_session`'s render bank, which is where that keeping happens.

use auris_core::AudioBuffer;
use auris_core::project::{MAX_STRETCH, MIN_STRETCH};
use ndarray::ArrayView1;
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// How long a window is, in seconds.
///
/// Fifty milliseconds holds several periods of anything above 60 Hz, which is what lets the search
/// find a join, and is short enough that a drum hit is not smeared across two of them.
const WINDOW_SECONDS: f64 = 0.050;

/// How far either side of its nominal position a window may be taken from, in seconds.
///
/// Ten milliseconds is a whole period at 100 Hz, so the search can always reach a matching phase
/// of the lowest pitch this is likely to be asked about.
const SEARCH_SECONDS: f64 = 0.010;

/// How far the coarse pass steps, in samples of the decimated guide.
///
/// The search is the whole cost of the method, so it runs twice: once over a signal decimated by
/// [`DECIMATION`], then once at full rate over the few samples either side of what that found.
const DECIMATION: usize = 4;

/// The shortest window worth splicing at, in samples.
const MIN_WINDOW: usize = 64;

/// Stretches `input` to `ratio` times its length, keeping its pitch.
///
/// A ratio above one makes it longer — 2.0 is half speed without the octave drop — and one below
/// makes it shorter. The ratio is clamped to [`MIN_STRETCH`]..=[`MAX_STRETCH`], and a ratio of one
/// (or a buffer too short to cut into windows) is a copy.
///
/// The first and last few samples fade, because an overlap-add has nothing to overlap with at its
/// own ends. It is a handful of samples rather than a window's worth — the windows are divided
/// back out by how much of them landed on each sample — and it sits under whatever fade the clip
/// itself carries.
pub fn time_stretch(input: &AudioBuffer, ratio: f64) -> AudioBuffer {
    let frames = input.frame_count();
    let channels = input.channel_count();
    if !ratio.is_finite() || channels == 0 || frames == 0 {
        return input.clone();
    }
    let ratio = ratio.clamp(MIN_STRETCH, MAX_STRETCH);
    if (ratio - 1.0).abs() < 1e-6 {
        return input.clone();
    }
    let out_frames = ((frames as f64) * ratio).round().max(1.0) as usize;

    // Windows have to fit twice over, or there is nothing to search among.
    let window = window_frames(input.sample_rate()).min(frames / 4) & !1;
    if window < MIN_WINDOW {
        return truncated(input, out_frames);
    }
    let hop_out = window / 2;
    let hop_in = hop_out as f64 / ratio;
    let search = (SEARCH_SECONDS * input.sample_rate()).round().max(1.0) as usize;

    let guide = mono_guide(input);
    let coarse: Vec<f32> = guide.iter().step_by(DECIMATION).copied().collect();
    let guide_energy = squared_prefix(&guide);
    let coarse_energy = squared_prefix(&coarse);
    let search_guide = SearchGuide {
        full: &guide,
        full_energy: &guide_energy,
        coarse: &coarse,
        coarse_energy: &coarse_energy,
    };
    let shape = hann(window);
    let coarse_candidates = search
        .saturating_mul(2)
        .checked_div(DECIMATION)
        .and_then(|count| count.checked_add(1));
    let mut correlation =
        coarse_candidates.and_then(|count| FftCorrelation::new(hop_out / DECIMATION, count));

    let mut out = vec![vec![0.0f32; out_frames]; channels];
    // How much window landed on each output sample. The windows are divided back out by it, which
    // is what makes the overlap exact rather than merely close — a Hann at half-overlap sums to
    // one everywhere except at the very ends of the clip, and this is what fixes those too.
    let mut laid = vec![0.0f32; out_frames];

    let last_start = frames - window;
    let mut taken = 0usize;
    let mut nominal = 0f64;
    let mut at = 0usize;
    while at < out_frames {
        let want = (nominal.round() as usize).min(last_start);
        // What the previous window would run into if the material simply carried on. The first
        // window has nothing behind it, so it is taken where it was asked for.
        let position = match at {
            0 => want,
            _ => {
                let target = (taken + hop_out).min(last_start);
                best_match(
                    search_guide,
                    correlation.as_mut(),
                    target,
                    want,
                    search,
                    hop_out,
                    last_start,
                )
            }
        };
        let span = window.min(out_frames - at);
        for (channel, buffer) in out.iter_mut().enumerate() {
            let source = input.channel(channel);
            for offset in 0..span {
                buffer[at + offset] += source[position + offset] * shape[offset];
            }
        }
        for offset in 0..span {
            laid[at + offset] += shape[offset];
        }
        taken = position;
        nominal += hop_in;
        at += hop_out;
    }

    for buffer in out.iter_mut() {
        for (sample, weight) in buffer.iter_mut().zip(laid.iter()) {
            // Below a thousandth there is no signal to recover, only the noise of dividing by
            // nearly nothing — those samples are the ends of the clip and stay as they are.
            if *weight > 1.0e-3 {
                *sample /= *weight;
            }
        }
    }

    AudioBuffer::from_planar(out, input.sample_rate()).unwrap_or_else(|_| input.clone())
}

/// A window length in samples, at least [`MIN_WINDOW`] and always even.
fn window_frames(sample_rate: f64) -> usize {
    let rate = sample_rate.max(1.0);
    ((WINDOW_SECONDS * rate).round() as usize).max(MIN_WINDOW) & !1
}

/// `input` cut or padded to `frames`, for material too short to splice.
///
/// A clip of a few dozen samples has no pitch to protect: whatever it is, it is over in under a
/// millisecond, and cutting it is both honest and inaudible.
fn truncated(input: &AudioBuffer, frames: usize) -> AudioBuffer {
    let channels = input
        .iter_channels()
        .map(|source| {
            let mut channel = vec![0.0f32; frames];
            let span = frames.min(source.len());
            channel[..span].copy_from_slice(&source[..span]);
            channel
        })
        .collect();
    AudioBuffer::from_planar(channels, input.sample_rate()).unwrap_or_else(|_| input.clone())
}

/// One signal to search along, whatever the channel count.
///
/// The same offset is used for every channel, so the position has to be chosen from something
/// that holds all of them: a stereo pair searched separately would drift apart by a few
/// milliseconds per window, which is the image collapsing.
fn mono_guide(input: &AudioBuffer) -> Vec<f32> {
    let channels = input.channel_count().max(1);
    let mut guide = vec![0.0f32; input.frame_count()];
    for channel in input.iter_channels() {
        for (sum, sample) in guide.iter_mut().zip(channel.iter()) {
            *sum += *sample;
        }
    }
    let scale = 1.0 / channels as f32;
    for sample in guide.iter_mut() {
        *sample *= scale;
    }
    guide
}

/// A Hann window of `frames` samples.
fn hann(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|index| {
            let phase = std::f64::consts::TAU * index as f64 / frames as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

/// Where to take the next window from: the position near `want` that best continues `target`.
///
/// Two passes, because the search is the whole cost of the method. The first walks a signal
/// decimated by [`DECIMATION`] across the whole search range; the second looks at every sample
/// within one decimated step of what it found.
#[derive(Clone, Copy)]
struct SearchGuide<'a> {
    full: &'a [f32],
    full_energy: &'a [f64],
    coarse: &'a [f32],
    coarse_energy: &'a [f64],
}

fn best_match(
    guide: SearchGuide<'_>,
    correlation: Option<&mut FftCorrelation>,
    target: usize,
    want: usize,
    search: usize,
    overlap: usize,
    last_start: usize,
) -> usize {
    let low = want.saturating_sub(search);
    let high = (want + search).min(last_start);
    if low >= high {
        return want.min(last_start);
    }
    let coarse_overlap = overlap / DECIMATION;
    let candidate_count = (high - low) / DECIMATION + 1;
    let best = correlation
        .and_then(|correlation| {
            correlation.best_offset(
                guide.coarse,
                guide.coarse_energy,
                target / DECIMATION,
                low / DECIMATION,
                candidate_count,
            )
        })
        .map(|offset| low + offset * DECIMATION)
        .unwrap_or_else(|| {
            let mut best = want.min(last_start);
            let mut best_score = f32::MIN;
            let mut candidate = low;
            while candidate <= high {
                let score = similarity(
                    guide.coarse,
                    guide.coarse_energy,
                    target / DECIMATION,
                    candidate / DECIMATION,
                    coarse_overlap,
                );
                if score > best_score {
                    best_score = score;
                    best = candidate;
                }
                candidate += DECIMATION;
            }
            best
        });
    let low = best.saturating_sub(DECIMATION).max(low);
    let high = (best + DECIMATION).min(high);
    let mut refined = best;
    let mut refined_score = f32::MIN;
    for candidate in low..=high {
        let score = similarity(guide.full, guide.full_energy, target, candidate, overlap);
        if score > refined_score {
            refined_score = score;
            refined = candidate;
        }
    }
    refined
}

/// A planned real FFT used to correlate one target with every coarse candidate at once.
struct FftCorrelation {
    overlap: usize,
    transform_len: usize,
    forward: std::sync::Arc<dyn RealToComplex<f32>>,
    inverse: std::sync::Arc<dyn ComplexToReal<f32>>,
    target: Vec<f32>,
    source: Vec<f32>,
    target_spectrum: Vec<Complex<f32>>,
    source_spectrum: Vec<Complex<f32>>,
    forward_scratch: Vec<Complex<f32>>,
    inverse_scratch: Vec<Complex<f32>>,
    correlation: Vec<f32>,
}

impl FftCorrelation {
    /// Plans enough room for `candidate_count` windows of `overlap` samples.
    fn new(overlap: usize, candidate_count: usize) -> Option<Self> {
        if overlap == 0 || candidate_count == 0 {
            return None;
        }
        let segment_len = candidate_count.checked_add(overlap)?.checked_sub(1)?;
        let convolution_len = segment_len.checked_add(overlap)?.checked_sub(1)?;
        let transform_len = convolution_len.checked_next_power_of_two()?;
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(transform_len);
        let inverse = planner.plan_fft_inverse(transform_len);
        Some(Self {
            overlap,
            transform_len,
            target: forward.make_input_vec(),
            source: forward.make_input_vec(),
            target_spectrum: forward.make_output_vec(),
            source_spectrum: forward.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            inverse_scratch: inverse.make_scratch_vec(),
            correlation: inverse.make_output_vec(),
            forward,
            inverse,
        })
    }

    /// Index of the strongest candidate, or `None` when the fixed plan cannot represent it.
    fn best_offset(
        &mut self,
        signal: &[f32],
        squared_prefix: &[f64],
        target: usize,
        first_candidate: usize,
        candidate_count: usize,
    ) -> Option<usize> {
        let segment_len = candidate_count.checked_add(self.overlap)?.checked_sub(1)?;
        if candidate_count == 0
            || target.checked_add(self.overlap)? > signal.len()
            || first_candidate.checked_add(segment_len)? > signal.len()
            || squared_prefix.len() != signal.len() + 1
            || segment_len.checked_add(self.overlap)?.checked_sub(1)? > self.transform_len
        {
            return None;
        }

        self.target.fill(0.0);
        self.source.fill(0.0);
        for (to, from) in self
            .target
            .iter_mut()
            .take(self.overlap)
            .zip(signal[target..target + self.overlap].iter().rev())
        {
            *to = *from;
        }
        self.source[..segment_len]
            .copy_from_slice(&signal[first_candidate..first_candidate + segment_len]);
        // Convolution with the reversed target puts candidate `offset` at
        // `overlap - 1 + offset` in the inverse transform.
        self.forward
            .process_with_scratch(
                &mut self.target,
                &mut self.target_spectrum,
                &mut self.forward_scratch,
            )
            .ok()?;
        self.forward
            .process_with_scratch(
                &mut self.source,
                &mut self.source_spectrum,
                &mut self.forward_scratch,
            )
            .ok()?;
        for (target, source) in self.target_spectrum.iter_mut().zip(&self.source_spectrum) {
            *target *= *source;
        }
        self.inverse
            .process_with_scratch(
                &mut self.target_spectrum,
                &mut self.correlation,
                &mut self.inverse_scratch,
            )
            .ok()?;

        let scale = 1.0 / self.transform_len as f32;
        let mut best = 0usize;
        let mut best_score = f32::MIN;
        for offset in 0..candidate_count {
            let candidate = first_candidate + offset;
            let energy = squared_prefix[candidate + self.overlap] - squared_prefix[candidate];
            let dot = self.correlation[self.overlap - 1 + offset] * scale;
            let score = if energy > 1.0e-12 {
                dot / energy.sqrt() as f32
            } else {
                0.0
            };
            if score > best_score {
                best_score = score;
                best = offset;
            }
        }
        Some(best)
    }
}

/// How alike two stretches of `signal` are, from -1 to 1.
///
/// Normalised, so a quiet passage that matches beats a loud one that does not. A stretch running
/// off the end of the signal scores nothing rather than being clamped into range, which would
/// make every candidate near the end look identical.
fn similarity(
    signal: &[f32],
    squared_prefix: &[f64],
    target: usize,
    candidate: usize,
    overlap: usize,
) -> f32 {
    if target + overlap > signal.len() || candidate + overlap > signal.len() || overlap == 0 {
        return f32::MIN;
    }
    let a = ArrayView1::from(&signal[target..target + overlap]);
    let b = ArrayView1::from(&signal[candidate..candidate + overlap]);
    let dot = a.dot(&b);
    let energy = squared_prefix[candidate + overlap] - squared_prefix[candidate];
    match energy > 1.0e-12 {
        true => dot / energy.sqrt() as f32,
        // Silence continues anything equally well, and saying so keeps the search from preferring
        // it to a real match.
        false => 0.0,
    }
}

/// Cumulative squared energy, so every candidate window is normalised in constant time.
fn squared_prefix(signal: &[f32]) -> Vec<f64> {
    let mut sum = 0.0f64;
    std::iter::once(0.0)
        .chain(signal.iter().map(|sample| {
            sum += f64::from(*sample) * f64::from(*sample);
            sum
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_windows_are_always_even() {
        assert_eq!(window_frames(44_100.0), 2_204);
        assert_eq!(window_frames(22_050.0) % 2, 0);
        assert!(window_frames(1.0) >= MIN_WINDOW);
    }

    #[test]
    fn fft_search_finds_the_same_join_as_direct_correlation() {
        let guide: Vec<f32> = (0..48_000)
            .map(|index| {
                let at = index as f32;
                (at * 0.031).sin() + 0.2 * (at * at * 0.000_007).sin()
            })
            .collect();
        let coarse: Vec<f32> = guide.iter().step_by(DECIMATION).copied().collect();
        let guide_energy = squared_prefix(&guide);
        let coarse_energy = squared_prefix(&coarse);
        let search_guide = SearchGuide {
            full: &guide,
            full_energy: &guide_energy,
            coarse: &coarse,
            coarse_energy: &coarse_energy,
        };
        let target = 12_000;
        let want = 13_000;
        let search = 480;
        let overlap = 1_200;
        let last_start = guide.len() - 2_400;
        let direct = best_match(
            search_guide,
            None,
            target,
            want,
            search,
            overlap,
            last_start,
        );
        let mut correlation =
            FftCorrelation::new(overlap / DECIMATION, search / DECIMATION * 2 + 1).unwrap();
        let transformed = best_match(
            search_guide,
            Some(&mut correlation),
            target,
            want,
            search,
            overlap,
            last_start,
        );
        assert_eq!(transformed, direct);
    }

    const RATE: f64 = 48_000.0;

    /// A sine of `hz`, `seconds` long.
    fn sine(hz: f64, seconds: f64) -> AudioBuffer {
        let frames = (RATE * seconds) as usize;
        let channel: Vec<f32> = (0..frames)
            .map(|index| (std::f64::consts::TAU * hz * index as f64 / RATE).sin() as f32 * 0.5)
            .collect();
        AudioBuffer::from_planar(vec![channel], RATE).expect("a buffer")
    }

    /// The pitch of `buffer`, in hertz, from the zero crossings of its first channel.
    ///
    /// Crude and exactly right for a sine: two crossings a period, counted over a stretch well
    /// inside the buffer so the ends do not matter.
    fn pitch(buffer: &AudioBuffer) -> f64 {
        let channel = buffer.channel(0);
        let from = channel.len() / 4;
        let to = channel.len() * 3 / 4;
        let span = &channel[from..to];
        let crossings = span
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        crossings as f64 * RATE / (2.0 * span.len() as f64)
    }

    #[test]
    fn a_stretched_tone_lasts_longer_and_sounds_the_same_note() {
        // The whole point of the exercise. Resampling would give the length and take the pitch
        // with it — an octave down at half speed — so both halves are asserted. Within one per
        // cent, which is a fifth of the smallest interval anybody can name and is also about
        // where counting zero crossings over a quarter of a second stops being able to tell.
        let source = sine(440.0, 1.0);
        for ratio in [0.5, 0.75, 1.5, 2.0] {
            let stretched = time_stretch(&source, ratio);
            let expected = (source.frame_count() as f64 * ratio).round() as usize;
            assert_eq!(
                stretched.frame_count(),
                expected,
                "a stretch of {ratio} came out the wrong length"
            );
            let heard = pitch(&stretched);
            assert!(
                (heard - 440.0).abs() < 4.4,
                "a stretch of {ratio} moved the pitch to {heard} Hz"
            );
        }
    }

    #[test]
    fn a_stretched_tone_keeps_its_level_all_the_way_through() {
        // Overlap-add's own failure: windows laid down where their phases disagree cancel, and a
        // stretched sine comes out with holes in it. The level is measured in tenths of the clip
        // so that a hole anywhere but the very ends is caught.
        let stretched = time_stretch(&sine(220.0, 1.0), 1.5);
        let channel = stretched.channel(0);
        let step = channel.len() / 10;
        for slice in 1..9 {
            let span = &channel[slice * step..(slice + 1) * step];
            let peak = span
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(
                peak > 0.4,
                "the tenth starting at {} dropped to {peak}",
                slice * step
            );
        }
    }

    #[test]
    fn both_channels_are_cut_in_the_same_places() {
        // The search reads one mono signal and every channel takes the offset it found. Searched
        // separately, a stereo pair would drift apart by milliseconds and the image would go with
        // it — so the two channels of an identical pair have to come back identical.
        let mono = sine(330.0, 0.5);
        let pair = AudioBuffer::from_planar(
            vec![mono.channel(0).to_vec(), mono.channel(0).to_vec()],
            RATE,
        )
        .expect("a pair");
        let stretched = time_stretch(&pair, 1.75);
        assert_eq!(stretched.channel_count(), 2);
        assert_eq!(
            stretched.channel(0),
            stretched.channel(1),
            "the two channels were cut in different places"
        );
    }

    #[test]
    fn a_ratio_of_one_and_a_ratio_of_nonsense_leave_the_material_alone() {
        let source = sine(440.0, 0.25);
        for ratio in [1.0, f64::NAN, f64::INFINITY] {
            let same = time_stretch(&source, ratio);
            assert_eq!(same.frame_count(), source.frame_count(), "ratio {ratio}");
            assert_eq!(same.channel(0), source.channel(0), "ratio {ratio}");
        }
        // An absurd ratio is clamped rather than obeyed: the buffer is allocated from it, and a
        // tempo field with a typo in it should not ask for a gigabyte.
        let huge = time_stretch(&source, 1_000.0);
        assert_eq!(
            huge.frame_count(),
            (source.frame_count() as f64 * MAX_STRETCH) as usize
        );
    }

    #[test]
    fn silence_stretches_to_silence_and_a_scrap_is_cut_rather_than_spliced() {
        let quiet = AudioBuffer::new(2, 4_800, RATE);
        let stretched = time_stretch(&quiet, 2.0);
        assert_eq!(stretched.frame_count(), 9_600);
        assert_eq!(
            stretched.peak(),
            0.0,
            "silence came back with something in it"
        );

        // Too short to hold a window, let alone search among them. It is under a millisecond of
        // audio, and there is no pitch in it to protect.
        let scrap = AudioBuffer::new(1, 16, RATE);
        assert_eq!(time_stretch(&scrap, 2.0).frame_count(), 32);
    }
}
