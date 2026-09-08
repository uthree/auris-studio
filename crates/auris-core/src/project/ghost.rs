//! Seeded ghost-note placement on the musical grid.

use super::performance::{chords, milliseconds, ornament, overlaps};
use super::{Note, PerformanceContext};
use crate::Ticks;
use crate::rng::{Key, Rng};
use serde::{Deserialize, Serialize};

/// Candidate positions for quiet auxiliary notes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GhostPattern {
    /// Every unoccupied sixteenth-note position.
    Sixteenths,
    /// The eighth-note offbeat of each quarter note.
    Offbeats,
    /// The last sixteenth before the next written attack.
    #[default]
    Pickup,
    /// A seeded pattern repeated at the same positions in each bar.
    Repeating,
}

/// Independent placement and sound controls for ghost notes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhostNotes {
    /// Probability of accepting a candidate, in 0..=1; independent of its velocity.
    pub density: f32,
    /// Gain relative to the preceding note or chord, in 0..=1.
    pub velocity: f32,
    /// Auxiliary note duration, clamped to 1..=100 ms and the clip boundary.
    pub length_ms: f32,
    /// Where notes may be inserted.
    pub pattern: GhostPattern,
    /// How often a repeating pattern uses a fresh bar/pass draw, in 0..=1.
    pub variation: f32,
    /// Preserve the tail after the final attack and gaps longer than `max_gap_beats`.
    pub preserve_rests: bool,
    /// Maximum gap between written releases and the next attack, in quarter notes.
    pub max_gap_beats: f32,
    /// Only this MIDI pitch supplies ghosts and blocks their placement; None recalls chords.
    pub target_pitch: Option<u8>,
    /// Stored take identity. Loudness and length never change the random draws.
    pub seed: u64,
}

impl Default for GhostNotes {
    fn default() -> Self {
        Self {
            density: 0.5,
            velocity: 0.3,
            length_ms: 20.0,
            pattern: GhostPattern::Pickup,
            variation: 0.15,
            preserve_rests: true,
            max_gap_beats: 2.0,
            target_pitch: None,
            seed: 0,
        }
    }
}

pub(super) fn ghost_notes(
    notes: &mut Vec<Note>,
    settings: &GhostNotes,
    context: PerformanceContext<'_>,
) {
    if settings.density <= 0.0 || settings.velocity <= 0.0 || notes.is_empty() {
        return;
    }
    let selected: Vec<Note> = notes
        .iter()
        .filter(|note| {
            settings
                .target_pitch
                .is_none_or(|pitch| note.pitch == pitch)
        })
        .cloned()
        .collect();
    let groups = chords(&selected);
    let mut added = Vec::new();
    let mut previous = None;
    let mut next_group = 0;
    let step = Ticks(Ticks::QUARTER.raw() / 4);
    let mut absolute = context.signatures.bar_floor(context.start);
    while absolute < context.start + context.length {
        let start = absolute - context.start;
        if start >= Ticks::ZERO {
            while next_group < groups.len() && selected[groups[next_group][0]].start < start {
                previous = Some(next_group);
                next_group += 1;
            }
            let next = groups.get(next_group).map(|g| selected[g[0]].start);
            let bar = context.signatures.bar_floor(absolute);
            let slot = (absolute - bar).raw() / step.raw();
            let eligible = match settings.pattern {
                GhostPattern::Sixteenths | GhostPattern::Repeating => true,
                GhostPattern::Offbeats => slot % 4 == 2,
                GhostPattern::Pickup => {
                    next.is_some_and(|next| next > start && next - start <= step)
                }
            };
            let mut draw = Rng::stream(
                settings.seed,
                &[
                    Key::Word("ghost"),
                    Key::Index(context.pass),
                    Key::Index(absolute.raw() as u64),
                ],
            );
            let chance = if settings.pattern == GhostPattern::Repeating
                && !draw.chance(settings.variation.clamp(0.0, 1.0))
            {
                Rng::stream(
                    settings.seed,
                    &[Key::Word("ghost_pattern"), Key::Index(slot as u64)],
                )
                .unit()
            } else {
                draw.unit()
            };
            let length = milliseconds(settings.length_ms.clamp(1.0, 100.0), context.bpm)
                .min(context.length - start);
            if let Some(group) = previous
                && eligible
                && chance < settings.density.clamp(0.0, 1.0)
                && !selected
                    .iter()
                    .any(|note| overlaps(note, start, start + length))
            {
                let release = groups[group]
                    .iter()
                    .map(|&i| selected[i].end())
                    .max()
                    .unwrap_or(start);
                let preserve = settings.preserve_rests
                    && next.is_none_or(|next| {
                        (next - release).raw() as f32
                            > settings.max_gap_beats.clamp(0.0, 16.0) * Ticks::QUARTER.raw() as f32
                    });
                if !preserve {
                    for &index in &groups[group] {
                        let note = &selected[index];
                        if !added
                            .iter()
                            .any(|other: &Note| other.start == start && other.pitch == note.pitch)
                        {
                            added.push(ornament(
                                note,
                                note.pitch,
                                start,
                                length,
                                settings.velocity.clamp(0.0, 1.0),
                            ));
                        }
                    }
                }
            }
        }
        absolute += step;
    }
    notes.extend(added);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipId, MidiClip, NoteTransform, SignatureMap};

    fn run(notes: Vec<Note>, settings: GhostNotes, length: i64) -> Vec<Note> {
        let clip = MidiClip {
            notes,
            transforms: vec![NoteTransform::Ghost { settings }],
            ..MidiClip::new(ClipId(1), "Ghost", Ticks::ZERO, Ticks(length))
        };
        clip.sounding_notes_with_meter(120.0, SignatureMap::default())
            .collect()
    }

    #[test]
    fn pickup_respects_rests_tails_and_written_attacks() {
        let source = vec![
            Note::new(60, Ticks(0), Ticks(480)),
            Note::new(64, Ticks(960), Ticks(480)),
            Note::new(67, Ticks(6000), Ticks(480)),
        ];
        let output = run(
            source.clone(),
            GhostNotes {
                density: 1.0,
                ..GhostNotes::default()
            },
            7680,
        );
        assert_eq!(&output[..source.len()], &source);
        assert_eq!(output.len(), 4);
        assert_eq!(
            (output[3].pitch, output[3].start, output[3].length),
            (60, Ticks(720), Ticks(38))
        );
        assert!((output[3].velocity - source[0].velocity * 0.3).abs() < 1e-6);
    }

    #[test]
    fn density_is_monotonic_and_velocity_does_not_reroll_placement() {
        let source = vec![Note::new(60, Ticks::ZERO, Ticks(120))];
        let settings = GhostNotes {
            pattern: GhostPattern::Sixteenths,
            preserve_rests: false,
            seed: 52,
            ..GhostNotes::default()
        };
        let low = run(
            source.clone(),
            GhostNotes {
                density: 0.2,
                ..settings.clone()
            },
            3840 * 8,
        );
        let high = run(
            source.clone(),
            GhostNotes {
                density: 0.8,
                ..settings.clone()
            },
            3840 * 8,
        );
        assert!(high.len() > low.len());
        assert!(
            low.iter()
                .all(|note| high.iter().any(|other| other.start == note.start))
        );
        let soft = run(
            source.clone(),
            GhostNotes {
                velocity: 0.05,
                ..settings.clone()
            },
            3840 * 8,
        );
        let loud = run(source, settings, 3840 * 8);
        assert_eq!(
            soft.iter().map(|n| n.start).collect::<Vec<_>>(),
            loud.iter().map(|n| n.start).collect::<Vec<_>>()
        );
    }

    #[test]
    fn repeating_pattern_repeats_and_target_ignores_other_drum_voices() {
        let source = vec![
            Note::new(38, Ticks::ZERO, Ticks(120)),
            Note::new(36, Ticks(240), Ticks(15000)),
        ];
        let settings = GhostNotes {
            pattern: GhostPattern::Repeating,
            preserve_rests: false,
            variation: 0.0,
            target_pitch: Some(38),
            seed: 123,
            ..GhostNotes::default()
        };
        let output = run(source, settings, 3840 * 4);
        let slots = |bar: i64| {
            output[2..]
                .iter()
                .filter(|n| n.start.raw() / 3840 == bar)
                .map(|n| n.start.raw() % 3840)
                .collect::<Vec<_>>()
        };
        assert!(!slots(1).is_empty());
        assert_eq!(slots(1), slots(2));
        assert_eq!(slots(2), slots(3));
        assert!(output[2..].iter().all(|note| note.pitch == 38));
    }

    #[test]
    fn ghost_settings_round_trip_and_missing_fields_take_defaults() {
        let transform: NoteTransform = serde_json::from_str(
            r#"{"kind":"ghost","settings":{"density":0.8,"target_pitch":38}}"#,
        )
        .unwrap();
        let encoded = serde_json::to_string(&transform).unwrap();
        assert_eq!(
            serde_json::from_str::<NoteTransform>(&encoded).unwrap(),
            transform
        );
        let NoteTransform::Ghost { settings } = transform else {
            panic!()
        };
        assert_eq!(settings.length_ms, 20.0);
        assert_eq!(settings.pattern, GhostPattern::Pickup);
        assert!(settings.preserve_rests);
    }
}
