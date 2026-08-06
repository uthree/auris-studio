//! The tune.
//!
//! One figure per part and section, restated bar after bar and turned over where a phrase turns
//! over, which is what leaves a section with something an ear can hold on to. The motif, the way
//! it is varied and the two scale walks it is stated through are all here because nothing else in
//! the band writes one: this is the only part whose notes are a shape rather than a harmony.

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::PartSpec;
use crate::theory::chord_scale::ChordScale;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

use super::writer::{bar_onsets, bar_stream, density, dynamic, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// Fewest notes a generated figure is allowed to have.
///
/// Three is the smallest number that can carry a shape: two notes are an interval, and one is a
/// note. It is also the smallest [`vary_motif`] has anything to work with.
const MOTIF_MINIMUM: usize = 3;

/// A short figure the melody is built out of.
///
/// Written in scale steps from whatever pitch the frame's skeleton puts under it rather than in
/// absolute notes, so restating it over a different chord keeps its shape while still belonging
/// to the harmony.
#[derive(Clone, Debug)]
struct Motif {
    cells: Vec<Cell>,
}

/// One note of a [`Motif`].
#[derive(Copy, Clone, Debug)]
struct Cell {
    /// Step of the bar it starts on.
    step: usize,
    accent: Accent,
    /// Steps it sounds for. Fewer than the gap to the next cell leaves a rest.
    length: usize,
    /// Scale steps above or below the bar's anchor pitch.
    degree: i32,
}

/// Invents the figure a section is built from.
///
/// Drawn once per part and section and then restated, which is what gives a section something an
/// ear can hold on to. A part with a written rhythm gets that rhythm; only the shape is invented.
fn motif(
    grid: Grid,
    pattern: Option<&Pattern>,
    density: f32,
    syncopation: f32,
    rng: &mut Rng,
) -> Motif {
    let steps = grid.steps_per_bar();
    let mut onsets = bar_onsets(grid, pattern, density, syncopation, rng);
    // A figure needs a few notes to be one, and one note cannot be varied at all. A thin roll
    // used to average out because every bar rolled again; now a single roll decides the whole
    // section, so a thin one would leave the section with nothing rather than with a quiet bar.
    // The strongest free steps are filled first, which is where a note would have gone anyway.
    if pattern.is_none() && onsets.len() < MOTIF_MINIMUM {
        let mut spare: Vec<usize> = (0..steps)
            .filter(|step| !onsets.iter().any(|(taken, _)| taken == step))
            .collect();
        spare.sort_by_key(|step| std::cmp::Reverse(grid.weight(*step)));
        for step in spare.into_iter().take(MOTIF_MINIMUM - onsets.len()) {
            onsets.push((step, Accent::Normal));
        }
        onsets.sort_by_key(|(step, _)| *step);
    }

    let mut cells = Vec::with_capacity(onsets.len());
    let mut degree = 0i32;

    for (position, (step, accent)) in onsets.iter().enumerate() {
        // Mostly steps with the occasional leap, and bounded either side of the anchor: a figure
        // that wandered off would not be recognisable when it came back.
        if position > 0 {
            let move_by = *rng.pick(&[-2, -1, -1, 1, 1, 2, 3, -3]).unwrap_or(&1);
            degree = (degree + move_by).clamp(-6, 6);
        }
        let next = onsets
            .get(position + 1)
            .map(|(next, _)| *next)
            .unwrap_or(steps);
        let gap = next.saturating_sub(*step).max(1);
        // The figure's last note stops short of the next bar, which is where the rest that lets a
        // phrase breathe comes from. Inside the figure a note is occasionally detached too.
        let length = if position + 1 == onsets.len() || rng.chance(0.25) {
            1 + rng.below(gap)
        } else {
            gap
        };
        cells.push(Cell {
            step: *step,
            accent: *accent,
            length: length.clamp(1, gap),
            degree,
        });
    }
    Motif { cells }
}

/// The figure with one thing about it changed.
///
/// Enough to stop four bars of the same bar, not so much that it stops being the same figure —
/// which is the difference between a variation and a different tune.
fn vary_motif(figure: &Motif, rng: &mut Rng) -> Motif {
    let mut cells = figure.cells.clone();
    if cells.len() < 2 {
        return Motif { cells };
    }
    match rng.below(3) {
        // Move the last note somewhere else, which is what turns a statement into a question.
        0 => {
            let last = cells.len() - 1;
            cells[last].degree += if rng.chance(0.5) { 2 } else { -2 };
        }
        // Take a note out, leaving a hole where the ear expects one.
        1 if cells.len() > 2 => {
            let doomed = 1 + rng.below(cells.len() - 1);
            cells.remove(doomed);
        }
        // Turn the figure over from its second note on.
        _ => {
            for cell in cells.iter_mut().skip(1) {
                cell.degree = -cell.degree;
            }
        }
    }
    Motif { cells }
}

/// The tune.
pub(super) fn melody(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let grid = part_grid(frame, part);
    let (low, high) = part.range();
    let density = density(settings, part, section);

    // One figure per part and section, restated bar after bar. Keyed by neither the bar nor the
    // instance, so every bar of every playing reaches for the same one.
    let mut invent = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("motif"),
            RngKey::Word(&section.name),
        ],
    );
    let figure = motif(
        grid,
        part.rhythm.as_ref(),
        density,
        settings.mood.syncopation,
        &mut invent,
    );

    let mut notes = Vec::new();
    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "melody", bar);
        // Four bars is the phrase almost everything is built in: state the figure, restate it,
        // and then answer it. The fourth bar is where a tune stops repeating and goes somewhere.
        let closing = bar % 4 == 3;
        let cells = if closing || rng.chance(0.15) {
            vary_motif(&figure, &mut rng)
        } else {
            figure.clone()
        };
        let bar_start = grid.bar_ticks() * bar as i64;

        for cell in &cells.cells {
            let at = bar_start + grid.tick_of(cell.step);
            let Some(event) = section.chord_at(at) else {
                continue;
            };
            let event_index = section.event_index_at(at);
            let weight = grid.weight(cell.step);
            let anchor = section
                .skeleton
                .get(event_index)
                .copied()
                .unwrap_or((low + high) / 2);

            // The figure is written in scale steps from the chord's structural pitch, so it keeps
            // its shape while the harmony moves under it — in the scale of the *event's* key,
            // so a modulation inside the range moves the scale at the tick it moves the chords,
            // and in the scale the *chord* implies there, so a borrowed note is not answered by
            // the degree it borrowed from.
            let scale = ChordScale::new(event.key, event.chord);
            let mut pitch = shift_within(&scale, anchor, cell.degree, low, high);
            // A note on a strong step has to agree with the chord, or the figure's shape wins an
            // argument with the harmony that it should not be having.
            if weight >= 3 {
                pitch = fold_into(event.chord.nearest_tone(pitch), low, high);
            }

            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(weight, section.intensity, settings.dynamics)
                    * dynamic(cell.accent.scale(), settings.dynamics)
                    * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: grid.step_ticks() * cell.length.max(1) as i64,
            });
        }
    }
    notes
}

/// The pitch `steps` scale degrees from `from`.
///
/// The scale is a parameter rather than the section's, for two reasons. A section written over a
/// document can modulate inside itself, so the caller passes the key of the chord the note sounds
/// over; and a chord that borrows a note replaces the degree it borrowed from, so what the caller
/// passes is [`ChordScale`] rather than the key's own seven notes.
fn scale_shift(scale: &ChordScale, from: i32, steps: i32) -> i32 {
    let tonic = scale.tonic();
    let semitones = tonic.distance_up_to(PitchClass::new(from));
    let octaves = (from - tonic.midi(0) - semitones) / OCTAVE;
    let degree = scale.nearest_degree(semitones) + octaves * scale.degree_count() as i32;
    tonic.midi(0) + scale.semitone(degree + steps)
}

/// `anchor` shifted by `degree` scale steps, kept inside `low..=high` by shrinking the interval.
///
/// Folding an out-of-range note back by octaves moves it twelve semitones, which is a wider leap
/// than any the figure asked for — so a shape chosen to be smooth arrived with a jump in it that
/// nothing had priced. Pulling the interval in instead keeps the direction the figure was going,
/// which is what an ear follows. Folding is kept only for the case where even the anchor is out
/// of range, where there is nothing left to shrink.
fn shift_within(scale: &ChordScale, anchor: i32, degree: i32, low: i32, high: i32) -> i32 {
    let mut steps = degree;
    loop {
        let pitch = scale_shift(scale, anchor, steps);
        if (low..=high).contains(&pitch) {
            return pitch;
        }
        if steps == 0 {
            return fold_into(pitch, low, high);
        }
        steps -= steps.signum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixture::{BASE, bar_steps, draft, part};
    use crate::spec::Role;
    use auris_core::time::Ticks;

    #[test]
    fn the_melody_restates_its_figure_bar_after_bar() {
        // Every bar used to roll its own rhythm from its own stream, so no figure ever recurred
        // and a section had nothing in it to recognise.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                chords = "@axis"
                humanize = 0
                variation = 0
                [section.verse]
                bars = 8
                [[part]]
                name = "lead"
                "#,
        );
        let lead = part(&parts, "lead");
        let figure = bar_steps(&frame, lead, 0);
        assert!(!figure.is_empty(), "the melody played nothing at all");

        let restated = (0..8)
            .filter(|bar| bar_steps(&frame, lead, *bar) == figure)
            .count();
        assert!(
            restated >= 4,
            "only {restated} of 8 bars restate the figure {figure:?}"
        );
    }

    #[test]
    fn the_melody_leaves_room_to_breathe() {
        // A note used to be held until the next onset or the bar line, so every bar was full of
        // sound from end to end and a phrase never finished — it only stopped.
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
        let mut longest_rest = Ticks::ZERO;
        let mut sounded_to = Ticks::ZERO;
        for note in &lead.notes {
            longest_rest = longest_rest.max(note.start - sounded_to);
            sounded_to = sounded_to.max(note.start + note.length);
        }
        let beat = frame.grid.signature.ticks_per_beat();
        assert!(
            longest_rest >= beat,
            "the longest rest in eight bars is {} ticks, under one beat of {}",
            longest_rest.raw(),
            beat.raw()
        );
    }

    #[test]
    fn a_figure_too_wide_for_the_range_shrinks_rather_than_folding() {
        // Folding moves a note a whole octave, which is a wider leap than any figure asks for and
        // usually in the opposite direction to the one it was going.
        let (_, frame, _) = draft(BASE);
        let section = &frame.sections[0];
        let (low, high) = Role::Melody.range();
        let anchor = high - 2;
        let pitch = shift_within(&ChordScale::of_key(section.key), anchor, 6, low, high);
        assert!(
            (low..=high).contains(&pitch),
            "{pitch} is outside the range"
        );
        assert!(
            pitch > anchor - OCTAVE,
            "a figure reaching upward was folded an octave down: {pitch} from {anchor}"
        );
    }
}
