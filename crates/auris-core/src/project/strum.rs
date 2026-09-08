//! Chord strokes driven by a continuous hand clock, including silent grid positions.

use super::performance::{chords, milliseconds};
use super::{Note, PerformanceContext, StrokeDirection};
use crate::Ticks;
use serde::{Deserialize, Serialize};

/// What advances alternating strokes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrumClock {
    /// Advance on attacks only.
    Attacks,
    /// Advance on every eighth note, including rests.
    Eighths,
    /// Advance on every sixteenth note, including rests.
    #[default]
    Sixteenths,
}

/// A non-destructive approximation of right-hand guitar motion using ordered pitches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strum {
    /// Milliseconds between the first and last struck pitch, clamped to 0..=100.
    pub spread_ms: f32,
    /// Lowest first, highest first, or alternating strokes.
    pub direction: StrokeDirection,
    /// Clock for alternating direction; metrical clocks restart at each bar line.
    pub clock: StrumClock,
    /// Number of highest pitches struck on an upstroke; zero strikes the full chord.
    pub up_notes: u8,
    /// Upstroke velocity relative to the source, in 0..=1.
    pub up_velocity: f32,
    /// Extra gain on the lowest downstroke pitch, tapering to zero at the highest, in 0..=1.
    pub low_accent: f32,
}

impl Default for Strum {
    fn default() -> Self {
        Self {
            spread_ms: 20.0,
            direction: StrokeDirection::Alternate,
            clock: StrumClock::Sixteenths,
            up_notes: 0,
            up_velocity: 1.0,
            low_accent: 0.0,
        }
    }
}

pub(super) fn strum(
    notes: &mut [Note],
    settings: &Strum,
    context: PerformanceContext<'_>,
) -> Vec<usize> {
    let mut suppressed = Vec::new();
    let groups = chords(notes);
    for (index, group) in groups.iter().enumerate() {
        let start = notes[group[0]].start;
        let absolute = context.start + start;
        let hand = match settings.clock {
            StrumClock::Attacks => index as i64,
            clock => {
                let step = Ticks::QUARTER.raw() / if clock == StrumClock::Eighths { 2 } else { 4 };
                (absolute - context.signatures.bar_floor(absolute))
                    .raw()
                    .div_euclid(step)
            }
        };
        let up = match settings.direction {
            StrokeDirection::LowToHigh => false,
            StrokeDirection::HighToLow => true,
            StrokeDirection::Alternate => hand.rem_euclid(2) == 1,
        };
        let skip = if up && settings.up_notes > 0 {
            group.len().saturating_sub(usize::from(settings.up_notes))
        } else {
            0
        };
        suppressed.extend_from_slice(&group[..skip]);
        let played = &group[skip..];
        if up && settings.up_velocity <= 0.0 {
            suppressed.extend_from_slice(played);
            continue;
        }
        let next = groups
            .get(index + 1)
            .map_or(context.length, |g| notes[g[0]].start);
        let shortest = played
            .iter()
            .map(|&i| notes[i].length.raw())
            .min()
            .unwrap_or(1);
        let spread = if settings.spread_ms <= 0.0 {
            0
        } else {
            milliseconds(settings.spread_ms.clamp(0.0, 100.0), context.bpm)
                .raw()
                .min(shortest - 1)
                .min((next - start).raw() - 1)
                .max(0)
        };
        let last_rank = played.len().saturating_sub(1).max(1);
        for (rank, &i) in played.iter().enumerate() {
            let order = if up { played.len() - 1 - rank } else { rank };
            let offset = Ticks(spread * order as i64 / last_rank as i64);
            notes[i].start += offset;
            notes[i].length -= offset;
            let gain = if up {
                settings.up_velocity.clamp(0.0, 1.0)
            } else {
                1.0 + settings.low_accent.clamp(0.0, 1.0) * (1.0 - rank as f32 / last_rank as f32)
            };
            notes[i].velocity = (notes[i].velocity * gain).clamp(0.0, 1.0);
        }
    }
    suppressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipId, MidiClip, NoteTransform, SignatureMap, TimeSignature};
    fn chord(at: i64) -> Vec<Note> {
        [48, 60, 64, 67]
            .map(|pitch| Note {
                velocity: 0.5,
                ..Note::new(pitch, Ticks(at), Ticks(240))
            })
            .to_vec()
    }
    fn performed(
        source: Vec<Note>,
        settings: Strum,
        start: Ticks,
        meter: SignatureMap,
    ) -> Vec<Note> {
        MidiClip {
            notes: source,
            transforms: vec![NoteTransform::Strum { settings }],
            ..MidiClip::new(ClipId(1), "Strum", start, Ticks(3840))
        }
        .sounding_notes_with_meter(120.0, meter)
        .collect()
    }
    #[test]
    fn rests_advance_the_hand_and_unrelated_attacks_do_not_flip_it() {
        let settings = Strum::default();
        let sparse = performed(
            [chord(0), chord(480), chord(720)].concat(),
            settings.clone(),
            Ticks::ZERO,
            SignatureMap::default(),
        );
        assert!(
            sparse[4].start < sparse[7].start,
            "the empty upstroke at 240 preserves the next downstroke"
        );
        assert!(sparse[8].start > sparse[11].start);
        let dense = performed(
            [chord(0), chord(240), chord(480), chord(720)].concat(),
            settings,
            Ticks::ZERO,
            SignatureMap::default(),
        );
        assert_eq!(&sparse[4..], &dense[8..]);
    }
    #[test]
    fn upstrokes_keep_only_high_pitches_and_downstrokes_accent_the_bass() {
        let source = [chord(0), chord(240)].concat();
        let output = performed(
            source,
            Strum {
                up_notes: 2,
                up_velocity: 0.6,
                low_accent: 0.5,
                ..Strum::default()
            },
            Ticks::ZERO,
            SignatureMap::default(),
        );
        assert_eq!(output.len(), 6);
        assert_eq!(output[0].velocity, 0.75);
        assert_eq!(output[3].velocity, 0.5);
        assert_eq!(output[4].pitch, 64);
        assert_eq!(output[5].pitch, 67);
        assert!((output[4].velocity - 0.3).abs() < 1e-6);
        assert!(output[4..].iter().all(|n| n.end() == Ticks(480)));
    }
    #[test]
    fn skipped_strings_stay_absent_through_scoped_and_later_stages() {
        let mut source = chord(240);
        for note in &mut source {
            note.drum_voice = "strings".into();
        }
        source[3].velocity = 0.0;
        source.push(Note::new(36, Ticks(240), Ticks(240)));
        let slots = crate::performed_note_slots(
            source,
            &[
                NoteTransform::ForDrumVoice {
                    voice: "strings".into(),
                    transforms: vec![NoteTransform::Strum {
                        settings: Strum {
                            up_notes: 2,
                            ..Strum::default()
                        },
                    }],
                },
                NoteTransform::Transpose { semitones: 12 },
            ],
            PerformanceContext {
                bpm: 120.0,
                pass: 0,
                start: Ticks::ZERO,
                length: Ticks(960),
                signatures: &SignatureMap::default(),
            },
        );
        assert!(slots[0].is_none() && slots[1].is_none());
        assert_eq!(slots[2].as_ref().unwrap().pitch, 76);
        assert_eq!(slots[3].as_ref().unwrap().velocity, 0.0);
        assert_eq!(slots[4].as_ref().unwrap().pitch, 48);
    }
    #[test]
    fn offgrid_clip_origins_use_the_project_meter() {
        let meter = SignatureMap::constant(TimeSignature::new(3, 4));
        let output = performed(chord(0), Strum::default(), Ticks(3120), meter);
        assert!(
            output[0].start > output[3].start,
            "one sixteenth after the 3/4 bar line is up"
        );
    }
}
