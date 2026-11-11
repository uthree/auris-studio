//! What the tests of every writer are written against.
//!
//! `#[cfg(test)]` and nothing else. The tests all ask the same two questions — write these parts
//! over these chords, then look at what one part played — so the recipe and the readers for it
//! live where each writer's own file can reach them rather than being copied into all six.
//!
//! A [`Song`] is written through [`frame_over`], the same one-section frame
//! [`write_phrase`](crate::write_phrase) builds. Its chords are the chart resolved bar by bar
//! rather than stamped into a document's harmony, which would merge a chord written twice into
//! one: a writer's tests say which spelling they mean.

use auris_core::time::TimeSignature;

use crate::frame::Frame;
use crate::parts::{PartDraft, ScoreSettings, write_parts};
use crate::phrase::frame_over;
use crate::rhythm::Grid;
use crate::spec::{Mood, PartSpec, Role};
use crate::theory::chart::Chart;
use crate::theory::key::Key;

/// One stretch of music and the parts to write over it.
#[derive(Clone, Debug)]
pub(crate) struct Song {
    /// The key the chords are read in.
    pub key: &'static str,
    /// The chart, in anything [`Chart::parse`] reads; it is fitted to `bars`.
    pub chords: String,
    /// How many bars are written.
    pub bars: usize,
    /// The meter.
    pub meter: TimeSignature,
    /// The seed every stream is drawn from.
    pub seed: u64,
    /// The label the streams are keyed by, with which occurrence of it this is.
    pub section: (&'static str, usize),
    /// How hard the stretch is played.
    pub intensity: f32,
    /// The dials every part reads; its mood is the frame's too.
    pub settings: ScoreSettings,
    /// The parts.
    pub parts: Vec<PartSpec>,
}

impl Default for Song {
    fn default() -> Self {
        Self {
            key: "C major",
            chords: "@axis".to_string(),
            bars: 4,
            meter: TimeSignature::default(),
            seed: 0,
            section: ("verse", 1),
            intensity: 0.55,
            settings: ScoreSettings {
                style: None,
                mood: Mood::default(),
                swing: 50,
                dynamics: 1.0,
                fill: 0.5,
                groove: "basic-rock".to_string(),
                motif: Vec::new(),
            },
            parts: vec![
                PartSpec::of_role("lead", Role::Melody),
                PartSpec::of_role("chords", Role::Chords),
                PartSpec::of_role("bass", Role::Bass),
                PartSpec::of_role("kick", Role::Kick),
                PartSpec::of_role("snare", Role::Snare),
                PartSpec::of_role("hat", Role::Hat),
            ],
        }
    }
}

impl Song {
    /// The same stretch with only `parts` playing.
    pub fn with_parts(parts: impl IntoIterator<Item = PartSpec>) -> Self {
        Self {
            parts: parts.into_iter().collect(),
            ..Self::default()
        }
    }

    /// One part of a role, named after it, with `patch` applied.
    pub fn part(role: Role, patch: impl FnOnce(&mut PartSpec)) -> PartSpec {
        let mut part = PartSpec::of_role(role.name(), role);
        patch(&mut part);
        part
    }

    /// The frame the parts are written against.
    pub fn frame(&self) -> Frame {
        let key = Key::parse(self.key).expect("the fixture's key parses");
        let chart = Chart::parse(&self.chords).expect("the fixture's chart parses");
        let bar = self.meter.ticks_per_bar();
        let length = bar * self.bars as i64;
        let events = chart.spelled_in(key).fit_to(self.bars).resolve(key, bar);
        frame_over(
            Grid::new(self.meter, 4),
            events,
            key,
            length,
            self.seed,
            self.section,
            self.intensity,
            self.settings.style,
            self.settings.mood,
        )
    }

    /// The frame and every part written against it.
    pub fn draft(&self) -> (Frame, Vec<PartDraft>) {
        let frame = self.frame();
        let parts = write_parts(&self.settings, &self.parts, &frame);
        (frame, parts)
    }
}

/// The steps of `bar` that `draft` starts a note on, without repeats.
pub(crate) fn bar_steps(frame: &Frame, draft: &PartDraft, bar: usize) -> Vec<usize> {
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

/// Everything `draft` plays, as comparable tuples.
///
/// The velocity travels as bits because two performances of the same music are equal or they are
/// not.
pub(crate) fn notes_of(draft: &PartDraft) -> Vec<(i64, u8, i64, u32)> {
    let mut notes: Vec<(i64, u8, i64, u32)> = draft
        .notes
        .iter()
        .map(|note| {
            (
                note.start.raw(),
                note.pitch,
                note.length.raw(),
                note.velocity.to_bits(),
            )
        })
        .collect();
    notes.sort_unstable();
    notes
}

/// The part called `name`.
pub(crate) fn part<'a>(parts: &'a [PartDraft], name: &str) -> &'a PartDraft {
    parts
        .iter()
        .find(|part| part.name == name)
        .unwrap_or_else(|| panic!("no part called {name}"))
}
