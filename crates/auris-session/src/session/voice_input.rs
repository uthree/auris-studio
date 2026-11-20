//! Humming a part into a note take.
//!
//! A note take writes down keys going down and coming up; this is a third keyboard, made of the
//! microphone. With voice input on, Record on a track that holds notes opens the input device as
//! well, and what is sung is followed by [`auris_analysis::hum`] and written into the same take,
//! at the same heard frames, as a key would be. Nothing else about the take changes: the record
//! mode, the cycle passes and the quantise all apply to a sung note as they do to a struck one.
//!
//! # Off the session's thread
//!
//! The capture's reader goes to a thread of its own, as it does for an audio take: a window that
//! stalls for a second must not cost the take a second of singing. The thread decides the notes
//! and sends them back, each stamped with the input frame it began or ended at, counted from the
//! take's first sample; the session turns that into a heard frame when it next polls.
//!
//! # When a note was sung
//!
//! An input frame becomes a heard frame through the same three numbers an audio take is placed
//! by — where the capture stamped its first sample, how much count-in that sample was into, and
//! the round trip back to what the singer was hearing — plus a measured input calibration, if
//! there is one for these devices. So a hummed note lands where a recording of the same humming
//! would, and the tracker's own delay in *deciding* costs nothing in *placing*: every event
//! carries the frame it happened at, not the frame it was decided at.
//!
//! # Hearing it
//!
//! A sung note is written down and not played. Playing it would sound it the better part of a
//! tenth of a second late — the tracker waits for four frames to agree before it is sure — over
//! the singer's own voice, which is a worse guide to singing in tune than the voice alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use auris_analysis::hum::{HumEvent, HumTracker};
use auris_core::TrackId;
use auris_engine::CaptureReader;

use super::Session;
use crate::error::SessionError;

/// How long the listening thread sleeps when the pool was empty.
const IDLE: Duration = Duration::from_millis(5);

/// A voice being followed into a note take.
pub(super) struct VoiceTake {
    /// The track the take is writing onto.
    track: TrackId,
    /// Notes decided on the listening thread, in input frames.
    events: Receiver<HumEvent>,
    /// Set to end the listening thread.
    stop: Arc<AtomicBool>,
    /// The listening thread, which hands the capture's reader back when it ends. `None` in a
    /// test, which feeds `events` itself.
    worker: Option<JoinHandle<CaptureReader>>,
    /// Input frames to engine frames.
    ratio: f64,
    /// The heard frame of input frame zero, once the capture has stamped its first sample.
    origin: Option<f64>,
}

/// What the listening thread does: drains the capture, follows the voice, and sends the notes
/// back, until it is told to stop and the pool runs dry.
fn listen(
    mut reader: CaptureReader,
    stop: Arc<AtomicBool>,
    events: Sender<HumEvent>,
) -> CaptureReader {
    let channels = reader.channel_count().max(1);
    let mut tracker = HumTracker::new(reader.sample_rate());
    let mut mono = Vec::new();
    let mut decided = Vec::new();
    loop {
        // Read before draining: a stop seen here means every block that was coming has had a
        // pass to arrive in, the one about to run.
        let stopping = stop.load(Ordering::Acquire);
        let drained = reader.drain(|block| {
            mono.clear();
            mono.extend(
                block
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
            tracker.push(&mono, &mut decided);
        });
        for event in decided.drain(..) {
            let _ = events.send(event);
        }
        if reader.is_finished() || (stopping && drained == 0) {
            break;
        }
        if drained == 0 {
            std::thread::sleep(IDLE);
        }
    }
    tracker.finish(&mut decided);
    for event in decided {
        let _ = events.send(event);
    }
    reader
}

impl Session {
    /// Whether Record on a track that holds notes also follows the microphone.
    pub fn voice_input(&self) -> bool {
        self.voice_input
    }

    /// Chooses whether the next note take follows the microphone as well.
    ///
    /// Off by default: it opens the input device, which is a light on the menu bar and a
    /// question from the operating system the first time. A preference of the person recording,
    /// like the record mode, so not an undo step.
    pub fn set_voice_input(&mut self, on: bool) {
        self.voice_input = on;
    }

    /// `true` while a note take is following the microphone.
    pub fn following_voice(&self) -> bool {
        self.voice.is_some()
    }

    /// Opens the input device and starts a thread listening to it for a take on `track`.
    ///
    /// The device is not fed until [`Self::begin_voice`]: a note take writes down its count-in
    /// before the capture may stamp a first sample, as an audio take does.
    pub(super) fn start_voice(&mut self, track: TrackId) -> Result<(), SessionError> {
        self.open_input()?;
        if self.input.as_ref().is_some_and(|c| !c.has_reader()) {
            self.input = None;
            self.open_input()?;
        }
        let Some(capture) = self.input.as_mut() else {
            return Err(auris_engine::EngineError::NoInputDevice.into());
        };
        let reader = capture
            .take_reader()
            .expect("a capture with no take running has its reader");
        let ratio = self.engine.sample_rate().max(1.0) / reader.sample_rate().max(1.0);
        let stop = Arc::new(AtomicBool::new(false));
        let (send, events) = channel();
        let thread_stop = Arc::clone(&stop);
        // A thread that cannot be spawned takes the reader down with it, which leaves the capture
        // without one; the next take finds that and opens the device again, as an audio take's
        // does.
        let spawned = std::thread::Builder::new()
            .name("auris-voice".to_string())
            .spawn(move || listen(reader, thread_stop, send));
        let worker = match spawned {
            Ok(worker) => worker,
            Err(error) => {
                self.close_input_if_idle();
                return Err(SessionError::AudioRestart(format!(
                    "could not start listening to the microphone: {error}"
                )));
            }
        };
        self.voice = Some(VoiceTake {
            track,
            events,
            stop,
            worker: Some(worker),
            ratio,
            origin: None,
        });
        Ok(())
    }

    /// Lets the input feed the listening thread. Called once the count-in is written down.
    pub(super) fn begin_voice(&self) {
        if self.voice.is_some()
            && let Some(capture) = self.input.as_ref()
        {
            capture.begin_take();
        }
    }

    /// The heard frame of the take's first input sample, once the capture has stamped one.
    fn voice_origin(&self) -> Option<f64> {
        let capture = self.input.as_ref()?;
        let started = capture.started_at()? as f64;
        let rate = self.engine.sample_rate().max(1.0);
        let measured = self
            .audio
            .input_calibration()
            .map_or(0.0, |calibration| calibration.offset_ms);
        let lag = capture.latency_at_start() as f64 + measured / 1000.0 * rate;
        Some(started - capture.count_in_at_start() as f64 - lag)
    }

    /// Writes into the take whatever the listening thread has decided since the last poll.
    pub(super) fn service_voice(&mut self) {
        let Some(voice) = self.voice.as_ref() else {
            return;
        };
        if voice.origin.is_none() {
            let origin = self.voice_origin();
            if let Some(voice) = self.voice.as_mut() {
                voice.origin = origin;
            }
        }
        let Some(voice) = self.voice.as_ref() else {
            return;
        };
        // Until the first sample is stamped there is no saying where anything was sung, and
        // nothing can have been: the events wait in the channel.
        let Some(origin) = voice.origin else {
            return;
        };
        let (track, ratio) = (voice.track, voice.ratio);
        let events: Vec<HumEvent> = voice.events.try_iter().collect();
        for event in events {
            match event {
                HumEvent::On {
                    pitch,
                    velocity,
                    frame,
                } => {
                    let at = origin + frame as f64 * ratio;
                    self.note_take_struck_at(track, pitch, velocity, at);
                }
                HumEvent::Off { pitch, frame } => {
                    let at = origin + frame as f64 * ratio;
                    self.note_take_released_at(track, pitch, at);
                }
            }
        }
    }

    /// Stops following the microphone, writes down what was still on its way, and gives the
    /// capture's reader back.
    pub(super) fn finish_voice(&mut self) {
        let Some(voice) = self.voice.as_mut() else {
            return;
        };
        voice.stop.store(true, Ordering::Release);
        let worker = voice.worker.take();
        if let Some(capture) = self.input.as_ref() {
            capture.end_take();
        }
        if let Some(worker) = worker {
            match worker.join() {
                Ok(reader) => {
                    if let Some(capture) = self.input.as_mut() {
                        capture.restore_reader(reader);
                    }
                }
                Err(_) => log::warn!("the thread following the microphone panicked"),
            }
        }
        self.service_voice();
        self.voice = None;
        self.close_input_if_idle();
    }

    /// Stands a voice in for the microphone, fed by the test through the returned sender.
    #[cfg(test)]
    pub(super) fn fake_voice(&mut self, track: TrackId, ratio: f64) -> Sender<HumEvent> {
        let (send, events) = channel();
        self.voice = Some(VoiceTake {
            track,
            events,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            ratio,
            origin: Some(0.0),
        });
        send
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session")
    }

    #[test]
    fn voice_input_is_off_until_it_is_asked_for() {
        let session = session();
        assert!(!session.voice_input());
        assert!(!session.following_voice());
    }

    #[test]
    fn asking_for_it_on_a_session_with_no_microphone_says_so_and_records_nothing() {
        let mut session = session();
        let track = session.add_default_instrument_track("Hum").unwrap();
        session.set_voice_input(true);
        let refused = session.start_recording(Some(track));
        assert!(refused.is_err(), "{refused:?}");
        assert!(!session.is_recording());
        assert!(!session.following_voice());
    }

    #[test]
    fn hummed_notes_land_in_the_take_where_they_were_sung() {
        let mut session = session();
        let track = session.add_default_instrument_track("Hum").unwrap();
        session.start_recording(Some(track)).unwrap();
        // The input at half the engine's rate, so a frame of it is two of the engine's.
        let voice = session.fake_voice(track, 2.0);
        let rate = session.engine().sample_rate();
        let half = (rate / 4.0) as u64;
        voice
            .send(HumEvent::On {
                pitch: 60,
                velocity: 0.8,
                frame: 0,
            })
            .unwrap();
        voice
            .send(HumEvent::Off {
                pitch: 60,
                frame: half,
            })
            .unwrap();
        voice
            .send(HumEvent::On {
                pitch: 64,
                velocity: 0.4,
                frame: half,
            })
            .unwrap();
        voice
            .send(HumEvent::Off {
                pitch: 64,
                frame: 2 * half,
            })
            .unwrap();
        session.service_voice();
        session.finish_voice();
        let report = session.stop_recording().unwrap();
        let clip = report.takes[0].clip.expect("a clip");
        let (_, clip) = session.project().midi_clip(clip).expect("a MIDI clip");
        let mut notes: Vec<_> = clip
            .notes
            .iter()
            .map(|note| (note.pitch, note.velocity))
            .collect();
        notes.sort_by_key(|note| note.0);
        assert_eq!(notes.len(), 2, "{:?}", clip.notes);
        assert_eq!(notes[0].0, 60);
        assert_eq!(notes[1].0, 64);
        assert!(notes[0].1 > notes[1].1);
        // Half a second each, at the input's half rate.
        let tempo = &session.project().tempo_map;
        let first = clip.notes.iter().find(|note| note.pitch == 60).unwrap();
        let second = clip.notes.iter().find(|note| note.pitch == 64).unwrap();
        let seconds = |ticks| tempo.ticks_to_seconds(ticks).0;
        let length = seconds(first.start + first.length) - seconds(first.start);
        assert!((length - 0.5).abs() < 0.01, "{first:?}");
        assert!(
            (seconds(second.start) - seconds(first.start) - 0.5).abs() < 0.01,
            "{second:?}"
        );
    }
}
