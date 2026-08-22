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
//! # More than one track at once
//!
//! Every armed track records, each from its own input channels: [`InputChannels`] is a run of
//! them, one wide for a microphone and two for anything stereo. There is still one device, one
//! pool and one thread emptying it — the block that comes off the callback is split by channel
//! and written to a file per track, which is why `auris_engine::capture` takes care to hand over
//! whole frames.
//!
//! A selection standing in for an arm takes the whole device, so a stereo interface with one
//! track selected records what it always did. Arming a second track is what says the inputs are
//! separate players rather than two halves of one signal, and from then on an arm is one channel
//! — [`free_channels`] is where a new one lands and why.
//!
//! Channels the device does not have are recorded as silence rather than refused. An arm outlives
//! the interface it was made for, and a track that came back silent is a thing somebody can see
//! and re-point; a take that would not start because of an arm made last week is not.
//!
//! What is deliberately *not* here is a monitor per armed track. There is one ring and it carries
//! one stereo pair, so monitoring follows the single track it was pointed at and takes that
//! track's own channels. A room full of players recording at once hears itself through the
//! interface, which is what an interface is for.
//!
//! # Counting in
//!
//! A take that is started from a standstill can have bars counted in front of it, and the count
//! is a property of the *transport*: the playhead is held where the take will begin while the
//! click counts, and nothing in the arrangement sounds. `auris_engine::transport::CountIn` is
//! where that is arranged and why it is not a stretch of the timeline.
//!
//! What happens here is the two ends of it. [`count_in_for`] decides how many beats a press of
//! Record is worth — the meter at the playhead, the tempo at the playhead, and nothing at all
//! when the transport is already rolling. And the take, which begins during the count rather than
//! after it, has whatever it caught of the count taken off the front of it on the way to becoming
//! a clip. The file keeps it, in the same way punch leaves the whole take on disk: what the
//! player heard is part of what happened, and a take trimmed to the wrong bar can be recovered by
//! hand from the project folder.
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

use auris_core::time::{SignatureMap, TempoMap};
use auris_core::{AssetPath, Seconds, Ticks, TrackId};
use auris_engine::{CaptureReader, CaptureSettings, CountIn, EngineCommand};
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
    /// The thread writing the files. It hands back the pool's reader — there is one, and the next
    /// take needs it — along with the frame count, or why it stopped.
    writer: std::thread::JoinHandle<(CaptureReader, Result<u64, IoError>)>,
    /// One file per armed track, in the order they were armed.
    files: Vec<TakeFile>,
    /// Whether the playhead has been inside the punch region since the take began.
    ///
    /// What makes rolling out automatic work under a cycle. "Past the punch-out" is not a
    /// condition a looping transport ever meets — it wraps before it gets there — so what is
    /// watched for is *leaving* the region, and leaving needs having been in.
    pub(super) entered_punch: bool,
}

/// One track's half of a take: a file being written, and where it will land.
struct TakeFile {
    /// Where the file is, absolute.
    path: PathBuf,
    /// Its name inside the project folder, which is what the document will store.
    inside: PathBuf,
    /// The track the clip will land on.
    track: TrackId,
}

/// A run of input channels one take reads.
///
/// One channel wide for a microphone, two for anything stereo. Wider is allowed and nothing
/// stops it, because a device with more channels than a pair is exactly the device this exists
/// for and an ambisonic rig is not worth refusing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputChannels {
    /// The device channel the take's first channel comes from, counting from zero.
    pub first: usize,
    /// How many channels, so the file is mono at one and stereo at two.
    pub count: usize,
}

impl InputChannels {
    /// A run of `count` channels starting at `first`, never narrower than one.
    pub fn new(first: usize, count: usize) -> Self {
        Self {
            first,
            count: count.max(1),
        }
    }

    /// One channel.
    pub fn mono(first: usize) -> Self {
        Self::new(first, 1)
    }

    /// A pair, starting at `first`.
    pub fn stereo(first: usize) -> Self {
        Self::new(first, 2)
    }

    /// One past the last channel this reads.
    pub fn end(self) -> usize {
        self.first + self.count
    }

    /// Whether device channel `channel` is one of these.
    pub fn contains(self, channel: usize) -> bool {
        (self.first..self.end()).contains(&channel)
    }
}

/// A track armed to record, and where its audio comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm {
    /// The track a take will land on.
    pub track: TrackId,
    /// The device channels it reads.
    pub input: InputChannels,
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

/// What one armed track's half of a finished take produced.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeReport {
    /// The track it was recorded onto.
    pub track: TrackId,
    /// The clip, or `None` when the take was empty and nothing was kept.
    pub clip: Option<auris_core::ClipId>,
    /// The file, if one was kept.
    pub path: Option<PathBuf>,
    /// How long it turned out to be.
    pub seconds: f64,
    /// The take was recorded and none of it fell inside the punch region.
    ///
    /// A different thing from an empty take and it has to read as one: nothing was wrong with the
    /// microphone, the player rolled past their own punch or stopped before reaching it. The file
    /// is kept — see [`Self::path`] — because a punch set to the wrong bar is the likeliest reason
    /// to be reading this.
    pub outside_punch: bool,
}

/// What a finished take produced, one entry per track that was armed for it.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingReport {
    /// Each armed track's half, in the order they were armed.
    ///
    /// Never empty for a take that started: a track that came back with nothing still has an
    /// entry, because "which of the four came back silent" is the question somebody reading this
    /// is asking.
    pub takes: Vec<TakeReport>,
    /// Frames the disk could not keep up with.
    ///
    /// One number for the whole take rather than one per track: there is a single pool behind
    /// every file, and a block lost is lost from all of them at once.
    ///
    /// Not an error and not silently swallowed either: the take is usable and everything after
    /// the gap has moved earlier by that much, which is a thing to be told rather than to
    /// discover in a mix a week later.
    pub dropped_frames: u64,
}

impl RecordingReport {
    /// How many of the armed tracks came back with a clip.
    pub fn clips(&self) -> usize {
        self.takes.iter().filter(|take| take.clip.is_some()).count()
    }

    /// How long the take was, from the longest of its tracks.
    ///
    /// The longest rather than a sum: they were all recorded at once and are the same length,
    /// unless a punch region trimmed one of them differently.
    pub fn seconds(&self) -> f64 {
        self.takes
            .iter()
            .map(|take| take.seconds)
            .fold(0.0, f64::max)
    }

    /// Nothing became a clip, and at least one track was recorded and missed the punch region.
    ///
    /// The question a status line asks to tell "the microphone was dead" from "you played
    /// outside your own punch", and it has to be asked of the whole take: one track landing in
    /// the region and another missing it is a take that worked.
    pub fn outside_punch(&self) -> bool {
        self.clips() == 0 && self.takes.iter().any(|take| take.outside_punch)
    }
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

    /// Every track that has been armed by hand, and the channels each one reads.
    ///
    /// Not the same question as "where would a take land" — that is
    /// [`record_targets`](Session::record_targets), which falls back to the selection. This one is
    /// for drawing arm buttons, which have to show what was *chosen* rather than what would
    /// happen.
    pub fn armed_tracks(&self) -> &[Arm] {
        &self.armed
    }

    /// The channels `track` is armed to read, if it is armed at all.
    pub fn track_arm(&self, track: TrackId) -> Option<InputChannels> {
        self.armed
            .iter()
            .find(|arm| arm.track == track)
            .map(|arm| arm.input)
    }

    /// Whether a take would land on `track` without it having been armed.
    ///
    /// The selection standing in for an arm, which is what makes recording "click the track and
    /// press Record". Separate from [`track_arm`](Session::track_arm) because a button has to
    /// show the difference: one of them is a choice somebody made and the other is where the eye
    /// happens to be.
    pub fn is_record_target(&self, track: TrackId, selected: Option<TrackId>) -> bool {
        self.armed.is_empty() && selected == Some(track) && self.records_audio(track)
    }

    /// The tracks a take would be recorded onto, given whatever the caller has selected.
    ///
    /// The selection is passed in rather than held, because a selection belongs to whatever is
    /// showing the document — a headless session has none, and two windows onto one session would
    /// have one each.
    ///
    /// `&mut` because an implicit target has to be given channels, and how many the device has is
    /// remembered rather than asked for every time. See
    /// [`input_channel_count`](Session::input_channel_count).
    pub fn record_targets(&mut self, selected: Option<TrackId>) -> Vec<Arm> {
        let records = selected.is_some_and(|id| self.records_audio(id));
        let channels = self.input_channel_count();
        take_tracks(&self.armed, selected, records, channels)
    }

    /// Arms an audio track for recording, on `input` or on whatever channels are free.
    ///
    /// Every armed track records, each from its own channels, which is how a band goes down at
    /// once. Arming a track that is already armed re-points it rather than adding a second entry;
    /// passing `None` for one leaves the channels it already has, so a caller that only wants it
    /// armed does not have to know where it is listening.
    ///
    /// Only an audio track, because only an audio track has anywhere for a take to land.
    ///
    /// Deliberately not an undo step. Arming is how somebody prepares to play, not something they
    /// wrote, and a take is usually preceded by several attempts at arming the right track.
    pub fn arm_track(
        &mut self,
        track: TrackId,
        input: Option<InputChannels>,
    ) -> Result<(), SessionError> {
        let found = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if found.kind.as_audio().is_none() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: found.kind.label(),
                expected: "Audio",
            });
        }
        match (self.armed.iter().position(|arm| arm.track == track), input) {
            (Some(at), Some(input)) => self.armed[at].input = input,
            (Some(_), None) => {}
            (None, given) => {
                let claimed: Vec<InputChannels> = self.armed.iter().map(|arm| arm.input).collect();
                let channels = self.input_channel_count();
                let input = given.unwrap_or_else(|| free_channels(&claimed, channels));
                self.armed.push(Arm { track, input });
            }
        }
        self.point_monitor();
        Ok(())
    }

    /// Takes a track out of the arm, leaving every other one where it was.
    pub fn disarm_track(&mut self, track: TrackId) {
        self.armed.retain(|arm| arm.track != track);
        self.point_monitor();
    }

    /// Disarms everything, so a take would follow the selection again.
    pub fn disarm_all(&mut self) {
        self.armed.clear();
        self.point_monitor();
    }

    /// How many channels the input device has, as far as the session knows.
    ///
    /// The open device's own count, or the last count seen, or what the host says the configured
    /// device offers. Remembered rather than asked for each time, because a picker asks this
    /// while it draws and [`input_devices`](Session::input_devices) talks to the OS audio server.
    /// Forgotten when the audio settings change, which is the only thing that can make it stale.
    pub fn input_channel_count(&mut self) -> usize {
        if let Some(capture) = self.input.as_ref() {
            let channels = capture.channel_count().max(1);
            self.input_channels = Some(channels);
            return channels;
        }
        if let Some(channels) = self.input_channels {
            return channels;
        }
        if self.headless {
            // A headless session has no device policy and nothing to play through, and asking the
            // OS audio server what the interface can do is a question that would be answered
            // differently on every machine a test ran on. Not remembered either, so a device
            // opened later is still read from.
            return 2;
        }
        let wanted = self.audio.input_device.clone();
        let found =
            auris_engine::input_devices()
                .into_iter()
                .find(|device| match wanted.as_deref() {
                    Some(name) => device.name == name,
                    None => device.is_default,
                });
        // Two, when there is no device to ask. It is what a laptop has, and the alternative is
        // arming a track to a device with no channels at all.
        let channels = found.map_or(2, |device| (device.max_channels as usize).max(1));
        self.input_channels = Some(channels);
        channels
    }

    /// Points the monitor's ring at the channels the track it plays is armed to read.
    ///
    /// Called wherever either of those can change. An unarmed track monitors the first pair,
    /// which is what a laptop microphone is and what this did before an arm named channels at all.
    pub(super) fn point_monitor(&self) {
        if let (Some(track), Some(ring)) = (self.monitored, self.monitor_ring()) {
            ring.set_source(self.track_arm(track).map_or(0, |input| input.first));
        }
    }

    /// Whether `track` is one a take could land on.
    fn records_audio(&self, track: TrackId) -> bool {
        self.project
            .track(track)
            .is_some_and(|found| found.kind.as_audio().is_some())
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

    /// The loudest sample on each input channel since this was last called, for a meter per
    /// armed track.
    ///
    /// `out` is the caller's own buffer, resized to the device's channel count and left empty
    /// when no device is open. Reset on read like [`Self::input_peak`], and separately from it,
    /// so a frontend can draw both without either reading silence.
    ///
    /// What a track's own reading is comes from [`input_level_of`], which is where the arm's
    /// channels are turned into a number.
    pub fn take_input_peaks(&self, out: &mut Vec<f32>) {
        match self.input.as_ref() {
            Some(capture) => capture.take_channel_peaks(out),
            None => out.clear(),
        }
    }

    /// `true` while the input device is open and [`Self::input_peak`] means something.
    ///
    /// What a frontend hangs an input meter on. Asking instead whether a take or a monitor is
    /// running would be asking two questions to answer one, and would get the answer wrong in the
    /// moment between them: the device is opened by either and closed again only once neither
    /// wants it, and this is that rule rather than a restatement of it.
    pub fn input_is_open(&self) -> bool {
        self.input.is_some()
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
        // Whatever else happens below, the device this counted the channels of may not be the
        // device any more. Cleared here rather than at every call site, because this is the one
        // funnel a change of input passes through.
        self.input_channels = None;
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

    /// Finishes a take that is still running because the session is going away.
    ///
    /// The thread writing the file patches the header with the take's real length only when it is
    /// told the take has ended. A [`JoinHandle`](std::thread::JoinHandle) that is merely dropped
    /// detaches that thread instead, so a process leaving mid-take leaves behind a WAV whose
    /// header says it holds nothing — which is every player's answer as well.
    ///
    /// Nothing else is rescued on the way out: the document has either been asked about already
    /// or is being abandoned deliberately. This one thing is waited for because it is the only
    /// thing here that cannot be made again by doing the same work twice.
    pub(super) fn abandon_take(&mut self) {
        let Some(take) = self.take.take() else {
            return;
        };
        // The writer stops when the capture says the take is over, not when the handle is joined;
        // joining first would wait for a thread that has not been told to finish.
        if let Some(capture) = self.input.as_ref() {
            capture.end_take();
        }
        let files: Vec<String> = take
            .files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect();
        let named = files.join(", ");
        match take.writer.join() {
            Ok((_, Ok(frames))) => log::info!("closed {named} at {frames} frames"),
            Ok((_, Err(error))) => log::warn!("could not finish {named}: {error}"),
            Err(_) => log::warn!("the thread writing {named} panicked"),
        }
    }

    /// Starts recording onto every track [`record_targets`](Session::record_targets) names.
    ///
    /// Opens the input device and begins writing immediately; the transport is left alone, so a
    /// caller that wants to record *against* the rest of the song plays it as well.
    ///
    /// One file per track, all of them fed by one thread from one pool, because they are one
    /// device's channels split up rather than several devices. A track armed to channels the
    /// device does not have records silence — see the module note on why that is not a refusal.
    ///
    /// The arm is left exactly as it was found. A take that armed the track it landed on would
    /// pin the next one there too, so selecting another track and pressing Record again would
    /// quietly record onto the first — which is the sequence this whole fallback exists to fix.
    pub fn start_recording(&mut self, selected: Option<TrackId>) -> Result<(), SessionError> {
        if self.take.is_some() {
            return Err(SessionError::AlreadyRecording);
        }
        let targets = self.record_targets(selected);
        if targets.is_empty() {
            return Err(SessionError::NothingToRecordOnto);
        }
        // Re-checked rather than trusted: a track may have been deleted, or turned into something
        // else, since it was armed. All of them before any file is opened, so a take that cannot
        // happen leaves nothing behind.
        let mut named = Vec::with_capacity(targets.len());
        for arm in targets {
            match self.project.track(arm.track) {
                Some(found) if found.kind.as_audio().is_some() => {
                    named.push((arm, found.name.clone()));
                }
                Some(found) => {
                    return Err(SessionError::WrongTrackKind {
                        id: arm.track.0,
                        actual: found.kind.label(),
                        expected: "Audio",
                    });
                }
                None => return Err(SessionError::UnknownTrack(arm.track.0)),
            }
        }
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
        let reader = self
            .input
            .as_mut()
            .expect("the input was just opened")
            .take_reader()
            .expect("a fresh capture has its reader");

        let rate = reader.sample_rate();
        let mut files: Vec<TakeFile> = Vec::with_capacity(named.len());
        let mut streams = Vec::with_capacity(named.len());
        for (arm, name) in named {
            // Named after the track, and free by the time the next one is chosen: creating the
            // file is what stops two tracks of the same name landing on one path.
            let inside = PathBuf::from(auris_io::AUDIO_DIR).join(take_file_name(&folder, &name));
            let path = folder.join(&inside);
            match WavRecorder::create(&path, rate, arm.input.count) {
                Ok(recorder) => {
                    streams.push(TakeStream::new(recorder, arm.input));
                    files.push(TakeFile {
                        path,
                        inside,
                        track: arm.track,
                    });
                }
                Err(error) => {
                    // Everything this take had already opened goes with it. Those files hold
                    // nothing, and a folder with three empty takes in it from a recording that
                    // never started is litter somebody has to work out the meaning of.
                    drop(streams);
                    for file in &files {
                        let _ = std::fs::remove_file(&file.path);
                    }
                    // The reader has to go back even when nothing is going to use it, or the
                    // device is stuck with no consumer and every later take reopens it.
                    if let Some(capture) = self.input.as_mut() {
                        capture.restore_reader(reader);
                    }
                    self.close_input_if_idle();
                    return Err(error.into());
                }
            }
        }

        let writer = match std::thread::Builder::new()
            .name("auris-record".to_string())
            .spawn(move || write_take(reader, streams))
        {
            Ok(writer) => writer,
            Err(error) => {
                let path = files
                    .first()
                    .map(|file| file.path.clone())
                    .unwrap_or_else(|| folder.clone());
                for file in &files {
                    let _ = std::fs::remove_file(&file.path);
                }
                self.close_input_if_idle();
                return Err(SessionError::Io(IoError::Filesystem {
                    path,
                    source: error,
                }));
            }
        };

        // The count is written down before it is asked for, and before the take opens, so that a
        // first block arriving in the gap between the command and the audio thread picking it up
        // finds a count rather than a zero — see `EngineHandle::expect_count_in`.
        let count = self.count_in();
        self.engine
            .expect_count_in(count.map_or(0, |count| count.remaining_frames));

        // Last, so the callback starts feeding the pool with a writer already draining it.
        self.input
            .as_ref()
            .expect("the input was just opened")
            .begin_take();

        // And the transport after that, because a count-in is a thing the transport does: the
        // file is already open, so what the count occupies at the head of it is trimmed off on
        // the way to becoming a clip rather than missed.
        self.counting = count;
        if let Some(count) = count {
            self.send(EngineCommand::CountIn(count));
            self.play();
        }
        self.take = Some(Take {
            writer,
            files,
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
        let Take { writer, files, .. } = take;

        // Read before the take is closed, because closing it is what stops them moving.
        let (started_at, count_in_frames, dropped_frames) = match self.input.as_ref() {
            Some(capture) => {
                let seen = (
                    capture.started_at(),
                    capture.count_in_at_start(),
                    capture.dropped_frames(),
                );
                // The writer's signal. The device stays open if somebody is monitoring through it.
                capture.end_take();
                seen
            }
            None => (None, 0, 0),
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
                    files
                        .first()
                        .map(|file| file.path.display().to_string())
                        .unwrap_or_default()
                ))));
            }
        };

        if frames == 0 {
            // Nothing arrived: the device opened and produced no audio before it was stopped. An
            // empty file and a zero-length clip would both be litter. Every track still gets an
            // entry, because which of them came back with nothing is the question being asked.
            let takes = files
                .iter()
                .map(|file| {
                    let _ = std::fs::remove_file(&file.path);
                    TakeReport {
                        track: file.track,
                        clip: None,
                        path: None,
                        seconds: 0.0,
                        outside_punch: false,
                    }
                })
                .collect();
            return Ok(RecordingReport {
                takes,
                dropped_frames,
            });
        }

        // Everything from here counts in project frames, including where the take begins: the
        // stamp is in *engine* frames, and the two rates are free to disagree.
        let project_rate = self.project.sample_rate.max(1.0);
        let engine_rate = self.engine.sample_rate().max(1.0);
        let in_project_frames =
            |frame: u64| (frame as f64 / engine_rate * project_rate).round() as u64;
        let take_start = started_at.map_or(0, in_project_frames);
        // Whatever the take caught of its own count-in sits at the head of every one of its
        // files, in front of the position they were stamped with: the playhead does not move
        // while a count is played. Dropping it is what puts the clip on the downbeat.
        let count_in = in_project_frames(count_in_frames);

        // A transaction rather than a single `record`, because clearing the punch region goes
        // through `split_clip` and `remove_clip` and each of those records a step of its own when
        // there is no transaction open. Punching over one clip that spanned the region pushed four
        // entries — the take, two splits and a removal — so the one Undo the user expected to
        // reverse "the recording" instead landed them between the splits, holding three fragments
        // with new ids and no take.
        //
        // One transaction for the whole press, however many tracks it landed on: four tracks
        // recorded at once are one thing that happened, and undoing three quarters of it is not a
        // state anybody meant to be in.
        self.begin_transaction(Edit::RecordTake);
        let mut takes = Vec::with_capacity(files.len());
        for file in &files {
            match self.land_take(file, take_start, count_in, project_rate) {
                Ok(report) => takes.push(report),
                Err(error) => {
                    // The take's own files stay — they are what was played — but the document
                    // goes back the way it was. A cleared punch region with no clip to show for
                    // it is not a state one Undo could sort out, and the transaction is what
                    // makes putting it back possible.
                    self.revert_transaction();
                    return Err(error);
                }
            }
        }
        self.invalidate_graph();
        self.end_transaction();

        Ok(RecordingReport {
            takes,
            dropped_frames,
        })
    }

    /// Turns one finished file into a clip on the track it was recorded for.
    ///
    /// Called inside the take's transaction, once per armed track. Everything it does to the
    /// document is undone with the rest of the take when a later track fails.
    fn land_take(
        &mut self,
        file: &TakeFile,
        take_start: u64,
        count_in: u64,
        project_rate: f64,
    ) -> Result<TakeReport, SessionError> {
        // Read back through the importer rather than kept from the pool: the engine renders every
        // source at the project's rate, and a device that could not give us that rate has just
        // written a file at its own.
        let buffer = import_audio_file(&file.path, self.project.sample_rate)?;
        let punch = self.punch_frames_at(project_rate);
        // The count-in comes off the front before the punch is worked out, because everything the
        // punch is measured in — the take's position, what it covers — begins where the count
        // ends. A take stopped during its own count leaves nothing to keep.
        let played = (buffer.frame_count() as u64).saturating_sub(count_in);
        let Some((offset, kept)) =
            punch_window(punch, take_start, played).filter(|(_, kept)| *kept > 0)
        else {
            // The take never reached the punch region — rolled past it, or stopped before it. The
            // file stays: it is what was played, and the punch being set to the wrong bar is the
            // likeliest reason to be standing here.
            //
            // Or there was no region and the take was stopped during its own count-in, which is
            // nothing kept for an entirely different reason. Saying "outside the punch" about a
            // project that has no punch would send somebody looking for a region to fix.
            return Ok(TakeReport {
                track: file.track,
                clip: None,
                path: Some(file.path.clone()),
                seconds: 0.0,
                outside_punch: punch.is_some(),
            });
        };
        let buffer = match (count_in + offset, kept == buffer.frame_count() as u64) {
            (0, true) => buffer,
            (from, _) => buffer.slice(from as usize, kept as usize),
        };
        let seconds = buffer.frame_count() as f64 / project_rate;
        let start = self
            .project
            .tempo_map
            .seconds_to_ticks(Seconds((take_start + offset) as f64 / project_rate));

        // Before the new clip exists so it cannot clear itself. Only what the take actually
        // covers: a player who rolled in late replaces less.
        if punch.is_some() {
            let over = self
                .project
                .tempo_map
                .seconds_to_ticks(Seconds((take_start + offset + kept) as f64 / project_rate));
            self.clear_punch_range(file.track, start, over);
        }
        let name = file
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Take".to_string());
        let source = self.project.add_audio_source(
            name,
            AssetPath::inside(&file.inside),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        self.record_source_size(source, &file.path);
        let clip = self
            .project
            .add_audio_clip(file.track, source, start)
            .ok_or(SessionError::UnknownTrack(file.track.0))?;
        // A take knows what tempo it was played at, because the transport was running it. Stamped
        // rather than switched on: a take does not *follow* the tempo by default — a vocal that
        // moved when somebody nudged the tempo would be a surprise — but the day it is asked to,
        // the number it needs is already there and right.
        let played_at = self.project.tempo_map.bpm_at(start);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.source_bpm = Some(played_at);
        }
        self.install_source(source, Arc::new(buffer));

        Ok(TakeReport {
            track: file.track,
            clip: Some(clip),
            path: Some(file.path.clone()),
            seconds,
            outside_punch: false,
        })
    }

    /// The count-in a press of Record would get, from where the playhead is now.
    ///
    /// `None` for a project that asks for none and for a transport that is already rolling; the
    /// rule, and the reason for it, is the free `count_in_for` at the foot of this module.
    pub fn count_in(&self) -> Option<CountIn> {
        count_in_for(
            self.project.count_in_bars,
            self.is_playing(),
            self.playhead(),
            &self.project.signatures,
            &self.project.tempo_map,
            self.engine.sample_rate(),
        )
    }

    /// Where an engine frame sits on the musical timeline.
    fn tick_of_frame(&self, frame: u64) -> Ticks {
        let rate = self.engine.sample_rate().max(1.0);
        self.project
            .tempo_map
            .seconds_to_ticks(Seconds(frame as f64 / rate))
    }
}

/// What one arm's meter reads, given every channel's level.
///
/// The loudest of the channels it takes rather than a level per channel: an arm is one track and
/// a track has one meter, and a stereo pair whose sides were metered apart would be two readings
/// of one thing. Channels the device does not have contribute nothing, which is the same answer
/// recording gives them — silence, rather than a neighbour's level.
///
/// Free of the session because it is the whole of what a meter needs to know, and a frontend
/// asking about a track it is drawing should not have to reach for a device to be told.
pub fn input_level_of(peaks: &[f32], input: InputChannels) -> f32 {
    peaks
        .iter()
        .skip(input.first)
        .take(input.count)
        .fold(0.0f32, |loudest, level| loudest.max(*level))
}

/// The count-in a press of Record gets, in frames of whatever clock `rate` counts.
///
/// Nothing to do with a document or a device, so the rule can be read and tested on its own: how
/// many beats, how long each of them is, and which of them are downbeats.
///
/// **Bars are counted in the meter at `at`**, so two bars of 7/8 is fourteen beats and two bars of
/// 6/8 is four — the felt beat, the one somebody counts out loud, which is the same beat the click
/// uses. **One tempo throughout**, read where the take begins: a count that accelerated into the
/// first bar would be a count nobody could play to, and the number a musician needs from it is the
/// one the song is about to be at.
///
/// `None` when no bars were asked for, and when the transport is **already rolling** — a count-in
/// is how a take begins from a standstill. Somebody who is playing along to the song already has
/// their bars in front of them, and stopping the music to count them again would be a strange
/// thing for Record to do.
fn count_in_for(
    bars: u32,
    playing: bool,
    at: Ticks,
    signatures: &SignatureMap,
    tempo_map: &TempoMap,
    rate: f64,
) -> Option<CountIn> {
    if bars == 0 || playing {
        return None;
    }
    let at = at.max_zero();
    let signature = signatures.signature_at(signatures.bar_floor(at));
    let per_bar = signature.felt_beats().max(1);
    let rate = rate.max(1.0);
    let from = tempo_map.ticks_to_samples(at, rate).raw();
    let to = tempo_map
        .ticks_to_samples(at + signature.beat_ticks(), rate)
        .raw();
    let beat_frames = to.saturating_sub(from);
    (beat_frames > 0).then(|| CountIn::new(bars * per_bar, beat_frames, per_bar))
}

/// The tracks a Record press lands on: everything armed, or the selection when nothing is.
///
/// Takes what the arms and the selection *are* rather than looking anything up, so the rule can be
/// read and tested without a document, a device or a folder to write into. It is the whole of the
/// feature, and inside a method that opens a sound stream it would be a rule with no test.
///
/// Arming wins, and that is what keeping both is for: the selection is where somebody is *looking*,
/// and there is no other way to say "record the vocals while I read the drum part". A selection
/// that could not hold a take — an instrument track, a bus — is not a target and does not become
/// one by being the only thing selected.
fn take_tracks(
    armed: &[Arm],
    selected: Option<TrackId>,
    selected_records: bool,
    device_channels: usize,
) -> Vec<Arm> {
    if !armed.is_empty() {
        return armed.to_vec();
    }
    selected
        .filter(|_| selected_records)
        .map(|track| Arm {
            track,
            input: free_channels(&[], device_channels),
        })
        .into_iter()
        .collect()
}

/// Where a newly armed track listens, given what the other arms have taken.
///
/// The whole device when it has a pair or fewer and nothing else is armed, because that is what
/// recording did before there was a second arm at all: a laptop's microphone is one channel and an
/// interface's pair is a pair, and a stereo synth plugged into both should not come back as its
/// left half. Every arm after that is a single channel, because a second armed track means a
/// second player rather than the other half of one signal.
///
/// The lowest channel nobody else has taken. When they all have, the last one, shared: two tracks
/// reading one input is a strange thing to have asked for and a visible one, where a refusal at
/// the moment somebody armed a track would be neither.
fn free_channels(claimed: &[InputChannels], device_channels: usize) -> InputChannels {
    let channels = device_channels.max(1);
    if claimed.is_empty() && channels <= 2 {
        return InputChannels::new(0, channels);
    }
    let free = (0..channels).find(|channel| !claimed.iter().any(|input| input.contains(*channel)));
    InputChannels::mono(free.unwrap_or(channels - 1))
}

/// The channels of an interleaved `block` that belong to one take, lifted out of it.
///
/// `out` is cleared and refilled with `input.count` samples per frame. Channels the device does
/// not have come out silent rather than shifting the others along: an arm outlives the interface
/// it was made for, and a track that came back silent says which input went missing where a track
/// that came back holding its neighbour's audio would not.
///
/// A free function over three numbers because it is the whole of the channel map, and this is the
/// one place a mistake in it would be inaudible until the take was over.
fn pick_channels(block: &[f32], channels: usize, input: InputChannels, out: &mut Vec<f32>) {
    let channels = channels.max(1);
    out.clear();
    for frame in 0..block.len() / channels {
        let base = frame * channels;
        for channel in input.first..input.end() {
            out.push(match channel < channels {
                true => block[base + channel],
                false => 0.0,
            });
        }
    }
}

/// One track's file inside a running take, and where its channels come from.
struct TakeStream {
    /// The file being written.
    recorder: WavRecorder,
    /// The device channels that go into it.
    input: InputChannels,
    /// Its own channels, lifted out of the interleaved block. Kept between blocks so that
    /// emptying the pool allocates nothing after the first one.
    scratch: Vec<f32>,
}

impl TakeStream {
    /// A stream writing `input`'s channels into `recorder`.
    fn new(recorder: WavRecorder, input: InputChannels) -> Self {
        Self {
            recorder,
            input,
            scratch: Vec::new(),
        }
    }
}

/// Drains the capture into the files until the take ends, then closes them.
///
/// Runs on a thread of its own rather than on the session's, because the session's thread is a
/// UI's: a dialog that blocks it for a second would cost the take a second of audio, and the pool
/// this is emptying is the only thing standing between that and a hole in the recording.
///
/// One thread for every armed track rather than one each, because there is one pool and it has
/// exactly one consumer by construction. Each block is split by channel as it comes off.
///
/// Hands the reader back whatever happened, including on a write error. There is one reader, the
/// next take needs it, and a full disk should cost this take rather than every take after it.
fn write_take(
    mut reader: CaptureReader,
    mut streams: Vec<TakeStream>,
) -> (CaptureReader, Result<u64, IoError>) {
    let channels = reader.channel_count();
    let mut quiet = 0;
    let result = loop {
        let mut failure = None;
        let samples = reader.drain(|block| {
            // The whole block is still taken from the pool after a failure — the buffers have to
            // go back or the callback starts dropping audio into a take nobody is keeping.
            if failure.is_some() {
                return;
            }
            for stream in streams.iter_mut() {
                pick_channels(block, channels, stream.input, &mut stream.scratch);
                if let Err(error) = stream.recorder.write(&stream.scratch) {
                    failure = Some(error);
                    break;
                }
            }
        });
        if let Some(error) = failure {
            break Err(error);
        }
        // The device itself went away — unplugged, or the whole capture dropped. Nothing more is
        // coming and the pool is drained.
        if reader.is_finished() {
            break finish_all(streams);
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
                break finish_all(streams);
            }
        }
        if samples == 0 {
            std::thread::sleep(POLL);
        }
    };
    (reader, result)
}

/// Closes every file the take was writing, and says how long it turned out to be.
///
/// All of them, even after one has failed. A WAV whose header was never patched says it holds
/// nothing, whatever is actually in the file, and losing three good tracks to one bad one would
/// be a worse answer than the error itself.
fn finish_all(streams: Vec<TakeStream>) -> Result<u64, IoError> {
    let mut frames = 0;
    let mut failure = None;
    for stream in streams {
        match stream.recorder.finish() {
            Ok(written) => frames = frames.max(written),
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(frames),
    }
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
    use auris_core::time::TimeSignature;

    use crate::{Session, SessionOptions};

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session")
    }

    /// A count-in of `bars` at 120 BPM in `signature`, from the start of the song.
    fn counted(bars: u32, signature: TimeSignature) -> Option<CountIn> {
        count_in_for(
            bars,
            false,
            Ticks::ZERO,
            &SignatureMap::constant(signature),
            &TempoMap::constant(120.0),
            48_000.0,
        )
    }

    #[test]
    fn a_count_in_is_as_many_beats_as_the_meter_puts_in_a_bar() {
        // Two bars of four at 120: eight beats of half a second.
        let count = counted(2, TimeSignature::new(4, 4)).expect("a count");
        assert_eq!(count.beats, 8);
        assert_eq!(count.per_bar, 4);
        assert_eq!(count.beat_frames, 24_000);
        assert_eq!(count.total_frames(), 192_000);

        // Seven-eight is counted in sevens, and the beat is the eighth it is written in.
        let seven = counted(1, TimeSignature::new(7, 8)).expect("a count");
        assert_eq!(seven.beats, 7);
        assert_eq!(seven.per_bar, 7);
        assert_eq!(seven.beat_frames, 12_000);

        // And a compound meter is counted in the beats it is felt in: two dotted quarters, not
        // six eighths, which is the same beat the click takes.
        let six = counted(1, TimeSignature::new(6, 8)).expect("a count");
        assert_eq!(six.beats, 2);
        assert_eq!(six.per_bar, 2);
        assert_eq!(six.beat_frames, 36_000);
    }

    #[test]
    fn nothing_is_counted_in_when_nothing_was_asked_for() {
        assert_eq!(counted(0, TimeSignature::new(4, 4)), None);
    }

    #[test]
    fn a_transport_that_is_already_rolling_is_not_counted_in() {
        // The bars are going past already. Stopping the song to count them again is not what
        // pressing Record over a playing arrangement means.
        assert_eq!(
            count_in_for(
                2,
                true,
                Ticks::ZERO,
                &SignatureMap::default(),
                &TempoMap::constant(120.0),
                48_000.0,
            ),
            None
        );
    }

    #[test]
    fn a_count_in_takes_the_tempo_where_the_take_begins() {
        // Ninety at the top and one-eighty from bar three; a take starting at bar three is
        // counted in at one-eighty, which is what the player is about to have to play at.
        let mut tempo_map = TempoMap::constant(90.0);
        let bar_three = SignatureMap::default().bar_start(3);
        tempo_map.set_point(bar_three, 180.0);
        let count = count_in_for(
            1,
            false,
            bar_three,
            &SignatureMap::default(),
            &tempo_map,
            48_000.0,
        )
        .expect("a count");
        assert_eq!(
            count.beat_frames, 16_000,
            "a beat at 180 is a third of a second"
        );
    }

    #[test]
    fn the_count_in_setting_is_kept_but_never_recorded() {
        let mut session = session();
        assert_eq!(session.count_in_bars(), 0);
        assert!(session.count_in().is_none());

        session.set_count_in_bars(2);
        assert_eq!(session.count_in_bars(), 2);
        assert!(session.is_dirty(), "the setting has to reach the file");
        assert!(
            !session.can_undo(),
            "counting in is preparation, not an edit"
        );

        // And it is bounded, so that nobody sits through sixteen bars twice.
        session.set_count_in_bars(99);
        assert_eq!(session.count_in_bars(), Session::MAX_COUNT_IN_BARS);
    }

    /// The tracks a take would land on, by id, which is what most of these are asking about.
    fn targets(session: &mut Session, selected: Option<TrackId>) -> Vec<TrackId> {
        session
            .record_targets(selected)
            .into_iter()
            .map(|arm| arm.track)
            .collect()
    }

    #[test]
    fn only_an_audio_track_can_be_armed() {
        // An instrument track has no clips a recording could become, and arming one would either
        // fail at the far end of a take or quietly put the audio somewhere else.
        let mut session = session();
        let instrument = session.add_default_instrument_track("Synth").unwrap();
        assert!(session.arm_track(instrument, None).is_err());
        assert!(session.armed_tracks().is_empty());

        let audio = session.add_audio_track("Vocals");
        session.arm_track(audio, None).unwrap();
        assert_eq!(session.track_arm(audio), Some(InputChannels::stereo(0)));

        session.disarm_track(audio);
        assert!(session.armed_tracks().is_empty());
    }

    #[test]
    fn every_armed_track_stays_armed_and_takes_its_own_channel() {
        // The whole of multitrack recording as the document sees it: arming a second track adds
        // a second take rather than moving the first one.
        let mut session = session();
        let kick = session.add_audio_track("Kick");
        let snare = session.add_audio_track("Snare");
        session
            .arm_track(kick, Some(InputChannels::mono(0)))
            .unwrap();
        session.arm_track(snare, None).unwrap();

        assert_eq!(session.armed_tracks().len(), 2);
        assert_eq!(session.track_arm(kick), Some(InputChannels::mono(0)));
        assert_eq!(
            session.track_arm(snare),
            Some(InputChannels::mono(1)),
            "the second arm should have found the channel the first left"
        );

        // Arming one of them again re-points it rather than adding a third entry.
        session
            .arm_track(snare, Some(InputChannels::stereo(0)))
            .unwrap();
        assert_eq!(session.armed_tracks().len(), 2);
        assert_eq!(session.track_arm(snare), Some(InputChannels::stereo(0)));

        session.disarm_all();
        assert!(session.armed_tracks().is_empty());
    }

    #[test]
    fn deleting_a_track_takes_its_arm_with_it() {
        // An arm on a track that has gone would refuse the next take rather than being ignored by
        // it, and the button that could have cleared it went with the track.
        let mut session = session();
        let kick = session.add_audio_track("Kick");
        let snare = session.add_audio_track("Snare");
        session.arm_track(kick, None).unwrap();
        session.arm_track(snare, None).unwrap();

        session.remove_track(kick).unwrap();
        assert_eq!(session.armed_tracks().len(), 1);
        assert_eq!(session.armed_tracks()[0].track, snare);
    }

    #[test]
    fn arming_is_not_an_undo_step() {
        // It is how somebody gets ready to play, not something they wrote — and a take is usually
        // preceded by two or three attempts at arming the right track.
        let mut session = session();
        let audio = session.add_audio_track("Vocals");
        let before = session.history.undo_edit();
        session.arm_track(audio, None).unwrap();
        session.disarm_track(audio);
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
        assert!(session.is_record_target(vocals, Some(vocals)));
        assert!(!session.is_record_target(vocals, Some(guitar)));
        assert_eq!(targets(&mut session, Some(guitar)), vec![guitar]);
        assert!(
            session.armed_tracks().is_empty(),
            "nothing was armed by asking"
        );

        // And an arm overrides it, because "record the vocal while I read the drum part" has no
        // other way of being said.
        session.arm_track(vocals, None).unwrap();
        assert_eq!(targets(&mut session, Some(guitar)), vec![vocals]);
        assert!(
            !session.is_record_target(guitar, Some(guitar)),
            "a selection standing in for an arm, with an arm already there"
        );
        session.disarm_track(vocals);
        assert_eq!(targets(&mut session, Some(guitar)), vec![guitar]);
    }

    #[test]
    fn a_selection_that_could_not_hold_a_take_is_not_a_target() {
        // Selecting a synth and pressing Record must say so rather than reaching past it for
        // whichever audio track happens to be nearest.
        let mut session = session();
        let synth = session.add_default_instrument_track("Synth").unwrap();
        session.add_audio_track("Vocals");
        assert!(targets(&mut session, Some(synth)).is_empty());
        assert!(targets(&mut session, None).is_empty());
        assert!(matches!(
            session.start_recording(Some(synth)),
            Err(SessionError::NothingToRecordOnto)
        ));
    }

    #[test]
    fn the_arm_wins_and_the_selection_only_stands_in_for_it() {
        let kick = Arm {
            track: TrackId(1),
            input: InputChannels::mono(0),
        };
        let snare = Arm {
            track: TrackId(2),
            input: InputChannels::mono(1),
        };
        let selected = TrackId(3);
        // Every armed track, in the order they were armed, whatever is selected.
        assert_eq!(
            take_tracks(&[kick, snare], Some(selected), true, 8),
            vec![kick, snare]
        );
        assert_eq!(take_tracks(&[kick], None, false, 8), vec![kick]);
        // Nothing armed: the selection, and only when it could hold a take.
        assert_eq!(
            take_tracks(&[], Some(selected), true, 2),
            vec![Arm {
                track: selected,
                input: InputChannels::stereo(0),
            }]
        );
        assert!(take_tracks(&[], Some(selected), false, 2).is_empty());
        assert!(take_tracks(&[], None, false, 2).is_empty());
    }

    #[test]
    fn an_arms_meter_reads_the_loudest_of_its_own_channels() {
        // Four channels, and a stereo arm on the third and fourth. What it reads is what is on
        // those two — the loud microphone on channel one is somebody else's meter.
        let peaks = [0.9, 0.1, 0.2, 0.4];
        assert!((input_level_of(&peaks, InputChannels::stereo(2)) - 0.4).abs() < 1e-6);
        assert!((input_level_of(&peaks, InputChannels::mono(0)) - 0.9).abs() < 1e-6);

        // A channel the device does not have reads silence, which is what it records.
        assert_eq!(input_level_of(&peaks, InputChannels::mono(9)), 0.0);
        assert!((input_level_of(&peaks, InputChannels::stereo(3)) - 0.4).abs() < 1e-6);
        assert_eq!(input_level_of(&[], InputChannels::mono(0)), 0.0);
    }

    #[test]
    fn a_new_arm_takes_the_lowest_channel_nobody_else_is_reading() {
        // A device with a pair or fewer and nothing else armed: the whole thing, which is what
        // recording did before an arm could name channels at all.
        assert_eq!(free_channels(&[], 2), InputChannels::stereo(0));
        assert_eq!(free_channels(&[], 1), InputChannels::mono(0));
        // An interface: one channel, because a second armed track is a second player.
        assert_eq!(free_channels(&[], 8), InputChannels::mono(0));
        assert_eq!(
            free_channels(&[InputChannels::stereo(0)], 8),
            InputChannels::mono(2)
        );
        // The lowest gap rather than the next one along, so disarming a track and arming another
        // fills the hole.
        assert_eq!(
            free_channels(&[InputChannels::mono(0), InputChannels::mono(2)], 4),
            InputChannels::mono(1)
        );
        // Every channel spoken for: the last one, shared. Two tracks on one input is visible;
        // refusing to arm a track would only be baffling.
        assert_eq!(
            free_channels(&[InputChannels::stereo(0)], 2),
            InputChannels::mono(1)
        );
    }

    #[test]
    fn a_take_reads_only_the_channels_its_track_was_armed_to() {
        // Four inputs, each carrying its own number, one frame after another.
        let block: Vec<f32> = (0..3)
            .flat_map(|frame| [0.0, 1.0, 2.0, 3.0].map(|channel| frame as f32 + channel / 10.0))
            .collect();
        let mut out = Vec::new();

        pick_channels(&block, 4, InputChannels::mono(2), &mut out);
        assert_eq!(out, vec![0.2, 1.2, 2.2]);

        pick_channels(&block, 4, InputChannels::stereo(1), &mut out);
        assert_eq!(out, vec![0.1, 0.2, 1.1, 1.2, 2.1, 2.2]);

        // The whole device, which is the ordinary single-track take.
        pick_channels(&block, 4, InputChannels::new(0, 4), &mut out);
        assert_eq!(out, block);
    }

    #[test]
    fn a_channel_the_device_does_not_have_is_recorded_as_silence() {
        // Not as its neighbour. An arm outlives the interface it was made for, and a track that
        // came back holding the microphone next door would pass for a good take.
        let block = vec![0.5, -0.5, 0.5, -0.5];
        let mut out = Vec::new();

        pick_channels(&block, 2, InputChannels::stereo(1), &mut out);
        assert_eq!(out, vec![-0.5, 0.0, -0.5, 0.0]);

        pick_channels(&block, 2, InputChannels::stereo(6), &mut out);
        assert_eq!(out, vec![0.0; 4]);
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
