//! Sample-timed selection audition without moving the document's transport.

use super::Session;
use auris_core::time::Ticks;
use auris_core::{ClipId, Note, NoteEvent, TrackId};
use auris_engine::{EngineCommand, ScheduledEvent};
use std::sync::Arc;

/// A snapshot of selected notes, including their timing at the current song location.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSelectionPreview {
    /// Track whose instrument sounds the phrase.
    pub track: TrackId,
    /// Selected notes in clip order, with offsets relative to the first selected onset.
    pub notes: Vec<Note>,
    events: Arc<[ScheduledEvent]>,
}

impl Session {
    /// Prepares only the selected notes, preserving rests, overlaps, lengths and velocities.
    /// Timing uses the song's tempo map at the clip's moved position.
    pub fn note_selection_preview(
        &self,
        clip: ClipId,
        indices: &[usize],
    ) -> Option<NoteSelectionPreview> {
        let (track, clip) = self.project.midi_clip(clip)?;
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        let mut notes: Vec<_> = indices
            .into_iter()
            .filter_map(|index| clip.notes.get(index).cloned())
            .collect();
        let first = notes.iter().map(|note| note.start).min()?;
        let rate = self.sample_rate();
        let tempo = &self.project.tempo_map;
        let origin = tempo.ticks_to_samples(clip.start + first, rate).raw();
        let mut events = Vec::with_capacity(notes.len() * 2);
        for note in &mut notes {
            let start = tempo
                .ticks_to_samples(clip.start + note.start, rate)
                .raw()
                .saturating_sub(origin);
            let end = tempo
                .ticks_to_samples(clip.start + note.start + note.length.max(Ticks(1)), rate)
                .raw()
                .saturating_sub(origin)
                .max(start + 1);
            events.push(ScheduledEvent {
                frame: start,
                event: NoteEvent::NoteOn {
                    frame: 0,
                    pitch: note.pitch,
                    velocity: note.velocity,
                },
            });
            events.push(ScheduledEvent {
                frame: end,
                event: NoteEvent::NoteOff {
                    frame: 0,
                    pitch: note.pitch,
                },
            });
            note.start -= first;
        }
        events.sort_by_key(|event| (event.frame, matches!(event.event, NoteEvent::NoteOn { .. })));
        Some(NoteSelectionPreview {
            track,
            notes,
            events: events.into(),
        })
    }

    /// Starts a prepared phrase once on its track's live preview instrument.
    pub fn play_note_selection_preview(&mut self, preview: &NoteSelectionPreview) {
        if let Some(track) = self.project.track_index(preview.track) {
            self.send(EngineCommand::PlayNotePreview {
                track,
                events: Arc::clone(&preview.events),
            });
        }
    }

    /// Cancels a selection preview and releases any notes still held by it.
    pub fn stop_note_selection_preview(&mut self) {
        self.send(EngineCommand::StopNotePreview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    #[test]
    fn selected_phrase_keeps_gaps_velocities_and_tempo_changes() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::QUARTER * 4, Ticks::QUARTER * 8)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();
        let mut second = Note::new(64, Ticks::QUARTER * 3, Ticks::QUARTER * 2);
        second.velocity = 0.37;
        session.add_note(clip, second).unwrap();
        session
            .add_note(clip, Note::new(70, Ticks::QUARTER * 2, Ticks::QUARTER))
            .unwrap();
        session.set_tempo_point(Ticks::ZERO, 120.);
        session.set_tempo_point(Ticks::QUARTER * 6, 60.);
        let preview = session.note_selection_preview(clip, &[1, 0, 1]).unwrap();
        let rate = session.sample_rate() as u64;
        assert_eq!(
            preview
                .notes
                .iter()
                .map(|note| note.pitch)
                .collect::<Vec<_>>(),
            vec![60, 64]
        );
        assert_eq!(
            preview
                .events
                .iter()
                .map(|event| event.frame)
                .collect::<Vec<_>>(),
            vec![0, rate / 2, rate * 3 / 2, rate * 7 / 2]
        );
        assert!(matches!(
            preview.events[2].event,
            NoteEvent::NoteOn {
                pitch: 64,
                velocity: 0.37,
                ..
            }
        ));
        assert_eq!(session.playhead(), Ticks::ZERO);
        assert!(session.note_selection_preview(clip, &[]).is_none());
    }
}
