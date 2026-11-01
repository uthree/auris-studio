//! The window's one bar: project navigation, history and menus, and — in a window wide enough
//! to hold it — the transport as well. A narrower window keeps the transport in a row below.

use auris_i18n::Key;
use gpui::{AnyElement, Context, IntoElement, Pixels, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::titlebar;
use crate::ui::icons::Icon;
use crate::ui::widgets::icon_button;

impl AurisApp {
    /// Draws project identity, the application menu, native window controls and, when the
    /// window is wide enough, the transport.
    pub(crate) fn render_title_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let transport = super::transport_bar::transport_in_title_bar(
            window.viewport_size().width,
            self.appearance.top_bar_layout,
        )
        .then(|| self.transport_parts(window, true, cx));
        let theme = &self.theme;
        let controls = titlebar::controls(
            window,
            theme,
            cx.listener(|this, _, window, cx| {
                this.close_project_window(window, cx);
                cx.notify();
            }),
        );

        let mut parts = titlebar::Parts::new("project-title", self.document_title()).leading(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(self.render_application_menu_button(cx))
                .child(
                    atelier::Button::icon_only("editor-start", atelier::IconName::ChevronLeft)
                        .ghost()
                        .size(atelier::ControlSize::Small)
                        .tooltip(self.t(Key::BackToStart))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_editor_start(window, cx)),
                        ),
                )
                .child(self.render_history_buttons(cx))
                .children(self.render_titlebar_menus(window, cx)),
        );
        if let Some(transport) = transport {
            // The playback strip leads, after the navigation, so play sits where it sits in the
            // row the narrower window gives it; the grid, zoom and meters close the far end.
            parts = parts.leading(transport.primary).trailing(
                div()
                    .debug_selector(|| "titlebar-transport-trailing".into())
                    .flex()
                    .items_center()
                    .gap_3()
                    .children(transport.editing)
                    .children(transport.meters),
            );
        }
        titlebar::bar(window, theme, parts, controls)
    }

    /// A single entry point to all application commands on Windows and Linux.
    pub(crate) fn render_application_menu_button(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div().when(Self::wants_menu_bar(), |slot| {
            slot.child(
                icon_button(
                    "application-menu",
                    Icon::Menu,
                    self.menu_bar.is_some_and(|menu| !menu.from_titlebar),
                    self.theme.accent,
                    &self.theme,
                    cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::OpenMenuBar), cx);
                    }),
                )
                .tooltip(self.tip(Key::CmdOpenMenuBar, "view.menu_bar")),
            )
        })
    }
}

impl AurisApp {
    /// Measured menu shortcuts that fit beside navigation, title and native window controls.
    /// Remaining categories stay available in the application menu.
    pub(crate) fn titlebar_menu_layout(
        &self,
        window: &Window,
    ) -> Vec<(usize, gpui::SharedString, Pixels)> {
        if !Self::wants_menu_bar() {
            return Vec::new();
        }
        let spacing = self.theme.atelier.spacing;
        let reserved = titlebar::controls_width(window)
            + spacing.md * 2.0
            + self.theme.atelier.density.control_sm * 4.0
            + spacing.sm * 2.0
            + spacing.xxl * 4.0
            + if super::transport_bar::transport_in_title_bar(
                window.viewport_size().width,
                self.appearance.top_bar_layout,
            ) {
                super::transport_bar::TITLE_BAR_TRANSPORT_WIDTH
            } else {
                px(0.0)
            };
        let mut available = (window.viewport_size().width - reserved).max(px(0.0));
        let mut visible = Vec::new();
        for (index, section) in self.menu_model().into_iter().enumerate() {
            if !section.category.is_some_and(|category| {
                self.appearance
                    .titlebar_menus
                    .iter()
                    .any(|id| id == category.id())
            }) {
                continue;
            }
            let width = super::paint::measure_label(
                window,
                section.name.clone(),
                self.theme.atelier.typography.size_sm,
            ) + spacing.sm * 2.0;
            if width + spacing.xs > available {
                break;
            }
            available -= width + spacing.xs;
            visible.push((index, section.name, width));
        }
        visible
    }

    fn render_titlebar_menus(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let layout = self.titlebar_menu_layout(window);
        self.menu_title_bounds
            .borrow_mut()
            .retain(|index, _| layout.iter().any(|(shown, _, _)| index == shown));
        layout
            .into_iter()
            .map(|(index, label, width)| {
                let bounds = self.menu_title_bounds.clone();
                let open = self
                    .menu_bar
                    .is_some_and(|menu| menu.from_titlebar && menu.index == index);
                div()
                    .relative()
                    .w(width)
                    .flex_shrink_0()
                    .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                        if let Some(menu) = this.menu_bar
                            && (menu.index != index || !menu.from_titlebar)
                        {
                            this.set_menu_bar(Some(super::menu_bar::OpenMenu {
                                index,
                                from_titlebar: true,
                                clicked: false,
                                highlighted: None,
                            }));
                            cx.notify();
                        }
                    }))
                    .child(
                        atelier::Button::new(("titlebar-menu", index), label)
                            .ghost()
                            .size(atelier::ControlSize::Small)
                            .radius(self.theme.atelier.shape.radius_sm)
                            .full_width()
                            .selected(open)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let close = this.menu_bar.is_some_and(|menu| {
                                    menu.index == index && menu.from_titlebar && menu.clicked
                                });
                                this.menu = None;
                                if !close && this.menu_focus_return.is_none() {
                                    this.menu_focus_return = window.focused(cx);
                                }
                                this.set_menu_bar((!close).then_some(super::menu_bar::OpenMenu {
                                    from_titlebar: true,
                                    ..super::menu_bar::OpenMenu::at(index)
                                }));
                                if !close {
                                    window.focus(&this.menu_focus);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        gpui::canvas(
                            move |painted, _, _| {
                                bounds.borrow_mut().insert(index, painted);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// Undo and Redo in the title bar, beside project navigation.
    pub(crate) fn render_history_buttons(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = &self.theme;
        let step = |id: &'static str, glyph: Icon, enabled: bool, label, command| {
            let shortcut = self.keystroke_for(command);
            atelier::Button::icon_only(id, glyph.source())
                .ghost()
                .size(atelier::ControlSize::Small)
                .disabled(!enabled)
                .tooltip(format!("{} · {shortcut}", self.t(label)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if glyph == Icon::Undo {
                        this.undo();
                    } else {
                        this.redo();
                    }
                    cx.notify();
                }))
        };
        div()
            .debug_selector(|| "editing-history".into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(theme.atelier.spacing.xxs)
            .child(step(
                "editing-undo",
                Icon::Undo,
                self.session.can_undo(),
                Key::CmdUndo,
                "edit.undo",
            ))
            .child(step(
                "editing-redo",
                Icon::Redo,
                self.session.can_redo(),
                Key::CmdRedo,
                "edit.redo",
            ))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, px, size};

    use crate::dock::Panel;
    use crate::harness::{click, open, paint, resize, with_a_clip};

    #[gpui::test]
    fn start_button_keeps_unsaved_work_and_escape_resumes_it(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        let before = app.read_with(cx, |this, _| this.project().clone());
        click("editor-start", cx);
        paint(&app, cx);
        assert!(cx.debug_bounds("project-chooser").is_some());
        app.read_with(cx, |this, _| {
            assert_eq!(this.project(), &before);
            assert!(this.project().midi_clip(clip).is_some());
            assert!(this.session.is_dirty());
        });
        cx.simulate_keystrokes("escape");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.project_chooser.is_none());
            assert_eq!(this.project(), &before);
        });
        click("editor-start", cx);
        paint(&app, cx);
        click("chooser-close", cx);
        app.read_with(cx, |this, _| assert!(this.project_chooser.is_none()));
    }

    #[gpui::test]
    fn creating_from_the_returned_start_screen_asks_before_replacing_unsaved_work(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _, _) = with_a_clip(cx);
        let before = app.read_with(cx, |this, _| this.project().clone());
        click("editor-start", cx);
        paint(&app, cx);
        click("chooser-confirm", cx);
        paint(&app, cx);
        assert!(cx.debug_bounds("prompt-cancel").is_some());
        click("prompt-cancel", cx);
        app.read_with(cx, |this, _| {
            assert!(this.project_chooser.is_none());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn the_compact_transport_joins_the_titlebar_only_where_it_fits(cx: &mut TestAppContext) {
        use crate::appearance::TopBarLayout;
        use crate::ui::transport_bar::ONE_BAR_FROM;
        let (app, cx) = open(cx);
        // The spacious layout, the default: however wide the window, the transport keeps a row
        // of its own under the title bar. Set rather than read, since other window tests save
        // their appearance to the configuration directory this one shares.
        assert_eq!(
            crate::appearance::Appearance::default().top_bar_layout,
            TopBarLayout::Spacious
        );
        app.update(cx, |this, _| {
            this.appearance.top_bar_layout = TopBarLayout::Spacious
        });
        for width in [2560.0, 1440.0] {
            resize(&app, cx, size(px(width), px(700.0)));
            let transport = cx.debug_bounds("transport-buttons").unwrap();
            assert!(
                transport.top() >= crate::titlebar::HEIGHT,
                "{width} {transport:?}"
            );
            let workspace = app.read_with(cx, |this, _| this.chrome_height());
            assert_eq!(
                workspace,
                crate::titlebar::HEIGHT
                    + crate::theme::Metrics::TRANSPORT_HEIGHT
                    + crate::theme::Metrics::STATUS_HEIGHT
            );
        }
        app.update(cx, |this, _| {
            this.appearance.top_bar_layout = TopBarLayout::Compact
        });
        app.read_with(cx, |this, _| {
            assert_eq!(this.document_title(), "Untitled");
            assert!(this.window_title().contains("Auris Studio"));
        });
        // Every menu category pinned, so the menu titles want as much of the bar as they can
        // get, in the language with the longer captions.
        app.update(cx, |this, _| {
            this.language = auris_i18n::Language::Japanese;
            this.appearance.titlebar_menus = crate::menu::MenuCategory::ALL
                .iter()
                .map(|category| category.id().to_owned())
                .collect()
        });
        for width in [2560.0, 1440.0, f32::from(ONE_BAR_FROM), 1100.0, 640.0] {
            resize(&app, cx, size(px(width), px(700.0)));
            let one_bar = px(width) >= ONE_BAR_FROM;
            let title = cx.debug_bounds("project-title").unwrap();
            // Native title bars keep the title centred until side controls need the room; once
            // they do, Atelier shifts it away rather than letting it overlap a control.
            assert!(title.left() >= px(0.0) && title.right() <= px(width));
            let history = cx.debug_bounds("editing-history").unwrap();
            assert!(history.top() >= px(0.0) && history.bottom() <= crate::titlebar::HEIGHT);
            let transport = cx.debug_bounds("transport-buttons").unwrap();
            let panels = cx.debug_bounds("panel-switch-5").unwrap();
            assert!(panels.top() > transport.bottom());
            assert!(panels.right() <= px(width));
            if one_bar {
                // One bar, and nothing on it covering anything else: the navigation, the
                // playback strip, the title and the grid-to-meters run each in their own span.
                let primary = cx.debug_bounds("transport-primary").unwrap();
                let trailing = cx.debug_bounds("titlebar-transport-trailing").unwrap();
                assert!(
                    primary.bottom() <= crate::titlebar::HEIGHT,
                    "{width} {title:?} {history:?} {primary:?}"
                );
                assert!(
                    history.right() <= primary.left(),
                    "{width} {title:?} {history:?} {primary:?}"
                );
                assert!(
                    primary.right() <= title.left(),
                    "{width} {title:?} {history:?} {primary:?}"
                );
                assert!(
                    title.right() <= trailing.left(),
                    "{width} {title:?} {trailing:?} {primary:?}"
                );
                assert!(trailing.right() <= px(width), "{width} {trailing:?}");
                // The application menu is drawn only where there is no system menu bar.
                if crate::app::AurisApp::wants_menu_bar() {
                    assert!(cx.debug_bounds("application-menu").unwrap().left() >= px(0.0));
                }
                // The arrangement starts right under it.
                let workspace = app.read_with(cx, |this, _| this.chrome_height());
                assert_eq!(
                    workspace,
                    crate::titlebar::HEIGHT + crate::theme::Metrics::STATUS_HEIGHT
                );
            } else {
                assert!(transport.top() >= crate::titlebar::HEIGHT);
            }
            let looping = app.read_with(cx, |this, _| this.project().loop_enabled);
            click("loop", cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.project().loop_enabled, !looping)
            });

            let open = app.read_with(cx, |this, _| this.panels.is_open(Panel::Mixer));
            click("panel-switch-2", cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.panels.is_open(Panel::Mixer), !open)
            });
            paint(&app, cx);
        }
    }

    #[gpui::test]
    fn pinned_menu_categories_open_locally_and_keyboard_navigation_stays_with_them(
        cx: &mut TestAppContext,
    ) {
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.appearance.titlebar_menus = crate::appearance::Appearance::default().titlebar_menus
        });
        // Narrow enough that the transport keeps its own row, so the menu titles have the bar's
        // room to themselves; where the transport joins the bar they give way to it instead.
        resize(&app, cx, size(px(1100.), px(700.)));
        for index in [0, 1, 4] {
            let selector: &'static str =
                Box::leak(format!("titlebar-menu-{index}").into_boxed_str());
            let button = cx.debug_bounds(selector).unwrap();
            assert!(button.bottom() <= crate::titlebar::HEIGHT);
        }
        click("titlebar-menu-0", cx);
        paint(&app, cx);
        let popup = cx.debug_bounds("titlebar-menu-panel").unwrap();
        assert!(popup.top() >= crate::titlebar::HEIGHT);
        assert!(
            (popup.left() - cx.debug_bounds("titlebar-menu-0").unwrap().left()).abs() <= px(1.)
        );
        for index in [1, 4, 0] {
            cx.simulate_keystrokes("right");
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                let menu = this.menu_bar.unwrap();
                assert_eq!(menu.index, index);
                assert!(menu.from_titlebar);
            });
        }
        let return_focus = app.read_with(cx, |this, _| this.menu_focus_return.clone().unwrap());
        cx.simulate_keystrokes("escape");
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.menu_bar.is_none()));
        cx.update(|window, _| assert!(return_focus.is_focused(window)));
        click("titlebar-menu-4", cx);
        cx.simulate_keystrokes("down enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.palette.is_some()));
    }

    #[gpui::test]
    fn all_pinned_categories_yield_to_the_title_and_keep_the_full_menu_accessible(
        cx: &mut TestAppContext,
    ) {
        if !crate::app::AurisApp::wants_menu_bar() {
            return;
        }
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.appearance.titlebar_menus = crate::menu::MenuCategory::ALL
                .iter()
                .map(|category| category.id().to_owned())
                .collect()
        });
        for width in [1360., 640., 480.] {
            resize(&app, cx, size(px(width), px(700.)));
            let shown = cx.update(|window, cx| {
                app.read_with(cx, |this, _| this.titlebar_menu_layout(window))
            });
            let title = cx.debug_bounds("project-title").unwrap();
            for (index, _, _) in shown {
                let selector: &'static str =
                    Box::leak(format!("titlebar-menu-{index}").into_boxed_str());
                let button = cx.debug_bounds(selector).unwrap();
                assert!(button.right() <= title.left());
            }
            assert!(cx.debug_bounds("editing-redo").unwrap().right() <= title.left());
            click("application-menu", cx);
            paint(&app, cx);
            assert!(cx.debug_bounds("application-menu-panel").is_some());
            cx.simulate_keystrokes("escape");
        }
    }

    #[gpui::test]
    fn closing_from_the_titlebar_keeps_unsaved_work_until_answered(cx: &mut TestAppContext) {
        if cfg!(target_os = "macos") {
            return; // macOS draws its own traffic lights outside the headless view tree.
        }
        let (app, cx, _, clip) = with_a_clip(cx);
        click("window-close", cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_some());
            assert!(this.session.midi_clip(clip).is_some());
            assert!(this.session.is_dirty());
        });
        click("prompt-cancel", cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert!(this.session.midi_clip(clip).is_some());
        });
    }
}
