//! The lyrics sheet: every section's words on one page, one of them being typed into.
//!
//! The song sheet's lyric field started as the one-line prompt every other sheet text uses,
//! and words outgrew it immediately: a verse is lines, and a section's words mean most next to
//! the other sections'. So the sheet lays the whole song out — one block per section, in the
//! order the form plays them — and the block that was clicked is a real multi-line editor
//! while the rest stand around it as text. Return breaks a line, because here a line is a
//! phrase; Tab walks to the next section, because a verse is usually followed by writing the
//! chorus.
//!
//! Everything typed lands on the song sheet's dials immediately — the same state the 歌詞
//! buttons light from, and the same state Write reads. Closing this sheet loses nothing and
//! commits nothing, exactly like every other dial: nothing sings until Write.

use auris_i18n::Key;
use gpui::{Context, IntoElement, MouseButton, MouseDownEvent, div, prelude::*, px, relative};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::compose_sheet::{SongDials, section_at};
use crate::ui::text_area::{area_height, area_offset_at, editable_area};
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

/// The open lyrics sheet: which section is being written, and the editor holding its words.
///
/// The field is the working copy for exactly as long as the keystroke takes: every change is
/// copied straight onto the song sheet's dials, so the rest of the application never has to
/// know which of the two is current.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricsSheet {
    /// Index into the song sheet's `sections` of the block being edited.
    pub section: usize,
    /// The words being typed.
    pub field: TextField,
}

/// The sections the sheet lists: each one once, in the order the form first plays them.
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

/// The fewest and most rows a section's editor shows before the sheet itself scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 12;

impl AurisApp {
    /// Opens the lyrics sheet on one section of the song sheet.
    pub(crate) fn open_lyrics_sheet(&mut self, section: usize) {
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
        self.lyrics_sheet = Some(LyricsSheet { section, field });
    }

    /// Moves the editor to another section, leaving the words it held on the dials.
    ///
    /// There is nothing to save first — every keystroke already landed on the sheet — so
    /// switching is only loading the other section's words.
    pub(crate) fn lyrics_pick_section(&mut self, section: usize) {
        if self.lyrics_sheet.is_some() {
            self.open_lyrics_sheet(section);
        }
    }

    /// Copies the editor's words onto the song sheet's dials.
    ///
    /// Called from every path that changes the field — the platform's input handler and the
    /// key handler both — because the dials are what the 歌詞 buttons light from and what
    /// Write reads, and a field that drifted from them would sing something the sheet never
    /// showed.
    pub(crate) fn sync_lyrics_sheet(&mut self) {
        let Some(sheet) = self.lyrics_sheet.as_ref() else {
            return;
        };
        let (section, words) = (sheet.section, sheet.field.content().to_string());
        if let Some(spec) = self
            .song_sheet
            .as_mut()
            .and_then(|dials| dials.sections.get_mut(section))
        {
            spec.lyrics = words;
        }
    }

    /// Handles a keystroke aimed at the open lyrics sheet.
    ///
    /// Returns `true` when the key was used. Return breaks a line rather than committing —
    /// there is nothing to commit; the dials already have every keystroke — and Tab walks the
    /// sections, which is why this sheet has no default button to walk to instead.
    pub(crate) fn lyrics_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sheet) = self.lyrics_sheet.as_mut() else {
            return false;
        };
        let shift = event.keystroke.modifiers.shift;
        let command = event.keystroke.modifiers.secondary();
        // While the IME is composing, Escape, Return, Tab and the arrows belong to the
        // candidate window, and the platform has already offered them to it before we see them.
        let composing = sheet.field.marked().is_some();

        match event.keystroke.key.as_str() {
            "escape" if !composing => {
                self.lyrics_sheet = None;
            }
            "enter" if !composing => {
                sheet.field.insert("\n");
                self.sync_lyrics_sheet();
            }
            "tab" if !composing => {
                let order = self
                    .song_sheet
                    .as_ref()
                    .map(sections_in_form_order)
                    .unwrap_or_default();
                if let Some(at) = order.iter().position(|&index| index == sheet.section) {
                    let next = match shift {
                        true => (at + order.len() - 1) % order.len(),
                        false => (at + 1) % order.len(),
                    };
                    self.lyrics_pick_section(order[next]);
                }
            }
            "up" => sheet.field.move_up(shift),
            "down" => sheet.field.move_down(shift),
            // Copy, cut and paste — and paste keeps its newlines, because here they mean what
            // they mean everywhere else words are written.
            "c" if command => {
                let selected = sheet.field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
            }
            "x" if command => {
                let selected = sheet.field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                    sheet.field.backspace();
                    self.sync_lyrics_sheet();
                }
            }
            "v" if command => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = pasted {
                    sheet
                        .field
                        .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    self.sync_lyrics_sheet();
                }
            }
            key => {
                let effect = sheet.field.apply_key(key, shift, command);
                if effect == crate::ui::text_field::KeyEffect::Changed {
                    self.sync_lyrics_sheet();
                }
                return effect != crate::ui::text_field::KeyEffect::Ignored;
            }
        }
        true
    }

    /// Draws the sheet over the song sheet.
    pub(crate) fn render_lyrics_sheet(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let sheet = self.lyrics_sheet.clone()?;
        let dials = self.song_sheet.clone()?;
        let theme = self.theme.clone();
        let order = sections_in_form_order(&dials);

        let blocks: Vec<gpui::AnyElement> = order
            .iter()
            .map(|&index| self.lyrics_block(&dials, index, &sheet, cx))
            .collect();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.55))
                .occlude()
                // A click outside the sheet closes it — nothing is lost, the dials have
                // everything — which is what every sheet in the application does.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.lyrics_sheet = None;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(560.0))
                        .max_h(relative(0.92))
                        .p_4()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::PromptSectionLyrics)),
                        )
                        .child(
                            div()
                                .id("lyrics-sheet-sections")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .children(blocks),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .child(self.t(Key::HintSectionLyrics)),
                        )
                        .child(div().flex().justify_end().child(button(
                            "lyrics-sheet-close",
                            self.t(Key::Close),
                            ButtonStyle::Primary,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.lyrics_sheet = None;
                                cx.notify();
                            }),
                        ))),
                ),
        )
    }

    /// One section's block: its name over its words, editable where it is the one being
    /// written and standing text everywhere else.
    fn lyrics_block(
        &self,
        dials: &SongDials,
        index: usize,
        sheet: &LyricsSheet,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let Some(spec) = dials.sections.get(index) else {
            return div().into_any_element();
        };
        let active = sheet.section == index;
        let heading = format!(
            "{} · {} {}",
            spec.name,
            spec.bars,
            self.t(Key::SongBarsUnit)
        );

        let words: gpui::AnyElement = if active {
            let field = &sheet.field;
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
                        let Some(sheet) = this.lyrics_sheet.as_ref() else {
                            return;
                        };
                        let text = sheet.field.content().to_string();
                        if let Some(offset) = area_offset_at(window, &text, event.position)
                            && let Some(sheet) = this.lyrics_sheet.as_mut()
                        {
                            sheet.field.place_caret(offset, event.modifiers.shift);
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
                        this.lyrics_pick_section(index);
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
                    .text_color(match active {
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
    fn the_sheet_lists_each_section_once_in_the_order_the_form_plays_them() {
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

        // A section the form never plays is not on the sheet: nothing would sing it.
        dials.form = vec![names[1].clone()];
        assert_eq!(sections_in_form_order(&dials), vec![1]);
    }
}
