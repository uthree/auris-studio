//! Atelier's [`atelier::TextInput`] standing in for a field whose text the window keeps itself.
//!
//! A search box's query is read in a dozen places that have no [`gpui::App`] to hand — the rows
//! a query filters, the tests that set one up — so the window keeps the text as a plain string
//! and the input is its editor. Each change typed into the input is copied into the string; each
//! change the window makes to the string (clearing it, say) is given back to the input the next
//! time it is drawn, by [`MirroredInput::sync`].

use atelier::{TextInput, TextInputEvent};
use gpui::{App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, Window};

use crate::app::AurisApp;

/// An input and its focus handle, wired to a handler on the window.
pub(crate) struct MirroredInput {
    input: Entity<TextInput>,
    focus: FocusHandle,
}

impl MirroredInput {
    /// A one-line input whose every event — the text changed, Return, Escape — is handed to
    /// `on_event` with the text as it now stands.
    pub(crate) fn new(
        cx: &mut Context<AurisApp>,
        on_event: impl Fn(&mut AurisApp, &TextInputEvent, String, &mut Context<AurisApp>) + 'static,
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

    /// The input's focus handle.
    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
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
