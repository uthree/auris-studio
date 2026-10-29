//! Shared agent-facing presentation primitives.
//!
//! The editor transcript and the song workshop are different workflows, but they speak with
//! the same two voices. Keeping those voices in one adapter makes the Atelier design language
//! visible in both places and prevents a new entry point from inventing a third chat treatment.

use atelier::agent::{ChatNotice, ThinkingIndicator, UserMessage};
use gpui::prelude::*;
use gpui::{AnyElement, ElementId, IntoElement, ParentElement, SharedString, div};

use super::widgets::{ButtonState, ButtonStyle, button_enabled};
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
        .child(atelier::Avatar::agent())
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

/// Draws a person's message using the shared trailing bubble treatment.
pub(crate) fn user_message(id: impl Into<ElementId>, text: impl Into<SharedString>) -> AnyElement {
    let id = id.into();
    UserMessage::new(text).id(id).into_any_element()
}

/// Draws an agent answer: the prose alone, with no avatar or name above it.
///
/// Both conversations have exactly two voices, and the person's own sits in an accent bubble on
/// the trailing edge; whatever is not in a bubble is the agent. Atelier's `AssistantMessage`
/// always draws an identity line, which here only repeats what the layout already says, so the
/// answer is drawn directly with the same type, spacing, arrival and streaming caret.
pub(crate) fn assistant_message(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    body: Option<AnyElement>,
    streaming: bool,
    theme: &Theme,
    cx: &gpui::App,
) -> AnyElement {
    let id = id.into();
    let text = text.into();
    let atelier = &theme.atelier;
    let body = body.unwrap_or_else(|| div().child(text).into_any_element());
    let caret = streaming.then(|| {
        atelier::motion::looping(
            div()
                .w(gpui::px(7.0))
                .h(atelier.typography.size_md)
                .rounded(gpui::px(1.0))
                .bg(atelier.colors.agent),
            (id.clone(), "caret"),
            std::time::Duration::from_millis(1000),
            1.0,
            cx,
            |caret, delta| caret.opacity(gpui::pulsating_between(0.2, 1.0)(delta)),
        )
    });
    let message = div()
        .flex()
        .flex_col()
        .gap(atelier.spacing.sm)
        .text_size(atelier.typography.size_md)
        .text_color(atelier.colors.text)
        .child(body)
        .children(caret);
    atelier::motion::appear(message, id, cx, |row, delta| {
        row.opacity(delta)
            .mt(gpui::px(atelier::motion::lerp(8.0, 0.0, delta)))
    })
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
        user_message(id, text)
    } else {
        let body = super::agent_markdown::render(
            SharedString::from(format!("workshop-markdown-{id}")),
            text.clone(),
            theme,
            window,
            cx,
        );
        assistant_message(id, text, Some(body), false, theme, cx)
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

/// Draws an active agent phase with the shared avatar and reduced-motion-aware indicator.
pub(crate) fn thinking_indicator(id: &'static str, label: impl Into<SharedString>) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .min_w_0()
        .child(ThinkingIndicator::new(label))
        .into_any_element()
}

/// Starts the shared composer shell used by the editor and workshop.
///
/// Callers append their own editable field and command row so each workflow keeps its existing
/// keyboard and permission semantics while the containment, radius, inset and focus treatment
/// stay identical.
pub(crate) fn composer_frame(
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
        .border_color(if focused {
            theme.accent.opacity(0.7)
        } else if warning {
            theme.warning
        } else {
            theme.atelier.colors.border_strong
        })
        .shadow(theme.atelier.elevation.low.clone())
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
    let (glyph, style, color) = match action {
        ComposerAction::Send => (
            atelier::IconName::ArrowUp,
            ButtonStyle::Primary,
            theme.text_on_accent,
        ),
        ComposerAction::Stop => (atelier::IconName::Stop, ButtonStyle::Normal, theme.text),
    };
    button_enabled(
        id,
        "",
        style,
        ButtonState::available(false, enabled),
        color,
        theme,
        on_click,
    )
    .gap(theme.atelier.spacing.xs)
    .when(icon_only, |button| {
        // The square follows the composer's corner, as Atelier's send button does.
        let size = theme.atelier.density.control_md;
        button.size(size).px_0().rounded(
            atelier::Theme::concentric(theme.atelier.shape.radius_lg, theme.atelier.spacing.sm)
                .min(size / 2.0),
        )
    })
    .child(
        atelier::Icon::new(glyph)
            .size(theme.atelier.typography.size_sm)
            .color(if enabled { color } else { theme.text_faint }),
    )
    .when(!icon_only, |button| button.child(label.clone()))
    .tooltip(super::tooltip::keyed_tip(label, "", theme))
}
