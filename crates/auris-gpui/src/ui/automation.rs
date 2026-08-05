//! The geometry of an automation lane: where its row is, and where a value sits in it.
//!
//! The lane column used to be one row per track, and five separate walks over the track list
//! agreed about where each one began by all doing the same arithmetic. A sub-lane under a track
//! makes that five chances to disagree, so the walk happens once here and everything else reads
//! [`lane_rows`] — the painter, the hit tests, the scroll limit and the reveal.
//!
//! Everything in this module is a free function over plain geometry, so the rules can be asserted
//! without a window: which row a pointer is in, what value a height means, and what shape a curve
//! draws between two points.

use std::collections::BTreeMap;

use auris_session::prelude::*;
use gpui::{Bounds, Pixels, Point, point, px};

use crate::ui::timeline::TimelineView;

/// Height of an automation row, in pixels.
///
/// Fixed rather than proportional to the track: a curve needs enough room to be aimed at, and a
/// track squeezed down to a strip of waveform still wants a lane you can put a point on.
pub const ROW_HEIGHT: Pixels = px(56.0);

/// How close to a point the pointer has to be to take hold of it.
pub const GRAB: Pixels = px(5.0);

/// What a row in the lane column holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// The track's clips.
    Clips,
    /// A parameter's curve, under the track it belongs to.
    Automation(ParamTarget),
}

/// One row of the lane column, with where it sits in the column's own coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LaneRow {
    /// The track this row belongs to. An automation row belongs to the track it sits under.
    pub track: TrackId,
    /// Top of the row, measured from the top of the column.
    pub top: Pixels,
    /// How tall the row is.
    pub height: Pixels,
    /// What the row holds.
    pub kind: RowKind,
}

impl LaneRow {
    /// Whether `y` falls inside this row.
    pub fn contains(&self, y: Pixels) -> bool {
        y >= self.top && y < self.top + self.height
    }

    /// The parameter this row draws, or `None` for a row of clips.
    pub fn target(&self) -> Option<ParamTarget> {
        match self.kind {
            RowKind::Clips => None,
            RowKind::Automation(target) => Some(target),
        }
    }
}

/// Every row of the lane column, top to bottom.
///
/// `tracks` is each track's id and lane height, in the order they are drawn; `open` says which of
/// them have an automation lane showing and which parameter it is on. A track's automation row
/// always sits directly under its clips, so the two move together when anything above them
/// changes height.
pub fn lane_rows(tracks: &[(TrackId, f32)], open: &BTreeMap<TrackId, ParamTarget>) -> Vec<LaneRow> {
    let mut rows = Vec::with_capacity(tracks.len());
    let mut top = px(0.0);
    for (track, height) in tracks {
        let height = px(*height);
        rows.push(LaneRow {
            track: *track,
            top,
            height,
            kind: RowKind::Clips,
        });
        top += height;
        if let Some(target) = open.get(track) {
            rows.push(LaneRow {
                track: *track,
                top,
                height: ROW_HEIGHT,
                kind: RowKind::Automation(*target),
            });
            top += ROW_HEIGHT;
        }
    }
    rows
}

/// Combined height of every row.
pub fn rows_height(rows: &[LaneRow]) -> Pixels {
    rows.last().map_or(px(0.0), |row| row.top + row.height)
}

/// The row containing `y`.
pub fn row_at(rows: &[LaneRow], y: Pixels) -> Option<LaneRow> {
    rows.iter().copied().find(|row| row.contains(y))
}

/// Where a track's rows begin and how tall they are together.
///
/// The clips and the automation under them as one block, because that is what a person means by
/// "this track" when the view scrolls to reveal it.
pub fn track_span(rows: &[LaneRow], track: TrackId) -> Option<(Pixels, Pixels)> {
    let mut span: Option<(Pixels, Pixels)> = None;
    for row in rows.iter().filter(|row| row.track == track) {
        span = Some(match span {
            None => (row.top, row.height),
            Some((top, height)) => (top, height + row.height),
        });
    }
    span
}

/// Each track's block of rows, in the order they are drawn.
///
/// A track's clips and the automation lane under them are one block, because a person dragging a
/// header is carrying the whole thing and lets go of it somewhere among the others.
pub fn track_blocks(rows: &[LaneRow]) -> Vec<(TrackId, Pixels, Pixels)> {
    let mut blocks: Vec<(TrackId, Pixels, Pixels)> = Vec::new();
    for row in rows {
        match blocks.last_mut() {
            Some(block) if block.0 == row.track => block.2 += row.height,
            _ => blocks.push((row.track, row.top, row.height)),
        }
    }
    blocks
}

/// Where a track being dragged up or down the list should land.
///
/// Returns the index [`Session::move_track`](auris_session::Session::move_track) should be given,
/// or `None` when the drop would leave the order as it is.
///
/// The rule is the gap nearest the pointer: above a track while the pointer is in its top half,
/// below it once past the middle. Measuring against the *neighbour's* midpoint rather than the
/// dragged track's own edges is what gives the gesture its hysteresis — a track that has just
/// swapped places does not swap back until the pointer crosses the same line again — so a hand
/// resting on a boundary cannot make the list flicker between two orders.
pub fn reorder_target(rows: &[LaneRow], dragged: TrackId, y: Pixels) -> Option<usize> {
    let blocks = track_blocks(rows);
    let from = blocks.iter().position(|(track, ..)| *track == dragged)?;
    // Gaps run 0..=len: one before the first track, one after the last, and one between each pair.
    let gap = blocks
        .iter()
        .position(|(_, top, height)| y < *top + *height * 0.5)
        .unwrap_or(blocks.len());
    // `Project::move_track` removes before it inserts, so a gap below the track's own position has
    // already closed up by one frame by the time the insert happens.
    let to = if gap > from { gap - 1 } else { gap };
    (to != from).then_some(to)
}

/// Where `value` sits vertically inside a row spanning `min` to `max`.
///
/// The maximum is at the top, the way a fader reads and the way every automation lane in every
/// DAW draws. A range with no width puts everything on the centre line rather than dividing by
/// zero — that is a parameter that cannot be automated meaningfully, and a flat line says so.
pub fn value_to_y(value: f32, min: f32, max: f32, top: Pixels, height: Pixels) -> Pixels {
    let span = max - min;
    let fraction = match span.abs() < f32::EPSILON {
        true => 0.5,
        false => ((value - min) / span).clamp(0.0, 1.0),
    };
    top + height * (1.0 - fraction)
}

/// The value a vertical position inside a row means.
///
/// The inverse of [`value_to_y`], and it has to stay one: a point dropped where the pointer is
/// has to read back as the value that was drawn there, or a drag walks the curve away from the
/// hand every time it is picked up again.
pub fn y_to_value(y: Pixels, min: f32, max: f32, top: Pixels, height: Pixels) -> f32 {
    if height <= px(0.0) {
        return min;
    }
    let fraction = 1.0 - (f32::from(y - top) / f32::from(height)).clamp(0.0, 1.0);
    min + (max - min) * fraction
}

/// Where each of a lane's points is drawn, in window coordinates.
pub fn point_positions(
    lane: &AutomationLane,
    range: (f32, f32),
    view: &TimelineView,
    row: Bounds<Pixels>,
) -> Vec<Point<Pixels>> {
    lane.points()
        .iter()
        .map(|written| {
            point(
                row.origin.x + view.tick_to_x(written.tick),
                value_to_y(
                    written.value,
                    range.0,
                    range.1,
                    row.origin.y,
                    row.size.height,
                ),
            )
        })
        .collect()
}

/// The polyline a lane draws, in window coordinates.
///
/// A held lane gets the corner it needs: the value stays where it was until the next point and
/// then steps, so the line is drawn as an L rather than a diagonal. Drawing a slope over a lane
/// that holds would promise a sweep that is not going to happen.
///
/// Both ends run out to the edges of the row, because a lane holds its nearest value flat outside
/// the stretch it was written over — and a curve that stopped at its last point would look like a
/// parameter that stops being driven there.
pub fn curve_polyline(
    lane: &AutomationLane,
    range: (f32, f32),
    view: &TimelineView,
    row: Bounds<Pixels>,
) -> Vec<Point<Pixels>> {
    let written = point_positions(lane, range, view, row);
    let Some(first) = written.first().copied() else {
        return Vec::new();
    };
    let last = written[written.len() - 1];
    let mut line = Vec::with_capacity(written.len() * 2 + 2);
    line.push(point(row.origin.x, first.y));
    for (index, at) in written.iter().copied().enumerate() {
        if lane.curve == AutomationCurve::Hold && index > 0 {
            line.push(point(at.x, line[line.len() - 1].y));
        }
        line.push(at);
    }
    line.push(point(row.origin.x + row.size.width, last.y));
    line
}

/// The point under the pointer, if the pointer is on one.
///
/// Returns the point's tick, which is what every command addresses a point by. Compared in pixels
/// rather than in ticks so the grab is the same size at every zoom — at two bars to the screen a
/// tick-wide tolerance would be untouchable, and at two beats it would swallow the whole lane.
pub fn point_at(
    positions: &[Point<Pixels>],
    lane: &AutomationLane,
    pointer: Point<Pixels>,
) -> Option<Ticks> {
    positions
        .iter()
        .enumerate()
        .filter(|(_, at)| (at.x - pointer.x).abs() <= GRAB && (at.y - pointer.y).abs() <= GRAB)
        // The nearest, so overlapping points at a zoomed-out view still resolve to one of them
        // rather than always to the earliest.
        .min_by(|(_, a), (_, b)| {
            let distance = |at: &Point<Pixels>| {
                let (dx, dy) = (f32::from(at.x - pointer.x), f32::from(at.y - pointer.y));
                dx * dx + dy * dy
            };
            distance(a).total_cmp(&distance(b))
        })
        .map(|(index, _)| lane.points()[index].tick)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn tracks() -> Vec<(TrackId, f32)> {
        vec![(TrackId(1), 72.0), (TrackId(2), 100.0), (TrackId(3), 72.0)]
    }

    fn opened(track: u64) -> BTreeMap<TrackId, ParamTarget> {
        BTreeMap::from([(TrackId(track), ParamTarget::TrackGain(TrackId(track)))])
    }

    #[test]
    fn with_nothing_open_the_rows_are_the_tracks() {
        let rows = lane_rows(&tracks(), &BTreeMap::new());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].top, px(0.0));
        assert_eq!(rows[1].top, px(72.0));
        assert_eq!(rows[2].top, px(172.0));
        assert_eq!(rows_height(&rows), px(244.0));
    }

    #[test]
    fn an_open_lane_sits_under_its_own_track_and_pushes_the_rest_down() {
        let rows = lane_rows(&tracks(), &opened(1));
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows[1].kind,
            RowKind::Automation(ParamTarget::TrackGain(TrackId(1)))
        );
        assert_eq!(
            rows[1].track,
            TrackId(1),
            "it belongs to the track above it"
        );
        assert_eq!(rows[1].top, px(72.0));
        // Everything below moves down by exactly the row's height, and nothing else changes.
        assert_eq!(rows[2].top, px(72.0) + ROW_HEIGHT);
        assert_eq!(rows_height(&rows), px(244.0) + ROW_HEIGHT);
    }

    #[test]
    fn the_row_under_the_pointer_is_the_one_the_painter_drew_there() {
        // The whole reason this module exists: one walk, so a press and a paint cannot disagree.
        let rows = lane_rows(&tracks(), &opened(1));
        assert_eq!(row_at(&rows, px(0.0)).map(|r| r.kind), Some(RowKind::Clips));
        assert_eq!(
            row_at(&rows, px(71.9)).map(|r| r.kind),
            Some(RowKind::Clips)
        );
        assert!(matches!(
            row_at(&rows, px(72.0)).map(|r| r.kind),
            Some(RowKind::Automation(_))
        ));
        assert_eq!(
            row_at(&rows, px(72.0) + ROW_HEIGHT).map(|r| r.track),
            Some(TrackId(2))
        );
        assert_eq!(row_at(&rows, px(10_000.0)), None);
    }

    #[test]
    fn a_dragged_track_lands_in_the_gap_the_pointer_is_nearest() {
        // Tracks at 0..72, 72..172, 172..244; the midpoints are 36, 122 and 208.
        let rows = lane_rows(&tracks(), &BTreeMap::new());
        let target = |dragged: u64, y: f32| reorder_target(&rows, TrackId(dragged), px(y));

        // Carrying the first track down: nothing happens until the pointer passes the *second*
        // track's midpoint, and passing the third's puts it at the end.
        assert_eq!(target(1, 100.0), None);
        assert_eq!(target(1, 130.0), Some(1));
        assert_eq!(target(1, 220.0), Some(2));
        // And carrying the last one up, the same rule read the other way.
        assert_eq!(target(3, 190.0), None);
        assert_eq!(target(3, 100.0), Some(1));
        assert_eq!(target(3, 10.0), Some(0));
        // Past either end of the column the answer is the end of the list, not nothing: a hand
        // that overshoots means the top or the bottom.
        assert_eq!(target(2, -500.0), Some(0));
        assert_eq!(target(2, 5_000.0), Some(2));
    }

    #[test]
    fn a_reorder_that_has_just_happened_does_not_happen_back() {
        // The gesture's hysteresis, as numbers. A pointer resting where a swap was decided must
        // read as "already there" once the swap has been made, or the list flickers between two
        // orders for as long as the hand is still.
        let before = lane_rows(&tracks(), &BTreeMap::new());
        assert_eq!(reorder_target(&before, TrackId(1), px(130.0)), Some(1));

        // The same pointer against the list that move produced: 2 at 0..100, 1 at 100..172.
        let after = lane_rows(
            &[(TrackId(2), 100.0), (TrackId(1), 72.0), (TrackId(3), 72.0)],
            &BTreeMap::new(),
        );
        assert_eq!(reorder_target(&after, TrackId(1), px(130.0)), None);
    }

    #[test]
    fn a_track_is_carried_with_the_lane_it_has_open() {
        // An open automation lane belongs to the track above it, so the two move as one block and
        // the gaps are between *tracks* rather than between rows.
        let rows = lane_rows(&tracks(), &opened(1));
        let blocks = track_blocks(&rows);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0], (TrackId(1), px(0.0), px(72.0) + ROW_HEIGHT));
        assert_eq!(blocks[1].0, TrackId(2));
        // The second track's midpoint has moved down by the open row, and the drop follows it.
        let middle = px(72.0) + ROW_HEIGHT + px(50.0);
        assert_eq!(reorder_target(&rows, TrackId(1), middle - px(1.0)), None);
        assert_eq!(reorder_target(&rows, TrackId(1), middle + px(1.0)), Some(1));
    }

    #[test]
    fn a_track_is_revealed_with_the_lane_it_has_open() {
        let rows = lane_rows(&tracks(), &opened(2));
        let (top, height) = track_span(&rows, TrackId(2)).expect("a span");
        assert_eq!(top, px(72.0));
        assert_eq!(
            height,
            px(100.0) + ROW_HEIGHT,
            "scrolling to a track has to bring its curve along, not cut it off"
        );
        assert_eq!(track_span(&rows, TrackId(9)), None);
    }

    #[test]
    fn the_top_of_a_row_is_the_top_of_the_range() {
        // Which way up a lane reads is not arbitrary — every fader and every DAW puts the
        // maximum at the top, and a lane drawn the other way would be read wrong at a glance.
        assert_eq!(
            value_to_y(12.0, -60.0, 12.0, px(100.0), px(56.0)),
            px(100.0)
        );
        assert_eq!(
            value_to_y(-60.0, -60.0, 12.0, px(100.0), px(56.0)),
            px(156.0)
        );
        assert_eq!(
            value_to_y(-24.0, -60.0, 12.0, px(100.0), px(56.0)),
            px(128.0)
        );
    }

    #[test]
    fn a_value_read_back_from_where_it_was_drawn_is_the_same_value() {
        // These two have to stay inverses or a point walks away from the hand each time it is
        // picked up: the drag reads a value from y and writes it back, and the painter puts it
        // where the value says.
        for value in [-60.0, -24.0, -6.0, 0.0, 12.0] {
            let y = value_to_y(value, -60.0, 12.0, px(40.0), px(56.0));
            let back = y_to_value(y, -60.0, 12.0, px(40.0), px(56.0));
            assert!((back - value).abs() < 0.01, "{value} came back as {back}");
        }
    }

    #[test]
    fn a_parameter_with_no_range_draws_a_flat_line_rather_than_dividing_by_zero() {
        assert_eq!(value_to_y(1.0, 1.0, 1.0, px(0.0), px(56.0)), px(28.0));
        assert_eq!(y_to_value(px(28.0), 1.0, 1.0, px(0.0), px(56.0)), 1.0);
    }

    fn row() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(200.0), px(100.0)),
            size: size(px(800.0), ROW_HEIGHT),
        }
    }

    fn lane(curve: AutomationCurve) -> AutomationLane {
        AutomationLane::new(
            ParamTarget::MasterGain,
            curve,
            vec![
                AutomationPoint::new(Ticks::ZERO, 0.0),
                AutomationPoint::new(Ticks(960), 12.0),
            ],
        )
        .expect("a valid lane")
    }

    #[test]
    fn a_curve_runs_out_to_both_edges_of_its_row() {
        // A lane holds its nearest value flat outside the stretch it was written over, so a line
        // that stopped at the last point would draw a parameter that stops being driven there.
        let view = TimelineView::default();
        let line = curve_polyline(&lane(AutomationCurve::Linear), (-60.0, 12.0), &view, row());
        assert_eq!(line[0].x, px(200.0));
        assert_eq!(line[line.len() - 1].x, px(1000.0));
        assert_eq!(line[0].y, line[1].y, "flat until the first point");
        let end = line.len() - 1;
        assert_eq!(line[end].y, line[end - 1].y, "and flat after the last");
    }

    #[test]
    fn a_holding_lane_is_drawn_as_a_corner_rather_than_a_slope() {
        // Drawing a diagonal over a lane that holds promises a sweep that is not going to happen.
        let view = TimelineView::default();
        let held = curve_polyline(&lane(AutomationCurve::Hold), (-60.0, 12.0), &view, row());
        let linear = curve_polyline(&lane(AutomationCurve::Linear), (-60.0, 12.0), &view, row());
        assert_eq!(
            held.len(),
            linear.len() + 1,
            "the corner is the extra vertex"
        );
        // The corner sits above the second point, at the first point's height.
        let corner = held[held.len() - 3];
        assert_eq!(corner.y, held[1].y);
        assert_eq!(corner.x, held[held.len() - 2].x);
    }

    #[test]
    fn a_point_is_grabbed_from_within_a_few_pixels_of_it() {
        let view = TimelineView::default();
        let lane = lane(AutomationCurve::Linear);
        let positions = point_positions(&lane, (-60.0, 12.0), &view, row());
        let on = positions[1];
        assert_eq!(point_at(&positions, &lane, on), Some(Ticks(960)));
        assert_eq!(
            point_at(&positions, &lane, point(on.x + GRAB, on.y)),
            Some(Ticks(960)),
            "the edge of the grab still counts"
        );
        assert_eq!(
            point_at(&positions, &lane, point(on.x + GRAB + px(1.0), on.y)),
            None
        );
        // Near the curve but not near a point is empty lane, which is where a new point is made.
        assert_eq!(
            point_at(&positions, &lane, point(on.x - px(40.0), on.y)),
            None
        );
    }
}
