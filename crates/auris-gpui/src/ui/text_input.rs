//! Atelier's [`atelier::TextInput`] standing in for a field whose text the window keeps itself.
//!
//! A search box's query is read in a dozen places that have no [`gpui::App`] to hand — the rows
//! a query filters, the tests that set one up — so the window keeps the text as a plain string
//! and the input is its editor. Each change typed into the input is copied into the string; each
//! change the window makes to the string (clearing it, say) is given back to the input the next
//! time it is drawn, by [`MirroredInput::sync`].

use atelier::{TextInput, TextInputEvent};
use gpui::{App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, Window};

/// An input and its focus handle, wired to a handler on the view that owns it.
pub(crate) struct MirroredInput {
    input: Entity<TextInput>,
    focus: FocusHandle,
}

impl MirroredInput {
    /// A one-line input whose every event — the text changed, Return, Escape — is handed to
    /// `on_event` with the text as it now stands.
    pub(crate) fn new<V: 'static>(
        cx: &mut Context<V>,
        on_event: impl Fn(&mut V, &TextInputEvent, String, &mut Context<V>) + 'static,
    ) -> Self {
        Self::build(cx, on_event)
    }

    fn build<V: 'static>(
        cx: &mut Context<V>,
        on_event: impl Fn(&mut V, &TextInputEvent, String, &mut Context<V>) + 'static,
    ) -> Self {
        let input = cx.new(TextInput::new);
        cx.subscribe(&input, move |this, input, event: &TextInputEvent, cx| {
            let text = input.read(cx).text().to_string();
            on_event(this, event, text, cx);
        })
        .detach();
        let focus = input.read(cx).focus_handle(cx);
        Self { input, focus }
    }

    /// The input, given `text` first when the window has changed it since it was typed.
    pub(crate) fn sync(&self, text: &str, cx: &mut App) -> Entity<TextInput> {
        if self.input.read(cx).text() != text {
            let text = text.to_string();
            self.input.update(cx, |input, cx| input.set_text(text, cx));
        }
        self.input.clone()
    }

    /// Selects everything the input holds, so the next keystroke types over it.
    pub(crate) fn select_all(&self, cx: &mut App) {
        self.input.update(cx, |input, cx| input.select_all(cx));
    }

    /// The input itself, for reading its selection.
    #[cfg(test)]
    pub(crate) fn input(&self) -> &Entity<TextInput> {
        &self.input
    }

    /// The input's focus handle.
    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The input's focus handle as a tab stop, for the field's frame to track so that Tab
    /// reaches the input: Atelier's input does not put itself in the tab order.
    pub(crate) fn tab_stop(&self) -> FocusHandle {
        self.focus.clone().tab_stop(true)
    }

    /// Whether the input holds the keyboard.
    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Whether an input method is composing in the input.
    pub(crate) fn is_composing(&self, cx: &App) -> bool {
        self.input.read(cx).buffer().marked().is_some()
    }
}

/// Shift-Tab, for a window whose fields are mirrored inputs.
///
/// A field's frame tracks its input's focus as the tab stop and the input then tracks the same
/// handle again, untabbed, inside it. gpui keeps the later of the two entries for the handle, so
/// the stop before a focused input is its own frame and a step back goes nowhere. Tab forward
/// skips the untabbed entry and works, so the stop before the field is found by walking forward
/// round the window's tab order until it comes back. Only the last focus of the walk is seen by
/// anything listening, since focus changes are reported when the window next draws.
pub(crate) fn focus_prev(window: &mut Window, cx: &App) {
    let Some(from) = window.focused(cx) else {
        window.focus_prev();
        return;
    };
    window.focus_prev();
    if window.focused(cx).as_ref() != Some(&from) {
        return;
    }
    let mut before = None;
    for _ in 0..1024 {
        window.focus_next();
        match window.focused(cx) {
            Some(now) if now != from => before = Some(now),
            _ => break,
        }
    }
    window.focus(before.as_ref().unwrap_or(&from));
}
