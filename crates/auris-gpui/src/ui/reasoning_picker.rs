//! Shared composer popover for reasoning effort and native fast mode.

use auris_i18n::{Key, Language};
use auris_session::ReasoningEffort;
use gpui::{prelude::*, *};

use super::agent_chat::controls::effort_key;
use crate::theme::Theme;

/// Dismiss the topmost reasoning popover before editor-wide Escape bindings run.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = auris_reasoning, no_json)]
pub(crate) struct DismissReasoningPicker;

/// A preference change, applied by the owning conversation.
#[derive(Clone, Copy)]
pub(crate) enum Change {
    Opened,
    Effort(ReasoningEffort),
    Fast,
    Reset,
}

/// A picker and its owner subscription, retained across composer renders.
pub(crate) struct Handle {
    pub(crate) view: Entity<ReasoningPicker>,
    pub(crate) _subscription: Subscription,
}

/// Presentation values supplied by either conversation workflow.
pub(crate) struct Options {
    pub(crate) id: &'static str,
    pub(crate) theme: Theme,
    pub(crate) language: Language,
    pub(crate) choices: Vec<ReasoningEffort>,
    pub(crate) effort: ReasoningEffort,
    pub(crate) fast: bool,
    pub(crate) supports_fast: bool,
    pub(crate) enabled: bool,
}

/// Focus and slider state belong to the popover; preference ownership stays with its caller.
pub(crate) struct ReasoningPicker {
    pub(crate) options: Options,
    focus: FocusHandle,
    slider_focus: FocusHandle,
    open: bool,
    slider_choices: Vec<ReasoningEffort>,
}

impl EventEmitter<Change> for ReasoningPicker {}

impl ReasoningPicker {
    pub(crate) fn new(options: Options, focus: FocusHandle, cx: &mut Context<Self>) -> Self {
        let slider_choices = normalized_choices(&options.choices);
        Self {
            options,
            focus,
            slider_focus: cx.focus_handle(),
            open: false,
            slider_choices,
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.focus.focus(window);
        cx.notify();
    }
}

/// Keep provider order while always offering a distinct provider-default position.
pub(crate) fn normalized_choices(choices: &[ReasoningEffort]) -> Vec<ReasoningEffort> {
    let mut result = vec![ReasoningEffort::Default];
    for effort in choices {
        if !result.contains(effort) {
            result.push(*effort);
        }
    }
    result
}

impl Render for ReasoningPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.slider_choices = normalized_choices(&self.options.choices);
        let theme = self.options.theme.clone();
        let language = self.options.language;
        let trigger_label = match self.options.effort {
            ReasoningEffort::Default => Key::AgentEffortDefaultShort,
            ReasoningEffort::Minimal => Key::AgentEffortMinimalShort,
            ReasoningEffort::Medium => Key::AgentEffortMediumShort,
            ReasoningEffort::Xhigh => Key::AgentEffortXhighShort,
            ReasoningEffort::Max => Key::AgentEffortMaxShort,
            effort => effort_key(effort),
        }
        .get(language);
        let enabled = self.options.enabled;
        if !enabled {
            self.open = false;
        }
        let index = self
            .slider_choices
            .iter()
            .position(|e| *e == self.options.effort);
        let id = self.options.id;
        let was_open = self.open;
        let fast = self.options.fast && self.options.supports_fast;
        div()
            .relative()
            .flex_shrink_0()
            .child(
                super::widgets::menu_picker(
                    id,
                    // Atelier's `EffortButton`: the bolt goes in front of the level while fast
                    // mode is on, and the button latches.
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.atelier.spacing.xxs)
                        .h_full()
                        .min_w_0()
                        .overflow_hidden()
                        .when(fast, |row| {
                            row.child(super::icons::icon(
                                super::icons::Icon::Bolt,
                                px(12.),
                                theme.accent_text,
                            ))
                        })
                        .child(div().min_w_0().truncate().child(trigger_label)),
                    &theme,
                    cx.listener(move |this, _, window, cx| {
                        if !this.options.enabled {
                            return;
                        }
                        this.open = !was_open;
                        if this.open {
                            cx.emit(Change::Opened);
                            this.slider_focus.focus(window);
                        } else {
                            this.focus.focus(window);
                        }
                        cx.notify();
                    }),
                )
                .track_focus(&self.focus.clone().tab_stop(enabled))
                .w_16()
                .bg(if self.open {
                    theme.surface_hover
                } else if fast {
                    theme.atelier.colors.selected
                } else {
                    gpui::transparent_black()
                })
                .border_color(gpui::transparent_black())
                .when(!enabled, |button| button.opacity(0.5))
                .tooltip(super::tooltip::keyed_tip(
                    Key::AgentEffort.get(language),
                    "",
                    &theme,
                )),
            )
            .when(self.open, |root| {
                root.child(gpui::deferred(
                    div().absolute().bottom_full().right_0().pb_2().child(
                        gpui::anchored()
                            .anchor(gpui::Corner::BottomRight)
                            .snap_to_window_with_margin(theme.atelier.spacing.sm)
                            .child(self.card(index, language, cx)),
                    ),
                ))
            })
    }
}

impl ReasoningPicker {
    /// Atelier's effort card, in a frame that holds the keyboard while it is open so the arrow
    /// keys step the level straight away, as they did when the card opened on its slider.
    fn card(
        &self,
        index: Option<usize>,
        language: Language,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let popup_id = format!("{}-popup", self.options.id);
        let levels: Vec<SharedString> = self
            .slider_choices
            .iter()
            .map(|effort| SharedString::from(effort_key(*effort).get(language)))
            .collect();
        // A saved level this model does not offer, and fast mode's cost, are the two things the
        // card has to say in words.
        let note = if index.is_none() {
            Some(Key::ReasoningUnavailable.get(language))
        } else if self.options.fast {
            Some(Key::AgentFastModeHelp.get(language))
        } else {
            None
        };
        let mut picker = atelier::agent::EffortPicker::new(
            SharedString::from(format!("{}-picker", self.options.id)),
            levels,
            index.unwrap_or(0),
        )
        // `normalized_choices` always puts the provider's default first.
        .default_level(0)
        .fast(self.options.fast)
        .show_fast(self.options.supports_fast)
        .on_select(cx.listener(|this, level: &usize, _, cx| {
            if this.options.enabled
                && let Some(effort) = this.slider_choices.get(*level)
            {
                cx.emit(Change::Effort(*effort));
                cx.notify();
            }
        }))
        .on_fast(cx.listener(|_, _, _, cx| cx.emit(Change::Fast)))
        .on_reset(cx.listener(|_, _, _, cx| cx.emit(Change::Reset)));
        if let Some(note) = note {
            picker = picker.note(note);
        }
        div()
            .id(SharedString::from(popup_id.clone()))
            .debug_selector(move || popup_id.clone()) // atelier: allow — the frame holding the keyboard, not the card.
            .track_focus(&self.slider_focus)
            .key_context("AurisReasoningPicker")
            .occlude()
            .tab_group()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DismissReasoningPicker, window, cx| {
                this.dismiss(window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if !this.options.enabled {
                    return;
                }
                let last = this.slider_choices.len().saturating_sub(1);
                let index = this
                    .slider_choices
                    .iter()
                    .position(|e| *e == this.options.effort)
                    .unwrap_or(0);
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => index.saturating_sub(1),
                    "right" | "up" => (index + 1).min(last),
                    "home" => 0,
                    "end" => last,
                    _ => return,
                };
                if next != index {
                    cx.emit(Change::Effort(this.slider_choices[next]));
                }
                cx.stop_propagation();
            }))
            .child(picker)
    }
}
