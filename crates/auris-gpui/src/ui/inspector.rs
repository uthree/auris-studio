//! The right-hand inspector: the selected track's instrument, its effect chain, and a browser
//! for everything the plugin registry knows about.

use auris_session::Session;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag, InspectorTab};
use crate::theme::Metrics;
use crate::ui::plugin_editor::{
    ParamControl, button_row, control_for, next_discrete_value, plugin_header, slider_row,
    value_after_drag, value_after_scroll,
};
use crate::ui::widgets::{ButtonStyle, button, divider};

impl AurisApp {
    /// Renders the inspector panel.
    pub(crate) fn render_inspector(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let tab = self.inspector;

        let body = match tab {
            InspectorTab::Track => self.render_track_inspector(cx),
            InspectorTab::Browser => self.render_browser(cx),
        };

        div()
            .flex()
            .flex_col()
            .w(Metrics::INSPECTOR_WIDTH)
            .flex_shrink_0()
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .gap_1()
                    .p_1()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(button(
                        "tab-track",
                        "Track",
                        ButtonStyle::Normal,
                        tab == InspectorTab::Track,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.inspector = InspectorTab::Track;
                            cx.notify();
                        }),
                    )))
                    .child(div().flex_1().child(button(
                        "tab-browser",
                        "Plugins",
                        ButtonStyle::Normal,
                        tab == InspectorTab::Browser,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.inspector = InspectorTab::Browser;
                            cx.notify();
                        }),
                    ))),
            )
            .child(
                div()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(body),
            )
    }

    fn render_track_inspector(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let Some(track_id) = self.selected_track else {
            return div()
                .text_xs()
                .text_color(theme.text_muted)
                .child("No track selected")
                .into_any_element();
        };
        let Some(track) = self.project().track(track_id) else {
            return div().into_any_element();
        };
        let track_name = track.name.clone();
        let instrument_id = track
            .kind
            .as_instrument()
            .map(|inner| inner.instrument_id.clone());
        let effect_slots: Vec<(EffectSlotId, String, bool)> = track
            .mixer
            .effects
            .iter()
            .map(|slot| (slot.id, slot.effect_id.clone(), slot.enabled))
            .collect();

        let mut sections: Vec<AnyElement> = Vec::new();
        sections.push(
            div()
                .text_sm()
                .text_color(theme.text)
                .pb_2()
                .child(track_name)
                .into_any_element(),
        );

        if let Some(instrument_id) = instrument_id {
            let name = self
                .registry()
                .descriptor(&instrument_id)
                .map(|d| d.name.to_string())
                .unwrap_or_else(|| instrument_id.clone());
            sections.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(24.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("Instrument"),
                    )
                    .child(div().text_xs().text_color(theme.text).child(name))
                    .into_any_element(),
            );
            let descriptors = self.session.param_descriptors(&instrument_id);
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(self.param_controls(
                        &descriptors,
                        move |param| ParamTarget::Instrument {
                            track: track_id,
                            param,
                        },
                        "inst",
                        cx,
                    ))
                    .into_any_element(),
            );
            sections.push(divider(&theme).into_any_element());
        }

        sections.push(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(24.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("Effects"),
                )
                .child(div().w(px(58.0)).child(button(
                    "add-effect",
                    "+ Add",
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.inspector = InspectorTab::Browser;
                        cx.notify();
                    }),
                )))
                .into_any_element(),
        );

        if effect_slots.is_empty() {
            sections.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("No effects — add one from the Plugins tab")
                    .into_any_element(),
            );
        }

        for (slot_index, (slot_id, effect_id, enabled)) in effect_slots.into_iter().enumerate() {
            let name = self
                .registry()
                .descriptor(&effect_id)
                .map(|d| d.name.to_string())
                .unwrap_or_else(|| effect_id.clone());
            let descriptors = self.session.param_descriptors(&effect_id);
            let controls = self.param_controls(
                &descriptors,
                move |param| ParamTarget::Effect {
                    track: Some(track_id),
                    slot: slot_id,
                    param,
                },
                "fx",
                cx,
            );

            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1p5()
                    .mb_2()
                    .rounded_md()
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_subtle)
                    .child(plugin_header(
                        ("fx-bypass", slot_index),
                        name,
                        enabled,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            this.toggle_effect(Some(track_id), slot_id);
                            cx.notify();
                        }),
                    ))
                    .children(controls)
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .pt_1()
                            .child(div().flex_1().child(button(
                                ("fx-up", slot_index),
                                "Move up",
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.move_effect(Some(track_id), slot_id, -1);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().child(button(
                                ("fx-remove", slot_index),
                                "Remove",
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.remove_effect(slot_id);
                                    cx.notify();
                                }),
                            ))),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(sections)
            .into_any_element()
    }

    /// The plugin browser, listing everything the registry knows.
    fn render_browser(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let instruments: Vec<(String, String, String)> = self
            .registry()
            .instruments()
            .map(|d| {
                (
                    d.id.to_string(),
                    d.name.to_string(),
                    d.description.to_string(),
                )
            })
            .collect();
        let effects: Vec<(String, String, String, String)> = self
            .registry()
            .effects()
            .map(|d| {
                (
                    d.id.to_string(),
                    d.name.to_string(),
                    d.description.to_string(),
                    d.category.label().to_string(),
                )
            })
            .collect();

        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .pb_1()
                .child("Instruments — click to set on the selected track")
                .into_any_element(),
        );
        for (index, (id, name, description)) in instruments.into_iter().enumerate() {
            rows.push(
                self.browser_row(("browser-inst", index), &name, &description, "", {
                    let id = id.clone();
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_track_instrument(&id);
                        cx.notify();
                    })
                })
                .into_any_element(),
            );
        }
        rows.push(divider(&theme).into_any_element());
        rows.push(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .py_1()
                .child("Effects — click to add to the selected track")
                .into_any_element(),
        );
        for (index, (id, name, description, category)) in effects.into_iter().enumerate() {
            rows.push(
                self.browser_row(("browser-fx", index), &name, &description, &category, {
                    let id = id.clone();
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.add_effect_to_selection(&id);
                        cx.notify();
                    })
                })
                .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .into_any_element()
    }

    fn browser_row<I, F>(
        &self,
        id: I,
        name: &str,
        description: &str,
        category: &str,
        on_click: F,
    ) -> impl IntoElement + use<I, F>
    where
        I: Into<gpui::ElementId>,
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        div()
            .id(id.into())
            .flex()
            .flex_col()
            .p_1p5()
            .rounded_sm()
            .cursor_pointer()
            .hover(|this| this.bg(theme.surface_hover))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text)
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(category.to_string()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(description.to_string()),
            )
            .on_mouse_down(gpui::MouseButton::Left, on_click)
    }

    /// Builds a control for every parameter of a plugin.
    ///
    /// `target_for` turns a parameter index into the routing enum, which is what lets the same
    /// code drive an instrument, a track effect and a master effect.
    pub(crate) fn param_controls(
        &self,
        descriptors: &[ParamDescriptor],
        target_for: impl Fn(ParamId) -> ParamTarget + Copy + 'static,
        id_prefix: &'static str,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        descriptors
            .iter()
            .map(|descriptor| {
                let target = target_for(descriptor.id);
                let value = self.session.param_value(target, descriptor);
                let element_id = (id_prefix, target_element_key(target, descriptor.id));

                match control_for(descriptor) {
                    ParamControl::Slider => slider_row(
                        element_id,
                        descriptor,
                        value,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            // `set_param_value` marks the document dirty once the drag actually
                            // moves something; a click that only grabs the control is not an edit.
                            this.begin_drag(Drag::Param {
                                target,
                                start_value: value,
                                start_x: event.position.x,
                            });
                        }),
                        {
                            let descriptor = descriptor.clone();
                            cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                                let current = this.session.param_value(target, &descriptor);
                                let next = value_after_scroll(&descriptor, current, notches);
                                this.session.set_param(target, next);
                                cx.notify();
                            })
                        },
                    )
                    .into_any_element(),
                    ParamControl::Toggle | ParamControl::Choice => {
                        let owned = descriptor.clone();
                        button_row(
                            element_id,
                            descriptor,
                            value,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                let current = this.session.param_value(target, &owned);
                                let next = next_discrete_value(&owned, current);
                                this.session.set_param(target, next);
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    }
                }
            })
            .collect()
    }

    /// A gain or pan fader, sharing the parameter drag machinery.
    ///
    /// Returns an erased element: the concrete type would otherwise name two closure types
    /// borrowed from `cx`, and every caller boxes it into a child list anyway.
    pub(crate) fn fader(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        target: ParamTarget,
        value: f32,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let descriptor = Session::mixer_descriptor(target)
            .unwrap_or_else(|| ParamDescriptor::new(0u32, "value", label, 0.0, 1.0, 0.0));
        crate::ui::widgets::value_slider(
            id,
            label,
            descriptor.format(value),
            descriptor.normalize(value),
            theme.accent,
            &theme,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.begin_drag(Drag::Param {
                    target,
                    start_value: value,
                    start_x: event.position.x,
                });
            }),
            cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                let Some(descriptor) = Session::mixer_descriptor(target) else {
                    return;
                };
                let current = this.session.param_value(target, &descriptor);
                this.session
                    .set_param(target, value_after_scroll(&descriptor, current, notches));
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// Applies a parameter drag.
    pub(crate) fn drag_param(&mut self, target: ParamTarget, start_value: f32, delta: f32) {
        let Some(descriptor) = self.session.descriptor_for(target) else {
            return;
        };
        let value = value_after_drag(&descriptor, start_value, delta);
        self.session.set_param(target, value);
    }

    /// Adds an effect to the selected track, or to the master bus when nothing is selected.
    pub(crate) fn add_effect_to_selection(&mut self, effect_id: &str) {
        match self.session.add_effect(self.selected_track, effect_id) {
            Ok(_) => self.inspector = InspectorTab::Track,
            Err(error) => self.set_status(format!("Could not add the effect: {error}")),
        }
    }

    /// Replaces the selected track's instrument.
    pub(crate) fn set_track_instrument(&mut self, instrument_id: &str) {
        let Some(track) = self.selected_track else {
            return;
        };
        match self.session.set_track_instrument(track, instrument_id) {
            Ok(()) => self.inspector = InspectorTab::Track,
            Err(error) => self.set_status(format!("Could not change the instrument: {error}")),
        }
    }

    /// Bypasses or re-enables an effect.
    pub(crate) fn toggle_effect(&mut self, track: Option<TrackId>, slot: EffectSlotId) {
        let enabled = self.session.effect_enabled(track, slot).unwrap_or(true);
        self.session.set_effect_enabled(track, slot, !enabled);
    }

    /// Moves an effect up or down its chain.
    pub(crate) fn move_effect(&mut self, track: Option<TrackId>, slot: EffectSlotId, delta: isize) {
        self.session.move_effect(track, slot, delta);
    }

    /// Removes an effect from wherever it is.
    pub(crate) fn remove_effect(&mut self, slot: EffectSlotId) {
        self.session.remove_effect(slot);
    }
}

/// A stable per-target element key, so gpui can track hover state across frames.
fn target_element_key(target: ParamTarget, param: ParamId) -> usize {
    let base = match target {
        ParamTarget::TrackGain(id) => id.0 as usize * 4,
        ParamTarget::TrackPan(id) => id.0 as usize * 4 + 1,
        ParamTarget::MasterGain => 2,
        ParamTarget::MasterPan => 3,
        ParamTarget::Instrument { track, .. } => track.0 as usize * 4096,
        ParamTarget::Effect { slot, .. } => slot.0 as usize * 4096,
    };
    base + param.index()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_keys_differ_between_targets() {
        let a = target_element_key(ParamTarget::TrackGain(TrackId(1)), ParamId(0));
        let b = target_element_key(ParamTarget::TrackPan(TrackId(1)), ParamId(0));
        let c = target_element_key(ParamTarget::TrackGain(TrackId(2)), ParamId(0));
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
