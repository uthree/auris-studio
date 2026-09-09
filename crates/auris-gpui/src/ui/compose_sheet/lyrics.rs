//! The lyrics column of the song sheet: every section's words, one of them being typed into.
//!
//! The words started life in the one-line rename prompt, then in a page of their own over the
//! sheet, and both were the same mistake at different sizes: the lyrics were somewhere else.
//! They belong *on the sheet*, beside the song controls — the lyrics column is
//! the words themselves, one multi-line box per section in the order the form first plays them,
//! and clicking a box makes it a real editor in place. Return breaks a line, because here a
//! line is a phrase; Tab walks to the next section, because a verse is usually followed by
//! writing the chorus; Escape puts the keyboard down without closing anything.
//!
//! Everything typed lands on the song sheet's dials immediately — the state Write reads.
//! Nothing sings until Write, exactly like every other dial.

use auris_i18n::Key;
use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, ScrollHandle, canvas, div, prelude::*, px,
};

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
    /// Keeps the editor attached to the current form and the words Write will read.
    ///
    /// Form edits can remove its section, and replacing the sheet can change its words without
    /// a keystroke. Reconcile before displaying or accepting input, leaving the caret and IME
    /// composition alone whenever the words still agree.
    pub(crate) fn reconcile_section_lyrics(&mut self) {
        let Some(edit) = self.lyrics_edit.as_mut() else {
            return;
        };
        let Some(section) = self
            .song_sheet
            .as_ref()
            .filter(|dials| dials.form.contains(&edit.section))
            .and_then(|dials| dials.sections.iter().find(|spec| spec.name == edit.section))
        else {
            self.lyrics_edit = None;
            return;
        };
        if edit.field.content() != section.lyrics {
            edit.field = TextField::new(section.lyrics.clone());
            edit.field.caret_to_end();
        }
    }

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
        self.song_lyrics_reveal = Some(section.clone());
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
        self.reconcile_section_lyrics();
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

    /// The lyrics column: a heading, then one box of words per section the form plays.
    pub(crate) fn song_lyrics_rows(
        &mut self,
        dials: &SongDials,
        scroll: &ScrollHandle,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let reveal_section = self.song_lyrics_reveal.take();
        let mut rows: Vec<AnyElement> = vec![
            self.group_heading(Key::PromptSectionLyrics)
                .into_any_element(),
        ];
        for index in sections_in_form_order(dials) {
            rows.push(self.lyrics_box(
                dials,
                index,
                reveal_section.as_deref() == Some(&dials.sections[index].name),
                scroll,
                cx,
            ));
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
    /// The margin counts moras and checks whether the fixed section can hold the words,
    /// using the same rhythm allocator as Write.
    fn lyrics_box(
        &self,
        dials: &SongDials,
        index: usize,
        reveal: bool,
        scroll: &ScrollHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            section_label(self, &spec.name),
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
        let fits = measure.fits_in_bars(dials.meter, spec.bars);
        let over = fits == Some(false) || mismatch;
        let readable = !measure.lines.contains(&None);
        let show_counts = !words_now.trim().is_empty();
        let counts: Vec<gpui::SharedString> = measure
            .lines
            .iter()
            .map(|line| match line {
                _ if !show_counts => "".into(),
                Some(0) => "".into(),
                Some(count) => count.to_string().into(),
                // A line nobody can read — kanji with no dictionary — measures as a shrug.
                None => "?".into(),
            })
            .collect();
        let tally = (!words_now.trim().is_empty()).then(|| {
            if !readable {
                return self.t(Key::SongLyricsUnreadable).to_string();
            }
            self.t(Key::SongLyricsEstimate)
                .replace("{notes}", &measure.notes.to_string())
                .replace("{bars}", &spec.bars.to_string())
        });
        let capacity = if readable && measure.notes > 0 {
            let key = if fits == Some(true) {
                Key::SongLyricsFits
            } else {
                Key::SongLyricsOverflow
            };
            Some(self.t(key).to_string())
        } else {
            None
        };
        let shared_status = source
            .filter(|_| !words_now.trim().is_empty())
            .map(|source| {
                let expected = expected.as_ref().unwrap();
                let key = if !readable || expected.lines.contains(&None) {
                    Key::SongLyricsMatchUnreadable
                } else if expected.notes == 0 {
                    Key::SongLyricsOriginalEmpty
                } else if expected.phrases == measure.phrases {
                    Key::SongLyricsMatched
                } else if expected.notes == measure.notes {
                    Key::SongLyricsPhraseMismatch
                } else {
                    Key::SongLyricsNoteMismatch
                };
                (
                    key,
                    self.t(key)
                        .replace("{section}", &section_label(self, &source.name))
                        .replace("{actual}", &measure.notes.to_string())
                        .replace("{expected}", &expected.notes.to_string()),
                )
            });

        let words: AnyElement = if let Some(edit) = edit {
            let field = &edit.field;
            div()
                .debug_selector(move || format!("song-lyrics-editor-{index}"))
                .relative()
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
                .when(reveal, |this| {
                    let scroll = scroll.clone();
                    let section = spec.name.clone();
                    let app = cx.entity().downgrade();
                    this.child(
                        canvas(
                            move |bounds, _, cx| {
                                // Measure after the display has expanded into an editor. The
                                // previous frame's bounds can be shorter or off screen entirely.
                                let viewport = scroll.bounds();
                                let offset = scroll.offset();
                                let margin = px(8.0);
                                let dy = if bounds.bottom() > viewport.bottom() - margin
                                    || bounds.size.height > viewport.size.height - margin * 2.0
                                {
                                    viewport.bottom() - margin - bounds.bottom()
                                } else if bounds.top() < viewport.top() + margin {
                                    viewport.top() + margin - bounds.top()
                                } else {
                                    px(0.0)
                                };
                                if dy != px(0.0) {
                                    let scroll = scroll.clone();
                                    let section = section.clone();
                                    let app = app.clone();
                                    // Finish the current render before mutating its entity. A
                                    // deferred effect also works without a platform frame tick.
                                    cx.defer(move |cx| {
                                        let Some(app) = app.upgrade() else {
                                            return;
                                        };
                                        app.update(cx, |this, cx| {
                                            if this
                                                .lyrics_edit
                                                .as_ref()
                                                .is_some_and(|edit| edit.section == section)
                                                && scroll.offset() == offset
                                            {
                                                scroll.set_offset(gpui::point(
                                                    offset.x,
                                                    (offset.y + dy).clamp(
                                                        -scroll.max_offset().height,
                                                        px(0.0),
                                                    ),
                                                ));
                                                cx.notify();
                                            }
                                        });
                                    });
                                }
                            },
                            |_, _, _, _| (),
                        )
                        .absolute()
                        .inset_0(),
                    )
                })
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
                div().flex().items_center().justify_between().gap_2().child(
                    div()
                        .text_xs()
                        .text_color(match edit.is_some() {
                            true => theme.text,
                            false => theme.text_muted,
                        })
                        .child(heading),
                ),
            )
            .children(tally.map(|tally| {
                div()
                    .debug_selector(move || format!("song-lyrics-estimate-{index}"))
                    .text_xs()
                    .text_color(if over || !readable {
                        theme.danger
                    } else {
                        theme.text_muted
                    })
                    .child(tally)
            }))
            .children(capacity.map(|capacity| {
                div()
                    .debug_selector(move || format!("song-lyrics-capacity-{index}"))
                    .text_xs()
                    .text_color(if fits == Some(false) {
                        theme.danger
                    } else {
                        theme.text_muted
                    })
                    .child(capacity)
            }))
            .children(shared_status.map(|(key, status)| {
                div()
                    .debug_selector(move || format!("song-lyrics-match-{index}"))
                    .text_xs()
                    .text_color(if mismatch {
                        theme.danger
                    } else {
                        theme.text_muted
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("song-lyrics-status-{index}-{key:?}"))
                            .child(status),
                    )
                    .when(
                        mismatch
                            && readable
                            && expected
                                .as_ref()
                                .is_some_and(|m| m.notes > 0 && !m.lines.contains(&None)),
                        |this| {
                            this.child(
                                div().child(
                                    self.t(Key::SongLyricsCounts)
                                        .replace("{actual}", &phrase_counts(self, &measure.phrases))
                                        .replace(
                                            "{expected}",
                                            &phrase_counts(
                                                self,
                                                &expected.as_ref().unwrap().phrases,
                                            ),
                                        ),
                                ),
                            )
                        },
                    )
            }))
            .child(words)
            .into_any_element()
    }
}

/// Render phrase sizes without Rust's debug-list notation.
fn phrase_counts(app: &AurisApp, counts: &[usize]) -> String {
    if counts.is_empty() {
        app.t(Key::LyricsNoWords).to_string()
    } else {
        counts
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// Give preset section names human labels while preserving custom names and stored identifiers.
pub(super) fn section_label(app: &AurisApp, name: &str) -> String {
    let mut labels: Vec<_> = app.song_sheet.as_ref().map_or_else(Vec::new, |dials| {
        dials
            .sections
            .iter()
            .map(|section| {
                (
                    section.name.as_str(),
                    translated_section_label(app, &section.name),
                )
            })
            .collect()
    });
    let target = labels
        .iter()
        .position(|(identifier, _)| *identifier == name)
        .unwrap_or_else(|| {
            labels.push((name, translated_section_label(app, name)));
            labels.len() - 1
        });
    loop {
        let collisions: Vec<_> = labels
            .iter()
            .map(|(identifier, label)| {
                labels
                    .iter()
                    .any(|(other, candidate)| other != identifier && candidate == label)
            })
            .collect();
        if !collisions.iter().any(|collides| *collides) {
            return labels.swap_remove(target).1;
        }
        // An identifier added for clarity can itself match a custom section's literal name.
        // Resolve those collisions too, using the same simultaneous updates at every call site.
        for ((identifier, label), collides) in labels.iter_mut().zip(collisions) {
            if collides {
                *label = format!("{label} ({identifier})");
            }
        }
    }
}

/// Translate without disambiguation so every occurrence can compare the same base labels.
fn translated_section_label(app: &AurisApp, name: &str) -> String {
    for (prefix, key) in [
        ("verse", Key::SongVerseLabel),
        ("pre", Key::SongPreChorusLabel),
        ("chorus", Key::SongChorusLabel),
        ("bridge", Key::SongBridgeLabel),
    ] {
        if let Some(suffix) = name.strip_prefix(prefix) {
            let suffix = suffix.trim();
            let number = if suffix.is_empty() {
                Some(1)
            } else {
                suffix.parse::<u32>().ok()
            };
            if let Some(number) = number {
                return app.t(key).replace("{n}", &number.to_string());
            }
        }
    }
    match name {
        "intro" => app.t(Key::SongIntroLabel).to_string(),
        "outro" => app.t(Key::SongOutroLabel).to_string(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
#[path = "lyrics_scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn choosing_a_style_replaces_the_lyrics_editor_and_the_words_saved(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{choose, click, open, paint};
        use crate::ui::context_menu::MenuCommand;
        use crate::ui::text_field::HasTextField;
        use auris_session::prelude::{SongSpec, preset};

        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.focus_section_lyrics(0);
        });
        paint(&app, cx);
        cx.simulate_input("さくらさいた");
        app.update(cx, |this, _| {
            let field = this.field().unwrap();
            field.replace_and_mark(field.selection(), "は", None);
            this.text_changed();
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.lyrics_edit.as_ref().unwrap().field.marked().is_some());
        });
        click("song-style", cx);
        paint(&app, cx);
        choose(&app, cx, &MenuCommand::SongPreset("pop-band"));
        paint(&app, cx);
        app.update(cx, |this, _| {
            assert!(
                this.lyrics_edit.is_none(),
                "the old editor is retired with its sheet"
            );
            assert_eq!(
                this.song_sheet.as_ref().unwrap(),
                &super::super::song_dials(&preset("pop-band").unwrap().spec())
            );
            this.focus_section_lyrics(0);
        });
        paint(&app, cx);
        cx.simulate_input("はるがきた");
        app.read_with(cx, |this, _| {
            let edit = this.lyrics_edit.as_ref().unwrap();
            let spec = super::super::song_spec(this.song_sheet.as_ref().unwrap());
            assert_eq!(edit.field.content(), "はるがきた");
            assert_eq!(spec.sections[&edit.section].lyrics, edit.field.content());
            let saved = SongSpec::parse(&spec.to_toml()).unwrap();
            assert_eq!(saved.sections[&edit.section].lyrics, edit.field.content());
        });
    }

    #[gpui::test]
    fn song_menus_own_navigation_editing_keys_and_text_until_closed(cx: &mut gpui::TestAppContext) {
        use crate::harness::{open, paint};
        use crate::ui::context_menu::{ContextMenu, MenuCommand};
        use crate::ui::text_field::HasTextField;

        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.focus_section_lyrics(0);
        });
        paint(&app, cx);
        cx.simulate_input("さくら");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("さいた");
        let before = app.update(cx, |this, _| {
            let before = this.lyrics_edit.clone();
            this.open_menu(
                ContextMenu::new(gpui::point(px(120.0), px(120.0)), "Groove")
                    .item("Straight", MenuCommand::SongGroove("straight"))
                    .item("Swing", MenuCommand::SongGroove("swing")),
            );
            before
        });
        paint(&app, cx);
        cx.simulate_keystrokes("down down");
        app.read_with(cx, |this, _| {
            assert_eq!(this.menu.as_ref().unwrap().highlighted, Some(1));
            assert!(this.readable_field().is_none());
        });
        for key in ["left", "right", "backspace", "delete", "secondary-a", "tab"] {
            cx.simulate_keystrokes(key);
        }
        cx.simulate_input("隠れた入力");
        app.update(cx, |this, _| {
            assert!(
                this.field().is_none(),
                "IME insertions have no covered field"
            );
            assert_eq!(this.lyrics_edit, before);
        });
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            assert_eq!(this.song_sheet.as_ref().unwrap().groove, "swing");
            assert_eq!(
                this.lyrics_edit, before,
                "Return chooses without adding a lyric line"
            );
        });
        cx.simulate_input("はる");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.lyrics_edit.as_ref().unwrap().field.content(),
                "さくら\nさいたはる"
            );
        });
        app.update(cx, |this, _| {
            this.open_menu(
                ContextMenu::new(gpui::point(px(120.0), px(120.0)), "Groove")
                    .item("Straight", MenuCommand::SongGroove("straight")),
            );
        });
        paint(&app, cx);
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            assert!(
                this.lyrics_edit.is_some(),
                "Escape dismisses only the foreground menu"
            );
        });
    }

    #[gpui::test]
    fn removing_the_last_playing_of_a_section_releases_its_lyrics_editor(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{click, open, paint, resize};

        let (app, cx) = open(cx);
        resize(&app, cx, gpui::size(px(1600.0), px(2000.0)));
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.song_advanced = true;
            let dials = this.song_sheet.as_mut().unwrap();
            dials.form = vec![
                dials.sections[0].name.clone(),
                dials.sections[0].name.clone(),
                dials.sections[1].name.clone(),
            ];
            this.focus_section_lyrics(0);
        });
        paint(&app, cx);
        cx.simulate_input("さくら");
        let before = app.read_with(cx, |this, _| this.lyrics_edit.clone());
        click("song-form-remove-0", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.lyrics_edit, before,
                "a repeated section is still editable"
            );
        });
        click("song-form-remove-0", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.lyrics_edit.is_none(),
                "there is no invisible editor after removal"
            );
            assert_eq!(this.song_sheet.as_ref().unwrap().form.len(), 1);
        });
    }

    #[gpui::test]
    fn replacing_the_focused_form_section_releases_its_lyrics_editor(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{open, paint};
        use crate::ui::context_menu::MenuCommand;

        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            this.open_song_sheet();
            this.song_sheet.as_mut().unwrap().form.truncate(1);
            this.focus_section_lyrics(0);
            this.run_menu_command(
                MenuCommand::SongFormName {
                    place: 0,
                    name: "new verse".into(),
                },
                cx,
            );
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.lyrics_edit.is_none());
            assert_eq!(
                this.song_sheet.as_ref().unwrap().sections[0].name,
                "new verse"
            );
        });
    }

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
    fn basic_lyrics_show_mismatch_feedback_without_an_empty_count_warning(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let later = app.update(cx, |this, _| {
            let spec = auris_session::prelude::preset("pop-band").unwrap().spec();
            this.song_sheet = Some(super::super::song_dials(&spec));
            this.song_sheet
                .as_ref()
                .unwrap()
                .sections
                .iter()
                .position(|s| s.name == "verse2")
                .unwrap()
        });
        crate::harness::paint(&app, cx);
        let selector: &'static str =
            Box::leak(format!("song-lyrics-match-{later}").into_boxed_str());
        assert!(cx.debug_bounds(selector).is_none());
        app.update(cx, |this, _| {
            for section in &mut this.song_sheet.as_mut().unwrap().sections {
                if section.name == "verse" {
                    section.lyrics = "さくら".into();
                }
                if section.name == "verse2" {
                    section.lyrics = "はる".into();
                }
            }
        });
        crate::harness::paint(&app, cx);
        assert!(
            cx.debug_bounds(selector).is_some(),
            "basic mode keeps actionable lyric feedback"
        );
    }

    #[gpui::test]
    fn the_basic_sheet_evaluates_note_counts_and_phrase_boundaries(cx: &mut gpui::TestAppContext) {
        for (original, later, status) in [
            ("さくら\nさいた", "ひかり\nとどく", Key::SongLyricsMatched),
            ("さくら", "はる", Key::SongLyricsNoteMismatch),
            (
                "さくら\nさいた",
                "はる\nがきたよ",
                Key::SongLyricsPhraseMismatch,
            ),
            ("", "はる", Key::SongLyricsOriginalEmpty),
            ("漢字", "はる", Key::SongLyricsMatchUnreadable),
        ] {
            let (app, cx) = crate::harness::open(cx);
            let index = app.update(cx, |this, _| {
                let mut spec = auris_session::prelude::preset("pop-band").unwrap().spec();
                spec.sections.get_mut("verse").unwrap().lyrics = original.into();
                spec.sections.get_mut("verse2").unwrap().lyrics = later.into();
                this.song_sheet = Some(super::super::song_dials(&spec));
                assert!(!this.song_advanced);
                this.song_sheet
                    .as_ref()
                    .unwrap()
                    .sections
                    .iter()
                    .position(|s| s.name == "verse2")
                    .unwrap()
            });
            crate::harness::paint(&app, cx);
            let selector: &'static str =
                Box::leak(format!("song-lyrics-status-{index}-{status:?}").into_boxed_str());
            assert!(
                cx.debug_bounds(selector).is_some(),
                "the matching assessment is visible: {status:?}"
            );
        }
    }

    #[gpui::test]
    fn lyrics_in_basic_mode_keep_the_preset_bars(cx: &mut gpui::TestAppContext) {
        for bars in [1, 8] {
            let (app, cx) = crate::harness::open(cx);
            crate::harness::resize(&app, cx, gpui::size(px(1500.0), px(1800.0)));
            let (index, before) = app.update(cx, |this, _| {
                let mut spec = auris_session::prelude::preset("pop-band").unwrap().spec();
                let section = spec.sections.get_mut("verse").unwrap();
                section.lyrics = "さくらさいた\nはるがきた".into();
                section.bars = bars;
                let dials = super::super::song_dials(&spec);
                let index = dials
                    .sections
                    .iter()
                    .position(|s| s.name == "verse")
                    .unwrap();
                this.song_sheet = Some(dials.clone());
                (index, dials)
            });
            crate::harness::paint(&app, cx);
            let estimate: &'static str =
                Box::leak(format!("song-lyrics-estimate-{index}").into_boxed_str());
            assert!(
                cx.debug_bounds(estimate).is_some(),
                "the estimate is shown even when it fits"
            );
            let fit: &'static str = Box::leak(format!("song-fit-lyrics-{index}").into_boxed_str());
            assert!(cx.debug_bounds(fit).is_none());
            app.read_with(cx, |this, _| {
                assert_eq!(this.song_sheet.as_ref().unwrap(), &before);
            });
        }
    }

    #[gpui::test]
    fn mismatched_later_words_leave_the_sheet_and_document_intact(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            let mut spec = auris_session::prelude::preset("pop-band").unwrap().spec();
            spec.sections.get_mut("verse").unwrap().lyrics = "さくら".into();
            spec.sections.get_mut("verse2").unwrap().lyrics = "はる".into();
            this.song_sheet = Some(super::super::song_dials(&spec));
            let before = this.project().clone();
            assert!(!this.write_song_from_sheet(true, cx));
            assert!(this.song_sheet.is_some());
            assert!(this.prompt.is_some());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn drums_have_a_separate_area_and_can_be_removed_and_added_without_changing_instruments(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{click, open, paint, resize};
        use gpui::{ScrollDelta, ScrollWheelEvent};
        let (app, cx) = open(cx);
        resize(&app, cx, gpui::size(gpui::px(1600.0), gpui::px(2000.0)));
        let band = app.update(cx, |this, _| {
            this.open_song_sheet();
            this.song_advanced = true;
            this.song_sheet
                .as_ref()
                .unwrap()
                .parts
                .iter()
                .filter(|part| !part.role.is_drum())
                .cloned()
                .collect::<Vec<_>>()
        });
        paint(&app, cx);
        let drums = cx.debug_bounds("song-sheet-drums").unwrap();
        let instruments = cx.debug_bounds("song-sheet-parts").unwrap();
        assert!(
            drums.bottom() < instruments.top(),
            "the kit occupies its own area above the instrument grid"
        );
        let reveal = |selector: &'static str, cx: &mut gpui::VisualTestContext| {
            for _ in 0..4 {
                let body = cx.debug_bounds("song-sheet-body").unwrap();
                let target = cx.debug_bounds(selector).unwrap();
                if target.top() >= body.top() && target.bottom() <= body.bottom() {
                    break;
                }
                let delta = if target.top() < body.top() {
                    body.top() + px(8.0) - target.top()
                } else {
                    body.bottom() - px(8.0) - target.bottom()
                };
                cx.simulate_event(ScrollWheelEvent {
                    position: body.center(),
                    delta: ScrollDelta::Pixels(gpui::point(px(0.0), delta)),
                    ..Default::default()
                });
                paint(&app, cx);
            }
            let body = cx.debug_bounds("song-sheet-body").unwrap();
            let target = cx.debug_bounds(selector).unwrap();
            assert!(
                target.top() >= body.top() && target.bottom() <= body.bottom(),
                "{selector} must be inside the scrolling body before clicking: target={target:?}, body={body:?}"
            );
        };
        reveal("song-remove-drums", cx);
        click("song-remove-drums", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_sheet.as_ref().unwrap().parts, band)
        });
        reveal("song-add-drums", cx);
        click("song-add-drums", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let parts = &this.song_sheet.as_ref().unwrap().parts;
            assert!(parts.iter().any(|part| part.role.is_drum()));
            assert_eq!(
                parts
                    .iter()
                    .filter(|part| !part.role.is_drum())
                    .cloned()
                    .collect::<Vec<_>>(),
                band
            );
        });
    }

    #[gpui::test]
    fn choosing_one_drum_source_keeps_other_kits_and_the_band(cx: &mut gpui::TestAppContext) {
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
            dials.parts.push(other.clone());
            let target = dials
                .parts
                .iter()
                .position(|part| part.role.is_drum())
                .unwrap();
            let source = PartSource::SoundFont {
                path: std::env::temp_dir().join("song-kit.sf2"),
                bank: 128,
                patch: 16,
            };
            this.open_song_library(target, cx);
            this.choose_song_library_source(source.clone());
            let dials = this.song_sheet.as_ref().unwrap();
            assert!(
                dials
                    .parts
                    .iter()
                    .filter(|p| p.role.is_drum() && p.name != "other-kit")
                    .all(|p| p.source == Some(source.clone()) && p.program.is_none())
            );
            assert_eq!(dials.parts.last(), Some(&other));
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
                2
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
