//! Mapping between musical time and screen space.
//!
//! The arrangement ruler, the clip lanes and the piano roll all have to agree pixel-for-pixel
//! about where a tick lands, so that conversion lives here once instead of in each view.

use auris_core::time::{TICKS_PER_QUARTER, Ticks, TimeSignature};
use gpui::{Pixels, px};

/// Horizontal scroll and zoom of a timeline view.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineView {
    /// Width of one quarter note in pixels.
    pub pixels_per_beat: f32,
    /// Tick at the left edge of the visible area.
    pub scroll_ticks: Ticks,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks::ZERO,
        }
    }
}

impl TimelineView {
    /// Narrowest a quarter note may be drawn.
    pub const MIN_PIXELS_PER_BEAT: f32 = 6.0;
    /// Widest a quarter note may be drawn.
    pub const MAX_PIXELS_PER_BEAT: f32 = 720.0;

    /// Pixels per tick at the current zoom.
    pub fn pixels_per_tick(&self) -> f32 {
        self.pixels_per_beat / TICKS_PER_QUARTER as f32
    }

    /// Screen x of `tick`, relative to the left edge of the timeline body.
    pub fn tick_to_x(&self, tick: Ticks) -> Pixels {
        px((tick.0 - self.scroll_ticks.0) as f32 * self.pixels_per_tick())
    }

    /// Tick at screen x, relative to the left edge of the timeline body.
    pub fn x_to_tick(&self, x: Pixels) -> Ticks {
        let per_tick = self.pixels_per_tick();
        if per_tick <= 0.0 {
            return self.scroll_ticks;
        }
        Ticks(self.scroll_ticks.0 + (f32::from(x) / per_tick).round() as i64)
    }

    /// Width in pixels of a `duration`-long span.
    pub fn duration_to_width(&self, duration: Ticks) -> Pixels {
        px(duration.0 as f32 * self.pixels_per_tick())
    }

    /// Duration covered by `width` pixels.
    pub fn width_to_duration(&self, width: Pixels) -> Ticks {
        let per_tick = self.pixels_per_tick();
        if per_tick <= 0.0 {
            return Ticks::ZERO;
        }
        Ticks((f32::from(width) / per_tick).round() as i64)
    }

    /// Tick range visible in a body `width` pixels wide.
    pub fn visible_range(&self, width: Pixels) -> (Ticks, Ticks) {
        (
            self.scroll_ticks,
            self.scroll_ticks + self.width_to_duration(width),
        )
    }

    /// Zooms by `factor` while keeping the tick under `anchor_x` in place.
    ///
    /// Anchoring on the pointer is what makes wheel-zoom feel right: without it, zooming in
    /// walks the region of interest off the side of the view.
    pub fn zoom_by(&mut self, factor: f32, anchor_x: Pixels) {
        let anchor_tick = self.x_to_tick(anchor_x);
        self.pixels_per_beat = (self.pixels_per_beat * factor)
            .clamp(Self::MIN_PIXELS_PER_BEAT, Self::MAX_PIXELS_PER_BEAT);
        let per_tick = self.pixels_per_tick();
        if per_tick > 0.0 {
            let ticks_before_anchor = (f32::from(anchor_x) / per_tick) as i64;
            self.scroll_ticks = Ticks((anchor_tick.0 - ticks_before_anchor).max(0));
        }
    }

    /// Scrolls by a pixel delta, clamping at the timeline start.
    pub fn scroll_by(&mut self, delta_x: Pixels) {
        let ticks = self.width_to_duration(delta_x);
        self.scroll_ticks = Ticks((self.scroll_ticks.0 + ticks.0).max(0));
    }

    /// Scrolls so `tick` is visible in a body `width` pixels wide, with a small margin.
    pub fn scroll_to_reveal(&mut self, tick: Ticks, width: Pixels) {
        let (start, end) = self.visible_range(width);
        let margin = Ticks(self.width_to_duration(width).0 / 8);
        if tick < start {
            self.scroll_ticks = Ticks((tick.0 - margin.0).max(0));
        } else if tick > end {
            self.scroll_ticks = Ticks((tick.0 - self.width_to_duration(width).0 + margin.0).max(0));
        }
    }

    /// Finest grid subdivision that still leaves at least `min_spacing` pixels between lines.
    ///
    /// Drawing every 16th note at low zoom produces a grey smear, so the grid coarsens as the
    /// view zooms out, stepping through the musically meaningful divisions only.
    pub fn grid_step(&self, signature: TimeSignature, min_spacing: Pixels) -> Ticks {
        const DIVISIONS: [i64; 7] = [
            TICKS_PER_QUARTER / 8,
            TICKS_PER_QUARTER / 4,
            TICKS_PER_QUARTER / 2,
            TICKS_PER_QUARTER,
            TICKS_PER_QUARTER * 2,
            TICKS_PER_QUARTER * 4,
            TICKS_PER_QUARTER * 8,
        ];
        let min = f32::from(min_spacing);
        for step in DIVISIONS {
            if step as f32 * self.pixels_per_tick() >= min {
                return Ticks(step);
            }
        }
        // Past the largest fixed division, fall back to whole bars and keep doubling.
        let mut step = signature.ticks_per_bar().0.max(1);
        while (step as f32) * self.pixels_per_tick() < min {
            step *= 2;
        }
        Ticks(step)
    }
}

/// Vertical scroll and zoom of the piano roll's pitch axis.
#[derive(Clone, Debug, PartialEq)]
pub struct PitchView {
    /// Height of one semitone row in pixels.
    pub row_height: f32,
    /// MIDI note number at the top of the visible area.
    pub top_pitch: u8,
}

impl Default for PitchView {
    fn default() -> Self {
        Self {
            row_height: f32::from(crate::theme::Metrics::NOTE_ROW_HEIGHT),
            // C6 at the top puts middle C comfortably in view for a typical roll height.
            top_pitch: 96,
        }
    }
}

impl PitchView {
    /// Shortest a note row may be drawn.
    pub const MIN_ROW_HEIGHT: f32 = 5.0;
    /// Tallest a note row may be drawn.
    pub const MAX_ROW_HEIGHT: f32 = 40.0;
    /// Highest MIDI note the roll shows.
    pub const MAX_PITCH: u8 = 127;

    /// Screen y of the top edge of `pitch`'s row.
    pub fn pitch_to_y(&self, pitch: u8) -> Pixels {
        px((self.top_pitch as i32 - pitch as i32) as f32 * self.row_height)
    }

    /// Pitch whose row contains screen y, clamped to the MIDI range.
    pub fn y_to_pitch(&self, y: Pixels) -> u8 {
        if self.row_height <= 0.0 {
            return self.top_pitch;
        }
        let rows = (f32::from(y) / self.row_height).floor() as i32;
        (self.top_pitch as i32 - rows).clamp(0, Self::MAX_PITCH as i32) as u8
    }

    /// Zooms the pitch axis by `factor`, keeping the pitch under `anchor_y` in place.
    pub fn zoom_by(&mut self, factor: f32, anchor_y: Pixels) {
        let anchor_pitch = self.y_to_pitch(anchor_y);
        self.row_height =
            (self.row_height * factor).clamp(Self::MIN_ROW_HEIGHT, Self::MAX_ROW_HEIGHT);
        let rows_above = (f32::from(anchor_y) / self.row_height) as i32;
        self.top_pitch = (anchor_pitch as i32 + rows_above).clamp(0, Self::MAX_PITCH as i32) as u8;
    }

    /// Scrolls the pitch axis by a pixel delta.
    pub fn scroll_by(&mut self, delta_y: Pixels) {
        if self.row_height <= 0.0 {
            return;
        }
        let rows = (f32::from(delta_y) / self.row_height).round() as i32;
        self.top_pitch = (self.top_pitch as i32 + rows).clamp(0, Self::MAX_PITCH as i32) as u8;
    }

    /// Centres the view on `pitch` given a body `height` pixels tall.
    pub fn center_on(&mut self, pitch: u8, height: Pixels) {
        if self.row_height <= 0.0 {
            return;
        }
        let visible_rows = (f32::from(height) / self.row_height) as i32;
        let top = pitch as i32 + visible_rows / 2;
        self.top_pitch = top.clamp(0, Self::MAX_PITCH as i32) as u8;
    }
}

/// `true` when `pitch` is a black key, used to tint alternating rows.
pub fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// Scientific pitch name for a MIDI note, e.g. 60 is `C4`.
pub fn pitch_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    // MIDI 60 is middle C, named C4 in scientific pitch notation, so octave = pitch/12 - 1.
    let octave = pitch as i32 / 12 - 1;
    format!("{}{}", NAMES[(pitch % 12) as usize], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_and_x_round_trip() {
        let view = TimelineView {
            pixels_per_beat: 60.0,
            scroll_ticks: Ticks(1920),
        };
        for beat in 0..16 {
            let tick = Ticks(1920 + beat * TICKS_PER_QUARTER);
            assert_eq!(view.x_to_tick(view.tick_to_x(tick)), tick);
        }
        assert_eq!(view.tick_to_x(Ticks(1920)), px(0.0));
        assert_eq!(view.tick_to_x(Ticks(1920 + TICKS_PER_QUARTER)), px(60.0));
    }

    #[test]
    fn zoom_keeps_the_tick_under_the_pointer() {
        let mut view = TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks(TICKS_PER_QUARTER * 8),
        };
        let anchor = px(300.0);
        let before = view.x_to_tick(anchor);
        view.zoom_by(2.0, anchor);
        let after = view.x_to_tick(anchor);
        // One tick of slack: scroll is stored as an integer tick position.
        assert!((before.0 - after.0).abs() <= 1, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_is_clamped_at_both_ends() {
        let mut view = TimelineView::default();
        for _ in 0..40 {
            view.zoom_by(2.0, px(0.0));
        }
        assert_eq!(view.pixels_per_beat, TimelineView::MAX_PIXELS_PER_BEAT);
        for _ in 0..80 {
            view.zoom_by(0.5, px(0.0));
        }
        assert_eq!(view.pixels_per_beat, TimelineView::MIN_PIXELS_PER_BEAT);
    }

    #[test]
    fn scrolling_never_goes_before_the_start() {
        let mut view = TimelineView::default();
        view.scroll_by(px(-10_000.0));
        assert_eq!(view.scroll_ticks, Ticks::ZERO);
    }

    #[test]
    fn grid_coarsens_as_the_view_zooms_out() {
        let signature = TimeSignature::default();
        let zoomed_in = TimelineView {
            pixels_per_beat: 400.0,
            scroll_ticks: Ticks::ZERO,
        };
        let zoomed_out = TimelineView {
            pixels_per_beat: 8.0,
            scroll_ticks: Ticks::ZERO,
        };
        let fine = zoomed_in.grid_step(signature, px(8.0));
        let coarse = zoomed_out.grid_step(signature, px(8.0));
        assert!(fine < coarse, "{fine:?} should be finer than {coarse:?}");
        assert!(f32::from(zoomed_out.duration_to_width(coarse)) >= 8.0);
    }

    #[test]
    fn pitch_rows_map_both_ways() {
        let view = PitchView {
            row_height: 12.0,
            top_pitch: 84,
        };
        assert_eq!(view.pitch_to_y(84), px(0.0));
        assert_eq!(view.pitch_to_y(83), px(12.0));
        assert_eq!(view.y_to_pitch(px(0.0)), 84);
        assert_eq!(view.y_to_pitch(px(13.0)), 83);
        assert_eq!(view.y_to_pitch(px(100_000.0)), 0);
    }

    #[test]
    fn pitch_zoom_is_clamped_and_holds_its_anchor() {
        let mut view = PitchView::default();
        let anchor = px(120.0);
        let before = view.y_to_pitch(anchor);
        view.zoom_by(1.5, anchor);
        // One row of slack: the top pitch is stored as an integer.
        assert!((view.y_to_pitch(anchor) as i32 - before as i32).abs() <= 1);

        for _ in 0..40 {
            view.zoom_by(2.0, anchor);
        }
        assert_eq!(view.row_height, PitchView::MAX_ROW_HEIGHT);
        for _ in 0..80 {
            view.zoom_by(0.5, anchor);
        }
        assert_eq!(view.row_height, PitchView::MIN_ROW_HEIGHT);
    }

    #[test]
    fn centring_puts_the_pitch_in_the_middle() {
        let mut view = PitchView {
            row_height: 10.0,
            top_pitch: 127,
        };
        view.center_on(60, px(200.0));
        // 200 px of body is 20 rows, so the target should sit about ten rows down.
        assert_eq!(view.y_to_pitch(px(100.0)), 60);
    }

    #[test]
    fn revealing_a_tick_scrolls_the_minimum_needed() {
        let mut view = TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks::ZERO,
        };
        let width = px(480.0);
        // Already visible: nothing moves.
        view.scroll_to_reveal(Ticks(TICKS_PER_QUARTER), width);
        assert_eq!(view.scroll_ticks, Ticks::ZERO);

        let far = Ticks(TICKS_PER_QUARTER * 40);
        view.scroll_to_reveal(far, width);
        let (start, end) = view.visible_range(width);
        assert!(
            far >= start && far <= end,
            "{far:?} not in {start:?}..{end:?}"
        );
    }

    #[test]
    fn pitch_names_follow_scientific_notation() {
        assert_eq!(pitch_name(60), "C4");
        assert_eq!(pitch_name(69), "A4");
        assert_eq!(pitch_name(21), "A0");
        assert_eq!(pitch_name(61), "C#4");
    }

    #[test]
    fn black_keys_are_the_five_accidentals() {
        let black: Vec<u8> = (60..72).filter(|p| is_black_key(*p)).collect();
        assert_eq!(black, vec![61, 63, 66, 68, 70]);
    }
}
