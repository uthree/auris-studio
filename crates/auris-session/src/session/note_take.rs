//! Recording what is played on the typing keyboard into a clip.
//!
//! [`typing`](super::typing) is an instrument somebody listens to; this is the same instrument
//! with somebody writing it down. Record on a track that holds notes, rather than on an audio
//! track, starts a *note take*: no device to open and no file to write, only a list of what was
//! struck, when, and for how long. Stopping turns the list into one clip, as one undo step, the
//! way an audio take becomes one.
//!
//! # When a key was pressed
//!
//! Not when the session heard about it, and not where the playhead is: the playhead is the frame
//! being *rendered*, which reaches the speaker a buffer and the graph's compensation later. What
//! the player was playing along to is the frame they were *hearing*, and
//! [`EngineHandle::heard_frame_now`](auris_engine::EngineHandle::heard_frame_now) reads it off
//! the output callback's own account of when its blocks play. What it cannot know is how long
//! the key took to reach the session — the keyboard, the operating system, the window's event
//! loop — which is what the typing calibration measures and takes off.
//!
//! A virtual frame, too, so a note struck during the count-in lands in front of the downbeat it
//! was counted into rather than on it: somebody who plays a pickup means the pickup.
//!
//! # What becomes of a note still held
//!
//! It ends where the take does. Letting go after pressing Stop is a release nobody wrote down,
//! and a note that ran for ever would be worse than one cut at the end of the take.

use auris_core::{Note, Seconds, Ticks, TrackId};
use auris_engine::EngineCommand;

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;
use super::record::{RecordingReport, RecordingStatus, TakeReport};

/// A note take under way.
#[derive(Clone, Debug)]
pub(super) struct NoteTake {
    /// The track being written onto.
    track: TrackId,
    /// The virtual frame the take was started at, in engine frames: where the clip begins
    /// unless something was played in front of it.
    start: f64,
    /// Notes struck and not yet released: pitch, velocity, where struck.
    held: Vec<(u8, f32, f64)>,
    /// Notes struck and released: pitch, velocity, where struck, where released.
    played: Vec<(u8, f32, f64, f64)>,
}

impl Session {
    /// `true` while what is played on the typing keyboard is being written down.
    pub fn recording_notes(&self) -> bool {
        self.note_take.is_some()
    }

    /// How the running note take is doing, in the shape an audio take reports it.
    ///
    /// No device, so nothing can be dropped or lost; the clock runs from where the take began.
    pub(super) fn note_take_status(&self) -> Option<RecordingStatus> {
        let take = self.note_take.as_ref()?;
        let rate = self.engine.sample_rate().max(1.0);
        Some(RecordingStatus {
            device: String::new(),
            seconds: ((self.performed_frame() - take.start) / rate).max(0.0),
            start: Some(self.tick_of_frame(take.start.max(0.0) as u64)),
            dropped_frames: 0,
            running: true,
        })
    }

    /// Starts writing down what the typing keyboard plays on `track`.
    ///
    /// Called by [`Self::start_recording`] when the track it is aimed at holds notes. Counts in
    /// the way an audio take does.
    pub(super) fn start_note_take(&mut self, track: TrackId) -> Result<(), SessionError> {
        let index = self.require_track(track)?;
        if !self.project.tracks[index].kind.holds_notes() {
            return Err(SessionError::NothingToRecordOnto);
        }
        let count = self.count_in();
        self.engine
            .expect_count_in(count.map_or(0, |count| count.remaining_frames));
        let start = self.engine.playhead_frames() as f64;
        self.counting = count;
        if let Some(count) = count {
            self.send(EngineCommand::CountIn(count));
            self.play();
        }
        self.note_take = Some(NoteTake {
            track,
            start,
            held: Vec::new(),
            played: Vec::new(),
        });
        Ok(())
    }

    /// Where in the music the person playing is at this instant, as a virtual engine frame.
    ///
    /// The heard frame when the output can say, which is whenever it is running; otherwise the
    /// rendered one, which is the best a silent session has. Less whatever the typing
    /// calibration found a key takes to arrive.
    pub(super) fn performed_frame(&self) -> f64 {
        let rate = self.engine.sample_rate().max(1.0);
        self.heard_frame() - self.typing_lag_ms() / 1000.0 * rate
    }

    /// The virtual frame being heard at this instant, with nothing taken off for the keyboard.
    pub(super) fn heard_frame(&self) -> f64 {
        self.engine.heard_frame_now().unwrap_or_else(|| {
            self.engine.playhead_frames() as f64 - self.engine.count_in_frames() as f64
        })
    }

    /// Writes down a note the typing keyboard struck, if a note take is listening to its track.
    pub(super) fn note_take_struck(&mut self, track: TrackId, pitch: u8, velocity: f32) {
        if !self
            .note_take
            .as_ref()
            .is_some_and(|take| take.track == track)
        {
            return;
        }
        let at = self.performed_frame();
        let take = self.note_take.as_mut().expect("checked above");
        // A key struck again before it was let go — through sustain — ends the first note
        // where the second begins, which is what an instrument does with it too.
        close_held(take, pitch, at);
        take.held.push((pitch, velocity, at));
    }

    /// Writes down the end of a note the typing keyboard let go of.
    pub(super) fn note_take_released(&mut self, track: TrackId, pitch: u8) {
        if !self
            .note_take
            .as_ref()
            .is_some_and(|take| take.track == track)
        {
            return;
        }
        let at = self.performed_frame();
        let take = self.note_take.as_mut().expect("checked above");
        close_held(take, pitch, at);
    }

    /// Ends the note take and turns what was played into a clip.
    pub(super) fn stop_note_take(&mut self) -> Result<RecordingReport, SessionError> {
        let mut take = self.note_take.take().ok_or(SessionError::NotRecording)?;
        let end = self.performed_frame();
        for (pitch, velocity, at) in std::mem::take(&mut take.held) {
            take.played.push((pitch, velocity, at, end.max(at)));
        }
        let rate = self.engine.sample_rate().max(1.0);
        let seconds = ((end - take.start) / rate).max(0.0);
        let Some((clip_start, clip_end, notes)) = written_notes(&take, end, |frame| {
            self.project
                .tempo_map
                .seconds_to_ticks(Seconds(frame / rate))
        }) else {
            return Ok(RecordingReport {
                takes: vec![TakeReport {
                    track: take.track,
                    clip: None,
                    path: None,
                    seconds,
                    outside_punch: false,
                }],
                dropped_frames: 0,
            });
        };
        if self.project.track(take.track).is_none() {
            return Ok(RecordingReport {
                takes: vec![TakeReport {
                    track: take.track,
                    clip: None,
                    path: None,
                    seconds,
                    outside_punch: false,
                }],
                dropped_frames: 0,
            });
        }
        let name = self
            .project
            .track(take.track)
            .map(|track| track.name.clone())
            .unwrap_or_default();
        self.begin_transaction(Edit::RecordTake);
        let clip = match self.add_midi_clip(
            take.track,
            name,
            clip_start,
            Ticks(clip_end.raw() - clip_start.raw()),
        ) {
            Ok(clip) => clip,
            Err(error) => {
                self.revert_transaction();
                return Err(error);
            }
        };
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.notes = notes;
        }
        self.invalidate_graph();
        self.end_transaction();
        Ok(RecordingReport {
            takes: vec![TakeReport {
                track: take.track,
                clip: Some(clip),
                path: None,
                seconds,
                outside_punch: false,
            }],
            dropped_frames: 0,
        })
    }
}

/// Ends the held note at `pitch`, if there is one, at `at`.
fn close_held(take: &mut NoteTake, pitch: u8, at: f64) {
    if let Some(position) = take.held.iter().position(|held| held.0 == pitch) {
        let (pitch, velocity, struck) = take.held.remove(position);
        take.played.push((pitch, velocity, struck, at.max(struck)));
    }
}

/// The clip a finished note take writes: where it starts and ends, and its notes.
///
/// `None` when nothing was played. The clip starts where the take did, or earlier for a pickup
/// played during the count, and never before the song; it ends where the take ended or the last
/// note did, whichever is later. A note shorter than a tick is a tick, so a key tapped as fast as
/// a keyboard can report still writes something that can be seen and heard.
pub(crate) fn written_notes(
    take: &NoteTake,
    end: f64,
    to_ticks: impl Fn(f64) -> Ticks,
) -> Option<(Ticks, Ticks, Vec<Note>)> {
    if take.played.is_empty() {
        return None;
    }
    let earliest = take
        .played
        .iter()
        .map(|note| note.2)
        .fold(take.start, f64::min)
        .max(0.0);
    let latest = take
        .played
        .iter()
        .map(|note| note.3)
        .fold(end.max(take.start), f64::max);
    let clip_start = to_ticks(earliest);
    let mut notes: Vec<Note> = take
        .played
        .iter()
        // A note that ended before the song began was played to nothing that is written down.
        .filter(|note| note.3 > 0.0 || note.2 >= 0.0)
        .map(|&(pitch, velocity, struck, released)| {
            let on = to_ticks(struck.max(0.0));
            let off = to_ticks(released.max(0.0));
            let mut note = Note::new(
                pitch,
                Ticks(on.raw() - clip_start.raw()),
                Ticks((off.raw() - on.raw()).max(1)),
            );
            note.velocity = velocity.clamp(0.0, 1.0);
            note
        })
        .collect();
    if notes.is_empty() {
        return None;
    }
    notes.sort_by_key(|note| (note.start, note.pitch));
    let clip_end = to_ticks(latest).max(Ticks(clip_start.raw() + 1));
    Some((clip_start, clip_end, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    /// Frames to ticks at one tick a frame, so the arithmetic is the test's and not a tempo map's.
    fn one_to_one(frame: f64) -> Ticks {
        Ticks(frame.round() as i64)
    }

    fn take(start: f64, played: &[(u8, f64, f64)]) -> NoteTake {
        NoteTake {
            track: TrackId(1),
            start,
            held: Vec::new(),
            played: played
                .iter()
                .map(|&(pitch, on, off)| (pitch, 0.7, on, off))
                .collect(),
        }
    }

    #[test]
    fn notes_land_where_they_were_played_relative_to_the_take() {
        let take = take(1_000.0, &[(60, 1_200.0, 1_500.0), (64, 1_500.0, 1_900.0)]);
        let (start, end, notes) = written_notes(&take, 2_000.0, one_to_one).unwrap();
        assert_eq!(start, Ticks(1_000));
        assert_eq!(end, Ticks(2_000));
        assert_eq!(notes.len(), 2);
        assert_eq!(
            (notes[0].pitch, notes[0].start, notes[0].length),
            (60, Ticks(200), Ticks(300))
        );
        assert_eq!(
            (notes[1].pitch, notes[1].start, notes[1].length),
            (64, Ticks(500), Ticks(400))
        );
        assert!((notes[0].velocity - 0.7).abs() < 1e-6);
    }

    #[test]
    fn a_pickup_played_during_the_count_starts_the_clip_early() {
        let take = take(1_000.0, &[(67, 800.0, 1_000.0), (60, 1_000.0, 1_400.0)]);
        let (start, _, notes) = written_notes(&take, 1_500.0, one_to_one).unwrap();
        assert_eq!(start, Ticks(800));
        assert_eq!(notes[0].start, Ticks(0));
        assert_eq!(notes[1].start, Ticks(200));
    }

    #[test]
    fn nothing_before_the_song_is_written() {
        let take = take(100.0, &[(60, -300.0, -50.0), (62, -20.0, 400.0)]);
        let (start, _, notes) = written_notes(&take, 500.0, one_to_one).unwrap();
        assert_eq!(start, Ticks(0));
        assert_eq!(
            notes.len(),
            1,
            "the note that ended before the song is gone"
        );
        assert_eq!((notes[0].start, notes[0].length), (Ticks(0), Ticks(400)));
    }

    #[test]
    fn a_take_with_nothing_played_writes_no_clip() {
        assert!(written_notes(&take(0.0, &[]), 10_000.0, one_to_one).is_none());
    }

    #[test]
    fn a_tap_shorter_than_a_tick_is_still_a_note() {
        let take = take(0.0, &[(60, 500.2, 500.3)]);
        let (_, _, notes) = written_notes(&take, 600.0, one_to_one).unwrap();
        assert_eq!(notes[0].length, Ticks(1));
    }

    #[test]
    fn what_is_typed_on_an_instrument_track_becomes_one_undoable_clip() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();
        session.set_musical_typing(true);
        session.start_recording(Some(track)).unwrap();
        assert!(session.is_recording());
        assert!(session.typing_press(track, "a"));
        assert!(session.typing_release("a"));
        assert!(session.typing_press(track, "d"));
        let report = session.stop_recording().unwrap();
        assert!(!session.is_recording());
        let clip = report.takes[0].clip.expect("a clip");
        let (_, written) = session.project().midi_clip(clip).unwrap();
        let pitches: Vec<u8> = written.notes.iter().map(|note| note.pitch).collect();
        assert_eq!(pitches.len(), 2);
        assert_eq!(pitches[1] - pitches[0], 4, "C then E");
        assert!(session.undo().is_some());
        assert!(session.project().midi_clip(clip).is_none());
        session.release_typed_notes();
    }

    #[test]
    fn notes_played_on_another_track_are_not_written_down() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let other = session.add_default_instrument_track("Pad").unwrap();
        session.set_musical_typing(true);
        session.start_recording(Some(track)).unwrap();
        session.typing_press(other, "a");
        session.typing_release("a");
        let report = session.stop_recording().unwrap();
        assert_eq!(report.takes[0].clip, None);
    }
}
