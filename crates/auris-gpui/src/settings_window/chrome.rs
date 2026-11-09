//! Persistent title-bar menu choices, separate from the transport toolbar.

use super::*;
use crate::menu::MenuCategory;

impl SettingsWindow {
    fn toggle_titlebar_menu(
        &mut self,
        category: MenuCategory,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let mut appearance = self.appearance.clone();
        appearance.titlebar_menus.retain(|id| id != category.id());
        if enabled {
            appearance.titlebar_menus.push(category.id().to_owned());
        }
        self.apply_appearance_preferences(appearance, cx);
    }

    pub(super) fn render_titlebar_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        FormGroup::new(self.t(Key::TitlebarMenusHeading))
            .description(self.t(Key::TitlebarMenusNote))
            .when(cfg!(target_os = "macos"), |group| {
                group.child(note(self.t(Key::TitlebarMenusNative), &self.theme))
            })
            .children(MenuCategory::ALL.into_iter().map(|category| {
                let on = self
                    .appearance
                    .titlebar_menus
                    .iter()
                    .any(|id| id == category.id());
                FormField::new(self.t(category.label())).beside().child(
                    Switch::new(
                        gpui::SharedString::from(format!(
                            "titlebar-menu-visible-{}",
                            category.id()
                        )),
                        on,
                    )
                    .disabled(cfg!(target_os = "macos"))
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        this.toggle_titlebar_menu(category, *on, cx)
                    })),
                )
            }))
            .child(
                Button::new("titlebar-menus-reset", self.t(Key::TitlebarMenusReset))
                    .ghost()
                    .disabled(cfg!(target_os = "macos"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let mut appearance = this.appearance.clone();
                        appearance.titlebar_menus = Appearance::default().titlebar_menus;
                        this.apply_appearance_preferences(appearance, cx);
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, size};

    #[gpui::test]
    fn titlebar_menu_settings_save_selected_categories_and_reset_without_changing_the_theme(
        cx: &mut TestAppContext,
    ) {
        let _appearance = crate::harness::AppearanceTestGuard::new();
        let (app, main_cx) = crate::harness::open(cx);
        app.update(main_cx, |this, cx| {
            this.appearance.titlebar_menus = Appearance::default().titlebar_menus;
            this.open_settings(cx);
        });
        let before = app.read_with(main_cx, |this, _| this.appearance.clone());
        let handle = app.read_with(main_cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), main_cx);
        handle
            .update(cx, |settings, _, cx| {
                settings.search = TextField::new("Title bar menus");
                cx.notify();
            })
            .unwrap();
        cx.simulate_resize(size(px(980.), px(1100.)));
        cx.run_until_parked();
        if cfg!(target_os = "macos") {
            // Native menus remain available; exercise the shared persistence path on this host.
            handle
                .update(cx, |settings, _, cx| {
                    settings.toggle_titlebar_menu(MenuCategory::File, false, cx);
                    settings.toggle_titlebar_menu(MenuCategory::Track, true, cx);
                })
                .unwrap();
        } else {
            crate::harness::click("titlebar-menu-visible-file", cx);
            crate::harness::click("titlebar-menu-visible-track", cx);
        }
        cx.run_until_parked();
        let saved = Appearance::load();
        assert!(!saved.titlebar_menus.iter().any(|id| id == "file"));
        assert!(saved.titlebar_menus.iter().any(|id| id == "track"));
        assert_eq!(saved.scheme, before.scheme);
        assert_eq!(saved.font_family, before.font_family);
        app.read_with(cx, |this, _| {
            assert_eq!(this.appearance.titlebar_menus, saved.titlebar_menus)
        });
        if !cfg!(target_os = "macos") {
            crate::harness::click("titlebar-menus-reset", cx);
            cx.run_until_parked();
            assert_eq!(
                Appearance::load().titlebar_menus,
                Appearance::default().titlebar_menus
            );
        }
    }
}
