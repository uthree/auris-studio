//! Application actions and their default key bindings.
//!
//! Actions are gpui's routed commands: the menu bar, the keymap and the buttons in the UI all
//! dispatch the same action type, so a feature is bound once and reachable three ways.

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
    ]
);

/// Installs the default key bindings.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-n", NewProject, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("cmd-s", SaveProject, None),
        KeyBinding::new("cmd-shift-s", SaveProjectAs, None),
        KeyBinding::new("cmd-i", ImportAudio, None),
        KeyBinding::new("cmd-e", ExportAudio, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-t", AddInstrumentTrack, None),
        KeyBinding::new("cmd-shift-t", AddAudioTrack, None),
        // Space is the universal DAW play/stop toggle.
        KeyBinding::new("space", TogglePlay, None),
        KeyBinding::new("enter", ReturnToZero, None),
        KeyBinding::new("cmd-l", ToggleLoop, None),
        KeyBinding::new("escape", PanicStop, None),
        KeyBinding::new("backspace", DeleteSelection, None),
        KeyBinding::new("delete", DeleteSelection, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        // Bare letters, as Logic binds them: there is no text field to steal them.
        KeyBinding::new("i", ToggleInspector, None),
        KeyBinding::new("p", ToggleEditor, None),
    ]);
}
