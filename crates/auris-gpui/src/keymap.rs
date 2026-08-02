//! How input maps to commands: the user's key bindings and pointer gestures.
//!
//! Only *changes* are written to disk. Storing the full set would freeze today's defaults into
//! every existing settings file, so a later change to a default would silently not reach anyone
//! who had ever opened the settings window.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{App, KeyBinding};
use serde::{Deserialize, Serialize};

use crate::actions::{self, BINDABLE, Bindable};
use crate::gestures::PointerGestures;

/// Everything in `keymap.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct InputSettings {
    /// Key bindings the user has changed.
    pub keys: Keymap,
    /// What a click creates and what deletes.
    pub pointer: PointerGestures,
}

/// The file as it may be found on disk.
///
/// Before pointer gestures existed the file was a bare map of bindings, and a great many of
/// those files exist. Reading both shapes costs one enum and keeps a user's bindings when they
/// update; reading only the new shape would parse the old file as "no overrides" and quietly
/// throw them away.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredInput {
    Current(CurrentInput),
    Legacy(BTreeMap<String, String>),
}

/// The current shape.
///
/// `deny_unknown_fields` is what makes the two shapes distinguishable: without it a bare map of
/// bindings would match here as "every field defaulted" and the bindings would vanish.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentInput {
    #[serde(default)]
    keys: Keymap,
    #[serde(default)]
    pointer: PointerGestures,
}

impl InputSettings {
    /// Where the file lives.
    pub fn path() -> PathBuf {
        auris_session::config_dir().join("keymap.json")
    }

    /// Loads the file, falling back to the defaults.
    ///
    /// A missing file is a first run. A malformed one is logged and then also falls back,
    /// because refusing to start over a preferences file is a poor trade.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<StoredInput>(&text) {
            Ok(stored) => {
                let mut settings = Self::from(stored);
                settings.keys.discard_unusable();
                settings
            }
            Err(error) => {
                log::warn!("ignoring malformed {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the file, creating the configuration directory if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(path, text)
    }
}

impl From<StoredInput> for InputSettings {
    fn from(stored: StoredInput) -> Self {
        match stored {
            StoredInput::Current(current) => Self {
                keys: current.keys,
                pointer: current.pointer,
            },
            StoredInput::Legacy(overrides) => Self {
                keys: Keymap { overrides },
                pointer: PointerGestures::default(),
            },
        }
    }
}

/// Key bindings, as overrides on top of the defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, transparent)]
pub struct Keymap {
    /// Command id to keystroke, for the commands the user has changed.
    overrides: BTreeMap<String, String>,
}

impl Keymap {
    /// Drops overrides that name a command this build does not have, or a keystroke gpui
    /// cannot parse.
    ///
    /// The file is user-editable text and survives across versions, so both are reachable
    /// without anyone doing anything wrong — and an unparseable keystroke would otherwise
    /// panic inside `KeyBinding::new`.
    fn discard_unusable(&mut self) {
        self.overrides.retain(|id, keystroke| {
            let usable = actions::bindable(id).is_some() && actions::is_valid_keystroke(keystroke);
            if !usable {
                log::warn!("ignoring key binding `{id}` = `{keystroke}`");
            }
            usable
        });
    }

    /// The keystroke bound to `command`, whether default or overridden.
    ///
    /// As stored, which for an untouched command is the `secondary-` spelling. Use
    /// [`Keymap::display`] for anything a person reads.
    pub fn keystroke(&self, command: &Bindable) -> &str {
        self.overrides
            .get(command.id)
            .map(String::as_str)
            .unwrap_or(command.default)
    }

    /// The keystroke bound to `command`, written the way this platform writes it.
    ///
    /// `secondary-s` is how the default is stored and is not how anyone would say it out loud;
    /// the settings window shows `cmd-s` or `ctrl-s` depending on the keyboard in front of it.
    pub fn display(&self, command: &Bindable) -> String {
        actions::normalise_keystroke(self.keystroke(command))
    }

    /// `true` when the user has changed this command's binding.
    pub fn is_overridden(&self, command: &Bindable) -> bool {
        self.overrides.contains_key(command.id)
    }

    /// Binds `command` to `keystroke`, or clears the override when it matches the default.
    ///
    /// Returns `false` for a keystroke gpui cannot parse, leaving the binding untouched.
    ///
    /// "Matches the default" is decided after normalising, because the keyboard reports `cmd-s`
    /// where the table says `secondary-s`. Comparing them raw would write an override every
    /// time someone pressed a default back in, freezing today's defaults into their file.
    pub fn set(&mut self, command: &Bindable, keystroke: &str) -> bool {
        if !actions::is_valid_keystroke(keystroke) {
            return false;
        }
        if actions::normalise_keystroke(keystroke) == actions::normalise_keystroke(command.default)
        {
            self.overrides.remove(command.id);
        } else {
            self.overrides
                .insert(command.id.to_string(), keystroke.to_string());
        }
        true
    }

    /// Restores one command to its default.
    pub fn clear(&mut self, command: &Bindable) {
        self.overrides.remove(command.id);
    }

    /// Restores every command to its default.
    pub fn reset(&mut self) {
        self.overrides.clear();
    }

    /// Commands currently bound to `keystroke`, other than `except`.
    ///
    /// A conflict is shown rather than refused: two commands on one keystroke is a mistake
    /// worth pointing at, but the user may be in the middle of swapping a pair over.
    ///
    /// Compared after normalising, so a ⌘L the user just pressed is recognised as clashing with
    /// a default stored as `secondary-l`.
    pub fn conflicts(&self, keystroke: &str, except: &Bindable) -> Vec<&'static Bindable> {
        let wanted = actions::normalise_keystroke(keystroke);
        BINDABLE
            .iter()
            .filter(|other| other.id != except.id && self.display(other) == wanted)
            .collect()
    }

    /// Installs every binding into the application.
    pub fn apply(&self, cx: &mut App) {
        let bindings: Vec<KeyBinding> = BINDABLE
            .iter()
            .map(|command| command.binding(self.keystroke(command)))
            .collect();
        actions::install_bindings(cx, bindings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(id: &str) -> &'static Bindable {
        actions::bindable(id).expect("test refers to a real command")
    }

    #[test]
    fn an_empty_keymap_reports_the_defaults() {
        let keymap = Keymap::default();
        let play = command("transport.play");
        assert_eq!(keymap.keystroke(play), play.default);
        assert!(!keymap.is_overridden(play));
    }

    #[test]
    fn setting_a_binding_back_to_its_default_stops_being_an_override() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");

        assert!(keymap.set(play, "cmd-p"));
        assert_eq!(keymap.keystroke(play), "cmd-p");
        assert!(keymap.is_overridden(play));

        assert!(keymap.set(play, play.default));
        assert!(
            !keymap.is_overridden(play),
            "an override equal to the default would freeze today's default into the file"
        );
    }

    #[test]
    fn pressing_a_default_back_in_is_recognised_through_its_platform_spelling() {
        // The table stores `secondary-s`; the keyboard reports `cmd-s` or `ctrl-s`. Comparing
        // them raw would store an override identical to the default, which is exactly what the
        // "only overrides are written" rule exists to prevent.
        let mut keymap = Keymap::default();
        let save = command("file.save");
        assert!(keymap.set(save, "shift-f7"));
        assert!(keymap.is_overridden(save));

        assert!(keymap.set(save, &actions::normalise_keystroke(save.default)));
        assert!(
            !keymap.is_overridden(save),
            "a default pressed on the keyboard was stored as an override"
        );
    }

    #[test]
    fn a_conflict_is_found_through_its_platform_spelling() {
        let keymap = Keymap::default();
        let save = command("file.save");
        let undo = command("edit.undo");
        // `file.save` is bound to the default `secondary-s`; asking about it in the form the
        // keyboard produces must still find it.
        let clash = keymap.conflicts(&actions::normalise_keystroke(save.default), undo);
        assert_eq!(clash.len(), 1);
        assert_eq!(clash[0].id, save.id);
    }

    #[test]
    fn what_is_displayed_is_what_the_platform_calls_it() {
        let keymap = Keymap::default();
        let expected = if cfg!(target_os = "macos") {
            "cmd-s"
        } else {
            "ctrl-s"
        };
        assert_eq!(keymap.display(command("file.save")), expected);
        assert_eq!(
            keymap.keystroke(command("file.save")),
            "secondary-s",
            "the stored form stays portable; only the display is localised to the keyboard"
        );
    }

    #[test]
    fn an_unparseable_keystroke_is_refused_and_changes_nothing() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        assert!(!keymap.set(play, "notakey-x"));
        assert_eq!(keymap.keystroke(play), play.default);
    }

    #[test]
    fn conflicts_report_the_other_command_only() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        let undo = command("edit.undo");

        assert!(keymap.conflicts(keymap.keystroke(play), play).is_empty());

        keymap.set(undo, play.default);
        let clash = keymap.conflicts(play.default, play);
        assert_eq!(clash.len(), 1);
        assert_eq!(clash[0].id, undo.id);
        // And the command doing the asking is never its own conflict.
        assert!(
            keymap
                .conflicts(play.default, undo)
                .iter()
                .all(|c| c.id != undo.id)
        );
    }

    #[test]
    fn loading_discards_bindings_this_build_cannot_use() {
        let mut keymap = Keymap {
            overrides: BTreeMap::from([
                ("transport.play".to_string(), "cmd-p".to_string()),
                ("gone.in.a.later.build".to_string(), "cmd-g".to_string()),
                ("edit.undo".to_string(), "notakey-x".to_string()),
            ]),
        };
        keymap.discard_unusable();

        assert_eq!(keymap.keystroke(command("transport.play")), "cmd-p");
        assert!(!keymap.is_overridden(command("edit.undo")));
        assert_eq!(keymap.overrides.len(), 1);
    }

    #[test]
    fn a_file_written_before_pointer_gestures_existed_keeps_its_bindings() {
        // The old shape was a bare map. Parsing it as the new shape must not quietly yield
        // "no overrides", which is what would happen without the untagged fallback.
        let legacy = r#"{"transport.play":"cmd-p","view.zoom_in":"cmd-shift-="}"#;
        let stored: StoredInput = serde_json::from_str(legacy).unwrap();
        let settings = InputSettings::from(stored);

        assert_eq!(settings.keys.keystroke(command("transport.play")), "cmd-p");
        assert_eq!(
            settings.keys.keystroke(command("view.zoom_in")),
            "cmd-shift-="
        );
        assert_eq!(
            settings.pointer,
            PointerGestures::default(),
            "an old file has no gestures, so it gets the defaults"
        );
    }

    #[test]
    fn the_current_shape_round_trips() {
        let mut settings = InputSettings::default();
        settings.keys.set(command("transport.play"), "cmd-p");
        settings
            .pointer
            .set_create(crate::gestures::PointerGesture::OptionClick);

        let text = serde_json::to_string(&settings).unwrap();
        let restored = InputSettings::from(serde_json::from_str::<StoredInput>(&text).unwrap());
        assert_eq!(restored, settings);
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let settings = InputSettings::from(serde_json::from_str::<StoredInput>("{}").unwrap());
        assert_eq!(settings, InputSettings::default());
    }

    #[test]
    fn only_overrides_are_written() {
        let mut keymap = Keymap::default();
        assert_eq!(serde_json::to_string(&keymap).unwrap(), "{}");

        keymap.set(command("view.zoom_in"), "cmd-shift-=");
        let text = serde_json::to_string(&keymap).unwrap();
        assert_eq!(text, r#"{"view.zoom_in":"cmd-shift-="}"#);
        assert_eq!(serde_json::from_str::<Keymap>(&text).unwrap(), keymap);
    }

    #[test]
    fn reset_restores_every_default() {
        let mut keymap = Keymap::default();
        keymap.set(command("transport.play"), "cmd-p");
        keymap.set(command("edit.undo"), "cmd-shift-u");
        keymap.reset();
        for entry in BINDABLE {
            assert_eq!(keymap.keystroke(entry), entry.default);
        }
    }
}
