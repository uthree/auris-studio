//! Colours and metrics for the whole UI.
//!
//! Everything visual reads from one [`Theme`] value so the palette can be retuned — or a light
//! variant added — without hunting hex literals through the view code.

use gpui::{Font, FontFallbacks, Hsla, Pixels, px, rgb};

/// The font every panel draws in, with fallbacks for scripts the base family has no glyphs for.
///
/// The base family is each platform's own interface font, named rather than left to a
/// substitution table: asking Windows for Helvetica gets you Arial through a mapping written in
/// the nineties, which is close enough to look like nothing went wrong and far enough to look
/// unlike every other window on the screen.
///
/// None of the three covers Japanese, so a Japanese track name or menu would come out as empty
/// boxes without the fallbacks. They are listed for every platform at once because a family that
/// is not installed is simply skipped, which makes an unused entry free.
pub fn ui_font() -> Font {
    let base = if cfg!(target_os = "macos") {
        "Helvetica"
    } else if cfg!(target_os = "windows") {
        "Segoe UI"
    } else {
        "DejaVu Sans"
    };
    Font {
        fallbacks: Some(FontFallbacks::from_fonts(vec![
            // macOS
            "Hiragino Sans".into(),
            "Apple SD Gothic Neo".into(),
            // Windows
            "Segoe UI".into(),
            "Yu Gothic UI".into(),
            "Meiryo".into(),
            // Linux
            "Noto Sans CJK JP".into(),
            "DejaVu Sans".into(),
        ])),
        ..gpui::font(base)
    }
}

/// The application colour palette.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Window background, behind every panel.
    pub background: Hsla,
    /// Standard panel surface.
    pub surface: Hsla,
    /// Slightly raised surface, for headers and toolbars.
    pub surface_raised: Hsla,
    /// Recessed surface, for timeline and piano-roll backgrounds.
    pub surface_sunken: Hsla,
    /// Hover highlight.
    pub surface_hover: Hsla,
    /// Border between panels.
    pub border: Hsla,
    /// Softer border, for internal dividers.
    pub border_subtle: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Secondary text and inactive labels.
    pub text_muted: Hsla,
    /// Captions above a readout, and other text that should recede entirely.
    pub text_faint: Hsla,
    /// Text on an accent-filled surface.
    pub text_on_accent: Hsla,
    /// Interactive accent.
    pub accent: Hsla,
    /// Accent used for large filled areas.
    pub accent_soft: Hsla,
    /// Transport playing indicator.
    pub playing: Hsla,
    /// Playhead line.
    pub playhead: Hsla,
    /// Bar lines in the timeline grid.
    pub grid_bar: Hsla,
    /// Beat lines in the timeline grid.
    pub grid_beat: Hsla,
    /// Subdivision lines in the timeline grid.
    pub grid_subdivision: Hsla,
    /// Selection outline.
    pub selection: Hsla,
    /// Loop region tint.
    pub loop_region: Hsla,
    /// Meter fill below -12 dBFS.
    pub meter_low: Hsla,
    /// Meter fill between -12 and -3 dBFS.
    pub meter_mid: Hsla,
    /// Meter fill above -3 dBFS.
    pub meter_high: Hsla,
    /// Solo button when engaged.
    pub solo: Hsla,
    /// Mute button when engaged.
    pub mute: Hsla,
    /// White keys in the piano roll keyboard.
    pub key_white: Hsla,
    /// Black keys in the piano roll keyboard.
    pub key_black: Hsla,
    /// Lane background behind a black key's row.
    pub key_row_black: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The default dark palette.
    pub fn dark() -> Self {
        Self {
            background: rgb(0x14161b).into(),
            surface: rgb(0x1c1f26).into(),
            surface_raised: rgb(0x242832).into(),
            surface_sunken: rgb(0x101216).into(),
            surface_hover: rgb(0x2e333f).into(),
            border: rgb(0x343a49).into(),
            border_subtle: rgb(0x252932).into(),
            text: rgb(0xe6e9f0).into(),
            text_muted: rgb(0x8b93a7).into(),
            text_faint: rgb(0x666e80).into(),
            text_on_accent: rgb(0x0b0d11).into(),
            accent: rgb(0x4f9dde).into(),
            accent_soft: rgb(0x2b4d6b).into(),
            playing: rgb(0x5fc98b).into(),
            playhead: rgb(0xff6b5b).into(),
            grid_bar: rgb(0x3d4457).into(),
            grid_beat: rgb(0x2a2f3b).into(),
            grid_subdivision: rgb(0x1f232c).into(),
            selection: rgb(0x7dc4ff).into(),
            loop_region: rgb(0x3a5c7a).into(),
            meter_low: rgb(0x4fbf7f).into(),
            meter_mid: rgb(0xd8c04a).into(),
            meter_high: rgb(0xe05252).into(),
            solo: rgb(0xe8c05a).into(),
            mute: rgb(0xe07a4a).into(),
            key_white: rgb(0xd8dce6).into(),
            key_black: rgb(0x2a2e38).into(),
            key_row_black: rgb(0x171a20).into(),
        }
    }

    /// Colour for a meter or clip indicator at `level_db`.
    pub fn meter_color(&self, level_db: f32) -> Hsla {
        if level_db >= -3.0 {
            self.meter_high
        } else if level_db >= -12.0 {
            self.meter_mid
        } else {
            self.meter_low
        }
    }

    /// Converts a packed `0xRRGGBB` track colour into an `Hsla`.
    pub fn track_color(&self, packed: u32) -> Hsla {
        rgb(packed).into()
    }

    /// A translucent variant of `color`, for clip fills over a grid.
    pub fn translucent(color: Hsla, alpha: f32) -> Hsla {
        Hsla { a: alpha, ..color }
    }

    /// Blends `color` towards white by `amount` (0.0..1.0), for hover states.
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        Hsla {
            l: (color.l + (1.0 - color.l) * amount).clamp(0.0, 1.0),
            ..color
        }
    }
}

/// Fixed sizes shared across panels, so the timeline and its ruler stay aligned.
pub struct Metrics;

impl Metrics {
    /// Height of the top transport bar.
    ///
    /// Tall enough for the transport buttons and the readouts stacked under them, which is how
    /// Logic arranges the same controls.
    pub const TRANSPORT_HEIGHT: Pixels = px(84.0);
    /// Height of the timeline ruler above the arrangement.
    pub const RULER_HEIGHT: Pixels = px(28.0);
    /// Height of the harmony lane, between the ruler and the clip lanes.
    ///
    /// Two rows: a thin strip of key changes over a taller strip of chords. See
    /// [`paint::harmony_rows`](crate::ui::paint::harmony_rows) for why they are not one.
    pub const HARMONY_LANE_HEIGHT: Pixels = px(34.0);
    /// Everything above the clip lanes on the right, and the strip that matches it on the left.
    ///
    /// The track headers line up with their lanes only because the left column reserves exactly
    /// what the right column spends above them. Adding a lane on one side and not the other slides
    /// every header out of register with the track it names — a bug that reads as a paint glitch
    /// and that no test can see, because nothing here is ever rendered in one. Both sides read
    /// this, so they cannot disagree.
    ///
    /// Spelled out rather than summed because `Pixels` keeps its inner value private and there is
    /// no const arithmetic to be had. `the_two_columns_reserve_the_same_height_above_the_lanes`
    /// is what keeps the three numbers honest.
    pub const TIMELINE_HEADER_HEIGHT: Pixels = px(62.0);
    /// Width of the track header column.
    pub const TRACK_HEADER_WIDTH: Pixels = px(196.0);
    /// Width of the piano-roll keyboard.
    pub const KEYBOARD_WIDTH: Pixels = px(56.0);
    /// Height of one piano-roll note row at 100 % zoom.
    pub const NOTE_ROW_HEIGHT: Pixels = px(14.0);
    /// Width of the left-hand library panel.
    pub const LIBRARY_WIDTH: Pixels = px(240.0);
    /// Width of the right-hand inspector panel.
    pub const INSPECTOR_WIDTH: Pixels = px(300.0);
    /// Height of the bottom editor panel, including its own header strip.
    pub const EDITOR_HEIGHT: Pixels = px(280.0);
    /// Height of the header strip at the top of the bottom editor panel.
    pub const EDITOR_HEADER_HEIGHT: Pixels = px(22.0);
    /// Height of the status bar along the bottom of the window.
    pub const STATUS_HEIGHT: Pixels = px(22.0);
    /// Height of a control row inside the inspector.
    pub const CONTROL_HEIGHT: Pixels = px(22.0);

    /// Corner radius for small controls: buttons, sliders, meters.
    pub const RADIUS_SM: Pixels = px(4.0);
    /// Corner radius for panels, clips and readouts.
    pub const RADIUS_MD: Pixels = px(6.0);
    /// Corner radius for floating surfaces such as the export sheet.
    pub const RADIUS_LG: Pixels = px(10.0);
    /// Corner radius for notes and other very small marks.
    pub const RADIUS_XS: Pixels = px(2.5);

    /// Thickness of a draggable panel divider — the grab zone, not the drawn line.
    pub const SPLITTER: Pixels = px(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_columns_reserve_the_same_height_above_the_lanes() {
        // The track headers line up with their lanes only because the left column reserves
        // exactly what the ruler and the harmony lane spend on the right. Nothing renders in a
        // test, so this arithmetic is the only place the misalignment can be caught before a
        // person sees every header sitting twenty-two pixels off the track it names.
        assert_eq!(
            Metrics::TIMELINE_HEADER_HEIGHT,
            Metrics::RULER_HEIGHT + Metrics::HARMONY_LANE_HEIGHT
        );
    }

    #[test]
    fn meter_colour_changes_at_the_documented_thresholds() {
        let theme = Theme::dark();
        assert_eq!(theme.meter_color(-20.0), theme.meter_low);
        assert_eq!(theme.meter_color(-6.0), theme.meter_mid);
        assert_eq!(theme.meter_color(-1.0), theme.meter_high);
    }

    #[test]
    fn the_base_family_is_one_this_platform_ships() {
        // Asking for a family the system does not have lands the whole interface on whatever
        // the substitution table picks, which is a thing you only notice in a screenshot.
        let font = ui_font();
        let expected = if cfg!(target_os = "macos") {
            "Helvetica"
        } else if cfg!(target_os = "windows") {
            "Segoe UI"
        } else {
            "DejaVu Sans"
        };
        assert_eq!(font.family.as_ref(), expected);
        assert!(
            font.fallbacks
                .as_ref()
                .is_some_and(|fallbacks| fallbacks.fallback_list().len() >= 5),
            "the Japanese fallbacks are what keep track names from being empty boxes"
        );
    }
}
