//! What the tests of every file here are written against.
//!
//! `#[cfg(test)]` and nothing else. Two arrangements answer most of the questions this module
//! asks — one instrument track with a riff on it, and two tracks routed through a bus — so they
//! live where each file's own tests can reach them rather than being copied into four.

use crate::time::Ticks;

use super::clip::Note;
use super::routing::Output;
use super::{Project, TrackId};

pub(super) fn demo_project() -> Project {
    let mut project = Project::new("Demo", 48_000.0);
    let track = project.add_instrument_track("Lead", "auris.synth.pulse");
    let clip = project
        .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    let midi = project.midi_clip_mut(clip).unwrap();
    midi.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    midi.notes
        .push(Note::new(64, Ticks::QUARTER, Ticks::QUARTER));
    project
}

/// Two instrument tracks routed into one bus, which goes to the master.
pub(super) fn bussed_project() -> (Project, TrackId, TrackId, TrackId) {
    let mut project = Project::new("Routing", 48_000.0);
    let kick = project.add_instrument_track("Kick", "auris.synth.pulse");
    let snare = project.add_instrument_track("Snare", "auris.synth.pulse");
    let bus = project.add_bus_track("Drums");
    project.track_mut(kick).unwrap().output = Output::Bus(bus);
    project.track_mut(snare).unwrap().output = Output::Bus(bus);
    (project, kick, snare, bus)
}
