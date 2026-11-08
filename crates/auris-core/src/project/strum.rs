//! Chord strokes driven by a continuous hand clock, including silent grid positions.
//!
//! A pick sweeping across strings reaches them one after another, and no two sweeps take the
//! same time or space the strings alike. Listeners need about 15 to 20 ms to tell which of two
//! sounds came first, so a chord whose strings all start within a few milliseconds is heard as
//! one hit, a sound no hand makes. The spread is therefore given either for the whole chord or
//! per string, and each stroke wanders a little in speed and spacing.

use super::performance::{chords, milliseconds};
use super::{Note, PerformanceContext, StrokeDirection};
use crate::Ticks;
use crate::rng::{Key, Rng};
use serde::{Deserialize, Serialize};

/// The longest time between adjacent struck strings, in milliseconds.
pub const MAX_STRING_MS: f32 = 30.0;

/// How far a stroke's overall speed wanders at full variation, as a fraction of its spread.
const SPEED_WANDER: f32 = 0.35;

/// How far one string's gap wanders at full variation, as a fraction of an even gap.
const GAP_WANDER: f32 = 0.5;

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
    /// Milliseconds between adjacent struck pitches, clamped to `0..=MAX_STRING_MS`. When
    /// positive it replaces `spread_ms`, so a six-note chord takes longer than a power chord
    /// and an upstroke over fewer strings is quicker than the downstroke before it.
    pub string_ms: f32,
    /// How far each stroke's speed and string spacing wander, in 0..=1. Zero strikes every
    /// chord identically; the wander is reproducible and depends only on the chord's position.
    pub variation: f32,
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
            string_ms: 0.0,
            variation: 0.3,
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
        let stroke = Stroke::drawn(settings, played.len(), absolute, context.pass);
        let spread = if stroke.spread_ms <= 0.0 {
            0
        } else {
            milliseconds(stroke.spread_ms, context.bpm)
                .raw()
                .min(shortest - 1)
                .min((next - start).raw() - 1)
                .max(0)
        };
        let last_rank = played.len().saturating_sub(1).max(1);
        for (rank, &i) in played.iter().enumerate() {
            let order = if up { played.len() - 1 - rank } else { rank };
            // The epsilon keeps an even split exact where a third is not representable.
            let offset = Ticks((spread as f64 * stroke.position(order) + 1e-6).floor() as i64);
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

/// One sweep of the hand: how long it takes and where along it each string is struck.
struct Stroke {
    /// From the first struck string to the last, before the chord's own limits.
    spread_ms: f32,
    /// Each string's place along the sweep, from 0 at the first to 1 at the last.
    positions: Vec<f64>,
}

impl Stroke {
    /// The sweep over `strings` notes of the chord at `absolute`, wandering by `variation`.
    ///
    /// Each chord draws from a stream named by its timeline position and loop pass, so the
    /// wander of one chord never depends on the chords around it or on an edit elsewhere, and a
    /// copied clip is struck differently from its original.
    fn drawn(settings: &Strum, strings: usize, absolute: Ticks, pass: u64) -> Self {
        let variation = settings.variation.clamp(0.0, 1.0);
        let mut draw = Rng::stream(
            0,
            &[
                Key::Word("strum"),
                Key::Index(pass),
                Key::Index(absolute.raw().max(0) as u64),
            ],
        );
        let mut wander = |amount: f32| 1.0 + amount * variation * (2.0 * draw.unit() - 1.0);
        let speed = wander(SPEED_WANDER);
        let gaps: Vec<f64> = (1..strings)
            .map(|_| f64::from(wander(GAP_WANDER)))
            .collect();
        let base = if settings.string_ms > 0.0 {
            settings.string_ms.clamp(0.0, MAX_STRING_MS) * gaps.len() as f32
        } else {
            settings.spread_ms
        };
        let total: f64 = gaps.iter().sum();
        let mut positions = vec![0.0];
        let mut reached = 0.0;
        for gap in &gaps {
            reached += gap;
            positions.push(if total > 0.0 { reached / total } else { 0.0 });
        }
        Self {
            spread_ms: (base * speed).clamp(0.0, 100.0),
            positions,
        }
    }

    /// The fraction of the sweep elapsed when the string at `order` is struck.
    fn position(&self, order: usize) -> f64 {
        self.positions.get(order).copied().unwrap_or(0.0)
    }
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
    /// The onsets of each chord in `output`, relative to its first string, chord by chord.
    fn shapes(output: &[Note], strings: usize) -> Vec<Vec<i64>> {
        output
            .chunks(strings)
            .map(|chord| {
                let first = chord[0].start.raw();
                chord.iter().map(|note| note.start.raw() - first).collect()
            })
            .collect()
    }

    fn downstrokes(spread_ms: f32, string_ms: f32, variation: f32) -> Strum {
        Strum {
            spread_ms,
            string_ms,
            variation,
            direction: StrokeDirection::LowToHigh,
            clock: StrumClock::Attacks,
            ..Strum::default()
        }
    }

    #[test]
    fn without_variation_every_chord_is_split_evenly_and_alike() {
        let source = (0..4).flat_map(|i| chord(i * 480)).collect();
        let output = performed(
            source,
            downstrokes(30.0, 0.0, 0.0),
            Ticks::ZERO,
            SignatureMap::default(),
        );
        // 30 ms at 120 bpm is 58 ticks, split evenly over the three gaps of a four-note chord.
        assert_eq!(shapes(&output, 4), vec![vec![0, 19, 38, 58]; 4]);
    }

    #[test]
    fn string_time_makes_a_wide_chord_take_longer_than_a_power_chord() {
        let notes = |pitches: &[u8]| -> Vec<Note> {
            pitches
                .iter()
                .map(|&pitch| Note::new(pitch, Ticks::ZERO, Ticks(960)))
                .collect()
        };
        let settings = downstrokes(5.0, 10.0, 0.0);
        let power = performed(
            notes(&[40, 47, 52]),
            settings.clone(),
            Ticks::ZERO,
            SignatureMap::default(),
        );
        let open = performed(
            notes(&[40, 45, 50, 55, 59, 64]),
            settings,
            Ticks::ZERO,
            SignatureMap::default(),
        );
        // Ten milliseconds a string: two gaps for the power chord, five for the open chord.
        assert_eq!(
            shapes(&power, 3)[0].last(),
            Some(&milliseconds(20.0, 120.0).raw())
        );
        assert_eq!(
            shapes(&open, 6)[0].last(),
            Some(&milliseconds(50.0, 120.0).raw())
        );
    }

    #[test]
    fn strokes_wander_in_speed_and_spacing_but_keep_their_order() {
        let source: Vec<Note> = (0..8).flat_map(|i| chord(i * 480)).collect();
        let settings = downstrokes(30.0, 0.0, 1.0);
        let output = performed(
            source.clone(),
            settings.clone(),
            Ticks::ZERO,
            SignatureMap::default(),
        );
        let strokes = shapes(&output, 4);
        let even = milliseconds(30.0, 120.0).raw() as f32;
        for stroke in &strokes {
            assert!(
                stroke.windows(2).all(|pair| pair[0] < pair[1]),
                "{stroke:?}"
            );
            let spread = *stroke.last().unwrap() as f32;
            assert!(spread >= even * (1.0 - SPEED_WANDER) - 1.0, "{stroke:?}");
            assert!(spread <= even * (1.0 + SPEED_WANDER) + 1.0, "{stroke:?}");
        }
        let distinct: std::collections::BTreeSet<_> = strokes.iter().collect();
        assert!(distinct.len() > 4, "strokes should differ: {strokes:?}");
        let again = performed(source, settings, Ticks::ZERO, SignatureMap::default());
        assert_eq!(output, again, "the wander is reproducible");
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
