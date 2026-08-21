//! The right-hand inspector: the selected track's instrument and its effect chain.
//!
//! The list of everything that *could* go on it is the library, on the other side of the
//! arrangement — see [`crate::ui::library`]. The two panels share [`panel_header`], because they
//! sit either side of the arrangement at the same height.

use auris_i18n::Key;
use auris_session::Session;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::theme::Theme;
use crate::ui::icons::Icon;
use crate::ui::plugin_editor::{
    ParamControl, button_row, control_for, next_discrete_value, slider_row, value_after_drag,
};
use crate::ui::plugin_window::PluginSubject;
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::widgets::{chain_button, divider};

/// One row of a channel strip's insert list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Insert {
    /// A slot holding an effect.
    Filled {
        /// Which slot in the strip's chain.
        slot: EffectSlotId,
        /// Registry id of the effect in it.
        effect_id: String,
        /// Whether it is in the signal path.
        enabled: bool,
    },
    /// The empty slot at the end, which adds one.
    Empty,
}

/// A chain's slots, followed by exactly one empty one.
///
/// This is both Logic's shape and the only shape the document can express. `Session::add_effect`
/// appends, `move_effect` clamps within the existing length, and a strip's effects are a plain
/// `Vec` — so there is no such thing as slot 4 empty while slot 5 is full, and no cap on how many
/// there may be. The fixed slots stay a view-side idea, because nothing at or below
/// `auris-session` may be shaped by one.
pub(crate) fn insert_rows(chain: &[(EffectSlotId, String, bool)]) -> Vec<Insert> {
    let mut rows: Vec<Insert> = chain
        .iter()
        .map(|(slot, effect_id, enabled)| Insert::Filled {
            slot: *slot,
            effect_id: effect_id.clone(),
            enabled: *enabled,
        })
        .collect();
    rows.push(Insert::Empty);
    rows
}

/// How far a held effect has to move to land where the pointer now is.
///
/// `over` is the slot the pointer has entered, or `None` for the empty slot at the end of the
/// list, which stands for the end of the chain. A *delta* rather than an index because that is
/// what `Session::move_effect` takes, and it takes one because the two chevrons beside each row
/// were the first thing to ask it.
///
/// `None` when nothing should happen: the pointer is over the row it set off from, or over a row
/// belonging to some other strip — the mixer draws a chain per channel, and a drag that wandered
/// sideways must not reorder the one it wandered into.
pub(crate) fn reorder_delta(
    chain: &[EffectSlotId],
    held: EffectSlotId,
    over: Option<EffectSlotId>,
) -> Option<isize> {
    let from = chain.iter().position(|slot| *slot == held)?;
    let to = match over {
        Some(slot) => chain.iter().position(|candidate| *candidate == slot)?,
        None => chain.len().checked_sub(1)?,
    };
    (to != from).then(|| to as isize - from as isize)
}

/// A stable per-slot element key, so gpui can track hover state across frames.
///
/// Keyed by the slot's own id rather than by its position. The mixer used to pack a strip index
/// and a slot index into `index * 64 + slot_index`, which collided past sixty-four effects on a
/// strip and moved every key in a chain whenever it was reordered — worth retiring now that a
/// third surface draws the same chain. Zero is reserved for the empty slot, which has no id.
pub(crate) fn insert_element_key(slot: Option<EffectSlotId>) -> usize {
    slot.map_or(0, |id| id.0 as usize + 1)
}

/// The title strip at the top of a side panel.
///
/// Shared by the library and the inspector so the two line up: they sit either side of the
/// arrangement at the same height, and a few pixels of difference between them is the kind of
/// thing that is invisible in isolation and obvious once both are on screen.
pub(crate) fn panel_header(title: &str, theme: &Theme) -> impl IntoElement + use<> {
    let title: gpui::SharedString = title.to_string().into();
    div()
        .flex()
        .items_center()
        .h(Metrics::PANEL_HEADER_HEIGHT)
        .px_2()
        .flex_shrink_0()
        .bg(theme.surface_raised)
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.text_muted)
        .child(title)
}

impl AurisApp {
    /// Renders the inspector panel.
    pub(crate) fn render_inspector(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let body = self.render_track_inspector(cx);

        // One page now, with no tab bar. The plugin browser that used to share this panel is the
        // library on the left: a picker that hid the thing it was editing had to be dismissed
        // before its own result could be seen.
        //
        // The header names the panel rather than what is in it. It said "Track" until the selected
        // clip's recipe joined the track's own controls here, and a panel showing two things under
        // the name of one of them is worse than a panel that says which panel it is — the groups
        // inside carry their own headings.
        //
        // The width comes from the parent, which owns the resizable panel geometry, and the
        // divider line is drawn by the splitter beside it.
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .child(panel_header(self.t(Key::Inspector), &theme))
            .child(
                self.scrolling(
                    ScrollPanel::Inspector,
                    div()
                        .id("inspector-body")
                        .overflow_y_scroll()
                        .p_2()
                        .child(body),
                    cx,
                ),
            )
    }

    fn render_track_inspector(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        // The selected clip's recipe, when it has one, above the track that plays it. That order
        // is the order of the sentence it makes: this part, on this instrument, through these
        // effects. It is also what was just clicked, and so what the eye is already on.
        let mut sections: Vec<AnyElement> = self.part_rows(cx);

        let Some(track_id) = self.selected_track else {
            sections.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::NoTrackSelected))
                    .into_any_element(),
            );
            return div()
                .flex()
                .flex_col()
                .gap_1()
                .children(sections)
                .into_any_element();
        };
        let Some(track) = self.project().track(track_id) else {
            return div()
                .flex()
                .flex_col()
                .gap_1()
                .children(sections)
                .into_any_element();
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

        sections.push(self.group_heading(Key::Track).into_any_element());
        sections.push(
            div()
                .text_sm()
                .text_color(theme.text)
                .pb_2()
                .child(track_name)
                .into_any_element(),
        );

        if let Some(instrument_id) = instrument_id {
            let name = self.instrument_label(track_id, &instrument_id);
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
                            .child(self.t(Key::Instrument)),
                    )
                    .child(crate::ui::widgets::button(
                        "inst-open",
                        name,
                        crate::ui::widgets::ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            this.open_plugin_window(
                                PluginSubject::Instrument(track_id),
                                event.position(),
                            );
                            cx.notify();
                        }),
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
                        .child(self.t(Key::Inserts)),
                )
                .into_any_element(),
        );

        // Logic's shape: the slots that are filled, then one empty one that adds another. The
        // empty slot replaces both the old "+ Add" button in this heading and the "No effects"
        // line that used to stand in for an empty chain — one affordance in the place the next
        // effect will actually appear, rather than two somewhere else.
        let rows = insert_rows(&effect_slots);
        for row in rows {
            let (slot_id, effect_id, enabled) = match row {
                Insert::Filled {
                    slot,
                    effect_id,
                    enabled,
                } => (slot, effect_id, enabled),
                Insert::Empty => {
                    sections.push(
                        // Wrapped so the empty slot can be dragged onto as well as clicked: the
                        // row that adds the next effect is also where the end of the chain is,
                        // and it is the only way to say "last" with the pointer.
                        div()
                            .on_mouse_move(cx.listener(
                                move |this, _: &gpui::MouseMoveEvent, _, cx| {
                                    this.drag_effect_onto(Some(track_id), None, cx);
                                },
                            ))
                            .child(crate::ui::widgets::icon_label(
                                ("insert-empty", insert_element_key(None)),
                                Icon::Plus,
                                self.t(Key::Effect),
                                &theme,
                                Self::opens_menu(cx, move |this, at| {
                                    this.effect_picker_menu(at, Some(track_id))
                                }),
                            ))
                            .into_any_element(),
                    );
                    continue;
                }
            };
            let slot_index = insert_element_key(Some(slot_id));
            let name = self.effect_label(slot_id, &effect_id);
            let menu_name = name.clone();

            // One row per insert, and its parameters live in the plugin editor rather than
            // underneath it. Expanding every effect in place pushed the rest of the chain down
            // the panel, so a four-effect strip could not be read without scrolling — and the
            // slot you were about to reach for kept moving.
            //
            // The reorder and remove buttons stay visible here, unlike on the mixer's 128px
            // strips where the same row has room for none of them. Losing them to the right-click
            // menu everywhere would be a real reduction in discoverability, and this panel has
            // the width to keep them.
            sections.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(Metrics::CONTROL_HEIGHT)
                    // A row in hand is lifted off the list, the way a track header is: what
                    // follows the pointer is the chain itself rather than a line predicting it.
                    .when(self.dragging_effect(slot_id), |this| {
                        this.bg(theme.surface_raised).opacity(0.8)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            // The fallback grab: a press that landed on one of the chevrons has
                            // claimed the gesture already, and this runs after it because a
                            // parent's handler bubbles last.
                            if this.drag.is_none() {
                                this.begin_drag(Drag::EffectReorder {
                                    track: Some(track_id),
                                    slot: slot_id,
                                });
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                        this.drag_effect_onto(Some(track_id), Some(slot_id), cx);
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let menu = this.effect_menu(
                                event.position,
                                Some(track_id),
                                slot_id,
                                menu_name.clone(),
                            );
                            this.open_menu(menu);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(div().flex_1().min_w_0().child(crate::ui::widgets::button(
                        ("insert-open", slot_index),
                        name,
                        crate::ui::widgets::ButtonStyle::Ghost,
                        enabled,
                        theme.accent_soft,
                        &theme,
                        cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            this.open_plugin_window(
                                PluginSubject::Insert {
                                    track: Some(track_id),
                                    slot: slot_id,
                                },
                                event.position(),
                            );
                            cx.notify();
                        }),
                    )))
                    .child(
                        chain_button(
                            ("fx-up", slot_index),
                            Icon::ChevronUp,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.move_effect(Some(track_id), slot_id, -1);
                                cx.notify();
                            }),
                        )
                        .tooltip(self.tip(Key::MenuMoveUp, "")),
                    )
                    .child(
                        chain_button(
                            ("fx-down", slot_index),
                            Icon::ChevronDown,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.move_effect(Some(track_id), slot_id, 1);
                                cx.notify();
                            }),
                        )
                        .tooltip(self.tip(Key::MenuMoveDown, "")),
                    )
                    .child(
                        chain_button(
                            ("fx-remove", slot_index),
                            Icon::Cross,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.remove_effect(slot_id);
                                cx.notify();
                            }),
                        )
                        .tooltip(self.tip(Key::MenuRemove, "")),
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

                let label = self.param_label(&descriptor.name);
                // The label again for the menu, which the control itself consumes.
                let menu_label = label.clone();
                let value_text = self.format_param(descriptor, value);
                match control_for(descriptor) {
                    ParamControl::Slider => slider_row(
                        element_id,
                        descriptor,
                        label,
                        value_text,
                        value,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if event.click_count >= 2 {
                                this.reset_param(target);
                                cx.notify();
                                return;
                            }
                            // `set_param_value` marks the document dirty once the drag actually
                            // moves something; a click that only grabs the control is not an edit.
                            this.begin_drag(Drag::Param {
                                target,
                                start_value: value,
                                start_x: event.position.x,
                                fine: event.modifiers.shift,
                            });
                        }),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        Self::opens_menu(cx, move |this, at| {
                            this.param_menu(at, target, menu_label.clone())
                        }),
                    )
                    .into_any_element(),
                    // A toggle flips on the press, because two positions are a switch. A choice
                    // opens the list instead: cycling through eight waveforms to reach the pulse
                    // means counting them, and overshooting means going round again.
                    ParamControl::Toggle => {
                        let owned = descriptor.clone();
                        button_row(
                            element_id,
                            descriptor,
                            label,
                            value_text,
                            value,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                let current = this.session.param_value(target, &owned);
                                let next = next_discrete_value(&owned, current);
                                this.session.set_param(target, next);
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            Self::opens_menu(cx, move |this, at| {
                                this.param_menu(at, target, menu_label.clone())
                            }),
                        )
                        .into_any_element()
                    }
                    ParamControl::Choice => {
                        let owned = descriptor.clone();
                        button_row(
                            element_id,
                            descriptor,
                            label,
                            value_text,
                            value,
                            &theme,
                            Self::opens_menu(cx, move |this, at| {
                                this.param_choice_menu(at, target, &owned)
                            }),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            Self::opens_menu(cx, move |this, at| {
                                this.param_menu(at, target, menu_label.clone())
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
            .unwrap_or_else(|| ParamDescriptor::new(0u32, "value", "value", 0.0, 1.0, 0.0));
        crate::ui::widgets::value_slider(
            id,
            label,
            self.format_param(&descriptor, value),
            descriptor.normalize(value),
            theme.accent,
            crate::ui::plugin_editor::slider_fill_for(&descriptor),
            &theme,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.click_count >= 2 {
                    this.reset_param(target);
                    cx.notify();
                    return;
                }
                this.begin_drag(Drag::Param {
                    target,
                    start_value: value,
                    start_x: event.position.x,
                    fine: event.modifiers.shift,
                });
            }),
        )
        .on_mouse_down(
            gpui::MouseButton::Right,
            Self::opens_menu(cx, move |this, at| this.param_menu(at, target, label)),
        )
        .into_any_element()
    }

    /// Asks for a parameter's value as a number.
    ///
    /// The title carries the range and the unit, because there is no shared notation to hang a
    /// hint on: every parameter is written in its own. The field starts on the value the control
    /// is showing, so correcting a fader by half a decibel is an edit to what is there rather
    /// than typing it out again from nothing.
    pub(crate) fn prompt_for_param(&mut self, target: ParamTarget) {
        let Some(descriptor) = self.session.descriptor_for(target) else {
            return;
        };
        let value = self.session.param_value(target, &descriptor);
        let title = format!(
            "{} ({} – {})",
            self.param_label(&descriptor.name),
            self.format_param(&descriptor, descriptor.min),
            self.format_param(&descriptor, descriptor.max),
        );
        self.open_prompt(crate::ui::prompt::Prompt::new(
            title,
            crate::ui::prompt::PromptTarget::Param(target),
            format!("{value}"),
        ));
    }

    /// Puts a parameter back to whatever its descriptor calls the default.
    ///
    /// Double-click, which is how every mixer in the world brings a fader back to 0 dB and a pan
    /// back to centre. The number was already there; there was simply no gesture that asked for
    /// it, and a fader nudged off unity could only be walked back by eye.
    pub(crate) fn reset_param(&mut self, target: ParamTarget) {
        let Some(descriptor) = self.session.descriptor_for(target) else {
            return;
        };
        self.session.set_param(target, descriptor.default);
    }

    /// Moves a parameter drag's anchor to where the pointer is, without changing the value.
    ///
    /// What the fine modifier being pressed or released half way through a drag does. See
    /// [`Drag::Param::fine`](crate::app::Drag).
    pub(crate) fn reanchor_param_drag(&mut self, target: ParamTarget, at: Pixels, fine: bool) {
        let Some(descriptor) = self.session.descriptor_for(target) else {
            return;
        };
        let value = self.session.param_value(target, &descriptor);
        if let Some(Drag::Param {
            start_value,
            start_x,
            fine: held,
            ..
        }) = self.drag.as_mut()
        {
            *start_value = value;
            *start_x = at;
            *held = fine;
        }
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
    ///
    /// Neither this nor [`Self::set_track_instrument`] bounces a panel back afterwards any more.
    /// They both used to return the inspector to its Track tab, because the browser they were
    /// called from had covered the strip being edited; a library that never covered it needs no
    /// such correction, and the result is simply visible where it always was.
    pub(crate) fn add_effect_to_selection(&mut self, effect_id: &str) {
        self.add_effect_to(self.selected_track, effect_id);
    }

    /// Adds an effect to one named strip, or to the master bus when `track` is `None`.
    ///
    /// The explicit target is what an insert slot needs: a slot on the master strip and a slot on
    /// a track strip can be on screen at once, and neither should have to move the selection to
    /// say which one it is.
    pub(crate) fn add_effect_to(&mut self, track: Option<TrackId>, effect_id: &str) {
        if let Err(error) = self.session.add_effect(track, effect_id) {
            self.set_failed_status(self.failure(Key::MenuAddEffect, &error));
        }
    }

    /// Adds a plugin hosted from a file to the selected strip.
    pub(crate) fn add_hosted_effect_to_selection(&mut self, file: &std::path::Path, clap_id: &str) {
        let track = self.selected_track;
        if let Err(error) = self.session.add_hosted_effect(track, file, clap_id) {
            self.set_failed_status(self.failure(Key::MenuAddEffect, &error));
        }
    }

    /// Points the selected track's instrument at a plugin hosted from a file.
    ///
    /// Nothing happens with no track selected, exactly as choosing a built-in instrument does
    /// nothing: an instrument belongs to a track, and there is no sensible track to guess at.
    pub(crate) fn set_hosted_instrument_on_selection(
        &mut self,
        file: &std::path::Path,
        clap_id: &str,
    ) {
        let Some(track) = self.selected_track else {
            return;
        };
        if let Err(error) = self.session.set_hosted_instrument(track, file, clap_id) {
            self.set_failed_status(self.failure(Key::EditChangeInstrument, &error));
        }
    }

    /// Replaces the selected track's instrument.
    pub(crate) fn set_track_instrument(&mut self, instrument_id: &str) {
        let Some(track) = self.selected_track else {
            return;
        };
        if let Err(error) = self.session.set_track_instrument(track, instrument_id) {
            self.set_failed_status(self.failure(Key::EditChangeInstrument, &error));
        }
    }

    /// Points the selected track at one of an imported SoundFont's sounds.
    ///
    /// The session switches the track to the sampler as part of the same edit, so this is one
    /// click and one undo step rather than "load the sampler, then choose a sound".
    pub(crate) fn set_track_preset(&mut self, preset: PresetRef) {
        let Some(track) = self.selected_track else {
            return;
        };
        if let Err(error) = self.session.set_track_preset(track, preset) {
            self.set_failed_status(self.failure(Key::EditChoosePreset, &error));
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

    /// The slots of a strip's chain, in the order the signal meets them.
    pub(crate) fn chain_slots(&self, track: Option<TrackId>) -> Vec<EffectSlotId> {
        let strip = match track {
            Some(id) => self.project().track(id).map(|track| &track.mixer),
            None => Some(&self.project().master),
        };
        strip.map_or_else(Vec::new, |strip| {
            strip.effects.iter().map(|slot| slot.id).collect()
        })
    }

    /// Carries a dragged effect onto the row the pointer has just entered.
    ///
    /// Driven by the row rather than by a coordinate. An element the pointer is inside knows
    /// which slot it stands for without anybody measuring the list, and it is the only thing that
    /// does — the chain is drawn in two panels at two row heights, and one of them is a column of
    /// mixer strips whose rectangles nothing records.
    pub(crate) fn drag_effect_onto(
        &mut self,
        over_track: Option<TrackId>,
        over: Option<EffectSlotId>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(Drag::EffectReorder { track, slot }) = self.drag else {
            return;
        };
        if over_track != track {
            return;
        }
        let Some(delta) = reorder_delta(&self.chain_slots(track), slot, over) else {
            return;
        };
        self.move_effect(track, slot, delta);
        cx.notify();
    }

    /// Whether an effect is the one currently in hand.
    pub(crate) fn dragging_effect(&self, slot: EffectSlotId) -> bool {
        matches!(self.drag, Some(Drag::EffectReorder { slot: held, .. }) if held == slot)
    }

    /// Removes an effect from wherever it is.
    pub(crate) fn remove_effect(&mut self, slot: EffectSlotId) {
        self.session.remove_effect(slot);
    }
}

/// A plugin's display name, translated where the term is known.
pub(crate) fn audio_name(app: &AurisApp, english: &str) -> String {
    auris_i18n::audio::plugin_name(english, app.language()).to_string()
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
        // A send id comes from the same counter as a slot id, so the same stride keeps it clear
        // of everything else.
        ParamTarget::Send { send, .. } => send.0 as usize * 4096,
    };
    base + param.index()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_an_effect_onto_a_row_moves_it_exactly_that_far() {
        let chain = vec![EffectSlotId(4), EffectSlotId(9), EffectSlotId(2)];

        assert_eq!(
            reorder_delta(&chain, EffectSlotId(4), Some(EffectSlotId(2))),
            Some(2)
        );
        assert_eq!(
            reorder_delta(&chain, EffectSlotId(2), Some(EffectSlotId(4))),
            Some(-2)
        );

        // The row it is already on asks for nothing, which is what stops the chain being
        // rewritten on every pointer move across the row the drag began in.
        assert_eq!(
            reorder_delta(&chain, EffectSlotId(9), Some(EffectSlotId(9))),
            None
        );

        // The empty slot at the end of the list means the end of the chain — and means nothing
        // at all to the effect that is already there.
        assert_eq!(reorder_delta(&chain, EffectSlotId(4), None), Some(2));
        assert_eq!(reorder_delta(&chain, EffectSlotId(2), None), None);

        // A row from another strip is not in this chain, and neither is anything at all when the
        // chain is empty.
        assert_eq!(
            reorder_delta(&chain, EffectSlotId(4), Some(EffectSlotId(7))),
            None
        );
        assert_eq!(reorder_delta(&[], EffectSlotId(4), None), None);
    }

    #[test]
    fn a_chain_always_ends_in_one_empty_insert() {
        // The whole insert-slot model as numbers: an empty strip is one empty slot, and a chain
        // is its effects in order with exactly one empty slot after them. Anything else and the
        // strip either offers no way to add an effect or offers several.
        assert_eq!(insert_rows(&[]), vec![Insert::Empty]);

        let chain = vec![
            (EffectSlotId(7), "a".to_string(), true),
            (EffectSlotId(3), "b".to_string(), false),
        ];
        let rows = insert_rows(&chain);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            Insert::Filled {
                slot: EffectSlotId(7),
                effect_id: "a".to_string(),
                enabled: true
            }
        );
        assert_eq!(
            rows[1],
            Insert::Filled {
                slot: EffectSlotId(3),
                effect_id: "b".to_string(),
                enabled: false
            }
        );
        assert_eq!(rows[2], Insert::Empty);
    }

    #[test]
    fn every_insert_row_gets_its_own_element_key() {
        // Zero belongs to the empty slot, which is why the filled ones are offset by one — slot
        // id 0 is a real slot and would otherwise share a key with it.
        assert_eq!(insert_element_key(None), 0);
        assert_ne!(
            insert_element_key(None),
            insert_element_key(Some(EffectSlotId(0)))
        );
        assert_ne!(
            insert_element_key(Some(EffectSlotId(1))),
            insert_element_key(Some(EffectSlotId(2)))
        );
        // The packing this replaces collided here: strip 1 slot 0 and strip 0 slot 64 both came
        // out as 64. Keyed by the slot's own id, sixty-five effects on one strip stay distinct.
        assert_ne!(
            insert_element_key(Some(EffectSlotId(64))),
            insert_element_key(Some(EffectSlotId(0)))
        );
    }

    #[test]
    fn element_keys_differ_between_targets() {
        let a = target_element_key(ParamTarget::TrackGain(TrackId(1)), ParamId(0));
        let b = target_element_key(ParamTarget::TrackPan(TrackId(1)), ParamId(0));
        let c = target_element_key(ParamTarget::TrackGain(TrackId(2)), ParamId(0));
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
