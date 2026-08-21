//! The menu bar the window draws for itself.
//!
//! macOS owns the menu bar and gpui hands the menu straight to it. Windows and Linux have no
//! system menu bar at all — gpui's `set_menus` stores the menu and never shows it — so without
//! this the File, Edit, Track and Transport menus simply would not exist off a Mac, and every
//! command in them would be reachable only by knowing its keystroke.
//!
//! The rows come from [`crate::menu::model`], the same table the system menu bar is built from.

use gpui::{
    Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Window, deferred,
    div, prelude::*, px,
};

use crate::app::AurisApp;
use crate::menu::{MenuRow, MenuSection, model};
use crate::theme::Metrics;

/// Height of the bar.
pub const HEIGHT: Pixels = px(26.0);

/// Height of one row in an open menu.
const ROW_HEIGHT: Pixels = px(22.0);

/// Width of an open menu.
///
/// Fixed rather than measured: every label here is short, and a menu that changes width between
/// languages looks like a bug. Long labels truncate.
const MENU_WIDTH: Pixels = px(260.0);

/// Which menu-bar menu is open, and how it came to be.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpenMenu {
    /// Index into [`crate::menu::model`].
    pub index: usize,
    /// `true` when a click opened it, `false` when the pointer slid onto it with another
    /// already open.
    pub clicked: bool,
    /// The row the keyboard is on, once the keyboard has been used.
    ///
    /// `None` until then, so a menu dropped open with the mouse does not draw a highlight nobody
    /// asked for — and so the first Down lands on the first row rather than the second.
    pub highlighted: Option<usize>,
}

impl OpenMenu {
    /// The menu at `index`, opened by a click, with nothing highlighted yet.
    pub fn at(index: usize) -> Self {
        Self {
            index,
            clicked: true,
            highlighted: None,
        }
    }
}

/// What the bar shows after a click on the title at `index`.
///
/// A click closes only the menu a click opened. One the pointer merely slid onto opens
/// properly instead — otherwise clicking a menu you are already hovering toggles shut the one
/// the slide had just opened, and the click looks like it was swallowed.
pub fn after_click(open: Option<OpenMenu>, index: usize) -> Option<OpenMenu> {
    match open {
        Some(open) if open.index == index && open.clicked => None,
        // The highlight is not carried across. It belongs to the row the keyboard was on, and
        // the pointer has just said it is somewhere else entirely.
        _ => Some(OpenMenu::at(index)),
    }
}

/// What the bar shows after the pointer moves onto the title at `index`.
///
/// Sliding along the bar moves between menus without clicking again. Doing nothing while none
/// is open is the other half of that rule: a menu must not open because the pointer passed over.
pub fn after_hover(open: Option<OpenMenu>, index: usize) -> Option<OpenMenu> {
    match open {
        Some(open) if open.index != index => Some(OpenMenu {
            index,
            clicked: false,
            highlighted: None,
        }),
        other => other,
    }
}

/// The menu `delta` titles along from `index`, wrapping round the ends of the bar.
pub fn stepped_section(count: usize, index: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (index as isize + delta).rem_euclid(count as isize) as usize
}

/// Where the highlight lands after moving `delta` rows through `rows`.
///
/// Wraps, and starts from whichever end the direction implies, so the first Down lands on the
/// first row and the first Up on the last. Rules are stepped over, and so is the submenu the
/// system fills in — Return on either runs nothing, and stopping on a row that does nothing is a
/// keypress the user has to make twice. A menu with nothing to choose highlights nothing.
pub fn stepped(rows: &[MenuRow], from: Option<usize>, delta: isize) -> Option<usize> {
    let choosable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, MenuRow::Command { .. }))
        .map(|(index, _)| index)
        .collect();
    let first = *choosable.first()?;
    let last = *choosable.last()?;
    let Some(current) = from else {
        return Some(if delta >= 0 { first } else { last });
    };
    let at = choosable
        .iter()
        .position(|index| *index == current)
        .unwrap_or(0) as isize;
    let count = choosable.len() as isize;
    Some(choosable[(at + delta).rem_euclid(count) as usize])
}

impl AurisApp {
    /// Whether this platform needs the window to draw its own menu bar.
    ///
    /// A runtime `cfg!` rather than `#[cfg]` so the whole thing is compiled — and its tests run
    /// — on macOS too. A menu bar that only exists on the platform nobody develops on is a menu
    /// bar that is broken on the platform nobody develops on.
    pub(crate) fn wants_menu_bar() -> bool {
        !cfg!(target_os = "macos")
    }

    /// Closes any open menu-bar dropdown, reporting whether there was one.
    pub(crate) fn close_menu_bar(&mut self) -> bool {
        self.menu_bar.take().is_some()
    }

    /// Draws the bar, on the platforms that need one.
    pub(crate) fn render_menu_bar(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !Self::wants_menu_bar() {
            return None;
        }
        let theme = self.theme.clone();
        let open = self.menu_bar;
        let sections = model(self.language);

        Some(
            div()
                .flex()
                .items_center()
                .h(HEIGHT)
                .flex_shrink_0()
                .px_1()
                .gap_px()
                .bg(theme.surface_raised)
                .border_b_1()
                .border_color(theme.border)
                .children(sections.into_iter().enumerate().map(|(index, section)| {
                    let is_open = open.is_some_and(|open| open.index == index);
                    div()
                        // `relative` so the dropdown hangs off this title rather than off the
                        // window, which is what keeps it under the word that opened it.
                        .relative()
                        .id(("menu-title", index))
                        .flex()
                        .items_center()
                        .h(px(20.0))
                        .px_2()
                        .rounded(Metrics::RADIUS_SM)
                        .text_xs()
                        .cursor_pointer()
                        .when(is_open, |this| {
                            this.bg(theme.accent).text_color(theme.text_on_accent)
                        })
                        .when(!is_open, |this| {
                            this.text_color(theme.text)
                                .hover(|this| this.bg(theme.surface_hover))
                        })
                        .child(section.name.clone())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.menu_bar = after_click(this.menu_bar, index);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            let next = after_hover(this.menu_bar, index);
                            if next != this.menu_bar {
                                this.menu_bar = next;
                                cx.notify();
                            }
                        }))
                        .children(is_open.then(|| {
                            let highlighted = open.and_then(|open| open.highlighted);
                            self.render_menu_bar_dropdown(index, &section, highlighted, cx)
                        }))
                }))
                .into_any_element(),
        )
    }

    /// One open menu, hanging below its title.
    fn render_menu_bar_dropdown(
        &self,
        section_index: usize,
        section: &MenuSection,
        highlighted: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();

        let rows: Vec<gpui::AnyElement> = section
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| match row {
                MenuRow::Separator => div()
                    .my(px(3.0))
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    .bg(theme.border)
                    .into_any_element(),
                // A submenu the system fills in has nothing behind it here, so it is not drawn.
                // `model` only produces one on macOS, where this bar does not run at all.
                MenuRow::System { .. } => div().into_any_element(),
                MenuRow::Command {
                    label,
                    action,
                    binding,
                } => {
                    let action = action.boxed_clone();
                    let keystroke = self.keystroke_for(binding);
                    div()
                        .id(("menu-bar-item", section_index * 100 + index))
                        .flex()
                        .items_center()
                        .gap_4()
                        .flex_shrink_0()
                        .h(ROW_HEIGHT)
                        .px_2()
                        .rounded(Metrics::RADIUS_SM)
                        .text_xs()
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.accent).text_color(theme.text_on_accent))
                        // The same fill the pointer gives it, because it means the same thing:
                        // this is the row Return would run.
                        .when(highlighted == Some(index), |this| {
                            this.bg(theme.accent).text_color(theme.text_on_accent)
                        })
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .child(
                            div()
                                .flex_shrink_0()
                                // Not `text_faint`: the hover state fills the row with the
                                // accent, and the dimmest text would disappear into it.
                                .text_color(theme.text_muted)
                                .child(keystroke),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.close_menu_bar();
                                // Dispatched rather than called: the root view already handles
                                // every one of these actions for the keymap and the system menu
                                // bar, and routing through it keeps the three in step.
                                window.dispatch_action(action.boxed_clone(), cx);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                }
            })
            .collect();

        // Deferred so it paints above the panels below it. Without this the menu would open
        // *behind* the transport bar, because gpui paints siblings in the order they are given
        // and the bar is the first child of the window.
        deferred(
            div()
                .absolute()
                .top(HEIGHT - px(3.0))
                .left(px(0.0))
                .w(MENU_WIDTH)
                .flex()
                .flex_col()
                .p_1()
                .rounded(Metrics::RADIUS_MD)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .text_color(theme.text)
                // A click inside the menu must not reach the title behind it, which would
                // toggle the menu shut before the row could act.
                .on_mouse_down(
                    MouseButton::Left,
                    |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                )
                .children(rows),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_platforms_without_a_system_menu_bar_draw_one() {
        assert_eq!(AurisApp::wants_menu_bar(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn the_bar_is_tall_enough_for_its_rows() {
        // The dropdown hangs from `HEIGHT`; a bar shorter than the text in it would clip.
        assert!(HEIGHT >= px(20.0));
        assert!(ROW_HEIGHT < HEIGHT);
    }

    fn clicked(index: usize) -> Option<OpenMenu> {
        Some(OpenMenu::at(index))
    }

    fn slid(index: usize) -> Option<OpenMenu> {
        Some(OpenMenu {
            index,
            clicked: false,
            highlighted: None,
        })
    }

    /// The rows of the first menu that has a rule in it, which is the case worth stepping over.
    fn rows_with_a_rule() -> Vec<MenuRow> {
        model(auris_i18n::Language::English)
            .into_iter()
            .map(|section| section.rows)
            .find(|rows| rows.iter().any(|row| matches!(row, MenuRow::Separator)))
            .expect("the menus have rules in them")
    }

    #[test]
    fn the_first_arrow_lands_on_the_end_it_came_from() {
        let rows = rows_with_a_rule();
        let commands: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, MenuRow::Command { .. }))
            .map(|(index, _)| index)
            .collect();

        assert_eq!(
            stepped(&rows, None, 1),
            Some(commands[0]),
            "Down opens at the top"
        );
        assert_eq!(
            stepped(&rows, None, -1),
            commands.last().copied(),
            "and Up at the bottom"
        );
    }

    #[test]
    fn the_highlight_steps_over_the_rules_and_wraps() {
        let rows = rows_with_a_rule();
        let commands: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, MenuRow::Command { .. }))
            .map(|(index, _)| index)
            .collect();

        // Walking the whole menu one row at a time visits every command and nothing else — a
        // separator that held the highlight would be a Return that ran nothing.
        let mut at = stepped(&rows, None, 1);
        let mut visited = vec![at.unwrap()];
        for _ in 1..commands.len() {
            at = stepped(&rows, at, 1);
            visited.push(at.unwrap());
        }
        assert_eq!(visited, commands);
        assert_eq!(
            stepped(&rows, at, 1),
            Some(commands[0]),
            "and one more comes back round to the top"
        );
    }

    #[test]
    fn a_menu_with_nothing_to_choose_highlights_nothing() {
        assert_eq!(stepped(&[], None, 1), None);
        assert_eq!(stepped(&[MenuRow::Separator], None, 1), None);
    }

    #[test]
    fn the_titles_wrap_in_both_directions() {
        assert_eq!(stepped_section(4, 0, -1), 3);
        assert_eq!(stepped_section(4, 3, 1), 0);
        assert_eq!(stepped_section(4, 1, 1), 2);
        // A bar with no menus in it is not a division by zero.
        assert_eq!(stepped_section(0, 0, 1), 0);
    }

    #[test]
    fn the_pointer_takes_the_highlight_off_the_row_the_keyboard_had() {
        // The two are the same highlight, and the pointer has just said it is somewhere else.
        let walked = Some(OpenMenu {
            index: 1,
            clicked: true,
            highlighted: Some(4),
        });
        assert_eq!(after_click(walked, 2).unwrap().highlighted, None);
        assert_eq!(after_hover(walked, 2).unwrap().highlighted, None);
    }

    #[test]
    fn a_click_opens_a_closed_menu_and_closes_the_one_it_opened() {
        assert_eq!(after_click(None, 1), clicked(1));
        assert_eq!(after_click(clicked(1), 1), None);
    }

    #[test]
    fn a_click_on_a_menu_the_pointer_slid_onto_opens_it_rather_than_toggling() {
        // Slide from File onto Transport and Transport opens; the click that follows must keep
        // it open. Toggling here reads as a click the application ignored, and the user clicks
        // again to get back exactly where they were.
        let after_slide = after_hover(clicked(0), 3);
        assert_eq!(after_slide, slid(3));
        assert_eq!(after_click(after_slide, 3), clicked(3));
        // And now that a click put it there, clicking again does close it.
        assert_eq!(after_click(clicked(3), 3), None);
    }

    #[test]
    fn a_click_on_another_title_switches_rather_than_closes() {
        assert_eq!(after_click(clicked(0), 2), clicked(2));
    }

    #[test]
    fn the_pointer_alone_never_opens_a_menu() {
        // Sliding along a bar with nothing open must stay closed, or the menus would drop down
        // whenever the pointer crossed the top of the window on its way somewhere else.
        assert_eq!(after_hover(None, 0), None);
        assert_eq!(after_hover(None, 3), None);
        // Nor does the pointer resting on the open menu's own title change how it was opened.
        assert_eq!(after_hover(clicked(1), 1), clicked(1));
        assert_eq!(after_hover(slid(1), 1), slid(1));
    }
}
