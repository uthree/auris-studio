//! The on-demand application menu for platforms without a system menu bar.
//!
//! macOS owns the menu bar and gpui hands the menu straight to it. Windows and Linux have no
//! system menu bar at all — gpui's `set_menus` stores the menu and never shows it — so without
//! this the File, Edit, Track and Transport menus simply would not exist off a Mac, and every
//! command in them would be reachable only by knowing its keystroke.
//!
//! The rows come from [`crate::menu::model`], the same table the system menu bar is built from.

use gpui::{
    Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Window, deferred,
    div, point, prelude::*, px,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use crate::app::AurisApp;
use crate::menu::{MenuRow, MenuSection, model};
use crate::theme::Metrics;

/// Last painted title-button bounds, used to anchor its menu without guessing text widths.
pub(crate) type MenuTitleBounds =
    std::rc::Rc<std::cell::RefCell<std::collections::HashMap<usize, gpui::Bounds<Pixels>>>>;

/// Height of one row in an open menu.
const ROW_HEIGHT: Pixels = px(22.0);

/// Total vertical space occupied by one separator, including its margins.
const SEPARATOR_HEIGHT: Pixels = px(7.0);

/// Padding and border above and below the scrolling rows.
const MENU_FRAME_HEIGHT: Pixels = px(10.0);

/// The narrowest an open menu is drawn.
///
/// A floor rather than a width. It was fixed, on the reasoning that every label is short, and the
/// Japanese File menu's longer rows were cut off against their keystrokes.
const MENU_WIDTH: Pixels = px(260.0);

/// The widest a dropdown grows to fit its longest row; past this a label truncates.
const MENU_MAX_WIDTH: Pixels = px(440.0);

/// The mark beside a switched-on row.
///
/// A character rather than a drawn glyph, because it is the one mark in the application that
/// every font on both platforms has and that every menu everywhere already uses.
const CHECK: &str = "✓";

/// Width of the column the tick sits in, reserved on every row so the labels line up.
const CHECK_WIDTH: Pixels = px(10.0);

/// Which menu-bar menu is open, and how it came to be.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpenMenu {
    /// Opened from a pinned title button rather than the full application menu.
    pub from_titlebar: bool,
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
            from_titlebar: false,
            index,
            clicked: true,
            highlighted: None,
        }
    }
}

/// What the bar shows after the pointer moves onto the title at `index`.
///
/// Sliding along the bar moves between menus without clicking again. Doing nothing while none
/// is open is the other half of that rule: a menu must not open because the pointer passed over.
pub fn after_hover(open: Option<OpenMenu>, index: usize) -> Option<OpenMenu> {
    match open {
        Some(open) if open.index != index => Some(OpenMenu {
            from_titlebar: open.from_titlebar,
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
/// keypress the user has to make twice. A disabled command is that same case and is stepped over
/// for that same reason. A menu with nothing to choose highlights nothing.
pub fn stepped(rows: &[MenuRow], from: Option<usize>, delta: isize) -> Option<usize> {
    let choosable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, MenuRow::Command { enabled: true, .. }))
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
        .map_or(-1, |position| position as isize);
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
        let was_open = self.menu_bar.is_some();
        self.set_menu_bar(None);
        was_open
    }

    /// Replaces the open dropdown and starts a newly opened section at its first row.
    pub(crate) fn set_menu_bar(&mut self, next: Option<OpenMenu>) {
        if self.menu_bar.map(|open| open.index) != next.map(|open| open.index) {
            self.menu_bar_scroll.set_offset(point(px(0.0), px(0.0)));
        }
        self.menu_bar = next;
    }

    /// Keeps pointer hover and keyboard activation aimed at the same dropdown row.
    fn set_menu_bar_highlight(
        &mut self,
        section: usize,
        highlighted: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.menu_bar else {
            return;
        };
        if open.index == section && open.highlighted != highlighted {
            open.highlighted = highlighted;
            cx.notify();
        }
    }

    /// The menu as it stands right now: this language, these panels, these switches.
    ///
    /// Rebuilt on every frame the bar is drawn, and on every keystroke that walks it, so that the
    /// ticks and the dimming are never one gesture out of date. It is a few dozen strings and no
    /// allocation the frame was not going to make anyway.
    pub(crate) fn menu_model(&self) -> Vec<MenuSection> {
        model(
            self.language(),
            &self.panels,
            crate::menu::MenuState {
                can_undo: self.session.can_undo(),
                can_redo: self.session.can_redo(),
                looping: self.project().loop_enabled,
                punching: self.project().punch_enabled,
                recording: self.session.is_recording(),
                monitoring: self.session.monitoring(),
                metronome: self.session.metronome(),
                musical_typing: self.session.typing_keyboard().enabled(),
            },
        )
    }

    /// Draws a menu overlay with categories beside the existing command rows.
    pub(crate) fn render_menu_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !Self::wants_menu_bar() {
            return None;
        }
        let theme = self.theme.clone();
        let open = self.menu_bar.filter(|_| {
            self.event_window
                .is_none_or(|id| id == window.window_handle().window_id())
        });
        let open = open?;
        let sections = self.menu_model();
        let section = sections.get(open.index)?;
        let viewport_height = window.viewport_size().height;
        let commands = self.render_menu_bar_dropdown(
            open.index,
            section,
            open.highlighted,
            viewport_height,
            cx,
        );

        if open.from_titlebar
            && let Some(bounds) = self.menu_title_bounds.borrow().get(&open.index).copied()
        {
            return Some(
                deferred(
                    gpui::anchored()
                        .anchor(gpui::Corner::TopLeft)
                        .position(gpui::point(bounds.left(), crate::titlebar::HEIGHT))
                        .snap_to_window_with_margin(theme.atelier.spacing.sm)
                        .child(
                            div()
                                .debug_selector(|| "titlebar-menu-panel".into())
                                .track_focus(&self.menu_focus)
                                .rounded(Metrics::RADIUS_MD)
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surface_raised)
                                .shadow_lg()
                                .occlude()
                                .child(commands),
                        ),
                )
                .into_any_element(),
            );
        }

        Some(
            deferred(
                div()
                    .debug_selector(|| "application-menu-panel".into())
                    .track_focus(&self.menu_focus)
                    .absolute()
                    .top(crate::titlebar::HEIGHT)
                    .left(theme.atelier.spacing.md)
                    .flex()
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Metrics::RADIUS_MD)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex().flex_col().w_32().p_1().gap_1().children(
                        sections.into_iter().enumerate().map(|(index, section)| {
                            let is_open = open.index == index;
                            div()
                                .on_mouse_move(cx.listener(
                                    move |this, _: &MouseMoveEvent, _, cx| {
                                        let next = after_hover(this.menu_bar, index).map(|menu| {
                                            OpenMenu {
                                                from_titlebar: false,
                                                ..menu
                                            }
                                        });
                                        if next != this.menu_bar {
                                            this.set_menu_bar(next);
                                            cx.notify();
                                        }
                                    },
                                ))
                                .child(
                                    atelier::Button::new(("menu-title", index), section.name)
                                        .ghost()
                                        .size(atelier::ControlSize::Small)
                                        .full_width()
                                        .selected(is_open)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_menu_bar(Some(OpenMenu::at(index)));
                                            window.focus(&this.menu_focus);
                                            cx.notify();
                                        })),
                                )
                        }),
                    ))
                    .child(commands),
            )
            .into_any_element(),
        )
    }

    /// The command list beside the selected application-menu category.
    fn render_menu_bar_dropdown(
        &self,
        section_index: usize,
        section: &MenuSection,
        highlighted: Option<usize>,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();

        let rows: Vec<gpui::AnyElement> = section
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| match row {
                MenuRow::Separator => div()
                    .debug_selector(move || format!("menu-bar-separator-{section_index}-{index}"))
                    .my(px(3.0))
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    .bg(theme.border)
                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                        this.set_menu_bar_highlight(section_index, None, cx);
                    }))
                    .into_any_element(),
                // A submenu the system fills in has nothing behind it here, so it is not drawn.
                // `model` only produces one on macOS, where this bar does not run at all.
                MenuRow::System { .. } => div().into_any_element(),
                MenuRow::Command {
                    label,
                    action,
                    binding,
                    enabled,
                    checked,
                } => {
                    let (enabled, checked) = (*enabled, *checked);
                    let action = action.boxed_clone();
                    let keystroke = self.keystroke_for(binding);
                    div()
                        .id(("menu-bar-item", section_index * 100 + index))
                        .debug_selector(move || format!("menu-bar-item-{section_index}-{index}"))
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .h(ROW_HEIGHT)
                        .px_2()
                        .rounded(Metrics::RADIUS_SM)
                        .text_xs()
                        // A row that can do nothing says so by receding. It keeps its keystroke,
                        // because the key is as unavailable as the row and hiding that would only
                        // make the row look like a different command.
                        .text_color(match enabled {
                            true => theme.text,
                            false => theme.text_faint,
                        })
                        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            this.set_menu_bar_highlight(
                                section_index,
                                enabled.then_some(index),
                                cx,
                            );
                        }))
                        // Neither the pointer's fill nor the keyboard's lands on a dead row.
                        .when(enabled, |this| {
                            this.hover(|this| {
                                this.bg(theme.accent).text_color(theme.text_on_accent)
                            })
                            // The same fill the pointer gives it, because it means the same
                            // thing: this is the row Return would run.
                            .when(highlighted == Some(index), |this| {
                                this.bg(theme.accent).text_color(theme.text_on_accent)
                            })
                        })
                        // A fixed column whether this row ticks or not, so every label in the
                        // menu starts in the same place. A tick that pushed its own row along
                        // would make the menu twitch as things were switched on and off.
                        .child(div().w(CHECK_WIDTH).flex_shrink_0().child(if checked {
                            CHECK
                        } else {
                            ""
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .child(
                            div()
                                .flex_shrink_0()
                                // Not `text_faint`: the hover state fills the row with the
                                // accent, and the dimmest text would disappear into it.
                                .text_color(theme.text_muted)
                                .child(keystroke),
                        )
                        // No handler at all when it is disabled. The dropdown behind it already
                        // swallows the press, so the menu stays open and nothing happens — which
                        // is what every menu does with a dimmed row.
                        .when(enabled, |this| {
                            this.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.close_menu_bar();
                                    // Dispatched rather than called: the root view already handles
                                    // every one of these actions for the keymap and the system
                                    // menu bar, and routing through it keeps the three in step.
                                    window.dispatch_action(action.boxed_clone(), cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                        })
                        .into_any_element()
                }
            })
            .collect();

        let natural_height = section
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Command { .. } => ROW_HEIGHT,
                MenuRow::Separator => SEPARATOR_HEIGHT,
                MenuRow::System { .. } => px(0.0),
            })
            .fold(MENU_FRAME_HEIGHT, |height, row| height + row);
        let available_height =
            (viewport_height - crate::titlebar::HEIGHT - self.theme.atelier.spacing.md)
                .max(ROW_HEIGHT + MENU_FRAME_HEIGHT);
        let height = natural_height.min(available_height);
        let scroll = self.menu_bar_scroll.clone();
        let needs_scroll = natural_height > available_height;

        div()
            // At least the usual width, and as wide as the longest label beside its keystroke
            // beyond that: a fixed width cut a translated label short against its shortcut.
            .min_w(MENU_WIDTH)
            .max_w(MENU_MAX_WIDTH)
            .flex_none()
            .h(height)
            .flex()
            .flex_col()
            .p_1()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_raised)
            .border_l_1()
            .border_color(theme.border)
            .text_color(theme.text)
            // A click inside the menu must not reach the title behind it, which would
            // toggle the menu shut before the row could act.
            .on_mouse_down(
                MouseButton::Left,
                |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("menu-bar-scroll")
                            .debug_selector(|| "menu-bar-scroll".to_string())
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .when(needs_scroll, |this| this.pr_3())
                            .children(rows),
                    )
                    .when(needs_scroll, |this| {
                        this.child(
                            Scrollbar::vertical(&scroll).scrollbar_show(ScrollbarShow::Always),
                        )
                    }),
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

    fn clicked(index: usize) -> Option<OpenMenu> {
        Some(OpenMenu::at(index))
    }

    fn slid(index: usize) -> Option<OpenMenu> {
        Some(OpenMenu {
            from_titlebar: false,
            index,
            clicked: false,
            highlighted: None,
        })
    }

    /// The rows of the first menu that has a rule in it, which is the case worth stepping over.
    fn rows_with_a_rule() -> Vec<MenuRow> {
        model(
            auris_i18n::Language::English,
            &crate::dock::PanelLayout::default(),
            crate::menu::MenuState::default(),
        )
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
    fn a_stale_highlight_steps_from_before_the_first_choosable_row() {
        let rows = rows_with_a_rule();
        let first = rows
            .iter()
            .position(|row| matches!(row, MenuRow::Command { enabled: true, .. }))
            .expect("a choosable row");
        assert_eq!(stepped(&rows, Some(usize::MAX), 1), Some(first));
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
    fn the_highlight_steps_over_a_row_that_cannot_be_chosen() {
        // The Edit menu opens with Undo and Redo, and both are dim in a document nobody has
        // touched. Landing on one is a Return that runs nothing, which is the same keypress
        // wasted that a separator would waste — so the walk starts below them.
        let edit = model(
            auris_i18n::Language::English,
            &crate::dock::PanelLayout::default(),
            crate::menu::MenuState::default(),
        )
        .into_iter()
        .find(|section| {
            section.name == auris_i18n::Key::GroupEdit.get(auris_i18n::Language::English)
        })
        .expect("there is an Edit menu");

        let dim: Vec<usize> = edit
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, MenuRow::Command { enabled: false, .. }))
            .map(|(index, _)| index)
            .collect();
        assert!(!dim.is_empty(), "nothing in Edit is dim to step over");

        // Every stop of a full walk, from either end, is a row that can actually be chosen.
        for delta in [1, -1] {
            let mut at = stepped(&edit.rows, None, delta);
            for _ in 0..edit.rows.len() {
                let index = at.expect("the walk never runs out");
                assert!(!dim.contains(&index), "the highlight landed on a dim row");
                at = stepped(&edit.rows, at, delta);
            }
        }
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
            from_titlebar: false,
            index: 1,
            clicked: true,
            highlighted: Some(4),
        });
        assert_eq!(after_hover(walked, 2).unwrap().highlighted, None);
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

/// The in-window menu bar, driven through the window.
///
/// This is the half of the application that rots, because nobody here develops on the platform it
/// runs on: `wants_menu_bar` is false on macOS, so on a Mac none of the elements below are drawn
/// at all. `cfg!` rather than `#[cfg]`, so both arms compile everywhere and the ones that can run
/// do — which is the only reason a Mac can find out that the Windows menu bar has stopped opening.
#[cfg(test)]
mod window_tests {
    use gpui::{Action, Modifiers, MouseButton, TestAppContext, px, size};

    use crate::harness::{click, open, paint, resize};
    use crate::menu::MenuRow;

    fn open_category(
        app: &gpui::Entity<crate::app::AurisApp>,
        selector: &'static str,
        cx: &mut gpui::VisualTestContext,
    ) {
        if app.read_with(cx, |app, _| app.menu_bar.is_none()) {
            click("application-menu", cx);
            paint(app, cx);
        }
        click(selector, cx);
    }

    /// Where the row carrying `action` is, as the section it is in and its place in that section.
    ///
    /// By action name — `gpui::Action::name` is a stable identifier — because a row's label is in
    /// whatever language the interface is in.
    fn row_of(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &gpui::TestAppContext,
        action: &str,
    ) -> (usize, usize) {
        app.read_with(cx, |this, _| {
            this.menu_model()
                .iter()
                .enumerate()
                .find_map(|(section, model)| {
                    model
                        .rows
                        .iter()
                        .enumerate()
                        .find_map(|(index, row)| match row {
                            MenuRow::Command { action: a, .. } if a.name() == action => {
                                Some((section, index))
                            }
                            _ => None,
                        })
                })
                .unwrap_or_else(|| panic!("no menu row runs {action}"))
        })
    }

    /// Which menu is dropped open, if any.
    fn open_menu(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &gpui::TestAppContext,
    ) -> Option<usize> {
        app.read_with(cx, |this, _| this.menu_bar.map(|open| open.index))
    }

    #[gpui::test]
    fn analysis_menu_opens_results_only_when_requested(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.read_with(cx, |this, _| assert!(!this.analysis_panel));
        if !crate::app::AurisApp::wants_menu_bar() {
            cx.dispatch_action(crate::actions::OpenAnalysisResults);
        } else {
            let (section, index) = row_of(&app, cx, crate::actions::OpenAnalysisResults.name());
            paint(&app, cx);
            let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());
            open_category(&app, title, cx);
            paint(&app, cx);
            let row: &'static str =
                Box::leak(format!("menu-bar-item-{section}-{index}").into_boxed_str());
            click(row, cx);
        }
        app.read_with(cx, |this, _| assert!(this.analysis_panel));
        paint(&app, cx);
        let handle = app.read_with(cx, |app, _| {
            app.auxiliary_windows[&crate::auxiliary_window::Surface::Analysis]
        });
        let mut analysis = gpui::VisualTestContext::from_window(handle.into(), cx);
        click("analysis-close", &mut analysis);
        app.read_with(cx, |this, _| assert!(!this.analysis_panel));
    }

    #[gpui::test]
    fn application_menu_opens_on_demand_and_its_trigger_closes_it(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            // macOS puts these in the system bar, where this window has nothing to click.
            assert!(open_menu(&app, cx).is_none());
            return;
        }
        paint(&app, cx);

        click("application-menu", cx);
        assert_eq!(open_menu(&app, cx), Some(0));

        paint(&app, cx);
        click("application-menu", cx);
        assert_eq!(open_menu(&app, cx), None, "the same title shuts it again");
    }

    /// Switching categories replaces the command list inside the same overlay.
    #[gpui::test]
    fn a_second_title_takes_the_menu_over_rather_than_opening_another(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        paint(&app, cx);

        open_category(&app, "menu-title-0", cx);
        paint(&app, cx);
        open_category(&app, "menu-title-1", cx);

        assert_eq!(open_menu(&app, cx), Some(1));
    }

    #[gpui::test]
    fn keyboard_menu_preserves_workspace_bounds_and_restores_focus(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        for width in [640.0, 1360.0] {
            resize(&app, cx, size(px(width), px(700.0)));
            let ruler = app.read_with(cx, |app, _| app.canvas.ruler.get().unwrap());
            let original = cx.update(|window, cx| window.focused(cx).unwrap());
            cx.simulate_keystrokes("f10");
            paint(&app, cx);
            assert_eq!(open_menu(&app, cx), Some(0));
            assert_eq!(
                app.read_with(cx, |app, _| app.canvas.ruler.get().unwrap()),
                ruler
            );
            let menu = cx.debug_bounds("application-menu-panel").unwrap();
            assert!(menu.left() >= px(0.0) && menu.right() <= px(width));
            assert!(menu.bottom() <= px(700.0));
            cx.simulate_keystrokes("right down");
            app.read_with(cx, |app, _| {
                let menu = app.menu_bar.unwrap();
                assert_eq!(menu.index, 1);
                assert!(menu.highlighted.is_some());
            });
            cx.simulate_keystrokes("escape");
            paint(&app, cx);
            assert_eq!(open_menu(&app, cx), None);
            cx.update(|window, _| assert!(original.is_focused(window)));
        }
    }

    #[gpui::test]
    fn hovering_a_row_replaces_the_keyboard_highlight(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        let (section, first) = row_of(&app, cx, crate::actions::NewProject.name());
        let (same_section, hovered) = row_of(&app, cx, crate::actions::OpenProject.name());
        assert_eq!(same_section, section);
        paint(&app, cx);
        let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());
        open_category(&app, title, cx);
        paint(&app, cx);

        cx.simulate_keystrokes("down");
        app.read_with(cx, |this, _| {
            assert_eq!(this.menu_bar.and_then(|open| open.highlighted), Some(first));
        });
        paint(&app, cx);
        let row: &'static str =
            Box::leak(format!("menu-bar-item-{section}-{hovered}").into_boxed_str());
        let bounds = cx.debug_bounds(row).expect("the hovered row is drawn");
        cx.simulate_mouse_move(bounds.center(), MouseButton::Left, Modifiers::none());

        app.read_with(cx, |this, _| {
            assert_eq!(
                this.menu_bar.and_then(|open| open.highlighted),
                Some(hovered),
                "the pointer and Enter share one current row"
            );
        });
    }

    #[gpui::test]
    fn hovering_a_disabled_row_or_separator_clears_keyboard_activation(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        app.update(cx, |this, _| {
            this.session
                .add_default_instrument_track("Undo sentinel")
                .unwrap();
            assert!(this.session.can_undo());
        });
        let before = app.read_with(cx, |this, _| this.project().clone());
        let (section, undo) = row_of(&app, cx, crate::actions::Undo.name());
        let (same_section, disabled) = row_of(&app, cx, crate::actions::Redo.name());
        assert_eq!(same_section, section);
        let separator = app.read_with(cx, |this, _| {
            this.menu_model()[section]
                .rows
                .iter()
                .position(|row| matches!(row, MenuRow::Separator))
                .expect("the Edit menu separates history from clipboard commands")
        });
        let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());

        for selector in [
            format!("menu-bar-item-{section}-{disabled}"),
            format!("menu-bar-separator-{section}-{separator}"),
        ] {
            paint(&app, cx);
            open_category(&app, title, cx);
            cx.simulate_keystrokes("down");
            app.read_with(cx, |this, _| {
                assert_eq!(
                    this.menu_bar.and_then(|open| open.highlighted),
                    Some(undo),
                    "Down initially arms the observable Undo command"
                );
            });
            paint(&app, cx);
            let selector: &'static str = Box::leak(selector.into_boxed_str());
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("`{selector}` is drawn"));
            cx.simulate_mouse_move(bounds.center(), MouseButton::Left, Modifiers::none());

            app.read_with(cx, |this, _| {
                assert_eq!(
                    this.menu_bar.and_then(|open| open.highlighted),
                    None,
                    "a non-actionable pointer target must clear stale keyboard activation"
                );
            });
            cx.simulate_keystrokes("enter");
            assert_eq!(
                open_menu(&app, cx),
                None,
                "Enter dismisses a menu with no highlighted command"
            );
            app.read_with(cx, |this, _| {
                assert_eq!(
                    this.project(),
                    &before,
                    "Enter must not run the stale Undo command"
                );
                assert!(this.session.can_undo());
            });
        }
    }

    /// Choosing a row runs its command, through the same dispatch the keymap uses.
    #[gpui::test]
    fn choosing_a_row_runs_the_command_and_shuts_the_menu(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        let (section, index) = row_of(&app, cx, crate::actions::ToggleLoop.name());
        let before = app.read_with(cx, |this, _| this.session.project().loop_enabled);
        paint(&app, cx);

        let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());
        open_category(&app, title, cx);
        paint(&app, cx);
        let row: &'static str =
            Box::leak(format!("menu-bar-item-{section}-{index}").into_boxed_str());
        click(row, cx);

        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.project().loop_enabled,
                !before,
                "the row did what it says"
            );
        });
        assert_eq!(open_menu(&app, cx), None, "and the menu shut behind it");
    }

    #[gpui::test]
    fn clicking_a_disabled_row_leaves_its_menu_open(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        let (section, index) = row_of(&app, cx, crate::actions::Undo.name());
        paint(&app, cx);
        let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());
        open_category(&app, title, cx);
        paint(&app, cx);
        let row: &'static str =
            Box::leak(format!("menu-bar-item-{section}-{index}").into_boxed_str());
        click(row, cx);

        assert_eq!(open_menu(&app, cx), Some(section));
    }

    /// A switch in the menu reads back the state it set, which is the whole job of the tick
    /// beside a label that is a noun.
    #[gpui::test]
    fn a_switch_in_the_menu_shows_the_state_it_is_in(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (section, index) = row_of(&app, cx, crate::actions::ToggleLoop.name());
        let ticked = |app: &gpui::Entity<crate::app::AurisApp>, cx: &gpui::TestAppContext| {
            app.read_with(cx, |this, _| {
                match &this.menu_model()[section].rows[index] {
                    MenuRow::Command { checked, .. } => *checked,
                    _ => panic!("row {index} of section {section} is not a command"),
                }
            })
        };

        assert!(!ticked(&app, cx), "a new project is not cycling");
        cx.dispatch_action(crate::actions::ToggleLoop);
        assert!(ticked(&app, cx), "and the row says so once it is");
    }

    #[gpui::test]
    fn a_long_menu_keeps_the_keyboard_highlight_inside_the_viewport(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        resize(&app, cx, size(px(800.0), px(500.0)));
        let (section, _) = row_of(&app, cx, crate::actions::NudgeNotesRight.name());
        let title: &'static str = Box::leak(format!("menu-title-{section}").into_boxed_str());
        open_category(&app, title, cx);
        paint(&app, cx);

        cx.simulate_keystrokes("end");
        paint(&app, cx);
        let last = app.read_with(cx, |this, _| {
            this.menu_bar
                .and_then(|open| open.highlighted)
                .expect("End highlights the last available row")
        });
        let row: &'static str =
            Box::leak(format!("menu-bar-item-{section}-{last}").into_boxed_str());
        let bounds = cx.debug_bounds(row).expect("the highlighted row is drawn");
        let viewport = cx
            .debug_bounds("menu-bar-scroll")
            .expect("the menu has a scrolling viewport");
        assert!(bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom());
        app.read_with(cx, |this, _| {
            assert!(this.menu_bar_scroll.offset().y < px(0.0))
        });

        cx.simulate_keystrokes("home");
        paint(&app, cx);
        let first = app.read_with(cx, |this, _| {
            this.menu_bar
                .and_then(|open| open.highlighted)
                .expect("Home highlights the first available row")
        });
        let row: &'static str =
            Box::leak(format!("menu-bar-item-{section}-{first}").into_boxed_str());
        let bounds = cx.debug_bounds(row).expect("the highlighted row is drawn");
        let viewport = cx.debug_bounds("menu-bar-scroll").unwrap();
        assert!(bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom());

        cx.simulate_keystrokes("escape");
        assert_eq!(open_menu(&app, cx), None, "Escape dismisses the dropdown");
    }
}
