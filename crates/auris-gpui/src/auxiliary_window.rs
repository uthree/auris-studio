//! Native windows sharing the main document and its command/gesture handlers.

use std::collections::BTreeMap;

use auris_i18n::Key;
use gpui::{
    App, Bounds, Context, IntoElement, Render, WeakEntity, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};

use crate::{app::AurisApp, dock::Panel, titlebar};

/// A single presentation surface, with at most one native window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Surface {
    Panel(Panel),
    Plugin,
    Typing,
    Analysis,
    TimbreMap,
    Visualizer,
}

/// Toggles a panel between its dock and a native window.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = auris, no_json)]
pub(crate) struct TogglePanelWindow {
    pub panel: Panel,
}

impl Surface {
    fn wanted(self, app: &AurisApp) -> bool {
        match self {
            Self::Panel(panel) => app.panels.is_detached(panel) && app.panels.is_open(panel),
            Self::Plugin => app.plugin_window.is_some(),
            Self::Typing => app.session.musical_typing(),
            Self::Analysis => app.analysis_panel,
            Self::TimbreMap => app.timbre_map.open,
            Self::Visualizer => app.visualizer.open,
        }
    }

    fn title(self, app: &AurisApp) -> String {
        match self {
            Self::Panel(panel) => app.t(panel.label()).to_owned(),
            Self::Plugin => app
                .plugin_window
                .and_then(|editor| {
                    app.resolve_plugin(editor.subject)
                        .map(|(id, _)| match editor.subject {
                            crate::ui::plugin_window::PluginSubject::Instrument(track) => {
                                app.instrument_label(track, &id)
                            }
                            crate::ui::plugin_window::PluginSubject::Insert { slot, .. } => {
                                app.effect_label(slot, &id)
                            }
                        })
                })
                .unwrap_or_default(),
            Self::Typing => app.t(Key::CmdMusicalTyping).to_owned(),
            Self::Analysis => app.t(Key::AnalysisTitle).to_owned(),
            Self::TimbreMap => app.t(Key::TimbreMap).to_owned(),
            Self::Visualizer => app.t(Key::VisualizerTitle).to_owned(),
        }
    }

    fn close(self, app: &mut AurisApp) {
        match self {
            Self::Panel(panel) => {
                app.panels.hide(panel);
                app.remember_layout();
            }
            Self::Plugin => {
                app.close_plugin_window();
            }
            Self::Typing => app.stop_musical_typing(),
            Self::Analysis => app.analysis_panel = false,
            Self::TimbreMap => app.close_timbre_map(),
            Self::Visualizer => app.close_visualizer(),
        }
    }
}

/// A view of the existing document, never a second session.
pub(crate) struct AuxiliaryWindow {
    app: WeakEntity<AurisApp>,
    surface: Surface,
    was_active: bool,
}

/// GPUI draws a new window synchronously. Open it outside the document's entity update.
fn open_surface(app: WeakEntity<AurisApp>, surface: Surface, cx: &mut App) {
    let Some(main) = app.upgrade() else {
        return;
    };
    if !surface.wanted(main.read(cx)) {
        main.update(cx, |app, _| {
            app.pending_auxiliary.remove(&surface);
        });
        return;
    }
    let title = surface.title(main.read(cx));
    // Native window bounds use screen pixels, independently of content spacing.
    let dimensions = match surface {
        Surface::Panel(Panel::PianoRoll | Panel::Mixer) => size(px(1000.), px(540.)),
        Surface::Panel(_) => size(px(560.), px(640.)),
        Surface::Plugin => size(px(500.), px(650.)),
        Surface::Typing => size(px(680.), px(340.)),
        Surface::Analysis => size(px(540.), px(650.)),
        Surface::TimbreMap => size(px(900.), px(700.)),
        Surface::Visualizer => {
            let preferred = size(px(780.), px(820.));
            cx.primary_display().map_or(preferred, |display| {
                crate::fitted_size(preferred, display.bounds().size)
            })
        }
    };
    let bounds = Bounds::centered(None, dimensions, cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(titlebar::options(title)),
            window_min_size: Some(match surface {
                Surface::Typing => size(px(640.), px(340.)),
                _ => size(px(320.), px(240.)),
            }),
            focus: true,
            ..Default::default()
        },
        |window, cx| {
            // Register before the first draw so focus can resolve this window's panel.
            if let Some(handle) = window.window_handle().downcast::<AuxiliaryWindow>() {
                main.update(cx, |app, _| {
                    app.auxiliary_windows.insert(surface, handle);
                });
            }
            let closing_app = app.clone();
            window.on_window_should_close(cx, move |window, cx| {
                closing_app
                    .update(cx, |app, cx| {
                        if app.compose_progress.is_some() {
                            return false;
                        }
                        app.end_drag(window, cx);
                        app.session.release_typed_notes();
                        surface.close(app);
                        cx.notify();
                        true
                    })
                    .unwrap_or(true)
            });
            let handle = window.window_handle();
            cx.new(|cx| {
                cx.observe(&main, |_, _, cx| cx.notify()).detach();
                cx.observe_release(&main, move |_, _, cx| {
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    });
                })
                .detach();
                AuxiliaryWindow {
                    app,
                    surface,
                    was_active: false,
                }
            })
        },
    );
    main.update(cx, |app, cx| {
        app.pending_auxiliary.remove(&surface);
        match opened {
            Ok(handle) => {
                app.auxiliary_windows.insert(surface, handle);
            }
            Err(error) => {
                if let Surface::Panel(panel) = surface {
                    app.panels.set_detached(panel, false);
                    app.remember_layout();
                } else {
                    surface.close(app);
                }
                app.set_failed_status(error.to_string());
            }
        }
        cx.notify();
    });
}

impl AurisApp {
    /// Focus handles must refer to a pane actually rendered in this window.
    pub(crate) fn local_pane(
        &self,
        requested: crate::app::Pane,
        window: &Window,
    ) -> Option<crate::app::Pane> {
        let current = self.auxiliary_windows.iter().find_map(|(surface, handle)| {
            (handle.window_id() == window.window_handle().window_id()).then_some(*surface)
        });
        match current {
            Some(Surface::Panel(panel)) => Some(panel.pane()),
            Some(_) => None,
            None => Some(
                if Panel::ALL
                    .into_iter()
                    .any(|panel| panel.pane() == requested && self.panels.is_detached(panel))
                {
                    crate::app::Pane::Arrangement
                } else {
                    requested
                },
            ),
        }
    }

    /// Binds document input to the native window that received it.
    pub(crate) fn window_listener<E: 'static>(
        cx: &Context<Self>,
        handler: impl Fn(&mut Self, &E, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        cx.listener(move |app, event, window, cx| {
            app.claim_event_window(window);
            handler(app, event, window, cx);
        })
    }

    pub(crate) fn claim_event_window(&mut self, window: &Window) {
        let id = window.window_handle().window_id();
        if self.event_window != Some(id)
            && self.prompt.is_none()
            && self.palette.is_none()
            && self.song_sheet.is_none()
            && !self.reference_match.open
        {
            self.close_menu();
            self.close_menu_bar();
            self.event_window = Some(id);
        }
    }

    /// Materializes requested surfaces and closes windows whose surface was hidden.
    pub(crate) fn sync_auxiliary_windows(&mut self, cx: &mut Context<Self>) {
        let surfaces = Panel::ALL.into_iter().map(Surface::Panel).chain([
            Surface::Plugin,
            Surface::Typing,
            Surface::Analysis,
            Surface::TimbreMap,
            Surface::Visualizer,
        ]);
        let mut handles = std::mem::take(&mut self.auxiliary_windows);
        let mut kept = BTreeMap::new();
        for surface in surfaces {
            if let Some(handle) = handles.remove(&surface) {
                if surface.wanted(self) {
                    // Reading the handle detects an OS window closed between frames.
                    if handle.read(cx).is_ok() {
                        kept.insert(surface, handle);
                        continue;
                    }
                } else {
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    });
                }
                if self.event_window == Some(handle.window_id()) {
                    self.event_window = None;
                }
            }
            if !surface.wanted(self) {
                continue;
            }
            if self.pending_auxiliary.insert(surface) {
                let app = cx.entity().downgrade();
                cx.defer(move |cx| open_surface(app, surface, cx));
            }
        }
        self.auxiliary_windows = kept;
        if let Some(surface) = self.raise_auxiliary.take()
            && let Some(handle) = self.auxiliary_windows.get(&surface)
        {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }

    pub(crate) fn toggle_panel_window(&mut self, panel: Panel) {
        self.panels
            .set_detached(panel, !self.panels.is_detached(panel));
        self.remember_layout();
    }

    /// Dismisses only the utility window receiving Escape.
    pub(crate) fn close_current_utility(&mut self, window: &Window) -> bool {
        let current = self.auxiliary_windows.iter().find_map(|(surface, handle)| {
            (handle.window_id() == window.window_handle().window_id()).then_some(*surface)
        });
        if let Some(
            surface @ (Surface::Plugin
            | Surface::Typing
            | Surface::Analysis
            | Surface::TimbreMap
            | Surface::Visualizer),
        ) = current
        {
            surface.close(self);
            true
        } else {
            false
        }
    }
}

impl Render for AuxiliaryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self.surface;
        let lost_focus = self.was_active && !window.is_window_active();
        self.was_active = window.is_window_active();
        let result = self.app.update(cx, |app, cx| {
            if lost_focus {
                app.session.release_typed_notes();
                app.end_drag(window, cx);
            }
            if !surface.wanted(app) {
                return div().into_any_element();
            }
            app.reconcile_focus(window);
            if window.focused(cx).is_none() {
                match surface {
                    Surface::Panel(panel) => app.focus_pane(panel.pane(), window),
                    _ => window.focus(&app.focus),
                }
            }
            let theme = app.theme.clone();
            let title = surface.title(app);
            window.set_window_title(&title);
            let chrome = titlebar::titlebar(window, &theme)
                .child(
                    titlebar::drag_region("utility-title")
                        .flex_1()
                        .px_3()
                        .child(div().min_w_0().truncate().text_xs().child(title)),
                )
                .child(titlebar::controls(
                    window,
                    &theme,
                    cx.listener(move |app, _, window, cx| {
                        if app.compose_progress.is_some() {
                            return;
                        }
                        app.end_drag(window, cx);
                        surface.close(app);
                        window.remove_window();
                        cx.notify();
                    }),
                ));
            let content = match surface {
                Surface::Panel(panel) => {
                    let content = app.render_panel(panel, window, cx);
                    app.pane(panel.pane(), window, cx)
                        .flex()
                        .size_full()
                        .min_w_0()
                        .min_h_0()
                        .child(content)
                        .into_any_element()
                }
                Surface::Plugin => app
                    .render_plugin_window(cx)
                    .unwrap_or_else(|| div().into_any_element()),
                Surface::Typing => app
                    .render_typing_panel(cx)
                    .unwrap_or_else(|| div().into_any_element()),
                Surface::Analysis => app
                    .render_analysis_panel(cx)
                    .unwrap_or_else(|| div().into_any_element()),
                Surface::Visualizer => app.render_visualizer(window, cx),
                Surface::TimbreMap => app
                    .render_timbre_map(cx)
                    .unwrap_or_else(|| div().into_any_element()),
            };
            let overlays = app.render_document_overlays(window, cx);
            if !surface.wanted(app) {
                cx.notify();
            }
            let menu_bar = app.render_menu_bar(window, cx);
            app.input_root(cx)
                .id("auxiliary-root")
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .key_context(app.window_context())
                .track_focus(&app.focus)
                .bg(theme.background)
                .text_color(theme.text)
                .font(theme.font.clone())
                .text_sm()
                .child(chrome)
                .children(menu_bar)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .child(content),
                )
                .children(overlays)
                .into_any_element()
        });
        result.unwrap_or_else(|_| {
            let handle = window.window_handle();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
            div().into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::plugin_window::PluginSubject;
    use crate::{
        dock::{Dock, PanelLayout},
        harness,
    };

    struct RestoreLayout(PanelLayout);

    impl Drop for RestoreLayout {
        fn drop(&mut self) {
            let _ = self.0.save();
        }
    }

    #[gpui::test]
    fn removing_the_plugin_subject_closes_its_native_editor(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        let track = app.update(cx, |app, cx| {
            let track = app
                .session
                .add_default_instrument_track("Temporary")
                .unwrap();
            app.open_plugin_window(PluginSubject::Instrument(track));
            cx.notify();
            track
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
        app.update(cx, |app, cx| {
            app.session.remove_track(track).unwrap();
            cx.notify();
        });
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| assert!(app.plugin_window.is_none()));
        assert!(handle.read_with(cx, |_, _| ()).is_err());
    }

    #[gpui::test]
    fn auxiliary_windows_do_not_keep_a_closed_document_alive(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(crate::actions::OpenAnalysisResults);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Analysis]);
        let weak = app.downgrade();
        cx.update(|window, _| window.remove_window());
        cx.cx.update(|_| drop(app));
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        assert!(handle.read_with(cx, |_, _| ()).is_err());
    }

    #[gpui::test]
    fn detached_piano_roll_keeps_gestures_and_scoped_delete(cx: &mut TestAppContext) {
        use auris_session::prelude::{Note, Ticks};
        let (app, cx, _, clip) = harness::with_a_clip(cx);
        let _restore = RestoreLayout(PanelLayout::load());
        let before = app.update(cx, |app, _| {
            app.panels = PanelLayout::default();
            app.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            app.open_clip_in_editor(clip);
            app.project().clone()
        });
        cx.dispatch_action(TogglePanelWindow {
            panel: Panel::PianoRoll,
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| {
            app.auxiliary_windows[&Surface::Panel(Panel::PianoRoll)]
        });
        let mut utility = VisualTestContext::from_window(handle.into(), cx);
        harness::show_pitch(&app, &mut utility, 60);
        let from = harness::roll_point(&app, &mut utility, Ticks(Ticks::QUARTER.0 / 2), 60);
        let to = harness::roll_point(&app, &mut utility, Ticks::QUARTER * 2, 62);
        harness::drag(&mut utility, from, to);
        app.read_with(&utility, |app, _| {
            assert_eq!(app.session.midi_clip(clip).unwrap().notes[0].pitch, 62)
        });
        utility.dispatch_action(crate::actions::Undo);
        app.read_with(&utility, |app, _| assert_eq!(app.project(), &before));
        app.update(&mut utility, |app, _| {
            app.selected_notes.insert(0);
        });
        utility.simulate_keystrokes("backspace");
        app.read_with(&utility, |app, _| {
            assert!(app.session.midi_clip(clip).unwrap().notes.is_empty())
        });
    }

    #[gpui::test]
    fn typing_window_receives_keys_and_closing_it_releases_notes(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, _| {
            let track = app.session.add_default_instrument_track("Typing").unwrap();
            app.select_track(track);
        });
        cx.dispatch_action(crate::actions::ToggleMusicalTyping);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Typing]);
        let mut utility = VisualTestContext::from_window(handle.into(), cx);
        utility.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
            is_held: false,
        });
        app.read_with(&utility, |app, _| {
            assert_eq!(app.session.typing_keyboard().sounding().count(), 1)
        });
        harness::click("tk-close", &mut utility);
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(!app.session.musical_typing());
            assert_eq!(app.session.typing_keyboard().sounding().count(), 0);
        });
    }
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};

    #[gpui::test]
    fn all_panels_open_once_and_return_to_their_docks(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        let _restore = RestoreLayout(PanelLayout::load());
        app.update(cx, |app, _| app.panels = PanelLayout::default());
        for panel in Panel::ALL {
            cx.dispatch_action(TogglePanelWindow { panel });
        }
        harness::paint(&app, cx);
        let ids = app.read_with(cx, |app, _| {
            assert_eq!(app.auxiliary_windows.len(), Panel::ALL.len());
            assert!(
                Dock::ALL
                    .into_iter()
                    .all(|dock| app.panels.showing(dock).is_none())
            );
            app.auxiliary_windows
                .values()
                .map(|handle| handle.window_id())
                .collect::<Vec<_>>()
        });
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                ids,
                app.auxiliary_windows
                    .values()
                    .map(|handle| handle.window_id())
                    .collect::<Vec<_>>()
            )
        });
        for panel in Panel::ALL {
            cx.dispatch_action(TogglePanelWindow { panel });
        }
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.auxiliary_windows.is_empty());
            assert!(
                Panel::ALL
                    .into_iter()
                    .all(|panel| !app.panels.is_detached(panel))
            );
        });
    }

    #[gpui::test]
    fn plugin_slider_edits_the_shared_document_and_undoes_from_its_window(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        let (target, descriptor, before) = app.update(cx, |app, cx| {
            let track = app
                .session
                .add_default_instrument_track("Window test")
                .unwrap();
            let subject = PluginSubject::Instrument(track);
            let descriptor = app
                .session
                .instrument_descriptors(track)
                .iter()
                .find(|descriptor| {
                    matches!(
                        crate::ui::plugin_editor::control_for(descriptor),
                        crate::ui::plugin_editor::ParamControl::Slider
                    )
                })
                .unwrap()
                .clone();
            let target = subject.param_target(descriptor.id);
            let before = app.session.param_value(target, &descriptor);
            app.open_plugin_window(subject);
            cx.notify();
            (target, descriptor, before)
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
        let mut utility = VisualTestContext::from_window(handle.into(), cx);
        let selector = Box::leak(format!("pw-inst-param-{}", descriptor.id.0).into_boxed_str());
        let bounds = utility
            .debug_bounds(selector)
            .expect("native editor contains the slider");
        harness::drag(
            &mut utility,
            bounds.center(),
            bounds.center() + point(px(60.), px(0.)),
        );
        app.read_with(&utility, |app, _| {
            assert!(!app.dragging());
            assert_ne!(app.session.param_value(target, &descriptor), before);
        });
        utility.dispatch_action(crate::actions::Undo);
        app.read_with(&utility, |app, _| {
            assert_eq!(app.session.param_value(target, &descriptor), before)
        });
        utility.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::none());
        utility.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::none());
        harness::paint(&app, cx);
        assert!(utility.debug_bounds("context-menu-panel").is_some());
        assert!(cx.debug_bounds("context-menu-panel").is_none());
        utility.simulate_keystrokes("escape");
        harness::click("pw-close", &mut utility);
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.plugin_window.is_none());
            assert!(!app.auxiliary_windows.contains_key(&Surface::Plugin));
        });
    }

    #[gpui::test]
    fn hidden_detached_panel_reopens_in_a_native_window(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        let _restore = RestoreLayout(PanelLayout::load());
        app.update(cx, |app, _| app.panels = PanelLayout::default());
        cx.dispatch_action(TogglePanelWindow {
            panel: Panel::Mixer,
        });
        harness::paint(&app, cx);
        cx.dispatch_action(crate::actions::ToggleMixer);
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.panels.is_detached(Panel::Mixer));
            assert!(
                !app.auxiliary_windows
                    .contains_key(&Surface::Panel(Panel::Mixer))
            );
        });
        cx.dispatch_action(crate::actions::ToggleMixer);
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(
                app.auxiliary_windows
                    .contains_key(&Surface::Panel(Panel::Mixer))
            )
        });
    }
}
