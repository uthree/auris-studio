//! Generated harmony is a sequence of short, complete phrases.
//!
//! Choose a phrase in the song's mode, fit it to two to four bars, and repeat the opening
//! phrase every other time to give the section an identity. Busy tonal phrases may add a
//! half-bar preparation before V. Chord colour and section arrivals belong to `frame`.

use crate::rng::{Key as RngKey, Rng};
use crate::spec::Mood;
use crate::theory::chart::{Chart, ChartMode, ChartOrigin};
use crate::theory::chord::Quality;
use crate::theory::key::Key;
use crate::theory::numeral::Numeral;
use crate::theory::scale::ScaleId;

/// Complete phrases, so each choice already has an opening, motion and a destination.
const MAJOR_PHRASES: [[u8; 4]; 6] = [
    [1, 6, 4, 5],
    [1, 3, 4, 5],
    [1, 4, 2, 5],
    [6, 4, 1, 5],
    [4, 5, 3, 6],
    [1, 5, 6, 4],
];

/// Minor phrases read their degrees directly from the scale.
const MINOR_PHRASES: [[u8; 4]; 6] = [
    [1, 6, 4, 5],
    [1, 4, 7, 6],
    [1, 3, 6, 5],
    [1, 7, 6, 5],
    [6, 7, 1, 5],
    [1, 6, 3, 7],
];

/// A characteristic chord and a supporting chord are enough to adapt the shared modal phrases.
fn modal_degrees(scale: ScaleId) -> Option<(u8, u8)> {
    match scale {
        ScaleId::Dorian => Some((4, 7)),
        ScaleId::Lydian => Some((2, 6)),
        ScaleId::Mixolydian => Some((7, 4)),
        ScaleId::Phrygian => Some((2, 4)),
        _ => None,
    }
}

/// The modal approach to the tonic; these modes do not need a tonal dominant cadence.
pub(crate) fn modal_approach(key: Key) -> Option<Numeral> {
    modal_degrees(key.scale).map(|(colour, _)| degree_numeral(colour, key))
}

fn phrases(key: Key) -> [[u8; 4]; 6] {
    if let Some((c, s)) = modal_degrees(key.scale) {
        // Every phrase anchors the tonic and visits the characteristic chord. Reusing this
        // shape keeps new modes a vocabulary change, not another progression algorithm.
        [
            [1, c, s, c],
            [1, s, c, 1],
            [1, c, 1, c],
            [1, s, 1, c],
            [1, c, s, 1],
            [1, c, 1, s],
        ]
    } else if key.is_minor() {
        MINOR_PHRASES
    } else {
        MAJOR_PHRASES
    }
}

/// Generates a section's harmony from its seed, chart name, key, mood and length.
///
/// The section is covered exactly, including odd lengths; zero bars gives an empty chart.
/// Rhythm has its own random stream, so changing energy or tension keeps the phrase chords.
pub fn invent_chart(seed: u64, name: &str, key: Key, mood: Mood, bar_count: usize) -> Chart {
    invent_chart_styled(seed, name, key, mood, bar_count, None)
}

/// Generates harmony against the same phrase boundaries used by the score writers.
pub fn invent_chart_styled(
    seed: u64,
    name: &str,
    key: Key,
    mood: Mood,
    bar_count: usize,
    style: Option<crate::PerformanceStyle>,
) -> Chart {
    let mode = ChartMode::of(key);
    let phrases = phrases(key);
    let modal = modal_degrees(key.scale).is_some();
    let mut rng = Rng::stream(seed, &[RngKey::Word("progression"), RngKey::Word(name)]);
    let mut rhythm = Rng::stream(seed, &[RngKey::Word("harmonic-rhythm"), RngKey::Word(name)]);
    let opening = rng.below(phrases.len());
    let split_rate = (mood.energy * mood.tension * 0.75).clamp(0.0, 0.75);
    let mut bars = Vec::with_capacity(bar_count);
    for (index, phrase_plan) in crate::phrasing::plan_phrases(bar_count, style)
        .into_iter()
        .enumerate()
    {
        let length = phrase_plan.bars;
        let phrase = &phrases[if index % 2 == 0 {
            opening
        } else {
            rng.below(phrases.len())
        }];
        for offset in 0..length {
            // Short phrases retain their opening and destination; the middle is compressed.
            let slot = if length == 1 {
                0
            } else {
                (offset * 3).div_ceil(length - 1)
            };
            let state = if length < 4 && offset == 1 {
                modal_degrees(key.scale).map_or(phrase[slot], |(colour, _)| colour)
            } else {
                phrase[slot]
            };
            let mut bar = Vec::with_capacity(2);
            let split = rhythm.chance(split_rate);
            if offset > 0 && !modal && state == 5 && split {
                bar.push(degree_numeral(if key.is_minor() { 4 } else { 2 }, key));
            }
            bar.push(degree_numeral(state, key));
            bars.push(bar);
        }
    }
    Chart::new(bars, ChartOrigin::Generated).written_in(mode)
}

/// Stack the chosen scale's thirds; only tonal minor asks for a raised leading tone.
fn degree_numeral(degree: u8, key: Key) -> Numeral {
    let numeral = Numeral::new(degree, false).as_diatonic(key);
    if degree == 5
        && matches!(
            key.scale,
            ScaleId::Minor
                | ScaleId::HarmonicMinor
                | ScaleId::MelodicMinor
                | ScaleId::MinorPentatonic
                | ScaleId::Blues
        )
    {
        numeral.with_quality(Quality::Major)
    } else {
        numeral
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::{TICKS_PER_QUARTER, Ticks};

    #[test]
    fn modal_phrases_anchor_the_tonic_and_sound_the_characteristic_note() {
        for (scale, character) in [
            ("dorian", 9),
            ("lydian", 6),
            ("mixolydian", 10),
            ("phrygian", 1),
        ] {
            for tonic in ["C", "D", "Eb", "F#"] {
                let key = Key::parse(&format!("{tonic} {scale}")).unwrap();
                for seed in 0..32 {
                    for bars in [0, 1, 2, 3, 4, 5, 7, 8, 13] {
                        let chart =
                            invent_chart(seed, "verse", key, Mood::named("tense").unwrap(), bars);
                        assert_eq!(chart.bar_count(), bars);
                        assert_eq!(
                            chart,
                            invent_chart(seed, "verse", key, Mood::named("tense").unwrap(), bars)
                        );
                        let events = chart.resolve(key, Ticks(TICKS_PER_QUARTER * 4));
                        for event in &events {
                            assert!(event.chord.quality.intervals().iter().all(|&i| {
                                key.scale
                                    .contains(key.tonic, event.chord.root.transposed(i))
                            }));
                        }
                        if bars > 0 {
                            assert_eq!(events[0].chord.root, key.tonic);
                        }
                        if bars >= 4 {
                            assert!(
                                events.iter().any(|event| event
                                    .chord
                                    .quality
                                    .intervals()
                                    .iter()
                                    .any(|&i| event.chord.root.transposed(i)
                                        == key.tonic.transposed(character))),
                                "{tonic} {scale}, seed {seed}, bars {bars}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn phrases_cover_every_length_and_stay_in_the_key() {
        let bar_ticks = Ticks(TICKS_PER_QUARTER * 4);
        for mode in ["C major", "A minor"] {
            let key = Key::parse(mode).unwrap();
            for bars in [0, 1, 2, 3, 4, 5, 7, 9, 10, 13, 16, 31] {
                for seed in 0..64 {
                    let chart = invent_chart(seed, "main", key, Mood::default(), bars);
                    assert_eq!(chart.bar_count(), bars);
                    assert_eq!(chart.origin, ChartOrigin::Generated);
                    let mut end = Ticks::ZERO;
                    for event in chart.resolve(key, bar_ticks) {
                        assert_eq!(event.start, end);
                        assert!(event.length > Ticks::ZERO);
                        assert!(key.scale.contains(key.tonic, event.chord.root));
                        if key.is_minor() && event.chord.root == key.tonic.transposed(7) {
                            assert_eq!(event.chord.quality, Quality::Major);
                            assert!(!event.numeral.is_colourable());
                        }
                        end = event.end();
                    }
                    assert_eq!(end, bar_ticks * bars as i64);
                }
            }
        }
    }

    #[test]
    fn seeds_and_names_choose_repeatable_phrases_with_variety() {
        for mode in ["C major", "A minor"] {
            let key = Key::parse(mode).unwrap();
            let mut choices = std::collections::BTreeSet::new();
            for seed in 0..64 {
                for name in ["verse", "chorus"] {
                    let chart = invent_chart(seed, name, key, Mood::default(), 16);
                    assert_eq!(chart, invent_chart(seed, name, key, Mood::default(), 16));
                    let roots: Vec<_> = chart.bars.iter().map(|bar| *bar.last().unwrap()).collect();
                    assert_eq!(roots[..4], roots[8..12], "the opening phrase returns");
                    choices.insert(chart.to_string());
                }
            }
            assert!(
                choices.len() > 12,
                "takes must offer more than a fixed loop"
            );
        }
    }

    #[test]
    fn busy_moods_prepare_dominants_without_redealing_the_roots() {
        for mode in ["C major", "A minor"] {
            let key = Key::parse(mode).unwrap();
            let mut splits = 0;
            for seed in 0..64 {
                let calm = invent_chart(
                    seed,
                    "main",
                    key,
                    Mood {
                        energy: 0.0,
                        tension: 0.0,
                        ..Mood::default()
                    },
                    13,
                );
                let busy = invent_chart(
                    seed,
                    "main",
                    key,
                    Mood {
                        energy: 1.0,
                        tension: 1.0,
                        ..Mood::default()
                    },
                    13,
                );
                for (plain, bar) in calm.bars.iter().zip(&busy.bars) {
                    assert_eq!(plain.len(), 1);
                    assert_eq!(plain.last(), bar.last());
                    assert!(bar.len() <= 2);
                    if bar.len() == 2 {
                        splits += 1;
                        assert_eq!(bar[1].degree, 5);
                        assert_eq!(bar[0].degree, if key.is_minor() { 4 } else { 2 });
                    }
                }
            }
            assert!(splits > 64);
        }
    }
}
