//! The seven helpers every role writer reads.
//!
//! A writer decides what its own part plays. These decide the handful of things a part cannot
//! decide by itself without the band coming apart: which grid it lands on, how busy it is, how
//! hard a note is struck, how a section leans as it goes, and which stream a bar draws its
//! material from. Sharing them is the whole of what makes five independent writers sound like one
//! group of players.
//!
//! A file of its own because that sharing is real rather than incidental — `bar_onsets` is read
//! by the melody and by the comp, so it belongs to neither and would have to be duplicated or
//! reached for across a seam if these lived with one writer.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{PartSpec, Role};

use super::ScoreSettings;

/// The grid a part's figures land on: the frame's meter at that part's own subdivision.
///
/// Per part and not per song, because a stab hammering triplets over a straight kit is the whole
/// reason for having the setting. The bar is the same length whichever way it is divided, so the
/// parts still line up at every bar line and every chord change.
///
/// A drum part is the exception and reads the frame's own grid. A groove is written in sixteenths
/// and read by index, so a kit on a grid of twelve would wrap its pattern a third of the way
/// through the bar: not a groove in triplets, a scrambled groove.
pub(super) fn part_grid(frame: &Frame, part: &PartSpec) -> Grid {
    if part.role.is_drum() {
        return frame.grid;
    }
    Grid::new(frame.grid.signature, part.subdivision.steps_per_beat())
}

/// How busy a part is, as a fraction of the available steps.
pub(super) fn density(settings: &ScoreSettings, part: &PartSpec, section: &SectionPlan) -> f32 {
    let base = part.density.unwrap_or_else(|| settings.mood.density());
    let role = match part.role {
        Role::Melody => 1.0,
        Role::Arp => 1.2,
        Role::Chords => 0.8,
        Role::Pad => 0.4,
        // A stab that is not busy is a chord part. The whole of what distinguishes it is that it
        // reaches for the figures nothing else does, so it starts near the top of the dial.
        Role::Stab => 1.3,
        Role::Bass => 0.9,
        _ => 1.0,
    };
    (base * role * (0.55 + 0.45 * section.intensity)).clamp(0.05, 1.0)
}

/// The grid weight the spread opens around.
///
/// The offbeat eighth, which is not the middle of the hierarchy but is close to the middle of
/// what actually gets written: four steps of a sixteen-step bar carry a beat and twelve do not,
/// and a part that favours the strong steps still puts most of its notes on the weak ones. Taking
/// the *hierarchy's* midpoint would have made flattening the dynamics audibly louder rather than
/// audibly flatter, which is the one thing this is meant not to do.
const MEAN_WEIGHT: f32 = 1.0;

/// A velocity for a note at grid weight `weight` in a section of `intensity`.
///
/// `dynamics` opens the spread *around* the level rather than raising the top of it, which is why
/// it is measured from a beat and not from zero. Widening it otherwise would quietly play the
/// whole part louder, and a control that changes two things is a control nobody can aim.
pub(super) fn velocity(weight: u8, intensity: f32, dynamics: f32) -> f32 {
    let spread = (f32::from(weight) - MEAN_WEIGHT) * 0.11 * dynamics.clamp(0.0, 1.0);
    let base = 0.45 + MEAN_WEIGHT * 0.11 + spread;
    (base * (0.7 + 0.35 * intensity)).clamp(0.08, 1.0)
}

/// A multiplier scaled toward 1 by `dynamics`.
///
/// Every other source of variation — an accent, the lean across a phrase — arrives as a factor
/// either side of unity, and flattening the hierarchy while leaving those at full strength would
/// leave a dial at zero still not flat. This is what makes it mean the same thing everywhere.
pub(super) fn dynamic(factor: f32, dynamics: f32) -> f32 {
    1.0 + (factor - 1.0) * dynamics.clamp(0.0, 1.0)
}

/// How hard a moment of a section is played, as a multiplier on its notes' velocity.
///
/// A section whose every bar sat at one level sounded like a loop rather than like a passage
/// going somewhere: the only dynamic in the piece was the step between one section's intensity
/// and the next's. This lifts the playing gently across each four-bar phrase and again across the
/// section as a whole, which is what a player does to a repeated figure without being asked to.
///
/// Every part reads it, so the band leans together rather than one instrument at a time.
pub(super) fn phrase_shape(grid: Grid, section: &SectionPlan, at: Ticks, dynamics: f32) -> f32 {
    let bar = (at.raw().max(0) / grid.bar_ticks().raw().max(1)) as usize;
    let within = (bar % 4) as f32 / 3.0;
    let through = if section.bars <= 1 {
        0.0
    } else {
        (bar.min(section.bars - 1)) as f32 / (section.bars - 1) as f32
    };
    dynamic(0.88 + 0.10 * within + 0.08 * through, dynamics)
}

/// The stream one bar of one pass draws its material from.
///
/// Keyed by the section's *name* and not by which playing of it this is, so the second chorus
/// reaches for the same numbers as the first and comes out the same chorus. That is the whole
/// point of a chorus: the composer used to put the instance in every stream, which made every
/// repeat a new piece of music and left the piece with nothing in it to recognise.
///
/// [`SongSpec::variation`](crate::spec::SongSpec::variation) buys the departures back. A bar it
/// selects mixes the instance into the name, so that one bar — and only that one — draws
/// different numbers and plays something else.
pub(super) fn bar_stream(
    settings: &ScoreSettings,
    frame: &Frame,
    part: &PartSpec,
    section: &SectionPlan,
    pass: &str,
    bar: usize,
) -> Rng {
    let mut path = vec![
        RngKey::Word("part"),
        RngKey::Word(&part.name),
        RngKey::Word(pass),
        RngKey::Word(&section.name),
        RngKey::Index(bar as u64),
    ];
    // The first playing is the one the others are repeats of, so it never departs from itself.
    if section.instance > 1 && settings.variation > 0.0 {
        let mut choose = Rng::stream(
            frame.seed,
            &[
                RngKey::Word("vary"),
                RngKey::Word(&part.name),
                RngKey::Word(pass),
                RngKey::Word(&section.name),
                RngKey::Index(section.instance as u64),
                RngKey::Index(bar as u64),
            ],
        );
        if choose.chance(settings.variation) {
            path.push(RngKey::Index(section.instance as u64));
        }
    }
    Rng::stream(frame.seed, &path)
}

/// Picks the onsets of one bar, either from a written rhythm or by rolling one.
///
/// The roll leans on the metric hierarchy: a strong step is far likelier to carry a note than a
/// weak one, which is what makes a generated rhythm feel like it is in the bar rather than
/// scattered across it.
pub(super) fn bar_onsets(
    grid: Grid,
    pattern: Option<&Pattern>,
    density: f32,
    syncopation: f32,
    rng: &mut Rng,
) -> Vec<(usize, Accent)> {
    let steps = grid.steps_per_bar();
    if let Some(pattern) = pattern {
        return (0..steps)
            .filter_map(|step| pattern.at(step).map(|accent| (step, accent)))
            .collect();
    }
    let mut onsets = Vec::new();
    for step in 0..steps {
        let weight = f32::from(grid.weight(step));
        // Syncopation lifts the weak steps toward the strong ones rather than adding notes.
        let pull = (weight / 4.0) * (1.0 - syncopation) + syncopation * 0.55;
        if rng.chance((density * (0.35 + 1.1 * pull)).clamp(0.0, 0.95)) {
            let accent = if grid.weight(step) >= 3 {
                Accent::Strong
            } else {
                Accent::Normal
            };
            onsets.push((step, accent));
        }
    }
    // A bar with nothing in it reads as a mistake rather than as a rest, so keep the downbeat.
    if onsets.is_empty() {
        onsets.push((0, Accent::Normal));
    }
    onsets
}

#[cfg(test)]
mod tests {
    use crate::parts::PartDraft;
    use crate::parts::fixture::{draft, part, roster, section_notes};

    #[test]
    fn a_section_leans_as_it_goes() {
        // Every bar used to be played at one level, so the only dynamic anywhere in a piece was
        // the step from one section's intensity to the next's.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                chords = "@axis"
                humanize = 0
                [section.verse]
                bars = 8
                [[part]]
                name = "lead"
                "#,
        );
        let lead = part(&parts, "lead");
        let mean = |from: i64, to: i64| -> f32 {
            let levels: Vec<f32> = lead
                .notes
                .iter()
                .filter(|note| note.start.raw() >= from && note.start.raw() < to)
                .map(|note| note.velocity)
                .collect();
            levels.iter().sum::<f32>() / levels.len().max(1) as f32
        };
        let half = frame.length.raw() / 2;
        let (early, late) = (mean(0, half), mean(half, frame.length.raw()));
        assert!(
            late > early,
            "the second half of the section is no louder than the first: {late} against {early}"
        );
    }

    #[test]
    fn variation_lets_a_repeat_depart_from_the_first_playing() {
        let text = r#"
            form = "verse verse"
            chords = "@axis"
            humanize = 0
            variation = 1.0
            [section.verse]
            bars = 4
            [[part]]
            name = "lead"
            "#;
        let (_, frame, parts) = draft(text);
        let lead = part(&parts, "lead");
        assert_ne!(
            section_notes(&frame, lead, 0),
            section_notes(&frame, lead, 1),
            "`variation: 1.0` left the repeat identical"
        );
    }

    #[test]
    fn a_part_on_a_triplet_grid_plays_where_no_straight_grid_reaches() {
        // The whole point of the setting: a position a sixteenth grid cannot express. A third of
        // a beat is 320 ticks, and no multiple of 320 but the beats themselves is a multiple of
        // the 240 a sixteenth is.
        //
        // Over several seeds, because the figure is now drawn once per section: a seed that draws
        // the held chord puts everything on a downbeat, which is a legitimate comp and lands on
        // both grids at once. What has to be true is that the setting is *reachable*.
        let mut off_the_straight_grid = 0;
        for seed in 1..=8u64 {
            let text = roster(seed, "subdivision = \"8t\"\n            density = 0.9");
            let (_, _, parts) = draft(&text);
            let chords = part(&parts, "chords");
            assert!(!chords.notes.is_empty(), "seed {seed} wrote nothing");
            for note in &chords.notes {
                assert_eq!(
                    note.start.raw() % 320,
                    0,
                    "seed {seed} put a note at {}, which is not on a triplet",
                    note.start.raw()
                );
            }
            if chords.notes.iter().any(|note| note.start.raw() % 240 != 0) {
                off_the_straight_grid += 1;
            }
        }
        assert!(
            off_the_straight_grid > 0,
            "not one of eight seeds put a note where a straight grid could not reach"
        );
    }

    /// Every velocity a part writes, as whole percent so a spread can be compared.
    fn levels(draft: &PartDraft) -> Vec<i32> {
        draft
            .notes
            .iter()
            .map(|note| (note.velocity * 100.0).round() as i32)
            .collect()
    }

    #[test]
    fn dynamics_opens_the_spread_without_moving_where_it_sits() {
        // Two claims, and the second is why this is not just a quieter intensity: turning the
        // dial down has to flatten the playing, not turn the part down. A control that changed
        // both would be a control nobody could aim.
        let spec = |dynamics: &str| {
            format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    seed = 4
                    dynamics = {dynamics}
                    [section.verse]
                    bars = 4
                    [[part]]
                    name = "lead"
                    "#
            )
        };
        let flat = draft(&spec("0")).2;
        let wide = draft(&spec("1")).2;
        let (flat, wide) = (part(&flat, "lead"), part(&wide, "lead"));

        let flat_levels = levels(flat);
        let wide_levels = levels(wide);
        assert_eq!(flat_levels.len(), wide_levels.len(), "the notes moved");

        let spread = |levels: &[i32]| {
            levels.iter().max().copied().unwrap_or(0) - levels.iter().min().copied().unwrap_or(0)
        };
        assert_eq!(
            spread(&flat_levels),
            0,
            "at zero every note is struck alike"
        );
        assert!(
            spread(&wide_levels) > 10,
            "at one the hierarchy is barely audible: {} percent",
            spread(&wide_levels)
        );

        // And the level stays roughly where it was. Roughly and not exactly: the spread opens
        // around a fixed point on the hierarchy, and which weights a part actually writes on
        // depends on the part. What matters is that flattening is heard as flattening rather
        // than as a change of level, and a few percent is well under that.
        let mean = |levels: &[i32]| levels.iter().sum::<i32>() as f32 / levels.len().max(1) as f32;
        assert!(
            (mean(&flat_levels) - mean(&wide_levels)).abs() < 8.0,
            "flattening moved the level: {} against {}",
            mean(&flat_levels),
            mean(&wide_levels)
        );
    }

    #[test]
    fn syncopation_moves_a_part_off_the_beat_without_making_it_busier() {
        // It lifts the weak steps toward the strong ones rather than adding notes, which is what
        // makes it a separate dial from the density rather than a second way to spell it.
        let spec = |syncopation: &str| {
            format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    seed = 6
                    syncopation = {syncopation}
                    [section.verse]
                    bars = 8
                    [[part]]
                    name = "lead"
                    "#
            )
        };
        let square = draft(&spec("0.0")).2;
        let awkward = draft(&spec("1.0")).2;
        let (square, awkward) = (part(&square, "lead"), part(&awkward, "lead"));

        let off_the_beat = |draft: &PartDraft| {
            draft
                .notes
                .iter()
                .filter(|note| note.start.raw() % 960 != 0)
                .count() as f32
                / draft.notes.len().max(1) as f32
        };
        assert!(
            off_the_beat(awkward) > off_the_beat(square),
            "{:.2} of the awkward take is off the beat against {:.2} of the square one",
            off_the_beat(awkward),
            off_the_beat(square)
        );
    }
}
