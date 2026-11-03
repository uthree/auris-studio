//! The pitch a single sounding note actually comes out at.
//!
//! YIN (de Cheveigné & Kawahara, 2002) on a handful of frames from the steady part of the note.
//! The note is read alone, so a frame never chooses between two notes, and the search is held to
//! a fifth either side of the written pitch, which is what keeps the cost to a few frames' work
//! instead of a whole spectrogram's. An instrument an octave away from its written pitch still
//! measures: the period of the written note is a whole number of the sounding one's.
//!
//! WORLD's estimators, through the Rust port `world-dsp`, were the first choice here. Measured
//! on plain tones at 24 kHz its Harvest found no pitch at all in a sine and halved A4, and its
//! DIO with StoneMask read 220 Hz as 239; this module is what the lint needs until those agree
//! with the C++ originals.

/// Frames measured per note.
const FRAMES: usize = 8;
/// The part of a note that is measured: past the attack, short of the release.
const MEASURED: (f64, f64) = (0.2, 0.9);
/// The YIN threshold on the normalised difference: below it, a dip is a period.
const THRESHOLD: f32 = 0.15;
/// How far from the written pitch the search reaches, as a frequency ratio: a fifth.
const REACH: f32 = 1.5;

/// What came out of one note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotePitch {
    /// The share of measured frames with a pitch, 0..1.
    pub voiced: f32,
    /// The median fundamental of the voiced frames, in hertz; `None` when no frame had one.
    pub hz: Option<f32>,
}

/// Measures one note's pitch from its mono samples, searching around `expected` hertz.
pub fn note_pitch(samples: &[f32], rate: f64, expected: f32) -> NotePitch {
    let rate_f = rate as f32;
    if expected <= 0.0 || rate_f <= 0.0 {
        return NotePitch::NONE;
    }
    let lag_min = ((rate_f / (expected * REACH)).floor() as usize).max(2);
    let lag_max = (rate_f / (expected / REACH)).ceil() as usize;
    yin(samples, rate_f, lag_min, lag_max)
}

/// The lowest fundamental [`recording_pitch`] looks for: just under E1, a bass's open string.
pub const LOWEST_HZ: f32 = 38.0;

/// The highest fundamental [`recording_pitch`] looks for: C7, past the top of a piccolo's
/// useful range and well past any voice.
pub const HIGHEST_HZ: f32 = 2_093.0;

/// Measures the pitch of a recording nobody has said the pitch of.
///
/// The same YIN as [`note_pitch`], over the whole range from [`LOWEST_HZ`] to [`HIGHEST_HZ`]
/// rather than a fifth around a written note. YIN takes the *first* lag whose dip clears the
/// threshold, which is what keeps a strong second harmonic from being read as the note: the
/// period of the fundamental is never shorter than one of its harmonics', and the octave below
/// it is only reached if the fundamental's own dip is shallower — which a pitched recording's is
/// not. The cost is the frames' and not the file's: eight windows wherever the steady part is.
pub fn recording_pitch(samples: &[f32], rate: f64) -> NotePitch {
    let rate_f = rate as f32;
    if rate_f <= 0.0 {
        return NotePitch::NONE;
    }
    // A semitone of slack at the top, so the highest note's dip has a neighbour to bend toward.
    let lag_min = ((rate_f / (HIGHEST_HZ * 1.06)).floor() as usize).max(2);
    let lag_max = (rate_f / LOWEST_HZ).ceil() as usize;
    yin(samples, rate_f, lag_min, lag_max)
}

impl NotePitch {
    /// No frame had a pitch.
    pub const NONE: Self = Self {
        voiced: 0.0,
        hz: None,
    };
}

/// YIN over eight frames of the steady part, between two lags in samples.
fn yin(samples: &[f32], rate: f32, lag_min: usize, lag_max: usize) -> NotePitch {
    if lag_max <= lag_min + 1 {
        return NotePitch::NONE;
    }
    let window = lag_max * 2;
    let first = (samples.len() as f64 * MEASURED.0) as usize;
    let last = (samples.len() as f64 * MEASURED.1) as usize;
    if last <= first || last - first < window + lag_max {
        return NotePitch::NONE;
    }
    let span = last - first - window - lag_max;
    let mut found = Vec::with_capacity(FRAMES);
    let mut difference = vec![0.0f32; lag_max + 1];
    let mut normalised = vec![1.0f32; lag_max + 1];
    for frame in 0..FRAMES {
        let at = first + span * frame / (FRAMES - 1).max(1);
        let x = &samples[at..at + window + lag_max];
        // The squared difference of the frame against itself `lag` samples on.
        for (lag, slot) in difference.iter_mut().enumerate().skip(1) {
            *slot = (0..window).map(|i| (x[i] - x[i + lag]).powi(2)).sum();
        }
        // Normalised by its running mean, so a dip means "like itself", whatever the level.
        let mut sum = 0.0;
        for lag in 1..=lag_max {
            sum += difference[lag];
            normalised[lag] = if sum > 0.0 {
                difference[lag] * lag as f32 / sum
            } else {
                1.0
            };
        }
        let Some(mut lag) = (lag_min..lag_max).find(|lag| normalised[*lag] < THRESHOLD) else {
            continue;
        };
        while lag + 1 < lag_max && normalised[lag + 1] < normalised[lag] {
            lag += 1;
        }
        // A parabola through the dip and its neighbours places it between samples.
        let (a, b, c) = (normalised[lag - 1], normalised[lag], normalised[lag + 1]);
        let bend = a - 2.0 * b + c;
        let shift = if bend.abs() > f32::EPSILON {
            (0.5 * (a - c) / bend).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        found.push(rate / (lag as f32 + shift));
    }
    if found.is_empty() {
        return NotePitch::NONE;
    }
    let voiced = found.len() as f32 / FRAMES as f32;
    found.sort_by(f32::total_cmp);
    NotePitch {
        voiced,
        hz: Some(found[found.len() / 2]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 24_000.0;

    fn tone(hz: f32, sine: bool) -> Vec<f32> {
        (0..(RATE * 0.5) as usize)
            .map(|i| {
                let phase = i as f32 * hz / RATE as f32;
                0.3 * if sine {
                    (phase * std::f32::consts::TAU).sin()
                } else {
                    phase.fract() * 2.0 - 1.0
                }
            })
            .collect()
    }

    fn cents(found: NotePitch, hz: f32) -> f32 {
        1200.0 * (found.hz.unwrap() / hz).log2()
    }

    #[test]
    fn sines_and_sawtooths_from_c2_to_c6_measure_within_ten_cents() {
        // Ten rather than five for the sawtooth, which is drawn naively and so aliases at 24 kHz:
        // A4 comes out six cents flat. The lint's tolerance is fifty.
        for pitch in [36, 45, 57, 69, 76, 84] {
            let hz = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
            for sine in [true, false] {
                let found = note_pitch(&tone(hz, sine), RATE, hz);
                assert!(found.voiced > 0.9, "{pitch} {sine} {found:?}");
                assert!(cents(found, hz).abs() < 10.0, "{pitch} {sine} {found:?}");
            }
        }
    }

    #[test]
    fn a_semitone_sharp_note_measures_a_semitone_sharp() {
        let found = note_pitch(&tone(233.08, false), RATE, 220.0);
        assert!((cents(found, 220.0) - 100.0).abs() < 5.0, "{found:?}");
    }

    #[test]
    fn a_recording_with_no_written_pitch_measures_from_e1_to_c7() {
        // The naive sawtooth stops at C6: at 24 kHz a C7 one is twelve samples a period, aliased
        // into a different waveform every cycle, which no recording at a real rate resembles.
        for pitch in [28, 40, 57, 69, 81, 84, 96] {
            let hz = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
            for sine in [true, false] {
                if !sine && pitch > 84 {
                    continue;
                }
                let found = recording_pitch(&tone(hz, sine), RATE);
                assert!(found.voiced > 0.9, "{pitch} {sine} {found:?}");
                assert!(cents(found, hz).abs() < 10.0, "{pitch} {sine} {found:?}");
            }
        }
    }

    #[test]
    fn a_strong_second_harmonic_is_not_read_as_the_note() {
        // A fundamental at a third of the level of its octave, as a hollow reed or a voice on an
        // open vowel can be: still the fundamental's period, not the octave's.
        let hz = 196.0f32;
        let samples: Vec<f32> = (0..(RATE * 0.5) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32 * std::f32::consts::TAU * hz;
                0.1 * t.sin() + 0.3 * (2.0 * t).sin() + 0.1 * (3.0 * t).sin()
            })
            .collect();
        let found = recording_pitch(&samples, RATE);
        assert!(cents(found, hz).abs() < 10.0, "{found:?}");
    }

    #[test]
    fn white_noise_has_almost_no_pitch() {
        let mut state = 12345u32;
        let noise: Vec<f32> = (0..(RATE * 0.5) as usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32 - 0.5) * 0.6
            })
            .collect();
        let found = note_pitch(&noise, RATE, 220.0);
        assert!(found.voiced < 0.3, "{found:?}");
        let found = recording_pitch(&noise, RATE);
        assert!(found.voiced < 0.3, "{found:?}");
    }
}
