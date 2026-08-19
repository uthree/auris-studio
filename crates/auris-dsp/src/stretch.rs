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
    let shape = hann(window);

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
                best_match(&guide, &coarse, target, want, search, hop_out, last_start)
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
    ((WINDOW_SECONDS * rate).round() as usize).max(MIN_WINDOW)
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
fn best_match(
    guide: &[f32],
    coarse: &[f32],
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
    let mut best = want.min(last_start);
    let mut best_score = f32::MIN;
    let coarse_overlap = overlap / DECIMATION;
    let mut candidate = low;
    while candidate <= high {
        let score = similarity(
            coarse,
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
    let low = best.saturating_sub(DECIMATION).max(low);
    let high = (best + DECIMATION).min(high);
    let mut refined = best;
    let mut refined_score = f32::MIN;
    for candidate in low..=high {
        let score = similarity(guide, target, candidate, overlap);
        if score > refined_score {
            refined_score = score;
            refined = candidate;
        }
    }
    refined
}

/// How alike two stretches of `signal` are, from -1 to 1.
///
/// Normalised, so a quiet passage that matches beats a loud one that does not. A stretch running
/// off the end of the signal scores nothing rather than being clamped into range, which would
/// make every candidate near the end look identical.
fn similarity(signal: &[f32], target: usize, candidate: usize, overlap: usize) -> f32 {
    if target + overlap > signal.len() || candidate + overlap > signal.len() || overlap == 0 {
        return f32::MIN;
    }
    let a = &signal[target..target + overlap];
    let b = &signal[candidate..candidate + overlap];
    let mut dot = 0.0f32;
    let mut energy = 0.0f32;
    for (left, right) in a.iter().zip(b.iter()) {
        dot += left * right;
        energy += right * right;
    }
    match energy > 1.0e-12 {
        true => dot / energy.sqrt(),
        // Silence continues anything equally well, and saying so keeps the search from preferring
        // it to a real match.
        false => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
