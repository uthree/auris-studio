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
//! A weighted walk over root degrees, eight bars long, shaped like a period: two four-bar
//! phrases, the first leaning onto the dominant at its end and the second often restating the
//! opening before closing toward a cadence. The walk's weights are **the catalogue, counted**:
//! every move between two chords in the major-mode entries of [`CATALOG`], wraparound included
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
//! every section that names one, could ever be. What tension does add *here* is the ii–V: a
//! dominant bar may be split in two, which is the one change of harmonic rhythm this composer
//! makes and the reason a jazz-leaning mood gets `| ii V |` where a pop one gets `| V |`.
//!
//! # Determinism
//!
//! One stream, named by the chart's own name — `["progression", "sabi"]` — so two sections
//! pointing at one unwritten chart hear one progression, two unwritten charts in one song hear
//! two, and the seed dial re-deals all of them. Every draw is taken whether or not it is used,
//! so the phrase keeps its shape when one decision's odds are tuned.

use crate::rng::{Key as RngKey, Rng};
use crate::spec::Mood;
use crate::theory::chart::{Chart, ChartMode, ChartOrigin};
use crate::theory::chord::Quality;
use crate::theory::key::Key;
use crate::theory::numeral::Numeral;

/// How many bars an invented progression runs.
///
/// A period: two four-bar phrases. [`Chart::fit_to`] already turns that into anything a section
/// needs — a four-bar section takes the first phrase, which is written to stand alone, and a
/// sixteen-bar one plays the period twice.
const PHRASE_BARS: usize = 8;

/// How often the second phrase opens by restating the first's opening chord.
///
/// The antecedent–consequent shape: saying the beginning again is what makes eight bars one
/// period rather than two strangers. Well over half, because the catalogue's own eight-bar
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
/// [`CATALOG`] move from one degree to the other, adjacent chords within each chart plus the
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
/// seed and name always invent the same progression, which is what lets a `.asong` that says
/// `chords = "?"` describe one reproducible piece. The chart comes back
/// [`ChartOrigin::Generated`], so the mood may colour it and the turnaround may lean on it —
/// that is not a courtesy, it is the point.
pub fn invent_chart(seed: u64, name: &str, key: Key, mood: Mood) -> Chart {
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
    let mut line: Vec<usize> = Vec::with_capacity(PHRASE_BARS);
    for bar in 0..PHRASE_BARS {
        let mut weights = match line.last() {
            None => *openings,
            Some(previous) => moves[*previous],
        };
        if bar == PHRASE_BARS / 2 - 1 {
            // The half cadence: the first phrase leans onto the dominant and away from home,
            // which is what makes bar five feel like an answer. A lean and not a rule — a row
            // with no dominant in it simply is not leant.
            weights[dominant] *= 2.5;
            weights[tonic] *= 0.5;
        }
        if bar == PHRASE_BARS - 1 {
            // The close: toward the dominant above all, the subdominant as the plagal second
            // choice, and almost never the tonic — a loop that ends at home has nowhere to go
            // when it comes round again.
            weights[dominant] *= 3.0;
            weights[subdominant] *= 1.5;
            weights[tonic] *= 0.3;
        }
        let step = rng.weighted(&weights);
        let restate = rng.chance(RESTATE);
        line.push(if bar == PHRASE_BARS / 2 && restate {
            line[0]
        } else {
            step
        });
    }

    // The ii–V: a dominant bar may be split into approach and arrival. Major only — the minor
    // version is iiø7–V, a colour this vocabulary does not yet hold — but the roll is taken in
    // both modes and for every bar, so the walk above reads the same stream whatever the mood.
    let split_rate = (mood.tension * 0.4).min(0.35);
    let mut bars: Vec<Vec<Numeral>> = Vec::with_capacity(PHRASE_BARS);
    for (bar, state) in line.iter().enumerate() {
        let split = rng.chance(split_rate);
        let arriving = *state == dominant
            && bar > 0
            && line[bar - 1] != dominant
            && states[line[bar - 1]] != "ii";
        if !minor && split && arriving {
            bars.push(vec![state_numeral("ii", minor), state_numeral("V", minor)]);
        } else {
            bars.push(vec![state_numeral(states[*state], minor)]);
        }
    }

    Chart::new(bars, ChartOrigin::Generated).written_in(mode)
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
        let again = |seed, name: &str, key| invent_chart(seed, name, key, Mood::default());
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
                let chart = invent_chart(seed, "main", key, Mood::default());
                assert_eq!(chart.bar_count(), PHRASE_BARS);
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
            let chart = invent_chart(seed, "main", major(), Mood::default());
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
    fn tension_is_what_puts_a_two_five_in_a_bar() {
        let tense = Mood {
            tension: 1.0,
            ..Mood::default()
        };
        let calm = Mood {
            tension: 0.0,
            ..Mood::default()
        };
        let mut split = 0;
        for seed in 0..64 {
            let chart = invent_chart(seed, "main", major(), tense);
            for bar in &chart.bars {
                if bar.len() == 2 {
                    split += 1;
                    // The split bar is the ii–V, approach then arrival.
                    assert_eq!(bar[0].degree, 2);
                    assert_eq!(bar[1].degree, 5);
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
            let calm_chart = invent_chart(seed, "main", major(), calm);
            let calm_roots: Vec<u8> = calm_chart
                .bars
                .iter()
                .map(|bar| bar.last().unwrap().degree)
                .collect();
            assert_eq!(roots, calm_roots);
            assert!(calm_chart.bars.iter().all(|bar| bar.len() == 1));
        }
        assert!(split > 0, "full tension never wrote a single ii–V");
    }

    #[test]
    fn the_minor_dominant_keeps_its_third() {
        // A plain V would be colourable, and colouring resolves through the key's own scale —
        // natural minor's v, minor. The vocabulary writes the quality on, so what reaches the
        // chart is the harmonic-minor dominant however the mood leans on it.
        for seed in 0..64 {
            let chart = invent_chart(seed, "main", minor(), Mood::default());
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
    /// It last moved when it was written.
    #[test]
    fn the_inventor_writes_what_it_wrote_before() {
        let deal = |seed, key| invent_chart(seed, "main", key, Mood::default()).to_string();
        assert_eq!(deal(0, major()), "| I | V | vi | iii | IV | I | vi | IV |");
        assert_eq!(deal(1, major()), "| I | I | IV | V | I | I | V | vi |");
        assert_eq!(
            deal(0, minor()),
            "| i | bVII | bVI | bIII | iv | V | bVI | iv |"
        );
    }
}
