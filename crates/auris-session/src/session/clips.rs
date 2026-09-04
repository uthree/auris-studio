//! What sits on a track: adding, moving, trimming, splitting and deleting a clip.
//!
//! Both kinds together, because almost every command here answers for a block of notes and for a
//! window into a recorded file in the same body, and a boundary between the two would leave each
//! side reaching back over it. Where they genuinely differ — a gain and a pair of fades belong to
//! audio, a rewrite belongs to a clip with a recipe — the difference is a branch rather than a
//! second command.
//!
//! The curves are here for the same reason [`Session::set_curve_point`] takes a
//! [`ClipCurve`](auris_core::project::ClipCurve): a bend and a controller are the same shape drawn
//! across the same clip, and a copy of those four commands per kind of curve would be that many
//! chances for a wheel to behave differently from a bend for no reason anybody could see.
//!
//! A lane that has been emptied is *removed* rather than left holding nothing. A clip carries the
//! controllers somebody wrote on, and one carrying an empty controller 11 would save that into the
//! file, offer it in a menu, and hand it to a MIDI export as a lane that says nothing.
//!
//! A clip that writes itself is `generated`. It is a field on a clip rather than a kind of clip,
//! so the commands here are the ones that act on it too — which is why resizing and trimming call
//! [`Session::phrase`] back across that boundary.

use auris_core::project::{ClipCurve, CurvePoint, FadeCurve};
use auris_core::time::{Seconds, TempoMap, Ticks};
use auris_core::{ClipId, TrackId};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// Writes a point on one of a clip's curves, replacing whatever was at that instant.
    ///
    /// Kept in time order here rather than wherever a curve is read, because everything that
    /// reads one — the renderer, the MIDI writer, the roll — assumes it: a point out of order
    /// would draw a line backwards and schedule a jump.
    ///
    /// `which` is the only thing that tells the bend from the modulation anywhere in this crate.
    /// They are the same shape and obey the same rules, and two copies of these four commands
    /// would be two chances for the wheel to behave differently from the bend for no reason
    /// anybody could see.
    pub fn set_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        at: Ticks,
        value: f32,
    ) -> bool {
        if !value.is_finite() {
            return false;
        }
        let at = at.max_zero();
        let (low, high) = which.range();
        let value = value.clamp(low, high);
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return false;
        };
        let at = at.min(target.length);
        if target
            .curve(which)
            .iter()
            .any(|point| point.at == at && point.value == value)
        {
            return false;
        }
        self.record(Edit::write_curve(which, clip));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            let points = target.curve_mut(which);
            points.retain(|point| point.at != at);
            points.push(CurvePoint { at, value });
            points.sort_by_key(|point| point.at);
        }
        self.invalidate_graph();
        true
    }

    /// Moves a point along a curve, taking a new value with it.
    ///
    /// Returns where it landed, which is not always where it was asked to go: dropped onto another
    /// point it replaces that one, since one instant cannot hold two values. A drag wants
    /// [`Self::begin_transaction`] around the whole gesture, the way every other drag does.
    pub fn move_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Option<Ticks> {
        if !value.is_finite() {
            return None;
        }
        let (low, high) = which.range();
        let value = value.clamp(low, high);
        let length = self.project.midi_clip(clip)?.1.length;
        let to = to.max_zero().min(length);
        let held = self
            .project
            .midi_clip(clip)?
            .1
            .curve(which)
            .iter()
            .find(|point| point.at == from)
            .copied()?;
        if held.at == to && held.value == value {
            return Some(to);
        }
        self.record(Edit::write_curve(which, clip));
        let target = self.project.midi_clip_mut(clip)?;
        let points = target.curve_mut(which);
        points.retain(|point| point.at != from && point.at != to);
        points.push(CurvePoint { at: to, value });
        points.sort_by_key(|point| point.at);
        self.invalidate_graph();
        Some(to)
    }

    /// Takes one point off a curve.
    pub fn remove_curve_point(&mut self, clip: ClipId, which: ClipCurve, at: Ticks) -> bool {
        if !self
            .project
            .midi_clip(clip)
            .is_some_and(|target| target.1.curve(which).iter().any(|point| point.at == at))
        {
            return false;
        }
        self.record(Edit::erase_curve(which));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.curve_mut(which).retain(|point| point.at != at);
            target.forget_empty_curves();
        }
        self.invalidate_graph();
        true
    }

    /// Straightens a clip out, removing one of its curves entirely.
    pub fn clear_curve(&mut self, clip: ClipId, which: ClipCurve) -> bool {
        if !self
            .project
            .midi_clip(clip)
            .is_some_and(|target| !target.1.curve(which).is_empty())
        {
            return false;
        }
        self.record(Edit::erase_curve(which));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.curve_mut(which).clear();
            target.forget_empty_curves();
        }
        self.invalidate_graph();
        true
    }

    /// Adds an empty MIDI clip to a track that holds notes — an instrument track or a singer
    /// track.
    pub fn add_midi_clip(
        &mut self,
        track: TrackId,
        name: impl Into<String>,
        start: Ticks,
        length: Ticks,
    ) -> Result<ClipId, SessionError> {
        let index = self.require_track(track)?;
        if !self.project.tracks[index].kind.holds_notes() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                // The track's own word for itself, rather than "an audio track" — which was true
                // of the only other kind there used to be, and is a lie about a bus.
                actual: self.project.tracks[index].kind.label(),
                expected: "a track that holds notes",
            });
        }
        self.record(Edit::AddClip);
        let id = self
            .project
            .add_midi_clip(track, name, start.max_zero(), Ticks(length.raw().max(1)))
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.invalidate_graph();
        Ok(id)
    }

    /// Removes a clip of either kind.
    pub fn remove_clip(&mut self, clip: ClipId) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() && !self.audio_clip_exists(clip) {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::DeleteClip);
        self.project.remove_clip(clip);
        self.invalidate_graph();
        Ok(())
    }

    /// Which track a clip sits on.
    pub fn track_of_clip(&self, clip: ClipId) -> Option<TrackId> {
        self.project.track_of_clip(clip)
    }

    /// `true` when `clip` could be moved onto `track`.
    ///
    /// Asked before a move rather than discovered during one, so a pointer drag can refuse a
    /// lane it cannot land on instead of dropping half a selection onto it.
    pub fn clip_fits_track(&self, clip: ClipId, track: TrackId) -> bool {
        let Some(source) = self.project.track_of_clip(clip) else {
            return false;
        };
        // "Holds notes" against "holds audio", with a bus answering neither: asking
        // `is_instrument` here counted an audio track and a bus as the same kind, and a singer
        // track as neither of the kinds its own clips are.
        let kind_of = |id: TrackId| -> Option<bool> {
            let track = self.project.track(id)?;
            match (track.kind.holds_notes(), track.kind.as_audio().is_some()) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            }
        };
        match (kind_of(source), kind_of(track)) {
            (Some(from), Some(to)) => from == to,
            _ => false,
        }
    }

    /// Moves clips onto another track, keeping their positions.
    ///
    /// Every clip or none: a selection dragged across lanes is one gesture, and landing half of
    /// it on the new track and leaving the rest behind is not what dropping it meant. The whole
    /// move is refused when any clip does not belong on its destination.
    pub fn move_clips_to_track(&mut self, clips: &[(ClipId, TrackId)]) -> Result<(), SessionError> {
        if clips.is_empty() {
            return Ok(());
        }
        for (clip, track) in clips {
            self.require_clip(*clip)?;
            self.require_track(*track)?;
            if !self.clip_fits_track(*clip, *track) {
                return Err(SessionError::UnknownTrack(track.0));
            }
        }
        // Nothing to record when every clip is already where it is being sent, which is what a
        // pointer drag asks for on most of its moves.
        if clips
            .iter()
            .all(|(clip, track)| self.project.track_of_clip(*clip) == Some(*track))
        {
            return Ok(());
        }
        self.record(Edit::MoveClip);
        for (clip, track) in clips {
            self.project.move_clip_to_track(*clip, *track);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Copies a clip onto its own track, immediately after the original.
    pub fn duplicate_clip(&mut self, clip: ClipId) -> Result<ClipId, SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::DuplicateClip);
        let copy = self
            .project
            .duplicate_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        self.invalidate_graph();
        Ok(copy)
    }

    /// Divides a clip in two at a timeline position, returning the right-hand piece.
    pub fn split_clip(&mut self, clip: ClipId, at: Ticks) -> Result<ClipId, SessionError> {
        self.require_clip(clip)?;
        // The split is attempted before any history is recorded. A position outside the clip
        // leaves the document untouched, and an undo step that undoes nothing visible is worse
        // than no step at all.
        let before = self.project.clone();
        let Some(right) = self.project.split_clip(clip, at) else {
            return Err(SessionError::CannotSplit(clip.0));
        };
        if self.transaction.is_none() {
            self.history.push(Edit::SplitClip, &before);
        }
        // The halves of a generated clip keep its recipe, and their notes are the machine's own
        // arithmetic over text the recipe vouched for — so the digest follows, as it does for a
        // resize. Without this both halves would read as edited by hand the moment the knife
        // lifted.
        for half in [clip, right] {
            if let Some(midi) = self.project.midi_clip_mut(half)
                && midi.recipe.is_some()
            {
                let digest = auris_core::notes_digest(&midi.notes);
                if let Some(recipe) = &mut midi.recipe {
                    recipe.text_digest = digest;
                }
            }
        }
        // What `record` does for every other command, and this is the one that pushes its own
        // step instead of going through it: a split has to break a run of coalescing repeats, or a
        // tempo nudge either side of it folds into one step and undoing that step silently takes
        // the split with it.
        self.last_record = None;
        self.dirty = true;
        self.invalidate_graph();
        Ok(right)
    }

    /// Renames a clip of either kind.
    pub fn rename_clip(
        &mut self,
        clip: ClipId,
        name: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::RenameClip);
        let name = name.into();
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.name = name;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.name = name;
        }
        Ok(())
    }

    /// Silences or unsilences a single clip.
    pub fn set_clip_muted(&mut self, clip: ClipId, muted: bool) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::MuteClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.muted = muted;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.muted = muted;
        }
        self.invalidate_graph();
        Ok(())
    }

    fn require_clip(&self, clip: ClipId) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_some() || self.audio_clip_exists(clip) {
            Ok(())
        } else {
            Err(SessionError::UnknownClip(clip.0))
        }
    }

    /// Removes several clips as one edit.
    ///
    /// Ids that do not exist are ignored, so a stale selection cannot fail the whole delete.
    pub fn remove_clips(&mut self, clips: &[ClipId]) -> Result<(), SessionError> {
        self.remove_clips_as(Edit::DeleteClip, clips)
    }

    /// [`Self::remove_clips`] under a named edit, so a cut undoes as a cut.
    ///
    /// The two are the same removal and differ only in what Undo is called afterwards, which is
    /// worth a parameter and not worth a second copy of the loop.
    pub(super) fn remove_clips_as(
        &mut self,
        edit: Edit,
        clips: &[ClipId],
    ) -> Result<(), SessionError> {
        let present: Vec<ClipId> = clips
            .iter()
            .copied()
            .filter(|clip| self.require_clip(*clip).is_ok())
            .collect();
        if present.is_empty() {
            return Ok(());
        }
        self.record(edit);
        for clip in present {
            self.project.remove_clip(clip);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Moves several clips by one delta, from positions captured before the gesture began.
    ///
    /// The delta is clamped so that the earliest clip lands on zero rather than each clip being
    /// clamped separately — that would pile the leading clips on top of each other and quietly
    /// destroy the spacing the user is dragging.
    ///
    /// Repeats fold, for the reason [`Session::move_notes`] gives: a held arrow key is one
    /// gesture arriving as thirty calls, and a drag is unaffected either way.
    pub fn move_clips(&mut self, origins: &[(ClipId, Ticks)], delta: Ticks) {
        // Only clips that still exist: a selection can outlive an undo, and a gesture over
        // nothing must not record a step over nothing.
        let present: Vec<(ClipId, Ticks)> = origins
            .iter()
            .copied()
            .filter(|(clip, _)| self.require_clip(*clip).is_ok())
            .collect();
        let Some(earliest) = present.iter().map(|(_, start)| *start).min() else {
            return;
        };
        let delta = delta.max(-earliest);
        self.record_repeating(Edit::MoveClip);
        for (clip, start) in present {
            let start = (start + delta).max_zero();
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.start = start;
            } else if let Some(audio) = self.project.audio_clip_mut(clip) {
                audio.start = start;
                audio.tempo_anchor = None;
            }
        }
        self.invalidate_graph();
    }

    /// Moves a clip of either kind to a new start position.
    ///
    /// A moved audio clip forgets any tempo it was anchored to by an earlier split or trim: asking
    /// for it somewhere else is asking for it under whatever tempo is there, which is the whole of
    /// what following the tempo means. See [`AudioClip::tempo_anchor`](auris_core::AudioClip::tempo_anchor).
    pub fn move_clip(&mut self, clip: ClipId, start: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::MoveClip);
        let start = start.max_zero();
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.start = start;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.start = start;
            audio.tempo_anchor = None;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags a clip's end to `end`.
    ///
    /// The three cases answer differently, because a length means a different thing to each.
    ///
    /// A **generated** clip is its recipe rather than its notes: the notes were written to fill a
    /// length, so a new length gets them written again. Dragged out it fills the bars it gained
    /// instead of trailing silence; dragged in it stops where it stops instead of keeping notes
    /// hanging past its own end. Nothing is lost by it, because the recipe still says what the
    /// clip is and dragging back out writes the material back. Dragged shorter than a bar it has
    /// no bars to write and comes out empty, which is the honest reading of "this part, this
    /// long".
    ///
    /// A clip somebody **played** keeps every note exactly where it is. Its notes are derived
    /// from nothing, so there is nothing to derive them from again, and inventing or discarding
    /// one would be editing work the resize was not aimed at.
    ///
    /// An **audio** clip is a trim, and a trim stops where the material does.
    pub fn resize_clip(&mut self, clip: ClipId, end: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        let grid = self.project.grid;

        if let Some((start, recipe)) = self
            .project
            .midi_clip(clip)
            .map(|(_, midi)| (midi.start, midi.recipe.clone()))
        {
            let length = (end - start).max(grid);
            // Written before anything is recorded, so the length and the notes land in the one
            // undo step the drag opened rather than in two.
            let notes = recipe
                .as_ref()
                .map(|recipe| self.phrase(start, length, recipe));
            self.record(Edit::ResizeClip);
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.length = length;
                // The length is now the user's, so nothing grows it back. A clip dragged shorter
                // to hide a tail used to reappear at full length on the next note edit.
                midi.length_is_explicit = true;
                if let Some(notes) = notes {
                    midi.notes = notes;
                    // The composer wrote this text, so the recipe's digest follows it — a
                    // resize must not read as a hand edit.
                    if let Some(recipe) = &mut midi.recipe {
                        recipe.text_digest = auris_core::notes_digest(&midi.notes);
                    }
                }
            }
            self.invalidate_graph();
            return Ok(());
        }

        // An audio clip's length lives in source frames, so the dragged tick has to go back
        // through the tempo map rather than being stored as ticks.
        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        let Some(audio) = self.project.audio_clip(clip) else {
            return Ok(());
        };
        // What the source has left past the clip's own offset into it. Unbounded, the edge
        // dragged into a stretch of silence that the clip drew and saved with its waveform
        // stopping part way — and that the renderer clamped on the way to the speakers anyway,
        // so the picture and the sound disagreed.
        let available = self
            .project
            .audio_sources
            .get(&audio.source)
            .map_or(u64::MAX, |source| {
                source.frame_count.saturating_sub(audio.offset_frames)
            });
        let start_seconds = tempo.ticks_to_seconds(audio.start).0;
        let end_seconds = tempo.ticks_to_seconds(end).0;
        // Through the stretch as well as the tempo map: what is stored is a length of *material*,
        // and a clip playing at half speed covers a second of timeline with half a second of it.
        // Without this, dragging the edge of a following clip put it at twice the distance the
        // pointer had travelled.
        let stretch = audio.stretch_in(&tempo);
        let asked = ((end_seconds - start_seconds).max(0.0) * sample_rate / stretch) as u64;
        let length = asked.clamp(1, available.max(1));
        if length == audio.length_frames {
            // A drag that has run out of material still moves the pointer, and every frame of it
            // arrives here saying the same thing. Not an edit.
            return Ok(());
        }
        self.record(Edit::ResizeClip);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.length_frames = length;
            // The fades keep fitting inside the clip as it shrinks, under the same rule
            // `set_clip_fades` writes them by: the fade-in keeps its place and the fade-out
            // takes what is left.
            audio.fade_in_frames = audio.fade_in_frames.min(audio.length_frames);
            audio.fade_out_frames = audio
                .fade_out_frames
                .min(audio.length_frames - audio.fade_in_frames);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags a clip's *start* to `start`, keeping its end where it is.
    ///
    /// The other half of [`Self::resize_clip`], and it answers the three cases the same way. A
    /// generated clip is written again over the stretch it now covers. An audio clip walks its
    /// offset into the source along with its start, which is what makes this a trim rather than
    /// a move: the material under the clip stays where it sounds, and dragging the edge back out
    /// uncovers what was hidden rather than repeating what is left. A played clip's notes are
    /// rebased onto the new start, keeping the sounding half of anything the trim runs through —
    /// the rule a split already follows.
    ///
    /// Both ends are bounded by what there is. An audio clip's front stops at the first frame of
    /// its source, and neither kind may be dragged past its own end. A played or generated clip
    /// that is already shorter than the editing grid has nothing left to give and refuses to be
    /// shortened from the front at all — it can still be dragged the other way, which lengthens
    /// it.
    pub fn trim_clip_start(&mut self, clip: ClipId, start: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        let grid = self.project.grid;

        if let Some((was, length, recipe)) = self
            .project
            .midi_clip(clip)
            .map(|(_, midi)| (midi.start, midi.length, midi.recipe.clone()))
        {
            // Never past its own end: the clip keeps at least a grid division, which is the same
            // floor the other edge stops at. A clip that is *already* shorter than a division —
            // a piece of a split, anything drawn at a finer grid than the one now set — has no
            // room under that floor, and `was + length - grid` falls behind `was`. Clamped to
            // `was` it simply refuses to be shortened from the front, instead of being dragged
            // leftwards into a lengthening nobody asked for and, in the first bar, a start
            // before zero. Dragging the other way still lengthens it: it is only the shortening
            // that has nowhere to go.
            let now = start.max_zero().min((was + length - grid).max(was));
            let by = now - was;
            if by == Ticks::ZERO {
                return Ok(());
            }
            let length = length - by;
            let notes = match &recipe {
                Some(recipe) => self.phrase(now, length, recipe),
                None => self
                    .project
                    .midi_clip(clip)
                    .map(|(_, midi)| auris_core::notes_trimmed_from_front(&midi.notes, by))
                    .unwrap_or_default(),
            };
            self.record(Edit::ResizeClip);
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.start = now;
                midi.length = length;
                midi.length_is_explicit = true;
                midi.notes = notes;
                midi.bend.retain_mut(|point| {
                    if point.at < by {
                        false
                    } else {
                        point.at -= by;
                        true
                    }
                });
                for points in midi.controllers.values_mut() {
                    points.retain_mut(|point| {
                        if point.at < by {
                            false
                        } else {
                            point.at -= by;
                            true
                        }
                    });
                }
                // The same digest rule as the other edge: text the composer wrote is text the
                // recipe vouches for, so trimming a generated clip is not a hand edit.
                if let Some(recipe) = &mut midi.recipe {
                    recipe.text_digest = auris_core::notes_digest(&midi.notes);
                }
            }
            self.invalidate_graph();
            return Ok(());
        }

        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        let Some(audio) = self.project.audio_clip(clip) else {
            return Ok(());
        };
        let (was, offset, length) = (audio.start, audio.offset_frames, audio.length_frames);
        // A clip with no frames has no edge to move, and the forward bound below would come out
        // at -1 — behind the backward bound, which is what `Ord::clamp` asserts against and
        // aborts the process over, in release as much as in debug. The importer refuses to make
        // one of these now, but a project written before it did, or an asset that was replaced
        // on disk with an empty file, can still put one here.
        if length == 0 {
            return Ok(());
        }
        let was_seconds = tempo.ticks_to_seconds(was).0;
        // Every figure below is in *source* frames, which is what the offset and the trim are
        // counted in, so the distance the pointer travelled goes through the stretch on the way
        // in and the distance the start moves goes back through it on the way out.
        let stretch = audio.stretch_in(&tempo);
        let asked = ((tempo.ticks_to_seconds(start.max_zero()).0 - was_seconds) * sample_rate
            / stretch)
            .round() as i64;
        // How far back the edge can go: to the source's first frame, or to the start of the
        // timeline, whichever it meets first. The second bound matters for a clip that was
        // trimmed and then moved left — its window still has material behind it, but there is
        // nowhere on the timeline to put it, and clamping the *tick* alone would leave the start
        // at bar one while the window kept walking and the far end slid right.
        let head_room = (was_seconds * sample_rate / stretch).round() as i64;
        let back = (offset as i64).min(head_room.max(0));
        // Forward, to one frame short of the clip's own end. The *clamped* delta is what moves the
        // start, so an edge that has run out of material stops instead of sliding on with the
        // pointer and leaving the sound behind.
        let by = asked.clamp(-back, length as i64 - 1);
        if by == 0 {
            return Ok(());
        }
        let now = tempo.seconds_to_ticks(Seconds(
            was_seconds + by as f64 * stretch / sample_rate.max(1.0),
        ));
        self.record(Edit::ResizeClip);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            // Pinned before the start moves, for the same reason a split pins the half it moves:
            // hiding the front of a take is not a request to play the rest of it at another speed,
            // and dragging the edge past a tempo change would otherwise do exactly that.
            audio.tempo_anchor = Some(audio.anchored_at());
            audio.start = now.max_zero();
            audio.offset_frames = (offset as i64 + by) as u64;
            audio.length_frames = (length as i64 - by) as u64;
            audio.fade_in_frames = audio.fade_in_frames.min(audio.length_frames);
            audio.fade_out_frames = audio
                .fade_out_frames
                .min(audio.length_frames - audio.fade_in_frames);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Sets an audio clip's own gain, in decibels.
    ///
    /// Clip gain is the clip's, not the track's: it travels with the clip when it moves, and it
    /// is applied before the track's effect chain, which is what makes it the tool for evening
    /// out a loud take against its neighbours. Clamped to −60…+24 dB; a non-finite value is
    /// refused outright.
    pub fn set_clip_gain(&mut self, clip: ClipId, gain_db: f32) -> Result<(), SessionError> {
        if !gain_db.is_finite() {
            return Err(SessionError::NotFinite(f64::from(gain_db)));
        }
        let gain_db = gain_db.clamp(-60.0, 24.0);
        if self.require_audio_clip(clip)?.gain_db == gain_db {
            return Ok(());
        }
        self.record(Edit::SetClipGain);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.gain_db = gain_db;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Tells a clip what tempo its recording was made at, or that nobody knows.
    ///
    /// Half of following the tempo: a clip cannot be made to fit the bars until it is known what
    /// bars it was played in. A take recorded here is stamped by the recorder, and a loop that
    /// came from somewhere else is typed in by hand.
    ///
    /// Forgetting it (`None`) also stops the clip following, because a clip that followed a tempo
    /// it no longer knows would be stretched by nothing at all — the switch would be on and the
    /// audio would not move, which is a control that lies.
    pub fn set_clip_source_bpm(
        &mut self,
        clip: ClipId,
        bpm: Option<f64>,
    ) -> Result<(), SessionError> {
        let bpm = match bpm {
            Some(bpm) if !bpm.is_finite() => return Err(SessionError::NotFinite(bpm)),
            Some(bpm) => Some(bpm.clamp(TempoMap::MIN_BPM, TempoMap::MAX_BPM)),
            None => None,
        };
        if self.require_audio_clip(clip)?.source_bpm == bpm {
            return Ok(());
        }
        self.record(Edit::SetClipTempo);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.source_bpm = bpm;
            audio.follows_tempo = audio.follows_tempo && bpm.is_some();
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Stretches a clip so that it keeps its place in the bars, or stops.
    ///
    /// Switching it on for a clip that has never been told its own tempo **assumes the tempo it
    /// sits at**. That is right far more often than it is wrong — material is nearly always
    /// dropped into the piece it was made for, and a take recorded here was recorded at exactly
    /// that — and where it is wrong the number is on screen, in the same menu, to be corrected.
    /// The alternative was a switch that silently did nothing until a second command was found.
    pub fn set_clip_follows_tempo(
        &mut self,
        clip: ClipId,
        follows: bool,
    ) -> Result<(), SessionError> {
        let audio = self.require_audio_clip(clip)?;
        if audio.follows_tempo == follows {
            return Ok(());
        }
        let assumed = match follows && audio.source_bpm.is_none() {
            true => Some(self.project.tempo_map.bpm_at(audio.anchored_at())),
            false => None,
        };
        self.record(Edit::SetClipTempo);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.follows_tempo = follows;
            if let Some(bpm) = assumed {
                audio.source_bpm = Some(bpm);
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// What tempo a clip believes it was recorded at.
    pub fn clip_source_bpm(&self, clip: ClipId) -> Option<f64> {
        self.project.audio_clip(clip)?.source_bpm
    }

    /// Whether a clip is stretched to follow the piece's tempo.
    pub fn clip_follows_tempo(&self, clip: ClipId) -> bool {
        self.project
            .audio_clip(clip)
            .is_some_and(|audio| audio.follows_tempo)
    }

    /// How far a clip's audio is stretched where it sits, `1.0` being as it was recorded.
    pub fn clip_stretch(&self, clip: ClipId) -> f64 {
        self.project
            .audio_clip(clip)
            .map_or(1.0, |audio| audio.stretch_in(&self.project.tempo_map))
    }

    /// Sets an audio clip's fades, in frames of its source.
    ///
    /// The fade-in is clamped to the clip and the fade-out to what the fade-in leaves, so the
    /// two can meet but never cross — crossed fades would multiply into a dip no hand drew.
    /// [`auris_core::AudioClip::fade_gain_at`] is the shape, shared by playback and by
    /// whatever a frontend draws.
    pub fn set_clip_fades(
        &mut self,
        clip: ClipId,
        fade_in: u64,
        fade_out: u64,
    ) -> Result<(), SessionError> {
        let current = self.require_audio_clip(clip)?;
        let length = current.length_frames;
        let fade_in = fade_in.min(length);
        let fade_out = fade_out.min(length - fade_in);
        if current.fade_in_frames == fade_in && current.fade_out_frames == fade_out {
            return Ok(());
        }
        self.record(Edit::SetClipFade);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.fade_in_frames = fade_in;
            audio.fade_out_frames = fade_out;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// The clip `clip` overlaps on its own track, and by how much.
    ///
    /// `None` when nothing overlaps it, which is the ordinary case: clips sit end to end. The
    /// nearest one wins where several overlap, because a crossfade is a join and the join is with
    /// the neighbour.
    pub fn crossfade_partner(&self, clip: ClipId) -> Option<(ClipId, Ticks)> {
        let track = self.track_of_clip(clip)?;
        let audio = self.project.track(track)?.kind.as_audio()?;
        let this = audio.clips.iter().find(|other| other.id == clip)?;
        let span = |clip: &auris_core::AudioClip| {
            (
                clip.start,
                clip.start + self.project.audio_clip_sounding_ticks(clip),
            )
        };
        let (start, end) = span(this);
        audio
            .clips
            .iter()
            .filter(|other| other.id != clip)
            .filter_map(|other| {
                let (their_start, their_end) = span(other);
                let overlap = end.min(their_end) - start.max(their_start);
                (overlap > Ticks::ZERO).then_some((other.id, overlap))
            })
            .min_by_key(|(_, overlap)| *overlap)
    }

    /// Crossfades two overlapping audio clips into each other, and says how long the join is.
    ///
    /// The earlier clip fades out across the overlap while the later one fades in across the same
    /// stretch, both on the [`equal-power`](auris_core::project::FadeCurve::EqualPower) curve —
    /// which is what a join wants and a fade from silence does not. Nothing moves: the overlap is
    /// whatever dragging one clip over the other already made, and this shapes it.
    ///
    /// Refused when the two are on different tracks, when either is not audio, and when they do
    /// not overlap. A crossfade over nothing would be two fades of no length, which is what the
    /// clips already have.
    pub fn crossfade_clips(
        &mut self,
        first: ClipId,
        second: ClipId,
    ) -> Result<Ticks, SessionError> {
        self.begin_transaction(Edit::Crossfade);
        match self.shape_join(first, second) {
            Ok(overlap) => {
                self.invalidate_graph();
                self.end_transaction();
                Ok(overlap)
            }
            Err(error) => {
                self.revert_transaction();
                Err(error)
            }
        }
    }

    /// Crossfades every join the named clips have just landed in, and says how many were made.
    ///
    /// What a clip dropped over its neighbour gets. It is deliberately **not** every overlap in
    /// the project — only the clips the caller says have moved — and deliberately not one that
    /// somebody has already shaped: a join is made only where *neither* meeting edge carries a
    /// fade, so a hand-drawn fade is never written over by a drag.
    ///
    /// Called inside whatever transaction the move is already recording, so the join and the move
    /// it came from are one undo step. There is nothing to undo separately: the fade exists
    /// because the clip landed there.
    pub fn crossfade_landings(&mut self, clips: &[ClipId]) -> usize {
        let mut made = 0;
        for clip in clips {
            let Some((partner, _)) = self.crossfade_partner(*clip) else {
                continue;
            };
            if !self.join_is_clear(*clip, partner) {
                continue;
            }
            if self.shape_join(*clip, partner).is_ok() {
                made += 1;
            }
        }
        if made > 0 {
            self.invalidate_graph();
        }
        made
    }

    /// Whether the two edges that would meet in a join are both bare.
    ///
    /// The test a drag has to pass before it shapes anything. A fade somebody drew is a decision
    /// about how that clip ends, and a gesture aimed at *where the clip sits* has no business
    /// rewriting it.
    fn join_is_clear(&self, first: ClipId, second: ClipId) -> bool {
        let (Some(one), Some(two)) = (
            self.project.audio_clip(first),
            self.project.audio_clip(second),
        ) else {
            return false;
        };
        let (early, late) = match one.start <= two.start {
            true => (one, two),
            false => (two, one),
        };
        early.fade_out_frames == 0 && late.fade_in_frames == 0
    }

    /// The whole of a crossfade except the undo step, so a caller already recording one can
    /// borrow it.
    fn shape_join(&mut self, first: ClipId, second: ClipId) -> Result<Ticks, SessionError> {
        if first == second {
            return Err(SessionError::NotOverlapping);
        }
        let track = self.track_of_clip(first);
        if track.is_none() || track != self.track_of_clip(second) {
            return Err(SessionError::NotOverlapping);
        }
        // Both looked up before anything is written, so a pair that cannot be crossfaded leaves
        // the document exactly as it was.
        let one = self.require_audio_clip(first)?.clone();
        let two = self.require_audio_clip(second)?.clone();
        let (early, late) = match one.start <= two.start {
            true => (one, two),
            false => (two, one),
        };
        let early_pass = self.project.audio_clip_length_ticks(&early);
        let late_pass = self.project.audio_clip_length_ticks(&late);
        let early_end = early.start + self.project.audio_clip_sounding_ticks(&early);
        let late_end = late.start + self.project.audio_clip_sounding_ticks(&late);
        // Never longer than one pass of either clip: the fades sit on the clips' own edges, so a
        // join longer than the audio behind it is a fade that could not be drawn.
        let overlap = (early_end.min(late_end) - late.start)
            .min(early_pass)
            .min(late_pass);
        if overlap <= Ticks::ZERO {
            return Err(SessionError::NotOverlapping);
        }

        let out_frames = fade_frames(overlap, early_pass, early.length_frames);
        let in_frames = fade_frames(overlap, late_pass, late.length_frames);
        self.set_clip_fades(early.id, early.fade_in_frames, out_frames)?;
        self.set_clip_fades(late.id, in_frames, late.fade_out_frames)?;
        if let Some(audio) = self.project.audio_clip_mut(early.id) {
            audio.fade_out_curve = FadeCurve::EqualPower;
        }
        if let Some(audio) = self.project.audio_clip_mut(late.id) {
            audio.fade_in_curve = FadeCurve::EqualPower;
        }
        Ok(overlap)
    }

    /// Sets the shape of an audio clip's fade-in.
    ///
    /// The shape a crossfade sets for itself, offered by hand for the joins somebody made another
    /// way — a fade drawn by dragging, or a clip trimmed back until it met its neighbour. The
    /// shapes and what each is for are [`FadeCurve`].
    pub fn set_fade_in_curve(
        &mut self,
        clip: ClipId,
        curve: FadeCurve,
    ) -> Result<(), SessionError> {
        if self.require_audio_clip(clip)?.fade_in_curve == curve {
            return Ok(());
        }
        self.record(Edit::SetClipFade);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.fade_in_curve = curve;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Sets the shape of an audio clip's fade-out.
    pub fn set_fade_out_curve(
        &mut self,
        clip: ClipId,
        curve: FadeCurve,
    ) -> Result<(), SessionError> {
        if self.require_audio_clip(clip)?.fade_out_curve == curve {
            return Ok(());
        }
        self.record(Edit::SetClipFade);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.fade_out_curve = curve;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// The shapes of an audio clip's two fades, in and out.
    pub fn fade_curves(&self, clip: ClipId) -> Option<(FadeCurve, FadeCurve)> {
        let audio = self.project.audio_clip(clip)?;
        Some((audio.fade_in_curve, audio.fade_out_curve))
    }

    /// The audio clip called `clip`, or the error saying what was addressed instead.
    fn require_audio_clip(&self, clip: ClipId) -> Result<&auris_core::AudioClip, SessionError> {
        let found = self
            .project
            .tracks
            .iter()
            .find_map(|track| track.kind.as_audio()?.clips.iter().find(|c| c.id == clip));
        match found {
            Some(audio) => Ok(audio),
            None if self.project.midi_clip(clip).is_some() => Err(SessionError::NotAudio(clip.0)),
            None => Err(SessionError::UnknownClip(clip.0)),
        }
    }

    fn audio_clip_exists(&self, clip: ClipId) -> bool {
        self.project.tracks.iter().any(|track| {
            track
                .kind
                .as_audio()
                .is_some_and(|inner| inner.clips.iter().any(|c| c.id == clip))
        })
    }

    /// Length of an audio clip on the musical timeline, repeats not counted.
    pub fn audio_clip_length_ticks(&self, clip: &auris_core::AudioClip) -> Ticks {
        self.project.audio_clip_length_ticks(clip)
    }

    /// How long one pass of a clip of either kind is, before any repeats.
    pub fn clip_content_length(&self, clip: ClipId) -> Option<Ticks> {
        if let Some((_, midi)) = self.project.midi_clip(clip) {
            return Some(midi.length);
        }
        let audio = self.project.audio_clip(clip)?;
        Some(self.project.audio_clip_length_ticks(audio))
    }

    /// How far a clip of either kind reaches on the timeline, repeats included.
    pub fn clip_sounding_length(&self, clip: ClipId) -> Option<Ticks> {
        if let Some((_, midi)) = self.project.midi_clip(clip) {
            return Some(midi.sounding_length());
        }
        let audio = self.project.audio_clip(clip)?;
        Some(self.project.audio_clip_sounding_ticks(audio))
    }

    /// Where a clip's content repeats out to, measured from its own start.
    ///
    /// [`Ticks::ZERO`] for a clip that does not repeat, and never anything a caller has to
    /// compare against the clip's length itself — ask [`Self::clip_is_looped`] for that.
    pub fn clip_loop_end(&self, clip: ClipId) -> Ticks {
        if let Some((_, midi)) = self.project.midi_clip(clip) {
            return midi.loop_end;
        }
        self.project
            .audio_clip(clip)
            .map(|audio| audio.loop_end)
            .unwrap_or(Ticks::ZERO)
    }

    /// `true` when the clip's content repeats past its own end.
    pub fn clip_is_looped(&self, clip: ClipId) -> bool {
        self.clip_content_length(clip)
            .is_some_and(|content| self.clip_loop_end(clip) > content)
    }

    /// Where the next clip on the same lane begins, if there is one.
    ///
    /// What "next" means is the next *start* rather than the next clip that does not overlap:
    /// clips on a lane may sit on top of one another, and a loop dragged under a neighbour would
    /// be repeating into material already sounding there.
    pub fn next_clip_start(&self, clip: ClipId) -> Option<Ticks> {
        let track = self.project.track_of_clip(clip)?;
        let here = self.clip_start(clip)?;
        let kind = &self.project.track(track)?.kind;
        let starts: Box<dyn Iterator<Item = Ticks> + '_> = if let Some(clips) = kind.note_clips() {
            Box::new(clips.iter().map(|clip| clip.start))
        } else {
            let inner = kind.as_audio()?;
            Box::new(inner.clips.iter().map(|c| c.start))
        };
        starts.filter(|start| *start > here).min()
    }

    /// Where a clip of either kind begins.
    pub fn clip_start(&self, clip: ClipId) -> Option<Ticks> {
        if let Some((_, midi)) = self.project.midi_clip(clip) {
            return Some(midi.start);
        }
        self.project.audio_clip(clip).map(|audio| audio.start)
    }

    /// Sets how far a clip's content repeats past its own end, measured from the clip's start.
    ///
    /// Anything no longer than the clip itself turns looping off, which is what makes dragging
    /// the loop edge back over the clip's own end the way to stop it repeating — the same gesture
    /// that started it, run the other way, rather than a second thing to know about.
    ///
    /// Both kinds of clip, and one command: a repeat is a repeat whether what is being said again
    /// is a bar of notes or a bar of a recording.
    pub fn set_clip_loop(&mut self, clip: ClipId, loop_end: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        let content = self.clip_content_length(clip).unwrap_or(Ticks::ZERO);
        let loop_end = match loop_end > content {
            true => loop_end,
            false => Ticks::ZERO,
        };
        if self.clip_loop_end(clip) == loop_end {
            // A drag that has run back over the clip's own end keeps sending the same answer,
            // and every frame of it arrives here. Not an edit.
            return Ok(());
        }
        self.record(Edit::LoopClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.loop_end = loop_end;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.loop_end = loop_end;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Turns a clip's loop on or off, returning whether it now repeats.
    ///
    /// On, it reaches as far as [`auris_core::default_loop_end`] says — the next clip on the lane,
    /// or one extra pass where there is none. Off, the length is forgotten rather than remembered
    /// for next time: a loop is a length, and half of a stored one showing up again under a clip
    /// that has since been resized would be a length nobody chose.
    pub fn toggle_clip_loop(&mut self, clip: ClipId) -> Result<bool, SessionError> {
        self.require_clip(clip)?;
        if self.clip_is_looped(clip) {
            self.set_clip_loop(clip, Ticks::ZERO)?;
            return Ok(false);
        }
        let start = self.clip_start(clip).unwrap_or(Ticks::ZERO);
        let content = self.clip_content_length(clip).unwrap_or(Ticks::ZERO);
        let next = self.next_clip_start(clip);
        self.set_clip_loop(clip, auris_core::default_loop_end(start, content, next))?;
        Ok(true)
    }
}

/// How many source frames of a clip cover `span` of the timeline.
///
/// Worked out as a fraction of the clip rather than through the sample rate, and deliberately: a
/// clip's fades are counted in the frames of the file it came from, which is not the rate the
/// project runs at, and a clip that follows the tempo plays more of them or fewer than it holds.
/// The fraction carries all three at once — it is the same arithmetic the arrangement uses to draw
/// a fade across a clip's width, so a join that covers half a clip on screen covers half of it in
/// the file.
///
/// Nothing when the clip has no length in either unit, there being no fraction of it to take.
fn fade_frames(span: Ticks, length_ticks: Ticks, length_frames: u64) -> u64 {
    if length_ticks <= Ticks::ZERO || length_frames == 0 {
        return 0;
    }
    let fraction = (span.raw() as f64 / length_ticks.raw() as f64).clamp(0.0, 1.0);
    (fraction * length_frames as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, numeral, session, session_with_clip, undo_depth};
    use auris_core::{AssetPath, ClipPreset, ClipRecipe, Note};

    #[test]
    fn a_lane_is_made_by_drawing_on_it_and_is_gone_when_the_last_point_is() {
        // Every controller goes through the commands the wheel goes through, and a clip carries
        // the ones somebody wrote on — no more. An emptied lane left behind would be saved into
        // the file and offered in a menu as a curve that says nothing.
        let (mut session, _track, clip) = session_with_clip();
        let pedal = ClipCurve::Controller(11);

        assert!(session.set_curve_point(clip, pedal, Ticks::ZERO, 1.0));
        assert!(session.set_curve_point(clip, pedal, BAR, 0.25));
        let carried = |session: &Session| {
            session
                .project()
                .midi_clip(clip)
                .expect("the clip")
                .1
                .curves()
                .collect::<Vec<_>>()
        };
        assert_eq!(carried(&session), vec![pedal]);

        // Clamped to the controller's own range rather than the bend's, whichever lane it is.
        assert!(session.set_curve_point(clip, pedal, BAR * 2, -4.0));
        let value = session
            .midi_clip(clip)
            .expect("the clip")
            .curve_at(pedal, BAR * 2);
        assert_eq!(value, 0.0, "a controller does not go below its floor");

        assert!(session.clear_curve(clip, pedal));
        assert!(
            carried(&session).is_empty(),
            "the lane outlived the last point on it"
        );
    }

    #[test]
    fn a_non_finite_curve_value_never_reaches_the_document() {
        let (mut session, _track, clip) = session_with_clip();
        let bend = ClipCurve::Bend;

        assert!(!session.set_curve_point(clip, bend, Ticks::ZERO, f32::NAN));
        assert!(
            session
                .midi_clip(clip)
                .expect("the clip")
                .curve(bend)
                .is_empty()
        );

        assert!(session.set_curve_point(clip, bend, Ticks::ZERO, 2.0));
        assert_eq!(
            session.move_curve_point(clip, bend, Ticks::ZERO, BAR, f32::INFINITY),
            None
        );
        let point = session.midi_clip(clip).expect("the clip").curve(bend)[0];
        assert_eq!(point.at, Ticks::ZERO);
        assert_eq!(point.value, 2.0);
    }

    #[test]
    fn a_loop_reaches_the_next_clip_and_switches_off_again() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let first = session
            .add_midi_clip(track, "A", Ticks::ZERO, BAR)
            .expect("clip");
        session
            .add_midi_clip(track, "B", BAR * 4, BAR)
            .expect("clip");

        assert!(!session.clip_is_looped(first));
        assert_eq!(session.clip_sounding_length(first), Some(BAR));

        assert_eq!(session.toggle_clip_loop(first).ok(), Some(true));
        assert!(session.clip_is_looped(first));
        assert_eq!(
            session.clip_sounding_length(first),
            Some(BAR * 4),
            "the loop should fill the gap in front of the clip"
        );
        // The clip itself has not changed; only how many times it is said.
        assert_eq!(session.midi_clip(first).expect("clip").length, BAR);

        assert_eq!(session.toggle_clip_loop(first).ok(), Some(false));
        assert!(!session.clip_is_looped(first));
        assert_eq!(session.clip_loop_end(first), Ticks::ZERO);

        assert!(matches!(
            session.toggle_clip_loop(ClipId(9_999)),
            Err(SessionError::UnknownClip(_))
        ));
    }

    #[test]
    fn dragging_the_loop_back_over_the_clip_stops_it_repeating() {
        // The same gesture that started the loop, run the other way. Anything landing inside the
        // clip's own length means "no repeats" rather than a loop shorter than one pass, which
        // would be a trim wearing the wrong edge.
        let (mut session, _, clip) = session_with_clip();
        let content = session.clip_content_length(clip).expect("a length");

        session.set_clip_loop(clip, content * 3).expect("looped");
        assert!(session.clip_is_looped(clip));

        session
            .set_clip_loop(clip, content - Ticks::QUARTER)
            .expect("unlooped");
        assert_eq!(session.clip_loop_end(clip), Ticks::ZERO);
        // Exactly on the clip's end is not a repeat either.
        session.set_clip_loop(clip, content * 3).expect("looped");
        session.set_clip_loop(clip, content).expect("unlooped");
        assert!(!session.clip_is_looped(clip));
    }

    #[test]
    fn a_loop_that_changes_nothing_is_not_an_undo_step() {
        // A drag sends the same answer on every frame once it has run out of travel.
        let (mut session, _, clip) = session_with_clip();
        session.set_clip_loop(clip, BAR * 4).expect("looped");
        let depth = undo_depth(&mut session);
        session.set_clip_loop(clip, BAR * 4).expect("again");
        assert_eq!(undo_depth(&mut session), depth);

        assert_eq!(session.undo(), Some(Edit::LoopClip));
        assert!(!session.clip_is_looped(clip));
    }

    #[test]
    fn an_audio_clip_loops_on_the_grid_rather_than_in_frames() {
        // A repeat lands on the musical grid, so the length is in ticks even though the trim
        // beside it is in source frames.
        let mut session = session();
        // 96 000 frames at 48 kHz is two seconds, which at 120 BPM is one bar.
        let clip = audio_clip(&mut session, 96_000);

        assert_eq!(session.clip_content_length(clip), Some(BAR));
        assert_eq!(session.toggle_clip_loop(clip).ok(), Some(true));
        assert_eq!(
            session.clip_sounding_length(clip),
            Some(BAR * 2),
            "with nothing in front of it, one extra pass"
        );
    }

    #[test]
    fn a_selection_dragged_across_lanes_moves_together_or_not_at_all() {
        // Dropping half a selection on a new track and leaving the rest behind is not what the
        // gesture meant, so one clip that cannot land refuses the whole move.
        let mut session = session();
        let source = session.add_default_instrument_track("Lead").expect("track");
        let destination = session
            .add_default_instrument_track("Second")
            .expect("track");
        let audio = session.add_audio_track("Sample");
        let first = session
            .add_midi_clip(source, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("clip");
        let second = session
            .add_midi_clip(source, "B", Ticks::from_beats(4.0), Ticks::from_beats(4.0))
            .expect("clip");

        // One of the two is sent somewhere it cannot go, so neither moves.
        let refused = session.move_clips_to_track(&[(first, destination), (second, audio)]);
        assert!(refused.is_err());
        assert_eq!(session.track_of_clip(first), Some(source));
        assert_eq!(session.track_of_clip(second), Some(source));

        // Both to a track that accepts them, and both arrive.
        session
            .move_clips_to_track(&[(first, destination), (second, destination)])
            .expect("both clips belong on an instrument track");
        assert_eq!(session.track_of_clip(first), Some(destination));
        assert_eq!(session.track_of_clip(second), Some(destination));
        assert!(session.can_undo(), "the move left no undo step");
    }

    #[test]
    fn moving_clips_nowhere_records_no_undo_step() {
        // A pointer drag calls this on every move, and most of them are within one track.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let clip = session
            .add_midi_clip(track, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("clip");
        session.forget_history();

        session
            .move_clips_to_track(&[(clip, track)])
            .expect("a clip always fits the track it is on");
        assert!(
            !session.can_undo(),
            "a move that moved nothing pushed a step onto the history"
        );
    }

    #[test]
    fn dragging_the_edge_of_a_stretched_clip_lands_where_the_pointer_did() {
        // The trim is a length of *material* and the pointer is on the timeline, so every drag on
        // a following clip goes through the stretch. Without it the edge landed at twice the
        // distance the pointer had travelled, and dragging the far end of a half-speed loop ran
        // off the end of the material after two bars of a four-bar drag.
        let mut session = session();
        session.set_bpm(120.0);
        // Four seconds of material, which at 120 is eight beats: two bars.
        let clip = audio_clip(&mut session, 192_000);
        session
            .set_clip_follows_tempo(clip, true)
            .expect("an audio clip");
        session.set_bpm(60.0);
        assert_eq!(
            session.clip_stretch(clip),
            2.0,
            "twice as long at half speed"
        );
        assert_eq!(
            session.clip_content_length(clip),
            Some(Ticks::from_beats(8.0)),
            "the two bars it was recorded as are the two bars it still covers"
        );

        // Dragging the end back to one bar: four beats at 60 is four seconds of timeline, which
        // is two seconds — half — of the material.
        let one_bar = Ticks::from_beats(4.0);
        session.resize_clip(clip, one_bar).expect("a clip");
        assert_eq!(
            audio_frames(&session, clip),
            96_000,
            "the drag was measured against the timeline rather than the material"
        );
        // And what it draws as is the bar that was asked for.
        assert_eq!(session.clip_content_length(clip), Some(one_bar));
    }

    #[test]
    fn switching_on_follow_tempo_assumes_the_tempo_the_clip_sits_at() {
        // A switch that silently did nothing until a second command was found would be a control
        // that lies. Material is nearly always dropped into the piece it was made for, so the
        // piece's own tempo is the assumption — and it is shown, in the row underneath.
        let mut session = session();
        session.set_bpm(90.0);
        let clip = audio_clip(&mut session, 96_000);
        assert_eq!(session.clip_source_bpm(clip), None);

        session
            .set_clip_follows_tempo(clip, true)
            .expect("an audio clip");
        assert_eq!(session.clip_source_bpm(clip), Some(90.0));
        assert_eq!(session.clip_stretch(clip), 1.0, "it fits as it stands");

        // Now the piece slows down, and the clip stretches to go on covering the same bars.
        session.set_bpm(45.0);
        assert_eq!(session.clip_stretch(clip), 2.0);

        // Forgetting what tempo it was recorded at stops it following: a clip stretched by
        // nothing, with the switch still on, is a control that lies in the other direction.
        session
            .set_clip_source_bpm(clip, None)
            .expect("an audio clip");
        assert!(!session.clip_follows_tempo(clip));
        assert_eq!(session.clip_stretch(clip), 1.0);
    }

    /// An audio clip of `frames` frames on its own track, with no samples behind it.
    ///
    /// Enough to exercise every command that shapes the clip; what it *sounds* like needs
    /// decoded audio, which is an importer's business rather than a fixture's.
    fn audio_clip(session: &mut Session, frames: u64) -> ClipId {
        let rate = session.project().sample_rate;
        let track = session.project.add_audio_track("Take");
        let source = session.project.add_audio_source(
            "take",
            AssetPath::external("/audio/take.wav"),
            frames,
            rate,
            2,
        );
        session
            .project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("the track was just added")
    }

    /// The audio clip's stored shape, read back for assertions.
    fn audio_shape(session: &Session, clip: ClipId) -> (f32, u64, u64) {
        let audio = session
            .project()
            .tracks
            .iter()
            .find_map(|track| track.kind.as_audio()?.clips.iter().find(|c| c.id == clip))
            .expect("the clip exists");
        (audio.gain_db, audio.fade_in_frames, audio.fade_out_frames)
    }

    /// How many source frames an audio clip plays.
    fn audio_frames(session: &Session, clip: ClipId) -> u64 {
        session
            .project()
            .audio_clip(clip)
            .expect("the clip exists")
            .length_frames
    }

    #[test]
    fn clip_gain_belongs_to_audio_and_comes_back_on_undo() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        let track = session.add_default_instrument_track("Lead").unwrap();
        let midi = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();

        session.set_clip_gain(clip, -6.0).unwrap();
        assert_eq!(audio_shape(&session, clip).0, -6.0);
        // Way past the range is the nearest gain that exists, not an error.
        session.set_clip_gain(clip, 100.0).unwrap();
        assert_eq!(audio_shape(&session, clip).0, 24.0);
        // NaN has no nearest anything and is refused outright.
        assert!(matches!(
            session.set_clip_gain(clip, f32::NAN),
            Err(SessionError::NotFinite(_))
        ));
        // A note clip's loudness is its velocities; addressing it here says so.
        assert!(matches!(
            session.set_clip_gain(midi, 0.0),
            Err(SessionError::NotAudio(_))
        ));
        assert!(matches!(
            session.set_clip_gain(ClipId(9_999), 0.0),
            Err(SessionError::UnknownClip(_))
        ));

        // A value that has not moved is not an edit.
        session.set_clip_gain(clip, 24.0).unwrap();
        assert_eq!(session.undo(), Some(Edit::SetClipGain));
        assert_eq!(session.undo(), Some(Edit::SetClipGain));
        assert_eq!(audio_shape(&session, clip).0, 0.0);
        assert!(!session.can_undo());
    }

    /// Two clips of `frames` frames on one track, the second starting at `second`.
    fn overlapping(session: &mut Session, frames: u64, second: Ticks) -> (ClipId, ClipId) {
        let rate = session.project().sample_rate;
        let track = session.project.add_audio_track("Take");
        let source = session.project.add_audio_source(
            "take",
            AssetPath::external("/audio/take.wav"),
            frames,
            rate,
            2,
        );
        let first = session
            .project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("the track was just added");
        let next = session
            .project
            .add_audio_clip(track, source, second)
            .expect("the track was just added");
        (first, next)
    }

    /// The curves on a clip's two edges.
    fn curves(session: &Session, clip: ClipId) -> (FadeCurve, FadeCurve) {
        let audio = session.project().audio_clip(clip).expect("the clip exists");
        (audio.fade_in_curve, audio.fade_out_curve)
    }

    #[test]
    fn a_crossfade_covers_the_overlap_from_both_sides() {
        // Two two-beat clips, the second starting one beat in: they overlap by a beat, which is
        // half of each clip — so half of each clip's frames become its half of the join.
        let mut session = session();
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER);
        session.forget_history();

        let overlap = session.crossfade_clips(first, second).expect("a crossfade");
        assert_eq!(overlap, Ticks::QUARTER);
        assert_eq!(audio_shape(&session, first), (0.0, 0, 24_000));
        assert_eq!(audio_shape(&session, second), (0.0, 24_000, 0));
        // Equal power on the two edges that meet, and nothing said about the two that do not.
        assert_eq!(curves(&session, first).1, FadeCurve::EqualPower);
        assert_eq!(curves(&session, second).0, FadeCurve::EqualPower);
        assert_eq!(curves(&session, first).0, FadeCurve::Linear);
        assert_eq!(curves(&session, second).1, FadeCurve::Linear);

        // One step for the pair. Two would leave an Undo holding half a join.
        assert_eq!(session.undo(), Some(Edit::Crossfade));
        assert_eq!(audio_shape(&session, first), (0.0, 0, 0));
        assert_eq!(audio_shape(&session, second), (0.0, 0, 0));
        assert!(!session.can_undo());
    }

    #[test]
    fn the_order_the_clips_are_named_in_does_not_matter() {
        let mut session = session();
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER);
        session.crossfade_clips(second, first).expect("a crossfade");
        assert_eq!(audio_shape(&session, first), (0.0, 0, 24_000));
        assert_eq!(audio_shape(&session, second), (0.0, 24_000, 0));
    }

    #[test]
    fn clips_that_do_not_overlap_are_not_crossfaded() {
        let mut session = session();
        // Two beats each, the second starting at beat two: they touch and do not overlap.
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER * 2);
        assert!(matches!(
            session.crossfade_clips(first, second),
            Err(SessionError::NotOverlapping)
        ));
        assert!(!session.can_undo(), "a refusal wrote to the document");

        // And neither is a clip with itself.
        assert!(matches!(
            session.crossfade_clips(first, first),
            Err(SessionError::NotOverlapping)
        ));
    }

    #[test]
    fn a_clip_swallowed_by_another_crossfades_over_its_own_length() {
        // The second clip is shorter and sits entirely inside the first. The join can only be as
        // long as the shorter clip, or the fade would run past the audio behind it.
        let mut session = session();
        let rate = session.project().sample_rate;
        let track = session.project.add_audio_track("Take");
        let long = session.project.add_audio_source(
            "long",
            AssetPath::external("/audio/long.wav"),
            96_000,
            rate,
            2,
        );
        let short = session.project.add_audio_source(
            "short",
            AssetPath::external("/audio/short.wav"),
            24_000,
            rate,
            2,
        );
        let first = session
            .project
            .add_audio_clip(track, long, Ticks::ZERO)
            .expect("a clip");
        let inside = session
            .project
            .add_audio_clip(track, short, Ticks::QUARTER)
            .expect("a clip");

        let overlap = session.crossfade_clips(first, inside).expect("a crossfade");
        // The short clip is 24 000 frames at 48 kHz, which is half a second — one beat at 120.
        assert_eq!(overlap, Ticks::QUARTER);
        assert_eq!(audio_shape(&session, inside), (0.0, 24_000, 0));
        // The same second of the timeline is a quarter of the long clip's four beats, and so a
        // quarter of the 96 000 frames behind them.
        assert_eq!(audio_shape(&session, first), (0.0, 0, 24_000));
    }

    #[test]
    fn a_fade_shape_can_be_chosen_by_hand_and_comes_back_on_undo() {
        // What a crossfade sets for itself, for the joins somebody made another way — a fade
        // drawn by dragging, or a clip trimmed back until it met its neighbour.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.set_clip_fades(clip, 1_000, 2_000).unwrap();
        session.forget_history();
        assert_eq!(
            session.fade_curves(clip),
            Some((FadeCurve::Linear, FadeCurve::Linear))
        );

        session
            .set_fade_out_curve(clip, FadeCurve::EqualPower)
            .unwrap();
        assert_eq!(
            session.fade_curves(clip),
            Some((FadeCurve::Linear, FadeCurve::EqualPower)),
            "the other edge was shaped as well"
        );
        assert_eq!(session.undo(), Some(Edit::SetClipFade));
        assert_eq!(
            session.fade_curves(clip),
            Some((FadeCurve::Linear, FadeCurve::Linear))
        );

        // Writing the shape it already has is not an edit.
        session.set_fade_in_curve(clip, FadeCurve::Linear).unwrap();
        assert!(!session.can_undo());
        // And a note clip has no fades to shape.
        let midi = session
            .add_default_instrument_track("Synth")
            .and_then(|track| session.add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::QUARTER))
            .expect("a midi clip");
        assert!(matches!(
            session.set_fade_in_curve(midi, FadeCurve::EqualPower),
            Err(SessionError::NotAudio(_))
        ));
    }

    #[test]
    fn a_clip_dropped_over_its_neighbour_is_joined_to_it() {
        // The gesture: drag a clip until it overlaps, let go, and the join is shaped. It goes in
        // the move's own transaction, so the fade and the move are one undo step — the fade
        // exists because the clip landed there.
        let mut session = session();
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER);
        session.forget_history();

        session.begin_transaction(Edit::MoveClip);
        assert_eq!(session.crossfade_landings(&[second]), 1);
        assert!(session.end_transaction());
        assert_eq!(audio_shape(&session, first), (0.0, 0, 24_000));
        assert_eq!(audio_shape(&session, second), (0.0, 24_000, 0));
        assert_eq!(session.undo(), Some(Edit::MoveClip));
        assert_eq!(audio_shape(&session, first), (0.0, 0, 0));
        assert!(!session.can_undo(), "the join was a second step");
    }

    #[test]
    fn a_drop_never_writes_over_a_fade_somebody_drew() {
        // A fade is a decision about how that clip ends, and a gesture aimed at *where the clip
        // sits* has no business rewriting it. The join is left for the menu row to make.
        let mut session = session();
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER);
        session.set_clip_fades(first, 0, 1_000).unwrap();
        session.forget_history();

        assert_eq!(session.crossfade_landings(&[second]), 0);
        assert_eq!(audio_shape(&session, first), (0.0, 0, 1_000));
        assert_eq!(audio_shape(&session, second), (0.0, 0, 0));

        // The same for a fade on the other side of the join.
        session.set_clip_fades(first, 0, 0).unwrap();
        session.set_clip_fades(second, 1_000, 0).unwrap();
        assert_eq!(session.crossfade_landings(&[second]), 0);
    }

    #[test]
    fn a_clip_dropped_where_nothing_meets_it_is_left_alone() {
        let mut session = session();
        let (_, second) = overlapping(&mut session, 48_000, Ticks::QUARTER * 2);
        assert_eq!(session.crossfade_landings(&[second]), 0);
        assert_eq!(audio_shape(&session, second), (0.0, 0, 0));
        // And a clip that is not audio at all is simply not a join.
        let midi = session
            .add_default_instrument_track("Synth")
            .and_then(|track| session.add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::QUARTER))
            .expect("a midi clip");
        assert_eq!(session.crossfade_landings(&[midi]), 0);
    }

    #[test]
    fn the_nearest_neighbour_is_the_one_a_clip_would_join() {
        let mut session = session();
        let (first, second) = overlapping(&mut session, 48_000, Ticks::QUARTER);
        assert_eq!(
            session.crossfade_partner(first),
            Some((second, Ticks::QUARTER))
        );
        assert_eq!(
            session.crossfade_partner(second),
            Some((first, Ticks::QUARTER))
        );

        // A clip on its own has nobody to join.
        let alone = audio_clip(&mut session, 48_000);
        assert_eq!(session.crossfade_partner(alone), None);
    }

    #[test]
    fn a_fade_is_the_fraction_of_the_clip_the_join_covers() {
        // Free of everything: what the conversion has to get right is that a clip's fades are
        // counted in the frames of its file while a join is measured in ticks.
        assert_eq!(
            fade_frames(Ticks::QUARTER, Ticks::QUARTER * 2, 48_000),
            24_000
        );
        assert_eq!(
            fade_frames(Ticks::QUARTER * 2, Ticks::QUARTER * 2, 48_000),
            48_000
        );
        // A join longer than the clip takes all of it rather than more than all of it.
        assert_eq!(
            fade_frames(Ticks::QUARTER * 4, Ticks::QUARTER * 2, 48_000),
            48_000
        );
        // And a clip with no length in either unit has no fraction to take.
        assert_eq!(fade_frames(Ticks::QUARTER, Ticks::ZERO, 48_000), 0);
        assert_eq!(fade_frames(Ticks::QUARTER, Ticks::QUARTER, 0), 0);
    }

    #[test]
    fn fades_fit_the_clip_and_never_cross() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        session.set_clip_fades(clip, 10_000, 6_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 10_000, 6_000));
        // A fade asked for past the end takes the whole clip and leaves the other nothing.
        session.set_clip_fades(clip, 96_000, 6_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 48_000, 0));
        // Two that would cross meet instead: the fade-out takes what the fade-in leaves.
        session.set_clip_fades(clip, 30_000, 30_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 30_000, 18_000));
        // Writing what is already there is not an edit.
        session.set_clip_fades(clip, 30_000, 18_000).unwrap();
        assert_eq!(undo_depth(&mut session), 3);
    }

    #[test]
    fn shrinking_a_clip_keeps_its_fades_inside_it() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.set_clip_fades(clip, 30_000, 18_000).unwrap();
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats; dragging the end to beat one
        // halves the clip to 24 000 frames, which the fades must fit inside.
        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 24_000, 0));
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(audio_shape(&session, clip), (0.0, 30_000, 18_000));
    }

    #[test]
    fn an_audio_clip_cannot_be_dragged_past_the_end_of_its_material() {
        // The right edge is a trim, and there is nothing past the last frame to trim to. Left
        // unbounded the clip drew — and saved — a block of silence with the waveform stopping
        // part way, which the renderer then clamped anyway: the picture and the sound disagreed.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats. Dragging the end to bar three asks
        // for four beats and gets the two that exist.
        session.resize_clip(clip, Ticks::QUARTER * 4).unwrap();
        assert_eq!(audio_shape(&session, clip).1, 0, "fades were not touched");
        assert_eq!(audio_frames(&session, clip), 48_000);
        assert!(
            !session.can_undo(),
            "a drag that could not lengthen the clip is not an edit"
        );

        // Shortening still works, and lengthening afterwards comes back to the whole source.
        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        assert_eq!(audio_frames(&session, clip), 24_000);
        session.resize_clip(clip, Ticks::QUARTER * 8).unwrap();
        assert_eq!(audio_frames(&session, clip), 48_000);
    }

    #[test]
    fn dragging_a_generated_clip_longer_writes_the_part_again_to_fill_it() {
        // A generated clip is its recipe, not its notes: the notes were written to fill a
        // length, so a new length wants them written again. Dragged out it used to gain a tail
        // of silence, and dragged in it kept notes hanging past its own end.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_chord(Ticks::ZERO, numeral("I"));
        let recipe = ClipRecipe::new(ClipPreset::Chords, 7);
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 2, recipe)
            .unwrap();
        let two_bars = session.midi_clip(clip).unwrap().notes.len();
        assert!(two_bars > 0, "the fixture wrote nothing to begin with");
        session.forget_history();

        session.resize_clip(clip, BAR * 4).unwrap();
        let four_bars = session.midi_clip(clip).unwrap().notes.len();
        assert!(
            four_bars > two_bars,
            "four bars of the same part wrote {four_bars} notes against {two_bars}"
        );
        assert!(
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .any(|note| note.start >= BAR * 2),
            "the new bars are empty"
        );

        // One drag, one undo step — and it puts back both the length and the notes.
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(session.midi_clip(clip).unwrap().length, BAR * 2);
        assert_eq!(session.midi_clip(clip).unwrap().notes.len(), two_bars);
    }

    #[test]
    fn dragging_a_played_clip_leaves_its_notes_exactly_where_they_are() {
        // The other half of the rule: a clip with no recipe is notes somebody put there, and
        // resizing it must not invent or discard any of them.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let before = session.midi_clip(clip).unwrap().notes.clone();

        session.resize_clip(clip, BAR * 3).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes, before);
        assert_eq!(session.midi_clip(clip).unwrap().length, BAR * 3);
    }

    #[test]
    fn trimming_an_audio_clip_from_the_front_moves_its_window_into_the_source() {
        // The difference between a trim and a move: the material under the clip has to stay
        // where it sounds. Walking `start` without walking `offset_frames` would slide the whole
        // take along the timeline and call it a trim.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats. Trimming to beat two hides the first
        // 24 000 frames and leaves the end where it was.
        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.start, Ticks::QUARTER);
        assert_eq!(audio.offset_frames, 24_000);
        assert_eq!(audio.length_frames, 24_000);

        // Dragging back out uncovers what was hidden rather than repeating what is left.
        session.trim_clip_start(clip, Ticks::ZERO).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.offset_frames, 0);
        assert_eq!(audio.length_frames, 48_000);

        // And it stops at the source's first frame: there is nothing before it to uncover.
        session.trim_clip_start(clip, -Ticks::QUARTER * 4).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.offset_frames, 0);
        assert_eq!(audio.length_frames, 48_000);
        assert_eq!(audio.start, Ticks::ZERO);
    }

    #[test]
    fn a_trimmed_clip_moved_to_the_start_cannot_uncover_what_will_not_fit() {
        // Its window still has material behind it, and there is nowhere on the timeline to put
        // it. Clamping the tick alone would leave the start pinned at bar one while the window
        // kept walking backwards — and the far end would slide right, off a drag aimed at the
        // left edge.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        session.move_clip(clip, Ticks::ZERO).unwrap();
        session.forget_history();

        let before = session.project().audio_clip(clip).unwrap().clone();
        session.trim_clip_start(clip, -Ticks::QUARTER * 4).unwrap();
        let after = session.project().audio_clip(clip).unwrap();
        assert_eq!(after.start, Ticks::ZERO);
        assert_eq!(after.offset_frames, before.offset_frames);
        assert_eq!(after.length_frames, before.length_frames);
        assert!(
            !session.can_undo(),
            "an edge with nowhere to go is not an edit"
        );
    }

    #[test]
    fn trimming_a_generated_clip_from_the_front_writes_it_again_over_what_is_left() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_chord(Ticks::ZERO, numeral("I"));
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Chords, 7),
            )
            .unwrap();
        session.forget_history();

        session.trim_clip_start(clip, BAR * 2).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR * 2);
        assert_eq!(midi.length, BAR * 2);
        assert!(!midi.notes.is_empty(), "the two bars left are empty");
        assert!(
            midi.notes.iter().all(|note| note.end() <= BAR * 2),
            "a note hangs past the clip it was written into"
        );
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
    }

    #[test]
    fn trimming_a_played_clip_from_the_front_rebases_the_notes_it_keeps() {
        // A played clip's notes are nobody's to reinvent, so they move with the edge. The rule is
        // the one a split already follows: a note the cut runs through keeps its sounding half.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR * 2)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER * 3))
            .unwrap();
        session
            .add_note(clip, Note::new(67, BAR, Ticks::QUARTER))
            .unwrap();
        assert!(session.set_curve_point(clip, ClipCurve::Bend, Ticks::QUARTER, 0.25));
        assert!(session.set_curve_point(clip, ClipCurve::Bend, Ticks::QUARTER * 3, 0.75));
        assert!(session.set_curve_point(clip, ClipCurve::MODULATION, BAR, 0.5));

        session.trim_clip_start(clip, Ticks::QUARTER * 2).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, Ticks::QUARTER * 2);
        assert_eq!(midi.length, BAR * 2 - Ticks::QUARTER * 2);
        // The first note is gone, the second keeps the half the cut left it, the third moved.
        let kept: Vec<(u8, i64, i64)> = midi
            .notes
            .iter()
            .map(|note| (note.pitch, note.start.raw(), note.length.raw()))
            .collect();
        assert_eq!(
            kept,
            vec![
                (64, 0, Ticks::QUARTER.raw() * 2),
                (67, Ticks::QUARTER.raw() * 2, Ticks::QUARTER.raw()),
            ]
        );
        assert_eq!(
            midi.bend,
            vec![CurvePoint {
                at: Ticks::QUARTER,
                value: 0.75,
            }]
        );
        assert_eq!(
            midi.curve(ClipCurve::MODULATION),
            &[CurvePoint {
                at: Ticks::QUARTER * 2,
                value: 0.5,
            }]
        );
    }

    #[test]
    fn neither_edge_may_be_dragged_past_the_other() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session.add_midi_clip(track, "Riff", BAR, BAR * 2).unwrap();

        // The front stops a grid division short of the end rather than turning the clip inside
        // out, which is the same floor the other edge keeps.
        session.trim_clip_start(clip, BAR * 9).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.length, session.project().grid);
        assert_eq!(midi.start + midi.length, BAR * 3, "the end moved");

        session.resize_clip(clip, Ticks::ZERO).unwrap();
        assert_eq!(
            session.midi_clip(clip).unwrap().length,
            session.project().grid
        );
    }

    #[test]
    fn a_clip_already_shorter_than_the_grid_refuses_to_be_trimmed_from_the_front() {
        // A clip shorter than a grid division is ordinary — a piece of a split, a clip drawn
        // before the grid was made coarser — and the floor the front stops at then sits behind
        // the clip's own start. Taken as a ceiling it dragged the start *backwards* on the first
        // mouse-move of a gesture with no threshold, lengthening a clip nobody asked to lengthen
        // and, in the first bar, pushing its start below zero.
        let mut session = session();
        session.set_grid(BAR);
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();

        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, Ticks::ZERO, "the start moved, and backwards");
        assert_eq!(
            midi.length,
            Ticks::QUARTER,
            "the clip grew of its own accord"
        );
        assert!(
            !session.can_undo(),
            "an edge with nowhere to go is not an edit"
        );

        // Dragging the other way is still a lengthening, because uncovering earlier material is
        // never the thing that runs out of room.
        let clip = session
            .add_midi_clip(track, "Short", BAR * 2, Ticks::QUARTER)
            .unwrap();
        session.trim_clip_start(clip, BAR).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR);
        assert_eq!(midi.length, BAR + Ticks::QUARTER, "the end moved");

        // And a clip longer than the grid trims exactly as it did: to where it was asked, and no
        // further than a division short of its own end.
        let clip = session
            .add_midi_clip(track, "Long", Ticks::ZERO, BAR * 4)
            .unwrap();
        session.trim_clip_start(clip, BAR).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR);
        assert_eq!(midi.length, BAR * 3);

        session.trim_clip_start(clip, BAR * 9).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR * 3);
        assert_eq!(midi.length, BAR);
    }

    #[test]
    fn a_clip_dragged_shorter_stays_shorter() {
        // The trimmed tail is still in the note list, and `fit_length_to_notes` grew the clip
        // back to cover it on the next edit — material the user had just cut reappeared and
        // started sounding again, with nothing on screen to explain it.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(clip, Note::new(67, Ticks::QUARTER * 2, Ticks::QUARTER))
            .unwrap();

        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        let trimmed = session.midi_clip(clip).unwrap().length;
        assert!(
            trimmed < Ticks::QUARTER * 2,
            "the second note is past the end now"
        );

        session
            .add_note(clip, Note::new(64, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        assert_eq!(
            session.midi_clip(clip).unwrap().length,
            trimmed,
            "the next note edit must not grow it back",
        );
    }

    #[test]
    fn a_clip_that_has_never_been_resized_still_grows_to_hold_what_is_written_in_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::QUARTER * 4, Ticks::QUARTER))
            .unwrap();
        assert!(session.midi_clip(clip).unwrap().length > Ticks::QUARTER * 4);
    }

    #[test]
    fn a_midi_clip_cannot_be_added_to_an_audio_track() {
        let mut session = session();
        let track = session.add_audio_track("Audio");
        let error = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap_err();
        assert!(matches!(error, SessionError::WrongTrackKind { .. }));
    }

    #[test]
    fn a_duplicated_clip_can_be_edited_without_touching_the_original() {
        let (mut session, _, clip) = session_with_clip();
        let copy = session.duplicate_clip(clip).unwrap();

        session
            .move_clip(copy, Ticks::from_beats(16.0))
            .expect("the copy is addressable in its own right");

        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
        assert_eq!(
            session.midi_clip(copy).unwrap().start,
            Ticks::from_beats(16.0)
        );
    }

    #[test]
    fn splitting_outside_a_clip_records_no_undo_step() {
        let (mut session, _, clip) = session_with_clip();
        session.forget_history();

        let error = session.split_clip(clip, Ticks::from_beats(99.0));
        assert!(matches!(error, Err(SessionError::CannotSplit(_))));
        assert!(
            !session.can_undo(),
            "a split that did nothing must not leave an undo step behind"
        );

        let right = session.split_clip(clip, Ticks::from_beats(1.0)).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().length, Ticks::QUARTER);
        assert_eq!(session.midi_clip(right).unwrap().start, Ticks::QUARTER);
        assert!(session.can_undo());
    }

    #[test]
    fn moving_several_clips_keeps_the_spacing_between_them() {
        let (mut session, track, first) = session_with_clip();
        let second = session
            .add_midi_clip(track, "B", Ticks::from_beats(8.0), Ticks::from_beats(4.0))
            .unwrap();
        let origins = [(first, Ticks::ZERO), (second, Ticks::from_beats(8.0))];

        // Far enough left that the first clip would go negative on its own.
        session.move_clips(&origins, Ticks::from_beats(-4.0));

        assert_eq!(session.midi_clip(first).unwrap().start, Ticks::ZERO);
        assert_eq!(
            session.midi_clip(second).unwrap().start,
            Ticks::from_beats(8.0),
            "the whole selection stops when the earliest clip reaches zero"
        );
    }

    #[test]
    fn deleting_several_clips_is_one_undo_step() {
        let (mut session, track, first) = session_with_clip();
        let second = session
            .add_midi_clip(track, "B", Ticks::from_beats(8.0), Ticks::from_beats(4.0))
            .unwrap();
        session.forget_history();

        session
            .remove_clips(&[first, second, ClipId(9999)])
            .unwrap();
        assert!(session.midi_clip(first).is_none());
        assert!(session.midi_clip(second).is_none());

        session.undo().unwrap();
        assert!(session.midi_clip(first).is_some());
        assert!(session.midi_clip(second).is_some());
    }

    #[test]
    fn a_muted_clip_is_silent_but_still_present() {
        let (mut session, _, clip) = session_with_clip();
        session.set_clip_muted(clip, true).unwrap();
        assert!(session.midi_clip(clip).unwrap().muted);

        let rendered = session
            .render_job()
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
            .unwrap();
        assert!(rendered.peak() < 1e-6, "a muted clip must not sound");

        session.undo().unwrap();
        assert!(!session.midi_clip(clip).unwrap().muted);
    }

    #[test]
    fn dragging_the_edge_of_a_clip_with_no_frames_does_nothing_instead_of_aborting() {
        // The importer refuses to make one of these, but a project saved before it did still
        // opens, and the first thing a user does with a clip that looks wrong is grab its edge.
        // The bound this used to compute was `length - 1`, which for no frames is behind the
        // bound below it — and `Ord::clamp` asserts on that in every profile, not just debug.
        let mut session = session();
        let clip = audio_clip(&mut session, 0);
        session.forget_history();

        assert!(session.trim_clip_start(clip, Ticks::QUARTER).is_ok());
        assert!(session.trim_clip_start(clip, Ticks::ZERO).is_ok());

        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.start, Ticks::ZERO);
        assert_eq!(audio.offset_frames, 0);
        assert_eq!(audio.length_frames, 0);
        assert_eq!(
            undo_depth(&mut session),
            0,
            "refusing to trim is not an edit"
        );
    }

    #[test]
    fn undo_pressed_during_a_drag_waits_for_the_drag_to_finish() {
        // Nothing in a frontend stops the keyboard reaching Undo while a mouse button is held, so
        // the rule has to live here. What it prevents: the open gesture being dropped rather than
        // closed, every further pointer move becoming its own undo step, and Escape no longer
        // having a position to put the clip back to.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();
        session.set_clip_gain(clip, -6.0).unwrap();

        session.begin_transaction(Edit::MoveClip);
        session.move_clip(clip, Ticks::QUARTER).unwrap();

        assert!(!session.can_undo(), "the gesture owns the document");
        assert_eq!(session.undo(), None);
        assert_eq!(session.redo(), None);

        // The gesture is still open, so it still closes as one step and Escape still works.
        assert_eq!(
            session.project().audio_clip(clip).unwrap().start,
            Ticks::QUARTER
        );
        assert!(
            session.revert_transaction(),
            "the picked-up position survived"
        );
        assert_eq!(
            session.project().audio_clip(clip).unwrap().start,
            Ticks::ZERO
        );

        // And with nothing open, undo is itself again: one step for the gain, and no strays.
        assert!(session.can_undo());
        assert_eq!(session.undo(), Some(Edit::SetClipGain));
        assert_eq!(session.project().audio_clip(clip).unwrap().gain_db, 0.0);
    }

    #[test]
    fn a_gesture_interrupted_by_undo_still_closes_as_one_step() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        session.begin_transaction(Edit::MoveClip);
        for beat in 1..=4 {
            session.undo();
            session
                .move_clip(clip, Ticks::from_beats(beat as f64))
                .unwrap();
        }
        assert!(
            session.end_transaction(),
            "the gesture was still there to close"
        );
        assert_eq!(
            undo_depth(&mut session),
            1,
            "four pointer moves and four rejected undos are one step"
        );
    }
}
