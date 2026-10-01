//! Visibility and ordering controls for the top toolbar.

use super::*;
use crate::ui::widgets::{ButtonState, button_enabled};

/// Localized name of a toolbar control.
fn label(item: ToolbarItem) -> Key {
    match item {
        ToolbarItem::Playback => Key::ToolbarPlayback,
        ToolbarItem::Record => Key::CmdRecord,
        ToolbarItem::Punch => Key::CmdTogglePunch,
        ToolbarItem::Loop => Key::CmdToggleCycle,
        ToolbarItem::Metronome => Key::CmdToggleMetronome,
        ToolbarItem::Grid => Key::Grid,
        ToolbarItem::Zoom => Key::Zoom,
        ToolbarItem::Position => Key::Position,
        ToolbarItem::Tempo => Key::Tempo,
        ToolbarItem::Signature => Key::Signature,
        ToolbarItem::Chord => Key::CurrentChord,
        ToolbarItem::Take => Key::TakeClock,
        ToolbarItem::Input => Key::InputMeter,
        ToolbarItem::Master => Key::Master,
        ToolbarItem::Visualizer => Key::ToolbarVisualizer,
    }
}

impl SettingsWindow {
    fn apply_toolbar(&mut self, mut toolbar: ToolbarPreferences, cx: &mut Context<Self>) {
        toolbar.normalize();
        self.toolbar = toolbar.clone();
        let _ = self.app.update(cx, |app, cx| {
            app.apply_toolbar(toolbar);
            if !app.settings.toolbar.contains(ToolbarItem::Visualizer) && !app.visualizer.open {
                app.session.stop_visualizer();
                app.visualizer.clear_live();
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn render_toolbar_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mut rows = vec![
            section_title(self.t(Key::ToolbarHeading), theme),
            note(self.t(Key::ToolbarNote), theme),
        ];
        for (lane, title) in [
            Key::ToolbarPlayback,
            Key::ToolbarEditing,
            Key::ToolbarReadouts,
            Key::ToolbarMeters,
        ]
        .into_iter()
        .enumerate()
        {
            rows.push(section_title(self.t(title), theme));
            let entries: Vec<_> = self
                .toolbar
                .entries
                .iter()
                .filter(|entry| entry.item.lane() == lane)
                .collect();
            for (position, entry) in entries.iter().enumerate() {
                let item = entry.item;
                let index = item as usize;
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(self.t(label(item))),
                    )
                    .child(button(
                        ("toolbar-visible", index),
                        self.t(if entry.visible {
                            Key::ToolbarShow
                        } else {
                            Key::ToolbarHide
                        }),
                        ButtonStyle::Normal,
                        entry.visible,
                        theme.accent,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            let mut toolbar = this.toolbar.clone();
                            if let Some(entry) =
                                toolbar.entries.iter_mut().find(|entry| entry.item == item)
                            {
                                entry.visible = !entry.visible;
                            }
                            this.apply_toolbar(toolbar, cx);
                        }),
                    ));
                for (forward, enabled, key, id) in [
                    (false, position > 0, Key::ToolbarEarlier, "toolbar-earlier"),
                    (
                        true,
                        position + 1 < entries.len(),
                        Key::ToolbarLater,
                        "toolbar-later",
                    ),
                ] {
                    row = row.child(button_enabled(
                        (id, index),
                        self.t(key),
                        ButtonStyle::Ghost,
                        if enabled {
                            ButtonState::Enabled(false.into())
                        } else {
                            ButtonState::Disabled
                        },
                        theme.accent,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            let mut toolbar = this.toolbar.clone();
                            toolbar.move_item(item, forward);
                            this.apply_toolbar(toolbar, cx);
                        }),
                    ));
                }
                rows.push(row.into_any_element());
            }
        }
        rows.push(
            button(
                "toolbar-reset",
                self.t(Key::ToolbarReset),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| this.apply_toolbar(ToolbarPreferences::default(), cx)),
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, size};

    #[gpui::test]
    fn toolbar_settings_apply_toggle_order_and_reset_through_buttons(cx: &mut TestAppContext) {
        let (app, main_cx) = crate::harness::open(cx);
        app.update(main_cx, |app, cx| app.open_settings(cx));
        let handle = app.read_with(main_cx, |app, _| app.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), main_cx);
        handle
            .update(cx, |settings, _, cx| {
                settings.search = TextField::new("Top toolbar");
                cx.notify();
            })
            .unwrap();
        cx.simulate_resize(size(px(760.), px(1100.)));
        cx.run_until_parked();
        crate::harness::click("toolbar-visible-14", cx);
        crate::harness::click("toolbar-earlier-6", cx);
        app.read_with(cx, |app, _| {
            assert!(app.settings.toolbar.contains(ToolbarItem::Visualizer));
            assert_eq!(
                app.settings.toolbar.visible_in(1).collect::<Vec<_>>(),
                vec![ToolbarItem::Zoom, ToolbarItem::Grid]
            );
        });
        crate::harness::click("toolbar-reset", cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.settings.toolbar, ToolbarPreferences::default())
        });
    }
}
