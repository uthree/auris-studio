//! Portable groove snapshots extracted from performed MIDI notes.
use super::{Note, Subdivision};
use crate::Ticks;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Timing and relative strength at one grid position in a groove.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroovePoint {
    /// Grid position relative to the beginning of the template.
    pub position: Ticks,
    /// Average offset from the grid, bounded to half a subdivision when applied.
    pub offset: Ticks,
    /// Velocity relative to the source phrase's mean, bounded to 0..=2 when applied.
    pub velocity: f32,
}

/// A stored MIDI groove. It is independent of the source clip after capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrooveTemplate {
    /// Display name of the source clip at capture time.
    pub name: String,
    /// Repeating span, rounded up to the extraction grid.
    pub length: Ticks,
    /// Grid used to associate attacks with groove positions.
    pub subdivision: Subdivision,
    /// Occupied source positions; empty slots do not alter the target.
    pub points: Vec<GroovePoint>,
}

impl GrooveTemplate {
    /// Captures timing and relative dynamics without copying pitches or adding target notes.
    /// Returns None for an empty phrase or an invalid span.
    pub fn capture(
        name: impl Into<String>,
        notes: &[Note],
        length: Ticks,
        subdivision: Subdivision,
    ) -> Option<Self> {
        if length <= Ticks::ZERO {
            return None;
        }
        let step = Ticks::QUARTER.raw() / i64::from(subdivision.steps_per_beat());
        let span = ((length.raw() - 1) / step + 1).checked_mul(step)?;
        let mut groups: BTreeMap<i64, (i64, f64, usize)> = BTreeMap::new();
        let mut sum = 0.0_f64;
        let mut count = 0;
        for note in notes
            .iter()
            .filter(|n| n.start >= Ticks::ZERO && n.start < length)
        {
            let grid = (note.start.raw() + step / 2).div_euclid(step) * step;
            let group = groups.entry(grid.rem_euclid(span)).or_default();
            group.0 += note.start.raw() - grid;
            let velocity = f64::from(note.velocity.clamp(1.0 / 127.0, 1.0));
            group.1 += velocity;
            group.2 += 1;
            sum += velocity;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let mean = sum / count as f64;
        let points = groups
            .into_iter()
            .map(|(position, (offset, velocity, count))| GroovePoint {
                position: Ticks(position),
                offset: Ticks((offset as f64 / count as f64).round() as i64),
                velocity: (velocity / count as f64 / mean) as f32,
            })
            .collect();
        Some(Self {
            name: name.into(),
            length: Ticks(span),
            subdivision,
            points,
        })
    }

    pub(super) fn apply(&self, note: &mut Note, timing: f32, velocity: f32) {
        if self.length <= Ticks::ZERO {
            return;
        }
        let step = Ticks::QUARTER.raw() / i64::from(self.subdivision.steps_per_beat());
        let grid = (note.start.raw() + step / 2).div_euclid(step) * step;
        let position = Ticks(grid.rem_euclid(self.length.raw()));
        let Some(point) = self.points.iter().find(|p| p.position == position) else {
            return;
        };
        let target = grid + point.offset.raw().clamp(-step / 2, step / 2);
        note.start = (note.start
            + Ticks(((target - note.start.raw()) as f32 * timing.clamp(0.0, 1.0)).round() as i64))
        .max_zero();
        note.velocity = (note.velocity
            * (1.0 + (point.velocity.clamp(0.0, 2.0) - 1.0) * velocity.clamp(0.0, 1.0)))
        .clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_groove_repeats_and_preserves_target_pitches_and_note_count() {
        let notes = vec![
            Note {
                velocity: 0.4,
                ..Note::new(60, Ticks(80), Ticks(100))
            },
            Note {
                velocity: 0.8,
                ..Note::new(64, Ticks(560), Ticks(100))
            },
        ];
        let groove =
            GrooveTemplate::capture("Reference", &notes, Ticks(1920), Subdivision::Sixteenth)
                .unwrap();
        let mut target = Note {
            velocity: 0.6,
            ..Note::new(36, Ticks(1920), Ticks(120))
        };
        groove.apply(&mut target, 1.0, 1.0);
        assert_eq!(target.start, Ticks(2000));
        assert_eq!(target.pitch, 36);
        assert_eq!(target.length, Ticks(120));
        assert!((target.velocity - 0.4).abs() < 1e-6);
        let mut empty = Note::new(36, Ticks(240), Ticks(120));
        let original = empty.clone();
        groove.apply(&mut empty, 1.0, 1.0);
        assert_eq!(
            empty, original,
            "empty reference slots leave the target alone"
        );
        assert_eq!(
            serde_json::from_str::<GrooveTemplate>(&serde_json::to_string(&groove).unwrap())
                .unwrap(),
            groove
        );
    }
    #[test]
    fn extraction_rejects_empty_phrases_and_averages_chords() {
        assert!(
            GrooveTemplate::capture("Empty", &[], Ticks(960), Subdivision::Sixteenth).is_none()
        );
        let notes = vec![
            Note::new(60, Ticks(40), Ticks(100)),
            Note::new(64, Ticks(80), Ticks(100)),
        ];
        let groove =
            GrooveTemplate::capture("Chord", &notes, Ticks(960), Subdivision::Sixteenth).unwrap();
        assert_eq!(groove.points.len(), 1);
        assert_eq!(groove.points[0].offset, Ticks(60));
        assert_eq!(groove.points[0].velocity, 1.0);
    }
}
