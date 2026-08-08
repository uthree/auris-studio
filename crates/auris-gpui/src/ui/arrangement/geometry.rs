//! Where a press lands: the arithmetic every surface of the arrangement shares.
//!
//! Pure functions and the constants they are measured in — how far a column may scroll, which
//! handle a press took hold of, where a grab zone parts around the band a fade owns, what is
//! left of a selection once a clip has gone. Its own file because this is the whole of the
//! arrangement that can be checked without a window: an element cannot be unit-tested and a free
//! function can, so a rule that decides something lives here rather than inside the surface that
//! asks it. Every test in the module is at the foot of this file for that reason.
//!
//! The constants are shared rather than merely convenient. A hit test measured from one number
//! and a painter from another is how a grab bar ends up a pixel to the left of the bar it is
//! drawn on, which is a bug nobody sees and everybody feels.

use auris_session::prelude::*;

use gpui::{Bounds, Pixels, point, px, size};

use crate::app::{ClipEdge, FadeEdge};
use crate::ui::paint;

/// Width of the grab zone on a clip's right edge, in pixels.
const RESIZE_HANDLE: f32 = 7.0;

/// How far a column of `content` can be scrolled inside a `viewport` before it runs out.
///
/// Never negative: content shorter than the viewport does not scroll at all, and a viewport
/// whose size has not been recorded yet reports zero rather than letting the column drift.
pub(crate) fn scroll_limit(content: Pixels, viewport: Pixels) -> Pixels {
    (content - viewport).max(px(0.0))
}

/// The scroll offset that brings a row into view, moving as little as possible.
///
/// A row already on screen returns the offset unchanged. One above the top scrolls up to it, one
/// below the bottom scrolls down until its foot is flush — and a row taller than the viewport
/// puts its *top* on screen, because that is the end with the name on it.
pub(crate) fn reveal_offset(
    scroll: Pixels,
    viewport: Pixels,
    row_top: Pixels,
    row_height: Pixels,
) -> Pixels {
    if row_top < scroll {
        return row_top;
    }
    let below = row_top + row_height - viewport;
    if below > scroll {
        return below.min(row_top);
    }
    scroll
}

/// How wide the grab zone on a clip's right edge is, for a clip drawn `width` across.
///
/// Never more than a third of the clip: zoomed far enough out, a fixed handle either side of the
/// end covers the whole block and the clip can be stretched but never moved.
pub(super) fn resize_grab(width: Pixels) -> f32 {
    RESIZE_HANDLE.min(f32::from(width) / 3.0)
}

/// How far a clip is drawn inside its lane, top and bottom.
///
/// Read by the cursor zones as well as the painter: the edges the pointer is offered are the
/// edges that were drawn, and a zone measured from the lane instead of the clip would light up
/// two pixels of the gap between two tracks.
pub(super) const CLIP_INSET: Pixels = px(2.0);

/// Height of a clip's name bar.
pub(super) const TITLE_HEIGHT: Pixels = px(14.0);

/// Height of the band under the name bar where an audio clip's fade handles live.
///
/// The band splits the clip's edges by height: a press in it takes hold of a fade, one below
/// it takes the resize handle or the clip body, exactly as the painter draws the handles.
const FADE_BAND: Pixels = px(12.0);

/// Half-width of the grab zone around a fade handle, in pixels.
const FADE_GRAB: f32 = 6.0;

/// Clips narrower than this draw no fade handles and offer none to grab.
///
/// On a sliver of a clip the two handles and the resize zone would overlap into a lottery;
/// the fades themselves still play and still draw, only the handles go.
pub(super) const FADE_HANDLE_MIN_WIDTH: f32 = 40.0;

/// The chord a press at `x` takes hold of, if it landed on that chord's handle.
///
/// Free rather than a method so the rule can be tested without a window: a handle is six pixels
/// wide, and which side of them a press lands on is the difference between moving a chord and
/// playing it.
pub(super) fn chord_handle_at(
    view: &crate::ui::timeline::TimelineView,
    handles: &[Ticks],
    x: Pixels,
) -> Option<Ticks> {
    handles.iter().copied().find(|start| {
        // The same inset the painter uses, or the zone would sit a pixel to the left of the bar
        // it belongs to and miss its rightmost column entirely.
        let left = view.tick_to_x(*start) + paint::CHORD_BLOCK_INSET;
        x >= left && x < left + paint::CHORD_HANDLE
    })
}

/// The section boundary a press at `x` takes hold of, if it landed on that boundary's handle.
///
/// Two decisions, and only the first of them is the structure lane's own. A handle is drawn on a
/// boundary that names a stretch and sits inside `visible`: the unnamed point that *ends* a
/// song's structure has nothing to move, and a section beginning off to the left of the window
/// is painted as a block with no grab bar on it, because dragging the visible edge of a clipped
/// span would move a boundary that is not there.
///
/// Where the bar itself is, on the other hand, is [`chord_handle_at`]'s question rather than a
/// second answer to it. `paint::structure_lane` insets its block by exactly the pixel
/// `paint::harmony_lane` insets its own by, and paints the same six-pixel bar on the front of it;
/// the copy of the rule that used to live here had drifted off that inset, which left the
/// rightmost painted column of every grab bar dead to the press that landed on it.
pub(super) fn section_handle_at(
    view: &crate::ui::timeline::TimelineView,
    points: &[SectionPoint],
    visible: (Ticks, Ticks),
    x: Pixels,
) -> Option<Ticks> {
    let (from, to) = visible;
    let handles: Vec<Ticks> = points
        .iter()
        .filter(|point| point.label.is_some() && point.tick >= from && point.tick <= to)
        .map(|point| point.tick)
        .collect();
    chord_handle_at(view, &handles, x)
}

/// One edge's grab zone, split around the band an audio clip gives to its fade handles.
///
/// Two rectangles rather than one when the band applies, because the name bar above it resizes
/// too — a press there falls past the fade check, which wants a `y` inside the band. Free rather
/// than a method so the split can be checked without a window.
pub(super) fn edge_zone_rows(
    x: Pixels,
    width: Pixels,
    top: Pixels,
    bottom: Pixels,
    fade_band: bool,
) -> Vec<Bounds<Pixels>> {
    let row = |from: Pixels, to: Pixels| {
        (to > from).then(|| Bounds {
            origin: point(x, from),
            size: size(width, to - from),
        })
    };
    if !fade_band {
        return row(top, bottom).into_iter().collect();
    }
    let band = top + TITLE_HEIGHT;
    [row(top, band), row(band + FADE_BAND, bottom)]
        .into_iter()
        .flatten()
        .collect()
}

/// The fade handle a press at `x`, `y_in_clip` takes hold of, on an audio clip drawn from
/// `start` for `length` whose fades cover `fade_in` and `fade_out` of its `frames` frames.
///
/// Free rather than a method for the same reason [`chord_handle_at`] is. `y_in_clip` is
/// measured from the clip's own top edge: the handles live in the band just under the title
/// strip, which is what leaves the rest of the right edge to the resize grab. When both
/// handles are within reach — a clip with no fades yet is exactly its two corners — the
/// nearer one wins, and a tie goes to the fade-in.
#[allow(clippy::too_many_arguments)]
pub(super) fn fade_handle_at(
    view: &crate::ui::timeline::TimelineView,
    start: Ticks,
    length: Ticks,
    frames: u64,
    fade_in: u64,
    fade_out: u64,
    x: Pixels,
    y_in_clip: Pixels,
) -> Option<FadeEdge> {
    if frames == 0 {
        return None;
    }
    let width = f32::from(view.duration_to_width(length));
    if width <= FADE_HANDLE_MIN_WIDTH {
        return None;
    }
    if y_in_clip < TITLE_HEIGHT || y_in_clip > TITLE_HEIGHT + FADE_BAND {
        return None;
    }
    let left = f32::from(view.tick_to_x(start));
    let in_x = left + width * (fade_in as f64 / frames as f64) as f32;
    let out_x = left + width * (1.0 - (fade_out as f64 / frames as f64) as f32);
    let x = f32::from(x);
    let to_in = (x - in_x).abs();
    let to_out = (x - out_x).abs();
    if to_in > FADE_GRAB && to_out > FADE_GRAB {
        return None;
    }
    Some(if to_in <= to_out {
        FadeEdge::In
    } else {
        FadeEdge::Out
    })
}

/// Which of a clip's three edge handles a press took hold of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ClipGrab {
    /// The far end of the repeats, which says how many times the clip is heard.
    Loop,
    /// One end of the clip itself, which says what the clip is.
    Resize(ClipEdge),
}

/// The handle a press at `x`, `y_in_clip` took hold of, on a clip drawn from `start_x` to `end_x`
/// whose repeats reach `loop_x`.
///
/// The three questions in the order they have to be asked, which *is* the behaviour:
///
/// 1. **The loop edge, but only in the name bar.** On a clip that has never been looped `loop_x`
///    and `end_x` are the same pixel, and the strip with the name on it is the half that starts a
///    loop — the way it is in Logic, and the only arrangement under which a clip nobody has
///    looped yet offers the gesture at all.
/// 2. **The end**, at any height, which is the edge people drag. Only the loop is confined to the
///    name bar; taking the resize away from the rest of the clip's height to match would cost the
///    common gesture far more than the rare one gains.
/// 3. **The start**, last, because a clip narrow enough for both zones to reach the middle would
///    otherwise be all front-trim.
///
/// Free rather than a method for the reason everything else in this file is: the order above is
/// the difference between stretching a phrase and repeating it, and a rule that decides that
/// belongs where it can be read and checked without a window.
pub(super) fn clip_grab_at(
    start_x: Pixels,
    end_x: Pixels,
    loop_x: Pixels,
    x: Pixels,
    y_in_clip: Pixels,
) -> Option<ClipGrab> {
    let grab = resize_grab(end_x - start_x);
    let within = |edge: Pixels| f32::from(edge - x).abs() <= grab;
    if y_in_clip < TITLE_HEIGHT && within(loop_x) {
        return Some(ClipGrab::Loop);
    }
    if within(end_x) {
        return Some(ClipGrab::Resize(ClipEdge::End));
    }
    if within(start_x) {
        return Some(ClipGrab::Resize(ClipEdge::Start));
    }
    None
}

/// What is left of a clip selection once `removed` has been deleted from the document, as the
/// surviving set and the clip the editors should point at.
///
/// The id has to leave the whole set and not merely the primary slot. A rubber band leaves
/// several clips selected and the delete gesture takes them one at a time, so pruning only when
/// the one deleted happened to be the primary left the id of a clip that no longer exists sitting
/// in the selection — invisible until a command read the set, and then Duplicate, which fails a
/// whole batch on the first id that resolves to nothing, reported failure for a gesture whose
/// copies it had already made. Clearing the selection outright is the other half of the same
/// mistake: the clips beside the deleted one are still there and were still chosen.
///
/// The primary is only dropped here, never re-pointed: `AurisApp::select_clips` puts it back on a
/// member of whatever set it is given, and that is the one place that rule should live.
pub(super) fn selection_without(
    selected: &std::collections::BTreeSet<ClipId>,
    primary: Option<ClipId>,
    removed: ClipId,
) -> (std::collections::BTreeSet<ClipId>, Option<ClipId>) {
    let surviving = selected
        .iter()
        .copied()
        .filter(|id| *id != removed)
        .collect();
    (surviving, primary.filter(|id| *id != removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::timeline::TimelineView;

    fn view() -> TimelineView {
        TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks::ZERO,
        }
    }

    #[test]
    fn the_name_bar_starts_a_loop_where_the_body_below_it_stretches_the_clip() {
        // The one rule the whole loop gesture rests on. Both edges sit on the same pixel until a
        // clip is looped, so which one a press means is answered entirely by its height — and if
        // the resize won that tie no clip could ever be looped by the mouse at all.
        let (start, end) = (px(100.0), px(300.0));
        let below = TITLE_HEIGHT + px(4.0);

        assert_eq!(
            clip_grab_at(start, end, end, end, px(2.0)),
            Some(ClipGrab::Loop)
        );
        assert_eq!(
            clip_grab_at(start, end, end, end, below),
            Some(ClipGrab::Resize(ClipEdge::End))
        );
        // The far edge of a clip that *is* looped is the loop's, and the clip's own end — now in
        // the middle of the block — is still the resize edge.
        let looped = px(700.0);
        assert_eq!(
            clip_grab_at(start, end, looped, looped, px(2.0)),
            Some(ClipGrab::Loop)
        );
        assert_eq!(
            clip_grab_at(start, end, looped, end, below),
            Some(ClipGrab::Resize(ClipEdge::End))
        );
        // Only the *loop* is confined to the name bar. The clip's own end answers at every
        // height, including inside the strip, because that is the edge people reach for.
        assert_eq!(
            clip_grab_at(start, end, looped, end, px(2.0)),
            Some(ClipGrab::Resize(ClipEdge::End))
        );
        // The front trims, and the middle is neither — that is where a move begins.
        assert_eq!(
            clip_grab_at(start, end, looped, start, below),
            Some(ClipGrab::Resize(ClipEdge::Start))
        );
        assert_eq!(clip_grab_at(start, end, looped, px(200.0), below), None);
    }

    #[test]
    fn a_chord_is_taken_hold_of_by_its_leading_edge_and_played_anywhere_else() {
        // The whole gesture rests on this: a press inside the handle moves the chord, and a press
        // one pixel past it sounds the chord instead and sweeps the progression. Both are the
        // same button on the same block.
        let view = view();
        let bar = Ticks(3840);
        let handles = [bar, bar * 4];

        // Where the *bar* is drawn, which is the block's inset edge and not the chord's tick.
        let edge = view.tick_to_x(bar) + paint::CHORD_BLOCK_INSET;
        assert_eq!(chord_handle_at(&view, &handles, edge), Some(bar));
        assert_eq!(
            chord_handle_at(&view, &handles, edge + paint::CHORD_HANDLE - px(1.0)),
            Some(bar),
            "the last column the handle is painted on is still the handle"
        );
        assert_eq!(
            chord_handle_at(&view, &handles, edge + paint::CHORD_HANDLE),
            None,
            "past the handle is the body of the chord"
        );
        assert_eq!(
            chord_handle_at(&view, &handles, edge - px(1.0)),
            None,
            "and before it is the gap between the blocks"
        );
        assert_eq!(
            chord_handle_at(
                &view,
                &handles,
                view.tick_to_x(bar * 4) + paint::CHORD_BLOCK_INSET
            ),
            Some(bar * 4)
        );
        assert_eq!(chord_handle_at(&view, &[], edge), None);
    }

    #[test]
    fn the_grab_zone_covers_the_bar_that_is_drawn_and_nothing_beside_it() {
        // The painter insets the block; the hit test did not, so the whole zone sat one pixel to
        // the left of the bar and its rightmost column could not be pressed at all.
        let view = view();
        let bar = Ticks(3840);
        let drawn = view.tick_to_x(bar) + paint::CHORD_BLOCK_INSET;
        for offset in 0..(f32::from(paint::CHORD_HANDLE) as i32) {
            assert_eq!(
                chord_handle_at(&view, &[bar], drawn + px(offset as f32)),
                Some(bar),
                "every column the bar is painted on takes hold of it",
            );
        }
    }

    #[test]
    fn a_section_is_taken_hold_of_by_the_bar_that_is_drawn_and_nothing_beside_it() {
        // The structure lane's grab bars are the chord lane's, painted at the same inset by the
        // same rule — and the second copy of that rule which used to live in the lane had drifted
        // off the inset, leaving the rightmost painted column of every bar dead to a press.
        let view = view();
        let bar = Ticks(3840);
        let points = [
            SectionPoint {
                tick: bar,
                label: Some("Chorus".to_string()),
            },
            SectionPoint {
                tick: bar * 4,
                label: None,
            },
        ];
        let visible = (Ticks::ZERO, bar * 8);
        let at = |x| section_handle_at(&view, &points, visible, x);

        let drawn = view.tick_to_x(bar) + paint::CHORD_BLOCK_INSET;
        for offset in 0..(f32::from(paint::CHORD_HANDLE) as i32) {
            assert_eq!(
                at(drawn + px(offset as f32)),
                Some(bar),
                "every column the bar is painted on takes hold of the section",
            );
        }
        assert_eq!(
            at(drawn + paint::CHORD_HANDLE - px(0.5)),
            Some(bar),
            "the last column the bar is painted on is still the bar"
        );
        assert_eq!(
            at(drawn - px(1.0)),
            None,
            "before it is the gap between two blocks"
        );
        assert_eq!(
            at(drawn + paint::CHORD_HANDLE),
            None,
            "past it is the block the name is written on"
        );

        // The unnamed point that ends a structure draws as bare background: nothing to take hold
        // of, because there is no section there to move.
        assert_eq!(at(view.tick_to_x(bar * 4) + paint::CHORD_BLOCK_INSET), None);

        // A section beginning just off the left of the window has no bar either. The painter
        // draws a clipped span without one, because the boundary is not at the edge the visible
        // block starts on — so the last columns of a bar that geometry alone would still answer
        // for must not be pressable.
        let scrolled = TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: bar + Ticks(60),
        };
        assert_eq!(scrolled.tick_to_x(bar), px(-3.0));
        assert_eq!(
            section_handle_at(
                &scrolled,
                &points,
                scrolled.visible_range(px(600.0)),
                px(0.0)
            ),
            None
        );
    }

    #[test]
    fn deleting_one_of_a_swept_selection_prunes_it_and_keeps_the_rest() {
        let swept: std::collections::BTreeSet<ClipId> =
            [ClipId(1), ClipId(2), ClipId(3)].into_iter().collect();

        // The gesture pruned the selection only when the clip deleted happened to be the primary
        // one, so deleting any of the others left the id of a clip that no longer exists behind —
        // and the next Duplicate failed the whole batch on it.
        let (surviving, primary) = selection_without(&swept, Some(ClipId(1)), ClipId(3));
        assert_eq!(
            surviving.iter().copied().collect::<Vec<_>>(),
            vec![ClipId(1), ClipId(2)],
            "the deleted clip stayed in the selection"
        );
        assert_eq!(
            primary,
            Some(ClipId(1)),
            "and the primary was not the one deleted"
        );

        // Deleting the primary is the other half of it: the clips beside it are still in the
        // document and were still chosen, so the selection must not go with it.
        let (surviving, primary) = selection_without(&swept, Some(ClipId(1)), ClipId(1));
        assert_eq!(
            surviving.iter().copied().collect::<Vec<_>>(),
            vec![ClipId(2), ClipId(3)]
        );
        assert_eq!(
            primary, None,
            "`select_clips` is what puts it back on a survivor"
        );

        // The last clip standing leaves nothing selected at all, exactly as it always did.
        let alone: std::collections::BTreeSet<ClipId> = [ClipId(7)].into_iter().collect();
        let (surviving, primary) = selection_without(&alone, Some(ClipId(7)), ClipId(7));
        assert!(surviving.is_empty());
        assert_eq!(primary, None);
    }

    #[test]
    fn a_fade_is_taken_hold_of_in_the_band_under_the_title() {
        // Four beats at 48 px/beat: a 192-pixel clip, 48 000 frames behind it.
        let view = view();
        let length = Ticks::from_beats(4.0);
        let frames = 48_000;
        let y = TITLE_HEIGHT + px(4.0);

        // With no fades yet, the handles are the clip's top corners.
        let grab = |fade_in, fade_out, x: f32, y| {
            fade_handle_at(
                &view,
                Ticks::ZERO,
                length,
                frames,
                fade_in,
                fade_out,
                px(x),
                y,
            )
        };
        assert_eq!(grab(0, 0, 0.0, y), Some(FadeEdge::In));
        assert_eq!(grab(0, 0, 192.0, y), Some(FadeEdge::Out));
        assert_eq!(grab(0, 0, 96.0, y), None, "between the handles is the body");
        assert_eq!(
            grab(0, 0, 192.0, TITLE_HEIGHT + FADE_BAND + px(1.0)),
            None,
            "below the band the same pixel is the resize edge"
        );
        assert_eq!(
            grab(0, 0, 96.0, TITLE_HEIGHT - px(2.0)),
            None,
            "the title strip stays the move grab"
        );

        // A fade already drawn moves its handle to the top of the ramp.
        let quarter = frames / 4;
        assert_eq!(grab(quarter, 0, 48.0, y), Some(FadeEdge::In));
        assert_eq!(
            grab(quarter, 0, 0.0, y),
            None,
            "the corner stopped being the handle once the fade moved it"
        );
        assert_eq!(grab(0, quarter, 144.0, y), Some(FadeEdge::Out));
    }

    #[test]
    fn a_corner_handle_answers_on_both_of_its_halves() {
        // With no fades yet the handles sit on the clip's corners, so half of each grab zone
        // hangs past the clip — ticks `clip_at` counts as outside, which is why the lane
        // press asks for fades before it asks what clip is under the pointer. Gated behind
        // the clip under the tick, the fade-out handle could only be taken by the sliver of
        // its zone that was still inside the clip.
        let view = view();
        let y = TITLE_HEIGHT + px(4.0);
        let grab = |x: f32| {
            fade_handle_at(
                &view,
                Ticks::ZERO,
                Ticks::from_beats(4.0),
                48_000,
                0,
                0,
                px(x),
                y,
            )
        };
        assert_eq!(grab(-4.0), Some(FadeEdge::In));
        assert_eq!(grab(196.0), Some(FadeEdge::Out));
    }

    #[test]
    fn a_sliver_of_a_clip_offers_no_fade_handles() {
        // Half a beat is 24 pixels — under the minimum, where the two handles and the resize
        // grab would overlap into a lottery.
        let view = view();
        assert_eq!(
            fade_handle_at(
                &view,
                Ticks::ZERO,
                Ticks::from_beats(0.5),
                6_000,
                0,
                0,
                px(0.0),
                TITLE_HEIGHT + px(4.0),
            ),
            None
        );
    }

    #[test]
    fn the_resize_cursor_covers_what_a_press_would_actually_grab() {
        // The zone the arrow lights up and the zone the button acts on are the same three
        // numbers, and this is the assertion that keeps them so: an arrow promising a grab the
        // press does not deliver is worse than no arrow at all.
        let top = px(0.0);
        let bottom = px(68.0);
        let rows = edge_zone_rows(px(100.0), px(6.0), top, bottom, false);
        assert_eq!(rows.len(), 1, "a clip with no fade handles is one zone");
        assert_eq!(rows[0].origin, point(px(100.0), top));
        assert_eq!(rows[0].size, size(px(6.0), bottom - top));

        // An audio clip's band belongs to its fades, so the zone parts around it — and the name
        // bar above the band still resizes, because a press there falls past the fade check.
        let rows = edge_zone_rows(px(100.0), px(6.0), top, bottom, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].size.height, TITLE_HEIGHT);
        assert_eq!(rows[1].origin.y, TITLE_HEIGHT + FADE_BAND);
        assert_eq!(rows[1].size.height, bottom - TITLE_HEIGHT - FADE_BAND);
        // Nothing offered inside the band, which is where `fade_handle_at` answers.
        let inside_band = TITLE_HEIGHT + px(4.0);
        assert!(
            !rows
                .iter()
                .any(|row| (row.origin.y..row.origin.y + row.size.height).contains(&inside_band)),
            "the cursor claimed a band the fades own: {rows:?}"
        );

        // A lane too short to hold the band drops the rows that would come out inside out.
        assert!(edge_zone_rows(px(0.0), px(6.0), px(0.0), px(10.0), true).len() <= 1);
        assert!(edge_zone_rows(px(0.0), px(6.0), px(0.0), px(0.0), false).is_empty());
    }

    #[test]
    fn the_two_edges_of_a_clip_never_reach_each_other() {
        // `resize_grab` caps each zone at a third of the clip, which is what leaves the middle
        // third to moving it. Without that a clip zoomed far enough out could be stretched from
        // either end and never dragged anywhere.
        for width in [3.0f32, 12.0, 40.0, 96.0, 400.0] {
            let grab = resize_grab(px(width));
            assert!(
                grab * 2.0 <= width,
                "a {width}px clip offered {grab}px of grab at each end"
            );
        }
    }

    #[test]
    fn a_column_shorter_than_its_panel_does_not_scroll() {
        assert_eq!(scroll_limit(px(300.0), px(600.0)), px(0.0));
        assert_eq!(scroll_limit(px(600.0), px(600.0)), px(0.0));
        assert_eq!(scroll_limit(px(900.0), px(600.0)), px(300.0));
        // Before the first paint the panel has no recorded height, and the column must not drift.
        assert_eq!(scroll_limit(px(900.0), px(0.0)), px(900.0));
    }

    #[test]
    fn revealing_a_row_moves_the_view_as_little_as_it_can() {
        let viewport = px(500.0);
        let row = px(100.0);

        // Already on screen: nothing moves, so a click does not shift the row out from under the
        // pointer that is still on it.
        assert_eq!(reveal_offset(px(0.0), viewport, px(200.0), row), px(0.0));
        // Below the bottom edge: down until its foot is flush, and no further.
        assert_eq!(reveal_offset(px(0.0), viewport, px(800.0), row), px(400.0));
        // Above the top edge: up to it exactly.
        assert_eq!(
            reveal_offset(px(400.0), viewport, px(200.0), row),
            px(200.0)
        );
    }

    #[test]
    fn a_row_taller_than_the_panel_is_shown_from_the_top() {
        // Scrolling until its foot is flush would put the name off the top of the screen.
        assert_eq!(
            reveal_offset(px(0.0), px(200.0), px(300.0), px(600.0)),
            px(300.0)
        );
    }

    #[test]
    fn a_resize_handle_never_swallows_the_thing_it_belongs_to() {
        // Zoomed far out a clip is a few pixels wide, and a fixed handle either side of its end
        // covered the whole block: the clip could be stretched and never moved.
        assert_eq!(resize_grab(px(300.0)), RESIZE_HANDLE);
        assert!(resize_grab(px(9.0)) < RESIZE_HANDLE);
        assert!(
            resize_grab(px(9.0)) * 2.0 < 9.0,
            "a grab zone on both sides of the end still leaves a middle to drag by",
        );
        assert_eq!(resize_grab(px(0.0)), 0.0);
    }

    #[test]
    fn a_chord_scrolled_off_the_left_keeps_its_handle_where_it_belongs() {
        // Handles are hit-tested in the same space the lane is painted in, so a scrolled view
        // must not leave them at the position they had before it scrolled.
        let view = TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks(3840),
        };
        let bar = Ticks(3840);
        assert_eq!(view.tick_to_x(bar), px(0.0));
        assert_eq!(
            chord_handle_at(&view, &[bar], paint::CHORD_BLOCK_INSET),
            Some(bar)
        );
        // A chord off the left edge is hit at a negative x, which no press can produce.
        assert_eq!(chord_handle_at(&view, &[Ticks::ZERO], px(0.0)), None);
    }
}
