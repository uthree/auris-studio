//! Duration-weighted chromatic templates and temporal decoding for written notes.

use crate::{AnalysisControl, AnalysisError};
use auris_core::{
    Note,
    theory::{
        chord::{Chord, Quality},
        pitch::PitchClass,
    },
    time::Ticks,
};
use serde::Serialize;

/// How a segment's harmonic evidence should be interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordState {
    /// A supported template explains the evidence.
    Recognized,
    /// Tonal evidence is insufficient or competing readings are too close.
    Unknown,
    /// No harmonic energy or written notes are present.
    NoChord,
}

/// One possible chord, ranked by template agreement.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChordCandidate {
    /// Absolute chord symbol, including a bass note when supported.
    pub symbol: String,
    /// Template agreement, in 0..=1; not a probability.
    pub score: f32,
}

/// Interpretation shared by symbolic and audio intervals.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChordReading {
    /// Whether the interval is recognized, ambiguous or silent.
    pub state: ChordState,
    /// Temporally selected candidate first, followed by alternatives.
    pub candidates: Vec<ChordCandidate>,
    /// Difference between the two highest local scores; not confidence calibration.
    pub margin: f32,
}

/// A chord interval in absolute project ticks, with an exclusive end.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolicChordSegment {
    /// Inclusive start in project ticks.
    pub start: Ticks,
    /// Exclusive end in project ticks.
    pub end: Ticks,
    /// Candidate harmony for the interval.
    pub reading: ChordReading,
}

/// Reproducible options for symbolic analysis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ChordOptions {
    /// Analysis grid in ticks; changes are localized to this resolution.
    pub window: Ticks,
}

impl Default for ChordOptions {
    fn default() -> Self {
        Self {
            window: Ticks::QUARTER,
        }
    }
}

const QUALITIES: [Quality; 13] = [
    Quality::Major,
    Quality::Minor,
    Quality::Diminished,
    Quality::Augmented,
    Quality::Sus2,
    Quality::Sus4,
    Quality::Dominant7,
    Quality::Major7,
    Quality::Minor7,
    Quality::HalfDiminished7,
    Quality::Diminished7,
    Quality::Major6,
    Quality::Minor6,
];

/// Recognizes absolute notes on a bounded, caller-selected grid.
///
/// Notes must already include clip placement/repeats. Octave and unison doubling cannot
/// contribute more than one full window per pitch class. Silence never inherits a chord.
pub fn analyze_notes(
    notes: &[Note],
    from: Ticks,
    to: Ticks,
    options: ChordOptions,
    control: &AnalysisControl,
) -> Result<Vec<SymbolicChordSegment>, AnalysisError> {
    control.check(0.0)?;
    if from.raw() < 0 || to <= from || options.window.raw() <= 0 || notes.len() > 200_000 {
        return Err(AnalysisError::Invalid(
            "invalid note range or analysis grid",
        ));
    }
    let count = ((to.raw() - from.raw() - 1) / options.window.raw() + 1) as usize;
    if count > 16_384 {
        return Err(AnalysisError::Invalid(
            "select a shorter range or a coarser chord grid",
        ));
    }
    let mut weights = vec![[0.0f32; 12]; count];
    let mut bass = vec![None; count];
    let mut operations = 0usize;
    for (n, note) in notes.iter().enumerate() {
        control.check(n as f32 / notes.len().max(1) as f32 * 0.6)?;
        if note.pitch > 127
            || !note.velocity.is_finite()
            || note.velocity <= 0.0
            || note.length.raw() <= 0
        {
            continue;
        }
        let start = note.start.max(from);
        let end = Ticks(note.start.raw().saturating_add(note.length.raw())).min(to);
        if end <= start {
            continue;
        }
        let first = ((start - from).raw() / options.window.raw()) as usize;
        let last = ((end - from).raw() - 1) / options.window.raw();
        for i in first..=last as usize {
            operations += 1;
            if operations > 4_000_000 {
                return Err(AnalysisError::Invalid(
                    "too many sustained notes for this analysis grid",
                ));
            }
            let a = from + options.window * i as i64;
            let b = Ticks(a.raw().saturating_add(options.window.raw())).min(to);
            let overlap = (end.min(b) - start.max(a)).raw() as f32 / (b - a).raw() as f32;
            let weight = &mut weights[i][(note.pitch % 12) as usize];
            *weight = (*weight + overlap).min(1.0);
            if overlap >= 0.2 {
                bass[i] = Some(bass[i].map_or(note.pitch, |p: u8| p.min(note.pitch)));
            }
        }
    }
    let readings = weights
        .iter()
        .zip(bass)
        .map(|(w, b)| rank(w, b, false))
        .collect();
    let readings = smooth(readings, control, 0.6)?;
    let mut result: Vec<SymbolicChordSegment> = Vec::new();
    for (i, reading) in readings.into_iter().enumerate() {
        let start = from + options.window * i as i64;
        let end = Ticks(start.raw().saturating_add(options.window.raw())).min(to);
        // Preserve every window's alternatives and scores; merging would hide local uncertainty.
        result.push(SymbolicChordSegment {
            start,
            end,
            reading,
        });
    }
    control.check(1.0)?;
    Ok(result)
}

pub(crate) fn rank(weights: &[f32; 12], bass: Option<u8>, audio: bool) -> ChordReading {
    let total: f32 = weights.iter().sum();
    let maximum = weights.iter().copied().fold(0.0f32, f32::max);
    if total <= 1e-6 {
        return ChordReading {
            state: ChordState::NoChord,
            candidates: vec![],
            margin: 0.0,
        };
    }
    let distinct = weights.iter().filter(|w| **w > maximum * 0.2).count();
    let mut candidates = Vec::new();
    for root in 0..12 {
        for quality in QUALITIES
            .into_iter()
            .filter(|q| !audio || matches!(q, Quality::Major | Quality::Minor))
        {
            let mut chord = Chord::new(PitchClass::new(root), quality);
            let classes: Vec<_> = quality
                .intervals()
                .iter()
                .map(|i| ((root + i) % 12) as usize)
                .collect();
            let inside: f32 = classes.iter().map(|i| weights[*i]).sum();
            let missing = classes
                .iter()
                .filter(|i| weights[**i] < maximum * 0.2)
                .count() as f32;
            let bass_root = bass.is_some_and(|b| i32::from(b % 12) == root);
            let score = (inside / total
                - (total - inside) / total * 0.7
                - missing * 0.18
                - (classes.len() as f32 - 3.0) * 0.015
                + if bass_root { 0.025 } else { 0.0 })
            .clamp(0.0, 1.0);
            if let Some(b) =
                bass.filter(|b| classes.contains(&usize::from(b % 12)) && i32::from(b % 12) != root)
            {
                chord = chord.over(PitchClass::new(i32::from(b % 12)));
            }
            candidates.push(ChordCandidate {
                symbol: chord.to_string(),
                score,
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    candidates.truncate(4);
    let margin = candidates[0].score - candidates[1].score;
    let state = if distinct < 3 || candidates[0].score < 0.65 || margin < 0.008 {
        ChordState::Unknown
    } else {
        ChordState::Recognized
    };
    ChordReading {
        state,
        candidates,
        margin,
    }
}

pub(crate) fn smooth(
    mut readings: Vec<ChordReading>,
    control: &AnalysisControl,
    start: f32,
) -> Result<Vec<ChordReading>, AnalysisError> {
    // Four candidates per frame make both time and backtracking storage linear in duration.
    let mut scores: Vec<Vec<f32>> = Vec::with_capacity(readings.len());
    let mut previous: Vec<Vec<usize>> = Vec::with_capacity(readings.len());
    for (i, reading) in readings.iter().enumerate() {
        control.check(start + (1.0 - start) * i as f32 / readings.len().max(1) as f32)?;
        let mut values = Vec::new();
        let mut links = Vec::new();
        for c in &reading.candidates {
            let best = if i > 0 {
                readings[i - 1]
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(j, p)| {
                        (
                            j,
                            scores[i - 1][j] - if p.symbol == c.symbol { 0.0 } else { 0.12 },
                        )
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((0, 0.0))
            } else {
                (0, 0.0)
            };
            values.push(best.1 + c.score);
            links.push(best.0);
        }
        scores.push(values);
        previous.push(links);
    }
    let mut next = None;
    for i in (0..readings.len()).rev() {
        if readings[i].candidates.is_empty() {
            next = None;
            continue;
        }
        let index = next.unwrap_or_else(|| {
            scores[i]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |x| x.0)
        });
        next = Some(previous[i][index]);
        let selected = readings[i].candidates.remove(index);
        if selected.score < 0.65 {
            readings[i].state = ChordState::Unknown;
        }
        readings[i].candidates.insert(0, selected);
    }
    Ok(readings)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notes(pitches: &[u8], start: i64, length: i64) -> Vec<Note> {
        pitches
            .iter()
            .map(|p| Note::new(*p, Ticks(start), Ticks(length)))
            .collect()
    }
    #[test]
    fn clean_triads_and_sevenths_transpose_and_preserve_inversions() {
        for root in 0..12u8 {
            for (quality, intervals) in [
                ("", vec![0, 4, 7]),
                ("m", vec![0, 3, 7]),
                ("7", vec![0, 4, 7, 10]),
                ("maj7", vec![0, 4, 7, 11]),
            ] {
                let input = notes(
                    &intervals.iter().map(|i| 48 + root + i).collect::<Vec<_>>(),
                    0,
                    960,
                );
                let result = analyze_notes(
                    &input,
                    Ticks(0),
                    Ticks(960),
                    ChordOptions::default(),
                    &AnalysisControl::default(),
                )
                .unwrap();
                let expected = format!("{}{}", PitchClass::new(i32::from(root)), quality);
                let actual = Chord::parse(&result[0].reading.candidates[0].symbol).unwrap();
                assert_eq!(
                    actual,
                    Chord::parse(&expected).unwrap(),
                    "{expected}: {:?}",
                    result
                );
            }
        }
        let input = notes(&[52, 55, 60], 0, 960);
        let result = analyze_notes(
            &input,
            Ticks(0),
            Ticks(960),
            ChordOptions::default(),
            &AnalysisControl::default(),
        )
        .unwrap();
        assert_eq!(result[0].reading.candidates[0].symbol, "C/E");
    }
    #[test]
    fn silence_single_notes_and_arpeggios_have_distinct_meanings() {
        let mut input = notes(&[60], 0, 960);
        input.extend(notes(&[60], 1920, 320));
        input.extend(notes(&[64], 2240, 320));
        input.extend(notes(&[67], 2560, 320));
        let r = analyze_notes(
            &input,
            Ticks(0),
            Ticks(2880),
            ChordOptions::default(),
            &AnalysisControl::default(),
        )
        .unwrap();
        assert_eq!(r[0].reading.state, ChordState::Unknown);
        assert_eq!(r[1].reading.state, ChordState::NoChord);
        assert_eq!(r[2].reading.candidates[0].symbol, "C");
    }
    #[test]
    fn changes_inside_a_bar_and_held_notes_keep_their_timing() {
        let mut input = notes(&[60, 64, 67], 0, 1920);
        input.extend(notes(&[62, 65, 69], 1920, 1920));
        let r = analyze_notes(
            &input,
            Ticks(960),
            Ticks(3840),
            ChordOptions::default(),
            &AnalysisControl::default(),
        )
        .unwrap();
        assert_eq!(r[0].reading.candidates[0].symbol, "C");
        assert_eq!(r[1].reading.candidates[0].symbol, "Dm");
        assert_eq!(r[2].end, Ticks(3840));
    }
    #[test]
    fn ambiguous_sixth_and_seventh_are_offered_and_requests_are_bounded() {
        let r = rank(
            &[1., 0., 0., 0., 1., 0., 0., 1., 0., 1., 0., 0.],
            Some(48),
            false,
        );
        assert!(r.candidates.iter().any(|c| c.symbol == "C6"));
        assert!(r.candidates.iter().any(|c| c.symbol == "Am7/C"));
        assert!(
            analyze_notes(
                &[],
                Ticks(0),
                Ticks(i64::MAX),
                ChordOptions::default(),
                &AnalysisControl::default()
            )
            .is_err()
        );
        let control = AnalysisControl::default();
        control.cancel();
        assert!(matches!(
            analyze_notes(&[], Ticks(0), Ticks(960), ChordOptions::default(), &control),
            Err(AnalysisError::Cancelled)
        ));
    }
}
