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

use std::path::PathBuf;

use auris_session::{Settings, WindowPlacement};
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

        let remembered = Settings::load().window;
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(opening_window_bounds(remembered, cx)),
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
                // The same write the window's own Quit does. This path is the one a Quit that
                // landed in the Settings window takes, and it is still the main window's
                // placement being put away.
                window
                    .update(cx, |view, _, _| view.save_window_placement())
                    .ok();
                cx.quit();
            }
        });

        // A project named on the command line, or handed over by the shell because somebody
        // double-clicked a `.auris` file with this registered against it. Opened *after* the
        // window exists, through the same path the file dialog and a dropped file use: it
        // reports what it opened, deals with missing audio, and paints a frame before it starts
        // decoding, none of which could happen if the document were loaded before the window.
        if let Some(path) = project_argument(std::env::args_os()) {
            window
                .update(cx, |view, _, cx| view.open_project_at(path, cx))
                .ok();
        }

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
                            if go {
                                this.save_window_placement();
                            }
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

/// The project to open at launch, out of the arguments the shell handed over.
///
/// The first argument that is not an option, and nothing after it: one window holds one
/// document, so a second path could only replace the first as it finished loading.
///
/// Anything starting with `-` is skipped rather than rejected. That is not politeness about
/// flags this binary does not have — it is what makes launching from the macOS Finder work at
/// all, because the process serial number arrives as `-psn_0_12345` in front of everything else.
///
/// The path is not checked here, for extension or for existence. `Session::open` has a sentence
/// for every way a file can fail to be a project, in the user's own language, and swallowing a
/// mistyped name would leave an empty window and no explanation.
fn project_argument(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<PathBuf> {
    args.into_iter()
        .skip(1)
        .find(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(PathBuf::from)
}

/// How wide and tall a remembered window has to still be showing to be worth restoring.
///
/// A rectangle is only useful if there is enough of it on a screen to take hold of. This much of
/// it — a strip of title bar and a corner — is the least that can be dragged back into view.
const RESCUABLE: gpui::Size<Pixels> = gpui::Size {
    width: px(160.),
    height: px(48.),
};

/// Where the window opens: back where it was, or centred if that is nowhere useful.
///
/// A remembered rectangle is not trusted on sight. The monitor it was on may be unplugged, the
/// desktop may have been rearranged, or the same file may have come from a machine with two
/// screens to one with none — and a window restored to a position off every display is a window
/// that cannot be reached at all, on an application whose only window it is. So the rectangle has
/// to still overlap the desktop by [`RESCUABLE`] before it is used.
///
/// A window that was maximised comes back maximised, carrying the size it restores to, so
/// unmaximising it lands where it was rather than filling the screen for ever.
fn opening_window_bounds(remembered: Option<WindowPlacement>, cx: &App) -> WindowBounds {
    let display = cx.primary_display().map(|display| display.bounds());
    match restorable(remembered, display) {
        Some((bounds, true)) => WindowBounds::Maximized(bounds),
        Some((bounds, false)) => WindowBounds::Windowed(bounds),
        None => WindowBounds::Windowed(opening_bounds(cx)),
    }
}

/// The remembered rectangle, if enough of it still lands on the desktop, and whether it was
/// maximised.
///
/// `None` for a display that could not be read at all: on a headless or unusual setup there is
/// nothing to check the rectangle against, and centring is the answer that cannot be wrong.
fn restorable(
    remembered: Option<WindowPlacement>,
    display: Option<Bounds<Pixels>>,
) -> Option<(Bounds<Pixels>, bool)> {
    let placement = remembered?;
    let display = display?;
    if !(placement.width > 0.0 && placement.height > 0.0) {
        return None;
    }
    let bounds = Bounds {
        origin: gpui::point(px(placement.x), px(placement.y)),
        size: size(px(placement.width), px(placement.height)),
    };
    let overlap = bounds.intersect(&display);
    let showing = overlap.size.width >= RESCUABLE.width && overlap.size.height >= RESCUABLE.height;
    showing.then_some((bounds, placement.maximized))
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

    fn args(list: &[&str]) -> Vec<std::ffi::OsString> {
        list.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn a_project_named_on_the_command_line_is_the_one_that_opens() {
        assert_eq!(
            project_argument(args(&["auris-studio", "Song/Song.auris"])),
            Some(PathBuf::from("Song/Song.auris"))
        );
        // Nothing to open is the ordinary launch.
        assert_eq!(project_argument(args(&["auris-studio"])), None);
    }

    #[test]
    fn the_finders_own_argument_is_stepped_over_rather_than_opened() {
        // Launching from the macOS Finder puts a process serial number in front of everything
        // else. Treating it as a filename would open every double-click on a broken path.
        assert_eq!(
            project_argument(args(&["auris-studio", "-psn_0_12345", "Song/Song.auris"])),
            Some(PathBuf::from("Song/Song.auris"))
        );
        assert_eq!(
            project_argument(args(&["auris-studio", "-psn_0_12345"])),
            None
        );
    }

    #[test]
    fn only_the_first_path_is_taken() {
        // One window holds one document, so the second could only replace the first as it
        // finished loading — two projects opening over each other, in whichever order the
        // disks happened to answer.
        assert_eq!(
            project_argument(args(&["auris-studio", "One.auris", "Two.auris"])),
            Some(PathBuf::from("One.auris"))
        );
    }

    fn placed(x: f32, y: f32, width: f32, height: f32) -> WindowPlacement {
        WindowPlacement {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    fn desktop() -> Bounds<Pixels> {
        Bounds {
            origin: gpui::point(px(0.), px(0.)),
            size: size(px(1920.), px(1080.)),
        }
    }

    #[test]
    fn a_window_comes_back_where_it_was_left() {
        let remembered = placed(200., 120., 1400., 900.);
        let (bounds, maximized) =
            restorable(Some(remembered), Some(desktop())).expect("still on screen");
        assert_eq!(bounds.origin.x, px(200.));
        assert_eq!(bounds.size.width, px(1400.));
        assert!(!maximized);
    }

    #[test]
    fn a_window_remembered_on_a_screen_that_is_gone_is_centred_instead() {
        // The second monitor was to the right and has been unplugged. Restoring this would open
        // the application's only window where no pointer can reach it.
        assert!(restorable(Some(placed(2400., 300., 1400., 900.)), Some(desktop())).is_none());
        // Above the desktop entirely, which is what a rearranged multi-monitor setup does.
        assert!(restorable(Some(placed(300., -1200., 1400., 900.)), Some(desktop())).is_none());
    }

    #[test]
    fn a_corner_still_showing_is_enough_to_drag_back_into_view() {
        // Mostly off the right edge, but 180 pixels of title bar remain. That can be taken hold
        // of, so it is not the emergency the case above is.
        let clinging = placed(1740., 200., 1400., 900.);
        assert!(restorable(Some(clinging), Some(desktop())).is_some());
    }

    #[test]
    fn nothing_remembered_and_nothing_measurable_both_fall_back() {
        assert!(restorable(None, Some(desktop())).is_none());
        assert!(restorable(Some(placed(100., 100., 800., 600.)), None).is_none());
        // A rectangle with no area, which is what a settings file filled in from defaults holds.
        assert!(restorable(Some(placed(0., 0., 0., 0.)), Some(desktop())).is_none());
    }

    #[test]
    fn a_maximised_window_carries_the_size_it_restores_to() {
        let remembered = WindowPlacement {
            maximized: true,
            ..placed(100., 80., 1200., 800.)
        };
        let (bounds, maximized) = restorable(Some(remembered), Some(desktop())).expect("on screen");
        assert!(maximized);
        assert_eq!(bounds.size.width, px(1200.), "not the whole screen");
    }
}
