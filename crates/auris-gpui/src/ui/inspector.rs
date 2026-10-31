//! The inspector: a fixed selection header and collapsible clip and track settings.
//!
//! The list of everything that *could* go on it is the library, on the other side of the
//! arrangement — see [`crate::ui::library`]. The two panels share [`panel_header`], because they
//! sit either side of the arrangement at the same height.

use auris_i18n::Key;
use auris_session::Session;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Window, div, prelude::*};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::theme::Theme;
use crate::ui::icons::Icon;
use crate::ui::plugin_editor::{
    ParamControl, button_row, control_for, next_discrete_value, slider_keyboard_step, slider_row,
    value_after_drag,
};
use crate::ui::plugin_window::PluginSubject;
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::widgets::{bounded_picker_label, disclosure};

/// Scroll and keyboard identity for the current large plugin parameter list.
#[derive(Default)]
pub(crate) struct ParameterListState {
    pub(crate) scroll: gpui::UniformListScrollHandle,
    pub(crate) rows: std::sync::Arc<Vec<gpui::FocusHandle>>,
    pending_focus: Option<usize>,
}

/// Stable groups whose disclosure state belongs to the view, not the selected document object.
#[derive(Clone, Copy)]
enum InspectorSection {
    Generation,
    Performance,
    Sound,
    Effects,
}

impl InspectorSection {
    fn id(self) -> &'static str {
        match self {
            Self::Generation => "inspector-generation",
            Self::Performance => "inspector-performance",
            Self::Sound => "inspector-sound",
            Self::Effects => "inspector-effects",
        }
    }

    fn title(self) -> Key {
        match self {
            Self::Generation => Key::InspectorGeneration,
            Self::Performance => Key::PerformHeading,
            Self::Sound => Key::InspectorSound,
            Self::Effects => Key::InspectorEffects,
        }
    }
}

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
/// what `Session::move_effect` takes for both drag gestures and menu commands.
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

        // The selection stays outside the scrolling region so every visible setting has a target.
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .child(panel_header(self.t(Key::Inspector), &theme))
            .child(self.inspector_selection())
            .child(
                self.scrolling(
                    ScrollPanel::Inspector,
                    div()
                        .id("inspector-body")
                        .overflow_y_scroll()
                        .px_2()
                        .pb_3()
                        .child(body),
                    cx,
                ),
            )
    }

    fn inspector_selection(&self) -> AnyElement {
        let theme = &self.theme;
        let track = self.selected_track.and_then(|id| self.project().track(id));
        let track_name: gpui::SharedString = track
            .map_or(self.t(Key::NoTrackSelected), |track| track.name.as_str())
            .to_string()
            .into();
        let mut header = div()
            .id("inspector-selection")
            .debug_selector(|| "inspector-selection".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .gap_1()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("inspector-selected-track")
                    .debug_selector(|| "inspector-selected-track".into())
                    .w_full()
                    .min_w_0()
                    .h_5()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .tooltip(crate::ui::tooltip::keyed_tip(track_name.clone(), "", theme))
                    .child(bounded_picker_label(track_name)),
            );
        if let Some(name) = self.selected_clip.and_then(|clip| {
            self.project()
                .midi_clip(clip)
                .map(|(_, clip)| clip.name.clone())
                .or_else(|| {
                    self.project()
                        .audio_clip(clip)
                        .map(|clip| clip.name.clone())
                })
        }) {
            let name: gpui::SharedString = name.into();
            header = header.child(
                div()
                    .id("inspector-selected-clip")
                    .debug_selector(|| "inspector-selected-clip".into())
                    .flex()
                    .min_w_0()
                    .h_4()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().flex_shrink_0().child(self.t(Key::InspectorClip)))
                    .child(
                        div()
                            .id("inspector-clip-name")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .tooltip(crate::ui::tooltip::keyed_tip(name.clone(), "", theme))
                            .child(bounded_picker_label(name)),
                    ),
            );
        }
        header.into_any_element()
    }

    fn inspector_section(
        &self,
        section: InspectorSection,
        summary: String,
        rows: Vec<AnyElement>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let expanded = !self.inspector_collapsed[section as usize];
        div()
            .debug_selector(move || format!("{}-section", section.id()))
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(
                disclosure(
                    section.id(),
                    self.t(section.title()),
                    expanded,
                    &self.theme,
                    cx.listener(move |this, _, _, cx| {
                        this.inspector_collapsed[section as usize] = expanded;
                        cx.notify();
                    }),
                )
                .h_7()
                .px_0()
                .border_1()
                .border_color(gpui::transparent_black())
                .child(
                    div()
                        .id(("inspector-summary", section as usize))
                        .debug_selector(move || format!("{}-summary", section.id()))
                        .max_w(gpui::relative(0.55))
                        .min_w_0()
                        .truncate()
                        .text_color(self.theme.text_muted)
                        .tooltip(crate::ui::tooltip::keyed_tip(
                            summary.clone(),
                            "",
                            &self.theme,
                        ))
                        .child(summary),
                ),
            )
            .when(expanded, |this| {
                this.child(div().flex().flex_col().gap_1().min_w_0().children(rows))
            })
            .into_any_element()
    }

    fn inspector_scope(&self, title: Key) -> impl IntoElement + use<> {
        div()
            .pt_3()
            .pb_1()
            .text_xs()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(self.theme.text)
            .child(self.t(title))
    }

    fn render_track_inspector(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let mut sections: Vec<AnyElement> = Vec::new();
        if let Some(clip) = self
            .selected_clip
            .filter(|clip| self.project().midi_clip(*clip).is_some())
        {
            sections.push(self.inspector_scope(Key::InspectorClip).into_any_element());
            if let Some(recipe) = self.session.clip_recipe(clip) {
                let mut summary = self
                    .t(crate::ui::context_menu::preset_key(recipe.preset))
                    .to_string();
                if self.session.clip_hand_edited(clip) {
                    summary.push_str(" · ");
                    summary.push_str(self.t(Key::PartEditedByHand));
                }
                let rows = self.part_rows(cx);
                sections.push(self.inspector_section(
                    InspectorSection::Generation,
                    summary,
                    rows,
                    cx,
                ));
            }
            let summary = self
                .session
                .clip_transforms(clip)
                .map(|stack| {
                    if stack.is_empty() {
                        self.t(Key::InspectorUnmodified).to_string()
                    } else {
                        let summary = auris_i18n::messages::inspector_performance(
                            self.language(),
                            stack.len(),
                        );
                        let swing = crate::ui::performance::swing_percent(stack);
                        if swing > crate::ui::part::SWING_MIN {
                            format!("{} {swing}% · {summary}", self.t(Key::PartSwing))
                        } else {
                            summary
                        }
                    }
                })
                .unwrap_or_default();
            let rows = self.perform_rows(cx);
            sections.push(self.inspector_section(InspectorSection::Performance, summary, rows, cx));
        }

        let Some(track_id) = self.selected_track else {
            sections.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .pt_3()
                    .child(self.t(Key::InspectorEmptyHint))
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
        let is_drum = track.kind.is_drum();
        let is_singer = track.kind.is_singer();
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

        sections.push(self.inspector_scope(Key::Track).into_any_element());
        let sound_summary = if is_singer {
            if self
                .auto_sing
                .as_ref()
                .is_some_and(|job| job.track == track_id)
            {
                self.t(Key::TakeRendering).to_string()
            } else if self.sung_retry.contains(&track_id) {
                self.t(Key::SingerQueued).to_string()
            } else if let Some(failure) = self.singer_failure(track_id) {
                failure.to_string()
            } else {
                self.singer_voice_label(track_id)
                    .unwrap_or_else(|| self.t(Key::SingerNoVoice).to_string())
            }
        } else {
            instrument_id
                .as_ref()
                .map(|id| self.instrument_label(track_id, id))
                .unwrap_or_default()
        };
        let mut sound_rows = self.singer_rows(track_id, cx);
        if is_singer || instrument_id.is_some() {
            let preset = self.session.track_preset(track_id);
            let id = instrument_id.as_deref().unwrap_or("");
            let family = self.library_sound_icon(id, preset, &sound_summary);
            let artwork = sound_artwork(
                id,
                family,
                is_drum
                    || self
                        .session
                        .track_preset(track_id)
                        .is_some_and(|preset| preset.bank == 128),
                is_singer,
            );
            sound_rows.insert(
                0,
                div()
                    .debug_selector(|| "inspector-sound-artwork".into())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .py_3()
                    .w_full()
                    .bg(theme.surface_raised)
                    .rounded(crate::theme::Metrics::RADIUS_SM)
                    .child(
                        gpui::svg()
                            .path(artwork)
                            .w(gpui::px(150.0))
                            .h(gpui::px(140.0))
                            .text_color(theme.accent),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text)
                            .text_center()
                            .child(sound_summary.clone()),
                    )
                    .into_any_element(),
            );
        }

        if let Some(instrument_id) = instrument_id {
            let name = self.instrument_label(track_id, &instrument_id);
            let subject = PluginSubject::Instrument(track_id);
            sound_rows.push(
                div()
                    .debug_selector(|| "instrument-editor-row".into())
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("inst-name")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .child(name.clone())
                            .tooltip(crate::ui::tooltip::keyed_tip(name.clone(), "", &theme)),
                    )
                    .child(self.plugin_editor_button("inst-open", subject, cx))
                    .child(self.plugin_menu_button("inst-menu", subject, name, cx))
                    .into_any_element(),
            );
            sound_rows.push(
                crate::ui::widgets::bounded_button_enabled(
                    "save-user-sound",
                    self.t(Key::SaveSound),
                    crate::ui::widgets::ButtonStyle::Ghost,
                    crate::ui::widgets::ButtonState::available(false, true),
                    theme.accent_soft,
                    &theme,
                    cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        let initial = this
                            .project()
                            .track(track_id)
                            .map(|track| track.name.clone())
                            .unwrap_or_default();
                        this.open_prompt(crate::ui::prompt::Prompt::new(
                            this.t(Key::SoundName),
                            crate::ui::prompt::PromptTarget::SaveSound(track_id),
                            initial,
                        ));
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if is_drum {
                sound_rows.extend(self.drum_assignment_rows(track_id, cx));
            }
        }
        if !sound_rows.is_empty() {
            sections.push(self.inspector_section(
                InspectorSection::Sound,
                sound_summary,
                sound_rows,
                cx,
            ));
        }
        let mut insert_controls = Vec::new();

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
                    insert_controls.push(
                        // Wrapped so the empty slot can be dragged onto as well as clicked: the
                        // row that adds the next effect is also where the end of the chain is,
                        // and it is the only way to say "last" with the pointer.
                        div()
                            .debug_selector(|| "inspector-insert-add".into())
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
            let subject = PluginSubject::Insert {
                track: Some(track_id),
                slot: slot_id,
            };

            // One row per insert, and its parameters live in the plugin editor rather than
            // underneath it. Expanding every effect in place pushed the rest of the chain down
            // the panel, so a four-effect strip could not be read without scrolling — and the
            // slot you were about to reach for kept moving.
            insert_controls.push(
                div()
                    .debug_selector(move || format!("insert-editor-row-{slot_index}"))
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
                            // Row controls handle their press first; the parent can begin a
                            // reorder only when no other drag has claimed the gesture.
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
                    .child(
                        crate::ui::widgets::button(
                            ("fx-enabled", slot_index),
                            self.t(if enabled { Key::ValueOn } else { Key::ValueOff }),
                            crate::ui::widgets::ButtonStyle::Ghost,
                            enabled,
                            theme.accent_soft,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.toggle_effect(Some(track_id), slot_id);
                                cx.notify();
                            }),
                        )
                        .flex_shrink_0()
                        .tooltip(self.tip(
                            if enabled {
                                Key::MenuEnabled
                            } else {
                                Key::PluginBypassed
                            },
                            "",
                        )),
                    )
                    .child(
                        div()
                            .id(("insert-name", slot_index))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .child(name.clone())
                            .tooltip(crate::ui::tooltip::keyed_tip(name.clone(), "", &theme)),
                    )
                    .child(self.plugin_editor_button(("insert-open", slot_index), subject, cx))
                    .child(self.plugin_menu_button(("insert-menu", slot_index), subject, name, cx))
                    .into_any_element(),
            );
        }

        let summary = auris_i18n::messages::inspector_effects(
            self.language(),
            effect_slots
                .iter()
                .filter(|(_, _, enabled)| *enabled)
                .count(),
            effect_slots.len(),
        );
        sections.push(self.inspector_section(
            InspectorSection::Effects,
            summary,
            insert_controls,
            cx,
        ));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(sections)
            .into_any_element()
    }

    fn focus_parameter_row(&mut self, index: usize, backwards: bool, cx: &mut gpui::Context<Self>) {
        let next = if backwards {
            index.checked_sub(1)
        } else {
            index
                .checked_add(1)
                .filter(|next| *next < self.parameter_list.rows.len())
        };
        let Some(next) = next else {
            cx.propagate();
            return;
        };
        self.parameter_list
            .scroll
            .scroll_to_item(next, gpui::ScrollStrategy::Top);
        self.parameter_list.pending_focus = Some(next);
        cx.notify();
    }

    /// The scrollable parameter body shared by large hosted editors.
    pub(crate) fn parameter_list(
        &mut self,
        descriptors: std::sync::Arc<Vec<ParamDescriptor>>,
        target_for: impl Fn(ParamId) -> ParamTarget + Copy + 'static,
        id_prefix: &'static str,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        if self.parameter_list.rows.len() != descriptors.len() {
            self.parameter_list = ParameterListState {
                scroll: Default::default(),
                rows: std::sync::Arc::new(
                    (0..descriptors.len()).map(|_| cx.focus_handle()).collect(),
                ),
                pending_focus: None,
            };
        }
        let rows = self.parameter_list.rows.clone();
        gpui::uniform_list(
            "pw-body",
            descriptors.len(),
            cx.processor(move |app, range: std::ops::Range<usize>, window, cx| {
                if let Some(next) = app.parameter_list.pending_focus
                    && range.contains(&next)
                {
                    app.parameter_list.pending_focus = None;
                    let focus = rows[next].clone();
                    // Defer until this render pass has installed the destination in tab order.
                    // Its row anchor is not a tab stop; advance to the actual control inside it.
                    window.defer(cx, move |window, _| {
                        window.focus(&focus);
                        window.focus_next();
                    });
                }
                let controls =
                    app.param_controls(&descriptors[range.clone()], target_for, id_prefix, cx);
                range
                    .zip(controls)
                    .map(|(index, control)| {
                        div()
                            .id(("parameter-row", index))
                            .track_focus(&rows[index])
                            .tab_index(0)
                            .tab_stop(false)
                            .pb_1()
                            .child(control)
                            .on_action(cx.listener(
                                move |app, _: &crate::actions::FocusNextPane, _, cx| {
                                    app.focus_parameter_row(index, false, cx);
                                },
                            ))
                            .on_action(cx.listener(
                                move |app, _: &crate::actions::FocusPreviousPane, _, cx| {
                                    app.focus_parameter_row(index, true, cx);
                                },
                            ))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.parameter_list.scroll.clone())
        .debug_selector(|| "pw-body".into())
        .flex_1()
        .min_h_0()
        .p_2()
        .into_any_element()
    }

    /// Builds controls for a slice of a plugin's parameters.
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
                let param_id = descriptor.id.0;
                let target = target_for(descriptor.id);
                let value = self.session.param_value(target, descriptor);
                let element_id = (id_prefix, target_element_key(target, descriptor.id));

                let label = self.param_label(&descriptor.name);
                // The label again for the menu, which the control itself consumes.
                let menu_label = label.clone();
                let value_text = self.format_param(descriptor, value);
                let keyboard_descriptor = descriptor.clone();
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
                        cx.listener(
                            move |this, event: &crate::ui::widgets::SliderKeyboardEvent, _, cx| {
                                this.session.set_param(
                                    target,
                                    keyboard_descriptor.denormalize(event.fraction),
                                );
                                cx.notify();
                            },
                        ),
                    )
                    .debug_selector(move || format!("{id_prefix}-param-{param_id}"))
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
        self.fader_control(id, label, target, value, cx)
            .into_any_element()
    }

    /// A fader with concrete styling, so a zoomed graph can size its control to the viewport.
    pub(crate) fn fader_control(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        target: ParamTarget,
        value: f32,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let descriptor = Session::mixer_descriptor(target)
            .unwrap_or_else(|| ParamDescriptor::new(0u32, "value", "value", 0.0, 1.0, 0.0));
        let keyboard_descriptor = descriptor.clone();
        crate::ui::widgets::value_slider(
            id,
            label,
            self.format_param(&descriptor, value),
            descriptor.normalize(value),
            theme.accent,
            crate::ui::plugin_editor::slider_fill_for(&descriptor),
            slider_keyboard_step(&descriptor),
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
            cx.listener(
                move |this, event: &crate::ui::widgets::SliderKeyboardEvent, _, cx| {
                    this.session
                        .set_param(target, keyboard_descriptor.denormalize(event.fraction));
                    cx.notify();
                },
            ),
        )
        .on_mouse_down(
            gpui::MouseButton::Right,
            Self::opens_menu(cx, move |this, at| this.param_menu(at, target, label)),
        )
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
        self.remember_drum_map(track);
        match self.session.set_hosted_instrument(track, file, clap_id) {
            Ok(()) => self.restore_drum_map_for_source(track),
            Err(error) => self.set_failed_status(self.failure(Key::EditChangeInstrument, &error)),
        }
    }

    /// Adds a VST3 effect to the selected strip.
    pub(crate) fn add_vst3_effect_to_selection(&mut self, file: &std::path::Path, class_id: &str) {
        let track = self.selected_track;
        if let Err(error) = self.session.add_vst3_effect(track, file, class_id) {
            self.set_failed_status(self.failure(Key::MenuAddEffect, &error));
        }
    }

    /// Points the selected track at a VST3 instrument.
    pub(crate) fn set_vst3_instrument_on_selection(
        &mut self,
        file: &std::path::Path,
        class_id: &str,
    ) {
        let Some(track) = self.selected_track else {
            return;
        };
        self.remember_drum_map(track);
        match self.session.set_vst3_instrument(track, file, class_id) {
            Ok(()) => self.restore_drum_map_for_source(track),
            Err(error) => self.set_failed_status(self.failure(Key::EditChangeInstrument, &error)),
        }
    }

    /// Replaces the selected track's instrument.
    pub(crate) fn set_track_instrument(&mut self, instrument_id: &str) {
        let Some(track) = self.selected_track else {
            return;
        };
        self.remember_drum_map(track);
        match self.session.set_track_instrument(track, instrument_id) {
            Ok(()) => self.restore_drum_map_for_source(track),
            Err(error) => self.set_failed_status(self.failure(Key::EditChangeInstrument, &error)),
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
        self.remember_drum_map(track);
        match self.session.set_track_preset(track, preset) {
            Ok(()) => self.restore_drum_map_for_source(track),
            Err(error) => self.set_failed_status(self.failure(Key::EditChoosePreset, &error)),
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

    /// Points an effect at a track to key from, or stops it listening to one.
    ///
    /// A refusal is reported rather than swallowed. The menu only offers sources that can be
    /// used, so getting one here means the routing changed under an open menu — which is worth a
    /// line in the log rather than a click that silently did nothing.
    pub(crate) fn set_effect_sidechain(
        &mut self,
        track: Option<TrackId>,
        slot: EffectSlotId,
        source: Option<TrackId>,
    ) {
        if let Err(error) = self.session.set_effect_sidechain(track, slot, source) {
            self.set_failed_status(self.failure(Key::CmdSetEffectSidechain, &error));
        }
    }
}

/// Presentation artwork follows the shared sound classification and manual corrections.
fn sound_artwork(
    id: &str,
    family: auris_session::SoundIcon,
    drum: bool,
    singer: bool,
) -> &'static str {
    if singer {
        return "sounds/voice.svg";
    }
    if drum {
        return "sounds/drums.svg";
    }
    use auris_session::SoundIcon;
    match family {
        SoundIcon::Piano | SoundIcon::Organ => return "sounds/keys.svg",
        SoundIcon::Guitar | SoundIcon::Bass | SoundIcon::Strings | SoundIcon::Pluck => {
            return "sounds/strings.svg";
        }
        SoundIcon::Brass | SoundIcon::Wind => return "sounds/wind.svg",
        SoundIcon::Drum => return "sounds/drums.svg",
        SoundIcon::Bell => return "sounds/bell.svg",
        SoundIcon::Voice => return "sounds/voice.svg",
        SoundIcon::Synth | SoundIcon::Pad => return "sounds/synth.svg",
        SoundIcon::Sound => {}
    }
    match id {
        "auris.physical.piano" => "sounds/keys.svg",
        "auris.physical.guitar"
        | "auris.physical.electric_guitar"
        | "auris.physical.bass"
        | "auris.physical.violin" => "sounds/strings.svg",
        "auris.physical.clarinet" | "auris.physical.tin_whistle" => "sounds/wind.svg",
        "auris.physical.mallet" | "auris.physical.hammered_dulcimer" => "sounds/mallet.svg",
        "auris.physical.bell" => "sounds/bell.svg",
        "auris.synth.drumkit" | "auris.synth.noisedrum" => "sounds/drums.svg",
        "auris.synth.vocal" | "auris.physical.choir" => "sounds/voice.svg",
        "auris.synth.chiptune" | "auris.synth.fm2" => "sounds/synth.svg",
        _ => "sounds/wave.svg",
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
    use crate::harness::{click, paint, resize, with_a_clip};
    use gpui::{TestAppContext, point, px, size};

    #[gpui::test]
    fn inspector_disclosure_activation_does_not_dispatch_transport_commands(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            this.select_clip(Some(clip));
        });
        paint(&app, cx);
        // There is no audio thread in the harness. Observe the dispatched action rather than
        // a transport atomic that would never acknowledge playback here.
        let transport_commands = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = transport_commands.clone();
        cx.update(|_, cx| {
            cx.observe_keystrokes(move |event, _, _| {
                if event.action.as_ref().is_some_and(|action| {
                    action.as_any().is::<crate::actions::TogglePlay>()
                        || action.as_any().is::<crate::actions::ReturnToZero>()
                }) {
                    observed.set(observed.get() + 1);
                }
            })
            .detach();
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(
            transport_commands.get(),
            1,
            "the observer sees the window's binding"
        );
        transport_commands.set(0);
        click("inspector-performance", cx);
        paint(&app, cx);
        for (key, collapsed) in [("enter", false), ("space", true)] {
            cx.simulate_keystrokes(key);
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
            });
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                assert_eq!(
                    this.inspector_collapsed[InspectorSection::Performance as usize],
                    collapsed
                );
            });
        }
        assert_eq!(
            transport_commands.get(),
            0,
            "activation belongs to the focused disclosure"
        );
    }

    #[gpui::test]
    fn inspector_disclosures_keep_their_state_across_selection_without_editing_the_song(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track, _) = with_a_clip(cx);
        let (generated, other, before) = app.update(cx, |this, _| {
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            let generated = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    ClipRecipe::new(ClipPreset::Bass, 12),
                )
                .unwrap();
            let other = this.session.add_default_instrument_track("Other").unwrap();
            this.session
                .add_midi_clip(other, "Other clip", Ticks::ZERO, Ticks::QUARTER * 4)
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(generated));
            (generated, other, this.project().clone())
        });
        resize(&app, cx, size(px(1920.0), px(3000.0)));
        // GPUI retains old selectors in its debug map. Compare the section that is painted in
        // both states rather than asking whether a previously painted child disappeared.
        for (section, region) in [
            ("inspector-generation", "inspector-generation-section"),
            ("inspector-performance", "inspector-performance-section"),
            ("inspector-sound", "inspector-sound-section"),
            ("inspector-effects", "inspector-effects-section"),
        ] {
            let expanded = cx.debug_bounds(region).unwrap().size.height;
            click(section, cx);
            paint(&app, cx);
            assert!(
                cx.debug_bounds(region).unwrap().size.height < expanded,
                "{section} must hide its rows"
            );
        }
        app.read_with(cx, |this, _| {
            assert_eq!(this.inspector_collapsed, [true; 4])
        });

        app.update(cx, |this, _| this.select_track(other));
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.inspector_collapsed, [true; 4])
        });
        app.update(cx, |this, _| {
            this.select_track(track);
            this.select_clip(Some(generated));
        });
        paint(&app, cx);
        for selector in [
            "inspector-generation-disclosure-collapsed",
            "inspector-performance-summary",
            "inspector-sound-summary",
            "inspector-effects-summary",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        click("inspector-sound", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(!this.inspector_collapsed[InspectorSection::Sound as usize])
        });
        // The disclosure keeps keyboard focus after the pointer opens it.
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.inspector_collapsed[InspectorSection::Sound as usize])
        });
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("space").unwrap(),
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(!this.inspector_collapsed[InspectorSection::Sound as usize])
        });
        cx.simulate_keystrokes("shift-tab");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(!this.inspector_collapsed[InspectorSection::Performance as usize])
        });
        app.read_with(cx, |this, _| assert_eq!(this.project(), &before));
    }

    #[gpui::test]
    fn inspector_selection_stays_above_scrolled_settings_and_drops_a_deselected_clip(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            this.select_clip(Some(clip));
            for _ in 0..18 {
                this.session
                    .add_effect(Some(track), "auris.fx.gain")
                    .unwrap();
            }
        });
        resize(&app, cx, size(px(960.0), px(720.0)));
        app.update(cx, |this, _| {
            this.inspector_scroll.set_offset(point(px(0.0), px(-200.0)))
        });
        paint(&app, cx);
        let selection = cx.debug_bounds("inspector-selection").unwrap();
        let viewport = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
        app.read_with(cx, |this, _| {
            assert!(this.inspector_scroll.offset().y < px(0.0))
        });
        assert!(
            selection.bottom() <= viewport.top(),
            "the target is outside the scrolling settings"
        );
        assert!(cx.debug_bounds("inspector-selected-track").is_some());
        assert!(cx.debug_bounds("inspector-selected-clip").is_some());
        app.update(cx, |this, _| this.select_clip(None));
        paint(&app, cx);
        app.read_with(cx, |this, _| assert_eq!(this.selected_clip, None));
        assert!(cx.debug_bounds("inspector-selected-track").is_some());
        assert!(
            cx.debug_bounds("inspector-selection").unwrap().size.height < selection.size.height
        );
    }

    #[test]
    fn sound_artwork_tracks_classification_and_track_roles() {
        assert_eq!(
            sound_artwork(
                "auris.sampler",
                auris_session::SoundIcon::Piano,
                false,
                false
            ),
            "sounds/keys.svg"
        );
        assert_eq!(
            sound_artwork(
                "auris.sampler",
                auris_session::SoundIcon::Strings,
                false,
                false
            ),
            "sounds/strings.svg"
        );
        assert_eq!(
            sound_artwork(
                "auris.sampler",
                auris_session::SoundIcon::Wind,
                false,
                false
            ),
            "sounds/wind.svg"
        );
        assert_eq!(
            sound_artwork(
                "auris.sampler",
                auris_session::SoundIcon::Piano,
                true,
                false
            ),
            "sounds/drums.svg"
        );
        assert_eq!(
            sound_artwork("", auris_session::SoundIcon::Sound, false, true),
            "sounds/voice.svg"
        );
        assert_eq!(
            sound_artwork(
                "auris.synth.fm2",
                auris_session::SoundIcon::Sound,
                false,
                false
            ),
            "sounds/synth.svg"
        );
        assert_eq!(
            sound_artwork("third.party", auris_session::SoundIcon::Sound, false, false),
            "sounds/wave.svg"
        );
    }

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
