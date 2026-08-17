//! Auris Studio — a digital audio workstation built with Rust and gpui.

#![warn(missing_docs)]
// A release build is a window and nothing else. Windows gives a console-subsystem binary a
// console whether it wants one or not, so double-clicking `auris-studio.exe` opened a black
// terminal beside the application and keeping that terminal open was the only way to keep the
// application running — which is not what a DAW looks like.
//
// The debug build keeps its console, because `cargo run` and `RUST_LOG=debug` are how this is
// worked on. What replaces it for everybody else is the log panel: the records are kept whether
// or not there is anywhere to print them. The attribute is ignored on every other platform.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod appearance;
mod dock;
mod gestures;
mod i18n;
mod keymap;
mod logbook;
mod menu;
mod settings_window;
mod theme;

mod ui;

use auris_session::Settings;
use gpui::{
    App, AppContext, Application, Bounds, Pixels, TitlebarOptions, WindowBounds, WindowOptions, px,
    size,
};

use app::AurisApp;
use menu::menus;

fn main() {
    // Warnings matter here — a missing audio file or a plugin the registry does not know is
    // logged rather than shown — so surface them by default instead of requiring RUST_LOG. And
    // they are *kept*, because the terminal they used to go to is not somewhere an application
    // launched from an icon has: **View → Log** is where they are read.
    logbook::install();

    // Before anything reads a preference: the configuration moved to `~/.config/auris-studio`,
    // and an installation that predates the move keeps its settings, keymap and colour scheme.
    auris_session::migrate_legacy_config();

    Application::new().run(|cx: &mut App| {
        // The menu bar is built before the window, so the language comes from the settings file
        // rather than from the view that has not been created yet. `AurisApp` loads the same
        // file a moment later, and rebuilds these menus whenever the choice changes.
        cx.set_menus(menus(Settings::load().language()));

        let bounds = opening_bounds(cx);
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

        // Quit is normally handled by the main window's view (`AurisApp::on_quit`) so an
        // unsaved document can stop it. But a Quit can also land where no view is listening —
        // dispatch goes to the *active* window, and the Settings window listens for nothing —
        // and quitting from there would lose the main window's document just the same. So the
        // fallback runs the same guard through the main window instead of quitting on the spot.
        cx.on_action(move |_: &actions::Quit, cx: &mut App| {
            let guarded = window.update(cx, |view, window, cx| {
                let go = view.confirm_discard(ui::prompt::PendingAction::Quit);
                if !go {
                    // The sheet is asking in the main window; bring it forward so the
                    // question is not left behind the window the keystroke landed in.
                    window.activate_window();
                }
                cx.notify();
                go
            });
            // A clean document quits now — and so does a main window that is already gone,
            // because then there is nothing left to guard.
            if guarded.unwrap_or(true) {
                cx.quit();
            }
        });

        // The key bindings are dispatched to the focused view, so focus the app up front —
        // otherwise the space bar would not start playback until something was clicked. The
        // arrangement rather than the window itself: a binding scoped to a panel is only on the
        // dispatch path while that panel holds the keyboard, and the arrangement is where the
        // work starts. Everything bound at the window level is on the path from there too.
        window
            .update(cx, |view, window, cx| {
                view.focus_pane(app::Pane::Arrangement, window);

                // The close button is the last thing standing between an afternoon's work and
                // nothing. Returning `false` keeps the window open and leaves the sheet asking.
                let asked = cx.entity().downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    asked
                        .update(cx, |this, cx| {
                            let go = this.confirm_discard(ui::prompt::PendingAction::CloseWindow);
                            cx.notify();
                            go
                        })
                        .unwrap_or(true)
                });
            })
            .ok();

        // The project window is the application. Settings is a panel that happens to be a window
        // of its own, with no document to lose; leaving it up after the project window closed
        // meant an app that had not quit and a panel whose controls silently did nothing.
        let main = gpui::AnyWindowHandle::from(window).window_id();
        cx.on_window_closed(move |cx| {
            if !cx.windows().iter().any(|open| open.window_id() == main) {
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
    });
}

/// Where the window opens, shrunk to fit the display it opens on.
///
/// 1500×940 is the size this interface was laid out at, and asking for it unconditionally is
/// what a fixed size costs: a 1366×768 laptop — or a 1920×1080 screen at 150% scaling, which
/// reports 1280×720 — got a window taller than the desktop, with the title bar above the top of
/// the screen and no way to drag it back down. Every launch, not an edge case.
fn opening_bounds(cx: &App) -> Bounds<Pixels> {
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, PREFERRED_SIZE, cx);
    };
    Bounds::centered(None, fitted_size(PREFERRED_SIZE, display.bounds().size), cx)
}

/// The size the interface was laid out at.
const PREFERRED_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(1500.),
    height: px(940.),
};

/// `wanted`, shrunk to leave a margin inside `available`.
///
/// Never smaller than something usable, on the theory that a window that has to be scrolled to
/// is still better than one that cannot be reached at all.
fn fitted_size(wanted: gpui::Size<Pixels>, available: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    // Left clear of the screen edges, so a taskbar or a dock does not sit on the status bar.
    let margin = px(80.);
    size(
        wanted.width.min((available.width - margin).max(px(640.))),
        wanted.height.min((available.height - margin).max(px(480.))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opens_no_larger_than_the_screen_it_opens_on() {
        // A 1920×1080 panel at 150% reports 1280×720, and the interface wants 1500×940. Asking
        // for it anyway put the title bar above the top of the screen at every launch.
        let small = fitted_size(PREFERRED_SIZE, size(px(1280.), px(720.)));
        assert!(small.width < PREFERRED_SIZE.width);
        assert!(small.height < PREFERRED_SIZE.height);
        assert!(small.width <= px(1200.) && small.height <= px(640.));
    }

    #[test]
    fn a_screen_with_room_to_spare_gets_the_size_the_layout_wants() {
        let roomy = fitted_size(PREFERRED_SIZE, size(px(2560.), px(1440.)));
        assert_eq!(roomy, PREFERRED_SIZE);
    }

    #[test]
    fn a_screen_smaller_than_the_margin_still_gets_a_window() {
        // Subtracting the margin from a tiny display would otherwise ask for a negative size.
        let tiny = fitted_size(PREFERRED_SIZE, size(px(40.), px(30.)));
        assert_eq!(tiny, size(px(640.), px(480.)));
    }
}
