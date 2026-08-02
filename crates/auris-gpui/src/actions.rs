//! Application actions and their default key bindings.
//!
//! Actions are gpui's routed commands: the menu bar, the keymap and the buttons in the UI all
//! dispatch the same action type, so a feature is bound once and reachable three ways.

use auris_i18n::Key;
use gpui::{App, KeyBinding, actions};

actions!(
    auris,
    [
        /// Quit the application.
        Quit,
        /// Create a new empty project.
        NewProject,
        /// Open a project file.
        OpenProject,
        /// Save the current project.
        SaveProject,
        /// Save the current project under a new name.
        SaveProjectAs,
        /// Import an audio file onto a new audio track.
        ImportAudio,
        /// Render the project to a WAV file.
        ExportAudio,
        /// Start or stop playback.
        TogglePlay,
        /// Stop playback and return to the start.
        StopPlayback,
        /// Move the playhead to the beginning.
        ReturnToZero,
        /// Toggle looping over the loop region.
        ToggleLoop,
        /// Add an instrument track.
        AddInstrumentTrack,
        /// Add an audio track.
        AddAudioTrack,
        /// Delete the selected track.
        DeleteTrack,
        /// Delete the current selection.
        DeleteSelection,
        /// Undo the last edit.
        Undo,
        /// Redo the last undone edit.
        Redo,
        /// Silence every voice and reset the engine.
        PanicStop,
        /// Zoom the timeline in.
        ZoomIn,
        /// Zoom the timeline out.
        ZoomOut,
        /// Show or hide the right-hand inspector.
        ToggleInspector,
        /// Show or hide the bottom editor panel.
        ToggleEditor,
        /// Open the settings window.
        OpenSettings,
    ]
);

/// One command the user can rebind.
///
/// `bind` exists because [`KeyBinding::new`] needs a concrete action type, so a table of
/// `Box<dyn Action>` would not do — each entry carries its own constructor instead.
pub struct Bindable {
    /// Stable identifier written to the settings file. Never change a released one.
    pub id: &'static str,
    /// Section heading in the settings window.
    pub group: Key,
    /// Name shown to the user.
    pub label: Key,
    /// Keystroke used when the user has not chosen one.
    pub default: &'static str,
    bind: fn(&str) -> KeyBinding,
}

impl Bindable {
    /// Builds the binding for `keystroke`.
    pub fn binding(&self, keystroke: &str) -> KeyBinding {
        (self.bind)(keystroke)
    }
}

/// Key context the bindings are scoped to.
///
/// Scoped rather than global so a text field can switch them off wholesale: with `space` bound
/// everywhere, typing a space into a rename box would start playback instead.
pub const KEY_CONTEXT: &str = "Auris";

macro_rules! bindable {
    ($($id:literal, $group:ident, $label:ident, $default:literal => $action:ident;)*) => {
        /// Every command the settings window offers to rebind, in display order.
        pub const BINDABLE: &[Bindable] = &[
            $(Bindable {
                id: $id,
                group: Key::$group,
                label: Key::$label,
                default: $default,
                bind: |keys| KeyBinding::new(keys, $action, Some(KEY_CONTEXT)),
            },)*
        ];
    };
}

bindable! {
    "transport.play",       GroupTransport, CmdPlayStop,           "space"       => TogglePlay;
    "transport.return",     GroupTransport, CmdReturnToZero,       "enter"       => ReturnToZero;
    "transport.loop",       GroupTransport, CmdToggleCycle,        "cmd-l"       => ToggleLoop;
    "transport.panic",      GroupTransport, CmdPanic,              "escape"      => PanicStop;

    "file.new",             GroupFile,      CmdNewProject,         "cmd-n"       => NewProject;
    "file.open",            GroupFile,      CmdOpenProject,        "cmd-o"       => OpenProject;
    "file.save",            GroupFile,      CmdSave,               "cmd-s"       => SaveProject;
    "file.save_as",         GroupFile,      CmdSaveAs,             "cmd-shift-s" => SaveProjectAs;
    "file.import",          GroupFile,      CmdImportAudio,        "cmd-i"       => ImportAudio;
    "file.export",          GroupFile,      CmdExportWav,          "cmd-e"       => ExportAudio;
    "file.quit",            GroupFile,      CmdQuit,               "cmd-q"       => Quit;

    "edit.undo",            GroupEdit,      CmdUndo,               "cmd-z"       => Undo;
    "edit.redo",            GroupEdit,      CmdRedo,               "cmd-shift-z" => Redo;
    "edit.delete",          GroupEdit,      CmdDeleteSelection,    "backspace"   => DeleteSelection;

    "track.add_instrument", GroupTrack,     CmdAddInstrumentTrack, "cmd-t"       => AddInstrumentTrack;
    "track.add_audio",      GroupTrack,     CmdAddAudioTrack,      "cmd-shift-t" => AddAudioTrack;
    "track.delete",         GroupTrack,     CmdDeleteTrack,        "cmd-backspace" => DeleteTrack;

    "view.inspector",       GroupView,      CmdShowInspector,      "i"           => ToggleInspector;
    "view.editor",          GroupView,      CmdShowEditor,         "p"           => ToggleEditor;
    "view.zoom_in",         GroupView,      CmdZoomIn,             "cmd-="       => ZoomIn;
    "view.zoom_out",        GroupView,      CmdZoomOut,            "cmd--"       => ZoomOut;
    "view.settings",        GroupView,      CmdSettings,           "cmd-,"       => OpenSettings;
}

/// The bindable command with this id.
pub fn bindable(id: &str) -> Option<&'static Bindable> {
    BINDABLE.iter().find(|entry| entry.id == id)
}

/// Whether `keystroke` is something the user could actually press.
///
/// Checked before anything is bound, because [`KeyBinding::new`] panics on a malformed
/// keystroke and a settings file is user-editable text. Modifiers alone are rejected too —
/// gpui parses `cmd-` happily, but a binding with no key can never fire.
pub fn is_valid_keystroke(keystroke: &str) -> bool {
    let mut chunks = keystroke.split_whitespace().peekable();
    if chunks.peek().is_none() {
        return false;
    }
    chunks
        .all(|chunk| gpui::Keystroke::parse(chunk).is_ok_and(|keystroke| !keystroke.key.is_empty()))
}

/// Installs `bindings`, replacing whatever was bound before.
///
/// Rebinding replaces rather than adds: `cx.bind_keys` appends, so without the clear an old
/// keystroke would keep working alongside the new one.
pub fn install_bindings(cx: &mut App, bindings: impl IntoIterator<Item = KeyBinding>) {
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_bindable_id_and_default_is_usable() {
        let mut ids = BTreeSet::new();
        for entry in BINDABLE {
            assert!(ids.insert(entry.id), "duplicate id `{}`", entry.id);
            assert!(
                is_valid_keystroke(entry.default),
                "`{}` has an unparseable default `{}`",
                entry.id,
                entry.default
            );
            assert_eq!(bindable(entry.id).map(|e| e.id), Some(entry.id));
        }
    }

    #[test]
    fn no_two_commands_share_a_default_keystroke() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for entry in BINDABLE {
            assert!(
                seen.insert(entry.default),
                "`{}` collides with another default on `{}`",
                entry.id,
                entry.default
            );
        }
    }

    #[test]
    fn malformed_keystrokes_are_rejected_before_they_can_panic() {
        assert!(is_valid_keystroke("cmd-shift-s"));
        assert!(is_valid_keystroke("g g"));
        assert!(!is_valid_keystroke(""));
        assert!(!is_valid_keystroke("   "));
        assert!(!is_valid_keystroke("notakey-x"));
        // gpui parses this, but a binding with no key can never fire.
        assert!(!is_valid_keystroke("cmd-"));
    }
}
