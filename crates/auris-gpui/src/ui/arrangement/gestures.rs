//! What the pointer does over the clip lanes: presses, drags, menus and the wheel.
//!
//! Its own file because a press here is a sequence of questions — is this an automation row, a
//! fade handle, a clip already selected, empty lane — and the order they are asked in *is* the
//! behaviour. Reading that order in one place is the only way to see it. The clip lookups it
//! asks along the way are here too, for the same reason.
//!
//! Not to be confused with `crate::gestures`, which is where a create or a delete gesture is
//! bound to a button and a modifier. This is what the arrangement does once that has answered.

use auris_session::prelude::*;

use gpui::{Bounds, MouseDownEvent, Pixels, point, px, size};

use crate::app::{AurisApp, ClipEdge, Drag, FadeEdge};
use crate::ui::automation::{self, LaneRow};

use super::geometry::{CLIP_INSET, fade_handle_at, resize_grab, selection_without};

impl AurisApp {
    /// Where every selected clip starts, captured before a move begins.
    fn selected_clip_origins(&self) -> Vec<(ClipId, Ticks)> {
        self.selected_clips
            .iter()
            .filter_map(|id| self.clip_start(*id).map(|start| (*id, start)))
            .collect()
    }

    /// The lane each selected clip currently sits on.
    fn selected_clip_lanes(&self) -> Vec<(ClipId, usize)> {
        self.selected_clips
            .iter()
            .filter_map(|id| {
                let track = self.session.track_of_clip(*id)?;
                Some((*id, self.project().track_index(track)?))
            })
            .collect()
    }

    /// Where a clip of either kind starts.
    fn clip_start(&self, clip: ClipId) -> Option<Ticks> {
        if let Some(midi) = self.session.midi_clip(clip) {
            return Some(midi.start);
        }
        self.project().tracks.iter().find_map(|track| {
            track
                .kind
                .as_audio()?
                .clips
                .iter()
                .find(|candidate| candidate.id == clip)
                .map(|clip| clip.start)
        })
    }

    /// The clip ids selected right now, for the paint closures.
    pub(super) fn selected_clip_ids(&self) -> std::collections::BTreeSet<ClipId> {
        self.selected_clips.clone()
    }

    /// An audio clip's stored shape: where it sits, how long it is both ways, and its dials.
    ///
    /// `None` for a MIDI clip or an id nothing answers to, which is what lets the fade
    /// gesture and the gain sheet simply not exist for note clips.
    pub(crate) fn audio_clip_shape(
        &self,
        clip: ClipId,
    ) -> Option<(Ticks, Ticks, u64, f32, u64, u64)> {
        self.project().tracks.iter().find_map(|track| {
            let audio = track.kind.as_audio()?;
            let found = audio.clips.iter().find(|c| c.id == clip)?;
            Some((
                found.start,
                self.session.audio_clip_length_ticks(found),
                found.length_frames,
                found.gain_db,
                found.fade_in_frames,
                found.fade_out_frames,
            ))
        })
    }

    /// The audio clip on `track` whose fade handle is under a press, and which handle it is.
    ///
    /// `x` is in timeline pixels and `y_in_lane` measured from the lane's top; the clip is
    /// drawn two pixels inside its lane, which this subtracts before asking the geometry.
    ///
    /// Every audio clip on the track is asked, not only the one under the pointer's tick: a
    /// handle is a grab zone around a point on the clip's edge, so with no fades yet half of
    /// it hangs past the clip — and the fade-out handle sits exactly on the clip's end, the
    /// first tick `clip_at` counts as outside. Searched backwards like `clip_at`, so where
    /// two clips meet the handle drawn on top is the one taken hold of.
    fn fade_grab_at(
        &self,
        track: TrackId,
        x: Pixels,
        y_in_lane: Pixels,
    ) -> Option<(ClipId, FadeEdge)> {
        let track = self.project().track(track)?;
        let audio = track.kind.as_audio()?;
        audio.clips.iter().rev().find_map(|clip| {
            let edge = fade_handle_at(
                &self.timeline,
                clip.start,
                self.audio_clip_length_ticks(clip),
                clip.length_frames,
                clip.fade_in_frames,
                clip.fade_out_frames,
                x,
                y_in_lane - CLIP_INSET,
            )?;
            Some((clip.id, edge))
        })
    }

    /// Shapes a fade towards the pointer, measured as a fraction of the clip.
    ///
    /// The pointer's tick becomes a fraction of the clip's tick length and that fraction a
    /// frame count, which is exactly how the painter spreads the frames across the width —
    /// so the ramp lands where the pointer is, whatever the zoom.
    pub(crate) fn drag_clip_fade(&mut self, clip: ClipId, edge: FadeEdge, at: Ticks) {
        let Some((start, length, frames, _, fade_in, fade_out)) = self.audio_clip_shape(clip)
        else {
            return;
        };
        let fraction = ((at - start).raw() as f64 / length.raw().max(1) as f64).clamp(0.0, 1.0);
        let at_frame = (fraction * frames as f64).round() as u64;
        let (fade_in, fade_out) = match edge {
            FadeEdge::In => (at_frame, fade_out),
            FadeEdge::Out => (fade_in, frames.saturating_sub(at_frame)),
        };
        let _ = self.session.set_clip_fades(clip, fade_in, fade_out);
    }

    /// Starts a drag on the ruler: plain drag scrubs, alt-drag sets the loop region.
    pub(super) fn begin_ruler_drag(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.snap(self.timeline.x_to_tick(x));
        if event.modifiers.alt {
            // A degenerate region disables the engine loop, which is what an alt-click that
            // never becomes a drag should do.
            self.begin_drag(Drag::LoopRegion { anchor: tick });
            self.session.set_loop_enabled(true);
            self.session.set_loop_region(tick, tick);
        } else {
            self.seek(tick);
            self.begin_drag(Drag::Playhead);
        }
        cx.notify();
    }

    /// A press inside an automation row: take a point, delete one, or write a new one.
    ///
    /// The three cases in the order a hand expects them. Delete first, so the gesture bound to
    /// deleting takes a point off rather than adding one on top of it; then the point under the
    /// pointer, which is a drag; and anything else is empty lane, where a press writes a point
    /// and immediately begins dragging it — so placing a value and shaping it is one gesture
    /// rather than click, look, click again.
    fn press_automation(
        &mut self,
        row: LaneRow,
        event: &MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(target) = row.target() else {
            return;
        };
        let Some(descriptor) = self.session.descriptor_for(target) else {
            return;
        };
        // The row in window coordinates, which is what the painter drew into and therefore what
        // the pointer has to be compared against.
        let origin = self.lanes_origin();
        let bounds = Bounds {
            origin: point(origin.x, origin.y + row.top - self.lane_scroll),
            size: size(self.arrangement_width, row.height),
        };
        let range = (descriptor.min, descriptor.max);
        let value = automation::y_to_value(
            event.position.y,
            range.0,
            range.1,
            bounds.origin.y,
            bounds.size.height,
        );
        let at = self
            .snap_unless_held(
                self.timeline.x_to_tick(event.position.x - origin.x),
                event.modifiers,
            )
            .max_zero();

        let grabbed = self
            .session
            .automation()
            .lane(target)
            .map(|lane| {
                let positions = automation::point_positions(lane, range, &self.timeline, bounds);
                (
                    lane.clone(),
                    automation::point_at(&positions, lane, event.position),
                )
            })
            .and_then(|(_, tick)| tick);

        self.select_track(row.track);
        if let Some(tick) = grabbed
            && self.pointer.delete.matches(event)
        {
            self.session.remove_automation_point(target, tick);
            cx.notify();
            return;
        }

        let from = match grabbed {
            Some(tick) => tick,
            None => {
                // A press on empty lane writes the point it is about to drag, so one gesture both
                // places a value and shapes it.
                if !self.session.set_automation_point(target, at, value) {
                    return;
                }
                at
            }
        };
        self.begin_drag(Drag::AutomationPoint { target, at: from });
        cx.notify();
    }

    /// Moves the point in hand to where the pointer is, and says where it landed.
    ///
    /// The row is looked up by the target rather than by where the pointer currently is, so a
    /// drag that wanders out of its own row still shapes the lane it took hold of — pulled above
    /// the top or below the bottom the value clamps, which is what a fader does too.
    pub(crate) fn drag_automation_point(
        &mut self,
        target: ParamTarget,
        at: Ticks,
        event: &gpui::MouseMoveEvent,
    ) -> Option<Ticks> {
        let row = self
            .lane_rows()
            .into_iter()
            .find(|row| row.target() == Some(target))?;
        let descriptor = self.session.descriptor_for(target)?;
        let origin = self.lanes_origin();
        let top = origin.y + row.top - self.lane_scroll;
        let value = automation::y_to_value(
            event.position.y,
            descriptor.min,
            descriptor.max,
            top,
            row.height,
        );
        let to = self
            .snap_unless_held(
                self.timeline.x_to_tick(event.position.x - origin.x),
                event.modifiers,
            )
            .max_zero();
        self.session.move_automation_point(target, at, to, value)
    }

    /// Starts a drag in the clip lanes.
    pub(super) fn begin_lane_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.lanes_origin();
        let local = point(event.position.x - origin.x, self.lane_y(event.position.y));
        let tick = self.timeline.x_to_tick(local.x);

        // An automation row is its own surface: what is in it is points, and a press there must
        // never reach the clip logic below — a rubber band swept across a curve would select the
        // clips of a track the pointer is not even over.
        if let Some(row) = self.automation_row_at(local.y) {
            self.press_automation(row, event, cx);
            return;
        }

        let Some((track_id, lane_top)) = self.track_at_y(local.y) else {
            // Below the last track there is nothing to grab, so the press can only be the start
            // of a sweep across the lanes above it.
            self.begin_rubber_band(
                crate::app::BandSurface::Lanes,
                event.position,
                event.modifiers.shift,
            );
            cx.notify();
            return;
        };
        let under_pointer = self.clip_at(track_id, tick);
        self.select_track_for_press(track_id, under_pointer.map(|(id, _, _)| id));
        if let Some((clip_id, _, _)) = under_pointer
            && self.pointer.delete.matches(event)
        {
            let _ = self.session.remove_clip(clip_id);
            let (surviving, primary) =
                selection_without(&self.selected_clips, self.selected_clip, clip_id);
            self.select_clips(surviving, primary);
            self.selected_notes.clear();
            cx.notify();
            return;
        }
        // Reached only when the user has not bound delete here, which the default no longer does.
        if let Some((clip_id, _, _)) = under_pointer
            && event.click_count >= 2
        {
            self.open_clip_in_editor(clip_id);
            cx.notify();
            return;
        }

        // The band under the title belongs to the fades; a press there takes hold of the
        // nearer handle, and the resize edge keeps everything below the band. Asked before
        // the clip under the pointer, not from inside its arm: a corner handle's grab zone
        // hangs past the clip's edge, over ticks where `clip_at` answers nothing — which
        // used to turn a press on the fade-out handle into an invisible rubber band.
        if let Some((clip_id, edge)) = self.fade_grab_at(track_id, local.x, local.y - lane_top) {
            if !self.selected_clips.contains(&clip_id) {
                self.select_clip(Some(clip_id));
            } else {
                self.selected_clip = Some(clip_id);
            }
            self.selected_notes.clear();
            self.begin_drag(Drag::ClipFade {
                clip: clip_id,
                edge,
            });
            cx.notify();
            return;
        }

        match under_pointer {
            Some((clip_id, clip_start, clip_length)) => {
                // Grabbing a clip that is already part of a selection keeps the selection, so a
                // rubber band followed by a drag moves everything it caught.
                if !self.selected_clips.contains(&clip_id) {
                    self.select_clip(Some(clip_id));
                } else {
                    self.selected_clip = Some(clip_id);
                }
                self.selected_notes.clear();
                let clip_start_x = self.timeline.tick_to_x(clip_start);
                let clip_end_x = self.timeline.tick_to_x(clip_start + clip_length);
                let grab = resize_grab(clip_end_x - clip_start_x);
                // The end is asked about first: a clip narrow enough for both zones to reach the
                // middle would otherwise be all front-trim, and the end is the edge people drag.
                if f32::from(clip_end_x - local.x).abs() <= grab {
                    self.begin_drag(Drag::ClipResize {
                        clip: clip_id,
                        edge: ClipEdge::End,
                    });
                } else if f32::from(local.x - clip_start_x).abs() <= grab {
                    self.begin_drag(Drag::ClipResize {
                        clip: clip_id,
                        edge: ClipEdge::Start,
                    });
                } else {
                    let origins = self.selected_clip_origins();
                    let origin_lanes = self.selected_clip_lanes();
                    let grab_lane = self.project().track_index(track_id).unwrap_or(0);
                    self.begin_drag(Drag::ClipMove {
                        clip: clip_id,
                        grab_offset: tick - clip_start,
                        origins,
                        origin_lanes,
                        grab_lane,
                        pressed_at: Some(event.position),
                    });
                }
            }
            None => match crate::gestures::empty_press(self.pointer, event) {
                crate::gestures::EmptyPress::Create => {
                    self.create_clip_at(track_id, self.snap(tick));
                }
                crate::gestures::EmptyPress::Band { extend } => {
                    self.begin_rubber_band(crate::app::BandSurface::Lanes, event.position, extend);
                }
            },
        }
        cx.notify();
    }

    /// Opens the menu for whatever is under the pointer in the clip lanes.
    pub(super) fn open_lane_menu(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.lanes_origin();
        let local = point(event.position.x - origin.x, self.lane_y(event.position.y));
        let tick = self.timeline.x_to_tick(local.x);

        let menu = match self.track_at_y(local.y) {
            Some((track_id, _)) => {
                let under_pointer = self.clip_at(track_id, tick);
                self.select_track_for_press(track_id, under_pointer.map(|(id, _, _)| id));
                match under_pointer {
                    Some((clip_id, _, _)) => {
                        // A right-click on a clip selects it, so Split at Playhead and the
                        // inspector are talking about the clip the menu is titled after — but a
                        // right-click *inside* a selection leaves that selection alone.
                        if !self.selected_clips.contains(&clip_id) {
                            self.select_clip(Some(clip_id));
                        } else {
                            self.selected_clip = Some(clip_id);
                        }
                        self.selected_notes.clear();
                        self.clip_menu(event.position, clip_id)
                    }
                    None => self.lane_menu(event.position, track_id, self.snap(tick).max_zero()),
                }
            }
            None => self.arrangement_menu(event.position),
        };
        self.open_menu(menu);
        cx.notify();
    }

    /// Wheel handling: plain moves down the tracks, Shift moves along the song, Ctrl or Alt zooms.
    pub(super) fn scroll_timeline(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(24.0));
        match crate::gestures::wheel_action(event.modifiers) {
            crate::gestures::Wheel::Zoom => {
                // About the pointer, so the bar under it stays under it: zooming towards a
                // fixed left edge walks the thing being looked at off the screen.
                let anchor = event.position.x - self.timeline_origin().x;
                let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
                self.timeline.zoom_by(factor, anchor);
            }
            crate::gestures::Wheel::AlongTheSong => self.timeline.scroll_by(-delta.y - delta.x),
            crate::gestures::Wheel::DownTheTracks => {
                // Plain wheel moves down the track list, the way it does in every other panel and
                // every other DAW; Shift is what turns it sideways. It used to scroll the timeline
                // horizontally, which was the only thing it could do while the lanes had no scroll
                // of their own — and left every track past the sixth unreachable.
                self.scroll_lanes_by(-delta.y);
                self.timeline.scroll_by(-delta.x);
            }
        }
        cx.notify();
    }

    /// Clip on `track` covering `tick`, with its start and length.
    ///
    /// Searched backwards, because [`paint_lane`](super::lane_paint::paint_lane) draws the list
    /// forwards: where two clips overlap the later one is on top, and a hit test that walked the
    /// same way as the painter answered with the clip *underneath* — so the click selected, moved
    /// and deleted something the user could not see.
    fn clip_at(&self, track: TrackId, tick: Ticks) -> Option<(ClipId, Ticks, Ticks)> {
        let track = self.project().track(track)?;
        match &track.kind {
            TrackKind::Instrument(inner) => inner
                .clips
                .iter()
                .rev()
                .find(|clip| tick >= clip.start && tick < clip.end())
                .map(|clip| (clip.id, clip.start, clip.length)),
            TrackKind::Audio(inner) => inner
                .clips
                .iter()
                .rev()
                .map(|clip| (clip.id, clip.start, self.audio_clip_length_ticks(clip)))
                .find(|(_, start, length)| tick >= *start && tick < *start + *length),
            TrackKind::Bus => None,
        }
    }
}
