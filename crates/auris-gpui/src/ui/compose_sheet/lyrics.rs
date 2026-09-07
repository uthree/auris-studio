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
    /// Stable name of the section being edited.
    pub section: String,
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
        let Some((section, lyrics)) = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.sections.get(section))
            .map(|spec| (spec.name.clone(), spec.lyrics.clone()))
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
        let (section, words) = (edit.section.clone(), edit.field.content().to_string());
        if let Some(spec) = self
            .song_sheet
            .as_mut()
            .and_then(|dials| dials.sections.iter_mut().find(|spec| spec.name == section))
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
                if let Some(at) = order.iter().position(|&index| {
                    self.song_sheet
                        .as_ref()
                        .and_then(|dials| dials.sections.get(index))
                        .is_some_and(|spec| spec.name == edit.section)
                }) {
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
    ///
    /// The margin shows what the words would cost as they are typed: a note count per line —
    /// one note per mora — and, in the heading, the bars the sung rhythm needs against the
    /// bars the section has. Measured by the same reading and the same rhythm Write will
    /// use, so the numbers cannot drift from what happens; words that would outrun the
    /// section turn the tally the colour of a problem *before* Write quietly cuts them.
    fn lyrics_box(&self, dials: &SongDials, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let Some(spec) = dials.sections.get(index) else {
            return div().into_any_element();
        };
        let edit = self
            .lyrics_edit
            .as_ref()
            .filter(|edit| edit.section == spec.name);
        let heading = format!(
            "{} · {} {}",
            spec.name,
            spec.bars,
            self.t(Key::SongBarsUnit)
        );

        // Measure what is actually on screen: the editor's text where one is open, the
        // dials otherwise.
        let words_now = edit.map_or(spec.lyrics.as_str(), |edit| edit.field.content());
        let measure = self.session.measure_lyrics(words_now, dials.meter);
        let source = spec
            .melody_from
            .as_ref()
            .and_then(|name| dials.sections.iter().find(|s| &s.name == name));
        let expected = source.map(|s| self.session.measure_lyrics(&s.lyrics, dials.meter));
        let mismatch = !words_now.trim().is_empty()
            && expected.as_ref().is_some_and(|expected| {
                expected.phrases.is_empty()
                    || expected.phrases != measure.phrases
                    || expected.lines.contains(&None)
                    || measure.lines.contains(&None)
            });
        let counts: Vec<gpui::SharedString> = measure
            .lines
            .iter()
            .map(|line| match line {
                Some(0) => "".into(),
                Some(count) => count.to_string().into(),
                // A line nobody can read — kanji with no dictionary — measures as a shrug.
                None => "?".into(),
            })
            .collect();
        let over = measure.bars > spec.bars || mismatch;
        let tally = (measure.notes > 0).then(|| {
            format!(
                "{} {} · {} / {} {}",
                measure.notes,
                self.t(Key::LyricsNotesUnit),
                measure.bars,
                spec.bars,
                self.t(Key::SongBarsUnit)
            )
        });

        let words: AnyElement = if let Some(edit) = edit {
            let field = &edit.field;
            div()
                .h(area_height(field.content(), MIN_ROWS, MAX_ROWS))
                .flex_shrink_0()
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
                    counts,
                    self.focus.clone(),
                    cx.entity(),
                    theme.clone(),
                ))
                .into_any_element()
        } else {
            let empty = spec.lyrics.is_empty();
            let lines: Vec<(String, gpui::SharedString)> = match empty {
                true => vec![(self.t(Key::LyricsNoWords).to_string(), "".into())],
                false => spec
                    .lyrics
                    .split('\n')
                    .map(str::to_string)
                    .zip(counts.into_iter().chain(std::iter::repeat("".into())))
                    .collect(),
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
                .children(lines.into_iter().map(|(line, count)| {
                    div()
                        .h(px(20.0))
                        .flex()
                        .justify_between()
                        .gap_2()
                        .child(div().min_w_0().truncate().child(match line.is_empty() {
                            true => " ".to_string(),
                            false => line,
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(theme.text_faint)
                                .child(count),
                        )
                }))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(match edit.is_some() {
                                true => theme.text,
                                false => theme.text_muted,
                            })
                            .child(heading),
                    )
                    // The running total: notes the words would sing, bars they need against
                    // bars the section has — the colour of a problem once they outrun it.
                    .children(tally.map(|tally| {
                        div()
                            .text_xs()
                            .text_color(match over {
                                true => theme.danger,
                                false => theme.text_faint,
                            })
                            .child(tally)
                    })),
            )
            .child(crate::ui::widgets::button(
                ("song-melody-source", index),
                format!(
                    "{} · {}",
                    self.t(Key::SongMelodyFrom),
                    spec.melody_from
                        .as_deref()
                        .unwrap_or(self.t(Key::SongChordsOwn))
                ),
                crate::ui::widgets::ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                Self::opens_menu(cx, move |this, at| this.song_melody_menu(at, index)),
            ))
            .children(expected.map(|expected| {
                div()
                    .text_xs()
                    .text_color(if mismatch {
                        theme.danger
                    } else {
                        theme.text_faint
                    })
                    .child(format!(
                        "{}: {:?} / {:?}",
                        self.t(Key::SongLyricsMatch),
                        measure.phrases,
                        expected.phrases
                    ))
            }))
            .child(words)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn section_tempo_accepts_decimals_rejects_invalid_input_and_can_follow_the_song(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{open, paint};
        use crate::ui::prompt::{Prompt, PromptTarget};
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.open_prompt(Prompt::new("BPM", PromptTarget::SongSectionTempo(0), ""));
        });
        paint(&app, cx);
        cx.simulate_input("123.45");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert_eq!(
                this.song_sheet.as_ref().unwrap().sections[0].tempo,
                Some(123.45)
            );
        });
        for invalid in ["NaN", "401", "19", "abc"] {
            app.update(cx, |this, _| {
                this.open_prompt(Prompt::new("BPM", PromptTarget::SongSectionTempo(0), ""))
            });
            paint(&app, cx);
            cx.simulate_input(invalid);
            cx.simulate_keystrokes("enter");
            app.read_with(cx, |this, _| {
                assert!(this.prompt.is_some());
                assert_eq!(
                    this.song_sheet.as_ref().unwrap().sections[0].tempo,
                    Some(123.45)
                );
            });
            cx.simulate_keystrokes("secondary-a");
            cx.simulate_keystrokes("backspace");
            cx.simulate_keystrokes("enter");
            app.update(cx, |this, _| {
                assert!(this.prompt.is_none());
                assert_eq!(this.song_sheet.as_ref().unwrap().sections[0].tempo, None);
                this.song_sheet.as_mut().unwrap().sections[0].tempo = Some(123.45);
            });
        }
    }

    #[gpui::test]
    fn mismatched_later_words_leave_the_sheet_and_document_intact(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let mut spec = auris_session::prelude::preset("pop-band").unwrap().spec();
            spec.sections.get_mut("verse").unwrap().lyrics = "さくら".into();
            spec.sections.get_mut("verse2").unwrap().lyrics = "はる".into();
            this.song_sheet = Some(super::super::song_dials(&spec));
            let before = this.project().clone();
            assert!(!this.write_song_from_sheet());
            assert!(this.song_sheet.is_some());
            assert!(this.prompt.is_some());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn choosing_one_drum_source_unifies_every_writer_and_keeps_the_band(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::ui::context_menu::MenuCommand;
        use auris_session::prelude::*;
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_song_sheet();
            let dials = this.song_sheet.as_mut().unwrap();
            let band: Vec<_> = dials
                .parts
                .iter()
                .filter(|p| !p.role.is_drum())
                .cloned()
                .collect();
            let mut other = PartSpec::of_role("other-kit", Role::Kick);
            other.program = Some(gm::Program(8));
            dials.parts.push(other);
            let instrument = PartSpec::of_role("kit", Role::Kick).instrument;
            this.run_menu_command(
                MenuCommand::SongDrumSource {
                    instrument: instrument.clone(),
                    program: Some(16),
                },
                cx,
            );
            let dials = this.song_sheet.as_ref().unwrap();
            assert!(
                dials
                    .parts
                    .iter()
                    .filter(|p| p.role.is_drum())
                    .all(|p| p.instrument == instrument && p.program == Some(gm::Program(16)))
            );
            assert_eq!(
                dials
                    .parts
                    .iter()
                    .filter(|p| !p.role.is_drum())
                    .cloned()
                    .collect::<Vec<_>>(),
                band
            );
            let piece = compose(&super::super::song_spec(dials));
            assert_eq!(
                piece
                    .tracks
                    .iter()
                    .filter(|t| !t.drum_parts.is_empty())
                    .count(),
                1
            );
            this.run_menu_command(
                MenuCommand::SongSinger(Some("C:/Voices/Test.onnx".into())),
                cx,
            );
            let spec = super::super::song_spec(this.song_sheet.as_ref().unwrap());
            assert_eq!(
                super::super::song_dials(&SongSpec::parse(&spec.to_toml()).unwrap()).singer,
                spec.singer
            );
        });
    }

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
