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
// The window itself, driven from `cargo test`. Test-only: it exists to be compiled with
// gpui's test platform, which is a dev-dependency and so is not in the shipped binary.
mod auxiliary_window;
#[cfg(test)]
mod harness;
mod i18n;
mod keymap;
mod logbook;
mod menu;
mod project_windows;
mod settings_window;
mod setup_guide;
mod startup_soundfonts;
mod termination;
mod theme;
mod titlebar;
mod update_check;
mod voice_setup_window;

mod ui;

use std::path::PathBuf;

use auris_session::{Settings, WindowPlacement};
use gpui::{App, Application, Bounds, Pixels, WindowBounds, px, size};

use dock::PanelLayout;
use menu::{MenuState, menus};

fn main() {
    if let Some(code) = auris_session::handle_drum_probe_worker() {
        std::process::exit(code);
    }
    if let Some(code) = auris_session::handle_plugin_discovery_worker() {
        std::process::exit(code);
    }
    // Warnings matter here — a missing audio file or a plugin the registry does not know is
    // logged rather than shown — so surface them by default instead of requiring RUST_LOG. And
    // they are *kept*, because the terminal they used to go to is not somewhere an application
    // launched from an icon has: **View → Log** is where they are read.
    logbook::install();

    // Before anything reads a preference: the configuration moved to `~/.config/auris-studio`,
    // and an installation that predates the move keeps its settings, keymap and colour scheme.

    // Finder hands a double-clicked document to a running or launching application as an
    // open-document event rather than an argument. The handler has no window to talk to, so it
    // posts the URLs here and the window collects them once it exists.
    let (handed_over, open_requests) = std::sync::mpsc::channel::<Vec<String>>();
    let application = Application::new().with_assets(atelier::Assets::with_fallback(
        ui::provider_logo::ProviderAssets,
    ));
    application.on_open_urls(move |urls| {
        let _ = handed_over.send(urls);
    });
    application.run(move |cx: &mut App| {
        termination::install();
        // The Markdown renderer and Atelier share the application's resolved palette.
        gpui_component::init(cx);
        appearance::Appearance::load().theme().install(cx);
        // The menu bar is built before the window, so the language comes from the settings file
        // rather than from the view that has not been created yet. `AurisApp` loads the same
        // file a moment later, and rebuilds these menus whenever the choice changes.
        cx.set_menus(menus(
            Settings::load().language(),
            &PanelLayout::default(),
            MenuState::default(),
        ));

        project_windows::install(cx);
        let window = project_windows::create(cx).expect("could not open the main window");
        window
            .update(cx, |view, _, cx| {
                view.download_missing_soundfonts(cx);
                view.start_auto_updates(cx);
            })
            .ok();
        cx.on_action(|_: &actions::Quit, cx| project_windows::request_quit(cx));
        if let Some(path) = project_argument(std::env::args_os()) {
            window
                .update(cx, |view, _, cx| view.open_project_at(path, cx))
                .ok();
        } else {
            window
                .update(cx, |view, window, cx| view.show_project_chooser(window, cx))
                .ok();
        }
        window
            .update(cx, |view, _, cx| view.offer_initial_setup(cx))
            .ok();
        cx.spawn(async move |cx| {
            loop {
                if termination::take_request() {
                    let _ = cx.update(project_windows::request_quit);
                }
                while let Ok(urls) = open_requests.try_recv() {
                    for path in urls.iter().filter_map(|url| handed_over_project(url)) {
                        let _ = cx.update(|cx| project_windows::open(Some(path), cx));
                    }
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The local file a URL from the shell's open-document event names.
///
/// Only `file:` URLs: the bundle declares the `.auris` document type and nothing else, so
/// anything else arriving here is not a project and is ignored.
fn handed_over_project(url: &str) -> Option<PathBuf> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
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
    let displays: Vec<_> = cx
        .displays()
        .iter()
        .map(|display| display.bounds())
        .collect();
    match restorable(remembered, &displays) {
        Some((bounds, true)) => WindowBounds::Maximized(bounds),
        Some((bounds, false)) => WindowBounds::Windowed(bounds),
        None => WindowBounds::Windowed(opening_bounds(cx)),
    }
}

/// The remembered rectangle, if enough of it still lands on the desktop, and whether it was
/// maximised.
///
/// `None` when no display could be read at all: on a headless or unusual setup there is
/// nothing to check the rectangle against, and centring is the answer that cannot be wrong.
fn restorable(
    remembered: Option<WindowPlacement>,
    displays: &[Bounds<Pixels>],
) -> Option<(Bounds<Pixels>, bool)> {
    let placement = remembered?;
    if !(placement.width > 0.0 && placement.height > 0.0) {
        return None;
    }
    let bounds = Bounds {
        origin: gpui::point(px(placement.x), px(placement.y)),
        size: size(px(placement.width), px(placement.height)),
    };
    let showing = displays.iter().any(|display| {
        let overlap = bounds.intersect(display);
        overlap.size.width >= RESCUABLE.width && overlap.size.height >= RESCUABLE.height
    });
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
    fn a_handed_over_file_url_is_the_project_it_names() {
        let (url, path) = if cfg!(windows) {
            (
                "file:///C:/Songs/My%20Song/My%20Song.auris",
                PathBuf::from("C:/Songs/My Song/My Song.auris"),
            )
        } else {
            (
                "file:///Users/bob/My%20Song/My%20Song.auris",
                PathBuf::from("/Users/bob/My Song/My Song.auris"),
            )
        };
        assert_eq!(handed_over_project(url), Some(path));
        assert_eq!(handed_over_project("https://example.com/Song.auris"), None);
        assert_eq!(handed_over_project("not a url"), None);
    }

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
            restorable(Some(remembered), &[desktop()]).expect("still on screen");
        assert_eq!(bounds.origin.x, px(200.));
        assert_eq!(bounds.size.width, px(1400.));
        assert!(!maximized);
    }

    #[test]
    fn a_window_remembered_on_a_screen_that_is_gone_is_centred_instead() {
        // The second monitor was to the right and has been unplugged. Restoring this would open
        // the application's only window where no pointer can reach it.
        assert!(restorable(Some(placed(2400., 300., 1400., 900.)), &[desktop()]).is_none());
        // Above the desktop entirely, which is what a rearranged multi-monitor setup does.
        assert!(restorable(Some(placed(300., -1200., 1400., 900.)), &[desktop()]).is_none());
    }

    #[test]
    fn a_window_on_a_connected_secondary_display_is_restored() {
        let secondary = Bounds {
            origin: gpui::point(px(1920.), px(0.)),
            size: size(px(1920.), px(1080.)),
        };
        assert!(
            restorable(
                Some(placed(2200., 120., 1400., 900.)),
                &[desktop(), secondary]
            )
            .is_some()
        );
    }

    #[test]
    fn a_corner_still_showing_is_enough_to_drag_back_into_view() {
        // Mostly off the right edge, but 180 pixels of title bar remain. That can be taken hold
        // of, so it is not the emergency the case above is.
        let clinging = placed(1740., 200., 1400., 900.);
        assert!(restorable(Some(clinging), &[desktop()]).is_some());
    }

    #[test]
    fn nothing_remembered_and_nothing_measurable_both_fall_back() {
        assert!(restorable(None, &[desktop()]).is_none());
        assert!(restorable(Some(placed(100., 100., 800., 600.)), &[]).is_none());
        // A rectangle with no area, which is what a settings file filled in from defaults holds.
        assert!(restorable(Some(placed(0., 0., 0., 0.)), &[desktop()]).is_none());
    }

    #[test]
    fn a_maximised_window_carries_the_size_it_restores_to() {
        let remembered = WindowPlacement {
            maximized: true,
            ..placed(100., 80., 1200., 800.)
        };
        let (bounds, maximized) = restorable(Some(remembered), &[desktop()]).expect("on screen");
        assert!(maximized);
        assert_eq!(bounds.size.width, px(1200.), "not the whole screen");
    }
}
