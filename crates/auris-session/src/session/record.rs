//! Recording a take onto an audio track.
//!
//! Three things happen at once and none of them can wait for the others. The input device fills a
//! pool from its own callback; a thread of this crate's own empties that pool into a file; and the
//! session goes on being a session, drawing meters and moving the playhead.
//!
//! # A take is a phase of an open device, not the device's life
//!
//! It used to be the device's life: the [`Capture`](auris_engine::Capture) *was* the take, and
//! dropping it closed the stream, which is how the writer found out it was over. That stopped
//! working the moment [`monitor`](super::monitor) existed, because monitoring holds the same
//! device open with no take running and a take that ended by closing it would take the monitor
//! down every time somebody pressed stop.
//!
//! So the device belongs to the [`Session`], one of it for both, and a take is
//! [`begin_take`](auris_engine::Capture::begin_take) to
//! [`end_take`](auris_engine::Capture::end_take) within that. Two things follow and both are load
//! bearing:
//!
//! * **The pool's reader is on loan, not given away.** There is one, a second take needs it, so
//!   the writer thread hands it back along with the frame count.
//! * **Stopping is a flag, not a disconnect,** and a flag can be seen before the last block that
//!   was already on its way. The writer waits for two quiet passes rather than closing on the
//!   first, which is comfortably longer than a callback and costs a fifth of the time a person
//!   takes to let go of a mouse button.
//!
//! # What a take needs before it can start
//!
//! A folder. A recording is a file, and a project that has never been saved has nowhere to put
//! one; every other asset a project refers to can sit outside the folder until a save picks it up,
//! but a take has to be *written* somewhere the moment it begins. So this is the one command that
//! refuses on an unsaved document, and says so rather than inventing a temporary directory whose
//! contents would be lost the first time the machine tidied it.
//!
//! A track to record onto, too, and an audio one. Which track that is follows the selection: an
//! audio track that is selected is where the take lands, so recording is choosing a track and
//! pressing Record rather than arming one first. The arm is still there and still wins, because
//! it says the one thing a selection cannot — record onto the vocal while I read the drum part.
//! What a take may never do is pick a track for itself; a take on a track nobody chose is a take
//! nobody finds.
//!
//! # Where the take lands
//!
//! At the playhead the transport was at when the *first block of audio arrived*, which
//! `auris_engine::capture` explains and which is not the same as when the button was pressed. The
//! transport does not have to be rolling: a take started from a standstill lands at the playhead
//! and runs from there, which is how somebody records an idea before there is a song around it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use auris_core::{AssetPath, Seconds, Ticks, TrackId};
use auris_engine::{CaptureReader, CaptureSettings};
use auris_io::{IoError, WavRecorder, import_audio_file};

use super::Session;
use super::punch::punch_window;
use crate::error::SessionError;
use crate::history::Edit;

/// How often the writing thread looks for more audio when the pool came up empty.
///
/// The pool holds well over a second, so this is nowhere near tight enough to matter for safety;
/// it is short enough that stopping a take feels immediate and long enough that an idle take is
/// a hundred wake-ups a second rather than a spin.
const POLL: Duration = Duration::from_millis(10);

/// A take being recorded.
///
/// The device is not here. It belongs to the session, because monitoring holds one open with no
/// take running and a take must be able to end without closing it — see
/// [`Session::set_monitoring`]. What ends the take is [`Capture::end_take`]; the writer sees the
/// flag go false and closes the file after a quiet pass or two.
pub(super) struct Take {
    /// The thread writing the file. It hands back the pool's reader — there is one, and the next
    /// take needs it — along with the frame count, or why it stopped.
    writer: std::thread::JoinHandle<(CaptureReader, Result<u64, IoError>)>,
    /// Where the file is, absolute.
    path: PathBuf,
    /// Its name inside the project folder, which is what the document will store.
    inside: PathBuf,
    /// The track the clip will land on.
    track: TrackId,
    /// Whether the playhead has been inside the punch region since the take began.
    ///
    /// What makes rolling out automatic work under a cycle. "Past the punch-out" is not a
    /// condition a looping transport ever meets — it wraps before it gets there — so what is
    /// watched for is *leaving* the region, and leaving needs having been in.
    pub(super) entered_punch: bool,
}

/// How a take that is running is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingStatus {
    /// The device being recorded from.
    pub device: String,
    /// How long the take is so far.
    pub seconds: f64,
    /// Where it will start on the timeline, once the first block has arrived.
    pub start: Option<Ticks>,
    /// Frames lost because the disk could not keep up. Anything but zero is a hole.
    pub dropped_frames: u64,
    /// `false` once the device has disappeared out from under the take.
    pub running: bool,
}

/// What a finished take produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingReport {
    /// The clip, or `None` when the take was empty and nothing was kept.
    pub clip: Option<auris_core::ClipId>,
    /// The file, if one was kept.
    pub path: Option<PathBuf>,
    /// How long it turned out to be.
    pub seconds: f64,
    /// Frames the disk could not keep up with.
    ///
    /// Not an error and not silently swallowed either: the take is usable and everything after
    /// the gap has moved earlier by that much, which is a thing to be told rather than to
    /// discover in a mix a week later.
    pub dropped_frames: u64,
    /// The take was recorded and none of it fell inside the punch region.
    ///
    /// A different thing from an empty take and it has to read as one: nothing was wrong with the
    /// microphone, the player rolled past their own punch or stopped before reaching it. The file
    /// is kept — see [`Self::path`] — because a punch set to the wrong bar is the likeliest reason
    /// to be reading this.
    pub outside_punch: bool,
}

impl Session {
    /// Every input device the host can see, and what each can do.
    ///
    /// Queried on demand for the same reason
    /// [`output_devices`](Session::output_devices) is: an interface plugged in while the window
    /// was open should appear in the list without a restart.
    pub fn input_devices(&self) -> Vec<auris_engine::AudioDeviceInfo> {
        auris_engine::input_devices()
    }

    /// The track that has been armed by hand, if any.
    ///
    /// Not the same question as "where would a take land" — that is
    /// [`record_target`](Session::record_target), which falls back to the selection. This one is
    /// for drawing the arm button, which has to show what was *chosen* rather than what would
    /// happen.
    pub fn armed_track(&self) -> Option<TrackId> {
        self.armed
    }

    /// The track a take would be recorded onto, given whatever the caller has selected.
    ///
    /// The selection is passed in rather than held, because a selection belongs to whatever is
    /// showing the document — a headless session has none, and two windows onto one session would
    /// have one each.
    pub fn record_target(&self, selected: Option<TrackId>) -> Option<TrackId> {
        let records = selected
            .and_then(|id| self.project.track(id))
            .is_some_and(|track| track.kind.as_audio().is_some());
        take_track(self.armed, selected, records)
    }

    /// Arms an audio track for recording, or disarms whatever was armed with `None`.
    ///
    /// Only needed to record onto something other than the selection — see
    /// [`record_target`](Session::record_target). Arming used to be the only way to name a track
    /// at all, which made every take begin with a button press that said what the selection
    /// already said.
    ///
    /// One at a time. Multitrack recording is a real thing and this is not it: one input device
    /// produces one stream, and arming three tracks would either record the same audio onto all
    /// of them or need a channel-to-track map nobody has been asked for yet.
    ///
    /// Deliberately not an undo step. Arming is how somebody prepares to play, not something they
    /// wrote, and a take is usually preceded by several attempts at arming the right track.
    pub fn arm_track(&mut self, track: Option<TrackId>) -> Result<(), SessionError> {
        match track {
            None => {
                self.armed = None;
                Ok(())
            }
            Some(id) => {
                let found = self
                    .project
                    .track(id)
                    .ok_or(SessionError::UnknownTrack(id.0))?;
                if found.kind.as_audio().is_none() {
                    return Err(SessionError::WrongTrackKind {
                        id: id.0,
                        actual: found.kind.label(),
                        expected: "Audio",
                    });
                }
                self.armed = Some(id);
                Ok(())
            }
        }
    }

    /// `true` while a take is running.
    pub fn is_recording(&self) -> bool {
        self.take.is_some()
    }

    /// How the running take is doing, for a clock.
    pub fn recording_status(&self) -> Option<RecordingStatus> {
        self.take.as_ref()?;
        let capture = self.input.as_ref()?;
        let rate = capture.sample_rate().max(1.0);
        Some(RecordingStatus {
            device: capture.name().to_string(),
            seconds: capture.frames() as f64 / rate,
            start: capture.started_at().map(|frame| self.tick_of_frame(frame)),
            dropped_frames: capture.dropped_frames(),
            running: capture.is_running(),
        })
    }

    /// The loudest input sample since this was last called, for a meter.
    ///
    /// Whenever the device is open, which is not the same as whenever a take is running: setting
    /// a level is what somebody does *before* pressing Record, and a meter that only appeared
    /// after the take began would be a meter that arrived too late to be used.
    ///
    /// Reset on read, like a peak-hold on a console, so there is exactly one meter reading it.
    /// Two would each see half the peaks.
    pub fn input_peak(&self) -> f32 {
        self.input
            .as_ref()
            .map_or(0.0, |capture| capture.take_peak())
    }

    /// Opens the input device if it is not already open.
    ///
    /// Idempotent, because both things that want a device — a take and a monitor — may want it at
    /// once and neither knows about the other.
    pub(super) fn open_input(&mut self) -> Result<(), SessionError> {
        if self.input.is_some() {
            return Ok(());
        }
        let settings = CaptureSettings {
            device: self.audio.input_device.clone(),
            // The rate the project renders at, so a take needs no resampling on the way in. A
            // device that cannot do it is recorded at whatever it can and resampled below.
            sample_rate: Some(self.project.sample_rate.round().max(1.0) as u32),
            block_frames: Some(self.audio.block_frames),
        };
        self.input = Some(auris_engine::start_capture(&settings, &self.engine)?);
        Ok(())
    }

    /// Closes and reopens the input device, so it follows a change in the audio settings.
    ///
    /// Does nothing while a take is running: the writer thread is holding the pool's reader, and
    /// pulling the device out from under it would end the take at a moment nobody chose. Nothing
    /// changes audio settings mid-take, and nothing here assumes that.
    ///
    /// A device that will not reopen turns the monitor off rather than leaving it pointing at
    /// nothing. Losing the microphone is worth saying out loud, which the caller does by noticing
    /// [`monitoring`](Session::monitoring) has gone false.
    pub(super) fn restart_input(&mut self) {
        if self.input.is_none() || self.take.is_some() {
            return;
        }
        self.input = None;
        if self.monitored.is_some() {
            match self.open_input() {
                Ok(()) => {
                    if let Some(ring) = self.monitor_ring() {
                        ring.set_enabled(true);
                    }
                }
                Err(error) => {
                    log::warn!("could not reopen the input device: {error}");
                    self.monitored = None;
                }
            }
        }
    }

    /// Closes the input device once neither a take nor a monitor wants it.
    ///
    /// A stream that is open is a microphone that is live, and on every operating system that
    /// shows an indicator it is a light saying the application is listening. Leaving one open
    /// against the next take would be convenient and would also be that.
    pub(super) fn close_input_if_idle(&mut self) {
        if self.take.is_none() && self.monitored.is_none() {
            self.input = None;
        }
    }

    /// Starts recording onto [`record_target(selected)`](Session::record_target).
    ///
    /// Opens the input device and begins writing immediately; the transport is left alone, so a
    /// caller that wants to record *against* the rest of the song plays it as well.
    ///
    /// The arm is left exactly as it was found. A take that armed the track it landed on would
    /// pin the next one there too, so selecting another track and pressing Record again would
    /// quietly record onto the first — which is the sequence this whole fallback exists to fix.
    pub fn start_recording(&mut self, selected: Option<TrackId>) -> Result<(), SessionError> {
        if self.take.is_some() {
            return Err(SessionError::AlreadyRecording);
        }
        let track = self
            .record_target(selected)
            .ok_or(SessionError::NothingToRecordOnto)?;
        // Re-checked rather than trusted: the track may have been deleted, or turned into
        // something else, since it was armed.
        let name = match self.project.track(track) {
            Some(found) if found.kind.as_audio().is_some() => found.name.clone(),
            Some(found) => {
                return Err(SessionError::WrongTrackKind {
                    id: track.0,
                    actual: found.kind.label(),
                    expected: "Audio",
                });
            }
            None => return Err(SessionError::UnknownTrack(track.0)),
        };
        let folder = self
            .project_folder()
            .ok_or(SessionError::RecordingNeedsFolder)?
            .to_path_buf();

        self.open_input()?;
        // The reader is out on loan for the length of a take and handed back at the end of it.
        // Missing means the last writer thread died holding it, which is not a state anything can
        // recover from except by opening the device again.
        if self.input.as_ref().is_some_and(|c| !c.has_reader()) {
            self.input = None;
            self.open_input()?;
        }
        let capture = self.input.as_mut().expect("the input was just opened");
        let reader = capture
            .take_reader()
            .expect("a fresh capture has its reader");

        let inside = PathBuf::from(auris_io::AUDIO_DIR).join(take_file_name(&folder, &name));
        let path = folder.join(&inside);
        let recorder =
            match WavRecorder::create(&path, reader.sample_rate(), reader.channel_count()) {
                Ok(recorder) => recorder,
                Err(error) => {
                    // The reader has to go back even when nothing is going to use it, or the device
                    // is stuck with no consumer and every later take reopens it.
                    capture.restore_reader(reader);
                    self.close_input_if_idle();
                    return Err(error.into());
                }
            };
        let writer = match std::thread::Builder::new()
            .name("auris-record".to_string())
            .spawn(move || write_take(reader, recorder))
        {
            Ok(writer) => writer,
            Err(error) => {
                self.close_input_if_idle();
                return Err(SessionError::Io(IoError::Filesystem {
                    path,
                    source: error,
                }));
            }
        };

        // Last, so the callback starts feeding the pool with a writer already draining it.
        self.input
            .as_ref()
            .expect("the input was just opened")
            .begin_take();
        self.take = Some(Take {
            writer,
            path,
            inside,
            track,
            entered_punch: false,
        });
        Ok(())
    }

    /// Ends the take and turns it into a clip on the armed track.
    ///
    /// The clip is one undo step, taken here rather than when recording began: a take that is
    /// abandoned should leave the history exactly as it found it, and until the file is closed
    /// there is nothing in the document to undo.
    pub fn stop_recording(&mut self) -> Result<RecordingReport, SessionError> {
        let take = self.take.take().ok_or(SessionError::NotRecording)?;
        let Take {
            writer,
            path,
            inside,
            track,
            ..
        } = take;

        // Read before the take is closed, because closing it is what stops them moving.
        let (started_at, dropped_frames) = match self.input.as_ref() {
            Some(capture) => {
                let seen = (capture.started_at(), capture.dropped_frames());
                // The writer's signal. The device stays open if somebody is monitoring through it.
                capture.end_take();
                seen
            }
            None => (None, 0),
        };
        let frames = match writer.join() {
            Ok((reader, result)) => {
                if let Some(capture) = self.input.as_mut() {
                    capture.restore_reader(reader);
                }
                self.close_input_if_idle();
                result?
            }
            Err(_) => {
                // The reader went down with the thread. The device cannot be recorded through
                // again until it is reopened, which `start_recording` checks for.
                self.close_input_if_idle();
                return Err(SessionError::Io(IoError::WavWrite(format!(
                    "the thread writing {} panicked",
                    path.display()
                ))));
            }
        };

        if frames == 0 {
            // Nothing arrived: the device opened and produced no audio before it was stopped. An
            // empty file and a zero-length clip would both be litter.
            let _ = std::fs::remove_file(&path);
            return Ok(RecordingReport {
                clip: None,
                path: None,
                seconds: 0.0,
                dropped_frames,
                outside_punch: false,
            });
        }

        // Read back through the importer rather than kept from the pool: the engine renders every
        // source at the project's rate, and a device that could not give us that rate has just
        // written a file at its own.
        let buffer = import_audio_file(&path, self.project.sample_rate)?;
        // Everything from here counts in project frames, including where the take begins: the
        // stamp is in *engine* frames, and the two rates are free to disagree.
        let project_rate = self.project.sample_rate.max(1.0);
        let engine_rate = self.engine.sample_rate().max(1.0);
        let take_start = started_at.map_or(0, |frame| {
            (frame as f64 / engine_rate * project_rate).round() as u64
        });

        let punch = self.punch_frames_at(project_rate);
        let Some((offset, kept)) = punch_window(punch, take_start, buffer.frame_count() as u64)
            .filter(|(_, kept)| *kept > 0)
        else {
            // The take never reached the punch region — rolled past it, or stopped before it. The
            // file stays: it is what was played, and the punch being set to the wrong bar is the
            // likeliest reason to be standing here.
            return Ok(RecordingReport {
                clip: None,
                path: Some(path),
                seconds: 0.0,
                dropped_frames,
                outside_punch: true,
            });
        };
        let buffer = match (offset, kept == buffer.frame_count() as u64) {
            (0, true) => buffer,
            _ => buffer.slice(offset as usize, kept as usize),
        };
        let seconds = buffer.frame_count() as f64 / project_rate;
        let start = self
            .project
            .tempo_map
            .seconds_to_ticks(Seconds((take_start + offset) as f64 / project_rate));

        self.record(Edit::RecordTake);
        // Inside the same undo step, and before the new clip exists so it cannot clear itself.
        // Only what the take actually covers: a player who rolled in late replaces less.
        if punch.is_some() {
            let over = self
                .project
                .tempo_map
                .seconds_to_ticks(Seconds((take_start + offset + kept) as f64 / project_rate));
            self.clear_punch_range(track, start, over);
        }
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Take".to_string());
        let source = self.project.add_audio_source(
            name,
            AssetPath::inside(&inside),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        self.record_source_size(source, &path);
        let clip = self
            .project
            .add_audio_clip(track, source, start)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();

        Ok(RecordingReport {
            clip: Some(clip),
            path: Some(path),
            seconds,
            dropped_frames,
            outside_punch: false,
        })
    }

    /// Where an engine frame sits on the musical timeline.
    fn tick_of_frame(&self, frame: u64) -> Ticks {
        let rate = self.engine.sample_rate().max(1.0);
        self.project
            .tempo_map
            .seconds_to_ticks(Seconds(frame as f64 / rate))
    }
}

/// The track a Record press lands on: whatever is armed, or the selection when nothing is.
///
/// Takes what the two tracks *are* rather than looking them up, so the rule can be read and tested
/// without a document, a device or a folder to write into. It is the whole of the feature, and
/// inside a method that opens a sound stream it would be a rule with no test.
///
/// Arming wins, and that is what keeping both is for: the selection is where somebody is *looking*,
/// and there is no other way to say "record the vocal while I read the drum part". A selection
/// that could not hold a take — an instrument track, a bus — is not a target and does not become
/// one by being the only thing selected.
fn take_track(
    armed: Option<TrackId>,
    selected: Option<TrackId>,
    selected_records: bool,
) -> Option<TrackId> {
    armed.or_else(|| selected.filter(|_| selected_records))
}

/// Drains the capture into the file until the take ends, then closes the file.
///
/// Runs on a thread of its own rather than on the session's, because the session's thread is a
/// UI's: a dialog that blocks it for a second would cost the take a second of audio, and the pool
/// this is emptying is the only thing standing between that and a hole in the recording.
///
/// Hands the reader back whatever happened, including on a write error. There is one reader, the
/// next take needs it, and a full disk should cost this take rather than every take after it.
fn write_take(
    mut reader: CaptureReader,
    mut recorder: WavRecorder,
) -> (CaptureReader, Result<u64, IoError>) {
    let mut quiet = 0;
    let result = loop {
        let mut failure = None;
        let samples = reader.drain(|block| {
            // The whole block is still taken from the pool after a failure — the buffers have to
            // go back or the callback starts dropping audio into a take nobody is keeping.
            if failure.is_none()
                && let Err(error) = recorder.write(block)
            {
                failure = Some(error);
            }
        });
        if let Some(error) = failure {
            break Err(error);
        }
        // The device itself went away — unplugged, or the whole capture dropped. Nothing more is
        // coming and the pool is drained.
        if reader.is_finished() {
            break recorder.finish();
        }
        // The take was stopped. Not a reason to close at once: the callback may have been part way
        // through a block when the flag went false, and that block belongs in the file. Two empty
        // passes with a `POLL` between them is comfortably longer than a callback.
        if !reader.is_recording() {
            match samples {
                0 => quiet += 1,
                _ => quiet = 0,
            }
            if quiet >= 2 {
                break recorder.finish();
            }
        }
        if samples == 0 {
            std::thread::sleep(POLL);
        }
    };
    (reader, result)
}

/// A file name for a take on `track`, not already taken inside `folder`.
///
/// Named after the track rather than the clock, because "Vocals 3" is what somebody looking
/// through the folder in a year is trying to find, and a timestamp is what a machine would have
/// chosen. Numbered from the first one free, so deleting take 2 and recording again fills the gap
/// rather than counting past it.
fn take_file_name(folder: &std::path::Path, track_name: &str) -> String {
    let stem = sanitised(track_name);
    let audio = folder.join(auris_io::AUDIO_DIR);
    for attempt in 1..=9_999 {
        let candidate = format!("{stem} {attempt}.wav");
        if !audio.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem}.wav")
}

/// A track name with everything a file name cannot hold taken out.
///
/// A track may be called anything at all, including things a filesystem will refuse — `Gtr/Bass`
/// is one slash away from being a directory that does not exist. What is left is trimmed, and an
/// empty result falls back rather than producing a file called nothing.
fn sanitised(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    match trimmed.is_empty() {
        true => "Take".to_string(),
        false => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions};

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session")
    }

    #[test]
    fn only_an_audio_track_can_be_armed() {
        // An instrument track has no clips a recording could become, and arming one would either
        // fail at the far end of a take or quietly put the audio somewhere else.
        let mut session = session();
        let instrument = session.add_default_instrument_track("Synth").unwrap();
        assert!(session.arm_track(Some(instrument)).is_err());
        assert_eq!(session.armed_track(), None);

        let audio = session.add_audio_track("Vocals");
        session.arm_track(Some(audio)).unwrap();
        assert_eq!(session.armed_track(), Some(audio));

        session.arm_track(None).unwrap();
        assert_eq!(session.armed_track(), None);
    }

    #[test]
    fn arming_is_not_an_undo_step() {
        // It is how somebody gets ready to play, not something they wrote — and a take is usually
        // preceded by two or three attempts at arming the right track.
        let mut session = session();
        let audio = session.add_audio_track("Vocals");
        let before = session.history.undo_edit();
        session.arm_track(Some(audio)).unwrap();
        session.arm_track(None).unwrap();
        assert_eq!(
            session.history.undo_edit(),
            before,
            "arming pushed a step onto the history"
        );
    }

    #[test]
    fn recording_needs_somewhere_to_write() {
        // The one command that refuses on an unsaved project. Every other asset can sit outside
        // the folder until a save picks it up; a take has to be written the moment it starts.
        let mut session = session();
        let audio = session.add_audio_track("Vocals");
        assert!(matches!(
            session.start_recording(Some(audio)),
            Err(SessionError::RecordingNeedsFolder)
        ));
    }

    #[test]
    fn recording_with_nothing_to_record_onto_says_so() {
        let mut session = session();
        assert!(matches!(
            session.start_recording(None),
            Err(SessionError::NothingToRecordOnto)
        ));
        assert!(matches!(
            session.stop_recording(),
            Err(SessionError::NotRecording)
        ));
        assert!(!session.is_recording());
        assert_eq!(session.recording_status(), None);
    }

    #[test]
    fn a_selected_audio_track_is_the_target_without_being_armed() {
        // The whole point of the fallback: choosing a track and pressing Record is the gesture,
        // and the arm button is for the case that gesture cannot express.
        let mut session = session();
        let vocals = session.add_audio_track("Vocals");
        let guitar = session.add_audio_track("Guitar");
        assert_eq!(session.record_target(Some(vocals)), Some(vocals));
        assert_eq!(session.record_target(Some(guitar)), Some(guitar));
        assert_eq!(session.armed_track(), None, "nothing was armed by asking");

        // And an arm overrides it, because "record the vocal while I read the drum part" has no
        // other way of being said.
        session.arm_track(Some(vocals)).unwrap();
        assert_eq!(session.record_target(Some(guitar)), Some(vocals));
        session.arm_track(None).unwrap();
        assert_eq!(session.record_target(Some(guitar)), Some(guitar));
    }

    #[test]
    fn a_selection_that_could_not_hold_a_take_is_not_a_target() {
        // Selecting a synth and pressing Record must say so rather than reaching past it for
        // whichever audio track happens to be nearest.
        let mut session = session();
        let synth = session.add_default_instrument_track("Synth").unwrap();
        session.add_audio_track("Vocals");
        assert_eq!(session.record_target(Some(synth)), None);
        assert_eq!(session.record_target(None), None);
        assert!(matches!(
            session.start_recording(Some(synth)),
            Err(SessionError::NothingToRecordOnto)
        ));
    }

    #[test]
    fn the_arm_wins_and_the_selection_only_stands_in_for_it() {
        let armed = TrackId(1);
        let selected = TrackId(2);
        assert_eq!(take_track(Some(armed), Some(selected), true), Some(armed));
        assert_eq!(take_track(Some(armed), None, false), Some(armed));
        assert_eq!(take_track(None, Some(selected), true), Some(selected));
        assert_eq!(take_track(None, Some(selected), false), None);
        assert_eq!(take_track(None, None, false), None);
    }

    #[test]
    fn a_take_is_named_after_its_track_and_numbered_from_the_first_gap() {
        let folder = std::env::temp_dir().join(format!("auris-take-{}", std::process::id()));
        let audio = folder.join(auris_io::AUDIO_DIR);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&audio).unwrap();

        assert_eq!(take_file_name(&folder, "Vocals"), "Vocals 1.wav");
        std::fs::write(audio.join("Vocals 1.wav"), b"").unwrap();
        std::fs::write(audio.join("Vocals 2.wav"), b"").unwrap();
        assert_eq!(take_file_name(&folder, "Vocals"), "Vocals 3.wav");

        // Deleting one fills the gap rather than counting past it — the folder is something
        // people read, and "Vocals 2" missing between 1 and 3 reads as a lost take.
        std::fs::remove_file(audio.join("Vocals 1.wav")).unwrap();
        assert_eq!(take_file_name(&folder, "Vocals"), "Vocals 1.wav");

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_track_name_a_filesystem_would_refuse_still_makes_a_file() {
        // A track may be called anything. `Gtr/Bass` is one slash away from being a directory
        // that does not exist, and `..` is worse than that.
        assert_eq!(sanitised("Gtr/Bass"), "Gtr-Bass");
        assert_eq!(sanitised("Lead: take?"), "Lead- take-");
        assert_eq!(sanitised("  Drums  "), "Drums");
        assert_eq!(sanitised(".."), "Take");
        assert_eq!(sanitised(""), "Take");
        assert_eq!(sanitised("ボーカル"), "ボーカル");
    }
}
