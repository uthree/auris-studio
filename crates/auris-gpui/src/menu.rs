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
use crate::dock::{Panel, PanelLayout};
use auris_session::prelude::{CC_MODULATION, ClipCurve};

/// What the menu needs to know about the document and the window to draw itself.
///
/// A flat snapshot rather than a borrow of the application, so [`model`] stays a function of its
/// arguments and the whole menu — every label, every tick, every dimmed row — can be asserted
/// without a window. Everything here comes from the session; what comes from the *window* is the
/// [`PanelLayout`] passed beside it, which already answers which panels and strips are showing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// There is a step on the undo stack.
    pub can_undo: bool,
    /// There is a step to put back.
    pub can_redo: bool,
    /// The transport is cycling over the loop region.
    pub looping: bool,
    /// Takes are being trimmed to the punch region.
    pub punching: bool,
    /// A take is running.
    pub recording: bool,
    /// The live input is being played through a track.
    pub monitoring: bool,
    /// The click is on.
    pub metronome: bool,
    /// The computer keyboard is playing notes.
    pub musical_typing: bool,
}

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
        /// Whether choosing it could do anything.
        ///
        /// Only Undo and Redo are ever `false`, and only because those two are the pair everybody
        /// looks at to find out whether there is anything to take back — the rest of the menu
        /// answers "nothing selected" with a line in the status bar, which says more than a
        /// greyed row would.
        enabled: bool,
        /// Whether the thing this row switches is currently on.
        ///
        /// The View and Transport menus are full of switches whose labels are nouns — "Mixer",
        /// "Metronome" — and a noun with no mark beside it cannot say which way it is set.
        checked: bool,
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

/// A command row: always available, never ticked.
fn command(label: SharedString, action: impl Action, binding: &'static str) -> MenuRow {
    MenuRow::Command {
        label,
        action: Box::new(action),
        binding,
        enabled: true,
        checked: false,
    }
}

/// A command row that is dimmed and inert while `enabled` is false.
fn command_if(
    enabled: bool,
    label: SharedString,
    action: impl Action,
    binding: &'static str,
) -> MenuRow {
    MenuRow::Command {
        label,
        action: Box::new(action),
        binding,
        enabled,
        checked: false,
    }
}

/// A command row that carries a tick while the thing it switches is `on`.
fn toggle(label: SharedString, action: impl Action, binding: &'static str, on: bool) -> MenuRow {
    MenuRow::Command {
        label,
        action: Box::new(action),
        binding,
        enabled: true,
        checked: on,
    }
}

/// The whole menu, in `language`, with every switch set the way `state` and `panels` say.
///
/// The shape differs by platform because the conventions do. macOS collects the application's
/// own commands into a menu named after the application, which the system draws first; Windows
/// and Linux have no such menu, and preferences and quit belong at the bottom of File.
pub fn model(language: Language, panels: &PanelLayout, state: MenuState) -> Vec<MenuSection> {
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
        command(
            t(Key::MenuExportStemsItem),
            actions::ExportStems,
            "file.export_stems",
        ),
        command(
            t(Key::CmdExportSingerFrames),
            actions::ExportSingerFrames,
            "file.export_frames",
        ),
    ];
    // Under Open rather than at the bottom, which is where every application on both platforms
    // puts it. A row rather than a submenu: gpui's menu rows carry an action and nothing else,
    // and an action cannot carry a path — so this one opens the list itself.
    file.insert(
        2,
        command(t(Key::CmdOpenRecent), actions::OpenRecent, "file.recent"),
    );
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
            command_if(state.can_undo, t(Key::CmdUndo), actions::Undo, "edit.undo"),
            command_if(state.can_redo, t(Key::CmdRedo), actions::Redo, "edit.redo"),
            MenuRow::Separator,
            // Cut, copy and paste at the top of Edit, where every application on both platforms
            // puts them, and in pairs for the same reason the two Select Alls below are: one
            // keystroke, two meanings, and the menu is where a person finds out which one they
            // just got.
            command(t(Key::CmdCutNotes), actions::CutNotes, "edit.cut"),
            command(t(Key::CmdCutClips), actions::CutClips, "clip.cut"),
            command(t(Key::CmdCopyNotes), actions::CopyNotes, "edit.copy"),
            command(t(Key::CmdCopyClips), actions::CopyClips, "clip.copy"),
            command(t(Key::CmdPasteNotes), actions::PasteNotes, "edit.paste"),
            command(t(Key::CmdPasteClips), actions::PasteClips, "clip.paste"),
            MenuRow::Separator,
            // The note commands and the clip commands sit together, in pairs, because they are
            // the same command asked of two different things — and each pair shares a keystroke,
            // scoped so that whichever panel has the keyboard answers. Saying so on the menu is
            // how a person finds out that ⌘A did not do nothing, it did the other one.
            command(
                t(Key::CmdSelectAllNotes),
                actions::SelectAllNotes,
                "edit.select_all",
            ),
            command(
                t(Key::CmdSelectAllClips),
                actions::SelectAllClips,
                "clip.select_all",
            ),
            command(
                t(Key::CmdDuplicateNotes),
                actions::DuplicateNotes,
                "edit.duplicate",
            ),
            command(
                t(Key::CmdDuplicateClip),
                actions::DuplicateClip,
                "clip.duplicate",
            ),
            command(t(Key::MenuDelete), actions::DeleteSelection, "edit.delete"),
            MenuRow::Separator,
            command(
                t(Key::CmdTransposeUp),
                actions::TransposeUp,
                "edit.transpose_up",
            ),
            command(
                t(Key::CmdTransposeDown),
                actions::TransposeDown,
                "edit.transpose_down",
            ),
            command(t(Key::CmdOctaveUp), actions::OctaveUp, "edit.octave_up"),
            command(
                t(Key::CmdOctaveDown),
                actions::OctaveDown,
                "edit.octave_down",
            ),
            MenuRow::Separator,
            // In pairs, and both spelled out, for the reason the cut and copy rows above are: the
            // two share a keystroke and the menu is where somebody finds out which one ⌥← just
            // gave them.
            command(
                t(Key::CmdNudgeNotesLeft),
                actions::NudgeNotesLeft,
                "edit.nudge_left",
            ),
            command(
                t(Key::CmdNudgeClipsLeft),
                actions::NudgeClipsLeft,
                "clip.nudge_left",
            ),
            command(
                t(Key::CmdNudgeNotesRight),
                actions::NudgeNotesRight,
                "edit.nudge_right",
            ),
            command(
                t(Key::CmdNudgeClipsRight),
                actions::NudgeClipsRight,
                "clip.nudge_right",
            ),
            MenuRow::Separator,
            // The three quantise passes together, in the order a person thinks of them: the
            // plain one, then each half of the note on its own.
            command(
                t(Key::CmdQuantize),
                actions::QuantizeNoteStarts,
                "edit.quantize",
            ),
            command(
                t(Key::CmdQuantizeLengths),
                actions::QuantizeNoteLengths,
                "edit.quantize_lengths",
            ),
            command(
                t(Key::CmdQuantizeBoth),
                actions::QuantizeNotes,
                "edit.quantize_both",
            ),
            MenuRow::Separator,
            command(t(Key::CmdSplitClip), actions::SplitClip, "clip.split"),
            command(
                t(Key::CmdToggleClipMute),
                actions::ToggleClipMute,
                "clip.mute",
            ),
            command(
                t(Key::CmdToggleClipLoop),
                actions::ToggleClipLoop,
                "clip.loop",
            ),
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
                t(Key::CmdAddSingerTrack),
                actions::AddSingerTrack,
                "track.add_singer",
            ),
            command(
                t(Key::CmdAddAudioTrack),
                actions::AddAudioTrack,
                "track.add_audio",
            ),
            command(
                t(Key::CmdAddBusTrack),
                actions::AddBusTrack,
                "track.add_bus",
            ),
            MenuRow::Separator,
            command(
                t(Key::CmdToggleTrackMute),
                actions::ToggleTrackMute,
                "track.mute",
            ),
            command(
                t(Key::CmdToggleTrackSolo),
                actions::ToggleTrackSolo,
                "track.solo",
            ),
            MenuRow::Separator,
            command(
                t(Key::CmdSelectPreviousTrack),
                actions::SelectPreviousTrack,
                "track.select_previous",
            ),
            command(
                t(Key::CmdSelectNextTrack),
                actions::SelectNextTrack,
                "track.select_next",
            ),
            MenuRow::Separator,
            command(
                t(Key::CmdDuplicateTrack),
                actions::DuplicateTrack,
                "track.duplicate",
            ),
            command(t(Key::CmdDeleteTrack), actions::DeleteTrack, "track.delete"),
        ],
    });

    // A menu of its own, and named for what it does rather than for how it is fed. Composing was
    // one row in the middle of File, between Open Project and Save, carrying the label of the
    // *specification file* route — so the sheet, which is the way in that needs no file at all,
    // was announced as "Compose from Specification…" and the file route was in no menu whatever.
    // Nobody who had not been told was going to find either.
    sections.push(MenuSection {
        name: t(Key::GroupCompose),
        rows: vec![
            command(t(Key::CmdComposeSong), actions::ComposeSong, "file.compose"),
            command(
                t(Key::CmdComposeFromSpec),
                actions::ComposeFromSpec,
                "file.compose_spec",
            ),
            MenuRow::Separator,
            // Under the same heading and below a rule, because it is the composer pointed the
            // other way: the two above replace the document with a piece, and this one writes
            // parts around a piece of it that is already there.
            command(
                t(Key::CmdAccompanyMelody),
                actions::AccompanyMelody,
                "file.accompany",
            ),
            // With the composer rather than with the mixer, because this is what composing already
            // does at the end of every piece — the row is here for the piece that was written
            // before it existed, and for the one whose instruments have been changed since.
            command(
                t(Key::CmdBalanceLevels),
                actions::BalanceLevels,
                "mix.balance",
            ),
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
            // Every row from here to the zoom is a switch whose label is a noun. "Mixer" cannot
            // say whether the mixer is showing; the tick beside it can.
            toggle(
                t(Key::CmdShowLibrary),
                actions::ToggleLibrary,
                "view.library",
                panels.is_open(Panel::Library),
            ),
            toggle(
                t(Key::CmdShowInspector),
                actions::ToggleInspector,
                "view.inspector",
                panels.is_open(Panel::Inspector),
            ),
            toggle(
                t(Key::CmdShowPianoRoll),
                actions::TogglePianoRoll,
                "view.piano_roll",
                panels.is_open(Panel::PianoRoll),
            ),
            toggle(
                t(Key::CmdShowMixer),
                actions::ToggleMixer,
                "view.mixer",
                panels.is_open(Panel::Mixer),
            ),
            toggle(
                t(Key::CmdShowLog),
                actions::ToggleLog,
                "view.log",
                panels.is_open(Panel::Log),
            ),
            toggle(
                t(Key::CmdShowAgent),
                actions::ToggleAgent,
                "view.agent",
                panels.is_open(Panel::Agent),
            ),
            MenuRow::Separator,
            toggle(
                t(Key::CmdShowStructureLane),
                actions::ToggleStructureLane,
                "view.structure_lane",
                panels.lanes.structure,
            ),
            toggle(
                t(Key::CmdShowHarmonyLane),
                actions::ToggleHarmonyLane,
                "view.harmony_lane",
                panels.lanes.harmony,
            ),
            toggle(
                t(Key::CmdShowTempoMarks),
                actions::ToggleTempoMarks,
                "view.tempo_marks",
                panels.lanes.tempo,
            ),
            toggle(
                t(Key::CmdShowBendLane),
                actions::ToggleBendLane,
                "view.bend_lane",
                panels.curve_lane(ClipCurve::Bend),
            ),
            toggle(
                t(Key::CmdShowModulationLane),
                actions::ToggleModulationLane,
                "view.modulation_lane",
                panels.curve_lane(ClipCurve::Controller(CC_MODULATION)),
            ),
            MenuRow::Separator,
            command(t(Key::CmdZoomIn), actions::ZoomIn, "view.zoom_in"),
            command(t(Key::CmdZoomOut), actions::ZoomOut, "view.zoom_out"),
            MenuRow::Separator,
            // Not a Help menu of its own for one row. What people are looking for when they
            // reach for Help in an application with no manual is the version number, and this
            // is the menu they are already in.
            command(t(Key::CmdAbout), actions::ShowAbout, "view.about"),
        ],
    });

    sections.push(MenuSection {
        name: t(Key::GroupTransport),
        rows: vec![
            // Play is not ticked, though it is as much a switch as the rest: the transport bar
            // says whether the song is rolling in a way nothing else on screen does — the button
            // is a pause sign and the playhead is moving — and a menu row that had to be opened
            // to answer it would be answering a question nobody has.
            command(t(Key::CmdPlayStop), actions::TogglePlay, "transport.play"),
            command(
                t(Key::CmdReturnToZero),
                actions::ReturnToZero,
                "transport.return",
            ),
            command(
                t(Key::CmdStepBack),
                actions::StepBack,
                "transport.step_back",
            ),
            command(
                t(Key::CmdStepForward),
                actions::StepForward,
                "transport.step_forward",
            ),
            // The rest are ticked, and the reason is the same one every time: they are switches
            // left set from an hour ago, and the only other thing that says which way is a lit
            // glyph on a bar the user is not looking at when they open this menu.
            toggle(
                t(Key::CmdRecord),
                actions::ToggleRecording,
                "transport.record",
                state.recording,
            ),
            toggle(
                t(Key::CmdToggleMonitoring),
                actions::ToggleMonitoring,
                "transport.monitor",
                state.monitoring,
            ),
            toggle(
                t(Key::CmdTogglePunch),
                actions::TogglePunch,
                "transport.punch",
                state.punching,
            ),
            toggle(
                t(Key::CmdToggleCycle),
                actions::ToggleLoop,
                "transport.loop",
                state.looping,
            ),
            toggle(
                t(Key::CmdToggleMetronome),
                actions::ToggleMetronome,
                "transport.metronome",
                state.metronome,
            ),
            toggle(
                t(Key::CmdMusicalTyping),
                actions::ToggleMusicalTyping,
                "transport.musical_typing",
                state.musical_typing,
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
///
/// Built from a default state, and the ticks and the dimming it produces are dropped on the
/// floor, because gpui's [`MenuItem`] has room for neither. That is not a decision made here: the
/// menu belongs to the system and is handed over once, so even a `MenuItem` that could carry a
/// tick would need the whole menu re-set on every change of any of the eight facts in
/// [`MenuState`]. The window's own bar — which is what Windows and Linux see — draws both. When
/// gpui grows the field, this is the one function that has to change.
pub fn menus(language: Language) -> Vec<Menu> {
    model(language, &PanelLayout::default(), MenuState::default())
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

    /// The menu of a window nobody has touched: nothing switched on, nothing to undo.
    ///
    /// What the tests about *shape* — where a rule falls, which menu a row is in — want, because
    /// none of that depends on the state and passing one in would only be noise on every line.
    fn plain(language: Language) -> Vec<MenuSection> {
        model(language, &PanelLayout::default(), MenuState::default())
    }

    /// Whether the row labelled `label` carries a tick, or `None` when there is no such row.
    fn checked(sections: &[MenuSection], label: Key) -> Option<bool> {
        let wanted = label.get(Language::English);
        sections
            .iter()
            .flat_map(|section| &section.rows)
            .find_map(|row| match row {
                MenuRow::Command { label, checked, .. } if label == wanted => Some(*checked),
                _ => None,
            })
    }

    /// Whether the row labelled `label` can be chosen, or `None` when there is no such row.
    fn enabled(sections: &[MenuSection], label: Key) -> Option<bool> {
        let wanted = label.get(Language::English);
        sections
            .iter()
            .flat_map(|section| &section.rows)
            .find_map(|row| match row {
                MenuRow::Command { label, enabled, .. } if label == wanted => Some(*enabled),
                _ => None,
            })
    }

    #[test]
    fn a_switch_in_the_menu_says_which_way_it_is_set() {
        // The whole point of the tick: "Mixer" and "Metronome" are nouns, and a noun on its own
        // cannot answer the question somebody opened the menu to ask.
        let mut layout = PanelLayout::default();
        let off = plain(Language::English);
        assert_eq!(checked(&off, Key::CmdToggleMetronome), Some(false));

        let on = model(
            Language::English,
            &layout,
            MenuState {
                metronome: true,
                looping: true,
                ..MenuState::default()
            },
        );
        assert_eq!(checked(&on, Key::CmdToggleMetronome), Some(true));
        assert_eq!(checked(&on, Key::CmdToggleCycle), Some(true));
        // And only the ones that were switched on. A tick that followed the wrong field would
        // pass every test that looked at one row.
        assert_eq!(checked(&on, Key::CmdTogglePunch), Some(false));

        // The panels read from the layout rather than from the state, so they are worth their own
        // half of this: the two halves are wired separately and either could be wired to nothing.
        assert_eq!(
            checked(&off, Key::CmdShowMixer),
            Some(layout.is_open(Panel::Mixer))
        );
        layout.toggle(Panel::Mixer);
        assert_eq!(
            checked(
                &model(Language::English, &layout, MenuState::default()),
                Key::CmdShowMixer
            ),
            Some(layout.is_open(Panel::Mixer))
        );
    }

    #[test]
    fn undo_and_redo_are_dim_until_there_is_something_to_take_back() {
        let empty = plain(Language::English);
        assert_eq!(enabled(&empty, Key::CmdUndo), Some(false));
        assert_eq!(enabled(&empty, Key::CmdRedo), Some(false));

        let stacked = model(
            Language::English,
            &PanelLayout::default(),
            MenuState {
                can_undo: true,
                ..MenuState::default()
            },
        );
        assert_eq!(enabled(&stacked, Key::CmdUndo), Some(true));
        // Undoing does not fill the redo stack in this snapshot, and the menu must not pretend
        // it did: the two are separate questions and the session answers them separately.
        assert_eq!(enabled(&stacked, Key::CmdRedo), Some(false));

        // Nothing else in the menu is ever dim. If that changes, this is the test that should be
        // rewritten deliberately rather than the one that quietly starts passing for a new reason.
        let dim: Vec<String> = empty
            .iter()
            .flat_map(|section| &section.rows)
            .filter_map(|row| match row {
                MenuRow::Command {
                    label,
                    enabled: false,
                    ..
                } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            dim,
            vec![
                Key::CmdUndo.get(Language::English).to_string(),
                Key::CmdRedo.get(Language::English).to_string(),
            ]
        );
    }

    #[test]
    fn every_row_names_a_key_binding_that_exists() {
        // The in-window bar shows the keystroke beside each command by looking the id up. A
        // typo would silently leave the column blank rather than fail anywhere.
        for section in plain(Language::English) {
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
        let labels: Vec<String> = plain(Language::English)
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
        let labels: Vec<String> = plain(Language::English)
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
    fn both_ways_into_the_composer_are_in_a_menu_of_their_own() {
        // `Session::compose` was written with the composer and the desktop application never
        // called it, so the whole feature existed only for `auris compose`. Then it was one row
        // in File — carrying the label of the specification-file route, while dispatching the
        // song sheet, and with the file route itself in no menu at all. Both are named for what
        // they are now, under a heading somebody looking for the composer would open.
        let compose = plain(Language::English)
            .into_iter()
            .find(|section| section.name == Key::GroupCompose.get(Language::English))
            .expect("composing has a menu of its own");
        let labels: Vec<String> = compose
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        for expected in [
            Key::CmdComposeSong.get(Language::English),
            Key::CmdComposeFromSpec.get(Language::English),
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "`{expected}` is not in the Compose menu"
            );
        }
    }

    #[test]
    fn no_command_appears_twice() {
        let mut labels: Vec<String> = plain(Language::English)
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
        let system = plain(Language::English)
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
        for section in plain(Language::English) {
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
        let model_rows: usize = plain(Language::Japanese)
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

#[cfg(test)]
mod binding_tests {
    use super::*;

    /// Every keystroke a menu row shows is one the binding table actually has.
    ///
    /// `AurisApp::keystroke_for` answers an id nothing knows with an empty string, so a binding id
    /// that has been renamed or mistyped shows a row with no key beside it — which looks exactly
    /// like a command that was never given one. Both are silent, and only one is a mistake.
    ///
    /// An empty id is the deliberate case: Open Recent and About have no keystroke, because the
    /// list one opens is the point and every letter worth spending is spent.
    #[test]
    fn every_binding_a_menu_row_names_is_one_the_table_has() {
        for language in Language::ALL {
            for section in model(language, &PanelLayout::default(), MenuState::default()) {
                for row in &section.rows {
                    let MenuRow::Command { binding, label, .. } = row else {
                        continue;
                    };
                    assert!(
                        binding.is_empty() || crate::actions::bindable(binding).is_some(),
                        "the row `{label}` names a binding `{binding}` that no command has"
                    );
                }
            }
        }
    }

    /// Every menu, in every language, has rows in it.
    ///
    /// A section that came out empty would be a word on the bar that opens onto nothing — and the
    /// rows are built conditionally, so one is an edit away at any time.
    #[test]
    fn no_menu_is_a_title_with_nothing_under_it() {
        for language in Language::ALL {
            for section in model(language, &PanelLayout::default(), MenuState::default()) {
                assert!(
                    section
                        .rows
                        .iter()
                        .any(|row| !matches!(row, MenuRow::Separator)),
                    "the {} menu holds nothing but rules in {language:?}",
                    section.name
                );
            }
        }
    }

    /// A rule leads no menu and never doubles.
    ///
    /// The rows are grouped conditionally, so a group that comes out empty leaves its rule with
    /// nothing on one side — a menu that opens with a line across the top, or two together.
    #[test]
    fn no_menu_shows_a_stray_rule() {
        for language in Language::ALL {
            for section in model(language, &PanelLayout::default(), MenuState::default()) {
                let separators: Vec<usize> = section
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| matches!(row, MenuRow::Separator))
                    .map(|(index, _)| index)
                    .collect();
                assert!(
                    !separators.contains(&0),
                    "the {} menu opens with a rule",
                    section.name
                );
                assert!(
                    !separators.contains(&(section.rows.len() - 1)),
                    "the {} menu ends with a rule",
                    section.name
                );
                assert!(
                    separators.windows(2).all(|pair| pair[1] - pair[0] > 1),
                    "the {} menu has two rules together",
                    section.name
                );
            }
        }
    }
}
