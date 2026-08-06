//! The application menu, as data.
//!
//! One table, two renderings. macOS has a menu bar that belongs to the system, so the menu is
//! handed over and drawn by it; Windows and Linux have no such thing, and gpui's `set_menus`
//! there stores the menu without displaying it, so the window draws its own — see
//! [`crate::ui::menu_bar`].
//!
//! Both read this table, which is the point: a command added here reaches every platform
//! without being written twice and drifting.

use auris_i18n::{Key, Language};
use gpui::{Action, Menu, MenuItem, SharedString, SystemMenuType};

use crate::actions;

/// One row of a menu.
pub enum MenuRow {
    /// The rule between two groups.
    Separator,
    /// A command.
    Command {
        /// Text shown in the row.
        label: SharedString,
        /// What choosing it does.
        action: Box<dyn Action>,
        /// Identifier of the key binding shown beside it, as in [`crate::actions::BINDABLE`].
        binding: &'static str,
    },
    /// A submenu the operating system fills in itself. macOS only.
    System {
        /// Text shown in the row.
        label: SharedString,
        /// Which submenu.
        menu: SystemMenuType,
    },
}

/// One menu on the bar.
pub struct MenuSection {
    /// Title shown on the bar.
    pub name: SharedString,
    /// The rows, in order.
    pub rows: Vec<MenuRow>,
}

/// A command row.
fn command(label: SharedString, action: impl Action, binding: &'static str) -> MenuRow {
    MenuRow::Command {
        label,
        action: Box::new(action),
        binding,
    }
}

/// The whole menu, in `language`.
///
/// The shape differs by platform because the conventions do. macOS collects the application's
/// own commands into a menu named after the application, which the system draws first; Windows
/// and Linux have no such menu, and preferences and quit belong at the bottom of File.
pub fn model(language: Language) -> Vec<MenuSection> {
    let t = |key: Key| -> SharedString { key.get(language).into() };
    let mut sections = Vec::new();

    if cfg!(target_os = "macos") {
        sections.push(MenuSection {
            // The application's own name is not translated — it is what the bundle is called.
            name: "Auris Studio".into(),
            rows: vec![
                command(
                    t(Key::MenuSettingsItem),
                    actions::OpenSettings,
                    "view.settings",
                ),
                MenuRow::Separator,
                MenuRow::System {
                    label: t(Key::MenuServices),
                    menu: SystemMenuType::Services,
                },
                MenuRow::Separator,
                command(t(Key::MenuQuitApp), actions::Quit, "file.quit"),
            ],
        });
    }

    let mut file = vec![
        command(t(Key::CmdNewProject), actions::NewProject, "file.new"),
        command(
            t(Key::MenuOpenProjectItem),
            actions::OpenProject,
            "file.open",
        ),
        // With New and Open rather than off on its own: all three replace the document, and
        // that is what a person needs to know before choosing one.
        command(
            t(Key::MenuComposeItem),
            actions::ComposeSong,
            "file.compose",
        ),
        MenuRow::Separator,
        command(t(Key::CmdSave), actions::SaveProject, "file.save"),
        command(
            t(Key::MenuSaveAsItem),
            actions::SaveProjectAs,
            "file.save_as",
        ),
        MenuRow::Separator,
        command(
            t(Key::MenuImportAudioItem),
            actions::ImportAudio,
            "file.import",
        ),
        command(
            t(Key::MenuImportSoundFontItem),
            actions::ImportSoundFont,
            "file.import_soundfont",
        ),
        command(
            t(Key::CmdImportMidi),
            actions::ImportMidi,
            "file.import_midi",
        ),
        command(
            t(Key::CmdExportMidi),
            actions::ExportMidi,
            "file.export_midi",
        ),
        command(
            t(Key::MenuCollectAssetsItem),
            actions::CollectAssets,
            "file.collect",
        ),
        command(
            t(Key::MenuExportWavItem),
            actions::ExportAudio,
            "file.export",
        ),
        command(
            t(Key::MenuExportCycleItem),
            actions::ExportCycle,
            "file.export_cycle",
        ),
    ];
    if !cfg!(target_os = "macos") {
        file.push(MenuRow::Separator);
        file.push(command(
            t(Key::MenuSettingsItem),
            actions::OpenSettings,
            "view.settings",
        ));
        file.push(MenuRow::Separator);
        file.push(command(t(Key::MenuQuitApp), actions::Quit, "file.quit"));
    }
    sections.push(MenuSection {
        name: t(Key::GroupFile),
        rows: file,
    });

    sections.push(MenuSection {
        name: t(Key::GroupEdit),
        rows: vec![
            command(t(Key::CmdUndo), actions::Undo, "edit.undo"),
            command(t(Key::CmdRedo), actions::Redo, "edit.redo"),
            MenuRow::Separator,
            command(t(Key::MenuDelete), actions::DeleteSelection, "edit.delete"),
            MenuRow::Separator,
            command(t(Key::CmdNextTool), actions::NextTool, "edit.next_tool"),
            MenuRow::Separator,
            // The three things about the song itself that a person otherwise changes by reaching
            // for a readout with the mouse.
            command(t(Key::CmdSetTempo), actions::SetTempo, "edit.tempo"),
            command(
                t(Key::CmdSetSignature),
                actions::SetTimeSignature,
                "edit.signature",
            ),
            command(t(Key::CmdCycleGrid), actions::CycleGrid, "edit.grid"),
        ],
    });

    sections.push(MenuSection {
        name: t(Key::GroupTrack),
        rows: vec![
            command(
                t(Key::CmdAddInstrumentTrack),
                actions::AddInstrumentTrack,
                "track.add_instrument",
            ),
            command(
                t(Key::CmdAddAudioTrack),
                actions::AddAudioTrack,
                "track.add_audio",
            ),
            MenuRow::Separator,
            command(t(Key::CmdDeleteTrack), actions::DeleteTrack, "track.delete"),
        ],
    });

    // The palette leads, because a menu is where somebody who does not know it exists will find
    // it — and once it is open, everything below is reachable by typing its name instead.
    sections.push(MenuSection {
        name: t(Key::GroupView),
        rows: vec![
            command(
                t(Key::CmdCommandPalette),
                actions::OpenCommandPalette,
                "view.palette",
            ),
            MenuRow::Separator,
            command(
                t(Key::CmdShowLibrary),
                actions::ToggleLibrary,
                "view.library",
            ),
            command(
                t(Key::CmdShowInspector),
                actions::ToggleInspector,
                "view.inspector",
            ),
            command(
                t(Key::CmdShowPianoRoll),
                actions::TogglePianoRoll,
                "view.piano_roll",
            ),
            command(t(Key::CmdShowMixer), actions::ToggleMixer, "view.mixer"),
            command(t(Key::CmdShowLog), actions::ToggleLog, "view.log"),
            MenuRow::Separator,
            command(
                t(Key::CmdShowStructureLane),
                actions::ToggleStructureLane,
                "view.structure_lane",
            ),
            command(
                t(Key::CmdShowHarmonyLane),
                actions::ToggleHarmonyLane,
                "view.harmony_lane",
            ),
            command(
                t(Key::CmdShowTempoMarks),
                actions::ToggleTempoMarks,
                "view.tempo_marks",
            ),
            command(
                t(Key::CmdShowBendLane),
                actions::ToggleBendLane,
                "view.bend_lane",
            ),
            command(
                t(Key::CmdShowModulationLane),
                actions::ToggleModulationLane,
                "view.modulation_lane",
            ),
            MenuRow::Separator,
            command(t(Key::CmdZoomIn), actions::ZoomIn, "view.zoom_in"),
            command(t(Key::CmdZoomOut), actions::ZoomOut, "view.zoom_out"),
        ],
    });

    sections.push(MenuSection {
        name: t(Key::GroupTransport),
        rows: vec![
            command(t(Key::CmdPlayStop), actions::TogglePlay, "transport.play"),
            command(
                t(Key::CmdReturnToZero),
                actions::ReturnToZero,
                "transport.return",
            ),
            command(
                t(Key::CmdToggleCycle),
                actions::ToggleLoop,
                "transport.loop",
            ),
            MenuRow::Separator,
            command(
                t(Key::CmdGoToPosition),
                actions::GoToPosition,
                "transport.go_to",
            ),
            MenuRow::Separator,
            command(t(Key::CmdPanic), actions::PanicStop, "transport.panic"),
        ],
    });

    sections
}

/// The menu in the shape gpui hands to the operating system.
///
/// Rebuilt rather than re-rendered when the language changes: the menu bar belongs to the
/// operating system, so nothing about a redraw would touch it.
pub fn menus(language: Language) -> Vec<Menu> {
    model(language)
        .into_iter()
        .map(|section| Menu {
            name: section.name,
            items: section
                .rows
                .into_iter()
                .map(|row| match row {
                    MenuRow::Separator => MenuItem::Separator,
                    // The keystroke is not passed along: the system menu bar looks each action
                    // up in the keymap and draws its own.
                    MenuRow::Command { label, action, .. } => MenuItem::Action {
                        name: label,
                        action,
                        os_action: None,
                    },
                    MenuRow::System { label, menu } => MenuItem::os_submenu(label, menu),
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_names_a_key_binding_that_exists() {
        // The in-window bar shows the keystroke beside each command by looking the id up. A
        // typo would silently leave the column blank rather than fail anywhere.
        for section in model(Language::English) {
            for row in section.rows {
                if let MenuRow::Command { label, binding, .. } = row {
                    assert!(
                        actions::bindable(binding).is_some(),
                        "`{label}` names a binding `{binding}` that does not exist"
                    );
                }
            }
        }
    }

    #[test]
    fn settings_and_quit_are_reachable_on_every_platform() {
        // They live in the application menu on macOS and at the bottom of File elsewhere, and
        // the second arrangement is easy to forget when adding to the first.
        let labels: Vec<String> = model(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        for expected in [
            Key::MenuSettingsItem.get(Language::English),
            Key::MenuQuitApp.get(Language::English),
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "`{expected}` is in no menu on this platform"
            );
        }
    }

    #[test]
    fn the_palette_is_in_a_menu_where_somebody_can_find_it() {
        // A palette reached only by a keystroke is a feature for people who already know about
        // it. The menu row is how anybody else finds out it exists.
        let labels: Vec<String> = model(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        let expected = Key::CmdCommandPalette.get(Language::English);
        assert!(
            labels.iter().any(|label| label == expected),
            "`{expected}` is in no menu"
        );
    }

    #[test]
    fn composing_is_reachable_without_the_command_line() {
        // `Session::compose` was written with the composer and the desktop application never
        // called it, so the whole feature existed only for `auris compose`. A menu row is what
        // makes it exist for everyone else.
        let labels: Vec<String> = model(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        let expected = Key::MenuComposeItem.get(Language::English);
        assert!(
            labels.iter().any(|label| label == expected),
            "`{expected}` is in no menu"
        );
    }

    #[test]
    fn no_command_appears_twice() {
        let mut labels: Vec<String> = model(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        labels.sort();
        let count = labels.len();
        labels.dedup();
        assert_eq!(count, labels.len(), "a command is in two menus at once");
    }

    #[test]
    fn the_system_submenu_is_offered_only_where_there_is_a_system_to_fill_it() {
        let system = model(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .any(|row| matches!(row, MenuRow::System { .. }));
        assert_eq!(
            system,
            cfg!(target_os = "macos"),
            "the Services submenu exists only on macOS; elsewhere it would be an empty row"
        );
    }

    #[test]
    fn a_menu_never_leads_or_ends_with_a_rule() {
        for section in model(Language::English) {
            assert!(
                !matches!(section.rows.first(), Some(MenuRow::Separator)),
                "`{}` starts with a rule against its own top edge",
                section.name
            );
            assert!(
                !matches!(section.rows.last(), Some(MenuRow::Separator)),
                "`{}` ends with a rule and nothing under it",
                section.name
            );
        }
    }

    #[test]
    fn the_gpui_menu_keeps_every_row() {
        let model_rows: usize = model(Language::Japanese)
            .iter()
            .map(|section| section.rows.len())
            .sum();
        let menu_items: usize = menus(Language::Japanese)
            .iter()
            .map(|menu| menu.items.len())
            .sum();
        assert_eq!(model_rows, menu_items);
    }
}
