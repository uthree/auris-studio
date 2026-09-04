//! How input maps to commands: the user's key bindings and pointer gestures.
//!
//! Only *changes* are written to disk. Storing the full set would freeze today's defaults into
//! every existing settings file, so a later change to a default would silently not reach anyone
//! who had ever opened the settings window.

use std::collections::BTreeMap;
use std::path::PathBuf;

use auris_i18n::Key;
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
    Legacy(Keymap),
}

/// Whether a stored list says exactly what this build already ships for the command.
fn matches_defaults(command: &Bindable, keystrokes: &[String]) -> bool {
    let shipped: Vec<String> = command
        .defaults()
        .map(actions::normalise_keystroke)
        .collect();
    let written: Vec<String> = keystrokes
        .iter()
        .map(|keystroke| actions::normalise_keystroke(keystroke))
        .collect();
    written == shipped
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
            StoredInput::Legacy(keys) => Self {
                keys,
                pointer: PointerGestures::default(),
            },
        }
    }
}

/// One command's overridden bindings, as a settings file may spell them.
///
/// A single keystroke was the only shape until a command could hold more than one, and files
/// written then are still on disk. Reading both costs one enum; reading only the list would parse
/// `"cmd-p"` as a malformed entry and drop a binding the user had chosen.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredBinding {
    One(String),
    Many(Vec<String>),
}

impl From<StoredBinding> for Vec<String> {
    fn from(stored: StoredBinding) -> Self {
        match stored {
            StoredBinding::One(keystroke) => vec![keystroke],
            StoredBinding::Many(keystrokes) => keystrokes,
        }
    }
}

/// Key bindings, as overrides on top of the defaults.
///
/// A command absent from the map keeps its default. A command mapped to an empty list is
/// *deliberately unbound* — the two are different answers, and a map that could only hold
/// keystrokes had no way to say the second one. Anything longer is a list of alternates, tried
/// in the order it is written.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Keymap {
    /// Command id to keystrokes, for the commands the user has changed.
    overrides: BTreeMap<String, Vec<String>>,
}

impl<'de> Deserialize<'de> for Keymap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = BTreeMap::<String, StoredBinding>::deserialize(deserializer)?;
        Ok(Self {
            overrides: stored
                .into_iter()
                .map(|(id, binding)| (id, binding.into()))
                .collect(),
        })
    }
}

impl Keymap {
    /// Drops overrides that name a command this build does not have, and keystrokes gpui cannot
    /// parse.
    ///
    /// The file is user-editable text and survives across versions, so both are reachable without
    /// anyone doing anything wrong — and an unparseable keystroke would otherwise panic inside
    /// `KeyBinding::new`.
    ///
    /// A command whose *every* keystroke is unusable keeps its entry as an empty list rather than
    /// losing it. Dropping the entry would restore the default, and a user who is looking at a
    /// command they unbound wants it to stay unbound, not to come back on the next launch.
    fn discard_unusable(&mut self) {
        self.overrides.retain(|id, keystrokes| {
            let Some(command) = actions::bindable(id) else {
                log::warn!("ignoring key bindings for unknown command `{id}`");
                return false;
            };
            keystrokes.retain(|keystroke| {
                let usable = actions::is_valid_keystroke(keystroke);
                if !usable {
                    log::warn!("ignoring key binding `{id}` = `{keystroke}`");
                }
                usable
            });
            // Filtering can turn an old two-key override into today's default. Keeping that
            // no-op entry would freeze the default into the file and label it as customised.
            !matches_defaults(command, keystrokes)
        });
    }

    /// Every keystroke bound to `command`, whether default or overridden.
    ///
    /// Empty when the user has deliberately unbound it, and equally when the command ships with
    /// no key — the two are the same to everything downstream, which is the point: a row with no
    /// keystroke beside it renders, conflicts with nothing, and binds nothing. As stored, which
    /// for an untouched command is the `secondary-` spelling — use [`Keymap::display`] for
    /// anything a person reads.
    pub fn keystrokes(&self, command: &Bindable) -> Vec<&str> {
        match self.overrides.get(command.id) {
            Some(keystrokes) => keystrokes.iter().map(String::as_str).collect(),
            None => command.defaults().collect(),
        }
    }

    /// The keystroke a menu shows beside `command`, or `None` when it is unbound.
    ///
    /// The first of them. A menu row has space for one, and the first is the one the user chose
    /// first — an alternate is an alternate.
    pub fn keystroke(&self, command: &Bindable) -> Option<&str> {
        self.keystrokes(command).first().copied()
    }

    /// Every keystroke bound to `command`, written the way this platform writes it.
    ///
    /// `secondary-s` is how a default is stored and is not how anyone would say it out loud; the
    /// settings window shows `cmd-s` or `ctrl-s` depending on the keyboard in front of it.
    pub fn displays(&self, command: &Bindable) -> Vec<String> {
        self.keystrokes(command)
            .into_iter()
            .map(actions::normalise_keystroke)
            .collect()
    }

    /// The primary keystroke, written the way this platform writes it.
    ///
    /// Empty when the command is unbound, which is what a menu and the palette want: a row with
    /// no keystroke beside it, rather than a row that is missing.
    pub fn display(&self, command: &Bindable) -> String {
        self.keystroke(command)
            .map(actions::normalise_keystroke)
            .unwrap_or_default()
    }

    /// `true` when the user has changed this command's bindings.
    pub fn is_overridden(&self, command: &Bindable) -> bool {
        self.overrides.contains_key(command.id)
    }

    /// `true` when this command answers to no keystroke at all.
    ///
    /// Whether that is the user's doing or the command's own default. The settings row it drives
    /// says "this has no key", which is equally true either way — asking instead whether an
    /// override exists would leave a command that ships unbound looking bound to nothing in
    /// particular.
    pub fn is_unbound(&self, command: &Bindable) -> bool {
        self.keystrokes(command).is_empty()
    }

    /// Replaces the keystroke in one slot, leaving the command's other keystrokes alone.
    ///
    /// Returns `false` for a keystroke gpui cannot parse, or one the command already holds in a
    /// *different* slot — the same duplicate rule [`Keymap::add`] applies on append, without
    /// which replacing a slot could store the same keystroke twice, show two identical chips
    /// with no conflict warning (`conflicts` excludes the command itself), and write the pair
    /// to disk. Re-entering the keystroke a slot already has is a no-op, not a refusal.
    ///
    /// A slot past the end appends, which is what makes [`Keymap::add`] this function with the
    /// length passed in.
    pub fn set_at(&mut self, command: &Bindable, index: usize, keystroke: &str) -> bool {
        if !actions::is_valid_keystroke(keystroke) {
            return false;
        }
        let normalised = actions::normalise_keystroke(keystroke);
        let elsewhere = self
            .displays(command)
            .iter()
            .enumerate()
            .any(|(slot, held)| slot != index && *held == normalised);
        if elsewhere {
            return false;
        }
        let mut keystrokes = self.owned_keystrokes(command);
        match keystrokes.get_mut(index) {
            Some(slot) => *slot = keystroke.to_string(),
            None => keystrokes.push(keystroke.to_string()),
        }
        self.store(command, keystrokes);
        true
    }

    /// Adds `keystroke` alongside whatever `command` already answers to.
    ///
    /// Returns `false` for a keystroke gpui cannot parse or one this command already has. The
    /// duplicate check is what stops the list growing every time a user presses the same key into
    /// the same row twice.
    pub fn add(&mut self, command: &Bindable, keystroke: &str) -> bool {
        if self
            .displays(command)
            .contains(&actions::normalise_keystroke(keystroke))
        {
            return false;
        }
        self.set_at(command, self.keystrokes(command).len(), keystroke)
    }

    /// Drops the keystroke in one slot. The command is left unbound when it was the last.
    pub fn remove_at(&mut self, command: &Bindable, index: usize) {
        let mut keystrokes = self.owned_keystrokes(command);
        if index >= keystrokes.len() {
            return;
        }
        keystrokes.remove(index);
        self.store(command, keystrokes);
    }

    /// This command's keystrokes, owned, so they can be edited without borrowing the map.
    fn owned_keystrokes(&self, command: &Bindable) -> Vec<String> {
        self.keystrokes(command)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// Writes a command's keystrokes back, dropping the override when it *is* the default.
    ///
    /// Compared after normalising, because the keyboard reports `cmd-s` where the table says
    /// `secondary-s`. Comparing them raw would write an override every time someone pressed a
    /// default back in, freezing today's defaults into their file.
    ///
    /// An empty list is stored rather than dropped — that is the user saying "no key at all", and
    /// dropping it would put the default back on the next launch — *unless* the command ships
    /// with no key, where the empty list already is the default and storing it would write an
    /// override that says nothing.
    fn store(&mut self, command: &Bindable, keystrokes: Vec<String>) {
        // Compared as a whole list rather than as one keystroke, because a command can ship with
        // two: putting Delete's second key back by hand has to read as "this is the default"
        // rather than as an override that happens to say the same thing.
        if matches_defaults(command, &keystrokes) {
            self.overrides.remove(command.id);
        } else {
            self.overrides.insert(command.id.to_string(), keystrokes);
        }
    }

    /// Leaves `command` with no keystroke at all.
    ///
    /// Distinct from [`Keymap::clear`], which puts the default back. Without this there was no way
    /// to say "not this one" — a user who did not want `space` to start playback could only move
    /// it somewhere else and hope that somewhere was out of the way.
    ///
    /// Through [`Keymap::store`], so unbinding a command that already ships unbound writes
    /// nothing: that override would say exactly what the default says.
    pub fn unbind(&mut self, command: &Bindable) {
        self.store(command, Vec::new());
    }

    /// Restores one command to its default.
    pub fn clear(&mut self, command: &Bindable) {
        self.overrides.remove(command.id);
    }

    /// Restores every command in `group` to its default.
    pub fn reset_group(&mut self, group: Key) {
        for command in BINDABLE.iter().filter(|command| command.group == group) {
            self.overrides.remove(command.id);
        }
    }

    /// Restores every command to its default.
    pub fn reset(&mut self) {
        self.overrides.clear();
    }

    /// Commands that would answer to `keystroke` at the same time as `except` does.
    ///
    /// A conflict is shown rather than refused: two commands on one keystroke is a mistake worth
    /// pointing at, but the user may be in the middle of swapping a pair over.
    ///
    /// Scoped as well as compared. Two panes are never focused at once, so the same key may mean
    /// one thing in the piano roll and another in the mixer without either being wrong — see
    /// [`Bindable::shares_reach_with`]. Compared after normalising, so a ⌘L the user just pressed
    /// is recognised as clashing with a default stored as `secondary-l`.
    pub fn conflicts(&self, keystroke: &str, except: &Bindable) -> Vec<&'static Bindable> {
        let wanted = actions::normalise_keystroke(keystroke);
        BINDABLE
            .iter()
            .filter(|other| other.id != except.id)
            .filter(|other| other.shares_reach_with(except))
            .filter(|other| self.displays(other).contains(&wanted))
            .collect()
    }

    /// Installs every binding into the application.
    pub fn apply(&self, cx: &mut App) {
        let bindings: Vec<KeyBinding> = BINDABLE
            .iter()
            .flat_map(|command| {
                self.keystrokes(command)
                    .into_iter()
                    .map(|keystroke| command.binding(keystroke))
                    .collect::<Vec<_>>()
            })
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

    /// The keystroke a command ships with.
    ///
    /// Every test below picks a command *because* it has one — the rules being checked are about
    /// what happens around a default, and a command that ships without one has no "back to the
    /// default" to be moved away from and returned to.
    fn preset(command: &'static Bindable) -> &'static str {
        command
            .default
            .expect("this test needs a command that ships with a key")
    }

    #[test]
    fn an_empty_keymap_reports_the_defaults() {
        let keymap = Keymap::default();
        let play = command("transport.play");
        assert_eq!(keymap.keystroke(play), Some(preset(play)));
        assert!(!keymap.is_overridden(play));
    }

    #[test]
    fn setting_a_binding_back_to_its_default_stops_being_an_override() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");

        assert!(keymap.set_at(play, 0, "cmd-p"));
        assert_eq!(keymap.keystroke(play), Some("cmd-p"));
        assert!(keymap.is_overridden(play));

        assert!(keymap.set_at(play, 0, preset(play)));
        assert!(
            !keymap.is_overridden(play),
            "an override equal to the default would freeze today's default into the file"
        );
    }

    #[test]
    fn replacing_a_slot_with_a_key_another_slot_holds_is_refused() {
        // `add` refuses a duplicate on append, but the settings window routes an existing
        // chip through `set_at`, which only checked parseability: replacing slot 0 with the
        // key in slot 1 stored the same keystroke twice, showed two identical chips with no
        // conflict warning (`conflicts` excludes the command itself), and wrote the pair to
        // disk.
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        assert!(keymap.set_at(play, 0, "f5"));
        assert!(keymap.add(play, "f6"));

        assert!(
            !keymap.set_at(play, 0, "f6"),
            "slot 0 took the keystroke slot 1 already holds"
        );
        assert_eq!(keymap.keystrokes(play), &["f5", "f6"]);

        // Re-entering the keystroke a slot already has is a no-op, not a refusal.
        assert!(keymap.set_at(play, 0, "f5"));
        assert_eq!(keymap.keystrokes(play), &["f5", "f6"]);
    }

    #[test]
    fn pressing_a_default_back_in_is_recognised_through_its_platform_spelling() {
        // The table stores `secondary-s`; the keyboard reports `cmd-s` or `ctrl-s`. Comparing
        // them raw would store an override identical to the default, which is exactly what the
        // "only overrides are written" rule exists to prevent.
        let mut keymap = Keymap::default();
        let save = command("file.save");
        assert!(keymap.set_at(save, 0, "shift-f7"));
        assert!(keymap.is_overridden(save));

        assert!(keymap.set_at(save, 0, &actions::normalise_keystroke(preset(save))));
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
        let clash = keymap.conflicts(&actions::normalise_keystroke(preset(save)), undo);
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
            Some("secondary-s"),
            "the stored form stays portable; only the display is localised to the keyboard"
        );
    }

    #[test]
    fn an_unparseable_keystroke_is_refused_and_changes_nothing() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        assert!(!keymap.set_at(play, 0, "notakey-x"));
        assert_eq!(keymap.keystroke(play), Some(preset(play)));
    }

    #[test]
    fn conflicts_report_the_other_command_only() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        let undo = command("edit.undo");

        assert!(keymap.conflicts(preset(play), play).is_empty());

        keymap.set_at(undo, 0, preset(play));
        let clash = keymap.conflicts(preset(play), play);
        assert_eq!(clash.len(), 1);
        assert_eq!(clash[0].id, undo.id);
        // And the command doing the asking is never its own conflict.
        assert!(
            keymap
                .conflicts(preset(play), undo)
                .iter()
                .all(|c| c.id != undo.id)
        );
    }

    #[test]
    fn loading_discards_bindings_this_build_cannot_use() {
        let mut keymap: Keymap = serde_json::from_str(
            r#"{
                "transport.play": "cmd-p",
                "gone.in.a.later.build": "cmd-g",
                "edit.undo": "notakey-x",
                "edit.redo": ["cmd-y", "notakey-z"]
            }"#,
        )
        .unwrap();
        keymap.discard_unusable();

        assert_eq!(keymap.keystroke(command("transport.play")), Some("cmd-p"));
        assert!(
            !keymap.overrides.contains_key("gone.in.a.later.build"),
            "a command this build does not have takes its whole entry with it"
        );
        assert_eq!(
            keymap.displays(command("edit.redo")),
            vec![actions::normalise_keystroke("cmd-y")],
            "one bad keystroke out of two loses only itself"
        );
        // The one command whose every keystroke was unusable keeps its entry, empty. Dropping it
        // would restore the default, and a command with nothing left on it reads as unbound.
        assert!(keymap.is_unbound(command("edit.undo")));
    }

    #[test]
    fn filtering_an_override_back_to_the_default_drops_the_override() {
        let undo = command("edit.undo");
        let mut keymap = Keymap::default();
        keymap.overrides.insert(
            undo.id.to_string(),
            vec![
                "notakey-x".to_string(),
                undo.defaults().next().unwrap().to_string(),
            ],
        );

        keymap.discard_unusable();

        assert!(!keymap.is_overridden(undo));
        assert_eq!(keymap.keystrokes(undo), undo.defaults().collect::<Vec<_>>());
    }

    #[test]
    fn a_file_written_before_pointer_gestures_existed_keeps_its_bindings() {
        // The old shape was a bare map. Parsing it as the new shape must not quietly yield
        // "no overrides", which is what would happen without the untagged fallback.
        let legacy = r#"{"transport.play":"cmd-p","view.zoom_in":"cmd-shift-="}"#;
        let stored: StoredInput = serde_json::from_str(legacy).unwrap();
        let settings = InputSettings::from(stored);

        assert_eq!(
            settings.keys.keystroke(command("transport.play")),
            Some("cmd-p")
        );
        assert_eq!(
            settings.keys.keystroke(command("view.zoom_in")),
            Some("cmd-shift-=")
        );
        assert_eq!(
            settings.pointer,
            PointerGestures::default(),
            "an old file has no gestures, so it gets the defaults"
        );
    }

    #[test]
    fn a_file_written_before_a_command_could_hold_two_keys_keeps_its_bindings() {
        // The value was a bare string until a command could answer to more than one keystroke.
        // Reading only the list would parse `"cmd-p"` as malformed and throw away a binding the
        // user had chosen — silently, since a malformed entry is dropped by design.
        let stored = r#"{"keys":{"transport.play":"cmd-p","edit.undo":["cmd-z","f1"]}}"#;
        let settings = InputSettings::from(serde_json::from_str::<StoredInput>(stored).unwrap());

        assert_eq!(
            settings.keys.keystrokes(command("transport.play")),
            vec!["cmd-p"]
        );
        assert_eq!(
            settings.keys.keystrokes(command("edit.undo")),
            vec!["cmd-z", "f1"]
        );
    }

    #[test]
    fn a_command_can_answer_to_more_than_one_key() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");

        assert!(keymap.add(play, "f5"));
        assert_eq!(keymap.keystrokes(play), vec![preset(play), "f5"]);
        assert_eq!(
            keymap.keystroke(play),
            Some(preset(play)),
            "the menu shows the first; an alternate is an alternate"
        );

        assert!(
            !keymap.add(play, "f5"),
            "the same key twice would grow the list on every press"
        );
        assert!(
            !keymap.add(play, &actions::normalise_keystroke(preset(play))),
            "and so would the default pressed back in through its platform spelling"
        );
        assert!(!keymap.add(play, "notakey-x"));
        assert_eq!(keymap.keystrokes(play), vec![preset(play), "f5"]);
    }

    #[test]
    fn one_of_several_keys_can_be_replaced_or_dropped() {
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        keymap.add(play, "f5");
        keymap.add(play, "f6");

        assert!(keymap.set_at(play, 1, "f7"));
        assert_eq!(keymap.keystrokes(play), vec![preset(play), "f7", "f6"]);

        keymap.remove_at(play, 1);
        assert_eq!(keymap.keystrokes(play), vec![preset(play), "f6"]);
        keymap.remove_at(play, 9);
        assert_eq!(
            keymap.keystrokes(play),
            vec![preset(play), "f6"],
            "a slot that is not there changes nothing"
        );

        // Down to the default alone, the override goes with it — an override equal to the
        // default would freeze today's default into the user's file.
        keymap.remove_at(play, 1);
        assert!(!keymap.is_overridden(play));
        assert_eq!(keymap.keystrokes(play), vec![preset(play)]);

        // And down from *one* leaves it unbound rather than back on the default, because that is
        // the only thing removing the last key can mean.
        keymap.remove_at(play, 0);
        assert!(keymap.is_unbound(play));
    }

    #[test]
    fn a_command_can_be_left_with_no_key_at_all() {
        // Distinct from restoring the default, and there was no way to say it: a user who did not
        // want the space bar to start playback could only move it somewhere else and hope.
        let mut keymap = Keymap::default();
        let play = command("transport.play");

        keymap.unbind(play);
        assert!(keymap.is_unbound(play));
        assert!(keymap.is_overridden(play));
        assert!(keymap.keystrokes(play).is_empty());
        assert_eq!(keymap.keystroke(play), None);
        assert_eq!(keymap.display(play), "");

        assert!(
            keymap
                .conflicts(preset(play), command("edit.undo"))
                .is_empty(),
            "and it stops clashing with anything, because it answers to nothing"
        );

        keymap.clear(play);
        assert_eq!(keymap.keystroke(play), Some(preset(play)));
    }

    #[test]
    fn the_same_key_in_two_panes_is_not_a_conflict() {
        // What the contexts are for. Two panes are never focused at once, so a key may mean one
        // thing in the roll and another in the mixer; the window's own bindings are on the path
        // from every pane, so those do clash with all of them.
        let mut keymap = Keymap::default();
        let tool = command("edit.next_tool");
        let undo = command("edit.undo");
        assert_eq!(tool.context, crate::actions::context::ROLL);
        assert_eq!(undo.context, crate::actions::context::WINDOW);

        keymap.set_at(undo, 0, preset(tool));
        let clash = keymap.conflicts(preset(tool), tool);
        assert_eq!(
            clash.len(),
            1,
            "a window binding is reachable from the roll, so it clashes"
        );
        assert_eq!(clash[0].id, undo.id);
    }

    #[test]
    fn the_current_shape_round_trips() {
        let mut settings = InputSettings::default();
        settings.keys.set_at(command("transport.play"), 0, "cmd-p");
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

        keymap.set_at(command("view.zoom_in"), 0, "cmd-shift-=");
        let text = serde_json::to_string(&keymap).unwrap();
        assert_eq!(text, r#"{"view.zoom_in":["cmd-shift-="]}"#);
        assert_eq!(serde_json::from_str::<Keymap>(&text).unwrap(), keymap);

        // And an unbinding survives the round trip as itself, not as an absent entry — the two
        // mean opposite things.
        keymap.unbind(command("transport.play"));
        let text = serde_json::to_string(&keymap).unwrap();
        assert!(text.contains(r#""transport.play":[]"#), "{text}");
        assert_eq!(serde_json::from_str::<Keymap>(&text).unwrap(), keymap);
    }

    #[test]
    fn reset_restores_every_default() {
        let mut keymap = Keymap::default();
        keymap.set_at(command("transport.play"), 0, "cmd-p");
        keymap.set_at(command("edit.undo"), 0, "cmd-shift-u");
        keymap.reset();
        for entry in BINDABLE {
            // Including the commands whose default *is* no key, which read back as `None`
            // rather than being skipped: reset means every row is where it started.
            assert_eq!(keymap.keystroke(entry), entry.default, "`{}`", entry.id);
        }
    }

    #[test]
    fn a_command_that_ships_with_no_key_can_be_given_one_and_handed_back() {
        // The row exists so a user can put a key on a command that did not earn a default. It has
        // to behave like any other row from there: bindable, conflicting, resettable — and the
        // "unbound" it starts in is the same unbound `unbind` produces, not a third state.
        let mut keymap = Keymap::default();
        let mute = BINDABLE
            .iter()
            .find(|entry| entry.default.is_none())
            .expect("some commands ship with no key");

        assert!(keymap.keystrokes(mute).is_empty());
        assert!(keymap.is_unbound(mute));
        assert!(!keymap.is_overridden(mute));
        assert_eq!(keymap.display(mute), "");
        assert!(
            keymap.conflicts("f8", mute).is_empty(),
            "and it collides with nothing until it is given a key"
        );

        assert!(keymap.set_at(mute, 0, "f8"));
        assert_eq!(keymap.keystrokes(mute), vec!["f8"]);
        assert!(keymap.is_overridden(mute));
        assert!(!keymap.is_unbound(mute));

        // Taking the key away again is a return to the default, not an override that says "no
        // key" — the default already said that, and writing it would freeze it into the file.
        keymap.remove_at(mute, 0);
        assert!(keymap.is_unbound(mute));
        assert!(
            !keymap.is_overridden(mute),
            "an override identical to the default was written"
        );
        keymap.unbind(mute);
        assert!(!keymap.is_overridden(mute), "and so was one from `unbind`");
    }

    #[test]
    fn a_group_can_be_put_back_without_touching_the_rest() {
        // `reset` was all or nothing, which is a poor trade for a user who has rearranged the
        // transport deliberately and made one mistake in the file menu.
        let mut keymap = Keymap::default();
        let play = command("transport.play");
        let save = command("file.save");
        keymap.set_at(play, 0, "f5");
        keymap.set_at(save, 0, "f6");

        keymap.reset_group(save.group);
        assert_eq!(keymap.keystroke(save), Some(preset(save)));
        assert_eq!(keymap.keystroke(play), Some("f5"));
    }
}
