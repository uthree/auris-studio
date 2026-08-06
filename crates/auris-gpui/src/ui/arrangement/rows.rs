//! Where each row of the lane column sits, and how far the column may be scrolled.
//!
//! The one walk over the tracks, in a file of its own because everything else in the arrangement
//! asks it rather than working it out: the header column, the painter, the hit tests, the scroll
//! limit and the reveal. Five of them used to do this arithmetic separately and agreed only by
//! doing it identically, which a sub-lane under a track turned into five chances to put a press
//! one row away from the thing it looks like it landed on.
//!
//! The list itself is built by `crate::ui::automation`, which is where an open automation lane
//! comes from; what is here is the column's own arithmetic on top of it.

use auris_session::prelude::*;

use gpui::{Pixels, px};

use crate::app::AurisApp;
use crate::ui::automation::{self, LaneRow};

use super::{reveal_offset, scroll_limit};

impl AurisApp {
    /// Moves the lane column, keeping it inside what there is to see.
    pub(crate) fn scroll_lanes_by(&mut self, delta: Pixels) {
        self.lane_scroll = (self.lane_scroll + delta).clamp(px(0.0), self.max_lane_scroll());
    }

    /// Combined height of every track's lane.
    /// Every row of the lane column, clips and open automation lanes alike.
    ///
    /// The one walk over the tracks. Five places used to do this arithmetic separately and agreed
    /// only by doing it identically; a sub-lane under a track turned that into five chances to
    /// put a press one row away from the thing it looks like it landed on.
    pub(crate) fn lane_rows(&self) -> Vec<LaneRow> {
        let tracks: Vec<(TrackId, f32)> = self
            .project()
            .tracks
            .iter()
            .map(|track| (track.id, track.height))
            .collect();
        automation::lane_rows(&tracks, &self.automation_lanes)
    }

    /// Combined height of every lane, the automation rows included.
    pub(crate) fn lanes_height(&self) -> Pixels {
        automation::rows_height(&self.lane_rows())
    }

    /// How far the lanes may be scrolled before the last one is flush with the bottom.
    pub(crate) fn max_lane_scroll(&self) -> Pixels {
        let viewport = self
            .canvas
            .lanes
            .get()
            .map_or(px(0.0), |bounds| bounds.size.height);
        scroll_limit(self.lanes_height(), viewport)
    }

    /// Scrolls the lanes just far enough to bring `track` into view.
    ///
    /// Just far enough, so selecting a track that is already on screen does not move the view out
    /// from under the pointer — which is exactly the frame a click is landing in.
    pub(crate) fn reveal_track(&mut self, track: TrackId) {
        let Some((top, height)) = self.lane_span(track) else {
            return;
        };
        let viewport = self
            .canvas
            .lanes
            .get()
            .map_or(px(0.0), |bounds| bounds.size.height);
        self.lane_scroll = reveal_offset(self.lane_scroll, viewport, top, height)
            .min(self.max_lane_scroll())
            .max(px(0.0));
    }

    /// Where a track's lane starts in the column, and how tall it is.
    fn lane_span(&self, track: TrackId) -> Option<(Pixels, Pixels)> {
        automation::track_span(&self.lane_rows(), track)
    }

    /// Where a window position falls in the lane column's own coordinates.
    ///
    /// Not merely the offset from the canvas: the column scrolls, so what is under the pointer is
    /// how far into the canvas it is *plus* how far the column has been pushed up.
    pub(crate) fn lane_y(&self, window_y: Pixels) -> Pixels {
        window_y - self.lanes_origin().y + self.lane_scroll
    }

    /// Track whose *clip* lane contains `y`, with the lane's top offset.
    ///
    /// An automation row answers `None`, because everything asking this — a clip drag, a clip
    /// menu, the lane a selection moves to — means the row clips live on. What is under the
    /// pointer in an automation row is [`Self::automation_row_at`]'s business.
    pub(crate) fn track_at_y(&self, y: Pixels) -> Option<(TrackId, Pixels)> {
        let row = automation::row_at(&self.lane_rows(), y)?;
        matches!(row.kind, automation::RowKind::Clips).then_some((row.track, row.top))
    }

    /// The automation row containing `y`, if `y` is in one.
    pub(crate) fn automation_row_at(&self, y: Pixels) -> Option<LaneRow> {
        automation::row_at(&self.lane_rows(), y).filter(|row| row.target().is_some())
    }

    /// Tracks whose *clip* lanes intersect the vertical span, in lane-local pixels.
    ///
    /// Clip rows only: this is what a rubber band sweeps, and a band that dipped into an open
    /// automation lane would otherwise select the clips of the track above it a second time.
    pub(crate) fn tracks_in_rows(&self, top: Pixels, bottom: Pixels) -> Vec<TrackId> {
        let (top, bottom) = (top.min(bottom), top.max(bottom));
        self.lane_rows()
            .iter()
            .filter(|row| {
                matches!(row.kind, automation::RowKind::Clips)
                    && row.top + row.height > top
                    && row.top < bottom
            })
            .map(|row| row.track)
            .collect()
    }
}
