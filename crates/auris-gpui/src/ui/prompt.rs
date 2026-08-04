//! The rename sheet, and the platform input plumbing behind it.
//!
//! gpui hands typed text to whichever view is registered as the window's input handler, so
//! [`AurisApp`] implements [`gpui::EntityInputHandler`] and forwards to the open prompt's
//! [`TextField`]. Registering happens inside the field's paint, which is the only place gpui
//! allows it and conveniently also the only time a prompt is on screen.

use std::ops::Range;

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    Bounds, Context, ElementInputHandler, IntoElement, MouseButton, MouseDownEvent, Pixels,
    SharedString, Window, canvas, div, point, prelude::*, px, size,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

/// What a prompt is editing.
///
/// A key and a chord are typed rather than picked from a list because there is no list worth
/// showing: twelve tonics times thirteen scales is a hundred and fifty-six menu rows, and
/// [`MusicalKey::parse`] already reads `Bb minor` — the thing a musician would have written down
/// anyway. [`Numeral::parse`] does the same for `bVII7`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PromptTarget {
    /// A track's name.
    Track(TrackId),
    /// A clip's name.
    Clip(ClipId),
    /// The key in force from a position on the timeline.
    Key(Ticks),
    /// The chord sounding from a position on the timeline.
    Chord(Ticks),
    /// The seed a generated clip is written from.
    ///
    /// Typed for a different reason than the other three: "another take" is the *next* seed, so
    /// the way back to a take somebody liked is to type the number it had. Undo reaches the same
    /// place while the take is still on the stack, and not afterwards.
    Seed(ClipId),
}

/// What the user was doing when the document turned out to be in the way.
///
/// Carried through the sheet so that answering it finishes the command the user actually gave,
/// rather than dismissing a box and leaving them to give it again.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    /// Empty the document.
    NewProject,
    /// Read another document over this one.
    OpenProject,
    /// Shut the window.
    CloseWindow,
    /// Leave.
    Quit,
}

/// Which button was pressed on a [`Question`].
///
/// Cancel is not one of them: it closes the sheet and nothing else, so it needs no answer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The safe one — Save, or Replace.
    Confirm,
    /// The destructive one — Discard. Only [`Question::Unsaved`] offers it.
    Deny,
}

/// A question the sheet is asking, and what turns on the answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// Something is about to destroy unsaved work.
    ///
    /// Three answers: save first, throw the changes away, or do neither.
    Unsaved(PendingAction),
    /// Saving would replace a project already in that folder.
    ///
    /// Two answers, because there is no third thing to do with it. The path is the one that
    /// would be *written*, which is not the one the system dialog asked about.
    Replace {
        /// What the user chose in the save dialog.
        chosen: std::path::PathBuf,
        /// The document that already exists there.
        existing: std::path::PathBuf,
    },
}

/// What an open sheet is for.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptBody {
    /// A line of text, committed with Return.
    Text {
        /// What gets renamed on commit.
        target: PromptTarget,
        /// The text being edited.
        field: TextField,
    },
    /// A question, answered with a button.
    Ask(Question),
    /// A list of things that went wrong, which the user reads and closes.
    ///
    /// For what will not fit on the status line: every audio file a project could not find,
    /// every complaint a specification's parser has. The status line is one truncated row that
    /// the next command overwrites, so a list put there is a list nobody sees.
    Notice(Vec<SharedString>),
}

/// An open sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// Heading above the body.
    pub title: SharedString,
    /// What it is asking for.
    pub body: PromptBody,
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
            body: PromptBody::Text {
                target,
                field: TextField::new(text),
            },
        }
    }

    /// A prompt asking a question rather than taking a name.
    pub fn ask(title: impl Into<SharedString>, question: Question) -> Self {
        Self {
            title: title.into(),
            body: PromptBody::Ask(question),
        }
    }

    /// A prompt reporting a list of things, with nothing to decide.
    pub fn notice(
        title: impl Into<SharedString>,
        lines: impl IntoIterator<Item = SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            body: PromptBody::Notice(lines.into_iter().collect()),
        }
    }

    /// The text being edited, when this sheet is editing any.
    pub fn field(&self) -> Option<&TextField> {
        match &self.body {
            PromptBody::Text { field, .. } => Some(field),
            PromptBody::Ask(_) | PromptBody::Notice(_) => None,
        }
    }

    /// The same field, to type into.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        match &mut self.body {
            PromptBody::Text { field, .. } => Some(field),
            PromptBody::Ask(_) | PromptBody::Notice(_) => None,
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

    /// Applies the prompt and closes it.
    ///
    /// For a question that is the affirmative answer, which is what Return means on a sheet with
    /// a default button.
    pub(crate) fn commit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let (target, field) = match prompt.body {
            PromptBody::Text { target, field } => (target, field),
            PromptBody::Ask(question) => {
                self.answer(question, Answer::Confirm, window, cx);
                return;
            }
            // Nothing to apply — it has been read, and taking it off the screen is the whole
            // action.
            PromptBody::Notice(_) => return,
        };
        let text = field.content().trim().to_string();
        if text.is_empty() {
            // An empty name would leave an unlabelled row the user cannot tell apart from its
            // neighbours, and an empty key or chord is not a key or a chord.
            self.set_status(self.t(Key::NameCannotBeEmpty));
            return;
        }
        let outcome = match target {
            PromptTarget::Track(track) => self.session.rename_track(track, text),
            PromptTarget::Clip(clip) => self.session.rename_clip(clip, text),
            // These two parse rather than rename, and a rejection has to say what was rejected:
            // `Bbb minor` and `H7` look plausible enough that "invalid input" would not help.
            PromptTarget::Key(at) => match MusicalKey::parse(&text) {
                Some(key) => {
                    self.session.set_key(at, key);
                    Ok(())
                }
                None => {
                    self.set_failed_status(messages::not_a_key(self.language(), &text));
                    return;
                }
            },
            PromptTarget::Chord(at) => match Numeral::parse(&text) {
                Some(chord) => {
                    self.session.set_chord(at, chord);
                    Ok(())
                }
                None => {
                    self.set_failed_status(messages::not_a_chord(self.language(), &text));
                    return;
                }
            },
            PromptTarget::Seed(clip) => match text.parse::<u64>() {
                Ok(seed) => {
                    self.set_clip_seed(clip, seed);
                    Ok(())
                }
                Err(_) => {
                    self.set_failed_status(messages::not_a_seed(self.language(), &text));
                    return;
                }
            },
        };
        if let Err(error) = outcome {
            self.set_failed_status(self.failure(Key::Rename, &error));
        }
    }

    /// Closes the prompt without applying it.
    pub(crate) fn cancel_prompt(&mut self) -> bool {
        self.prompt.take().is_some()
    }

    /// Asks about unsaved work before `next` destroys it.
    ///
    /// Returns `true` when there was nothing to ask about and the caller may go ahead. When it
    /// returns `false` the sheet is open and will finish the command itself.
    pub(crate) fn confirm_discard(&mut self, next: PendingAction) -> bool {
        if !self.session.is_dirty() {
            return true;
        }
        self.open_prompt(Prompt::ask(
            self.t(Key::UnsavedTitle),
            Question::Unsaved(next),
        ));
        false
    }

    /// Acts on a question's answer, having already closed the sheet.
    fn answer(
        &mut self,
        question: Question,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (question, answer) {
            // Save first, then carry on — but only if the save worked. A disk that is full must
            // not be the thing that throws the afternoon away.
            (Question::Unsaved(next), Answer::Confirm) => self.save_then(next, window, cx),
            (Question::Unsaved(next), Answer::Deny) => self.run_pending(next, window, cx),
            (Question::Replace { chosen, .. }, _) => {
                match self.session.save_as_replacing(&chosen) {
                    Ok(report) => self.report_save(&report),
                    Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
                }
            }
        }
    }

    /// Saves, and does `next` if that worked.
    fn save_then(&mut self, next: PendingAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.path().is_none() {
            // Never saved, so this needs a name — and the file dialog is asynchronous, which is
            // why `next` has to travel with it rather than being done when this returns.
            self.save_as_then(Some(next), window, cx);
            return;
        }
        match self.session.save_in_place() {
            Ok(()) => {
                let path = self.session.path().map(|p| p.display().to_string());
                self.set_status(messages::saved(self.language(), &path.unwrap_or_default()));
                self.run_pending(next, window, cx);
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
        }
    }

    /// Does what the user asked for before the document got in the way.
    pub(crate) fn run_pending(
        &mut self,
        next: PendingAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match next {
            PendingAction::NewProject => self.new_project(),
            PendingAction::OpenProject => self.pick_and_open_project(cx),
            PendingAction::CloseWindow => window.remove_window(),
            PendingAction::Quit => cx.quit(),
        }
    }

    /// Handles a keystroke aimed at the open prompt.
    ///
    /// Returns `true` when the key was used, so the caller can stop it reaching the rest of the
    /// application. Only the keys the platform does *not* deliver as text are handled here;
    /// everything else arrives through the input handler, which is what keeps an IME working.
    pub(crate) fn prompt_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let shift = event.keystroke.modifiers.shift;
        // ⌘ on macOS, Ctrl elsewhere. Reading `platform` directly would put select-all on the
        // Windows key, which opens the shell's own menu long before the application sees it.
        let command = event.keystroke.modifiers.secondary();
        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        // A question has no field to type into, so only the two keys that answer it apply.
        let Some(field) = prompt.field_mut() else {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.cancel_prompt();
                }
                "enter" => self.commit_prompt(window, cx),
                _ => return false,
            }
            return true;
        };
        // While the IME is composing, these keys belong to the candidate window, and the
        // platform has already offered them to it before we see them.
        let composing = field.marked().is_some();

        match event.keystroke.key.as_str() {
            "escape" if !composing => {
                self.cancel_prompt();
            }
            "enter" if !composing => {
                self.commit_prompt(window, cx);
            }
            "backspace" => field.backspace(),
            "delete" => field.delete_forward(),
            "left" => field.move_left(shift),
            "right" => field.move_right(shift),
            "home" | "up" => field.move_home(shift),
            "end" | "down" => field.move_end(shift),
            "a" if command => field.select_all(),
            // Copy, cut and paste. A rename box that cannot take a name off the clipboard means
            // retyping a chord symbol or a path by hand every time, and there is nowhere else in
            // the application to type text.
            "c" if command => {
                let selected = field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
            }
            "x" if command => {
                let selected = field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                    field.backspace();
                }
            }
            "v" if command => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = pasted {
                    // One line. A clipboard full of newlines would otherwise be typed into a
                    // field that draws exactly one row of text.
                    field.insert(&text.replace(['\n', '\r'], " "));
                }
            }
            _ => return false,
        }
        true
    }

    /// Draws the sheet over everything else.
    pub(crate) fn render_prompt(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let prompt = self.prompt.as_ref()?;
        let theme = self.theme.clone();
        let title = prompt.title.clone();
        let focus = self.focus.clone();
        let view = cx.entity();

        let (body, buttons) = match &prompt.body {
            PromptBody::Text { field, .. } => (
                self.render_prompt_field(
                    field.content().to_string().into(),
                    field.selection(),
                    field.marked(),
                    focus,
                    view,
                    &theme,
                )
                .into_any_element(),
                self.render_prompt_buttons(None, self.t(Key::Rename).into(), cx),
            ),
            PromptBody::Ask(question) => {
                let (message, confirm, deny) = self.question_text(question);
                (
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(message)
                        .into_any_element(),
                    self.render_prompt_buttons(deny, confirm, cx),
                )
            }
            PromptBody::Notice(lines) => (
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(280.0))
                    .overflow_hidden()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .children(lines.iter().cloned().map(|line| div().child(line)))
                    .into_any_element(),
                self.render_prompt_buttons(None, self.t(Key::Close).into(), cx),
            ),
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(120.0))
                .bg(Theme::translucent(theme.background, 0.55))
                // A sheet asking a question has to be answered, so nothing behind it is hit by
                // any button or by the wheel. Stopping left-click propagation was not enough: a
                // right-click went through the dim and opened a menu on top of the sheet.
                .occlude()
                // A click outside the sheet cancels, which is what every rename box does.
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
                        .child(body)
                        .child(buttons),
                ),
        )
    }

    /// The box the rename sheet types into.
    fn render_prompt_field(
        &self,
        text: SharedString,
        selection: Range<usize>,
        marked: Option<Range<usize>>,
        focus: gpui::FocusHandle,
        view: gpui::Entity<AurisApp>,
        theme: &Theme,
    ) -> impl IntoElement + use<> {
        div()
            .h(FIELD_HEIGHT)
            .w_full()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(theme.accent)
            .child(editable_text(
                text,
                selection,
                marked,
                focus,
                view,
                theme.clone(),
            ))
    }

    /// The row of answers: always Cancel, sometimes a destructive one, then the default.
    ///
    /// Cancel leads and the default trails, so the button under the pointer when a sheet appears
    /// is never the one that throws work away.
    fn render_prompt_buttons(
        &self,
        deny: Option<SharedString>,
        confirm: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
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
            .children(deny.map(|label| {
                button(
                    "prompt-deny",
                    label,
                    ButtonStyle::Normal,
                    false,
                    theme.danger,
                    &theme,
                    cx.listener(|this, _, window, cx| {
                        if let Some(Prompt {
                            body: PromptBody::Ask(question),
                            ..
                        }) = this.prompt.take()
                        {
                            this.answer(question, Answer::Deny, window, cx);
                        }
                        cx.notify();
                    }),
                )
            }))
            .child(button(
                "prompt-ok",
                confirm,
                ButtonStyle::Primary,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| {
                    this.commit_prompt(window, cx);
                    cx.notify();
                }),
            ))
    }

    /// The words a question is asked in: the body, the affirmative, and the destructive answer.
    fn question_text(
        &self,
        question: &Question,
    ) -> (SharedString, SharedString, Option<SharedString>) {
        match question {
            Question::Unsaved(_) => (
                self.t(Key::UnsavedBody).into(),
                self.t(Key::CmdSave).into(),
                Some(self.t(Key::Discard).into()),
            ),
            Question::Replace { existing, .. } => (
                messages::would_replace(self.language(), &existing.display().to_string()).into(),
                self.t(Key::Replace).into(),
                None,
            ),
        }
    }

    /// The field the platform is typing into, when one is open.
    ///
    /// The palette first: opening it closes the rename sheet, so the two are never both open, and
    /// asking in this order means the answer does not depend on that staying true.
    fn writable_field(&mut self) -> Option<&mut TextField> {
        match self.palette.as_mut() {
            Some(palette) => Some(&mut palette.field),
            None => self.prompt.as_mut().and_then(Prompt::field_mut),
        }
    }
}

impl crate::ui::text_field::HasTextField for AurisApp {
    fn field(&mut self) -> Option<&mut TextField> {
        self.writable_field()
    }

    fn readable_field(&self) -> Option<&TextField> {
        match self.palette.as_ref() {
            Some(palette) => Some(&palette.field),
            None => self.prompt.as_ref().and_then(Prompt::field),
        }
    }

    /// Puts the palette's highlight back on the first row.
    ///
    /// Typing narrows the list, and the row that inherits the highlight's position is not the row
    /// that had it. Done here because this is where typing arrives — the key handler never sees a
    /// character, which is what lets an IME compose into the field.
    fn text_changed(&mut self) {
        if let Some(palette) = self.palette.as_mut() {
            palette.selected = 0;
        }
    }
}

crate::entity_input_handler!(AurisApp);

/// A one-line editable text element: the caret, the selection, the IME's pre-edit, and the
/// registration that makes the platform type into it.
///
/// Everything is copied in rather than borrowed because a paint closure has to capture `'static`.
/// Shared by the rename sheet and the command palette, which are the same field with different
/// things underneath it.
pub(crate) fn editable_text<V: gpui::EntityInputHandler>(
    text: SharedString,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    focus: gpui::FocusHandle,
    view: gpui::Entity<V>,
    theme: Theme,
) -> impl IntoElement + use<V> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            // Registering the handler is only legal during paint, and only matters while this
            // element exists — which is exactly as long as the sheet holding it is open.
            window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
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
}

/// Draws the text, its selection and the IME's pre-edit underline.
/// The offset the field has to keep on screen.
///
/// The end of the IME's pre-edit while one is composing — that is where the candidate is being
/// chosen — and otherwise the moving end of the selection, which is the caret.
fn caret_offset(selection: &Range<usize>, marked: Option<&Range<usize>>) -> usize {
    marked.map_or(selection.end, |marked| marked.end)
}

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
        // Everything is drawn relative to a scrolled origin, so a name longer than the box does
        // not walk the caret and everything after it off the right-hand edge. Past about fifty
        // Latin characters — or twenty-five full-width ones, which is a sentence of Japanese —
        // the user was typing blind.
        let caret_at = advance(window, caret_offset(selection, marked.as_ref()));
        let visible = bounds.size.width - FIELD_PADDING * 2.0;
        let scroll = (caret_at - visible).max(px(0.0));
        let origin = point(origin.x - scroll, origin.y);

        // Where the platform should put an IME's candidate list. Only knowable here, from the
        // shaped line, and asked for outside a paint — see `text_field::set_caret_bounds`.
        crate::ui::text_field::set_caret_bounds(Bounds {
            origin: point(origin.x + caret_at, bounds.origin.y),
            size: size(px(1.0), bounds.size.height),
        });

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
