//! Small reusable controls shared by the panels.
//!
//! These are plain functions rather than `RenderOnce` components. Callbacks arrive already
//! bound with `cx.listener(...)`, which keeps every widget free of view-type generics while
//! leaving all state in the view that owns it.

use gpui::{
    App, Axis, ClickEvent, ElementId, Hsla, IntoElement, MouseDownEvent, Pixels, SharedString,
    Window, div, prelude::*, px, relative,
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

/// How far a latched [`button`] is latched.
///
/// Two states are enough for mute, solo and loop — a thing is on or it is not. The arm button
/// needs a third, because where a take would land is not the same question as which track somebody
/// armed: an audio track that is merely selected is where Record would go, and it has to look
/// different both from the track that was chosen by hand and from a track that is neither.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Latch {
    /// Not latched; the button's ordinary appearance for its style.
    Off,
    /// Outlined rather than filled: this is what would happen, but nobody said so.
    Ready,
    /// Filled with the latch colour: this was chosen.
    On,
}

impl From<bool> for Latch {
    fn from(active: bool) -> Self {
        match active {
            true => Latch::On,
            false => Latch::Off,
        }
    }
}

/// A clickable button.
///
/// `active` fills the button with `active_color` — used for latched controls such as mute,
/// solo and loop, where the button reflects state rather than just being pressed. It takes a
/// plain `bool` for those; see [`Latch`] for the third state the arm button needs.
pub fn button<I, L, A, F>(
    id: I,
    label: L,
    style: ButtonStyle,
    active: A,
    active_color: Hsla,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, L, A, F>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    A: Into<Latch>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let active = active.into();
    let (background, text_color, border) = match (style, active) {
        // Read against whatever it is latched to — mute is orange, solo is amber, a track's
        // colour is whatever the user chose — and not against the accent, which is behind none
        // of them.
        (_, Latch::On) => (active_color, theme.text_on(active_color), active_color),
        // The outline carries the colour and the surface stays where it was, so a ready button
        // reads as the same button rather than as a second latched one in a row of them.
        (_, Latch::Ready) => (theme.surface_raised, active_color, active_color),
        (ButtonStyle::Primary, Latch::Off) => (theme.accent, theme.text_on_accent, theme.accent),
        (ButtonStyle::Normal, Latch::Off) => (theme.surface_raised, theme.text, theme.border),
        (ButtonStyle::Ghost, Latch::Off) => (
            gpui::transparent_black(),
            theme.text_muted,
            gpui::transparent_black(),
        ),
    };
    // A Ghost button's background is transparent, and lightening transparency yields more
    // transparency: its hover painted a quad with alpha 0, so the only entry point to the
    // instrument editor in the whole application looked exactly like the label beside it.
    let hover = if matches!((style, active), (ButtonStyle::Ghost, Latch::Off)) {
        theme.surface_hover
    } else {
        theme.hovered(background, 0.12)
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
        .hover(|this| this.bg(hover))
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
        theme.text_on(active_color)
    } else {
        theme.text
    };
    let hover = theme.hovered(background, 0.14);

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
        .hover(|this| this.bg(hover))
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

/// Which half of a labelled row is a fixed column. The other half takes what is left.
///
/// The two shapes a row of "this is what it is, press to choose another" comes in, and the choice
/// is about the panel rather than about the control. Both are drawn by [`picker_row`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RowColumn {
    /// The label is a column this wide. For a panel of a fixed width whose values are long — a
    /// key, a groove, a progression — where the button wants every pixel the label does not.
    Label(Pixels),
    /// The value button is a column this wide. For a panel the user can resize, where buttons
    /// that all end in the same place read as a column and a label free to stretch never
    /// squeezes one.
    Value(Pixels),
}

/// A labelled row whose value is a button: this is what it is, press to choose another.
///
/// Every picker in the application, from a plugin's discrete parameters to the song sheet's key.
/// One shape deliberately: they all ask the same thing, and a second shape for the same idea
/// would only be a second thing to learn.
///
/// `label` and `value` arrive already translated, as [`button`]'s do. `active` fills the button
/// with the accent, for the rows that are a switch rather than a menu.
pub fn picker_row<I, L, V, F>(
    id: I,
    label: L,
    value: V,
    column: RowColumn,
    active: bool,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, L, V, F>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    V: Into<SharedString>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    // Truncating rather than wrapping or pushing: the row is one control high, and a label long
    // enough to need a second line would take the button off the end of it.
    let caption = div()
        .text_xs()
        .text_color(theme.text_muted)
        .truncate()
        .child(label.into());
    let control = button(
        id,
        value.into(),
        ButtonStyle::Normal,
        active,
        theme.accent,
        theme,
        on_click,
    );

    let row = div()
        .flex()
        .items_center()
        .gap_2()
        .h(Metrics::CONTROL_HEIGHT);
    match column {
        RowColumn::Label(width) => row
            .child(caption.w(width))
            .child(div().flex_1().min_w_0().child(control)),
        RowColumn::Value(width) => row
            .child(caption.flex_1().min_w_0())
            .child(div().w(width).child(control)),
    }
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

/// How far a drag travels before a [`value_slider`] has been swept end to end, in pixels.
///
/// Wide enough that a parameter can be dialled in precisely, short enough that sweeping a filter
/// end to end does not need two swipes. It belongs to the widget rather than to any one panel:
/// a plugin's parameters, a clip's dials and the song sheet's are the same bar, and a plugin that
/// answered a drag twice as fast as the sheet did would be a bar that means something different
/// depending on where it was drawn.
pub const DRAG_RANGE_PIXELS: f32 = 220.0;

/// Where a drag that began at `start` and has travelled `delta` pixels leaves a slider.
///
/// In the bar's own 0..1, so a caller with a range or a curve applies it afterwards.
pub fn dragged(start: f32, delta: f32) -> f32 {
    (start + delta / DRAG_RANGE_PIXELS).clamp(0.0, 1.0)
}

/// How short a scrollbar's thumb may get, as a fraction of its track.
///
/// A thumb is sized by how much of the content is on screen, and with enough content that
/// honest fraction is a few pixels — something that says where you are and cannot be taken hold
/// of. Below this the thumb stops shrinking and starts lying slightly about the proportion,
/// which is the trade every scrollbar makes.
const MIN_THUMB: f32 = 0.12;

/// Where a scrollbar's thumb sits, as fractions of its track: where it starts, and how long.
///
/// `offset` and `max_offset` are gpui's own: a scroll container's offset is zero at the start and
/// `-max_offset` at the end, because it measures how far the *content* has been moved rather than
/// how far the view has travelled. `viewport` is the visible extent, so `viewport + max_offset` is
/// the whole content and the thumb is as long a share of the track as the view is of the content.
///
/// `None` when there is nothing to scroll, which is what says the bar should not be drawn at all —
/// a scrollbar with a full-length thumb is furniture that answers no question.
pub fn scrollbar_thumb(offset: f32, max_offset: f32, viewport: f32) -> Option<(f32, f32)> {
    if max_offset <= 0.0 || viewport <= 0.0 {
        return None;
    }
    let length = (viewport / (viewport + max_offset)).clamp(MIN_THUMB, 1.0);
    let travelled = (-offset / max_offset).clamp(0.0, 1.0);
    Some((travelled * (1.0 - length), length))
}

/// The offset a thumb dragged `delta` pixels from where it was grabbed asks for.
///
/// The thumb only travels the part of the track it does not itself occupy, so a pixel of pointer
/// buys more than a pixel of content — by exactly the factor that makes the far end of the track
/// the far end of the content. Measured from `grabbed_at`, the offset when the drag began, so the
/// gesture is one rewrite and a drag past the end and back leaves the view where it was.
pub fn scrollbar_dragged(grabbed_at: f32, delta: f32, max_offset: f32, viewport: f32) -> f32 {
    let Some((_, length)) = scrollbar_thumb(grabbed_at, max_offset, viewport) else {
        return grabbed_at;
    };
    let track = viewport * (1.0 - length);
    if track <= 0.0 {
        return grabbed_at;
    }
    (grabbed_at - delta * max_offset / track).clamp(-max_offset, 0.0)
}

/// The offset a press `fraction` of the way along a scrollbar asks for.
///
/// A press on the thumb takes hold of it and moves nothing, which is what makes the drag that
/// follows feel attached to it. A press anywhere else jumps the thumb to the pointer first: with
/// forty channel strips the alternative is dragging the whole way, and the one gesture a
/// scrollbar exists to offer is going somewhere directly.
pub fn scrollbar_pressed(fraction: f32, offset: f32, max_offset: f32, viewport: f32) -> f32 {
    let Some((start, length)) = scrollbar_thumb(offset, max_offset, viewport) else {
        return offset;
    };
    if (start..start + length).contains(&fraction) {
        return offset;
    }
    let travelled = ((fraction - length / 2.0) / (1.0 - length)).clamp(0.0, 1.0);
    -travelled * max_offset
}

/// How thick a scrollbar is drawn.
pub const SCROLLBAR_THICKNESS: Pixels = px(9.0);

/// A horizontal scrollbar for a container that scrolls sideways.
///
/// Drawn as elements rather than painted on a canvas so that the thumb is a real hitbox: the
/// press has to be told apart from a press on the track beside it, and comparing pointer
/// positions against a remembered rectangle is the thing [`crate::app::CanvasBounds`] exists
/// because the application kept getting wrong.
///
/// The caller passes gpui's own `offset` and `max_offset` and is handed back the *fraction* a
/// press landed at; what that means for the offset is [`scrollbar_pressed`]'s to say.
pub fn horizontal_scrollbar<I, F>(
    id: I,
    offset: f32,
    max_offset: f32,
    viewport: f32,
    theme: &Theme,
    on_press: F,
) -> impl IntoElement + use<I, F>
where
    I: Into<ElementId>,
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let thumb = scrollbar_thumb(offset, max_offset, viewport);
    div()
        .id(id)
        .w_full()
        .flex_shrink_0()
        // Height and all, only when there is something to scroll: a bar that is always there is
        // a strip of the panel spent on saying that the panel fits.
        .when_some(thumb, |track, (start, length)| {
            track
                .h(SCROLLBAR_THICKNESS)
                .bg(theme.surface_sunken)
                .child(
                    div()
                        .absolute()
                        .left(relative(start))
                        .w(relative(length))
                        .h(SCROLLBAR_THICKNESS)
                        .rounded(Metrics::RADIUS_XS)
                        .bg(theme.border),
                )
                .on_mouse_down(gpui::MouseButton::Left, on_press)
        })
        .relative()
}

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
///
/// # Why there is no wheel
///
/// A bar is dragged, and that is the whole of how it is edited. It used to take a scroll handler
/// too, and every one of these lives inside a panel that scrolls — the mixer strips, the
/// inspector, the song sheet. Rolling the wheel to get down a column of tracks changed the level
/// of whichever fader the pointer happened to cross on the way, silently, with no drag to
/// remember having started and nothing on the screen saying which of them moved.
///
/// The handler is gone from the widget rather than from its callers so that it cannot come back
/// one control at a time: there is no parameter to pass one to. A view that wants the wheel to
/// mean something puts it on the region being scrolled, which is where a wheel belongs.
#[allow(clippy::too_many_arguments)]
pub fn value_slider<I, L, V, D>(
    id: I,
    label: L,
    value_text: V,
    fraction: f32,
    fill: Hsla,
    origin: SliderFill,
    theme: &Theme,
    on_drag_start: D,
) -> impl IntoElement + use<I, L, V, D>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    V: Into<SharedString>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
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
}

/// A compact slider for a view setting, with no label and no readout.
///
/// Deliberately not [`value_slider`]: that one names a parameter and prints its value, which is
/// what a plugin control needs and what a strip of window chrome has no room for. This is a bare
/// track and a thumb, sized so a full sweep of it covers the whole range it drives.
///
/// No wheel, for the reason [`value_slider`] has none. This one sits in window chrome rather than
/// in anything that scrolls, so it was never the control the accident happened on — but a rule
/// with one bar exempted is a rule the next bar gets exempted from too, and the wheel over a zoom
/// slider is the same unasked-for change as the wheel over a fader. Zooming by wheel still works
/// where it always did, over the timeline and the roll.
pub fn zoom_slider<I, D>(
    id: I,
    fraction: f32,
    theme: &Theme,
    on_drag_start: D,
) -> impl IntoElement + use<I, D>
where
    I: Into<ElementId>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
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
    fn a_full_sweep_of_a_bar_is_the_drag_range_and_stops_at_both_ends() {
        // Every bar in the application is dragged through this, so the number here is what makes
        // a plugin's filter and the song sheet's tempo answer a hand at the same rate.
        assert!((dragged(0.0, DRAG_RANGE_PIXELS) - 1.0).abs() < 1e-6);
        assert!((dragged(1.0, -DRAG_RANGE_PIXELS)).abs() < 1e-6);
        assert!((dragged(0.5, DRAG_RANGE_PIXELS / 2.0) - 1.0).abs() < 1e-6);
        // Past either end the bar stays where it is rather than running off it, so a drag that
        // overshot comes back the moment the pointer does.
        assert_eq!(dragged(0.5, DRAG_RANGE_PIXELS * 10.0), 1.0);
        assert_eq!(dragged(0.5, -DRAG_RANGE_PIXELS * 10.0), 0.0);
        assert_eq!(dragged(0.25, 0.0), 0.25);
    }

    #[test]
    fn a_scrollbar_thumb_is_as_much_of_the_track_as_the_view_is_of_the_content() {
        // Nothing to scroll: no bar. A full-length thumb is furniture, and drawing one would
        // take a strip of the mixer away from the strips in every project small enough to fit.
        assert_eq!(scrollbar_thumb(0.0, 0.0, 400.0), None);

        // Four hundred pixels of view over twelve hundred of content: a third of the track.
        let (start, length) = scrollbar_thumb(0.0, 800.0, 400.0).expect("scrollable");
        assert!((length - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(
            start, 0.0,
            "at the top of the content, at the top of the bar"
        );

        // And at the far end the thumb's *end* is the end of the track, not its start.
        let (start, length) = scrollbar_thumb(-800.0, 800.0, 400.0).expect("scrollable");
        assert!((start + length - 1.0).abs() < 1e-6);

        // A thumb never shrinks past the point of being grabbable, however long the content.
        let (_, length) = scrollbar_thumb(0.0, 100_000.0, 400.0).expect("scrollable");
        assert_eq!(length, MIN_THUMB);
    }

    #[test]
    fn dragging_a_thumb_to_the_end_of_its_track_reaches_the_end_of_the_content() {
        // The thumb travels the track less its own length, so that much pointer has to buy the
        // whole of `max_offset`. Getting this factor wrong is invisible until the last strip
        // turns out to be unreachable.
        let (_, length) = scrollbar_thumb(0.0, 800.0, 400.0).expect("scrollable");
        let travel = 400.0 * (1.0 - length);
        assert_eq!(scrollbar_dragged(0.0, travel, 800.0, 400.0), -800.0);
        assert_eq!(scrollbar_dragged(-800.0, -travel, 800.0, 400.0), 0.0);
        // Past either end it stops rather than running off, so overshooting and coming back
        // leaves the view where the pointer says it is.
        assert_eq!(scrollbar_dragged(0.0, travel * 10.0, 800.0, 400.0), -800.0);
        assert_eq!(scrollbar_dragged(0.0, -travel, 800.0, 400.0), 0.0);
    }

    #[test]
    fn a_press_on_the_thumb_holds_it_and_a_press_beside_it_jumps() {
        // At rest the thumb covers the first third of the track.
        assert_eq!(scrollbar_pressed(0.1, 0.0, 800.0, 400.0), 0.0);
        // Past its end, and the thumb comes to the pointer — centred on it, so what was pressed
        // is what ends up under the pointer to be dragged from.
        assert_eq!(scrollbar_pressed(1.0, 0.0, 800.0, 400.0), -800.0);
        assert_eq!(scrollbar_pressed(0.0, -800.0, 800.0, 400.0), 0.0);
        // And a bar with nothing to scroll answers no press at all.
        assert_eq!(scrollbar_pressed(0.5, 0.0, 0.0, 400.0), 0.0);
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
