//! Small reusable controls shared by the panels.
//!
//! These are plain functions rather than `RenderOnce` components. Callbacks arrive already
//! bound with `cx.listener(...)`, which keeps every widget free of view-type generics while
//! leaving all state in the view that owns it.

use gpui::{
    App, Axis, ClickEvent, ElementId, Hsla, IntoElement, MouseDownEvent, ScrollWheelEvent,
    SharedString, Window, div, prelude::*, px, relative,
};

use crate::theme::{Metrics, Theme};

/// A horizontal separator.
pub fn divider(theme: &Theme) -> impl IntoElement + use<> {
    div().h(px(1.0)).w_full().bg(theme.border_subtle)
}

/// Visual weight of a [`button`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Ordinary control.
    Normal,
    /// Filled with the accent colour; for the primary action in a dialog.
    Primary,
    /// Borderless, for dense toolbars.
    Ghost,
}

/// A clickable button.
///
/// `active` fills the button with `active_color` — used for latched controls such as mute,
/// solo and loop, where the button reflects state rather than just being pressed.
pub fn button<I, L, F>(
    id: I,
    label: L,
    style: ButtonStyle,
    active: bool,
    active_color: Hsla,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, L, F>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let (background, text_color, border) = match (style, active) {
        (_, true) => (active_color, theme.text_on_accent, active_color),
        (ButtonStyle::Primary, false) => (theme.accent, theme.text_on_accent, theme.accent),
        (ButtonStyle::Normal, false) => (theme.surface_raised, theme.text, theme.border),
        (ButtonStyle::Ghost, false) => (
            gpui::transparent_black(),
            theme.text_muted,
            gpui::transparent_black(),
        ),
    };

    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .h(Metrics::CONTROL_HEIGHT)
        .px_2()
        .rounded_sm()
        .border_1()
        .border_color(border)
        .bg(background)
        .text_xs()
        .text_color(text_color)
        .cursor_pointer()
        .hover(|this| this.bg(Theme::lighten(background, 0.12)))
        .active(|this| this.opacity(0.75))
        .child(label.into())
        .on_click(on_click)
}

/// A square button sized for a single glyph, used in the transport bar.
pub fn glyph_button<I, G, F>(
    id: I,
    glyph: G,
    active: bool,
    active_color: Hsla,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, G, F>
where
    I: Into<ElementId>,
    G: Into<SharedString>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let background = if active {
        active_color
    } else {
        theme.surface_raised
    };
    let text_color = if active {
        theme.text_on_accent
    } else {
        theme.text
    };

    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.0))
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(background)
        .text_color(text_color)
        .cursor_pointer()
        .hover(|this| this.bg(Theme::lighten(background, 0.14)))
        .active(|this| this.opacity(0.75))
        .child(glyph.into())
        .on_click(on_click)
}

/// A horizontal drag-to-edit control: label on the left, filled bar, value on the right.
///
/// `fraction` is the fill amount in 0..1 — normalised by the caller through the parameter's own
/// curve, so a logarithmic frequency control fills linearly with the knob's travel.
///
/// The widget only reports *where a drag began*; the owning view tracks the pointer from there,
/// which is what lets a drag continue after the pointer leaves the bar.
#[allow(clippy::too_many_arguments)]
pub fn value_slider<I, L, V, D, S>(
    id: I,
    label: L,
    value_text: V,
    fraction: f32,
    fill: Hsla,
    theme: &Theme,
    on_drag_start: D,
    on_scroll: S,
) -> impl IntoElement + use<I, L, V, D, S>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    V: Into<SharedString>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    S: Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
{
    let fraction = fraction.clamp(0.0, 1.0);

    div()
        .id(id.into())
        .relative()
        .flex()
        .items_center()
        .h(Metrics::CONTROL_HEIGHT)
        .w_full()
        .rounded_sm()
        .overflow_hidden()
        .bg(theme.surface_sunken)
        .border_1()
        .border_color(theme.border_subtle)
        .cursor_pointer()
        .hover(|this| this.border_color(theme.border))
        // The fill sits behind the labels so the text stays readable at every position.
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(relative(fraction))
                .bg(Theme::translucent(fill, 0.45)),
        )
        .child(
            div()
                .relative()
                .flex()
                .items_center()
                .justify_between()
                .size_full()
                .px_1p5()
                .text_xs()
                .child(div().text_color(theme.text_muted).child(label.into()))
                .child(div().text_color(theme.text).child(value_text.into())),
        )
        .on_mouse_down(gpui::MouseButton::Left, on_drag_start)
        .on_scroll_wheel(on_scroll)
}

/// A read-only level meter.
///
/// `level` and `peak` are normalised 0..1 positions produced by [`db_to_meter_position`], which
/// applies a piecewise scale so the useful -60..0 dB range is not crushed into the top pixels.
pub fn level_meter(
    level: f32,
    peak: f32,
    axis: Axis,
    color: Hsla,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let level = level.clamp(0.0, 1.0);
    let peak = peak.clamp(0.0, 1.0);

    let fill = match axis {
        Axis::Vertical => div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(relative(level))
            .bg(color),
        Axis::Horizontal => div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(relative(level))
            .bg(color),
    };

    // A thin line held at the loudest recent level, so brief transients stay visible.
    let peak_marker = match axis {
        Axis::Vertical => div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(relative(peak))
            .h(px(1.5))
            .bg(theme.text),
        Axis::Horizontal => div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(peak))
            .w(px(1.5))
            .bg(theme.text),
    };

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .rounded_sm()
        .bg(theme.surface_sunken)
        .child(fill)
        .when(peak > 0.001, |this| this.child(peak_marker))
}

/// Maps a level in dBFS onto a 0..1 meter position.
///
/// Linear amplitude would put everything below -20 dB in the bottom 10 % of the meter, so this
/// uses the scale broadcast meters use: the top 12 dB gets half the travel, and the range
/// bottoms out at -60 dB.
pub fn db_to_meter_position(db: f32) -> f32 {
    if !db.is_finite() || db <= -60.0 {
        return 0.0;
    }
    let db = db.min(6.0);
    if db >= -12.0 {
        // -12..+6 dB occupies the upper half.
        0.5 + ((db + 12.0) / 18.0) * 0.5
    } else {
        // -60..-12 dB occupies the lower half.
        ((db + 60.0) / 48.0) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_scale_matches_its_documented_breakpoints() {
        assert_eq!(db_to_meter_position(-70.0), 0.0);
        assert_eq!(db_to_meter_position(-60.0), 0.0);
        assert!((db_to_meter_position(-36.0) - 0.25).abs() < 1e-6);
        assert!((db_to_meter_position(-12.0) - 0.5).abs() < 1e-6);
        assert!((db_to_meter_position(0.0) - 0.833).abs() < 1e-3);
        assert_eq!(db_to_meter_position(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn meter_scale_is_monotonic() {
        let mut previous = -1.0;
        let mut db = -60.0;
        while db <= 6.0 {
            let position = db_to_meter_position(db);
            assert!(position >= previous, "regressed at {db} dB");
            previous = position;
            db += 0.5;
        }
    }
}
