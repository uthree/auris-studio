//! Generic parameter editing for any plugin.
//!
//! Because [`ParamDescriptor`] carries a plugin's whole control surface — range, curve, unit and
//! any discrete steps — the UI can build an editor for a plugin it has never seen. Adding a new
//! synth or effect therefore costs zero UI code: register it and its controls appear.

use auris_core::param::{ParamDescriptor, ParamUnit};
use gpui::{
    App, ClickEvent, ElementId, Hsla, IntoElement, MouseDownEvent, ScrollWheelEvent, SharedString,
    Window, div, prelude::*, px,
};

use crate::theme::{Metrics, Theme};
use crate::ui::widgets::{ButtonStyle, button, value_slider};

/// Which control shape suits a parameter.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamControl {
    /// A drag-to-edit bar; the common case.
    Slider,
    /// An on/off button.
    Toggle,
    /// A button that cycles through labelled options.
    Choice,
}

/// Chooses the control shape for a parameter.
pub fn control_for(descriptor: &ParamDescriptor) -> ParamControl {
    match descriptor.unit {
        ParamUnit::Toggle => ParamControl::Toggle,
        ParamUnit::Choice => ParamControl::Choice,
        _ => ParamControl::Slider,
    }
}

/// How far a full-scale drag has to travel, in pixels.
///
/// Wide enough that a parameter can be dialled in precisely, short enough that sweeping a filter
/// end to end does not need two swipes.
pub const DRAG_RANGE_PIXELS: f32 = 220.0;

/// How much one wheel notch moves a parameter, as a fraction of its full range.
pub const SCROLL_STEP: f32 = 0.02;

/// A drag-to-edit row for a continuous parameter.
#[allow(clippy::too_many_arguments)]
pub fn slider_row<I, D, S>(
    id: I,
    descriptor: &ParamDescriptor,
    value: f32,
    fill: Hsla,
    theme: &Theme,
    on_drag_start: D,
    on_scroll: S,
) -> impl IntoElement + use<I, D, S>
where
    I: Into<ElementId>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    S: Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
{
    value_slider(
        id,
        descriptor.name.to_string(),
        descriptor.format(value),
        descriptor.normalize(value),
        fill,
        theme,
        on_drag_start,
        on_scroll,
    )
}

/// A button row for a toggle or choice parameter.
pub fn button_row<I, F>(
    id: I,
    descriptor: &ParamDescriptor,
    value: f32,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<I, F>
where
    I: Into<ElementId>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let engaged = matches!(control_for(descriptor), ParamControl::Toggle) && value >= 0.5;
    div()
        .flex()
        .items_center()
        .gap_2()
        .h(Metrics::CONTROL_HEIGHT)
        .child(
            div()
                .flex_1()
                .text_xs()
                .text_color(theme.text_muted)
                .child(descriptor.name.to_string()),
        )
        .child(div().w(px(104.0)).child(button(
            id,
            descriptor.format(value),
            ButtonStyle::Normal,
            engaged,
            theme.accent,
            theme,
            on_click,
        )))
}

/// The value a choice or toggle parameter takes after one click.
///
/// Wrapping at the end is what makes a two-option choice behave like a toggle and a four-option
/// one cycle, without the caller needing to know which it has.
pub fn next_discrete_value(descriptor: &ParamDescriptor, current: f32) -> f32 {
    let steps = descriptor.steps.unwrap_or(2).max(2);
    let span = descriptor.max - descriptor.min;
    if span.abs() < f32::EPSILON {
        return descriptor.min;
    }
    let quantum = span / (steps - 1) as f32;
    let index = ((current - descriptor.min) / quantum).round() as i64;
    let next = (index + 1).rem_euclid(steps as i64);
    descriptor.clamp(descriptor.min + next as f32 * quantum)
}

/// The value after dragging `delta_pixels` horizontally from `start_value`.
///
/// The drag moves the *normalised* position, so a logarithmic frequency control keeps a
/// constant musical interval per pixel across its whole range.
pub fn value_after_drag(descriptor: &ParamDescriptor, start_value: f32, delta_pixels: f32) -> f32 {
    let start = descriptor.normalize(start_value);
    descriptor.denormalize(start + delta_pixels / DRAG_RANGE_PIXELS)
}

/// The value after one wheel notch.
pub fn value_after_scroll(descriptor: &ParamDescriptor, current: f32, notches: f32) -> f32 {
    let position = descriptor.normalize(current);
    descriptor.denormalize(position + notches * SCROLL_STEP)
}

/// A compact heading for a plugin in the inspector, with a bypass button.
pub fn plugin_header<I, N, F>(
    id: I,
    name: N,
    enabled: bool,
    theme: &Theme,
    on_toggle: F,
) -> impl IntoElement + use<I, N, F>
where
    I: Into<ElementId>,
    N: Into<SharedString>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(24.0))
        .child(
            div()
                .text_xs()
                .text_color(if enabled {
                    theme.text
                } else {
                    theme.text_muted
                })
                .child(name.into()),
        )
        .child(div().w(px(46.0)).child(button(
            id,
            if enabled { "On" } else { "Byp" },
            ButtonStyle::Normal,
            enabled,
            theme.accent,
            theme,
            on_toggle,
        )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::param::ParamValueCurve;

    fn frequency() -> ParamDescriptor {
        ParamDescriptor::hertz(0u32, "freq", "Frequency", 20.0, 20_000.0, 1_000.0)
    }

    #[test]
    fn dragging_moves_in_normalised_space() {
        let descriptor = frequency();
        // Half the drag range should move half the control's travel, which on a logarithmic
        // parameter multiplies the value by sqrt(max/min) rather than adding a fixed amount.
        let value = value_after_drag(&descriptor, 20.0, DRAG_RANGE_PIXELS / 2.0);
        let expected = 20.0 * (20_000.0f32 / 20.0).sqrt();
        assert!(
            (value / expected - 1.0).abs() < 1e-3,
            "got {value}, expected {expected}"
        );
    }

    #[test]
    fn dragging_is_clamped_to_the_range() {
        let descriptor = frequency();
        assert_eq!(value_after_drag(&descriptor, 1_000.0, 10_000.0), 20_000.0);
        assert_eq!(value_after_drag(&descriptor, 1_000.0, -10_000.0), 20.0);
    }

    #[test]
    fn a_toggle_flips_on_each_click() {
        let descriptor = ParamDescriptor::toggle(0u32, "on", "On", false);
        assert_eq!(next_discrete_value(&descriptor, 0.0), 1.0);
        assert_eq!(next_discrete_value(&descriptor, 1.0), 0.0);
    }

    #[test]
    fn a_choice_cycles_and_wraps() {
        const MODES: [std::borrow::Cow<'static, str>; 3] = [
            std::borrow::Cow::Borrowed("A"),
            std::borrow::Cow::Borrowed("B"),
            std::borrow::Cow::Borrowed("C"),
        ];
        let descriptor =
            ParamDescriptor::new(0u32, "mode", "Mode", 0.0, 1.0, 0.0).with_choices(&MODES);
        assert_eq!(next_discrete_value(&descriptor, 0.0), 1.0);
        assert_eq!(next_discrete_value(&descriptor, 1.0), 2.0);
        assert_eq!(next_discrete_value(&descriptor, 2.0), 0.0);
        assert_eq!(control_for(&descriptor), ParamControl::Choice);
    }

    #[test]
    fn scrolling_steps_a_linear_parameter_by_a_fixed_fraction() {
        let descriptor = ParamDescriptor::new(0u32, "gain", "Gain", -60.0, 12.0, 0.0)
            .with_curve(ParamValueCurve::Linear);
        let stepped = value_after_scroll(&descriptor, 0.0, 1.0);
        assert!((stepped - (0.0 + 72.0 * SCROLL_STEP)).abs() < 1e-3);
    }
}
