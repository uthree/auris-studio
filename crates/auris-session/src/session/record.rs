//! Recording a take onto an audio track.
//!
//! Three things happen at once and none of them can wait for the others. The input device fills a
//! pool from its own callback; a thread of this crate's own empties that pool into a file; and the
//! session goes on being a session, drawing meters and moving the playhead. What ties them
//! together is that the [`Capture`] *is* the take — dropping it closes the device, which is how
//! the writing thread finds out it is done.
//!
//! # What a take needs before it can start
//!
//! A folder. A recording is a file, and a project that has never been saved has nowhere to put
//! one; every other asset a project refers to can sit outside the folder until a save picks it up,
//! but a take has to be *written* somewhere the moment it begins. So this is the one command that
//! refuses on an unsaved document, and says so rather than inventing a temporary directory whose
//! contents would be lost the first time the machine tidied it.
//!
//! An armed track, too, and an audio one. Recording onto a track chosen for you is how a take
//! ends up somewhere nobody looks.
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
use auris_engine::{Capture, CaptureReader, CaptureSettings};
use auris_io::{IoError, WavRecorder, import_audio_file};

use super::Session;
use crate::error::SessionError;
use crate::history::Edit;

/// How often the writing thread looks for more audio when the pool came up empty.
///
/// The pool holds well over a second, so this is nowhere near tight enough to matter for safety;
/// it is short enough that stopping a take feels immediate and long enough that an idle take is
/// a hundred wake-ups a second rather than a spin.
const POLL: Duration = Duration::from_millis(10);

/// A take being recorded.
pub(super) struct Take {
    /// The device. Dropping this closes it, which is how the writer learns the take is over.
    capture: Capture,
    /// The thread writing the file; it hands back the frame count, or why it stopped.
    writer: std::thread::JoinHandle<Result<u64, IoError>>,
    /// Where the file is, absolute.
    path: PathBuf,
    /// Its name inside the project folder, which is what the document will store.
    inside: PathBuf,
    /// The track the clip will land on.
    track: TrackId,
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
    /// The loudest sample since this was last read, for an input meter.
    pub peak: f32,
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

    /// The track a take would be recorded onto.
    pub fn armed_track(&self) -> Option<TrackId> {
        self.armed
    }

    /// Arms an audio track for recording, or disarms whatever was armed with `None`.
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

    /// How the running take is doing, for a meter and a clock.
    pub fn recording_status(&self) -> Option<RecordingStatus> {
        let take = self.take.as_ref()?;
        let capture = &take.capture;
        let rate = capture.sample_rate().max(1.0);
        Some(RecordingStatus {
            device: capture.name().to_string(),
            seconds: capture.frames() as f64 / rate,
            start: capture.started_at().map(|frame| self.tick_of_frame(frame)),
            peak: capture.take_peak(),
            dropped_frames: capture.dropped_frames(),
            running: capture.is_running(),
        })
    }

    /// Starts recording onto the armed track.
    ///
    /// Opens the input device and begins writing immediately; the transport is left alone, so a
    /// caller that wants to record *against* the rest of the song plays it as well.
    pub fn start_recording(&mut self) -> Result<(), SessionError> {
        if self.take.is_some() {
            return Err(SessionError::AlreadyRecording);
        }
        let track = self.armed.ok_or(SessionError::NothingArmed)?;
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

        let settings = CaptureSettings {
            device: self.audio.input_device.clone(),
            // The rate the project renders at, so a take needs no resampling on the way in. A
            // device that cannot do it is recorded at whatever it can and resampled below.
            sample_rate: Some(self.project.sample_rate.round().max(1.0) as u32),
            block_frames: Some(self.audio.block_frames),
        };
        let mut capture = auris_engine::start_capture(&settings, &self.engine)?;
        let reader = capture
            .take_reader()
            .expect("a fresh capture has its reader");

        let inside = PathBuf::from(auris_io::AUDIO_DIR).join(take_file_name(&folder, &name));
        let path = folder.join(&inside);
        let recorder = WavRecorder::create(&path, reader.sample_rate(), reader.channel_count())?;
        let writer = std::thread::Builder::new()
            .name("auris-record".to_string())
            .spawn(move || write_take(reader, recorder))
            .map_err(|error| {
                SessionError::Io(IoError::Filesystem {
                    path: path.clone(),
                    source: error,
                })
            })?;

        self.take = Some(Take {
            capture,
            writer,
            path,
            inside,
            track,
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
            capture,
            writer,
            path,
            inside,
            track,
        } = take;

        let started_at = capture.started_at();
        let dropped_frames = capture.dropped_frames();
        // The device closes here, and that is the only signal the writer gets.
        drop(capture);
        let frames = match writer.join() {
            Ok(result) => result?,
            Err(_) => {
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
            });
        }

        // Read back through the importer rather than kept from the pool: the engine renders every
        // source at the project's rate, and a device that could not give us that rate has just
        // written a file at its own.
        let buffer = import_audio_file(&path, self.project.sample_rate)?;
        let seconds = buffer.frame_count() as f64 / self.project.sample_rate.max(1.0);
        let start = started_at.map_or(Ticks::ZERO, |frame| self.tick_of_frame(frame));

        self.record(Edit::RecordTake);
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

/// Drains the capture into the file until the device closes, then closes the file.
///
/// Runs on a thread of its own rather than on the session's, because the session's thread is a
/// UI's: a dialog that blocks it for a second would cost the take a second of audio, and the pool
/// this is emptying is the only thing standing between that and a hole in the recording.
fn write_take(mut reader: CaptureReader, mut recorder: WavRecorder) -> Result<u64, IoError> {
    loop {
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
            return Err(error);
        }
        if reader.is_finished() {
            break;
        }
        if samples == 0 {
            std::thread::sleep(POLL);
        }
    }
    recorder.finish()
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
        session.arm_track(Some(audio)).unwrap();
        assert!(matches!(
            session.start_recording(),
            Err(SessionError::RecordingNeedsFolder)
        ));
    }

    #[test]
    fn recording_with_nothing_armed_says_so() {
        let mut session = session();
        assert!(matches!(
            session.start_recording(),
            Err(SessionError::NothingArmed)
        ));
        assert!(matches!(
            session.stop_recording(),
            Err(SessionError::NotRecording)
        ));
        assert!(!session.is_recording());
        assert_eq!(session.recording_status(), None);
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
