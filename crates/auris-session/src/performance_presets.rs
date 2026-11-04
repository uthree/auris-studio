//! Performance presets somebody kept, beside the ones the application ships with.
//!
//! The built-in catalogue is [`builtin_performance_presets`] in `auris-core`. This is where the
//! ones a person saves from the window, or an agent saves with its preset tool, are kept: one file
//! in the configuration directory, shared by every project and every door, so a feel worked out
//! once on one song is there on the next.
//!
//! A kept preset is copied into a clip when it is applied, never referred to by name — see the
//! module documentation of [`PerformancePreset`] for why — so this file can be edited, emptied
//! or lost without changing how a single saved song sounds.

use std::path::{Path, PathBuf};

use auris_core::{
    NoteTransform, PerformancePreset, PresetFamily, builtin_performance_preset,
    builtin_performance_presets,
};
use serde::{Deserialize, Serialize};

use crate::error::SessionError;
use crate::settings::{config_dir, read_config_text, write_config_bytes};

/// The longest name a kept preset may have, in characters.
pub const PRESET_NAME_MAX: usize = 64;
/// The longest description a kept preset may have, in characters.
pub const PRESET_DESCRIPTION_MAX: usize = 400;
/// How many presets one installation keeps.
///
/// Far more than a picker can usefully show; the bound is there so a runaway agent cannot grow
/// the file without limit.
pub const PRESET_BOOK_MAX: usize = 256;

/// One preset in a listing, with where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedPreset {
    /// The preset.
    pub preset: PerformancePreset,
    /// Whether the application ships it. A built-in cannot be replaced or deleted.
    pub builtin: bool,
}

/// Every preset this installation has been given.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PerformancePresetBook {
    entries: Vec<PerformancePreset>,
}

/// Why a preset could not be kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepRefusal {
    /// The name is empty or longer than [`PRESET_NAME_MAX`].
    BadName,
    /// A built-in already uses the name.
    BuiltinName,
    /// A kept preset already uses the name and replacing it was not asked for.
    Exists,
    /// The description is longer than [`PRESET_DESCRIPTION_MAX`].
    LongDescription,
    /// The book already holds [`PRESET_BOOK_MAX`] presets.
    Full,
    /// A built-in preset cannot be deleted.
    Builtin,
}

impl std::fmt::Display for KeepRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BadName => "a preset name is 1 to 64 characters",
            Self::BuiltinName => "a built-in preset already uses that name",
            Self::Exists => "a kept preset already uses that name; ask to replace it",
            Self::LongDescription => "a preset description is at most 400 characters",
            Self::Full => "256 presets are already kept; delete one first",
            Self::Builtin => "a built-in preset cannot be deleted",
        })
    }
}

impl PerformancePresetBook {
    /// Where the book lives by default.
    pub fn path() -> PathBuf {
        config_dir().join("performance-presets.json")
    }

    /// Loads the book at `path`, falling back to an empty one.
    ///
    /// A missing file is the ordinary case, and a malformed one is logged and also falls back,
    /// for the reason a malformed progression book does.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = read_config_text(path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(book) => book,
            Err(error) => {
                log::warn!("ignoring malformed {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the book to `path`, creating its folder if needed.
    pub fn save(&self, path: &Path) -> Result<(), SessionError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::SettingsWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(auris_io::IoError::from)?;
        write_config_bytes(path, text.as_bytes()).map_err(|source| SessionError::SettingsWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Reloads, edits and saves the book at `path` while holding a cross-process lock, so the
    /// window and an agent in another process cannot each write over what the other kept.
    pub fn update<T>(
        path: &Path,
        edit: impl FnOnce(&mut Self) -> T,
    ) -> Result<(Self, T), SessionError> {
        let lock_path = path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::SettingsWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| SessionError::SettingsWrite {
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| SessionError::SettingsWrite {
            path: lock_path,
            source,
        })?;
        let mut book = Self::load(path);
        let before = book.clone();
        let result = edit(&mut book);
        if book != before {
            book.save(path)?;
        }
        Ok((book, result))
    }

    /// Everything in it, in the order it was kept.
    pub fn entries(&self) -> &[PerformancePreset] {
        &self.entries
    }

    /// The kept preset called `name`.
    pub fn get(&self, name: &str) -> Option<&PerformancePreset> {
        let name = name.trim();
        self.entries.iter().find(|preset| preset.name == name)
    }

    /// Keeps `preset`, trimming its name. `replace` allows an existing kept preset of the same
    /// name to be overwritten; a built-in's name is always refused, because a picker with the
    /// same word in it twice gives nobody a way to tell which one they are about to get.
    pub fn keep(
        &mut self,
        mut preset: PerformancePreset,
        replace: bool,
    ) -> Result<(), KeepRefusal> {
        preset.name = preset.name.trim().to_owned();
        preset.description = preset.description.trim().to_owned();
        let length = preset.name.chars().count();
        if length == 0 || length > PRESET_NAME_MAX || preset.name.chars().any(char::is_control) {
            return Err(KeepRefusal::BadName);
        }
        if builtin_performance_preset(&preset.name).is_some() {
            return Err(KeepRefusal::BuiltinName);
        }
        if preset.description.chars().count() > PRESET_DESCRIPTION_MAX {
            return Err(KeepRefusal::LongDescription);
        }
        let count = self.entries.len();
        match self
            .entries
            .iter_mut()
            .find(|held| held.name == preset.name)
        {
            Some(_) if !replace => Err(KeepRefusal::Exists),
            Some(held) => {
                *held = preset;
                Ok(())
            }
            None if count >= PRESET_BOOK_MAX => Err(KeepRefusal::Full),
            None => {
                self.entries.push(preset);
                Ok(())
            }
        }
    }

    /// Forgets the kept preset called `name`, reporting whether there was one.
    pub fn forget(&mut self, name: &str) -> bool {
        let name = name.trim();
        let before = self.entries.len();
        self.entries.retain(|preset| preset.name != name);
        self.entries.len() != before
    }
}

/// Where a session keeps the presets it has been given.
///
/// With a path, every change goes through the file under its lock; without one — a test, or a
/// tool that must not touch the user's configuration — the book lives in memory for as long as
/// the session does.
#[derive(Debug, Default)]
pub(crate) struct PresetLibrary {
    pub(crate) path: Option<PathBuf>,
    pub(crate) book: PerformancePresetBook,
}

impl PresetLibrary {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        let book = path
            .as_deref()
            .map(PerformancePresetBook::load)
            .unwrap_or_default();
        Self { path, book }
    }

    pub(crate) fn reload(&mut self) {
        if let Some(path) = &self.path {
            self.book = PerformancePresetBook::load(path);
        }
    }

    pub(crate) fn update<T>(
        &mut self,
        edit: impl FnOnce(&mut PerformancePresetBook) -> T,
    ) -> Result<T, SessionError> {
        match &self.path {
            Some(path) => {
                let (book, result) = PerformancePresetBook::update(path, edit)?;
                self.book = book;
                Ok(result)
            }
            None => Ok(edit(&mut self.book)),
        }
    }

    pub(crate) fn listed(&self) -> Vec<ListedPreset> {
        builtin_performance_presets()
            .into_iter()
            .map(|preset| ListedPreset {
                preset,
                builtin: true,
            })
            .chain(
                self.book
                    .entries()
                    .iter()
                    .cloned()
                    .map(|preset| ListedPreset {
                        preset,
                        builtin: false,
                    }),
            )
            .collect()
    }

    pub(crate) fn find(&self, name: &str) -> Option<ListedPreset> {
        let name = name.trim();
        builtin_performance_preset(name)
            .map(|preset| ListedPreset {
                preset,
                builtin: true,
            })
            .or_else(|| {
                self.book.get(name).cloned().map(|preset| ListedPreset {
                    preset,
                    builtin: false,
                })
            })
    }
}

/// A clip's stack as a preset keeps it: the drum-writer scopes are taken along, since a kit
/// written by the same writers can use them, and the seeds are cleared because applying gives
/// every clip its own take anyway.
pub(crate) fn kept_stack(stack: &[NoteTransform]) -> Vec<NoteTransform> {
    stack
        .iter()
        .map(|stage| {
            let mut stage = stage.clone();
            match &mut stage {
                NoteTransform::Humanize { seed, .. } => *seed = 0,
                NoteTransform::Expression { settings } => settings.seed = 0,
                NoteTransform::Ghost { settings } => settings.seed = 0,
                NoteTransform::ForDrumVoice { transforms, .. } => {
                    *transforms = kept_stack(transforms)
                }
                _ => {}
            }
            stage
        })
        .collect()
}

/// The family a preset falls in by default, read from a track's instrument.
pub fn family_hint(track: &auris_core::Track) -> PresetFamily {
    if track.kind.is_drum() {
        return PresetFamily::Drums;
    }
    let Some(instrument) = track.kind.as_instrument() else {
        return PresetFamily::Other;
    };
    let id = instrument.instrument_id.as_str();
    let name = format!("{id} {}", track.name).to_lowercase();
    let any = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if any(&["bass"]) {
        PresetFamily::Bass
    } else if any(&["guitar"]) {
        PresetFamily::Guitar
    } else if any(&["piano", "organ", "keys", "rhodes", "dulcimer", "epiano"]) {
        PresetFamily::Keys
    } else if any(&["violin", "viola", "cello", "string", "pizz"]) {
        PresetFamily::Strings
    } else if any(&["trumpet", "trombone", "horn", "brass", "tuba"]) {
        PresetFamily::Brass
    } else if any(&[
        "flute", "clarinet", "sax", "oboe", "bassoon", "whistle", "wind",
    ]) {
        PresetFamily::Woodwinds
    } else if any(&["synth", "chiptune", "fm", "pad", "lead", "arp"]) {
        PresetFamily::Synth
    } else {
        PresetFamily::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str) -> PerformancePreset {
        PerformancePreset {
            name: name.into(),
            family: PresetFamily::Keys,
            description: String::new(),
            transforms: vec![NoteTransform::Gate { amount: 0.5 }],
        }
    }

    #[test]
    fn a_builtin_name_is_refused_and_a_kept_one_is_replaced_only_when_asked() {
        let mut book = PerformancePresetBook::default();
        assert_eq!(
            book.keep(preset("piano_ballad"), true),
            Err(KeepRefusal::BuiltinName)
        );
        assert_eq!(book.keep(preset(" mine "), false), Ok(()));
        assert_eq!(book.entries()[0].name, "mine");
        assert_eq!(book.keep(preset("mine"), false), Err(KeepRefusal::Exists));
        let mut other = preset("mine");
        other.transforms.clear();
        assert_eq!(book.keep(other, true), Ok(()));
        assert!(book.get("mine").unwrap().transforms.is_empty());
        assert!(book.forget(" mine"));
        assert!(!book.forget("mine"));
        assert_eq!(book.keep(preset(""), false), Err(KeepRefusal::BadName));
    }

    #[test]
    fn the_book_survives_its_file_and_an_update_holds_the_lock() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("performance-presets.json");
        let (book, kept) =
            PerformancePresetBook::update(&path, |book| book.keep(preset("mine"), false)).unwrap();
        assert_eq!(kept, Ok(()));
        assert_eq!(PerformancePresetBook::load(&path), book);
        let mut library = PresetLibrary::new(Some(path.clone()));
        assert!(library.find("mine").is_some_and(|listed| !listed.builtin));
        assert!(
            library
                .find("piano_ballad")
                .is_some_and(|listed| listed.builtin)
        );
        library.update(|book| book.forget("mine")).unwrap();
        assert!(PerformancePresetBook::load(&path).entries().is_empty());
    }
}
