//! The rename sheet, and the platform input plumbing behind it.
//!
//! gpui hands typed text to whichever view is registered as the window's input handler, so
//! [`AurisApp`] implements [`EntityInputHandler`] and forwards to the open prompt's
//! [`TextField`]. Registering happens inside the field's paint, which is the only place gpui
//! allows it and conveniently also the only time a prompt is on screen.

use std::ops::Range;

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, IntoElement, MouseButton,
    MouseDownEvent, Pixels, SharedString, UTF16Selection, Window, canvas, div, point, prelude::*,
    px, size,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

/// What a prompt is renaming.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PromptTarget {
    /// A track's name.
    Track(TrackId),
    /// A clip's name.
    Clip(ClipId),
}

/// An open rename sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// Heading above the field.
    pub title: SharedString,
    /// What gets renamed on commit.
    pub target: PromptTarget,
    /// The text being edited.
    pub field: TextField,
}

impl Prompt {
    /// A prompt editing `text`.
    pub fn new(
        title: impl Into<SharedString>,
        target: PromptTarget,
        text: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            target,
            field: TextField::new(text),
        }
    }
}

/// Font size of the edited text.
const TEXT_SIZE: Pixels = px(13.0);
/// Height of the field's box.
const FIELD_HEIGHT: Pixels = px(28.0);
/// Space between the field's edge and its text.
const FIELD_PADDING: Pixels = px(8.0);

impl AurisApp {
    /// Opens a rename sheet, replacing any open menu.
    pub(crate) fn open_prompt(&mut self, prompt: Prompt) {
        self.menu = None;
        self.prompt = Some(prompt);
    }

    /// Applies the prompt's text and closes it.
    pub(crate) fn commit_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let name = prompt.field.content().trim().to_string();
        if name.is_empty() {
            // An empty name would leave an unlabelled row the user cannot tell apart from its
            // neighbours, and nothing here needs a nameless object.
            self.set_status(self.t(Key::NameCannotBeEmpty));
            return;
        }
        let outcome = match prompt.target {
            PromptTarget::Track(track) => self.session.rename_track(track, name),
            PromptTarget::Clip(clip) => self.session.rename_clip(clip, name),
        };
        if let Err(error) = outcome {
            self.set_status(self.failure(Key::Rename, &error));
        }
    }

    /// Closes the prompt without applying it.
    pub(crate) fn cancel_prompt(&mut self) -> bool {
        self.prompt.take().is_some()
    }

    /// Handles a keystroke aimed at the open prompt.
    ///
    /// Returns `true` when the key was used, so the caller can stop it reaching the rest of the
    /// application. Only the keys the platform does *not* deliver as text are handled here;
    /// everything else arrives through the input handler, which is what keeps an IME working.
    pub(crate) fn prompt_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
        let shift = event.keystroke.modifiers.shift;
        let command = event.keystroke.modifiers.platform;
        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        // While the IME is composing, these keys belong to the candidate window, and the
        // platform has already offered them to it before we see them.
        let composing = prompt.field.marked().is_some();

        match event.keystroke.key.as_str() {
            "escape" if !composing => {
                self.cancel_prompt();
            }
            "enter" if !composing => {
                self.commit_prompt();
            }
            "backspace" => prompt.field.backspace(),
            "delete" => prompt.field.delete_forward(),
            "left" => prompt.field.move_left(shift),
            "right" => prompt.field.move_right(shift),
            "home" | "up" => prompt.field.move_home(shift),
            "end" | "down" => prompt.field.move_end(shift),
            "a" if command => prompt.field.select_all(),
            _ => return false,
        }
        true
    }

    /// Draws the rename sheet over everything else.
    pub(crate) fn render_prompt(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let prompt = self.prompt.as_ref()?;
        let theme = self.theme.clone();
        let title = prompt.title.clone();
        let focus = self.focus.clone();
        let view = cx.entity();

        let text: SharedString = prompt.field.content().to_string().into();
        let selection = prompt.field.selection();
        let marked = prompt.field.marked();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(120.0))
                .bg(Theme::translucent(theme.background, 0.55))
                // A click outside the sheet cancels, which is what every rename box does. It
                // stops there rather than falling through: a click meant to dismiss the sheet
                // must not also move the playhead or reselect a clip behind it.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.cancel_prompt();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(360.0))
                        .p_3()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .child(div().text_sm().text_color(theme.text).child(title))
                        .child(
                            div()
                                .h(FIELD_HEIGHT)
                                .w_full()
                                .rounded(Metrics::RADIUS_SM)
                                .bg(theme.surface_sunken)
                                .border_1()
                                .border_color(theme.accent)
                                .child({
                                    let theme = theme.clone();
                                    canvas(
                                        |_, _, _| (),
                                        move |bounds, _, window, cx| {
                                            // Registering the handler is only legal during paint,
                                            // and only matters while this element exists — which
                                            // is exactly as long as the prompt is open.
                                            window.handle_input(
                                                &focus,
                                                ElementInputHandler::new(bounds, view.clone()),
                                                cx,
                                            );
                                            paint_field(
                                                window,
                                                cx,
                                                bounds,
                                                &text,
                                                &selection,
                                                marked.clone(),
                                                &theme,
                                            );
                                        },
                                    )
                                    .size_full()
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button(
                                    "prompt-cancel",
                                    self.t(Key::Cancel),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.cancel_prompt();
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "prompt-ok",
                                    self.t(Key::Rename),
                                    ButtonStyle::Primary,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.commit_prompt();
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// The prompt's field, when one is open.
    fn field(&mut self) -> Option<&mut TextField> {
        self.prompt.as_mut().map(|prompt| &mut prompt.field)
    }
}

/// Draws the text, its selection and the IME's pre-edit underline.
fn paint_field(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    text: &SharedString,
    selection: &Range<usize>,
    marked: Option<Range<usize>>,
    theme: &Theme,
) {
    let origin = point(
        bounds.origin.x + FIELD_PADDING,
        bounds.origin.y + (bounds.size.height - TEXT_SIZE * 1.35) / 2.0,
    );
    // Measuring by shaping the text up to an offset keeps the caret on the same glyph edge the
    // text is actually drawn at, whatever the font does with the characters in between.
    let advance = |window: &mut Window, offset: usize| -> Pixels {
        if offset == 0 {
            return px(0.0);
        }
        let head: SharedString = text[..offset.min(text.len())].to_string().into();
        let mut run = window.text_style().to_run(head.len());
        run.color = theme.text;
        window
            .text_system()
            .shape_line(head, TEXT_SIZE, &[run], None)
            .width
    };

    paint::clipped(window, bounds, |window| {
        if !selection.is_empty() {
            let start = advance(window, selection.start);
            let end = advance(window, selection.end);
            paint::rect(
                window,
                Bounds {
                    origin: point(origin.x + start, bounds.origin.y + px(3.0)),
                    size: size(end - start, bounds.size.height - px(6.0)),
                },
                Theme::translucent(theme.accent, 0.35),
            );
        }

        paint::label(window, cx, origin, text.clone(), TEXT_SIZE, theme.text);

        // The pre-edit is underlined rather than boxed, matching what every other application
        // on the platform does while an IME is composing.
        if let Some(marked) = marked {
            let start = advance(window, marked.start);
            let end = advance(window, marked.end);
            paint::rect(
                window,
                Bounds {
                    origin: point(
                        origin.x + start,
                        bounds.origin.y + bounds.size.height - px(5.0),
                    ),
                    size: size(end - start, px(1.5)),
                },
                theme.accent,
            );
        }

        if selection.is_empty() {
            let caret = advance(window, selection.start);
            paint::rect(
                window,
                Bounds {
                    origin: point(origin.x + caret, bounds.origin.y + px(4.0)),
                    size: size(px(1.5), bounds.size.height - px(8.0)),
                },
                theme.accent,
            );
        }
    });
}

/// Text input from the platform, including anything an IME composes.
///
/// Every offset crossing this boundary is in UTF-16 units, which is what the platform counts in;
/// [`TextField`] stores byte offsets, so each one is converted rather than passed through.
impl EntityInputHandler for AurisApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.field()?;
        let range = field.byte_range(&range_utf16);
        *adjusted_range = Some(field.utf16_range(&range));
        Some(field.content()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.field()?;
        Some(UTF16Selection {
            range: field.utf16_range(&field.selection()),
            reversed: field.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let field = &self.prompt.as_ref()?.field;
        Some(field.utf16_range(&field.marked()?))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(field) = self.field() {
            field.unmark();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.field() else { return };
        // No range means "whatever is being replaced right now" — the pre-edit if the IME is
        // composing, the selection otherwise.
        match range_utf16 {
            Some(range) => {
                let range = field.byte_range(&range);
                field.replace(range, text);
            }
            None => field.insert(text),
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.field() else { return };
        let range = match range_utf16 {
            Some(range) => field.byte_range(&range),
            None => field.marked().unwrap_or_else(|| field.selection()),
        };
        // This selection is relative to `new_text`, so it is measured against that rather than
        // against the field's own contents.
        let selected = new_selected_range_utf16.map(|range| {
            let start = utf16_to_byte(new_text, range.start);
            start..utf16_to_byte(new_text, range.end).max(start)
        });
        field.replace_and_mark(range, new_text, selected);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Good enough to put the candidate window under the field. Placing it under the
        // composing characters themselves would need the shaped line, which only exists during
        // paint, and the difference is a few pixels of horizontal offset.
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// Converts a UTF-16 offset into a byte offset within `text`.
fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (index, ch) in text.char_indices() {
        if utf16 >= offset {
            return index;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_inside_the_ime_s_own_text_are_converted() {
        assert_eq!(utf16_to_byte("かな", 0), 0);
        assert_eq!(utf16_to_byte("かな", 1), 3);
        assert_eq!(utf16_to_byte("かな", 2), 6);
        assert_eq!(utf16_to_byte("かな", 9), 6, "past the end is the end");
        // A surrogate pair is two UTF-16 units and four bytes.
        assert_eq!(utf16_to_byte("𝄞x", 2), 4);
    }
}
