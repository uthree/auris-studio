//! The status bar along the bottom of the window, and the panel switches at either end of it.
//!
//! Zed's arrangement: the row that reports what happened also carries an icon for every panel,
//! grouped by the dock it lives in — the left dock's at the left-hand end, the bottom dock's and
//! then the right dock's at the other. A press shows that panel, a press on the one already
//! showing shuts its dock, and a right-click offers to move it somewhere else.
//!
//! An icon for a panel that is *hidden* as well as for the one showing, which is the whole reason
//! the switches are here rather than on the panels themselves: a panel with no icon anywhere is a
//! panel with no way back except a menu nothing on screen mentions.

use auris_i18n::Key;

use gpui::{Context, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, div, prelude::*, px};

use crate::app::AurisApp;
use crate::dock::{Dock, Panel};
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::icons::icon;

/// Side of one switch, including the padding around its icon.
const SWITCH_SIZE: Pixels = px(18.0);
/// Side of the mark drawn inside it.
const SWITCH_ICON: Pixels = px(12.0);

impl AurisApp {
    /// Renders the status bar.
    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let audio = self.session.audio_status();
        let engine = if audio.running {
            format!("{:.0} Hz · {} ch", audio.sample_rate, audio.channels)
        } else {
            self.t(Key::EngineSilent).to_string()
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(Metrics::STATUS_HEIGHT)
            .px_1()
            .bg(theme.surface_raised)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.text_muted)
            .child(self.dock_switches(Dock::Left, cx))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    // A failure was reported in the same pale grey as the sample rate beside it,
                    // in a row a user has no reason to be looking at.
                    .when(self.status_failed, |this| this.text_color(theme.danger))
                    .child(self.status.clone()),
            )
            .child(self.window_title())
            .child(engine)
            // The bottom dock's switches before the right dock's, which is the order they sit in
            // going clockwise from the middle of the window.
            .child(self.dock_switches(Dock::Bottom, cx))
            .child(self.dock_switches(Dock::Right, cx))
    }

    /// The switches for every panel that lives in `dock`.
    fn dock_switches(&self, dock: Dock, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        div().flex().items_center().gap(px(1.0)).children(
            self.panels
                .panels_in(dock)
                .map(|panel| self.panel_switch(panel, &theme, cx)),
        )
    }

    /// One panel's switch.
    fn panel_switch(
        &self,
        panel: Panel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.panels.is_open(panel);
        // Latched against the accent, the way every other toggle in the window is, but softly: the
        // status bar is the quietest row on screen and a solid accent square down there reads as an
        // alert. The mark itself carries the state, and goes faint when the panel is away.
        let (background, mark) = match open {
            true => (Theme::translucent(theme.accent, 0.22), theme.text),
            false => (gpui::transparent_black(), theme.text_faint),
        };
        // The log is the one panel that is closed by default, so it is the one panel that has to
        // be able to say *look at me*. Nothing else on screen reports a warning, and a warning
        // nobody sees is the same as one that was never written: a font that could not be found,
        // a plugin substituted, a device that disappeared.
        let unread = match panel {
            Panel::Log if !open => crate::logbook::book().problems() > 0,
            _ => false,
        };
        let mark = match unread {
            true => theme.warning,
            false => mark,
        };
        let hover = theme.surface_hover;

        div()
            .id(("panel-switch", panel as usize))
            .flex()
            .items_center()
            .justify_center()
            .size(SWITCH_SIZE)
            .flex_shrink_0()
            .rounded(Metrics::RADIUS_SM)
            .bg(background)
            .cursor_pointer()
            .hover(|this| this.bg(hover))
            .active(|this| this.opacity(0.75))
            .child(icon(panel.icon(), SWITCH_ICON, mark))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.toggle_panel(panel);
                    cx.notify();
                }),
            )
            // Where a panel is moved from. The switch is the one part of a hidden panel that is
            // still on screen, so it is the only thing a "put that over there" can be aimed at.
            .on_mouse_down(
                MouseButton::Right,
                Self::opens_menu(cx, move |this, at| this.panel_menu(panel, at)),
            )
    }

    /// The menu a right-click on a panel's switch opens.
    fn panel_menu(&self, panel: Panel, anchor: Point<Pixels>) -> ContextMenu {
        let here = self.panels.dock(panel);
        let menu = Dock::ALL.into_iter().fold(
            ContextMenu::new(anchor, self.t(panel.label())),
            |menu, dock| {
                menu.toggle(
                    self.t(dock.label()),
                    MenuCommand::DockPanel { panel, dock },
                    dock == here,
                )
            },
        );
        // Only when there is something to hide. Offering it on a panel that is already away would
        // be a row that does nothing, next to three that say where it is.
        menu.separator().item_if(
            self.panels.is_open(panel),
            self.t(Key::HidePanel),
            MenuCommand::TogglePanel(panel),
        )
    }
}
