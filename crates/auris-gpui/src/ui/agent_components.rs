//! Shared agent-facing presentation primitives.
//!
//! The editor transcript and the song workshop are different workflows, but they speak with
//! the same two voices. Keeping those voices in one adapter makes the Atelier design language
//! visible in both places and prevents a new entry point from inventing a third chat treatment.

use atelier::agent::{AssistantMessage, ChatNotice, UserMessage};
use gpui::prelude::*;
use gpui::{AnyElement, ElementId, IntoElement, ParentElement, SharedString, div};

use crate::theme::Theme;

/// Introduces a new conversation without impersonating a generated assistant message.
pub(crate) fn conversation_intro(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    theme: &Theme,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.atelier.spacing.md)
        .py(theme.atelier.spacing.xl)
        .child(
            div()
                .text_size(theme.atelier.typography.size_xl)
                .font_weight(theme.atelier.typography.heading_weight)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(theme.atelier.typography.size_md)
                .text_color(theme.text_muted)
                .child(description.into()),
        )
        .into_any_element()
}

/// Read-only text that preserves the source exactly while supporting native selection and copy.
/// Atelier owns the keyboard, focus, selection colour, and UTF-8 boundary handling.
pub(crate) fn selectable_text(
    // atelier: allow — compatibility adapter delegates selection entirely to Atelier.
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    _window: &mut gpui::Window,
    _cx: &mut gpui::App,
) -> AnyElement {
    atelier::SelectableText::new(id, text).into_any_element()
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    struct Fixture;
    impl gpui::Render for Fixture {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .p(gpui::px(80.0))
                .w(gpui::px(560.0))
                .child(selectable_text(
                    "fixture-text",
                    "Plain <text> & **stars**\n\n  日本語",
                    window,
                    cx,
                ))
        }
    }
    #[gpui::test]
    fn plain_selection_preserves_whitespace_and_literal_markup(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::Theme::dark().install(cx));
        let (view, cx) = cx.add_window_view(|_, _| Fixture);
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let bounds = cx.debug_bounds("fixture-text").unwrap();
        crate::harness::press(
            cx,
            bounds.origin + gpui::point(gpui::px(0.5), gpui::px(0.5)),
        );
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let end = bounds.bottom_right() - gpui::point(gpui::px(0.5), gpui::px(0.5));
        crate::harness::drag_to(cx, end);
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        crate::harness::release(cx, end);
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
                Some("Plain <text> & **stars**\n\n  日本語")
            )
        });
        crate::harness::press(
            cx,
            bounds.origin + gpui::point(gpui::px(20.0), gpui::px(5.0)),
        );
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let end = bounds.origin + gpui::point(gpui::px(70.0), gpui::px(5.0));
        crate::harness::drag_to(cx, end);
        crate::harness::release(cx, end);
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            let copied = cx.read_from_clipboard().unwrap().text().unwrap();
            assert!(!copied.is_empty());
            assert!(copied.len() < "Plain <text> & **stars**".len());
            assert!("Plain <text> & **stars**".contains(&copied));
        });
    }
}

/// Draws a person's selectable message with Atelier's trailing bubble geometry and tokens.
pub(crate) fn user_message(
    // atelier: allow — both conversations delegate their person message to UserMessage.
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    _theme: &Theme,
    _window: &mut gpui::Window,
    _cx: &mut gpui::App,
) -> AnyElement {
    UserMessage::new(text).id(id).into_any_element()
}

/// Draws an agent answer: the prose alone, with no avatar or name above it.
///
/// Both conversations have exactly two voices, and the person's own sits in an accent bubble on
/// the trailing edge; whatever is not in a bubble is the agent, so the identity line is left out.
pub(crate) fn assistant_message(
    // atelier: allow — the shared adapter supplies the application's selectable Markdown body.
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    body: Option<AnyElement>,
    streaming: bool,
) -> AnyElement {
    AssistantMessage::new(text)
        .id(id)
        .anonymous()
        .streaming(streaming)
        .when_some(body, |message, body| message.body(body))
        .into_any_element()
}

/// Draws a workshop message through the same shared agent transcript primitives.
///
/// The agent's side is Markdown, rendered the way the editor's transcript renders it; the
/// person's side is shown as typed.
pub(crate) fn workshop_message(
    id: impl Into<ElementId>,
    user: bool,
    text: impl Into<SharedString>,
    theme: &Theme,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let id = id.into();
    let text = text.into();
    if user {
        user_message(id, text, theme, window, cx)
    } else {
        let body = super::agent_markdown::render(
            SharedString::from(format!("workshop-markdown-{id}")),
            text.clone(),
            theme,
            window,
            cx,
        );
        assistant_message(id, text, Some(body), false)
    }
}

/// Draws runtime feedback with the same shape in both workflows.
pub(crate) fn error_notice(text: impl Into<SharedString>) -> AnyElement {
    ChatNotice::error(text).into_any_element()
}

/// Draws a quiet runtime milestone between transcript entries.
pub(crate) fn status_notice(text: impl Into<SharedString>) -> AnyElement {
    ChatNotice::status(text).into_any_element()
}

/// Draws an active phase with an accent-coloured, reduced-motion-aware indicator.
pub(crate) fn thinking_indicator(
    id: &'static str,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .items_center()
        .gap(theme.atelier.spacing.sm)
        .min_w_0()
        .child(atelier::Spinner::new(id).color(theme.atelier.colors.accent))
        .child(
            atelier::Label::new(label)
                .size(atelier::TextSize::Small)
                .color(atelier::TextColor::Muted),
        )
        .into_any_element()
}

/// Starts the shared composer shell used by the editor and workshop.
///
/// Callers append their own editable field and command row so each workflow keeps its existing
/// keyboard and permission semantics while the containment, radius, inset and focus treatment
/// stay identical.
pub(crate) fn composer_frame(
    // atelier: allow — queued Send and Stop coexist with embedded MIDI editing and focus transfer.
    id: impl Into<ElementId>,
    theme: &Theme,
    focused: bool,
    warning: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .relative()
        .flex_shrink_0()
        .min_w_0()
        .m(theme.atelier.spacing.md)
        .p(theme.atelier.spacing.sm)
        .gap(theme.atelier.spacing.xs)
        // Atelier's `Composer`: a raised, hairlined well with the large radius, lit by the accent
        // while it has focus.
        .rounded(theme.atelier.shape.radius_lg)
        .bg(theme.atelier.colors.elevated)
        .border_1()
        .border_color(composer_border(theme, focused, warning))
        .shadow(theme.atelier.elevation.low.clone())
}

/// The composer frame's hairline: the warning tone outranks focus, as `Composer::tone` does,
/// because the mode it marks matters most while the person is typing into the box.
fn composer_border(theme: &Theme, focused: bool, warning: bool) -> gpui::Hsla {
    if warning {
        theme.warning
    } else if focused {
        theme.accent.opacity(0.7)
    } else {
        theme.atelier.colors.border_strong
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;

    #[test]
    fn a_warning_frame_stays_a_warning_while_the_composer_has_focus() {
        let theme = Theme::dark();
        assert_eq!(composer_border(&theme, true, true), theme.warning);
        assert_eq!(composer_border(&theme, false, true), theme.warning);
        assert_eq!(
            composer_border(&theme, true, false),
            theme.accent.opacity(0.7)
        );
        assert_eq!(
            composer_border(&theme, false, false),
            theme.atelier.colors.border_strong
        );
    }
}

/// The commands shared by both composers; each workflow owns their availability and effects.
#[derive(Clone, Copy)]
pub(crate) enum ComposerAction {
    Send,
    Stop,
}

/// A wrapping model/options/action lane inside the shared composer.
pub(crate) fn composer_controls(id: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .flex_wrap()
        .items_center()
        .min_w_0()
        .gap(theme.atelier.spacing.xs)
}

/// Shared keyboard-operable send/stop control, compact in a dock and labelled on a full page.
pub(crate) fn composer_action(
    id: &'static str,
    label: impl Into<SharedString>,
    action: ComposerAction,
    enabled: bool,
    icon_only: bool,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    // As in Atelier's `Composer`: send is the box's one primary commit, stop an ordinary action.
    let (glyph, variant) = match action {
        ComposerAction::Send => (atelier::IconName::ArrowUp, atelier::ButtonVariant::Primary),
        ComposerAction::Stop => (atelier::IconName::Stop, atelier::ButtonVariant::Secondary),
    };
    let command = if icon_only {
        atelier::Button::icon_only(id, glyph)
    } else {
        atelier::Button::new(id, label.clone()).icon(glyph)
    }
    .variant(variant)
    .disabled(!enabled)
    .radius(
        atelier::Theme::concentric(theme.atelier.shape.radius_lg, theme.atelier.spacing.sm)
            .min(theme.atelier.density.control_md / 2.0),
    )
    .tooltip(label)
    .on_click(on_click);
    div()
        .id((ElementId::from(id), "layout"))
        .flex()
        .flex_none()
        .child(command)
}

/// What the person sees on a choice card, besides the card itself: the words the card needs.
pub(crate) struct ChoiceCardText {
    /// The commit of a multiple-choice card.
    pub(crate) send: SharedString,
    /// The caption that says several answers may be picked.
    pub(crate) multiple: SharedString,
}

/// Draws a question the agent asked with fixed answers, in either conversation.
///
/// A card is not a tool row folded into the log: it is the turn's whole point, and the agent
/// is waiting on it. An exclusive card answers on the first click, the way a person answers a
/// question out loud, so each option is an Atelier `ListItem` — a tab stop that Enter and Space
/// also choose. A multiple-choice card holds its picks in `Checkbox`es until Send. Once answered,
/// or once the person typed something else, the card stays readable but takes no more input:
/// `picked` then shows what was chosen.
#[allow(clippy::too_many_arguments)]
pub(crate) fn choice_card(
    id: impl Into<SharedString>,
    card: &auris_session::choices::ChoiceCard,
    picked: &[usize],
    active: bool,
    text: ChoiceCardText,
    theme: &Theme,
    on_option: impl Fn(usize, &mut gpui::Window, &mut gpui::App) + 'static,
    on_send: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    let on_option = std::rc::Rc::new(on_option);
    let options = card.options.iter().enumerate().map(|(index, option)| {
        let chosen = picked.contains(&index);
        let option_id = SharedString::from(format!("{id}-option-{index}"));
        let on_option = on_option.clone();
        if card.multiple {
            let mut checkbox = atelier::Checkbox::new(ElementId::Name(option_id.clone()), chosen)
                .label(option.label.clone())
                .disabled(!active);
            if active {
                checkbox = checkbox.on_click(move |_, window, cx| on_option(index, window, cx));
            }
            div()
                .flex()
                .flex_col()
                .gap(theme.atelier.spacing.xxs)
                .px(theme.atelier.spacing.sm)
                .py(theme.atelier.spacing.xs)
                .min_w_0()
                .child(checkbox)
                .children(option.detail.clone().map(|detail| {
                    // Under the label, past the box and its gap, so the two read as one option.
                    div().pl(gpui::px(16.0) + theme.atelier.spacing.sm).child(
                        atelier::Label::new(detail)
                            .size(atelier::TextSize::Small)
                            .color(atelier::TextColor::Muted),
                    )
                }))
                .into_any_element()
        } else {
            let mut row = atelier::ListItem::new(ElementId::Name(option_id), option.label.clone())
                .icon(if chosen {
                    atelier::IconName::Check
                } else {
                    atelier::IconName::ChevronRight
                })
                .selected(chosen)
                .disabled(!active && !chosen);
            if let Some(detail) = option.detail.clone() {
                row = row.description(detail).description_lines(2);
            }
            if active {
                row = row.on_click(move |_, window, cx| on_option(index, window, cx));
            }
            row.into_any_element()
        }
    });
    let send = (card.multiple && active).then(|| {
        div()
            .flex()
            .justify_end()
            .pt(theme.atelier.spacing.xs)
            .child(
                atelier::Button::new(ElementId::Name(format!("{id}-send").into()), text.send)
                    .icon(atelier::IconName::ArrowUp)
                    .size(atelier::ControlSize::Small)
                    .variant(atelier::ButtonVariant::Primary)
                    .disabled(picked.is_empty())
                    .on_click(move |_, window, cx| on_send(window, cx)),
            )
    });
    let selector = id.clone();
    div()
        .id(ElementId::Name(id.clone()))
        .debug_selector(move || selector.to_string())
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .child(
            atelier::Card::new()
                .p(theme.atelier.spacing.sm)
                .gap(theme.atelier.spacing.xxs)
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.atelier.spacing.xxs)
                        .px(theme.atelier.spacing.sm)
                        .pt(theme.atelier.spacing.xs)
                        .pb(theme.atelier.spacing.xs)
                        .child(
                            atelier::Label::new(card.question.clone())
                                .weight(theme.atelier.typography.label_weight),
                        )
                        .when(card.multiple, |header| {
                            header.child(
                                atelier::Label::new(text.multiple)
                                    .size(atelier::TextSize::XSmall)
                                    .color(atelier::TextColor::Faint),
                            )
                        }),
                )
                .children(options)
                .children(send),
        )
        .into_any_element()
}
