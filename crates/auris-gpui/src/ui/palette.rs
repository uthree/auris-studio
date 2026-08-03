//! The command palette: find any command by typing, and run it from the keyboard.
//!
//! It runs commands rather than reimplementing them. A row that names an action dispatches that
//! action, which is the same path the keystroke and the menu bar take — so the palette cannot
//! drift out of step with either, and a command added to [`BINDABLE`] appears here for free.
//!
//! The matching and the row arithmetic are free functions, so what a query finds and what the
//! arrow keys land on can be checked without a window.

use std::ops::Range;

use auris_i18n::Key;

use gpui::{Context, IntoElement, MouseButton, MouseDownEvent, Pixels, div, prelude::*, px};

use crate::actions::{BINDABLE, Bindable, menu_keystroke};
use crate::app::AurisApp;
use crate::theme::{Metrics, SCHEMES, Scheme, Theme};
use crate::ui::text_field::TextField;

/// What running one row of the palette does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PaletteCommand {
    /// One of the bindable actions, dispatched exactly as its keystroke would be.
    Action(&'static Bindable),
    /// Repaint the window in a colour scheme.
    Scheme(&'static Scheme),
}

/// One row of the palette, with everything it needs to be matched and drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    /// What choosing it does.
    pub command: PaletteCommand,
    /// Section the command belongs to, shown before its name.
    pub group: &'static str,
    /// The command's name, in the interface language.
    pub label: &'static str,
    /// The keystroke that also runs it, written the way this platform writes it.
    pub keystroke: Option<String>,
}

impl PaletteEntry {
    /// The text a query is matched against: the group and the name together.
    ///
    /// Together, so that `view lib` finds the library toggle even though nothing in its own name
    /// says "view". The group is the word people reach for first when they cannot remember what a
    /// command is called.
    pub fn haystack(&self) -> String {
        format!("{} {}", self.group, self.label)
    }
}

/// Every command the palette offers, in the language the window is drawn in.
///
/// The bindable actions first, in the order the settings window lists them, then the colour
/// schemes — which are not actions because each one carries a value, and an action per scheme
/// would be four more entries in a table that exists to be rebound.
pub fn entries(
    language: auris_i18n::Language,
    keystroke_of: impl Fn(&Bindable) -> String,
) -> Vec<PaletteEntry> {
    let mut rows: Vec<PaletteEntry> = BINDABLE
        .iter()
        .map(|command| PaletteEntry {
            command: PaletteCommand::Action(command),
            group: command.group.get(language),
            label: command.label.get(language),
            keystroke: Some(menu_keystroke(&keystroke_of(command))),
        })
        .collect();
    rows.extend(SCHEMES.iter().map(|scheme| PaletteEntry {
        command: PaletteCommand::Scheme(scheme),
        group: Key::AppearanceHeading.get(language),
        label: scheme.name,
        keystroke: None,
    }));
    rows
}

/// How well `query` matches `text`, or `None` when it does not match at all.
///
/// Subsequence matching, which is what every palette does and what makes one worth having: `nit`
/// finds "New Instrument Track" without anybody having to remember the whole name. A match at the
/// start of a word counts for far more than one in the middle of it, and a run of adjacent
/// characters more than the same characters scattered — so `save` puts Save above Add Audio Track,
/// which also contains an s, an a, a v and an e.
pub fn match_score(query: &str, text: &str) -> Option<i32> {
    let needle: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if needle.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = text.to_lowercase().chars().collect();

    let mut score = 0;
    let mut cursor = 0;
    let mut previous: Option<usize> = None;
    for wanted in needle {
        let found = cursor + haystack[cursor..].iter().position(|c| *c == wanted)?;
        score += 1;
        if found == 0 || !haystack[found - 1].is_alphanumeric() {
            score += 8;
        }
        if found > 0 && previous == Some(found - 1) {
            score += 5;
        }
        previous = Some(found);
        cursor = found + 1;
    }
    // A short name that matches beats a long one that matches just as well, so `Undo` is not
    // buried under everything else with a u, an n, a d and an o in it.
    Some(score - haystack.len() as i32 / 8)
}

/// The rows `query` finds, best first.
///
/// Ties keep the order the commands were listed in, which is the order the settings window shows
/// them: an empty query therefore opens on the whole list, arranged the way it is arranged
/// everywhere else, rather than on an arbitrary permutation of it.
pub fn matches(entries: &[PaletteEntry], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            match_score(query, &entry.haystack()).map(|score| (index, score))
        })
        .collect();
    // A stable sort, which is what keeps the tie-break above true.
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(index, _)| index).collect()
}

/// How many rows the palette shows at once.
pub const VISIBLE_ROWS: usize = 9;

/// Which slice of the matches is on screen, given where the selection is.
///
/// The list does not scroll — it is rebuilt every frame from this — so the window has to follow
/// the selection itself. It moves as little as possible: holding the arrow key down walks the
/// selection to the bottom edge and then scrolls one row at a time, rather than jumping a page
/// whenever the selection leaves the window.
pub fn visible_window(count: usize, selected: usize, rows: usize) -> Range<usize> {
    if count <= rows {
        return 0..count;
    }
    let half = rows / 2;
    let start = selected.saturating_sub(half).min(count - rows);
    start..start + rows
}

/// The open command palette.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// The query being typed.
    pub field: TextField,
    /// Which of the matching rows the arrow keys are on.
    pub selected: usize,
}

impl Palette {
    /// A palette with an empty query, opened on the first row.
    pub fn new() -> Self {
        Self {
            field: TextField::new(String::new()),
            selected: 0,
        }
    }
}

/// Width of the palette sheet.
const WIDTH: Pixels = px(520.0);
/// Height of one row.
const ROW_HEIGHT: Pixels = px(26.0);
/// Height of the query field.
const FIELD_HEIGHT: Pixels = px(30.0);

impl AurisApp {
    /// Opens the palette, closing anything else that owns the keyboard.
    pub(crate) fn open_palette(&mut self) {
        self.menu = None;
        self.menu_bar = None;
        self.prompt = None;
        self.palette = Some(Palette::new());
    }

    /// Closes the palette, and says whether one was open.
    pub(crate) fn close_palette(&mut self) -> bool {
        self.palette.take().is_some()
    }

    /// Every command the palette offers, with the keystrokes actually bound to them.
    pub(crate) fn palette_entries(&self) -> Vec<PaletteEntry> {
        entries(self.language(), |command| {
            self.keymap.keystroke(command).to_string()
        })
    }

    /// Runs one command and closes the palette.
    ///
    /// An action is dispatched rather than called: that is the same path the keystroke takes, so
    /// the palette cannot come to disagree with the key binding about what a command does. gpui
    /// defers the dispatch to the next tick, by which time the palette is already gone.
    pub(crate) fn run_palette_command(
        &mut self,
        command: PaletteCommand,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette();
        match command {
            PaletteCommand::Action(action) => window.dispatch_action(action.action(), cx),
            PaletteCommand::Scheme(scheme) => self.apply_scheme(scheme.id),
        }
    }

    /// Handles a keystroke aimed at the open palette.
    ///
    /// Returns `true` when the key was used. Only the keys the platform does not deliver as text,
    /// exactly as the rename sheet does — everything else arrives through the input handler, which
    /// is what keeps an IME working here too.
    pub(crate) fn palette_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(palette) = self.palette.as_ref() else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        // While the IME is composing, these four belong to the candidate window: enter accepts
        // the composition and the arrows walk the candidates.
        if palette.field.marked().is_none() {
            match key {
                "escape" => {
                    self.close_palette();
                    return true;
                }
                "enter" => {
                    // A query nothing answers to leaves the palette open: the fix is to type
                    // something else, and closing would mean typing the whole query again.
                    if let Some(command) = self.palette_choice() {
                        self.run_palette_command(command, window, cx);
                    }
                    return true;
                }
                "up" | "down" => {
                    let count = self.palette_matches().len();
                    let delta = if key == "down" { 1 } else { -1 };
                    if let Some(palette) = self.palette.as_mut() {
                        palette.selected = next_row(palette.selected, delta, count);
                    }
                    return true;
                }
                _ => {}
            }
        }

        let shift = event.keystroke.modifiers.shift;
        let secondary = event.keystroke.modifiers.secondary();
        let Some(palette) = self.palette.as_mut() else {
            return false;
        };
        match key {
            // Anything that narrows the list starts again at the top, or the highlight would sit
            // on whatever row happened to inherit its position.
            "backspace" => {
                palette.field.backspace();
                palette.selected = 0;
            }
            "delete" => {
                palette.field.delete_forward();
                palette.selected = 0;
            }
            "left" => palette.field.move_left(shift),
            "right" => palette.field.move_right(shift),
            "home" => palette.field.move_home(shift),
            "end" => palette.field.move_end(shift),
            "a" if secondary => palette.field.select_all(),
            _ => return false,
        }
        true
    }

    /// The rows the query finds, best first.
    fn palette_matches(&self) -> Vec<usize> {
        let Some(palette) = self.palette.as_ref() else {
            return Vec::new();
        };
        matches(&self.palette_entries(), palette.field.content())
    }

    /// The command the selected row would run.
    fn palette_choice(&self) -> Option<PaletteCommand> {
        let palette = self.palette.as_ref()?;
        let entries = self.palette_entries();
        matches(&entries, palette.field.content())
            .get(palette.selected)
            .map(|index| entries[*index].command)
    }

    /// Draws the palette over everything else.
    pub(crate) fn render_palette(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let palette = self.palette.as_ref()?;
        let theme = self.theme.clone();
        let entries = self.palette_entries();
        let found = matches(&entries, palette.field.content());
        let selected = palette.selected.min(found.len().saturating_sub(1));
        let window = visible_window(found.len(), selected, VISIBLE_ROWS);

        let rows: Vec<gpui::AnyElement> = found[window.clone()]
            .iter()
            .enumerate()
            .map(|(offset, index)| {
                let entry = &entries[*index];
                let command = entry.command;
                let lit = window.start + offset == selected;
                div()
                    .id(("palette-row", *index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(ROW_HEIGHT)
                    .px_2()
                    .rounded(Metrics::RADIUS_SM)
                    .when(lit, |this| this.bg(Theme::translucent(theme.accent, 0.28)))
                    .cursor_pointer()
                    .hover(|this| this.bg(Theme::translucent(theme.accent, 0.16)))
                    .child(
                        div()
                            .w(px(96.0))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.text_faint)
                            .truncate()
                            .child(entry.group),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(theme.text)
                            .truncate()
                            .child(entry.label),
                    )
                    .children(entry.keystroke.clone().map(|keystroke| {
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(keystroke)
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.run_palette_command(command, window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(90.0))
                .bg(Theme::translucent(theme.background, 0.55))
                // A dimmed screen that still takes clicks is decoration, not a modal. This makes
                // it one: nothing behind it is hit by any button, or by the wheel.
                .occlude()
                // A click outside closes it, the way every palette does.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_palette();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .w(WIDTH)
                        .p_2()
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
                                .h(FIELD_HEIGHT)
                                .w_full()
                                .rounded(Metrics::RADIUS_SM)
                                .bg(theme.surface_sunken)
                                .border_1()
                                .border_color(theme.accent)
                                .child(crate::ui::prompt::editable_text(
                                    palette.field.content().to_string().into(),
                                    palette.field.selection(),
                                    palette.field.marked(),
                                    self.focus.clone(),
                                    cx.entity(),
                                    theme.clone(),
                                )),
                        )
                        .children(if rows.is_empty() {
                            Some(
                                div()
                                    .flex()
                                    .items_center()
                                    .h(ROW_HEIGHT)
                                    .px_2()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(self.t(Key::PaletteNothingMatches))
                                    .into_any_element(),
                            )
                        } else {
                            None
                        })
                        .children(rows),
                ),
        )
    }
}

/// The row `delta` away from `from`, wrapping at both ends.
///
/// Wrapping because the list is short and the alternative is a dead key at each end: pressing up
/// on the first row should reach the last one, which is how every palette and every menu behaves.
pub fn next_row(from: usize, delta: isize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let count = count as isize;
    (from as isize + delta).rem_euclid(count) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_letters_find_a_command_by_its_initials() {
        // The point of a palette: `nit` should reach New Instrument Track without anybody
        // remembering the whole name of it.
        let scored = match_score("nit", "Track New Instrument Track");
        assert!(scored.is_some());
        assert!(match_score("zzz", "New Instrument Track").is_none());
        // An empty query matches everything, which is what opens the palette on the whole list.
        assert_eq!(match_score("", "anything"), Some(0));
        // Case is ignored in both directions.
        assert_eq!(
            match_score("SAVE", "File Save"),
            match_score("save", "File Save")
        );
    }

    #[test]
    fn the_command_you_meant_comes_first() {
        let save = match_score("save", "File Save").expect("Save matches");
        let scattered = match_score("save", "Track Add Audio Track").unwrap_or(i32::MIN);
        assert!(
            save > scattered,
            "Save scored {save} against {scattered} for a name that merely contains the letters"
        );

        // A run of adjacent characters beats the same characters spread out.
        let run = match_score("undo", "Edit Undo").expect("Undo matches");
        let spread = match_score("undo", "Edit Under the Door").unwrap_or(i32::MIN);
        assert!(run > spread, "{run} vs {spread}");
    }

    #[test]
    fn the_whole_list_is_offered_before_anything_is_typed() {
        let all = entries(auris_i18n::Language::English, |command| {
            command.default.to_string()
        });
        assert_eq!(matches(&all, "").len(), all.len());
        assert!(
            all.len() > BINDABLE.len(),
            "the colour schemes belong in the palette too"
        );
        // Every row has something to show, or the palette would have blank lines in it.
        for entry in &all {
            assert!(!entry.label.is_empty() && !entry.group.is_empty());
        }
        // And an untyped palette lists them in the order everything else does.
        assert_eq!(matches(&all, "")[..3], [0, 1, 2]);
    }

    #[test]
    fn a_query_narrows_the_list_and_keeps_what_it_should() {
        let all = entries(auris_i18n::Language::English, |command| {
            command.default.to_string()
        });
        let found = matches(&all, "track");
        assert!(!found.is_empty());
        let first = &all[found[0]];
        assert!(
            first.haystack().to_lowercase().contains("track"),
            "`track` put {} first",
            first.label
        );
        assert!(matches(&all, "qqqqqq").is_empty());
    }

    #[test]
    fn the_arrow_keys_wrap_at_both_ends() {
        assert_eq!(next_row(0, 1, 5), 1);
        assert_eq!(next_row(4, 1, 5), 0, "off the bottom is back to the top");
        assert_eq!(next_row(0, -1, 5), 4, "and off the top is the bottom");
        // A query nothing matches leaves nothing to select, which must not divide by zero.
        assert_eq!(next_row(3, 1, 0), 0);
    }

    #[test]
    fn the_visible_rows_follow_the_selection_a_row_at_a_time() {
        // A short list is shown whole.
        assert_eq!(visible_window(4, 0, 9), 0..4);
        assert_eq!(visible_window(9, 8, 9), 0..9);

        // A long one keeps the selection in view, and never runs off either end.
        let window = visible_window(40, 0, 9);
        assert_eq!(window, 0..9);
        assert!(visible_window(40, 20, 9).contains(&20));
        assert_eq!(visible_window(40, 39, 9), 31..40);
        // Walking down one row at a time moves the window by at most one row.
        let mut previous = visible_window(40, 0, 9);
        for selected in 1..40 {
            let window = visible_window(40, selected, 9);
            assert!(
                window.contains(&selected),
                "row {selected} scrolled out of view"
            );
            assert!(window.start - previous.start <= 1, "the list jumped a page");
            previous = window;
        }
    }
}
