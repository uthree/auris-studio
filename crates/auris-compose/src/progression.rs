//! The progression the composer invents when a section says `chords = "?"`.
//!
//! Everything downstream of a [`ChartOrigin::Generated`] chart — the mood colouring its
//! qualities, the turnaround leaning its last bar into an arrival — was built for a progression
//! the composer made up, and until this module the only progression the composer ever made up
//! was the default `@axis`. This is the other half: a chart of the composer's own, drawn from
//! the seed, different for every song and the same for every playing of it.
//!
//! # How it composes
//!
//! A weighted walk over root degrees spans the requested section. Balanced phrases of at most
//! four bars lean onto the dominant at their ends and sometimes restate the opening. The
//! section's final bar leans toward a cadence. The walk's weights are **the catalogue, counted**:
//! every move between two chords in the major-mode entries of
//! [`CATALOG`](crate::theory::chart::CATALOG), wraparound included
//! because those charts are loops. That is the same trick the melody's interval table pulls —
//! the named progressions are a corpus of what this music actually does, and 王道進行's
//! V → iii "retrogression" is major vocabulary in it, where a textbook table would forbid the
//! move and generate chorales.
//!
//! The minor table cannot be counted the same way, because the minor-mode catalogue is two
//! entries. Their moves carry the heaviest weights and the rest of the row is the textbook,
//! marked as such below.
//!
//! Qualities are left to others on purpose. The walk emits plain triads (the minor dominant's
//! `V` excepted, which carries its major third explicitly so nothing demotes it), because
//! sevenths and ninths are what [`colour`](crate::frame) already adds in proportion to the
//! mood's tension — per section and per playing, which is finer-grained than a chart, shared by
//! every section that names one, could ever be. Energy and tension add harmonic motion here:
//! a bar may split into an approach and its destination, with both transitions supported by
//! the same vocabulary. This works for any destination in either mode, including major ii–V.
//!
//! # Determinism
//!
//! One stream, named by the chart's own name — `["progression", "sabi"]` — so two sections
//! pointing at one unwritten chart hear one progression, two unwritten charts in one song hear
//! two, and the seed dial re-deals all of them. Sharing also requires the same section length,
//! since length determines phrase boundaries. Harmonic rhythm has a separate random stream,
//! so changing density preserves the destination chords.

use crate::rng::{Key as RngKey, Rng};
use crate::spec::Mood;
use crate::theory::chart::{Chart, ChartMode, ChartOrigin};
use crate::theory::chord::Quality;
use crate::theory::key::Key;
use crate::theory::numeral::Numeral;

/// How often a later phrase opens by restating the first's opening chord.
///
/// Restating the beginning ties successive phrases together. Well over half, because the
/// catalogue's own eight-bar
/// entries (the canon and 純情進行) both do it.
const RESTATE: f32 = 0.6;

/// The degrees a major-mode walk moves between, in the spelling the chart will carry.
///
/// No `vii`: the catalogue never lands on it, and a diminished triad as a *bar* of harmony is a
/// chorale's move, not a song's.
const MAJOR_STATES: [&str; 6] = ["I", "ii", "iii", "IV", "V", "vi"];

/// The degrees a minor-mode walk moves between.
///
/// The flat degrees are spelled with their accidentals exactly as the minor catalogue entries
/// spell them, which is what makes the numerals resolve to the minor key's own chords. `V` is
/// the harmonic-minor dominant, not natural minor's `v` — the one place this vocabulary insists
/// on a quality.
const MINOR_STATES: [&str; 6] = ["i", "bIII", "iv", "V", "bVI", "bVII"];

/// The major-mode moves, counted from the catalogue.
///
/// `MAJOR_MOVES[from][to]` over [`MAJOR_STATES`]: the number of times the major-mode entries of
/// [`CATALOG`](crate::theory::chart::CATALOG) move from one degree to the other, adjacent
/// chords within each chart plus the
/// wraparound from its last chord to its first, secondary and slash chords counted by their
/// root degree. `the_tables_are_the_catalogue_counted` recounts it, so a catalogue that gains
/// an entry fails a test here rather than silently leaving this table describing a corpus that
/// no longer exists.
const MAJOR_MOVES: [[f32; 6]; 6] = [
    [3.0, 2.0, 0.0, 5.0, 7.0, 2.0],
    [0.0, 0.0, 0.0, 0.0, 2.0, 0.0],
    [0.0, 0.0, 0.0, 2.0, 0.0, 5.0],
    [8.0, 0.0, 2.0, 1.0, 6.0, 0.0],
    [7.0, 0.0, 3.0, 1.0, 0.0, 5.0],
    [1.0, 0.0, 2.0, 8.0, 1.0, 0.0],
];

/// The minor-mode moves: the two-entry catalogue's counts, filled out from the textbook.
///
/// `@epic` and `@andalusian` are the whole minor corpus, and eight observed moves do not make a
/// table. Every observed move carries a weight of `2.0` or more; every `1.0` is the textbook —
/// the plagal `iv`, the dominant's deceptive fall to `bVI` — added so the walk has somewhere to
/// go, and marked at this weight so the counted moves still dominate. The test only holds this
/// half of the table to "everything counted is possible".
const MINOR_MOVES: [[f32; 6]; 6] = [
    [0.0, 1.0, 2.0, 1.0, 2.0, 2.0],
    [0.0, 0.0, 1.0, 0.0, 1.0, 2.0],
    [1.0, 0.0, 0.0, 2.0, 1.0, 1.0],
    [3.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 2.0, 1.0, 2.0, 0.0, 1.0],
    [2.0, 1.0, 0.0, 0.0, 2.0, 0.0],
];

/// Where a major-mode phrase may open, over [`MAJOR_STATES`].
///
/// The tonic mostly, or the two off-tonic openings the catalogue itself uses: `vi` (小室進行,
/// the fifties loop's relative-minor cousins) and `IV` (丸サ進行 and the whole 王道 family).
const MAJOR_OPENINGS: [f32; 6] = [4.0, 0.0, 0.0, 1.5, 0.0, 2.0];

/// Where a minor-mode phrase may open, over [`MINOR_STATES`].
const MINOR_OPENINGS: [f32; 6] = [4.0, 0.0, 1.0, 0.0, 1.5, 0.0];

/// Invents the progression an unwritten chart stands for.
///
/// `name` is the chart's name in the song, which is the stream the draw comes from: the same
/// seed, name, key, length and mood invent the same progression, which lets a `.asong` that says
/// `chords = "?"` describe one reproducible piece. The chart comes back
/// [`ChartOrigin::Generated`], so the mood may colour it and the turnaround may lean on it —
/// that is not a courtesy, it is the point.
///
/// `bar_count` is the full section length; zero returns an empty chart.
pub fn invent_chart(seed: u64, name: &str, key: Key, mood: Mood, bar_count: usize) -> Chart {
    let mode = ChartMode::of(key);
    let minor = mode == ChartMode::Minor;
    let (states, moves, openings) = if minor {
        (&MINOR_STATES, &MINOR_MOVES, &MINOR_OPENINGS)
    } else {
        (&MAJOR_STATES, &MAJOR_MOVES, &MAJOR_OPENINGS)
    };
    // Indices, not constants, because the dominant sits at a different position in each
    // vocabulary and a hard-coded 4 would quietly bias the wrong minor degree.
    let tonic = 0;
    let dominant = states
        .iter()
        .position(|state| *state == "V")
        .expect("both vocabularies hold a dominant");
    let subdominant = states
        .iter()
        .position(|state| *state == "IV" || *state == "iv")
        .expect("both vocabularies hold a subdominant");

    let mut rng = Rng::stream(seed, &[RngKey::Word("progression"), RngKey::Word(name)]);

    // The walk itself: one root degree per bar. Two draws per bar whatever happens to them, so
    // the eighth bar of one seed is drawn from the same point of the stream as the eighth bar
    // of any other.
    let phrases = phrase_lengths(bar_count);
    let mut phrase_start = 0;
    let mut phrase_index = 0;
    let mut line: Vec<usize> = Vec::with_capacity(bar_count);
    for bar in 0..bar_count {
        let phrase_end = phrase_start + phrases[phrase_index] - 1;
        let mut weights = match line.last() {
            None => *openings,
            Some(previous) => moves[*previous],
        };
        if bar == phrase_end && bar + 1 < bar_count {
            // A phrase leans onto the dominant and away from home, making the next phrase
            // feel like an answer. A lean and not a rule — a row
            // with no dominant in it simply is not leant.
            weights[dominant] *= 2.5;
            weights[tonic] *= 0.5;
        }
        if bar + 1 == bar_count && bar > 0 {
            // The close: toward the dominant above all, the subdominant as the plagal second
            // choice, and almost never the tonic — a loop that ends at home has nowhere to go
            // when it comes round again.
            weights[dominant] *= 3.0;
            weights[subdominant] *= 1.5;
            weights[tonic] *= 0.3;
        }
        let step = rng.weighted(&weights);
        let restate = rng.chance(RESTATE);
        line.push(if bar == phrase_start && bar > 0 && restate {
            line[0]
        } else {
            step
        });
        if bar == phrase_end {
            phrase_start = bar + 1;
            phrase_index += 1;
        }
    }

    // A bridge preserves the destination while adding motion on the first half of the bar.
    // Excluding both endpoints avoids a repeated chord masquerading as harmonic motion.
    let mut rhythm = Rng::stream(seed, &[RngKey::Word("harmonic-rhythm"), RngKey::Word(name)]);
    let split_rate = (mood.energy * 0.35 + mood.tension * 0.4).clamp(0.0, 0.75);
    let mut bars: Vec<Vec<Numeral>> = Vec::with_capacity(bar_count);
    for (bar, state) in line.iter().enumerate() {
        let split = rhythm.chance(split_rate);
        let previous = line[bar.saturating_sub(1)];
        let weights: [f32; 6] = std::array::from_fn(|candidate| {
            if candidate == previous || candidate == *state {
                0.0
            } else {
                moves[previous][candidate] * moves[candidate][*state]
            }
        });
        let approach = rhythm.weighted(&weights);
        if bar > 0 && split && weights[approach] > 0.0 {
            bars.push(vec![
                state_numeral(states[approach], minor),
                state_numeral(states[*state], minor),
            ]);
        } else {
            bars.push(vec![state_numeral(states[*state], minor)]);
        }
    }

    Chart::new(bars, ChartOrigin::Generated).written_in(mode)
}

/// Divides the section evenly, avoiding a one-bar tail after a run of four-bar phrases.
/// For example, ten bars become 4 + 3 + 3, and five become 3 + 2.
fn phrase_lengths(bars: usize) -> Vec<usize> {
    let count = bars.div_ceil(4);
    (0..count)
        .map(|index| bars / count + usize::from(index < bars % count))
        .collect()
}

/// The numeral one state of the walk stands for.
///
/// Parsed from the vocabulary's own spelling rather than assembled field by field, so the chart
/// carries exactly what a person would have typed. The minor dominant is the one exception with
/// a quality written on it: a plain `V` is colourable, and colouring resolves a numeral through
/// the key's own scale — which in minor would quietly demote the dominant to natural minor's
/// `v` while adding its seventh.
fn state_numeral(state: &str, minor: bool) -> Numeral {
    let numeral = Numeral::parse(state).expect("the vocabulary parses");
    if minor && state == "V" {
        numeral.with_quality(Quality::Major)
    } else {
        numeral
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::chart::CATALOG;
    use auris_core::time::{TICKS_PER_QUARTER, Ticks};

    /// One bar of four four, in ticks.
    const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);

    fn major() -> Key {
        Key::parse("C major").unwrap()
    }

    fn minor() -> Key {
        Key::parse("A minor").unwrap()
    }

    /// The state a catalogue numeral counts as: its root degree's position in `states`.
    fn state_of(states: &[&str; 6], numeral: &Numeral) -> Option<usize> {
        states.iter().position(|state| {
            let vocabulary = Numeral::parse(state).unwrap();
            vocabulary.degree == numeral.degree && vocabulary.accidental == numeral.accidental
        })
    }

    /// Counts every move in the catalogue entries of one mode, wraparound included.
    fn count_catalogue(mode: ChartMode, states: &[&str; 6]) -> [[f32; 6]; 6] {
        let mut counted = [[0.0f32; 6]; 6];
        for entry in CATALOG {
            if entry.mode != mode {
                continue;
            }
            let chart = Chart::parse(entry.chart).unwrap();
            let flattened: Vec<Numeral> = chart.bars.iter().flatten().copied().collect();
            for (position, numeral) in flattened.iter().enumerate() {
                let next = &flattened[(position + 1) % flattened.len()];
                let from = state_of(states, numeral)
                    .unwrap_or_else(|| panic!("`{}` walks off the vocabulary", entry.name));
                let to = state_of(states, next)
                    .unwrap_or_else(|| panic!("`{}` walks off the vocabulary", entry.name));
                counted[from][to] += 1.0;
            }
        }
        counted
    }

    #[test]
    fn the_tables_are_the_catalogue_counted() {
        // The major table *is* the count: a new catalogue entry fails here, deliberately, so
        // the table is re-counted rather than left describing a corpus that no longer exists.
        assert_eq!(
            count_catalogue(ChartMode::Major, &MAJOR_STATES),
            MAJOR_MOVES
        );

        // The minor catalogue is two entries, so its table is counts plus the textbook; what is
        // held is that everything counted is possible, and heavier than anything merely added.
        let counted = count_catalogue(ChartMode::Minor, &MINOR_STATES);
        for from in 0..6 {
            for to in 0..6 {
                if counted[from][to] > 0.0 {
                    assert!(
                        MINOR_MOVES[from][to] >= 2.0,
                        "{} -> {} is in the catalogue but not really in the table",
                        MINOR_STATES[from],
                        MINOR_STATES[to],
                    );
                }
            }
        }
    }

    #[test]
    fn an_invented_progression_is_the_same_one_every_time() {
        let again = |seed, name: &str, key| invent_chart(seed, name, key, Mood::default(), 8);
        assert_eq!(again(7, "main", major()), again(7, "main", major()));
        assert_eq!(again(7, "main", minor()), again(7, "main", minor()));

        // A different name is a different progression — that is what lets one song hold an
        // invented verse and an invented chorus — and a different seed re-deals them all.
        assert_ne!(again(7, "main", major()), again(7, "sabi", major()));
        assert_ne!(again(7, "main", major()), again(8, "main", major()));
    }

    #[test]
    fn an_invented_progression_stays_inside_its_key() {
        for seed in 0..64 {
            for key in [major(), minor()] {
                let chart = invent_chart(seed, "main", key, Mood::default(), 8);
                assert_eq!(chart.bar_count(), 8);
                assert_eq!(chart.origin, ChartOrigin::Generated);
                assert!(!chart.is_unwritten());
                for event in chart.resolve(key, BAR) {
                    // Every root is a degree of the key. The harmonic-minor dominant's third
                    // is the one note outside the natural scale, and it is a chord tone —
                    // exactly the licence every part already has.
                    assert!(
                        key.scale.contains(key.tonic, event.chord.root),
                        "seed {seed}: {} has a root outside {}",
                        event.chord,
                        key.to_text(),
                    );
                }
            }
        }
    }

    #[test]
    fn the_phrase_opens_at_home_and_leans_on_the_dominant() {
        // Statistics over many deals, because any one chart is free to be the exception.
        let mut opens_home = 0;
        let mut closes_open = 0;
        let deals = 200;
        for seed in 0..deals {
            let chart = invent_chart(seed, "main", major(), Mood::default(), 8);
            let events = chart.resolve(major(), BAR);
            if events.first().unwrap().chord.root == major().tonic {
                opens_home += 1;
            }
            let last = events.last().unwrap();
            if last.chord.root == major().tonic.transposed(7)
                || last.chord.root == major().tonic.transposed(5)
            {
                closes_open += 1;
            }
        }
        assert!(
            opens_home * 2 > deals,
            "{opens_home}/{deals} open on the tonic"
        );
        assert!(
            closes_open * 2 > deals,
            "{closes_open}/{deals} close onto the dominant or the subdominant"
        );
    }

    #[test]
    fn busy_harmony_adds_connected_half_bar_chords_in_both_modes() {
        let tense = Mood {
            energy: 1.0,
            tension: 1.0,
            ..Mood::default()
        };
        let calm = Mood {
            energy: 0.0,
            tension: 0.0,
            ..Mood::default()
        };
        for key in [major(), minor()] {
            let mut split = 0;
            let mut destinations = std::collections::BTreeSet::new();
            for seed in 0..64 {
                let chart = invent_chart(seed, "main", key, tense, 13);
                let (states, moves) = if key.is_minor() {
                    (&MINOR_STATES, &MINOR_MOVES)
                } else {
                    (&MAJOR_STATES, &MAJOR_MOVES)
                };
                for (index, bar) in chart.bars.iter().enumerate() {
                    if bar.len() == 2 {
                        split += 1;
                        assert!(index > 0, "the opening is not displaced by an approach");
                        let previous =
                            state_of(states, chart.bars[index - 1].last().unwrap()).unwrap();
                        let approach = state_of(states, &bar[0]).unwrap();
                        let target = state_of(states, &bar[1]).unwrap();
                        assert_ne!(approach, previous);
                        assert_ne!(approach, target);
                        assert!(moves[previous][approach] > 0.0);
                        assert!(moves[approach][target] > 0.0);
                        destinations.insert(target);
                    }
                    assert!(bar.len() <= 2, "no bar holds more than an approach");
                }
                // And the walk itself is the same walk: tension splits bars, it does not re-deal
                // the progression underneath them.
                let roots: Vec<u8> = chart
                    .bars
                    .iter()
                    .map(|bar| bar.last().unwrap().degree)
                    .collect();
                let calm_chart = invent_chart(seed, "main", key, calm, 13);
                let calm_roots: Vec<u8> = calm_chart
                    .bars
                    .iter()
                    .map(|bar| bar.last().unwrap().degree)
                    .collect();
                assert_eq!(roots, calm_roots);
                assert!(calm_chart.bars.iter().all(|bar| bar.len() == 1));
            }
            assert!(
                split > 64,
                "busy harmony should regularly move within a bar"
            );
            assert!(
                destinations.len() >= 3,
                "approaches must serve more than the dominant"
            );
        }
    }

    #[test]
    fn sections_are_composed_to_length_including_odd_and_short_forms() {
        for bars in [0, 1, 2, 3, 5, 7, 9, 10, 13, 16, 31] {
            for key in [major(), minor()] {
                let chart = invent_chart(7, "main", key, Mood::default(), bars);
                assert_eq!(chart.bar_count(), bars);
                let events = chart.resolve(key, BAR);
                let mut end = Ticks::ZERO;
                for event in events {
                    assert_eq!(
                        event.start, end,
                        "harmony must cover the section without gaps"
                    );
                    assert!(event.length > Ticks::ZERO);
                    end = event.start + event.length;
                }
                assert_eq!(end, Ticks(BAR.raw() * bars as i64));
            }
        }
        let long = invent_chart(7, "main", major(), Mood::default(), 16);
        assert_ne!(
            long.bars[..8],
            long.bars[8..],
            "long sections must not tile eight bars"
        );
        assert_eq!(phrase_lengths(5), [3, 2]);
        assert_eq!(phrase_lengths(10), [4, 3, 3]);
    }

    #[test]
    fn the_minor_dominant_keeps_its_third() {
        // A plain V would be colourable, and colouring resolves through the key's own scale —
        // natural minor's v, minor. The vocabulary writes the quality on, so what reaches the
        // chart is the harmonic-minor dominant however the mood leans on it.
        for seed in 0..64 {
            let chart = invent_chart(seed, "main", minor(), Mood::default(), 8);
            for event in chart.resolve(minor(), BAR) {
                if event.chord.root == minor().tonic.transposed(7) {
                    assert!(!event.numeral.is_colourable());
                    assert!(!event.chord.quality.is_minor(), "{}", event.chord);
                }
            }
        }
    }

    #[test]
    fn a_whole_piece_can_be_written_over_an_invented_progression() {
        // End to end: the marker resolves inside `plan`, the mood colours what it resolves to,
        // and every part writes over it — the same pipeline every Generated chart rides.
        let spec = crate::SongSpec::parse(
            r#"
            seed = 3
            chords = "?"
            form = ["verse", "chorus"]
            "#,
        )
        .unwrap();
        let piece = crate::compose(&spec);
        assert!(piece.note_count() > 100, "{} notes", piece.note_count());
        assert_eq!(piece.summary(), crate::compose(&spec).summary());
    }

    /// What the composer invents, pinned.
    ///
    /// The intent is the module doc; this is the *outcome*, one chart per fixture, so a change
    /// to any constant above is a visible, deliberate act. When it moves: update the strings
    /// and prepend a line saying why.
    ///
    /// Updated for connecting half-bar approaches in both modes and a separate rhythm stream.
    #[test]
    fn the_inventor_writes_what_it_wrote_before() {
        let deal = |seed, key| invent_chart(seed, "main", key, Mood::default(), 8).to_string();
        assert_eq!(
            deal(0, major()),
            "| I | V | I vi | iii | IV | V I | vi | I IV |"
        );
        assert_eq!(
            deal(1, major()),
            "| I | IV I | vi IV | V | IV I | I | V | vi |"
        );
        assert_eq!(
            deal(0, minor()),
            "| i | bVII | i bVI | bIII | iv | bVI V | bVI | bIII iv |"
        );
    }
}
