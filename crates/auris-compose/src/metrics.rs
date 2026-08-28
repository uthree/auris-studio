//! Measuring what the composer wrote: the numbers design decisions are checked against.
//!
//! Everywhere else in this crate a specification goes in and notes come out; here notes go in and
//! *measurements* come out. Nothing in the crate reads them back — they exist so that a change to
//! a writer can be judged against numbers instead of against whoever listened last, which is how
//! every level and timing constant in this workspace is already calibrated.
//!
//! The two measures below are the ones with literature behind them rather than the ones that were
//! easy to write. Syncopation is Longuet-Higgins and Lee's metric-weight formulation, the one the
//! groove studies use: Witek and colleagues measured pleasure and the urge to move against it and
//! found an inverted U, with the middle degrees of syncopation rated highest — so the number is a
//! dial to aim, not a score to maximise. Pitch-class entropy is the plainest of the corpus
//! statistics the symbolic-evaluation toolkits (MGEval, MusPy) settled on: how evenly a part
//! spreads itself over the twelve classes, from 0 for one note hammered to log₂12 for white noise.
//! A tune wants neither end.
//!
//! `cargo run -p auris-compose --example measure` prints both for every shipped preset, which is
//! the intended way to read them while turning a dial. The example is not part of any release
//! build; neither is this module's cost — nothing here allocates unless called.

use auris_core::Note;

use crate::rhythm::{Grid, Pattern};

/// How syncopated a rhythm is, as Longuet-Higgins and Lee measure it, per bar.
///
/// The pattern is read as a loop, which is what a groove is. For each onset, the strongest silent
/// step it sounds through — up to the next onset, wrapping past the end into the pattern's own
/// repeat — is found on the grid's metric hierarchy ([`Grid::weight`]); where that silence
/// outranks the onset's own step, the difference is added. The sum is divided by the bar count so
/// a two-bar pattern is comparable with a one-bar one.
///
/// Zero is a rhythm that never sounds against the meter: four on the floor, or straight eighths.
/// The classic anticipation — a hit on the last sixteenth with the following downbeat left
/// silent — scores 4, the full depth of the hierarchy, and the backbeat scores 3, which is the
/// measure being honest rather than wrong: a snare on two and four *is* a stroke against the
/// meter's grain, and that is the whole reason it carries a rock beat. The number ranks rhythms
/// against each other; it does not rank them against taste. Witek's inverted U says the middle of
/// this scale is where the pleasure sits, and where the middle is depends on what the rest of the
/// kit is doing.
///
/// An empty pattern, or one with no onsets, is not syncopated at all.
pub fn syncopation(pattern: &Pattern, grid: Grid) -> f64 {
    let steps_per_bar = grid.steps_per_bar();
    let onsets = pattern.onsets();
    let Some(first) = onsets.first().copied() else {
        return 0.0;
    };
    let bars = pattern.len().div_ceil(steps_per_bar).max(1);
    let end = bars * steps_per_bar;
    let mut score = 0u32;
    for (index, &onset) in onsets.iter().enumerate() {
        // The next onset, reading the pattern as a loop: past the last one, the first onset of
        // the next repeat. That wrap is what lets the downbeat after the bar count as the silence
        // an anticipation sounds against — the case the measure exists for.
        let next = onsets.get(index + 1).copied().unwrap_or(end + first);
        let struck = grid.weight(onset);
        let strongest = (onset + 1..next).map(|step| grid.weight(step)).max();
        if let Some(silent) = strongest
            && silent > struck
        {
            score += u32::from(silent - struck);
        }
    }
    f64::from(score) / bars as f64
}

/// How evenly a part spreads itself over the twelve pitch classes, in bits.
///
/// Shannon entropy of the pitch-class distribution, weighted by duration and velocity the same
/// way the key detector weighs it — a whole note says more about where the part lives than four
/// passing semiquavers. Zero for one class only; log₂12 ≈ 3.58 for all twelve equally, which no
/// tonal music reaches. A diatonic part spread evenly over its scale sits at log₂7 ≈ 2.81, and a
/// real tune sits below that, because a tune has favourite notes.
///
/// Read it as a range check, in both directions: a melody near zero has collapsed onto a drone,
/// and one pressing past the diatonic ceiling is either modulating or lost. Notes with no length
/// and no velocity — or no notes at all — measure 0.
pub fn pitch_class_entropy(notes: &[Note]) -> f64 {
    let weights = crate::analysis::pitch_weights(notes);
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    -weights
        .iter()
        .filter(|weight| **weight > 0.0)
        .map(|weight| {
            let p = weight / total;
            p * p.log2()
        })
        .sum::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::Ticks;

    fn grid() -> Grid {
        Grid::default()
    }

    fn pattern(text: &str) -> Pattern {
        Pattern::parse(text).expect("the fixture parses")
    }

    #[test]
    fn a_rhythm_on_the_beat_is_not_syncopated_at_all() {
        // Four on the floor and straight eighths: every silence an onset sounds through is
        // weaker than the onset's own step.
        assert_eq!(
            syncopation(&pattern("x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~"), grid()),
            0.0
        );
        assert_eq!(
            syncopation(&pattern("x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~"), grid()),
            0.0
        );
        // And the eight-beat kick — downbeat and halfway — is as square as rhythms get.
        assert_eq!(
            syncopation(&pattern("x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~"), grid()),
            0.0
        );
    }

    #[test]
    fn the_classic_figures_score_what_the_literature_says_they_score() {
        // The anticipation: a hit on the last sixteenth, the downbeat it displaces left silent.
        // The strongest silence in reach is the downbeat itself, four levels up from a sixteenth.
        assert_eq!(
            syncopation(&pattern("~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ x"), grid()),
            4.0
        );
        // The backbeat: each snare sounds through a silent beat one level up (the half-bar after
        // two, the downbeat after four), 1 + 2 across the bar.
        assert_eq!(
            syncopation(&pattern("~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~"), grid()),
            3.0
        );
        // The sixteen-beat kick, hand-worked: the two off-sixteenth hits each sound through the
        // beat that follows them, two levels up apiece.
        assert_eq!(
            syncopation(&pattern("x ~ ~ x ~ ~ x ~ ~ ~ x ~ x ~ ~ ~"), grid()),
            4.0
        );
    }

    #[test]
    fn a_longer_pattern_is_measured_per_bar() {
        // The same bar twice is the same music, so it has to be the same number — the division
        // by bars is what makes an eight-bar drum part comparable with its own groove.
        let once = pattern("~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~");
        let twice = pattern("~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~");
        assert_eq!(syncopation(&once, grid()), syncopation(&twice, grid()));
    }

    #[test]
    fn silence_is_not_syncopated() {
        assert_eq!(syncopation(&Pattern::rests(16), grid()), 0.0);
        assert_eq!(syncopation(&Pattern::rests(0), grid()), 0.0);
    }

    #[test]
    fn the_grooves_rank_the_way_their_genres_say_they_should() {
        // The measure has to reproduce the ordering anybody would give by ear: the straight
        // eight-beat under the funk-leaning sixteen-beat, and the clave-carrying bossa above
        // both. This is the property the example's per-preset table stands on.
        let kick = |name: &str| {
            let groove = crate::rhythm::groove(name).expect("listed");
            syncopation(&groove.pattern(crate::rhythm::DrumVoice::Kick), grid())
        };
        assert!(kick("eight-beat") < kick("sixteen-beat"), "the funk kick");
        let snare = |name: &str| {
            let groove = crate::rhythm::groove(name).expect("listed");
            syncopation(&groove.pattern(crate::rhythm::DrumVoice::Snare), grid())
        };
        assert!(
            snare("basic-rock") < snare("bossa-nova"),
            "the clave sounds against the meter more than a backbeat does"
        );
    }

    #[test]
    fn entropy_runs_from_a_drone_to_the_diatonic_ceiling() {
        let note = |pitch: u8, at: i64| Note::new(pitch, Ticks(at * 960), Ticks(960));
        // One class only: nothing to be uncertain about.
        assert_eq!(pitch_class_entropy(&[note(60, 0), note(72, 1)]), 0.0);
        // A scale spread evenly over its seven degrees is log₂7, to the precision of floats.
        let scale: Vec<Note> = [60u8, 62, 64, 65, 67, 69, 71]
            .into_iter()
            .enumerate()
            .map(|(index, pitch)| note(pitch, index as i64))
            .collect();
        assert!((pitch_class_entropy(&scale) - 7.0f64.log2()).abs() < 1e-9);
        // All twelve equally is the ceiling of the measure itself.
        let chromatic: Vec<Note> = (0..12u8)
            .map(|step| note(60 + step, i64::from(step)))
            .collect();
        assert!((pitch_class_entropy(&chromatic) - 12.0f64.log2()).abs() < 1e-9);
        // And weighting is the key detector's: a note of no length and no velocity says nothing.
        assert_eq!(pitch_class_entropy(&[]), 0.0);
        assert_eq!(
            pitch_class_entropy(&[Note::new(60, Ticks::ZERO, Ticks(0))]),
            0.0
        );
    }
}
