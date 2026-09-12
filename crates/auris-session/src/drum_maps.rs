//! User-level drum maps, keyed by the sound source they describe.
//!
//! The chosen map is also copied into every project track. The library saves repeated setup when
//! another track uses the same source; it is never a dependency a portable project must resolve.

use std::path::PathBuf;

use auris_core::DrumMap;
use serde::{Deserialize, Serialize};

use crate::error::SessionError;
use crate::settings::config_dir;

/// Stable identity of a drum-producing source on this installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrumMapSource {
    /// Registered built-in or hosted instrument identifier.
    pub instrument_id: String,
    /// Hosted plug-in bundle, when the instrument comes from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_file: Option<PathBuf>,
    /// Selected SoundFont preset, when the sampler is the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundfont: Option<DrumMapSoundFont>,
}

/// The SoundFont and preset that distinguish one sampler drum source from another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrumMapSoundFont {
    /// Resolved local SoundFont path.
    pub path: PathBuf,
    /// Size recorded in the project, used to distinguish replacements at the same path.
    pub byte_size: u64,
    /// MIDI bank number.
    pub bank: i32,
    /// MIDI program number.
    pub patch: i32,
}

/// One reusable map and the source it was learned for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDrumMap {
    /// Name shown when choosing a saved map.
    pub name: String,
    /// Source that automatically receives this map.
    pub source: DrumMapSource,
    /// Complete ordered map copied into a project.
    pub map: DrumMap,
}

/// Every drum map this installation remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DrumMapBook {
    entries: Vec<SavedDrumMap>,
}

impl DrumMapBook {
    fn lock() -> Result<std::fs::File, SessionError> {
        let path = Self::path().with_extension("lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::SettingsWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| SessionError::SettingsWrite {
                path: path.clone(),
                source,
            })?;
        file.lock()
            .map_err(|source| SessionError::SettingsWrite { path, source })?;
        Ok(file)
    }

    /// Where the user-level map library lives.
    pub fn path() -> PathBuf {
        config_dir().join("drum-maps.json")
    }

    /// Loads the library, falling back to an empty one when missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
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

    /// Writes the library, creating its configuration directory when needed.
    pub fn save(&self) -> Result<(), SessionError> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::SettingsWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(auris_io::IoError::from)?;
        std::fs::write(&path, text).map_err(|source| SessionError::SettingsWrite { path, source })
    }

    /// Saved maps in picker order.
    pub fn entries(&self) -> &[SavedDrumMap] {
        &self.entries
    }

    /// Map remembered for an exact source.
    pub fn map_for(&self, source: &DrumMapSource) -> Option<&SavedDrumMap> {
        self.entries.iter().find(|entry| &entry.source == source)
    }

    /// Remembers a map for a source, replacing its previous map.
    pub fn keep(&mut self, name: impl Into<String>, source: DrumMapSource, map: DrumMap) -> bool {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return false;
        }
        let saved = SavedDrumMap { name, source, map };
        match self
            .entries
            .iter_mut()
            .find(|entry| entry.source == saved.source)
        {
            Some(existing) if *existing == saved => return false,
            Some(existing) => *existing = saved,
            None => self.entries.push(saved),
        }
        true
    }

    /// Reloads, remembers one source and saves under a cross-process lock.
    pub fn keep_saved(
        name: impl Into<String>,
        source: DrumMapSource,
        map: DrumMap,
    ) -> Result<bool, SessionError> {
        let _lock = Self::lock()?;
        let mut book = Self::load();
        let kept = book.keep(name, source, map);
        if kept {
            book.save()?;
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::DrumRole;

    fn source(path: &str, patch: i32) -> DrumMapSource {
        DrumMapSource {
            instrument_id: "auris.sampler".into(),
            plugin_file: None,
            soundfont: Some(DrumMapSoundFont {
                path: path.into(),
                byte_size: 123,
                bank: 128,
                patch,
            }),
        }
    }

    #[test]
    fn one_source_is_replaced_while_another_remains_available() {
        let first = source("kit.sf2", 0);
        let second = source("kit.sf2", 1);
        let mut book = DrumMapBook::default();
        assert!(book.keep(
            "Studio kit",
            first.clone(),
            DrumMap::from_voices([(DrumRole::Kick, 36)])
        ));
        assert!(book.keep(
            "Room kit",
            second.clone(),
            DrumMap::from_voices([(DrumRole::Snare, 38)])
        ));
        assert!(book.keep("Studio kit", first.clone(), DrumMap::general_midi()));
        assert_eq!(book.entries().len(), 2);
        assert_eq!(book.map_for(&first).unwrap().map, DrumMap::general_midi());
        assert_eq!(
            book.map_for(&second).unwrap().map.voices[&DrumRole::Snare],
            38
        );

        let json = serde_json::to_string(&book).unwrap();
        assert_eq!(serde_json::from_str::<DrumMapBook>(&json).unwrap(), book);
    }
}
