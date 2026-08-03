//! Small reusable controls shared by the panels.
//!
//! These are plain functions rather than `RenderOnce` components. Callbacks arrive already
//! bound with `cx.listener(...)`, which keeps every widget free of view-type generics while
//! leaving all state in the view that owns it.

use gpui::{
    App, Axis, ClickEvent, ElementId, Hsla, IntoElement, MouseDownEvent, Pixels, ScrollWheelEvent,
    SharedString, Window, div, prelude::*, px, relative,
};

use crate::theme::{Metrics, Theme};
use crate::ui::icons::{Icon, icon};

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
        .rounded(Metrics::RADIUS_SM)
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

/// A square button holding one drawn icon, used in the transport bar.
///
/// The icon is painted rather than typed so a row of them shares one weight and colour; see
/// [`crate::ui::icons`].
pub fn icon_button<I, F>(
    id: I,
    glyph: Icon,
    active: bool,
    active_color: Hsla,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, F>
where
    I: Into<ElementId>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let background = if active {
        active_color
    } else {
        theme.surface_raised
    };
    let foreground = if active {
        theme.text_on_accent
    } else {
        theme.text
    };

    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.0))
        .rounded(Metrics::RADIUS_MD)
        .border_1()
        .border_color(if active { active_color } else { theme.border })
        .bg(background)
        .cursor_pointer()
        .hover(|this| this.bg(Theme::lighten(background, 0.14)))
        .active(|this| this.opacity(0.75))
        .child(icon(glyph, px(15.0), foreground))
        .on_click(on_click)
}

/// A button showing an icon next to a word, for actions where the icon alone is ambiguous.
pub fn icon_label<I, L, F>(
    id: I,
    glyph: Icon,
    label: L,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, L, F>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .gap_1()
        .h(Metrics::CONTROL_HEIGHT)
        .px_1p5()
        .rounded(Metrics::RADIUS_SM)
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_raised)
        .text_xs()
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|this| this.bg(theme.surface_hover))
        .active(|this| this.opacity(0.75))
        .child(icon(glyph, px(10.0), theme.text_muted))
        .child(label.into())
        .on_click(on_click)
}

/// A small square icon button for repeated row actions, such as reordering an effect chain.
pub fn chain_button<I, F>(
    id: I,
    glyph: Icon,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, F>
where
    I: Into<ElementId>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .rounded(Metrics::RADIUS_SM)
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_subtle)
        .cursor_pointer()
        .hover(|this| this.bg(theme.surface_hover).border_color(theme.border))
        .active(|this| this.opacity(0.75))
        .child(icon(glyph, px(11.0), theme.text_muted))
        .on_click(on_click)
}

/// A draggable divider between two panels.
///
/// The whole strip is the grab zone and the line inside it is what you see. gpui hit-tests
/// against an element's own bounds, so a one-pixel divider would be a one-pixel target — the
/// strip is deliberately several pixels wide and the panels sit either side of it.
pub fn splitter<I, F>(
    id: I,
    axis: Axis,
    theme: &Theme,
    on_drag_start: F,
) -> impl IntoElement + use<I, F>
where
    I: Into<ElementId>,
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let line = div().flex_shrink_0().bg(theme.border);
    let base = div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .cursor(match axis {
            Axis::Vertical => gpui::CursorStyle::ResizeLeftRight,
            Axis::Horizontal => gpui::CursorStyle::ResizeUpDown,
        })
        // The line brightens on hover, so it is obvious the strip is draggable before the
        // pointer has to be tried on it.
        .hover(|this| this.bg(Theme::translucent(theme.accent, 0.35)));

    match axis {
        Axis::Vertical => base
            .w(Metrics::SPLITTER)
            .h_full()
            .child(line.w(px(1.0)).h_full()),
        Axis::Horizontal => base
            .h(Metrics::SPLITTER)
            .w_full()
            .child(line.h(px(1.0)).w_full()),
    }
    .on_mouse_down(gpui::MouseButton::Left, on_drag_start)
}

/// A readout with a caption above it, as a hardware transport displays one.
pub fn readout<C: Into<SharedString>, V: Into<SharedString>>(
    caption: C,
    value: V,
    sub: Option<SharedString>,
    width: Pixels,
    theme: &Theme,
) -> impl IntoElement + use<C, V> {
    div()
        .flex()
        .flex_col()
        .justify_center()
        .w(width)
        .px_2()
        .py_1()
        .rounded(Metrics::RADIUS_MD)
        .bg(theme.surface_sunken)
        .border_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .text_xs()
                .text_color(theme.text_faint)
                .child(caption.into()),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_1()
                .child(div().text_sm().text_color(theme.text).child(value.into()))
                .children(sub.map(|sub| div().text_xs().text_color(theme.text_muted).child(sub))),
        )
}

/// How wide a zoom slider is drawn, and therefore how far a full sweep of it drags.
pub const ZOOM_SLIDER_WIDTH: Pixels = px(96.0);

/// Where a slider's fill grows from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SliderFill {
    /// Fill grows from the left edge, for a value with a natural floor such as a level.
    FromStart,
    /// Fill grows outward from the centre, for a value centred on zero such as pan or detune.
    ///
    /// A left-anchored bar shows a centred pan as half full, which reads as "half of
    /// something" rather than "no offset"; growing from the middle shows nothing at all,
    /// which is what centred means.
    FromCentre,
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
    origin: SliderFill,
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
    let (fill_start, fill_width) = match origin {
        SliderFill::FromStart => (0.0, fraction),
        SliderFill::FromCentre => (fraction.min(0.5), (fraction - 0.5).abs()),
    };

    // The name lives outside the bar. When it sat inside, the fill boundary and the thumb cut
    // straight through the words — "Unison Spread" read as "Unison|Spread" — and the label was
    // the hardest thing on the control to read at exactly the moment it mattered.
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap_1p5()
        .h(Metrics::CONTROL_HEIGHT)
        .w_full()
        .cursor_pointer()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(theme.text_muted)
                .truncate()
                .child(label.into()),
        )
        .child(
            div()
                .relative()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .h_full()
                .rounded(Metrics::RADIUS_SM)
                .overflow_hidden()
                .bg(theme.surface_sunken)
                .border_1()
                .border_color(theme.border_subtle)
                .hover(|this| this.border_color(theme.border))
                .child(
                    div()
                        .absolute()
                        .left(relative(fill_start))
                        .top_0()
                        .bottom_0()
                        .w(relative(fill_width))
                        .bg(Theme::translucent(fill, 0.28)),
                )
                // A hairline at the centre, so "how far from zero" is readable at a glance.
                .when(origin == SliderFill::FromCentre, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(0.5))
                            .w(px(1.0))
                            .bg(theme.border),
                    )
                })
                // Two ticks rather than a full-height line: the value sits in the middle of
                // this bar, and a line through it turned "-6.0 dB" into "-6.0|dB". Marking
                // only the top and bottom edges reads just as precisely and never crosses text.
                .when(fraction > 0.004, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .h(px(5.0))
                            .left(relative(fraction))
                            .w(px(2.0))
                            .bg(fill),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .h(px(5.0))
                            .left(relative(fraction))
                            .w(px(2.0))
                            .bg(fill),
                    )
                })
                .child(
                    div()
                        .relative()
                        .flex()
                        .justify_end()
                        .w_full()
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.text)
                        .child(value_text.into()),
                ),
        )
        .on_mouse_down(gpui::MouseButton::Left, on_drag_start)
        .on_scroll_wheel(on_scroll)
}

/// A compact slider for a view setting, with no label and no readout.
///
/// Deliberately not [`value_slider`]: that one names a parameter and prints its value, which is
/// what a plugin control needs and what a strip of window chrome has no room for. This is a bare
/// track and a thumb, sized so a full sweep of it covers the whole range it drives.
pub fn zoom_slider<I, D, S>(
    id: I,
    fraction: f32,
    theme: &Theme,
    on_drag_start: D,
    on_scroll: S,
) -> impl IntoElement + use<I, D, S>
where
    I: Into<ElementId>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    S: Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
{
    let fraction = fraction.clamp(0.0, 1.0);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .w(ZOOM_SLIDER_WIDTH)
        .flex_shrink_0()
        .h(Metrics::CONTROL_HEIGHT)
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w_full()
                .h(px(4.0))
                .rounded(Metrics::RADIUS_SM)
                .bg(theme.surface_sunken)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative(fraction))
                        .rounded(Metrics::RADIUS_SM)
                        .bg(Theme::translucent(theme.accent, 0.5)),
                )
                .child(
                    // Inset by the thumb's own width so it stays inside the track at both ends
                    // rather than hanging off them.
                    div()
                        .absolute()
                        .top(px(-3.0))
                        .left(relative(fraction * 0.94))
                        .w(px(6.0))
                        .h(px(10.0))
                        .rounded(Metrics::RADIUS_SM)
                        .bg(theme.accent),
                ),
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
        .rounded(Metrics::RADIUS_XS)
        .bg(theme.surface_sunken)
        .border_1()
        .border_color(theme.border_subtle)
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
