//! Continuous YIN candidates and a fixed-cost temporal note decoder.
//!
//! Inspired by pYIN's separation of candidates and decoding, but this is not its probability
//! model: costs here are hand-set, uncalibrated and never learned from recordings.

use crate::audio::ANALYSIS_RATE;
use crate::{AnalysisControl, AnalysisError};

/// Support for each centered difference-function observation.
pub(crate) const WINDOW: usize = 1024;

#[derive(Clone, Copy, Debug)]
/// A continuous pitch hypothesis and its uncalibrated local cost.
pub(crate) struct Candidate {
    midi: f32,
    cost: f32,
}

/// Retains at most six plausible local periods, before quantizing to note identity.
pub(crate) fn candidates(samples: &[f32; WINDOW], difference: &mut [f32; 174]) -> Vec<Candidate> {
    difference.fill(1.0);
    let mut sum = 0.0;
    for tau in 1..difference.len() {
        // Center both compared sequences together on the timestamp, rather than putting
        // their shared support in the first half of the frame and delaying every onset.
        let begin = (WINDOW - WINDOW / 2 - tau) / 2;
        let d = (begin..begin + WINDOW / 2)
            .map(|i| (samples[i] - samples[i + tau]).powi(2))
            .sum::<f32>();
        sum += d;
        difference[tau] = if sum > 1e-10 {
            d * tau as f32 / sum
        } else {
            1.0
        };
    }
    let mut valleys = Vec::new();
    for tau in 11..difference.len() - 1 {
        let (a, b, c) = (difference[tau - 1], difference[tau], difference[tau + 1]);
        if b >= 0.35 || b > a || b >= c {
            continue;
        }
        let shift = if (a - 2.0 * b + c).abs() > 1e-8 {
            (0.5 * (a - c) / (a - 2.0 * b + c)).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let period = tau as f32 + shift;
        let hz = ANALYSIS_RATE as f32 / period;
        if !(65.0..=1000.0).contains(&hz) {
            continue;
        }
        valleys.push((period, b));
    }
    let first = valleys.first().map_or(1.0, |(p, _)| *p);
    let mut choices: Vec<_> = valleys
        .into_iter()
        .map(|(period, d)| Candidate {
            midi: 69.0 + 12.0 * (ANALYSIS_RATE as f32 / period / 440.0).log2(),
            cost: d * 4.0 + (period / first).ln() * 0.3,
        })
        .collect();
    choices.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    choices.truncate(6);
    choices
}

/// Decodes note identity before segmentation so vibrato does not repeatedly round across a key.
pub(crate) fn decode(
    frames: &[Vec<Candidate>],
    levels: &[f32],
    control: &AnalysisControl,
) -> Result<Vec<Option<u8>>, AnalysisError> {
    let maximum = levels.iter().copied().fold(0.0f32, f32::max);
    let mut previous: Vec<(Option<u8>, f32)> = Vec::new();
    let mut links: Vec<Vec<(Option<u8>, usize)>> = Vec::with_capacity(frames.len());
    for (i, frame) in frames.iter().enumerate() {
        if i % 256 == 0 {
            control.check(0.94 + 0.04 * i as f32 / frames.len().max(1) as f32)?;
        }
        let mut emission = [f32::INFINITY; 128];
        if levels[i] > maximum * 0.03 {
            for candidate in frame {
                let near = candidate.midi.round() as i32;
                for pitch in near - 1..=near + 1 {
                    let distance = candidate.midi - pitch as f32;
                    if (0..128).contains(&pitch) && distance.abs() <= 0.85 {
                        let cost = candidate.cost + 2.0 * distance * distance;
                        emission[pitch as usize] = emission[pitch as usize].min(cost);
                    }
                }
            }
        }
        let mut states: Vec<_> = emission
            .into_iter()
            .enumerate()
            .filter(|(_, c)| c.is_finite())
            .map(|(p, c)| (Some(p as u8), c))
            .collect();
        states.sort_by(|a, b| a.1.total_cmp(&b.1));
        states.truncate(8);
        states.push((None, if states.is_empty() { 0.0 } else { 2.5 }));
        let mut row = Vec::with_capacity(states.len());
        let mut next = Vec::with_capacity(states.len());
        for (pitch, cost) in states {
            let (parent, before) = previous
                .iter()
                .enumerate()
                .map(|(j, (p, c))| {
                    let transition = match (p, pitch) {
                        (Some(a), Some(b)) if *a != b => {
                            1.8 + f32::from(a.abs_diff(b)).min(12.0) * 0.08
                        }
                        (None, Some(_)) | (Some(_), None) => 0.7,
                        _ => 0.0,
                    };
                    (j, *c + transition)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or((0, 0.0));
            row.push((pitch, parent));
            next.push((pitch, cost + before));
        }
        // Subtract a shared constant to keep long files from losing local cost precision.
        let least = next.iter().map(|(_, c)| *c).fold(f32::INFINITY, f32::min);
        for (_, cost) in &mut next {
            *cost -= least;
        }
        previous = next;
        links.push(row);
    }
    let mut state = previous
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
        .map_or(0, |(i, _)| i);
    let mut result = vec![None; frames.len()];
    for i in (0..frames.len()).rev() {
        if i % 256 == 0 {
            control.check(0.99)?;
        }
        let (pitch, parent) = links[i][state];
        result[i] = pitch;
        state = parent;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vibrato_is_one_note_but_a_sustained_semitone_change_survives() {
        let frames: Vec<_> = (0..100)
            .map(|i| {
                vec![Candidate {
                    midi: if i < 50 {
                        69.0 + 0.65 * (i as f32 * 0.69).sin()
                    } else {
                        70.0
                    },
                    cost: 0.0,
                }]
            })
            .collect();
        let path = decode(&frames, &[1.0; 100], &AnalysisControl::default()).unwrap();
        // A vibrato crest before the new note can make the handoff a few frames early;
        // the contract here is stable note identity, not an unobservable exact boundary.
        assert!(path[..45].iter().all(|p| *p == Some(69)), "{path:?}");
        assert!(path[52..].iter().all(|p| *p == Some(70)));
        assert_eq!(path.windows(2).filter(|p| p[0] != p[1]).count(), 1);
    }
    #[test]
    fn silence_is_preserved_and_cancelled_decoding_stops() {
        let frames = vec![
            vec![Candidate {
                midi: 69.0,
                cost: 0.0
            }];
            20
        ];
        let mut levels = [1.0; 20];
        levels[8..12].fill(0.0);
        let control = AnalysisControl::default();
        let path = decode(&frames, &levels, &control).unwrap();
        assert!(path[8..12].iter().all(Option::is_none));
        control.cancel();
        assert!(matches!(
            decode(&frames, &levels, &control),
            Err(AnalysisError::Cancelled)
        ));
    }
}
