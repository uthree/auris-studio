//! The widget itself: what a menu is made of, how it is built, how big it is and how it draws.
//!
//! A menu is plain data — rows carrying a [`MenuCommand`] rather than closures — so the
//! component that knows what was clicked can build one, and this file can place and paint it
//! without knowing anything about what it offers.
//!
//! [`ContextMenu::size`] and [`AurisApp::render_context_menu`] stay together deliberately: the
//! measurement decides where the menu lands and the drawing has to agree with it pixel for
//! pixel, so the constants below are shared between them and a change to one is a change to the
//! other.

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, ScrollHandle,
    SharedString, Size, Window, div, point, prelude::*, px, size,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::icons::{Icon, icon};

use super::MenuCommand;

/// Height of one row.
const ITEM_HEIGHT: Pixels = px(22.0);
/// Height taken by the rule between two groups, including the space either side of it.
const SEPARATOR_HEIGHT: Pixels = px(7.0);
/// Height of the heading naming what the menu acts on.
const TITLE_HEIGHT: Pixels = px(20.0);
/// Padding above the first row and below the last.
const PADDING: Pixels = px(4.0);
/// The menu's own border, which sits inside the width and height it is given.
const BORDER: Pixels = px(1.0);
/// Width of the column holding the tick on a latched item.
const MARK_WIDTH: f32 = 18.0;
/// Narrowest a menu may be.
const MIN_WIDTH: f32 = 168.0;
/// Widest a menu may be.
const MAX_WIDTH: f32 = 300.0;
/// Rough advance width of one Latin character at the menu's text size.
///
/// Only used to pick a width — the labels themselves are truncated, so an over- or
/// under-estimate costs a little whitespace rather than a clipped word.
const CHARACTER_WIDTH: f32 = 6.6;

fn estimated_label_width(label: &str) -> f32 {
    label
        .chars()
        .map(|character| match character.is_ascii() {
            true => CHARACTER_WIDTH,
            false => CHARACTER_WIDTH * 2.0,
        })
        .sum()
}

/// One row in a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// Text shown in the row.
    pub label: SharedString,
    /// What choosing it does.
    pub command: MenuCommand,
    /// Whether the row can be chosen.
    ///
    /// `false` in three places in the application and no more, all of them lists: a bus that
    /// would loop if it were routed to, and the line that stands in for an empty Recent menu.
    /// There the greyed row is itself the answer — a bus quietly missing from the list of every
    /// bus reads as a bus that has been deleted.
    ///
    /// Everywhere else a row that does not apply is simply not built; see
    /// [`ContextMenu::item_if`]. A menu is titled after one object, and a row offering to clear
    /// the fades of a clip that has none is not saying "not now" — it is saying something untrue
    /// about the clip in its own title.
    pub enabled: bool,
    /// Whether the row shows a tick, for the items that latch.
    pub checked: bool,
    /// A colour shown at the end of the row, for choices that *are* a colour.
    ///
    /// The row still carries text, so the keyboard and the eye have something to land on — but
    /// the swatch is what the choice actually is, and it is the same in every language. The
    /// alternative was naming eight palette entries twice over, in a set where two of them are
    /// both fairly called orange.
    pub swatch: Option<gpui::Hsla>,
}

/// A row in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A choice.
    Item(MenuItem),
    /// The rule between two groups.
    Separator,
}

/// An open context menu.
#[derive(Clone, Debug)]
pub struct ContextMenu {
    /// Where the pointer was when it opened, in window coordinates.
    pub anchor: Point<Pixels>,
    /// What the menu acts on, shown as a heading.
    pub title: SharedString,
    /// The rows.
    pub entries: Vec<MenuEntry>,
    /// The row the keyboard is on, once the keyboard has been used.
    ///
    /// `None` until then, so a menu opened with the mouse does not draw a highlight nobody asked
    /// for — and so the first arrow key lands on the first row rather than the second.
    pub highlighted: Option<usize>,
    /// The rows' scroll position, shared with the keyboard so its highlight stays visible.
    pub scroll: ScrollHandle,
    /// An optional background request allowed to replace this menu's rows.
    pub(crate) async_request: Option<u64>,
}

impl ContextMenu {
    /// An empty menu anchored at `anchor`.
    pub fn new(anchor: Point<Pixels>, title: impl Into<SharedString>) -> Self {
        Self {
            anchor,
            title: title.into(),
            entries: Vec::new(),
            highlighted: None,
            scroll: ScrollHandle::new(),
            async_request: None,
        }
    }

    /// Moves the highlight `delta` rows, skipping separators and anything disabled.
    ///
    /// Wraps, and starts from whichever end the direction implies, so the first Down lands on the
    /// first row and the first Up on the last. A menu with nothing to choose leaves it alone.
    pub fn step(&mut self, delta: isize) {
        let choosable: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, MenuEntry::Item(item) if item.enabled))
            .map(|(index, _)| index)
            .collect();
        let Some(&first) = choosable.first() else {
            return;
        };
        let Some(&last) = choosable.last() else {
            return;
        };
        let Some(current) = self.highlighted else {
            let next = if delta >= 0 { first } else { last };
            self.highlighted = Some(next);
            self.scroll.scroll_to_item(next);
            return;
        };
        // Where the current row sits among the choosable ones, or just before the next one down
        // when it is a separator that somehow held the highlight.
        let at = choosable
            .iter()
            .position(|index| *index == current)
            .map_or(-1, |position| position as isize);
        let count = choosable.len() as isize;
        let next = (at + delta).rem_euclid(count) as usize;
        self.highlighted = Some(choosable[next]);
        self.scroll.scroll_to_item(choosable[next]);
    }

    /// The command the highlighted row would run.
    pub fn highlighted_command(&self) -> Option<MenuCommand> {
        match self.entries.get(self.highlighted?) {
            Some(MenuEntry::Item(item)) if item.enabled => Some(item.command.clone()),
            _ => None,
        }
    }

    /// Adds a row.
    pub fn item(self, label: impl Into<SharedString>, command: MenuCommand) -> Self {
        self.push(label, command, true, false)
    }

    /// Adds a row only when `shown`, and leaves it out entirely otherwise.
    ///
    /// The default for anything conditional, because a context menu is titled after one object
    /// and its rows are the things that can be done to *that* object. A MIDI clip has no fades,
    /// so a menu offering to clear them is not saying "not now" — it is saying something untrue
    /// about the clip it is named after. The rest of the application answers "nothing selected"
    /// with a line in the status bar rather than with a row.
    ///
    /// [`Self::item_greyed_unless`] is the other case, and is rare: a row worth keeping on screen
    /// *because* being unavailable is itself the answer.
    pub fn item_if(
        self,
        shown: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
    ) -> Self {
        match shown {
            true => self.push(label, command, true, false),
            false => self,
        }
    }

    /// Adds a run of rows that share one condition.
    pub fn items_if<L: Into<SharedString>>(
        self,
        shown: bool,
        rows: impl IntoIterator<Item = (L, MenuCommand)>,
    ) -> Self {
        rows.into_iter().fold(self, |menu, (label, command)| {
            menu.item_if(shown, label, command)
        })
    }

    /// Adds a row that is always on screen and is greyed unless `enabled`.
    ///
    /// For the row whose unavailability is the point. Three in the application, and each is the
    /// answer to a question the user is about to ask: a bus that cannot be routed to because it
    /// would loop, and the "no recent projects" line that stops an empty menu reading as a menu
    /// that failed. Everything else conditional wants [`Self::item_if`].
    pub fn item_greyed_unless(
        self,
        enabled: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
    ) -> Self {
        self.push(label, command, enabled, false)
    }

    /// Adds a row that shows a tick when `checked`, only when `shown`.
    pub fn toggle_if(
        self,
        shown: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
        checked: bool,
    ) -> Self {
        match shown {
            true => self.push(label, command, true, checked),
            false => self,
        }
    }

    /// A ticking row that is always on screen and greyed unless `enabled`.
    ///
    /// [`Self::item_greyed_unless`]'s reasoning, for a row that also has to say which way it is
    /// set — the bus a track is routed to, listed beside the ones it may not be routed to.
    pub fn toggle_greyed_unless(
        self,
        enabled: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
        checked: bool,
    ) -> Self {
        self.push(label, command, enabled, checked)
    }

    /// Adds a row that shows a tick when `checked`.
    pub fn toggle(
        self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        checked: bool,
    ) -> Self {
        self.push(label, command, true, checked)
    }

    /// Adds the rule between two groups.
    pub fn separator(mut self) -> Self {
        // Never leads, never doubles: a menu built from conditional groups would otherwise show
        // a rule against its own top edge or two rules in a row.
        if matches!(self.entries.last(), Some(MenuEntry::Item(_))) {
            self.entries.push(MenuEntry::Separator);
        }
        self
    }

    fn push(
        mut self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        enabled: bool,
        checked: bool,
    ) -> Self {
        self.entries.push(MenuEntry::Item(MenuItem {
            label: label.into(),
            command,
            enabled,
            checked,
            swatch: None,
        }));
        self
    }

    /// Adds a row whose choice is a colour, shown as a swatch and ticked when it is the one in use.
    pub fn colour(
        mut self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        swatch: gpui::Hsla,
        checked: bool,
    ) -> Self {
        self.entries.push(MenuEntry::Item(MenuItem {
            label: label.into(),
            command,
            enabled: true,
            checked,
            swatch: Some(swatch),
        }));
        self
    }

    /// `true` when the menu has nothing to show.
    pub fn is_empty(&self) -> bool {
        !self
            .entries
            .iter()
            .any(|entry| matches!(entry, MenuEntry::Item(_)))
    }

    /// The menu's natural size before constraining it to the window.
    pub fn size(&self) -> Size<Pixels> {
        let widest = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(estimated_label_width(&item.label)),
                MenuEntry::Separator => None,
            })
            .reduce(f32::max)
            .unwrap_or(0.0);
        let width = (widest + MARK_WIDTH + 24.0).clamp(MIN_WIDTH, MAX_WIDTH);
        let height = self.entries.iter().fold(
            TITLE_HEIGHT + PADDING * 2.0 + BORDER * 2.0,
            |total, entry| {
                total
                    + match entry {
                        MenuEntry::Item(_) => ITEM_HEIGHT,
                        MenuEntry::Separator => SEPARATOR_HEIGHT,
                    }
            },
        );
        size(px(width), height)
    }

    /// The on-screen size: long menus scroll their rows beneath a fixed heading.
    pub fn visible_size(&self, viewport: Size<Pixels>) -> Size<Pixels> {
        let natural = self.size();
        size(
            natural.width.min(viewport.width),
            natural.height.min(viewport.height),
        )
    }

    /// Where the menu's top-left corner goes inside a window of `viewport`.
    ///
    /// Flips to the other side of the pointer when there is room there. If neither side fits,
    /// clamps inside the viewport so every command remains reachable.
    pub fn origin(&self, viewport: Size<Pixels>) -> Point<Pixels> {
        let size = self.visible_size(viewport);
        let x = origin_on_axis(self.anchor.x, size.width, viewport.width);
        let y = origin_on_axis(self.anchor.y, size.height, viewport.height);
        point(x, y)
    }
}

/// Keeps the pointer clear when possible, otherwise gives the whole menu room on screen.
/// A menu opens on a right press or completed picker click; activation requires a fresh left
/// press, so clamping it under the pointer cannot activate a command with its opening gesture.
fn origin_on_axis(anchor: Pixels, extent: Pixels, viewport: Pixels) -> Pixels {
    let extent = extent.min(viewport);
    let before = anchor.max(px(0.0)).min(viewport);
    let after = (viewport - anchor).max(px(0.0));
    if extent <= after {
        anchor.max(px(0.0))
    } else if extent <= before {
        anchor.min(viewport) - extent
    } else {
        anchor.max(px(0.0)).min(viewport - extent)
    }
}

/// The pointer position a mouse event carries.
///
/// One trait rather than two helpers, because a menu is opened two ways and neither is the odd
/// one out: a picker button opens one on a click, and a surface — the ruler, a track header, a
/// mixer strip — opens one on a right-press. gpui hands those to handlers of different event
/// types, and the only thing either handler wants from the event is where it happened.
pub(crate) trait MenuAt {
    /// Where the pointer was, in window coordinates.
    fn menu_at(&self) -> Point<Pixels>;
}

impl MenuAt for gpui::ClickEvent {
    fn menu_at(&self) -> Point<Pixels> {
        self.position()
    }
}

impl MenuAt for MouseDownEvent {
    fn menu_at(&self) -> Point<Pixels> {
        self.position
    }
}

impl AurisApp {
    /// Shows a menu, unless it has nothing to offer.
    pub(crate) fn open_menu(&mut self, menu: ContextMenu) {
        if !menu.is_empty() {
            self.menu = Some(menu);
        }
    }

    /// A handler that opens the menu `build` makes, anchored where the pointer is.
    ///
    /// Nearly thirty controls in this application open a menu, and every one of them wants the
    /// same three lines: build it at the pointer, show it, redraw. Spelling those out at each
    /// site is how one of them ends up forgetting the redraw and opening a menu that only appears
    /// once something else asks for a frame.
    ///
    /// `build` is handed the application, so a control that has to do something else first — a
    /// mixer strip selects its track before offering the menu for it — still can.
    pub(crate) fn opens_menu<E, F>(
        cx: &Context<Self>,
        build: F,
    ) -> impl Fn(&E, &mut Window, &mut gpui::App) + use<E, F>
    where
        E: MenuAt + 'static,
        F: Fn(&mut Self, Point<Pixels>) -> ContextMenu + 'static,
    {
        // What `Context::listener` does, written out. Its return type borrows the context it was
        // made from, and a handler hung on an element has to outlive the frame that built it.
        let view = cx.entity().downgrade();
        move |event: &E, _: &mut Window, cx: &mut gpui::App| {
            let at = event.menu_at();
            view.update(cx, |this, cx| {
                let menu = build(this, at);
                let opened = !menu.is_empty();
                this.open_menu(menu);
                if opened {
                    cx.stop_propagation();
                }
                cx.notify();
            })
            .ok();
        }
    }

    /// Closes any open menu, reporting whether there was one.
    pub(crate) fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Draws the open menu over everything else.
    pub(crate) fn render_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let theme = self.theme.clone();
        let size = menu.visible_size(window.viewport_size());
        let origin = menu.origin(window.viewport_size());

        let rows: Vec<AnyElement> = menu
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                MenuEntry::Separator => div()
                    .my(px(3.0))
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    // The subtle border is a shade off the raised surface the menu is drawn on,
                    // which makes the rule invisible exactly where it has a job to do.
                    .bg(theme.border)
                    .into_any_element(),
                MenuEntry::Item(item) => {
                    let command = item.command.clone();
                    let enabled = item.enabled;
                    div()
                        .id(("menu-item", index))
                        // By position rather than by label: a row is named in whatever language
                        // the interface is in, and a test that found a row by its words would be
                        // a test of the translations. See `crate::harness::choose`.
                        .debug_selector(move || format!("menu-item-{index}"))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .h(ITEM_HEIGHT)
                        .px_1p5()
                        .rounded(Metrics::RADIUS_SM)
                        .text_xs()
                        .text_color(if enabled {
                            theme.text
                        } else {
                            theme.text_faint
                        })
                        .when(enabled, |this| {
                            this.cursor_pointer().hover(|this| {
                                this.bg(theme.accent).text_color(theme.text_on_accent)
                            })
                        })
                        // The keyboard's row is drawn as though the pointer were on it, so Down
                        // and a hover mean the same thing on screen as they do to the menu.
                        .when(menu.highlighted == Some(index), |this| {
                            this.bg(theme.accent).text_color(theme.text_on_accent)
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(MARK_WIDTH))
                                .flex_shrink_0()
                                .when(item.checked, |this| {
                                    // Not the accent colour: the hover state fills the row with
                                    // it, and a tick would vanish exactly when it is pointed at.
                                    this.child(icon(Icon::Check, px(10.0), theme.text))
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(item.label.clone()),
                        )
                        .children(item.swatch.map(|colour| {
                            div()
                                .flex_shrink_0()
                                .ml_2()
                                .w(px(22.0))
                                .h(px(10.0))
                                .rounded(Metrics::RADIUS_XS)
                                .bg(colour)
                        }))
                        .when(enabled, |this| {
                            this.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    let command = command.clone();
                                    this.close_menu();
                                    this.run_menu_command(command, cx);
                                    cx.notify();
                                }),
                            )
                        })
                        .into_any_element()
                }
            })
            .collect();

        Some(
            // A full-window backdrop, so a click anywhere else dismisses the menu the way a
            // native one does. It is transparent to the eye and not to the pointer: a dismissing
            // click used to reach whatever was behind it as well, so shutting a menu opened over
            // the arrangement also moved the playhead to wherever the pointer happened to be.
            div()
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_menu();
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_menu();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .debug_selector(|| "context-menu-panel".to_string())
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(size.width)
                        .h(size.height)
                        .flex()
                        .flex_col()
                        .p(PADDING)
                        .rounded(Metrics::RADIUS_MD)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        // Clicks inside the menu must not reach the backdrop behind it, or the
                        // menu would close before the row underneath the pointer could act.
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(TITLE_HEIGHT)
                                .flex_shrink_0()
                                .px_1p5()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .truncate()
                                .child(menu.title.clone()),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("context-menu-rows")
                                        .debug_selector(|| "context-menu-rows".to_string())
                                        .flex()
                                        .flex_col()
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&menu.scroll)
                                        .when(menu.size().height > size.height, |this| this.pr_3())
                                        .children(rows),
                                )
                                .child(
                                    Scrollbar::vertical(&menu.scroll)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(anchor: Point<Pixels>, items: usize) -> ContextMenu {
        (0..items).fold(ContextMenu::new(anchor, "Track 1"), |menu, index| {
            menu.item(format!("Item {index}"), MenuCommand::NewAudioTrack)
        })
    }

    #[test]
    fn the_keyboard_walks_the_rows_and_wraps() {
        let mut menu = menu(gpui::point(px(0.0), px(0.0)), 3);
        assert_eq!(
            menu.highlighted, None,
            "a menu opened with the mouse draws no highlight"
        );

        menu.step(1);
        assert_eq!(
            menu.highlighted,
            Some(0),
            "the first Down lands on the first row"
        );
        menu.step(1);
        menu.step(1);
        assert_eq!(menu.highlighted, Some(2));
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0), "and wraps round the end");
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(2), "and back the other way");
    }

    #[test]
    fn a_stale_highlight_steps_to_the_first_row() {
        let mut menu = menu(gpui::point(px(0.0), px(0.0)), 3);
        menu.highlighted = Some(usize::MAX);
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0));
    }

    #[test]
    fn japanese_labels_are_measured_as_full_width_text() {
        assert_eq!(estimated_label_width("保存"), estimated_label_width("save"));
        let japanese = ContextMenu::new(point(px(0.0), px(0.0)), "Menu")
            .item("長い日本語メニュー項目", MenuCommand::NewAudioTrack);
        let latin = ContextMenu::new(point(px(0.0), px(0.0)), "Menu")
            .item("xxxxxxxxxxxxxxxxxxxxxx", MenuCommand::NewAudioTrack);
        assert!(
            f32::from(japanese.size().width - latin.size().width).abs() < 0.001,
            "equivalent full- and half-width labels receive the same menu width"
        );
    }

    #[test]
    fn the_first_up_lands_on_the_last_row() {
        let mut menu = menu(gpui::point(px(0.0), px(0.0)), 3);
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(2));
    }

    #[test]
    fn separators_and_disabled_rows_are_stepped_over() {
        // Return on a separator would run nothing, and stopping on a disabled row is a keypress
        // the user has to make twice.
        let menu = ContextMenu::new(gpui::point(px(0.0), px(0.0)), "Track 1")
            .item("Rename", MenuCommand::NewAudioTrack)
            .separator()
            .item_greyed_unless(false, "Delete", MenuCommand::NewAudioTrack)
            .item("Duplicate", MenuCommand::NewAudioTrack);

        let mut walking = menu.clone();
        walking.step(1);
        assert_eq!(walking.highlighted, Some(0));
        walking.step(1);
        assert_eq!(
            walking.highlighted,
            Some(3),
            "past the rule and the dead row"
        );
        assert!(walking.highlighted_command().is_some());

        // A menu with nothing choosable in it leaves the highlight alone rather than pointing at
        // a rule.
        let mut rules = ContextMenu::new(gpui::point(px(0.0), px(0.0)), "Nothing").separator();
        rules.step(1);
        assert_eq!(rules.highlighted, None);
        assert!(rules.highlighted_command().is_none());
    }

    #[test]
    fn a_menu_that_fits_opens_at_the_pointer() {
        let anchor = point(px(100.0), px(80.0));
        let menu = menu(anchor, 4);
        assert_eq!(menu.origin(size(px(1200.0), px(800.0))), anchor);
    }

    #[test]
    fn a_menu_near_an_edge_flips_to_the_other_side_of_the_pointer() {
        let viewport = size(px(400.0), px(300.0));
        let menu = menu(point(px(390.0), px(290.0)), 6);
        let size = menu.size();
        let origin = menu.origin(viewport);

        assert_eq!(origin.x, px(390.0) - size.width);
        assert_eq!(origin.y, px(290.0) - size.height);
        assert!(
            origin.x + size.width <= px(390.0) && origin.y + size.height <= px(290.0),
            "a flipped menu must clear the pointer, or it swallows the next click"
        );
    }

    #[test]
    fn a_menu_in_a_narrow_gap_keeps_every_command_inside_the_window() {
        let viewport = size(px(500.0), px(500.0));
        let menu = (0..12).fold(
            ContextMenu::new(point(px(250.0), px(250.0)), "Clip"),
            |menu, index| {
                menu.item(
                    match index {
                        0 => "A label long enough to make this menu its maximum width".to_string(),
                        _ => format!("Item {index}"),
                    },
                    MenuCommand::NewAudioTrack,
                )
            },
        );
        let menu_size = menu.size();
        assert_eq!(menu_size.width, px(MAX_WIDTH));
        assert!(menu_size.height > px(250.0) && menu_size.height < viewport.height);

        let origin = menu.origin(viewport);
        assert!(
            origin.x >= px(0.0) && origin.x + menu_size.width <= viewport.width,
            "horizontal clamping keeps the menu in the window"
        );
        assert!(
            origin.y >= px(0.0) && origin.y + menu_size.height <= viewport.height,
            "vertical clamping keeps the last command reachable"
        );
    }

    #[test]
    fn a_menu_larger_than_the_window_scrolls_within_the_window() {
        let menu = menu(point(px(10.0), px(20.0)), 40);
        let viewport = size(px(400.0), px(200.0));
        let origin = menu.origin(viewport);
        assert_eq!(origin, point(px(10.0), px(0.0)));
        assert_eq!(menu.visible_size(viewport).height, viewport.height);
        assert!(menu.size().height > viewport.height);
    }

    #[test]
    fn separators_never_lead_or_double_up() {
        let built = ContextMenu::new(point(px(0.0), px(0.0)), "Track")
            .separator()
            .item("One", MenuCommand::NewAudioTrack)
            .separator()
            .separator()
            .item("Two", MenuCommand::NewAudioTrack);
        assert_eq!(
            built.entries,
            vec![
                MenuEntry::Item(MenuItem {
                    label: "One".into(),
                    command: MenuCommand::NewAudioTrack,
                    enabled: true,
                    checked: false,
                    swatch: None,
                }),
                MenuEntry::Separator,
                MenuEntry::Item(MenuItem {
                    label: "Two".into(),
                    command: MenuCommand::NewAudioTrack,
                    enabled: true,
                    checked: false,
                    swatch: None,
                }),
            ]
        );
    }

    #[test]
    fn a_menu_of_separators_alone_counts_as_empty() {
        let empty = ContextMenu::new(point(px(0.0), px(0.0)), "Nothing").separator();
        assert!(empty.is_empty());
        assert!(!menu(point(px(0.0), px(0.0)), 1).is_empty());
    }

    #[test]
    fn the_height_matches_what_gets_drawn() {
        let built = ContextMenu::new(point(px(0.0), px(0.0)), "Track")
            .item("One", MenuCommand::NewAudioTrack)
            .separator()
            .item("Two", MenuCommand::NewAudioTrack);
        assert_eq!(
            built.size().height,
            TITLE_HEIGHT + PADDING * 2.0 + BORDER * 2.0 + ITEM_HEIGHT * 2.0 + SEPARATOR_HEIGHT
        );
    }
}

/// Every menu the application can raise, asked whether it has anything in it.
///
/// `open_menu` drops an empty menu on the floor, so a menu whose every row turned out to be
/// conditional does not open at all — which reads as a broken control rather than as a menu with
/// nothing to offer. That became a live risk when `item_if` started leaving rows out instead of
/// greying them, and it is exactly the kind of thing that only shows up in the one document state
/// nobody tries by hand.
#[cfg(test)]
mod window_tests {
    use gpui::{TestAppContext, point, px, size};

    use auris_session::prelude::*;

    use crate::harness::{open, paint, resize, with_a_clip};

    #[gpui::test]
    fn tall_menus_keep_the_keyboard_selection_visible_and_clickable(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        for height in [600.0, 480.0] {
            let before = app.read_with(cx, |this, _| this.project().tracks.len());
            let viewport = size(px(640.0), px(height));
            resize(&app, cx, viewport);
            app.update(cx, |this, _| {
                let menu = (0..40).fold(
                    super::ContextMenu::new(point(px(320.0), px(height / 2.0)), "Long menu"),
                    |menu, index| {
                        menu.item(format!("Item {index}"), super::MenuCommand::NewAudioTrack)
                    },
                );
                this.open_menu(menu);
            });
            paint(&app, cx);
            let panel = cx.debug_bounds("context-menu-panel").unwrap();
            assert!(panel.top() >= px(0.0) && panel.bottom() <= viewport.height);
            assert!(panel.left() >= px(0.0) && panel.right() <= viewport.width);
            cx.simulate_keystrokes("up");
            paint(&app, cx);
            let last = cx.debug_bounds("menu-item-39").unwrap();
            let rows = cx.debug_bounds("context-menu-rows").unwrap();
            assert!(
                last.top() >= rows.top() && last.bottom() <= rows.bottom(),
                "Up reveals the last row: {last:?} inside {rows:?}"
            );
            cx.simulate_keystrokes("down");
            paint(&app, cx);
            let first = cx.debug_bounds("menu-item-0").unwrap();
            assert!(
                first.top() >= rows.top() && first.bottom() <= rows.bottom(),
                "wrapping reveals the first row again"
            );
            cx.simulate_keystrokes("up");
            paint(&app, cx);
            let last = cx.debug_bounds("menu-item-39").unwrap();
            cx.simulate_click(last.center(), gpui::Modifiers::none());
            app.read_with(cx, |this, _| {
                assert!(this.menu.is_none(), "the revealed command can be clicked");
                assert_eq!(this.project().tracks.len(), before + 1);
            });
        }
    }

    /// Where a menu is anchored. Nothing here depends on it.
    fn anchor() -> gpui::Point<gpui::Pixels> {
        point(px(100.0), px(100.0))
    }

    /// Every menu that can be raised over an empty document, by name.
    fn menus_of_an_empty_document(
        this: &mut crate::app::AurisApp,
    ) -> Vec<(&'static str, super::ContextMenu)> {
        let at = anchor();
        vec![
            ("arrangement", this.arrangement_menu(at)),
            ("ruler", this.ruler_menu(at, Ticks::ZERO)),
            ("signature", this.signature_menu(at, Ticks::ZERO)),
            ("structure", this.structure_menu(at, Ticks::ZERO)),
            ("harmony", this.harmony_menu(at, Ticks::ZERO)),
            (
                "progression picker",
                this.progression_picker_menu(at, Ticks::ZERO),
            ),
            ("count-in", this.count_in_menu(at)),
            ("recent", this.recent_menu(at)),
        ]
    }

    #[gpui::test]
    fn no_menu_over_an_empty_document_opens_onto_nothing(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            for (name, menu) in menus_of_an_empty_document(this) {
                assert!(
                    !menu.is_empty(),
                    "the {name} menu has no rows, so it would not open at all"
                );
            }
        });
    }

    /// The states a menu about a *clip* can be raised in, including the emptiest: a clip with no
    /// notes, nothing selected, and nothing on the clipboard.
    #[gpui::test]
    fn no_menu_about_a_clip_opens_onto_nothing(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.selected_notes.clear();
            assert!(
                this.session.clipboard().is_empty(),
                "the emptiest case is the one worth checking"
            );
            let at = anchor();
            for (name, menu) in [
                ("clip", this.clip_menu(at, clip)),
                ("track", this.track_menu(at, track)),
                ("lane", this.lane_menu(at, track, Ticks::ZERO)),
                ("output", this.output_menu(at, track)),
                ("effect picker", this.effect_picker_menu(at, Some(track))),
                ("clip preset", this.clip_preset_menu(at, clip)),
                ("clip subdivision", this.clip_subdivision_menu(at, clip)),
                ("clip octave", this.clip_octave_menu(at, clip)),
                ("clip groove", this.clip_groove_menu(at, clip)),
            ] {
                assert!(
                    !menu.is_empty(),
                    "the {name} menu has no rows, so it would not open at all"
                );
            }
        });
    }

    /// The piano roll's menu, in the state that has the least in it: empty grid, nothing
    /// selected, nothing to paste. Every row but one is conditional on a selection.
    #[gpui::test]
    fn the_roll_menu_over_empty_grid_still_has_something_in_it(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.selected_notes.clear();
            this.open_clip_in_editor(clip);
            let menu = this.roll_menu(anchor(), None, 60, Ticks::ZERO);
            assert!(
                !menu.is_empty(),
                "a right-press on empty grid has to offer at least Add Note Here"
            );
        });
    }
}
