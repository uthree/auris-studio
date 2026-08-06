//! What the tests of every writer are written against.
//!
//! `#[cfg(test)]` and nothing else. The tests all ask the same two questions — write this
//! specification, then look at what one part played — so the recipe and the readers for it live
//! where each writer's own file can reach them rather than being copied into all six.

use crate::frame::{Frame, plan};
use crate::spec::SongSpec;

use super::{PartDraft, ScoreSettings, write_parts};

pub(super) fn draft(text: &str) -> (SongSpec, Frame, Vec<PartDraft>) {
    let spec = SongSpec::parse(text).expect("the fixture parses");
    let frame = plan(&spec);
    let parts = write_parts(&ScoreSettings::from(&spec), &spec.parts, &frame);
    (spec, frame, parts)
}

/// The steps of `bar` that `draft` starts a note on, without repeats.
pub(super) fn bar_steps(frame: &Frame, draft: &PartDraft, bar: usize) -> Vec<usize> {
    let bar_ticks = frame.grid.bar_ticks();
    let start = bar_ticks * bar as i64;
    let mut steps: Vec<usize> = draft
        .notes
        .iter()
        .filter(|note| note.start >= start && note.start < start + bar_ticks)
        .map(|note| frame.grid.step_of(note.start - start))
        .collect();
    steps.sort_unstable();
    steps.dedup();
    steps
}

/// Everything `draft` plays in one section, positioned from that section's own start.
///
/// Rebased so two playings of the same section can be compared directly; the velocity travels
/// as bits because two performances of the same music are equal or they are not.
pub(super) fn section_notes(
    frame: &Frame,
    draft: &PartDraft,
    section: usize,
) -> Vec<(i64, u8, i64, u32)> {
    let start = frame.sections[section].start;
    let mut notes: Vec<(i64, u8, i64, u32)> = draft
        .notes
        .iter()
        .filter(|note| note.section == section)
        .map(|note| {
            (
                (note.start - start).raw(),
                note.pitch,
                note.length.raw(),
                note.velocity.to_bits(),
            )
        })
        .collect();
    notes.sort_unstable();
    notes
}

/// Everything `draft` plays in one section except its final bar.
///
/// The last bar carries the fill into whatever comes next, which is a property of where the
/// section sits in the form rather than of the section itself — the last section of a piece
/// has nothing to lead into and so plays the groove to the end.
pub(super) fn section_body(
    frame: &Frame,
    draft: &PartDraft,
    section: usize,
) -> Vec<(i64, u8, i64, u32)> {
    let body = frame.sections[section].length - frame.grid.bar_ticks();
    section_notes(frame, draft, section)
        .into_iter()
        .filter(|(start, ..)| *start < body.raw())
        .collect()
}

pub(super) fn part<'a>(parts: &'a [PartDraft], name: &str) -> &'a PartDraft {
    parts
        .iter()
        .find(|part| part.name == name)
        .unwrap_or_else(|| panic!("no part called {name}"))
}

pub(super) const BASE: &str = r#"
        form = "verse"
        chords = "@axis"
        humanize = 0
        swing = 50
        [section.verse]
        bars = 4
    "#;

pub(super) fn roster(seed: u64, extra: &str) -> String {
    format!(
        r#"
            form     = "verse"
            chords   = "@axis"
            humanize = 0
            seed     = {seed}

            [section.verse]
            bars = 4

            [[part]]
            name = "lead"

            [[part]]
            name = "chords"
            {extra}

            [[part]]
            name = "bass"

            [[part]]
            name = "kick"
            "#
    )
}
