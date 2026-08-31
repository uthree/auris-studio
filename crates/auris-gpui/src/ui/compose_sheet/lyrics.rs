//! The lyrics column of the song sheet: every section's words, one of them being typed into.
//!
//! The words started life in the one-line rename prompt, then in a page of their own over the
//! sheet, and both were the same mistake at different sizes: the lyrics were somewhere else.
//! They belong *on the sheet*, beside the form that plays them — so the sheet's third column is
//! the words themselves, one multi-line box per section in the order the form first plays them,
//! and clicking a box makes it a real editor in place. Return breaks a line, because here a
//! line is a phrase; Tab walks to the next section, because a verse is usually followed by
//! writing the chorus; Escape puts the keyboard down without closing anything.
//!
//! Everything typed lands on the song sheet's dials immediately — the state Write reads.
//! Nothing sings until Write, exactly like every other dial.

use auris_i18n::Key;
use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::text_area::{area_height, area_offset_at, editable_area};
use crate::ui::text_field::TextField;

use super::dials::{SongDials, section_at};

/// The section being written into, and the editor holding its words.
///
/// The field is the working copy for exactly as long as the keystroke takes: every change is
/// copied straight onto the song sheet's dials, so the rest of the application never has to
/// know which of the two is current.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricsEdit {
    /// Index into the song sheet's `sections` of the box being edited.
    pub section: usize,
    /// The words being typed.
    pub field: TextField,
}

/// The sections the column lists: each one once, in the order the form first plays them.
///
/// A chorus played three times is still one chorus with one set of words — the same rule the
/// form column lives by — and a section the form never plays is left out because nothing would
/// sing it.
pub fn sections_in_form_order(dials: &SongDials) -> Vec<usize> {
    let mut seen = Vec::new();
    for place in 0..dials.form.len() {
        if let Some(index) = section_at(dials, place)
            && !seen.contains(&index)
        {
            seen.push(index);
        }
    }
    seen
}

/// The fewest and most rows a section's box shows before the column scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 12;

impl AurisApp {
    /// Puts the keyboard into one section's lyrics box.
    pub(crate) fn focus_section_lyrics(&mut self, section: usize) {
        let Some(lyrics) = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.sections.get(section))
            .map(|spec| spec.lyrics.clone())
        else {
            return;
        };
        self.menu = None;
        let mut field = TextField::new(lyrics);
        // Caret at the end, selecting nothing: this editor opens on a verse somebody may have
        // half written, and a rename's select-all would put the whole of it one keystroke from
        // gone.
        field.caret_to_end();
        self.lyrics_edit = Some(LyricsEdit { section, field });
    }

    /// Copies the editor's words onto the song sheet's dials.
    ///
    /// Called from every path that changes the field — the platform's input handler and the
    /// key handler both — because the dials are what Write reads, and a field that drifted
    /// from them would sing something the sheet never showed.
    pub(crate) fn sync_section_lyrics(&mut self) {
        let Some(edit) = self.lyrics_edit.as_ref() else {
            return;
        };
        let (section, words) = (edit.section, edit.field.content().to_string());
        if let Some(spec) = self
            .song_sheet
            .as_mut()
            .and_then(|dials| dials.sections.get_mut(section))
        {
            spec.lyrics = words;
        }
    }

    /// Handles a keystroke aimed at the lyrics box being typed into.
    ///
    /// Returns `true` when the key was used. Return breaks a line rather than committing —
    /// there is nothing to commit; the dials already have every keystroke — and Escape puts
    /// the keyboard down while the sheet stays up.
    pub(crate) fn lyrics_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = self.lyrics_edit.as_mut() else {
            return false;
        };
        let shift = event.keystroke.modifiers.shift;
        let command = event.keystroke.modifiers.secondary();
        // While the IME is composing, Escape, Return, Tab and the arrows belong to the
        // candidate window, and the platform has already offered them to it before we see them.
        let composing = edit.field.marked().is_some();

        match event.keystroke.key.as_str() {
            "escape" if !composing => {
                self.lyrics_edit = None;
            }
            "enter" if !composing => {
                edit.field.insert("\n");
                self.sync_section_lyrics();
            }
            "tab" if !composing => {
                let order = self
                    .song_sheet
                    .as_ref()
                    .map(sections_in_form_order)
                    .unwrap_or_default();
                if let Some(at) = order.iter().position(|&index| index == edit.section) {
                    let next = match shift {
                        true => (at + order.len() - 1) % order.len(),
                        false => (at + 1) % order.len(),
                    };
                    self.focus_section_lyrics(order[next]);
                }
            }
            "up" => edit.field.move_up(shift),
            "down" => edit.field.move_down(shift),
            // Copy, cut and paste — and paste keeps its newlines, because here they mean what
            // they mean everywhere else words are written.
            "c" if command => {
                let selected = edit.field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
            }
            "x" if command => {
                let selected = edit.field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                    edit.field.backspace();
                    self.sync_section_lyrics();
                }
            }
            "v" if command => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = pasted {
                    edit.field
                        .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    self.sync_section_lyrics();
                }
            }
            key => {
                let effect = edit.field.apply_key(key, shift, command);
                if effect == crate::ui::text_field::KeyEffect::Changed {
                    self.sync_section_lyrics();
                }
                return effect != crate::ui::text_field::KeyEffect::Ignored;
            }
        }
        true
    }

    /// The third column: a heading, then one box of words per section the form plays.
    pub(crate) fn song_lyrics_rows(
        &mut self,
        dials: &SongDials,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = vec![
            self.group_heading(Key::PromptSectionLyrics)
                .into_any_element(),
        ];
        for index in sections_in_form_order(dials) {
            rows.push(self.lyrics_box(dials, index, cx));
        }
        rows.push(
            div()
                .text_xs()
                .text_color(self.theme.text_faint)
                .child(self.t(Key::HintSectionLyrics))
                .into_any_element(),
        );
        rows
    }

    /// One section's box: its name over its words, a live editor where it holds the keyboard
    /// and standing text everywhere else.
    fn lyrics_box(&self, dials: &SongDials, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let Some(spec) = dials.sections.get(index) else {
            return div().into_any_element();
        };
        let edit = self
            .lyrics_edit
            .as_ref()
            .filter(|edit| edit.section == index);
        let heading = format!(
            "{} · {} {}",
            spec.name,
            spec.bars,
            self.t(Key::SongBarsUnit)
        );

        let words: AnyElement = if let Some(edit) = edit {
            let field = &edit.field;
            div()
                .h(area_height(field.content(), MIN_ROWS, MAX_ROWS))
                .w_full()
                .rounded(Metrics::RADIUS_SM)
                .bg(theme.surface_sunken)
                .border_1()
                .border_color(theme.accent)
                // A click lands the caret on the character under it; a shift-click extends
                // the selection there, as it does in any editor.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        let Some(edit) = this.lyrics_edit.as_ref() else {
                            return;
                        };
                        let text = edit.field.content().to_string();
                        if let Some(offset) = area_offset_at(window, &text, event.position)
                            && let Some(edit) = this.lyrics_edit.as_mut()
                        {
                            edit.field.place_caret(offset, event.modifiers.shift);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(editable_area(
                    field.content().to_string().into(),
                    field.selection(),
                    field.marked(),
                    self.focus.clone(),
                    cx.entity(),
                    theme.clone(),
                ))
                .into_any_element()
        } else {
            let empty = spec.lyrics.is_empty();
            let lines: Vec<String> = match empty {
                true => vec![self.t(Key::LyricsNoWords).to_string()],
                false => spec.lyrics.split('\n').map(str::to_string).collect(),
            };
            div()
                .w_full()
                .min_h(px(30.0))
                .px_2()
                .py_1()
                .rounded(Metrics::RADIUS_SM)
                .bg(theme.surface_sunken)
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.focus_section_lyrics(index);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .text_size(crate::ui::prompt::TEXT_SIZE)
                .text_color(match empty {
                    true => theme.text_faint,
                    false => theme.text_muted,
                })
                .children(lines.into_iter().map(|line| {
                    div().h(px(20.0)).child(match line.is_empty() {
                        true => " ".to_string(),
                        false => line,
                    })
                }))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(match edit.is_some() {
                        true => theme.text,
                        false => theme.text_muted,
                    })
                    .child(heading),
            )
            .child(words)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_lists_each_section_once_in_the_order_the_form_plays_them() {
        let mut dials = SongDials::default();
        let names: Vec<String> = dials
            .sections
            .iter()
            .map(|section| section.name.clone())
            .collect();
        // A form that repeats itself: the chorus twice, the verse twice.
        dials.form = vec![
            names[0].clone(),
            names[1].clone(),
            names[0].clone(),
            names[1].clone(),
        ];
        assert_eq!(sections_in_form_order(&dials), vec![0, 1]);

        // A section the form never plays is not in the column: nothing would sing it.
        dials.form = vec![names[1].clone()];
        assert_eq!(sections_in_form_order(&dials), vec![1]);
    }
}
