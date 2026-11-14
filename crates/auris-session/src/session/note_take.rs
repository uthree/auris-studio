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
//!
//! # Recording round a cycle
//!
//! With the loop on, the heard frame jumps back to the loop start at every wrap, and that jump is
//! how a take tells one *pass* from the next: an event more than half a loop earlier than the one
//! before it began a new pass. (The heard frame is extrapolated from the last block published, so
//! one may overshoot the loop end by a block; it is folded back to the start, where it was heard.)
//! A note held across the wrap ends at the loop end.
//!
//! What the passes become is the [`NoteRecordMode`]:
//!
//! * **Takes** — every pass is a clip of its own, stacked on the same bars, and all but the last
//!   are muted. Choosing another is [`Session::choose_take`]. Without a cycle, a clip of its own.
//! * **Merge** — everything played goes into one clip, every pass on top of the last; and into
//!   the clip already on the track there, if there is one, rather than a new clip over it.
//! * **Replace** — the notes already on the track over the take's bars are taken away, and only
//!   the last pass is kept.

use std::collections::BTreeMap;

use auris_core::project::CurvePoint;
use auris_core::{ClipId, Note, Seconds, Ticks, TrackId};
use auris_engine::EngineCommand;
use serde::{Deserialize, Serialize};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;
use super::record::{RecordingReport, RecordingStatus, TakeReport};

/// What a note take does with what was there before, and with the passes of a cycle.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteRecordMode {
    /// A clip per take, and a clip per pass of a cycle with all but the last muted.
    #[default]
    Takes,
    /// Into the clip already there, every pass on top of the last.
    Merge,
    /// Clearing the notes already there, and keeping only the last pass.
    Replace,
}

/// How a take's notes are pulled towards a grid as they are written down.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordQuantize {
    /// The grid, in ticks.
    pub grid: Ticks,
    /// How far each note moves towards its nearest line: 0 leaves it where it was played, 1
    /// puts it on the line.
    pub strength: f32,
}

/// Where a note struck at `tick` is written, under `quantize`.
///
/// The nearest line of the grid, either way, approached by `strength` of the distance. Measured
/// on the timeline rather than from the clip's start, because a take's clip begins wherever the
/// first note was played and the grid does not.
pub fn record_quantized(tick: Ticks, quantize: RecordQuantize) -> Ticks {
    let grid = Ticks(quantize.grid.raw().max(1));
    let line = tick.max_zero().snap_nearest(grid);
    let strength = f64::from(quantize.strength.clamp(0.0, 1.0));
    let moved = (line.raw() - tick.raw()) as f64 * strength;
    Ticks(tick.raw() + moved.round() as i64)
}

/// A note struck and not yet released: pitch, velocity, where struck, which pass.
type Held = (u8, f32, f64, usize);

/// A note struck and released: pitch, velocity, where struck, where released, which pass.
type Played = (u8, f32, f64, f64, usize);

/// A note take under way.
#[derive(Clone, Debug)]
pub(super) struct NoteTake {
    /// The track being written onto.
    track: TrackId,
    /// The virtual frame the take was started at, in engine frames: where the clip begins
    /// unless something was played in front of it.
    start: f64,
    /// The loop the transport is cycling round, in frames, if it is.
    cycle: Option<(f64, f64)>,
    /// Which pass of the cycle the last event was in.
    pass: usize,
    /// Where the last event was, folded into the cycle.
    last_at: f64,
    /// When the take began on the process's clock, the virtual frame it began at, and the rate:
    /// what reckons how far round the cycle the transport has gone. `None` with no output
    /// running, where nothing moves and passes are told apart by the heard frame alone.
    began: Option<(std::time::Instant, f64, f64)>,
    /// Notes struck and not yet released.
    held: Vec<Held>,
    /// Notes struck and released.
    played: Vec<Played>,
    /// Where the bend was moved to, when, and in which pass.
    bend: Vec<(f64, f32, usize)>,
    /// Where each controller was moved to: number, where, value, which pass.
    controllers: Vec<(u8, f64, f32, usize)>,
}

impl NoteTake {
    /// The track being written onto.
    pub(super) fn track(&self) -> TrackId {
        self.track
    }

    /// Folds an event's frame into the cycle and says which pass it is in.
    fn stamp(&mut self, at: f64) -> (f64, usize) {
        let reckoned = self
            .began
            .map(|(instant, frame, rate)| frame + instant.elapsed().as_secs_f64() * rate);
        self.stamp_reckoned(at, reckoned)
    }

    /// [`Self::stamp`], given how far the transport has gone since the take began.
    ///
    /// The heard frame says *where* in the loop an event was, exactly; it cannot say which pass,
    /// because every pass reads the same. The process's clock can: time since the take began is
    /// how many times round the transport has been, to within the drift of two clocks — far less
    /// than the half a loop it takes to pick the wrong pass. Without a clock (a silent session)
    /// a new pass is an event more than half a loop *earlier* than the one before it, which
    /// misses a pass in which nothing was played late enough to jump back from.
    fn stamp_reckoned(&mut self, at: f64, reckoned: Option<f64>) -> (f64, usize) {
        let mut at = at;
        if let Some((start, end)) = self.cycle {
            let length = end - start;
            if at >= end {
                at = start + (at - end) % length;
            }
            match reckoned {
                Some(linear) if linear >= end => {
                    // Pass k (from one) puts `at` at end + (k - 1) * length + (at - start).
                    let k = ((linear - end - (at - start)) / length).round() + 1.0;
                    self.pass = k.max(0.0) as usize;
                }
                Some(_) => self.pass = 0,
                None => {
                    if at < self.last_at - length / 2.0 {
                        self.pass += 1;
                    }
                }
            }
        }
        self.last_at = at;
        (at, self.pass)
    }

    /// Where a note struck in `struck_pass` ends when it is let go at `at` in `pass`.
    fn release_point(&self, struck: f64, struck_pass: usize, at: f64, pass: usize) -> f64 {
        match self.cycle {
            Some((_, end)) if pass > struck_pass => end.max(struck),
            _ => at.max(struck),
        }
    }
}

/// One clip a finished take is to write: the stretch it covers and what was played in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TakePart {
    /// Which pass of the cycle, or zero without one.
    pub pass: usize,
    /// The frame the part starts at, before any pickup.
    pub start: f64,
    /// The frame it ends at, unless a note runs past it.
    pub end: f64,
    /// Its notes: pitch, velocity, where struck, where released.
    pub notes: Vec<(u8, f32, f64, f64)>,
    /// Whether it is to be muted: an earlier pass, under [`NoteRecordMode::Takes`].
    pub muted: bool,
}

/// How a finished take divides into clips, under `mode`.
///
/// Pure, so the rules about passes are tested without a transport going round. `end` is where
/// the take stopped and `end_pass` the pass it stopped in.
pub(crate) fn take_parts(
    take: &NoteTake,
    end: f64,
    end_pass: usize,
    mode: NoteRecordMode,
) -> Vec<TakePart> {
    let (cycle_start, cycle_end) = take.cycle.unwrap_or((take.start, end));
    let bounds = |pass: usize| {
        let start = if pass == 0 { take.start } else { cycle_start };
        let stop = if pass >= end_pass { end } else { cycle_end };
        (start, stop)
    };
    let notes_of = |pass: Option<usize>| -> Vec<(u8, f32, f64, f64)> {
        take.played
            .iter()
            .filter(|note| pass.is_none_or(|pass| note.4 == pass))
            .map(|&(pitch, velocity, on, off, _)| (pitch, velocity, on, off))
            .collect()
    };
    let mut passes: Vec<usize> = take.played.iter().map(|note| note.4).collect();
    passes.sort_unstable();
    passes.dedup();
    let Some(&last) = passes.last() else {
        return Vec::new();
    };
    match mode {
        NoteRecordMode::Takes => passes
            .iter()
            .map(|&pass| {
                let (start, end) = bounds(pass);
                TakePart {
                    pass,
                    start,
                    end,
                    notes: notes_of(Some(pass)),
                    muted: pass != last,
                }
            })
            .collect(),
        NoteRecordMode::Merge => {
            let start = if end_pass > 0 {
                take.start.min(cycle_start)
            } else {
                take.start
            };
            let end = if end_pass > 0 {
                end.max(cycle_end)
            } else {
                end
            };
            vec![TakePart {
                pass: 0,
                start,
                end,
                notes: notes_of(None),
                muted: false,
            }]
        }
        NoteRecordMode::Replace => {
            let (start, end) = bounds(last);
            vec![TakePart {
                pass: last,
                start,
                end,
                notes: notes_of(Some(last)),
                muted: false,
            }]
        }
    }
}

impl Session {
    /// `true` while what is played on the typing keyboard is being written down.
    pub fn recording_notes(&self) -> bool {
        self.note_take.is_some()
    }

    /// What a note take does with what was there before it.
    pub fn note_record_mode(&self) -> NoteRecordMode {
        self.note_record_mode
    }

    /// How a note take's notes are pulled to the grid, if they are.
    pub fn record_quantize(&self) -> Option<RecordQuantize> {
        self.record_quantize
    }

    /// Chooses how the next note take's notes are pulled to the grid, or that they are not.
    ///
    /// Applied when the take is written, never to the notes afterwards: a take that comes out
    /// too stiff is undone and played again, or quantised by hand to a gentler strength.
    pub fn set_record_quantize(&mut self, quantize: Option<RecordQuantize>) {
        self.record_quantize = quantize;
    }

    /// Chooses what the next note take does with what was there before it.
    ///
    /// A preference of the person recording, kept in their settings rather than the song:
    /// nothing is recorded in the history.
    pub fn set_note_record_mode(&mut self, mode: NoteRecordMode) {
        self.note_record_mode = mode;
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
        let cycle = self.cycle_frames().filter(|&(_, end)| start < end);
        let began = self.engine.is_running().then(|| {
            let counted = count.map_or(0, |count| count.remaining_frames) as f64;
            (
                std::time::Instant::now(),
                start - counted,
                self.engine.sample_rate().max(1.0),
            )
        });
        self.note_take = Some(NoteTake {
            track,
            start,
            cycle,
            pass: 0,
            last_at: start,
            began,
            held: Vec::new(),
            played: Vec::new(),
            bend: Vec::new(),
            controllers: Vec::new(),
        });
        Ok(())
    }

    /// The loop the transport goes round, in frames, when it is on.
    fn cycle_frames(&self) -> Option<(f64, f64)> {
        let (start, end) = self.project.loop_region?;
        if !self.project.loop_enabled || end <= start {
            return None;
        }
        let rate = self.engine.sample_rate();
        let frame = |tick| self.project.tempo_map.ticks_to_samples(tick, rate).raw() as f64;
        Some((frame(start), frame(end)))
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

    /// The take listening to `track`, if there is one.
    fn take_on(&mut self, track: TrackId) -> Option<&mut NoteTake> {
        self.note_take.as_mut().filter(|take| take.track == track)
    }

    /// Writes down a note the typing keyboard struck, if a note take is listening to its track.
    pub(super) fn note_take_struck(&mut self, track: TrackId, pitch: u8, velocity: f32) {
        let at = self.performed_frame();
        self.note_take_struck_at(track, pitch, velocity, at);
    }

    /// Writes down a note struck at the virtual frame `at`.
    ///
    /// The typing keyboard is stamped as it is heard of, less its calibrated lag; a MIDI
    /// keyboard is stamped on the thread that heard it, before the session ever does.
    pub(super) fn note_take_struck_at(
        &mut self,
        track: TrackId,
        pitch: u8,
        velocity: f32,
        at: f64,
    ) {
        let Some(take) = self.take_on(track) else {
            return;
        };
        let (at, pass) = take.stamp(at);
        // A key struck again before it was let go — through sustain — ends the first note
        // where the second begins, which is what an instrument does with it too.
        close_held(take, pitch, at, pass);
        take.held.push((pitch, velocity, at, pass));
    }

    /// Writes down the end of a note the typing keyboard let go of.
    pub(super) fn note_take_released(&mut self, track: TrackId, pitch: u8) {
        let at = self.performed_frame();
        self.note_take_released_at(track, pitch, at);
    }

    /// Writes down the end of a note at the virtual frame `at`.
    pub(super) fn note_take_released_at(&mut self, track: TrackId, pitch: u8, at: f64) {
        if let Some(take) = self.take_on(track) {
            let (at, pass) = take.stamp(at);
            close_held(take, pitch, at, pass);
        }
    }

    /// Writes down the bend moving to `semitones` at the virtual frame `at`.
    pub(super) fn note_take_bent_at(&mut self, track: TrackId, semitones: f32, at: f64) {
        if let Some(take) = self.take_on(track) {
            let (at, pass) = take.stamp(at);
            take.bend.push((at, semitones, pass));
        }
    }

    /// Writes down controller `number` moving to `value` at the virtual frame `at`.
    pub(super) fn note_take_controller_at(
        &mut self,
        track: TrackId,
        number: u8,
        value: f32,
        at: f64,
    ) {
        if let Some(take) = self.take_on(track) {
            let (at, pass) = take.stamp(at);
            take.controllers.push((number, at, value, pass));
        }
    }

    /// Ends the note take and turns what was played into clips, as one undo step.
    pub(super) fn stop_note_take(&mut self) -> Result<RecordingReport, SessionError> {
        let mut take = self.note_take.take().ok_or(SessionError::NotRecording)?;
        let rate = self.engine.sample_rate().max(1.0);
        let now = self.performed_frame();
        let seconds = ((now - take.start) / rate).max(0.0);
        let (end, end_pass) = take.stamp(now);
        for (pitch, velocity, at, pass) in std::mem::take(&mut take.held) {
            let release = take.release_point(at, pass, end, end_pass);
            take.played.push((pitch, velocity, at, release, pass));
        }
        let nothing = |track| RecordingReport {
            takes: vec![TakeReport {
                track,
                clip: None,
                path: None,
                seconds,
                outside_punch: false,
            }],
            dropped_frames: 0,
        };
        let mode = self.note_record_mode;
        let quantize = self.record_quantize;
        let parts = take_parts(&take, end, end_pass, mode);
        if parts.is_empty() || self.project.track(take.track).is_none() {
            return Ok(nothing(take.track));
        }
        let name = self
            .project
            .track(take.track)
            .map(|track| track.name.clone())
            .unwrap_or_default();
        let tempo = self.project.tempo_map.clone();
        let to_ticks = |frame: f64| tempo.seconds_to_ticks(Seconds(frame.max(0.0) / rate));
        self.begin_transaction(Edit::RecordTake);
        let mut takes = Vec::new();
        for part in &parts {
            let Some((clip_start, clip_end, notes)) =
                written_notes(&part.notes, part.start, part.end, quantize, to_ticks)
            else {
                continue;
            };
            let in_part = |pass: usize| mode == NoteRecordMode::Merge || pass == part.pass;
            let bend: Vec<(f64, f32)> = take
                .bend
                .iter()
                .filter(|moved| in_part(moved.2))
                .map(|&(at, value, _)| (at, value))
                .collect();
            let bend = written_curve(&bend, clip_start, clip_end, to_ticks);
            let mut controllers = BTreeMap::new();
            for &(number, ..) in &take.controllers {
                if controllers.contains_key(&number) {
                    continue;
                }
                let moves: Vec<(f64, f32)> = take
                    .controllers
                    .iter()
                    .filter(|moved| moved.0 == number && in_part(moved.3))
                    .map(|&(_, at, value, _)| (at, value))
                    .collect();
                let curve = written_curve(&moves, clip_start, clip_end, to_ticks);
                if !curve.is_empty() {
                    controllers.insert(number, curve);
                }
            }
            let written = Written {
                start: clip_start,
                end: clip_end,
                notes,
                bend,
                controllers,
                muted: part.muted,
            };
            let landed = match mode {
                NoteRecordMode::Merge => self.merge_take(take.track, &name, written),
                NoteRecordMode::Replace => {
                    self.clear_notes_between(take.track, clip_start, clip_end);
                    self.land_note_take(take.track, &name, written)
                }
                NoteRecordMode::Takes => self.land_note_take(take.track, &name, written),
            };
            match landed {
                Ok(clip) => takes.push(clip),
                Err(error) => {
                    self.revert_transaction();
                    return Err(error);
                }
            }
        }
        self.invalidate_graph();
        self.end_transaction();
        if takes.is_empty() {
            return Ok(nothing(take.track));
        }
        // The take that plays first: the last pass, which is the one left unmuted.
        Ok(RecordingReport {
            takes: takes
                .into_iter()
                .rev()
                .map(|clip| TakeReport {
                    track: take.track,
                    clip: Some(clip),
                    path: None,
                    seconds,
                    outside_punch: false,
                })
                .collect(),
            dropped_frames: 0,
        })
    }

    /// Puts a written part on the track as a clip of its own.
    fn land_note_take(
        &mut self,
        track: TrackId,
        name: &str,
        written: Written,
    ) -> Result<ClipId, SessionError> {
        let clip = self.add_midi_clip(
            track,
            name.to_string(),
            written.start,
            Ticks(written.end.raw() - written.start.raw()),
        )?;
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.notes = written.notes;
            target.bend = written.bend;
            target.controllers = written.controllers;
            target.muted = written.muted;
        }
        Ok(clip)
    }

    /// Puts a written part into the clip already on the track where it lands, or into a clip
    /// of its own when there is none. The clip grows to hold it, earlier as well as later.
    fn merge_take(
        &mut self,
        track: TrackId,
        name: &str,
        written: Written,
    ) -> Result<ClipId, SessionError> {
        let Some(clip) = self.midi_clip_under(track, written.start, written.end) else {
            return self.land_note_take(track, name, written);
        };
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return self.land_note_take(track, name, written);
        };
        let start = target.start.min(written.start);
        let shift = target.start.raw() - start.raw();
        if shift > 0 {
            for note in &mut target.notes {
                note.start = Ticks(note.start.raw() + shift);
            }
            for point in target
                .bend
                .iter_mut()
                .chain(target.controllers.values_mut().flatten())
            {
                point.at = Ticks(point.at.raw() + shift);
            }
            target.start = start;
            target.length = Ticks(target.length.raw() + shift);
        }
        let offset = written.start.raw() - start.raw();
        let moved = |at: Ticks| Ticks(at.raw() + offset);
        for mut note in written.notes {
            note.start = moved(note.start);
            target.notes.push(note);
        }
        target.notes.sort_by_key(|note| (note.start, note.pitch));
        let merge = |curve: &mut Vec<CurvePoint>, points: Vec<CurvePoint>| {
            curve.extend(points.into_iter().map(|point| CurvePoint {
                at: moved(point.at),
                value: point.value,
            }));
            curve.sort_by_key(|point| point.at);
        };
        merge(&mut target.bend, written.bend);
        for (number, points) in written.controllers {
            merge(target.controllers.entry(number).or_default(), points);
        }
        let end = Ticks(written.end.raw() - start.raw());
        if end > target.length {
            target.length = end;
        }
        target.looped_note_instances()?;
        Ok(clip)
    }

    /// The first unmuted MIDI clip on `track` that overlaps `start..end`.
    fn midi_clip_under(&self, track: TrackId, start: Ticks, end: Ticks) -> Option<ClipId> {
        self.project
            .track(track)?
            .kind
            .note_clips()?
            .iter()
            .find(|clip| !clip.muted && clip.start < end && clip_end(clip) > start)
            .map(|clip| clip.id)
    }

    /// Takes away every note on `track` that starts in `start..end`, in every clip.
    fn clear_notes_between(&mut self, track: TrackId, start: Ticks, end: Ticks) {
        let Some(index) = self
            .project
            .tracks
            .iter()
            .position(|found| found.id == track)
        else {
            return;
        };
        let Some(clips) = self.project.tracks[index].kind.note_clips_mut() else {
            return;
        };
        for clip in clips {
            let origin = clip.start.raw();
            clip.notes.retain(|note| {
                let at = origin + note.start.raw();
                at < start.raw() || at >= end.raw()
            });
        }
    }

    /// Makes `clip` the take that plays: unmutes it and mutes every other note clip on its
    /// track that it overlaps, as one undo step.
    ///
    /// How one of the takes a cycle stacked up is chosen, and how another is chosen later —
    /// the others are muted, never removed.
    pub fn choose_take(&mut self, clip: ClipId) -> Result<(), SessionError> {
        let (track, start, end) = {
            let (track, found) = self
                .project
                .midi_clip(clip)
                .ok_or(SessionError::UnknownClip(clip.0))?;
            (track, found.start, clip_end(found))
        };
        let index = self.require_track(track)?;
        self.begin_transaction(Edit::RecordTake);
        if let Some(clips) = self.project.tracks[index].kind.note_clips_mut() {
            for other in clips {
                if other.id == clip {
                    other.muted = false;
                } else if other.start < end && clip_end(other) > start {
                    other.muted = true;
                }
            }
        }
        self.invalidate_graph();
        self.end_transaction();
        Ok(())
    }

    /// Plays the next take in the stack `clip` belongs to, and mutes the rest, answering the
    /// take that now plays.
    ///
    /// The stack is `clip` and every note clip on its track that overlaps it, in the order they
    /// were recorded; after the last comes the first again. Stacked takes lie on top of one
    /// another, so a clip underneath could not be clicked to choose it, and this is what walks
    /// down to it.
    pub fn next_take(&mut self, clip: ClipId) -> Result<ClipId, SessionError> {
        let mut stack = self.takes_over(clip);
        stack.push(clip);
        stack.sort();
        let playing = stack.iter().position(|take| {
            self.project
                .midi_clip(*take)
                .is_some_and(|(_, found)| !found.muted)
        });
        let next = stack[playing.map_or(0, |at| (at + 1) % stack.len())];
        self.choose_take(next)?;
        Ok(next)
    }

    /// The other takes stacked on `clip`'s bars: note clips on its track that overlap it.
    pub fn takes_over(&self, clip: ClipId) -> Vec<ClipId> {
        let Some((track, found)) = self.project.midi_clip(clip) else {
            return Vec::new();
        };
        let (start, end) = (found.start, clip_end(found));
        self.project
            .track(track)
            .and_then(|track| track.kind.note_clips())
            .map(|clips| {
                clips
                    .iter()
                    .filter(|other| {
                        other.id != clip && other.start < end && clip_end(other) > start
                    })
                    .map(|other| other.id)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Where a clip ends on the timeline.
fn clip_end(clip: &auris_core::MidiClip) -> Ticks {
    Ticks(clip.start.raw() + clip.length.raw())
}

/// What one part of a take writes into a clip.
struct Written {
    start: Ticks,
    end: Ticks,
    notes: Vec<Note>,
    bend: Vec<CurvePoint>,
    controllers: BTreeMap<u8, Vec<CurvePoint>>,
    muted: bool,
}

/// Ends the held note at `pitch`, if there is one, at `at` in `pass`.
fn close_held(take: &mut NoteTake, pitch: u8, at: f64, pass: usize) {
    if let Some(position) = take.held.iter().position(|held| held.0 == pitch) {
        let (pitch, velocity, struck, struck_pass) = take.held.remove(position);
        let release = take.release_point(struck, struck_pass, at, pass);
        take.played
            .push((pitch, velocity, struck, release, struck_pass));
    }
}

/// A wheel's movements during a take, as a curve across the clip it wrote.
///
/// Measured from the clip's start. A movement made before the clip began is where the wheel
/// already *was* when it did, so the last of them is kept, at the clip's first tick; one made
/// after it ended is dropped. A movement to where the wheel already was writes nothing, so a
/// wheel at rest for a whole take leaves no curve at all.
pub(crate) fn written_curve(
    moves: &[(f64, f32)],
    clip_start: Ticks,
    clip_end: Ticks,
    to_ticks: impl Fn(f64) -> Ticks,
) -> Vec<CurvePoint> {
    let mut sorted = moves.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut curve: Vec<CurvePoint> = Vec::new();
    for &(at, value) in &sorted {
        let tick = to_ticks(at).max(clip_start);
        if tick > clip_end {
            break;
        }
        let point = CurvePoint {
            at: Ticks(tick.raw() - clip_start.raw()),
            value,
        };
        match curve.last_mut() {
            Some(last) if last.at == point.at => last.value = value,
            Some(last) if last.value == value => {}
            _ => curve.push(point),
        }
    }
    // A wheel that only ever rested where it starts moved nothing worth a lane.
    if curve.iter().all(|point| point.value == 0.0) {
        curve.clear();
    }
    curve
}

/// The clip a part of a finished note take writes: where it starts and ends, and its notes.
///
/// `None` when nothing was played. The clip starts where the part did, or earlier for a pickup
/// played during the count, and never before the song; it ends where the part ended or the last
/// note did, whichever is later. A note shorter than a tick is a tick, so a key tapped as fast as
/// a keyboard can report still writes something that can be seen and heard.
///
/// With `quantize`, each note's start is pulled towards the grid and its end moves with it, so it
/// keeps the length it was held for.
pub(crate) fn written_notes(
    played: &[(u8, f32, f64, f64)],
    start: f64,
    end: f64,
    quantize: Option<RecordQuantize>,
    to_ticks: impl Fn(f64) -> Ticks,
) -> Option<(Ticks, Ticks, Vec<Note>)> {
    if played.is_empty() {
        return None;
    }
    let earliest = played
        .iter()
        .map(|note| note.2)
        .fold(start, f64::min)
        .max(0.0);
    let latest = played
        .iter()
        .map(|note| note.3)
        .fold(end.max(start), f64::max);
    // Where each note lands on the timeline: struck, held for, pitch and velocity.
    let placed: Vec<(Ticks, i64, u8, f32)> = played
        .iter()
        // A note that ended before the song began was played to nothing that is written down.
        .filter(|note| note.3 > 0.0 || note.2 >= 0.0)
        .map(|&(pitch, velocity, struck, released)| {
            let on = to_ticks(struck.max(0.0));
            let held = (to_ticks(released.max(0.0)).raw() - on.raw()).max(1);
            let on = quantize.map_or(on, |quantize| record_quantized(on, quantize));
            (on, held, pitch, velocity)
        })
        .collect();
    let clip_start = placed
        .iter()
        .map(|note| note.0)
        .fold(to_ticks(earliest), Ticks::min);
    let mut notes: Vec<Note> = placed
        .iter()
        .map(|&(on, held, pitch, velocity)| {
            let mut note = Note::new(pitch, Ticks(on.raw() - clip_start.raw()), Ticks(held));
            note.velocity = velocity.clamp(0.0, 1.0);
            note
        })
        .collect();
    if notes.is_empty() {
        return None;
    }
    notes.sort_by_key(|note| (note.start, note.pitch));
    let last_end = placed
        .iter()
        .map(|note| note.0.raw() + note.1)
        .max()
        .unwrap_or(0);
    let clip_end = Ticks(to_ticks(latest).raw().max(last_end)).max(Ticks(clip_start.raw() + 1));
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

    /// What the take writes as one part, the way a take without a cycle is written.
    fn notes_of(
        take: &NoteTake,
        end: f64,
        to_ticks: impl Fn(f64) -> Ticks,
    ) -> Option<(Ticks, Ticks, Vec<Note>)> {
        let parts = take_parts(take, end, 0, NoteRecordMode::Takes);
        let part = parts.first()?;
        written_notes(&part.notes, part.start, part.end, None, to_ticks)
    }

    fn take(start: f64, played: &[(u8, f64, f64)]) -> NoteTake {
        NoteTake {
            track: TrackId(1),
            start,
            cycle: None,
            pass: 0,
            last_at: start,
            began: None,
            held: Vec::new(),
            bend: Vec::new(),
            controllers: Vec::new(),
            played: played
                .iter()
                .map(|&(pitch, on, off)| (pitch, 0.7, on, off, 0))
                .collect(),
        }
    }

    #[test]
    fn notes_land_where_they_were_played_relative_to_the_take() {
        let take = take(1_000.0, &[(60, 1_200.0, 1_500.0), (64, 1_500.0, 1_900.0)]);
        let (start, end, notes) = notes_of(&take, 2_000.0, one_to_one).unwrap();
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
        let (start, _, notes) = notes_of(&take, 1_500.0, one_to_one).unwrap();
        assert_eq!(start, Ticks(800));
        assert_eq!(notes[0].start, Ticks(0));
        assert_eq!(notes[1].start, Ticks(200));
    }

    #[test]
    fn nothing_before_the_song_is_written() {
        let take = take(100.0, &[(60, -300.0, -50.0), (62, -20.0, 400.0)]);
        let (start, _, notes) = notes_of(&take, 500.0, one_to_one).unwrap();
        assert_eq!(start, Ticks(0));
        assert_eq!(
            notes.len(),
            1,
            "the note that ended before the song is gone"
        );
        assert_eq!((notes[0].start, notes[0].length), (Ticks(0), Ticks(400)));
    }

    /// A take round a cycle of frames 1 000 to 2 000, started at 1 000.
    fn cycling() -> NoteTake {
        let mut take = take(1_000.0, &[]);
        take.cycle = Some((1_000.0, 2_000.0));
        take
    }

    fn strike(take: &mut NoteTake, pitch: u8, on: f64, off: f64) {
        let (on, pass) = take.stamp(on);
        take.held.push((pitch, 0.7, on, pass));
        let (off, pass) = take.stamp(off);
        close_held(take, pitch, off, pass);
    }

    #[test]
    fn each_pass_of_a_cycle_is_a_take_and_only_the_last_plays() {
        let mut take = cycling();
        strike(&mut take, 60, 1_600.0, 1_800.0);
        // The wrap: the heard frame jumps back to the loop start.
        strike(&mut take, 62, 1_100.0, 1_700.0);
        strike(&mut take, 64, 1_150.0, 1_400.0);
        let parts = take_parts(&take, 1_500.0, 2, NoteRecordMode::Takes);
        let summary: Vec<(usize, bool, Vec<u8>)> = parts
            .iter()
            .map(|part| {
                (
                    part.pass,
                    part.muted,
                    part.notes.iter().map(|n| n.0).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, true, vec![60]),
                (1, true, vec![62]),
                (2, false, vec![64])
            ]
        );
        assert_eq!((parts[1].start, parts[1].end), (1_000.0, 2_000.0));
        assert_eq!(
            parts[2].end, 1_500.0,
            "the last pass ends where the take did"
        );
    }

    #[test]
    fn merging_lays_every_pass_into_one_clip_and_replacing_keeps_the_last() {
        let mut take = cycling();
        strike(&mut take, 60, 1_600.0, 1_800.0);
        strike(&mut take, 62, 1_150.0, 1_350.0);
        let merged = take_parts(&take, 1_900.0, 1, NoteRecordMode::Merge);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].notes.len(), 2);
        assert_eq!((merged[0].start, merged[0].end), (1_000.0, 2_000.0));
        let replaced = take_parts(&take, 1_900.0, 1, NoteRecordMode::Replace);
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].notes[0].0, 62);
    }

    #[test]
    fn an_overshoot_past_the_loop_end_is_folded_back_to_the_start() {
        let mut take = cycling();
        assert_eq!(take.stamp(1_900.0), (1_900.0, 0));
        assert_eq!(
            take.stamp(2_010.0),
            (1_010.0, 1),
            "heard a block early, at the start"
        );
        assert_eq!(
            take.stamp(1_020.0),
            (1_020.0, 1),
            "and the real wrap is the same pass"
        );
    }

    #[test]
    fn the_clock_finds_a_pass_the_heard_frame_alone_would_miss() {
        let mut take = cycling();
        // Early in the first pass, then early in the second: no jump back between them.
        assert_eq!(take.stamp_reckoned(1_100.0, Some(1_100.0)), (1_100.0, 0));
        assert_eq!(take.stamp_reckoned(1_200.0, Some(2_190.0)), (1_200.0, 1));
        // Late in the third, read with the clock a little ahead of the audio.
        assert_eq!(take.stamp_reckoned(1_950.0, Some(3_990.0)), (1_950.0, 2));
        // And just after the wrap into the fourth, with the clock a little behind.
        assert_eq!(take.stamp_reckoned(1_010.0, Some(3_995.0)), (1_010.0, 3));
    }

    #[test]
    fn a_note_held_across_the_wrap_ends_at_the_loop_end() {
        let mut take = cycling();
        strike(&mut take, 60, 1_800.0, 1_100.0);
        assert_eq!(take.played[0].3, 2_000.0);
    }

    #[test]
    fn merge_writes_into_the_clip_already_there_and_choosing_a_take_mutes_the_rest() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_note_record_mode(NoteRecordMode::Merge);
        session.start_recording(Some(lead)).unwrap();
        session.note_take_struck_at(lead, 60, 0.7, 0.0);
        session.note_take_released_at(lead, 60, 100.0);
        let first = session.stop_recording().unwrap().takes[0].clip.unwrap();
        session.start_recording(Some(lead)).unwrap();
        session.note_take_struck_at(lead, 64, 0.7, 0.0);
        session.note_take_released_at(lead, 64, 100.0);
        let second = session.stop_recording().unwrap().takes[0].clip.unwrap();
        assert_eq!(first, second, "merged into the same clip");
        let (_, merged) = session.project().midi_clip(first).unwrap();
        assert_eq!(merged.notes.len(), 2);

        session.set_note_record_mode(NoteRecordMode::Takes);
        session.start_recording(Some(lead)).unwrap();
        session.note_take_struck_at(lead, 67, 0.7, 0.0);
        session.note_take_released_at(lead, 67, 100.0);
        let third = session.stop_recording().unwrap().takes[0].clip.unwrap();
        assert_ne!(third, first);
        assert_eq!(session.takes_over(third), vec![first]);
        session.choose_take(first).unwrap();
        assert!(!session.project().midi_clip(first).unwrap().1.muted);
        assert!(session.project().midi_clip(third).unwrap().1.muted);
        assert_eq!(session.next_take(first).unwrap(), third);
        assert_eq!(
            session.next_take(third).unwrap(),
            first,
            "round to the first again"
        );
    }

    #[test]
    fn replace_clears_what_was_under_the_take() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.start_recording(Some(lead)).unwrap();
        session.note_take_struck_at(lead, 60, 0.7, 0.0);
        session.note_take_released_at(lead, 60, 100.0);
        let first = session.stop_recording().unwrap().takes[0].clip.unwrap();
        session.set_note_record_mode(NoteRecordMode::Replace);
        session.start_recording(Some(lead)).unwrap();
        session.note_take_struck_at(lead, 64, 0.7, 0.0);
        session.note_take_released_at(lead, 64, 100.0);
        session.stop_recording().unwrap();
        assert!(
            session
                .project()
                .midi_clip(first)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
    }

    #[test]
    fn a_wheel_is_written_from_where_it_stood_when_the_clip_began() {
        let moves = [
            (500.0, 0.5),
            (800.0, 1.0),
            (1_200.0, 1.0),
            (1_500.0, 0.0),
            (3_000.0, 2.0),
        ];
        let curve = written_curve(&moves, Ticks(1_000), Ticks(2_000), one_to_one);
        let points: Vec<(i64, f32)> = curve.iter().map(|p| (p.at.raw(), p.value)).collect();
        assert_eq!(
            points,
            vec![(0, 1.0), (500, 0.0)],
            "the last move before the clip, then each change, none after it"
        );
    }

    #[test]
    fn a_wheel_left_at_rest_writes_no_curve() {
        let moves = [(100.0, 0.0), (200.0, 0.0)];
        assert!(written_curve(&moves, Ticks(0), Ticks(1_000), one_to_one).is_empty());
    }

    #[test]
    fn quantising_pulls_a_note_towards_its_nearest_line_by_the_strength() {
        let full = RecordQuantize {
            grid: Ticks(240),
            strength: 1.0,
        };
        assert_eq!(record_quantized(Ticks(250), full), Ticks(240));
        assert_eq!(record_quantized(Ticks(470), full), Ticks(480));
        let half = RecordQuantize {
            strength: 0.5,
            ..full
        };
        assert_eq!(record_quantized(Ticks(260), half), Ticks(250));
        let none = RecordQuantize {
            strength: 0.0,
            ..full
        };
        assert_eq!(record_quantized(Ticks(260), none), Ticks(260));
    }

    #[test]
    fn a_quantised_take_keeps_how_long_each_note_was_held() {
        let quantize = Some(RecordQuantize {
            grid: Ticks(100),
            strength: 1.0,
        });
        let played = [(60, 0.7, 1_030.0, 1_110.0), (62, 0.7, 1_190.0, 1_260.0)];
        let (start, _, notes) =
            written_notes(&played, 1_000.0, 1_300.0, quantize, one_to_one).unwrap();
        assert_eq!(start, Ticks(1_000));
        let got: Vec<(i64, i64)> = notes
            .iter()
            .map(|note| (note.start.raw(), note.length.raw()))
            .collect();
        assert_eq!(got, vec![(0, 80), (200, 70)]);
    }

    #[test]
    fn a_note_pulled_before_the_take_began_moves_the_clip_with_it() {
        let quantize = Some(RecordQuantize {
            grid: Ticks(100),
            strength: 1.0,
        });
        let played = [(60, 0.7, 1_020.0, 1_100.0)];
        let (start, _, notes) =
            written_notes(&played, 1_020.0, 1_200.0, quantize, one_to_one).unwrap();
        assert_eq!(start, Ticks(1_000));
        assert_eq!(notes[0].start, Ticks(0));
    }

    #[test]
    fn a_take_with_nothing_played_writes_no_clip() {
        assert!(notes_of(&take(0.0, &[]), 10_000.0, one_to_one).is_none());
    }

    #[test]
    fn a_tap_shorter_than_a_tick_is_still_a_note() {
        let take = take(0.0, &[(60, 500.2, 500.3)]);
        let (_, _, notes) = notes_of(&take, 600.0, one_to_one).unwrap();
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
