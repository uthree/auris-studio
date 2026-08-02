//! Auris Studio — a digital audio workstation built with Rust and gpui.

#![warn(missing_docs)]

mod actions;
mod app;
mod gestures;
mod i18n;
mod keymap;
mod settings_window;
mod theme;
mod ui;

use auris_i18n::{Key, Language};
use auris_session::Settings;
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

/// The platform menu bar, in `language`.
///
/// Rebuilt rather than re-rendered when the language changes: the menu bar belongs to the
/// operating system, so nothing about a redraw would touch it.
pub fn menus(language: Language) -> Vec<Menu> {
    let t = |key: Key| key.get(language);
    vec![
        Menu {
            // The application's own name is not translated — it is what the bundle is called.
            name: "Auris Studio".into(),
            items: vec![
                MenuItem::action(t(Key::MenuSettingsItem), actions::OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu(t(Key::MenuServices), SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(t(Key::MenuQuitApp), actions::Quit),
            ],
        },
        Menu {
            name: t(Key::GroupFile).into(),
            items: vec![
                MenuItem::action(t(Key::CmdNewProject), actions::NewProject),
                MenuItem::action(t(Key::MenuOpenProjectItem), actions::OpenProject),
                MenuItem::separator(),
                MenuItem::action(t(Key::CmdSave), actions::SaveProject),
                MenuItem::action(t(Key::MenuSaveAsItem), actions::SaveProjectAs),
                MenuItem::separator(),
                MenuItem::action(t(Key::MenuImportAudioItem), actions::ImportAudio),
                MenuItem::action(t(Key::MenuExportWavItem), actions::ExportAudio),
            ],
        },
        Menu {
            name: t(Key::GroupEdit).into(),
            items: vec![
                MenuItem::action(t(Key::CmdUndo), actions::Undo),
                MenuItem::action(t(Key::CmdRedo), actions::Redo),
                MenuItem::separator(),
                MenuItem::action(t(Key::MenuDelete), actions::DeleteSelection),
            ],
        },
        Menu {
            name: t(Key::GroupTrack).into(),
            items: vec![
                MenuItem::action(t(Key::CmdAddInstrumentTrack), actions::AddInstrumentTrack),
                MenuItem::action(t(Key::CmdAddAudioTrack), actions::AddAudioTrack),
                MenuItem::separator(),
                MenuItem::action(t(Key::CmdDeleteTrack), actions::DeleteTrack),
            ],
        },
        Menu {
            name: t(Key::GroupTransport).into(),
            items: vec![
                MenuItem::action(t(Key::CmdPlayStop), actions::TogglePlay),
                MenuItem::action(t(Key::CmdReturnToZero), actions::ReturnToZero),
                MenuItem::action(t(Key::CmdToggleCycle), actions::ToggleLoop),
                MenuItem::separator(),
                MenuItem::action(t(Key::CmdPanic), actions::PanicStop),
            ],
        },
    ]
}
