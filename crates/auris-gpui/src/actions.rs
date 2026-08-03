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
        /// Write a piece from a song specification, replacing the document.
        ComposeSong,
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
        /// Show or hide the left-hand library.
        ToggleLibrary,
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

// `secondary` rather than `cmd`: gpui resolves it to ⌘ on macOS and to Ctrl everywhere else,
// which is what each platform's users already have in their fingers. Writing `cmd` would bind
// the Windows key off a Mac, and the shell takes most of those combinations first.
bindable! {
    "transport.play",       GroupTransport, CmdPlayStop,           "space"       => TogglePlay;
    "transport.return",     GroupTransport, CmdReturnToZero,       "enter"       => ReturnToZero;
    "transport.loop",       GroupTransport, CmdToggleCycle,        "secondary-l" => ToggleLoop;
    "transport.panic",      GroupTransport, CmdPanic,              "escape"      => PanicStop;

    "file.new",             GroupFile,      CmdNewProject,         "secondary-n" => NewProject;
    "file.open",            GroupFile,      CmdOpenProject,        "secondary-o" => OpenProject;
    "file.compose",         GroupFile,      CmdComposeSong,        "secondary-shift-c" => ComposeSong;
    "file.save",            GroupFile,      CmdSave,               "secondary-s" => SaveProject;
    "file.save_as",         GroupFile,      CmdSaveAs,             "secondary-shift-s" => SaveProjectAs;
    "file.import",          GroupFile,      CmdImportAudio,        "secondary-i" => ImportAudio;
    "file.export",          GroupFile,      CmdExportWav,          "secondary-e" => ExportAudio;
    "file.quit",            GroupFile,      CmdQuit,               "secondary-q" => Quit;

    "edit.undo",            GroupEdit,      CmdUndo,               "secondary-z" => Undo;
    "edit.redo",            GroupEdit,      CmdRedo,               "secondary-shift-z" => Redo;
    "edit.delete",          GroupEdit,      CmdDeleteSelection,    "backspace"   => DeleteSelection;

    "track.add_instrument", GroupTrack,     CmdAddInstrumentTrack, "secondary-t" => AddInstrumentTrack;
    "track.add_audio",      GroupTrack,     CmdAddAudioTrack,      "secondary-shift-t" => AddAudioTrack;
    "track.delete",         GroupTrack,     CmdDeleteTrack,        "secondary-backspace" => DeleteTrack;

    // `y` is Logic's own Library key, and it is free here.
    "view.library",         GroupView,      CmdShowLibrary,        "y"           => ToggleLibrary;
    "view.inspector",       GroupView,      CmdShowInspector,      "i"           => ToggleInspector;
    "view.editor",          GroupView,      CmdShowEditor,         "p"           => ToggleEditor;
    "view.zoom_in",         GroupView,      CmdZoomIn,             "secondary-=" => ZoomIn;
    "view.zoom_out",        GroupView,      CmdZoomOut,            "secondary--" => ZoomOut;
    "view.settings",        GroupView,      CmdSettings,           "secondary-," => OpenSettings;
}

/// The bindable command with this id.
pub fn bindable(id: &str) -> Option<&'static Bindable> {
    BINDABLE.iter().find(|entry| entry.id == id)
}

/// `keystroke` written the way this platform writes it.
///
/// The defaults above say `secondary-`, which gpui resolves to ⌘ on macOS and to Ctrl
/// everywhere else; a keystroke captured from the keyboard arrives already resolved. The two
/// forms name the same key and compare unequal as text, so a ⌘L the user pressed would neither
/// count as "back to the default" nor be reported as clashing with one. Everything that
/// compares or displays a keystroke goes through here first.
///
/// A chunk gpui cannot parse is passed through untouched rather than dropped: this is used on
/// the display path, and showing the user what is actually in their settings file beats showing
/// them nothing.
pub fn normalise_keystroke(keystroke: &str) -> String {
    keystroke
        .split_whitespace()
        .map(|chunk| {
            gpui::Keystroke::parse(chunk)
                .map(|parsed| parsed.unparse())
                .unwrap_or_else(|_| chunk.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `keystroke` as a menu would print it.
///
/// `ctrl-shift-s` is the form gpui parses; it is not a form anyone would put in a menu. macOS
/// stacks glyphs in a fixed order and leaves out the separators, and Windows spells the
/// modifiers out and joins them with plus signs, so each platform gets its own.
pub fn menu_keystroke(keystroke: &str) -> String {
    normalise_keystroke(keystroke)
        .split_whitespace()
        .map(pretty_chunk)
        .collect::<Vec<_>>()
        .join(" ")
}

/// One keystroke of a possibly multi-stroke binding, prettified.
fn pretty_chunk(chunk: &str) -> String {
    let Ok(parsed) = gpui::Keystroke::parse(chunk) else {
        return chunk.to_string();
    };
    let modifiers = parsed.modifiers;
    let key = pretty_key(&parsed.key);

    if cfg!(target_os = "macos") {
        // ⌃⌥⇧⌘ then the key, which is the order Apple prints them in and the order people
        // read them in without thinking about it.
        let mut out = String::new();
        for (held, glyph) in [
            (modifiers.control, '⌃'),
            (modifiers.alt, '⌥'),
            (modifiers.shift, '⇧'),
            (modifiers.platform, '⌘'),
        ] {
            if held {
                out.push(glyph);
            }
        }
        out.push_str(&key);
        return out;
    }

    let mut parts: Vec<&str> = Vec::new();
    for (held, name) in [
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        // Never produced by a default, but a user can bind it and an empty gap would be worse.
        (modifiers.platform, "Win"),
    ] {
        if held {
            parts.push(name);
        }
    }
    parts.push(&key);
    parts.join("+")
}

/// A key's name as a menu prints it.
fn pretty_key(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let named = match key {
        "backspace" if mac => "⌫",
        "backspace" => "Backspace",
        "delete" if mac => "⌦",
        "delete" => "Del",
        "enter" if mac => "⏎",
        "enter" => "Enter",
        "escape" if mac => "⎋",
        "escape" => "Esc",
        "tab" if mac => "⇥",
        "tab" => "Tab",
        "space" => "Space",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    // A letter is shown in upper case — `⌘S`, never `⌘s` — and punctuation is left alone, so
    // `=` does not become something unrecognisable. Anything longer keeps its shape with a
    // capital, which is what turns `f1` into `F1`.
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
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
        // Compared as this platform writes them: two defaults spelled differently could still
        // resolve to the same physical keys.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for entry in BINDABLE {
            assert!(
                seen.insert(normalise_keystroke(entry.default)),
                "`{}` collides with another default on `{}`",
                entry.id,
                entry.default
            );
        }
    }

    #[test]
    fn no_default_names_a_key_this_platform_lacks() {
        // `cmd` is gpui's *platform* modifier, which off a Mac is the Windows or Super key —
        // reserved by the shell, so the binding would simply never fire.
        for entry in BINDABLE {
            assert!(
                !entry.default.contains("cmd-")
                    && !entry.default.contains("super-")
                    && !entry.default.contains("win-"),
                "`{}` binds a platform key directly; use `secondary-`",
                entry.id
            );
        }
    }

    #[test]
    fn normalising_makes_the_two_spellings_of_a_default_agree() {
        // What the table says and what the keyboard reports must land on the same string, or
        // conflict detection and "is this the default?" both quietly stop working.
        let native = if cfg!(target_os = "macos") {
            "cmd-l"
        } else {
            "ctrl-l"
        };
        assert_eq!(normalise_keystroke("secondary-l"), native);
        assert_eq!(normalise_keystroke(native), native);
        assert_eq!(normalise_keystroke("g g"), "g g");
        // Every default survives the round trip, punctuation and all.
        for entry in BINDABLE {
            let once = normalise_keystroke(entry.default);
            assert_eq!(
                normalise_keystroke(&once),
                once,
                "`{}` does not normalise to a fixed point",
                entry.id
            );
            assert!(
                is_valid_keystroke(&once),
                "`{}` normalises to an unbindable `{once}`",
                entry.id
            );
        }
    }

    #[test]
    fn a_menu_prints_keystrokes_the_way_the_platform_does() {
        if cfg!(target_os = "macos") {
            assert_eq!(menu_keystroke("secondary-s"), "⌘S");
            assert_eq!(menu_keystroke("secondary-shift-s"), "⇧⌘S");
            assert_eq!(menu_keystroke("secondary-backspace"), "⌘⌫");
            assert_eq!(menu_keystroke("escape"), "⎋");
        } else {
            assert_eq!(menu_keystroke("secondary-s"), "Ctrl+S");
            assert_eq!(menu_keystroke("secondary-shift-s"), "Ctrl+Shift+S");
            assert_eq!(menu_keystroke("secondary-backspace"), "Ctrl+Backspace");
            assert_eq!(menu_keystroke("escape"), "Esc");
        }
        assert_eq!(menu_keystroke("space"), "Space");
        assert_eq!(menu_keystroke("g g"), "G G");
    }

    #[test]
    fn every_default_prints_as_something_a_person_can_read() {
        for entry in BINDABLE {
            let printed = menu_keystroke(entry.default);
            assert!(
                !printed.is_empty(),
                "`{}` prints as nothing at all",
                entry.id
            );
            assert!(
                !printed.contains('-') || printed.contains("Ctrl+-") || printed.ends_with('-'),
                "`{}` still prints in gpui's own syntax: `{printed}`",
                entry.id
            );
            assert!(
                !printed.contains("secondary"),
                "`{}` leaks the portable spelling into the menu: `{printed}`",
                entry.id
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
