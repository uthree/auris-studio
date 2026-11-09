//! A clip read as a list of events, one row each, and the commands that edit one row.
//!
//! The piano roll and the curve strips show a clip as a picture; this is the same clip as the
//! table a step sequencer or a MIDI list editor shows — where each thing happens, what it is, how
//! long it is held and how hard it is. Nothing new is stored: a row is a [`Note`] or a
//! [`CurvePoint`] the clip already carries, addressed by an [`EventRef`], and an edit to a row is
//! the existing note or curve command for that one field, wrapped as a single undo step.
//!
//! A curve is listed as the points that were *written*, not as the stream it is sampled into on
//! playback. The samples are a rendering detail that changes whenever [`CURVE_STEP`] does; the
//! points are what somebody drew and what they would expect to correct.
//!
//! [`CURVE_STEP`]: auris_core::project::CURVE_STEP

use auris_core::project::{ClipCurve, CurvePoint};
use auris_core::time::Ticks;
use auris_core::{ClipId, MidiClip, Note};

use super::Session;
use crate::error::SessionError;
use crate::history::Edit;

/// Which of a clip's events a row of the list is.
///
/// A note is named by its index, the way every other note command names one; a curve point by
/// the curve and the instant it sits at, since a curve holds at most one point per instant and
/// its points carry no index that survives an edit.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventRef {
    /// The note at this index in [`MidiClip::notes`].
    Note(usize),
    /// The point on `curve` written at `at`, clip-relative.
    Curve {
        /// Which curve the point is on.
        curve: ClipCurve,
        /// Where it sits, from the clip's own start.
        at: Ticks,
    },
}

/// What one row of the list says, in the units the list shows.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ClipEventKind {
    /// A note: which key, how hard, for how long.
    Note {
        /// MIDI note number.
        pitch: u8,
        /// MIDI velocity, 0 to 127.
        velocity: u8,
        /// How long it is held — the gate.
        gate: Ticks,
    },
    /// A point on a controller curve.
    Controller {
        /// The MIDI controller number.
        number: u8,
        /// Its value on the wire's own scale, 0 to 127.
        value: u8,
    },
    /// A point on the pitch bend.
    Bend {
        /// Semitones either side of the note.
        semitones: f32,
    },
}

/// One row of the event list.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClipEvent {
    /// Where it happens, from the clip's own start.
    pub at: Ticks,
    /// How to address it in [`Session::edit_clip_event`].
    pub event: EventRef,
    /// What it is.
    pub kind: ClipEventKind,
}

/// One field of one row, changed to a new value.
///
/// Each variant is a column of the list. A row that has no such column — a gate on a controller,
/// a pitch on the bend — is refused rather than ignored, so a frontend that offered the cell by
/// mistake hears about it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EventEdit {
    /// Move the event to this clip-relative position.
    Position(Ticks),
    /// Put a note on this key.
    Pitch(u8),
    /// Hold a note this long.
    Gate(Ticks),
    /// The row's value in the units [`ClipEventKind`] shows it in: a note's velocity or a
    /// controller's value from 0 to 127, or the bend in semitones.
    Value(f32),
}

/// The wire's seven bits, read back into the 0-to-1 a controller curve stores.
fn controller_unit(value: f32) -> f32 {
    (value / 127.0).clamp(0.0, 1.0)
}

/// A curve value as the 0-to-127 a controller row shows.
fn controller_midi(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// The event list of `clip`, in time order.
///
/// At one instant the curves come before the notes — the bend first, then controllers by number,
/// then notes from low to high — because that is the order they have to reach an instrument in:
/// a controller written on the downbeat is meant to be in force when the downbeat's note sounds.
pub fn clip_events(clip: &MidiClip) -> Vec<ClipEvent> {
    let curves = std::iter::once((ClipCurve::Bend, clip.bend.as_slice())).chain(
        clip.controllers
            .iter()
            .map(|(number, points)| (ClipCurve::Controller(*number), points.as_slice())),
    );
    let mut events: Vec<(u8, ClipEvent)> = Vec::new();
    for (curve, points) in curves {
        events.extend(points.iter().map(|point: &CurvePoint| {
            let kind = match curve {
                ClipCurve::Bend => ClipEventKind::Bend {
                    semitones: point.value,
                },
                ClipCurve::Controller(number) => ClipEventKind::Controller {
                    number,
                    value: controller_midi(point.value),
                },
            };
            (
                0,
                ClipEvent {
                    at: point.at,
                    event: EventRef::Curve {
                        curve,
                        at: point.at,
                    },
                    kind,
                },
            )
        }));
    }
    events.extend(
        clip.notes
            .iter()
            .enumerate()
            .map(|(index, note): (usize, &Note)| {
                (
                    1,
                    ClipEvent {
                        at: note.start,
                        event: EventRef::Note(index),
                        kind: ClipEventKind::Note {
                            pitch: note.pitch,
                            velocity: (note.velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                            gate: note.length,
                        },
                    },
                )
            }),
    );
    // A stable sort: the curves went in bend first and controllers in number order, so within
    // one instant they keep that order without a key of their own.
    events.sort_by_key(|(group, event)| {
        let within = match (event.event, event.kind) {
            (EventRef::Note(index), ClipEventKind::Note { pitch, .. }) => (pitch, index),
            _ => (0, 0),
        };
        (event.at, *group, within)
    });
    events.into_iter().map(|(_, event)| event).collect()
}

impl Session {
    /// The event list of a MIDI clip, in time order. See [`clip_events`].
    pub fn clip_events(&self, clip: ClipId) -> Result<Vec<ClipEvent>, SessionError> {
        self.midi_clip(clip)
            .map(clip_events)
            .ok_or(SessionError::UnknownClip(clip.0))
    }

    /// Changes one field of one row of a clip's event list, as one undo step.
    ///
    /// Returns the address the event has afterwards. A note keeps its index whatever is changed;
    /// a curve point moved along the clip is found at its new instant, which is not always the
    /// one asked for — a curve point cannot leave the clip, and one dropped on another point
    /// replaces it.
    pub fn edit_clip_event(
        &mut self,
        clip: ClipId,
        event: EventRef,
        edit: EventEdit,
    ) -> Result<EventRef, SessionError> {
        let target = self
            .midi_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        let refused = || SessionError::InvalidNotes(format!("{edit:?} on {event:?}"));
        if let EventEdit::Value(value) = edit
            && !value.is_finite()
        {
            return Err(SessionError::NotFinite(f64::from(value)));
        }
        // What the edit starts from, read before the transaction borrows the session mutably.
        let held = match event {
            EventRef::Note(index) => {
                let note = target.notes.get(index).ok_or(SessionError::UnknownNote {
                    clip: clip.0,
                    index,
                })?;
                (note.start, note.pitch, 0.0)
            }
            EventRef::Curve { curve, at } => {
                if matches!(edit, EventEdit::Pitch(_) | EventEdit::Gate(_)) {
                    return Err(refused());
                }
                let point = target
                    .curve(curve)
                    .iter()
                    .find(|point| point.at == at)
                    .ok_or_else(refused)?;
                (point.at, 0, point.value)
            }
        };
        // Each command below validates its whole candidate before it writes anything, so a
        // refusal leaves the document as it was and the transaction is simply dropped.
        self.begin_transaction(Edit::EditEvent);
        let result = match event {
            EventRef::Note(index) => {
                let (start, pitch, _) = held;
                let origin = [(index, start, pitch)];
                match edit {
                    EventEdit::Position(at) => {
                        self.move_notes(clip, &origin, at.max_zero() - start, 0)
                    }
                    EventEdit::Pitch(to) => self.move_notes(
                        clip,
                        &origin,
                        Ticks::ZERO,
                        i32::from(to.min(127)) - i32::from(pitch),
                    ),
                    EventEdit::Gate(gate) => {
                        self.resize_note(clip, index, start + Ticks(gate.raw().max(1)))
                    }
                    EventEdit::Value(velocity) => self.set_note_velocity(
                        clip,
                        &[index],
                        velocity.round().clamp(1.0, 127.0) / 127.0,
                    ),
                }
                .map(|()| event)
            }
            EventRef::Curve { curve, at } => {
                let (_, _, value) = held;
                let wanted = match edit {
                    EventEdit::Position(to) => Some((to, value)),
                    EventEdit::Value(written) => Some(match curve {
                        ClipCurve::Bend => (at, written),
                        ClipCurve::Controller(_) => (at, controller_unit(written.round())),
                    }),
                    // Refused before the transaction began; kept total rather than panicking.
                    EventEdit::Pitch(_) | EventEdit::Gate(_) => None,
                };
                wanted
                    .and_then(|(to, value)| self.move_curve_point(clip, curve, at, to, value))
                    .map(|at| EventRef::Curve { curve, at })
                    .ok_or_else(refused)
            }
        };
        match result {
            Ok(event) => {
                self.end_transaction();
                Ok(event)
            }
            Err(error) => {
                self.cancel_transaction();
                Err(error)
            }
        }
    }

    /// Takes one row of a clip's event list away: the note, or the curve point.
    pub fn remove_clip_event(&mut self, clip: ClipId, event: EventRef) -> Result<(), SessionError> {
        match event {
            EventRef::Note(index) => {
                if self
                    .midi_clip(clip)
                    .ok_or(SessionError::UnknownClip(clip.0))?
                    .notes
                    .get(index)
                    .is_none()
                {
                    return Err(SessionError::UnknownNote {
                        clip: clip.0,
                        index,
                    });
                }
                self.remove_notes(clip, &[index])
            }
            EventRef::Curve { curve, at } => {
                if self.midi_clip(clip).is_none() {
                    return Err(SessionError::UnknownClip(clip.0));
                }
                match self.remove_curve_point(clip, curve, at) {
                    true => Ok(()),
                    false => Err(SessionError::InvalidNotes(format!(
                        "no {curve:?} point at {at:?}"
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{session_with_clip, undo_depth};

    #[test]
    fn the_list_runs_in_time_order_with_curves_ahead_of_notes_at_one_instant() {
        let (mut session, _, clip) = session_with_clip();
        session.set_curve_point(clip, ClipCurve::Controller(11), Ticks::ZERO, 1.0);
        session.set_curve_point(clip, ClipCurve::Bend, Ticks::ZERO, 2.0);
        session.set_curve_point(clip, ClipCurve::Controller(1), Ticks(480), 0.5);

        let events = session.clip_events(clip).unwrap();
        let kinds: Vec<(i64, ClipEventKind)> = events
            .iter()
            .map(|event| (event.at.raw(), event.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (0, ClipEventKind::Bend { semitones: 2.0 }),
                (
                    0,
                    ClipEventKind::Controller {
                        number: 11,
                        value: 127
                    }
                ),
                (
                    0,
                    ClipEventKind::Note {
                        pitch: 60,
                        velocity: 102,
                        gate: Ticks::QUARTER
                    }
                ),
                (
                    480,
                    ClipEventKind::Controller {
                        number: 1,
                        value: 64
                    }
                ),
                (
                    960,
                    ClipEventKind::Note {
                        pitch: 64,
                        velocity: 102,
                        gate: Ticks::QUARTER
                    }
                ),
            ]
        );
    }

    #[test]
    fn each_column_of_a_note_is_one_undo_step() {
        let (mut session, _, clip) = session_with_clip();
        let before = undo_depth(&mut session);
        let note = EventRef::Note(1);
        for edit in [
            EventEdit::Position(Ticks(1440)),
            EventEdit::Pitch(67),
            EventEdit::Gate(Ticks(240)),
            EventEdit::Value(100.0),
        ] {
            assert_eq!(session.edit_clip_event(clip, note, edit).unwrap(), note);
        }
        let edited = &session.midi_clip(clip).unwrap().notes[1];
        assert_eq!(edited.start, Ticks(1440));
        assert_eq!(edited.pitch, 67);
        assert_eq!(edited.length, Ticks(240));
        assert_eq!((edited.velocity * 127.0).round() as u8, 100);
        assert_eq!(undo_depth(&mut session), before + 4);
    }

    #[test]
    fn a_velocity_never_reaches_silence_and_a_gate_never_reaches_nothing() {
        let (mut session, _, clip) = session_with_clip();
        let note = EventRef::Note(0);
        session
            .edit_clip_event(clip, note, EventEdit::Value(0.0))
            .unwrap();
        session
            .edit_clip_event(clip, note, EventEdit::Gate(Ticks::ZERO))
            .unwrap();
        let edited = &session.midi_clip(clip).unwrap().notes[0];
        assert_eq!((edited.velocity * 127.0).round() as u8, 1);
        assert_eq!(edited.length, Ticks(1));
    }

    #[test]
    fn a_curve_point_moves_and_takes_a_new_value_on_the_wire_scale() {
        let (mut session, _, clip) = session_with_clip();
        let wheel = ClipCurve::MODULATION;
        session.set_curve_point(clip, wheel, Ticks(480), 0.5);
        let point = EventRef::Curve {
            curve: wheel,
            at: Ticks(480),
        };
        let moved = session
            .edit_clip_event(clip, point, EventEdit::Position(Ticks(960)))
            .unwrap();
        assert_eq!(
            moved,
            EventRef::Curve {
                curve: wheel,
                at: Ticks(960)
            }
        );
        session
            .edit_clip_event(clip, moved, EventEdit::Value(127.0))
            .unwrap();
        let curve = session.midi_clip(clip).unwrap().curve(wheel).to_vec();
        assert_eq!(
            curve,
            vec![CurvePoint {
                at: Ticks(960),
                value: 1.0
            }]
        );
    }

    #[test]
    fn a_column_the_row_does_not_have_is_refused_and_leaves_no_step() {
        let (mut session, _, clip) = session_with_clip();
        session.set_curve_point(clip, ClipCurve::Bend, Ticks::ZERO, 1.0);
        let before = undo_depth(&mut session);
        let bend = EventRef::Curve {
            curve: ClipCurve::Bend,
            at: Ticks::ZERO,
        };
        assert!(
            session
                .edit_clip_event(clip, bend, EventEdit::Gate(Ticks(10)))
                .is_err()
        );
        assert!(
            session
                .edit_clip_event(clip, EventRef::Note(9), EventEdit::Pitch(60))
                .is_err()
        );
        assert_eq!(undo_depth(&mut session), before);
    }

    #[test]
    fn a_row_can_be_taken_away() {
        let (mut session, _, clip) = session_with_clip();
        session.set_curve_point(clip, ClipCurve::Bend, Ticks::ZERO, 1.0);
        session
            .remove_clip_event(
                clip,
                EventRef::Curve {
                    curve: ClipCurve::Bend,
                    at: Ticks::ZERO,
                },
            )
            .unwrap();
        session.remove_clip_event(clip, EventRef::Note(0)).unwrap();
        let events = session.clip_events(clip).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, EventRef::Note(0));
    }
}
