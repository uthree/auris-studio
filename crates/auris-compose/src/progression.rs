//! Generated harmony is a sequence of short, complete phrases.
//!
//! Choose a phrase in the song's mode, fit it to two to four bars, and repeat the opening
//! phrase every other time to give the section an identity. Busy moods may add a half-bar
//! preparation before V. Chord colour and the arrival into the next section belong to `frame`.

use crate::rng::{Key as RngKey, Rng};
use crate::spec::Mood;
use crate::theory::chart::{Chart, ChartMode, ChartOrigin};
use crate::theory::chord::Quality;
use crate::theory::key::Key;
use crate::theory::numeral::Numeral;

/// Complete phrases, so each choice already has an opening, motion and a destination.
const MAJOR_PHRASES: [[&str; 4]; 6] = [
    ["I", "vi", "IV", "V"],
    ["I", "iii", "IV", "V"],
    ["I", "IV", "ii", "V"],
    ["vi", "IV", "I", "V"],
    ["IV", "V", "iii", "vi"],
    ["I", "V", "vi", "IV"],
];

/// Minor phrases spell the flat degrees explicitly and use the harmonic-minor dominant.
const MINOR_PHRASES: [[&str; 4]; 6] = [
    ["i", "bVI", "iv", "V"],
    ["i", "iv", "bVII", "bVI"],
    ["i", "bIII", "bVI", "V"],
    ["i", "bVII", "bVI", "V"],
    ["bVI", "bVII", "i", "V"],
    ["i", "bVI", "bIII", "bVII"],
];

/// Generates a section's harmony from its seed, chart name, key, mood and length.
///
/// The section is covered exactly, including odd lengths; zero bars gives an empty chart.
/// Rhythm has its own random stream, so changing energy or tension keeps the phrase chords.
pub fn invent_chart(seed: u64, name: &str, key: Key, mood: Mood, bar_count: usize) -> Chart {
    let mode = ChartMode::of(key);
    let minor = mode == ChartMode::Minor;
    let phrases = if minor {
        &MINOR_PHRASES
    } else {
        &MAJOR_PHRASES
    };
    let mut rng = Rng::stream(seed, &[RngKey::Word("progression"), RngKey::Word(name)]);
    let mut rhythm = Rng::stream(seed, &[RngKey::Word("harmonic-rhythm"), RngKey::Word(name)]);
    let opening = rng.below(phrases.len());
    let split_rate = (mood.energy * mood.tension * 0.75).clamp(0.0, 0.75);
    let mut bars = Vec::with_capacity(bar_count);
    for (index, length) in phrase_lengths(bar_count).into_iter().enumerate() {
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
            let state = phrase[slot];
            let mut bar = Vec::with_capacity(2);
            let split = rhythm.chance(split_rate);
            if offset > 0 && state == "V" && split {
                bar.push(state_numeral(if minor { "iv" } else { "ii" }, minor));
            }
            bar.push(state_numeral(state, minor));
            bars.push(bar);
        }
    }
    Chart::new(bars, ChartOrigin::Generated).written_in(mode)
}

/// Balanced phrases avoid a one-bar tail: five bars become 3 + 2, ten become 4 + 3 + 3.
fn phrase_lengths(bars: usize) -> Vec<usize> {
    let count = bars.div_ceil(4);
    (0..count)
        .map(|index| bars / count + usize::from(index < bars % count))
        .collect()
}

/// Keep V major in minor, including when the planner adds chord colour.
fn state_numeral(state: &str, minor: bool) -> Numeral {
    let numeral = Numeral::parse(state).expect("the phrase vocabulary parses");
    if minor && state == "V" {
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
        assert_eq!(phrase_lengths(5), [3, 2]);
        assert_eq!(phrase_lengths(10), [4, 3, 3]);
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
