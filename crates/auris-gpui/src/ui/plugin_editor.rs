//! Generic parameter editing for any plugin.
//!
//! Because [`ParamDescriptor`] carries a plugin's whole control surface — range, curve, unit and
//! any discrete steps — the UI can build an editor for a plugin it has never seen. Adding a new
//! synth or effect therefore costs zero UI code: register it and its controls appear.

use auris_session::prelude::*;
use gpui::{
    App, ClickEvent, ElementId, Hsla, IntoElement, MouseDownEvent, Pixels, SharedString, Window,
    div, prelude::*, px,
};

use crate::theme::Theme;
use crate::ui::widgets::{
    ButtonStyle, RowColumn, SliderFill, button, dragged, picker_row, value_slider,
};

/// Which control shape suits a parameter.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamControl {
    /// A drag-to-edit bar; the common case.
    Slider,
    /// An on/off button.
    Toggle,
    /// A button naming the position in force, which opens a menu of the rest.
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

/// A drag-to-edit row for a continuous parameter.
///
/// `label` and `value_text` arrive already translated: this module knows how a control behaves,
/// not what language it speaks.
#[allow(clippy::too_many_arguments)]
pub fn slider_row<I, D>(
    id: I,
    descriptor: &ParamDescriptor,
    label: String,
    value_text: String,
    value: f32,
    fill: Hsla,
    theme: &Theme,
    on_drag_start: D,
) -> gpui::Stateful<gpui::Div>
where
    I: Into<ElementId>,
    D: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    value_slider(
        id,
        label,
        value_text,
        descriptor.normalize(value),
        fill,
        slider_fill_for(descriptor),
        theme,
        on_drag_start,
    )
}

/// Which way a parameter's bar should fill.
///
/// A range *symmetric* about zero is an offset from a neutral centre — pan, detune, an EQ
/// band's gain — so its bar grows outward from the middle. Merely straddling zero is not
/// enough: a fader runs -60 to +6 dB and is still a fader, and drawing it from the middle
/// would put unity gain two thirds along a bar that reads as half empty.
pub fn slider_fill_for(descriptor: &ParamDescriptor) -> SliderFill {
    let span = descriptor.max - descriptor.min;
    let symmetric = (descriptor.min + descriptor.max).abs() <= span * 0.2;
    if descriptor.min < 0.0 && descriptor.max > 0.0 && symmetric {
        SliderFill::FromCentre
    } else {
        SliderFill::FromStart
    }
}

/// How wide the value button in a parameter row is drawn.
///
/// A column, so a plugin's rows line up down the panel however long their names are.
const VALUE_WIDTH: Pixels = px(104.0);

/// A button row for a toggle or choice parameter.
///
/// The shared [`picker_row`], plus the one thing that is the parameter's rather than the row's:
/// a toggle in its on position is latched and lights up, where a choice merely names where it
/// stands.
#[allow(clippy::too_many_arguments)]
pub fn button_row<I, F>(
    id: I,
    descriptor: &ParamDescriptor,
    label: String,
    value_text: String,
    value: f32,
    theme: &Theme,
    on_click: F,
) -> gpui::Div
where
    I: Into<ElementId>,
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let engaged = matches!(control_for(descriptor), ParamControl::Toggle) && value >= 0.5;
    picker_row(
        id,
        label,
        value_text,
        RowColumn::Value(VALUE_WIDTH),
        engaged,
        theme,
        on_click,
    )
}

/// How many positions a discrete parameter has.
fn discrete_steps(descriptor: &ParamDescriptor) -> u32 {
    descriptor.steps.unwrap_or(2).max(2)
}

/// The value of a discrete parameter's `index`-th position, counting from zero.
///
/// A choice stores its position as a number the plugin reads, so a menu naming the positions has
/// to be able to say what each one is worth. The arithmetic lives here rather than at the menu
/// because [`next_discrete_value`] does the same sum and the two must not drift.
pub fn discrete_value(descriptor: &ParamDescriptor, index: usize) -> f32 {
    let span = descriptor.max - descriptor.min;
    if span.abs() < f32::EPSILON {
        return descriptor.min;
    }
    let quantum = span / (discrete_steps(descriptor) - 1) as f32;
    descriptor.clamp(descriptor.min + index as f32 * quantum)
}

/// The value a choice or toggle parameter takes after one click.
///
/// Wrapping at the end is what makes a two-option choice behave like a toggle and a four-option
/// one cycle, without the caller needing to know which it has.
pub fn next_discrete_value(descriptor: &ParamDescriptor, current: f32) -> f32 {
    let steps = discrete_steps(descriptor);
    let span = descriptor.max - descriptor.min;
    if span.abs() < f32::EPSILON {
        return descriptor.min;
    }
    let quantum = span / (steps - 1) as f32;
    let index = ((current - descriptor.min) / quantum).round() as i64;
    discrete_value(descriptor, (index + 1).rem_euclid(steps as i64) as usize)
}

/// The value after dragging `delta_pixels` horizontally from `start_value`.
///
/// The drag moves the *normalised* position, so a logarithmic frequency control keeps a
/// constant musical interval per pixel across its whole range.
pub fn value_after_drag(descriptor: &ParamDescriptor, start_value: f32, delta_pixels: f32) -> f32 {
    descriptor.denormalize(dragged(descriptor.normalize(start_value), delta_pixels))
}

/// A compact heading for a plugin in the inspector, with a bypass button.
#[allow(clippy::too_many_arguments)]
pub fn plugin_header<I, N, L, F>(
    id: I,
    name: N,
    enabled: bool,
    state_label: L,
    theme: &Theme,
    on_toggle: F,
) -> impl IntoElement + use<I, N, L, F>
where
    I: Into<ElementId>,
    N: Into<SharedString>,
    L: Into<SharedString>,
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
            state_label,
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
    use crate::ui::widgets::DRAG_RANGE_PIXELS;

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
    fn only_symmetric_ranges_fill_from_the_centre() {
        // Offsets from a neutral centre.
        let pan = ParamDescriptor::new(0u32, "pan", "Pan", -1.0, 1.0, 0.0);
        let octave = ParamDescriptor::new(0u32, "oct", "Octave", -3.0, 3.0, 0.0);
        let eq_gain = ParamDescriptor::decibels(0u32, "gain", "Gain", -24.0, 24.0, 0.0);
        for descriptor in [&pan, &octave, &eq_gain] {
            assert_eq!(slider_fill_for(descriptor), SliderFill::FromCentre);
        }

        // Faders straddle zero too, but they are not centred on it.
        let fader = ParamDescriptor::decibels(0u32, "level", "Level", -60.0, 6.0, 0.0);
        let track = ParamDescriptor::decibels(0u32, "vol", "Volume", -60.0, 12.0, 0.0);
        // And plenty of parameters never go negative at all.
        let percent = ParamDescriptor::percent(0u32, "mix", "Mix", 0.5);
        for descriptor in [&fader, &track, &percent] {
            assert_eq!(slider_fill_for(descriptor), SliderFill::FromStart);
        }
    }

    #[test]
    fn a_toggle_flips_on_each_click() {
        let descriptor = ParamDescriptor::toggle(0u32, "on", "On", false);
        assert_eq!(next_discrete_value(&descriptor, 0.0), 1.0);
        assert_eq!(next_discrete_value(&descriptor, 1.0), 0.0);
    }

    const MODES: [std::borrow::Cow<'static, str>; 3] = [
        std::borrow::Cow::Borrowed("A"),
        std::borrow::Cow::Borrowed("B"),
        std::borrow::Cow::Borrowed("C"),
    ];

    #[test]
    fn a_choice_cycles_and_wraps() {
        let descriptor =
            ParamDescriptor::new(0u32, "mode", "Mode", 0.0, 1.0, 0.0).with_choices(&MODES);
        assert_eq!(next_discrete_value(&descriptor, 0.0), 1.0);
        assert_eq!(next_discrete_value(&descriptor, 1.0), 2.0);
        assert_eq!(next_discrete_value(&descriptor, 2.0), 0.0);
        assert_eq!(control_for(&descriptor), ParamControl::Choice);
    }

    #[test]
    fn every_named_position_has_a_value_a_menu_can_set() {
        // The menu lists the labels and sets a number, so the two have to line up: the nth label
        // must be the value that cycling n times from the first arrives at, or picking `C` from
        // the list would select `B`.
        let descriptor =
            ParamDescriptor::new(0u32, "mode", "Mode", 0.0, 1.0, 0.0).with_choices(&MODES);
        let mut walked = descriptor.min;
        for index in 0..MODES.len() {
            assert_eq!(
                discrete_value(&descriptor, index),
                walked,
                "position {index}"
            );
            walked = next_discrete_value(&descriptor, walked);
        }
        assert_eq!(walked, descriptor.min, "and back to the first");

        // A toggle is the same arithmetic with two positions, so its "off" and "on" are reachable
        // the same way even though nothing offers them as a list.
        let toggle = ParamDescriptor::toggle(0u32, "on", "On", false);
        assert_eq!(discrete_value(&toggle, 0), 0.0);
        assert_eq!(discrete_value(&toggle, 1), 1.0);
        // Past the end is clamped rather than running off the top of the range.
        assert_eq!(discrete_value(&toggle, 9), 1.0);
    }
}
