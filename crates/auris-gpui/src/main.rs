//! Auris Studio — a digital audio workstation built with Rust and gpui.

#![warn(missing_docs)]

mod actions;
mod app;
mod appearance;
mod gestures;
mod i18n;
mod keymap;
mod menu;
mod settings_window;
mod theme;
mod ui;

use auris_session::Settings;
use gpui::{
    App, AppContext, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions,
    px, size,
};

use app::AurisApp;
use menu::menus;

fn main() {
    // Warnings matter here — a missing audio file or a plugin the registry does not know is
    // logged rather than shown — so surface them by default instead of requiring RUST_LOG.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Before anything reads a preference: the configuration moved to `~/.config/auris-studio`,
    // and an installation that predates the move keeps its settings, keymap and colour scheme.
    auris_session::migrate_legacy_config();

    Application::new().run(|cx: &mut App| {
        cx.on_action(|_: &actions::Quit, cx: &mut App| cx.quit());
        // The menu bar is built before the window, so the language comes from the settings file
        // rather than from the view that has not been created yet. `AurisApp` loads the same
        // file a moment later, and rebuilds these menus whenever the choice changes.
        cx.set_menus(menus(Settings::load().language()));

        let bounds = Bounds::centered(None, size(px(1500.), px(940.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Auris Studio".into()),
                        ..Default::default()
                    }),
                    focus: true,
                    ..Default::default()
                },
                |_, cx| cx.new(AurisApp::new),
            )
            .expect("could not open the main window");

        // The key bindings are dispatched to the focused view, so focus the app up front —
        // otherwise the space bar would not start playback until something was clicked.
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
            })
            .ok();

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
    });
}
