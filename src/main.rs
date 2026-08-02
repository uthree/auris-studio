//! Auris Studio — a digital audio workstation built with Rust and gpui.

#![warn(missing_docs)]

mod actions;
mod app;
mod history;
mod theme;
mod ui;

use gpui::{
    App, AppContext, Application, Bounds, Focusable, Menu, MenuItem, SystemMenuType,
    TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

use app::AurisApp;

fn main() {
    // Warnings matter here — a missing audio file or a plugin the registry does not know is
    // logged rather than shown — so surface them by default instead of requiring RUST_LOG.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    Application::new().run(|cx: &mut App| {
        actions::bind_keys(cx);
        cx.on_action(|_: &actions::Quit, cx: &mut App| cx.quit());
        cx.set_menus(menus());

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

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Auris Studio".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Auris Studio", actions::Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Project", actions::NewProject),
                MenuItem::action("Open Project…", actions::OpenProject),
                MenuItem::separator(),
                MenuItem::action("Save", actions::SaveProject),
                MenuItem::action("Save As…", actions::SaveProjectAs),
                MenuItem::separator(),
                MenuItem::action("Import Audio…", actions::ImportAudio),
                MenuItem::action("Export WAV…", actions::ExportAudio),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", actions::Undo),
                MenuItem::action("Redo", actions::Redo),
                MenuItem::separator(),
                MenuItem::action("Delete", actions::DeleteSelection),
            ],
        },
        Menu {
            name: "Track".into(),
            items: vec![
                MenuItem::action("Add Instrument Track", actions::AddInstrumentTrack),
                MenuItem::action("Add Audio Track", actions::AddAudioTrack),
                MenuItem::separator(),
                MenuItem::action("Delete Track", actions::DeleteTrack),
            ],
        },
        Menu {
            name: "Transport".into(),
            items: vec![
                MenuItem::action("Play / Stop", actions::TogglePlay),
                MenuItem::action("Return to Zero", actions::ReturnToZero),
                MenuItem::action("Toggle Loop", actions::ToggleLoop),
                MenuItem::separator(),
                MenuItem::action("Panic", actions::PanicStop),
            ],
        },
    ]
}
