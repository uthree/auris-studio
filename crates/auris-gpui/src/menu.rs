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
use gpui::{Action, Menu, MenuItem, SharedString, SystemMenuType, actions};

use crate::actions;
use crate::dock::{Panel, PanelLayout};
use auris_session::prelude::{CC_MODULATION, ClipCurve};

actions!(auris_native_menu, [Unavailable]);

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
    /// Stable identity for configurable title-bar shortcuts; the macOS application menu has none.
    pub category: Option<MenuCategory>,
    /// Title shown on the bar.
    pub name: SharedString,
    /// The rows, in order.
    pub rows: Vec<MenuRow>,
}

/// Categories that can be shown directly in the editor title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCategory {
    /// Project files and import/export.
    File,
    /// Selection and musical editing.
    Edit,
    /// Track commands.
    Track,
    /// Accompaniment and level balancing.
    Compose,
    /// Panels, lanes and command search.
    View,
    /// Playback and recording.
    Transport,
    /// Auxiliary windows and project tabs.
    Window,
    /// Music and audio analysis.
    Analysis,
    /// Documentation and application information.
    Help,
}

impl MenuCategory {
    /// Categories in their conventional menu order.
    pub const ALL: [Self; 9] = [
        Self::File,
        Self::Edit,
        Self::Track,
        Self::Compose,
        Self::View,
        Self::Transport,
        Self::Window,
        Self::Analysis,
        Self::Help,
    ];

    /// Stable preference identifier, independent of language and platform menu offsets.
    pub fn id(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Edit => "edit",
            Self::Track => "track",
            Self::Compose => "compose",
            Self::View => "view",
            Self::Transport => "transport",
            Self::Window => "window",
            Self::Analysis => "analysis",
            Self::Help => "help",
        }
    }

    /// Localized category title.
    pub fn label(self) -> Key {
        match self {
            Self::File => Key::GroupFile,
            Self::Edit => Key::GroupEdit,
            Self::Track => Key::GroupTrack,
            Self::Compose => Key::GroupCompose,
            Self::View => Key::GroupView,
            Self::Transport => Key::GroupTransport,
            Self::Window => Key::MenuWindow,
            Self::Analysis => Key::GroupAnalysis,
            Self::Help => Key::GroupHelp,
        }
    }
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
            category: None,
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
                command(t(Key::MenuHideApp), actions::HideApplication, ""),
                command(t(Key::MenuHideOthers), actions::HideOtherApplications, ""),
                command(t(Key::MenuShowAll), actions::ShowAllApplications, ""),
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
        command(
            t(Key::CmdCloseTab),
            actions::CloseProjectTab,
            "file.close_tab",
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
        command(
            t(Key::CmdExportVideo),
            actions::ExportVideo,
            "file.export_video",
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
        category: Some(MenuCategory::File),
        name: t(Key::GroupFile),
        rows: file,
    });

    sections.push(MenuSection {
        category: Some(MenuCategory::Edit),
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
        category: Some(MenuCategory::Track),
        name: t(Key::GroupTrack),
        rows: vec![
            command(
                t(Key::CmdAddInstrumentTrack),
                actions::AddInstrumentTrack,
                "track.add_instrument",
            ),
            command(
                t(Key::CmdAddDrumTrack),
                actions::AddDrumTrack,
                "track.add_drum",
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
                t(Key::CmdChooseVoice),
                actions::ChooseSingerVoice,
                "track.choose_voice",
            ),
            command(
                t(Key::CmdNextSpeaker),
                actions::NextSingerSpeaker,
                "track.next_speaker",
            ),
            command(t(Key::CmdSing), actions::Sing, "track.sing"),
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

    sections.push(MenuSection {
        category: Some(MenuCategory::Compose),
        name: t(Key::GroupCompose),
        rows: vec![
            command(
                t(Key::CmdAccompanyMelody),
                actions::AccompanyMelody,
                "file.accompany",
            ),
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
        category: Some(MenuCategory::View),
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
                t(Key::CmdShowLyricsLane),
                actions::ToggleLyricsLane,
                "view.lyrics_lane",
                panels.lanes.lyrics,
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
            command(
                t(Key::CmdIncreaseTrackHeight),
                actions::IncreaseTrackHeight,
                "view.track_height_increase",
            ),
            command(
                t(Key::CmdDecreaseTrackHeight),
                actions::DecreaseTrackHeight,
                "view.track_height_decrease",
            ),
        ],
    });

    sections.push(MenuSection {
        category: Some(MenuCategory::Transport),
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

    sections.push(MenuSection {
        category: Some(MenuCategory::Window),
        name: t(Key::MenuWindow),
        // A Mac's Window menu opens with the window itself, whatever else the application adds.
        rows: cfg!(target_os = "macos")
            .then(|| {
                [
                    command(t(Key::MenuMinimize), actions::MinimizeWindow, ""),
                    command(t(Key::MenuZoom), actions::ZoomWindow, ""),
                    MenuRow::Separator,
                ]
            })
            .into_iter()
            .flatten()
            .chain(
                Panel::ALL
                    .into_iter()
                    .map(|panel| {
                        toggle(
                            t(panel.label()),
                            crate::auxiliary_window::TogglePanelWindow { panel },
                            "",
                            panels.is_detached(panel),
                        )
                    })
                    .chain([
                        command(
                            t(Key::VisualizerTitle),
                            actions::ToggleVisualizer,
                            "visualizer.toggle",
                        ),
                        MenuRow::Separator,
                        command(t(Key::CmdNextTab), actions::NextProjectTab, "view.next_tab"),
                        command(
                            t(Key::CmdPreviousTab),
                            actions::PreviousProjectTab,
                            "view.previous_tab",
                        ),
                    ]),
            )
            .collect(),
    });

    sections.push(MenuSection {
        category: Some(MenuCategory::Analysis),
        name: t(Key::GroupAnalysis),
        rows: vec![
            command(
                t(Key::MenuAnalyzeChords),
                actions::AnalyzeSelectedChords,
                "analysis.chords",
            ),
            command(
                t(Key::MenuAnalyzeAllChords),
                actions::AnalyzeAllChords,
                "analysis.all_chords",
            ),
            MenuRow::Separator,
            command(
                t(Key::MenuAnalyzeAudio),
                actions::AnalyzeSelectedAudio,
                "analysis.audio",
            ),
            command(
                t(Key::MenuAnalyzeInstruments),
                actions::AnalyzeSelectedInstruments,
                "analysis.instruments",
            ),
            MenuRow::Separator,
            command(
                t(Key::AnalysisDrums),
                actions::OpenDrumAnalysis,
                "analysis.drums",
            ),
            command(
                t(Key::AnalysisResults),
                actions::OpenAnalysisResults,
                "analysis.results",
            ),
        ],
    });
    sections.push(MenuSection {
        category: Some(MenuCategory::Help),
        name: t(Key::GroupHelp),
        rows: vec![
            command(
                t(Key::IssueReport),
                actions::ReportProblem,
                "help.report_problem",
            ),
            command(
                t(Key::CmdCheckForUpdates),
                actions::CheckForUpdates,
                "help.check_updates",
            ),
            MenuRow::Separator,
            command(t(Key::CmdAbout), actions::ShowAbout, "view.about"),
        ],
    });
    sections
}

/// The menu in the shape gpui hands to the operating system.
///
/// Rebuilt when its state changes rather than re-rendered: the menu bar belongs to the operating
/// system, so nothing about a window redraw would touch it.
///
/// gpui's [`MenuItem`] has no checked field, so a checked command receives the conventional
/// leading tick in its label. A disabled command carries an action with no handler; gpui's native
/// menu validation therefore dims it and refuses to dispatch it.
pub fn menus(language: Language, panels: &PanelLayout, state: MenuState) -> Vec<Menu> {
    model(language, panels, state)
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
                    MenuRow::Command {
                        label,
                        action,
                        enabled,
                        checked,
                        ..
                    } => MenuItem::Action {
                        name: native_label(label, checked),
                        action: if enabled {
                            action
                        } else {
                            Box::new(Unavailable)
                        },
                        os_action: None,
                    },
                    MenuRow::System { label, menu } => MenuItem::os_submenu(label, menu),
                })
                .collect(),
        })
        .collect()
}

fn native_label(label: SharedString, checked: bool) -> SharedString {
    if checked {
        format!("✓ {label}").into()
    } else {
        label
    }
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

    #[test]
    fn native_labels_show_the_state_gpui_cannot_carry() {
        assert_eq!(native_label("Mixer".into(), false), "Mixer");
        assert_eq!(native_label("Mixer".into(), true), "✓ Mixer");
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
    fn every_named_key_binding_exists() {
        // The in-window bar shows the keystroke beside each command by looking the id up. A
        // typo would silently leave the column blank rather than fail anywhere.
        for section in plain(Language::English) {
            for row in section.rows {
                if let MenuRow::Command { label, binding, .. } = row {
                    assert!(
                        binding.is_empty() || actions::bindable(binding).is_some(),
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
    fn help_menu_exposes_the_release_check_and_build_information() {
        let help = plain(Language::English)
            .into_iter()
            .find(|section| section.name == Key::GroupHelp.get(Language::English))
            .expect("Help menu");
        let labels: Vec<_> = help
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                Key::IssueReport.get(Language::English),
                Key::CmdCheckForUpdates.get(Language::English),
                Key::CmdAbout.get(Language::English),
            ]
        );
    }

    #[test]
    fn composition_menu_exposes_accompaniment_and_mix_balance() {
        let compose = plain(Language::English)
            .into_iter()
            .find(|section| section.name == Key::GroupCompose.get(Language::English))
            .expect("Compose menu");
        let labels: Vec<_> = compose
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Command { label, .. } => Some(label.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                Key::CmdAccompanyMelody.get(Language::English),
                Key::CmdBalanceLevels.get(Language::English),
            ]
        );
    }

    #[test]
    fn no_command_appears_twice() {
        let actions: Vec<Box<dyn Action>> = plain(Language::English)
            .into_iter()
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                MenuRow::Command { action, .. } => Some(action),
                _ => None,
            })
            .collect();
        for (index, action) in actions.iter().enumerate() {
            assert!(
                !actions[..index]
                    .iter()
                    .any(|other| action.partial_eq(other.as_ref())),
                "{} is in two menus at once",
                action.name()
            );
        }
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
        let menu_items: usize = menus(
            Language::Japanese,
            &PanelLayout::default(),
            MenuState::default(),
        )
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
    /// Open Recent and About have empty default keystrokes, but their menu rows still carry real
    /// binding ids so a user-defined binding can appear here.
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
