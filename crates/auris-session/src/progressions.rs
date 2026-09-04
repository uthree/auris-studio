//! Progressions somebody wrote down and kept, beside the ones the application shipped with.
//!
//! The built-in catalogue is a `const` in `auris-core`: sixteen specific, named, culturally
//! located loops, quoted rather than derived, and there is no reason a person should be limited to
//! them. This is where the ones they write live.
//!
//! **A kept progression is a snippet the picker offers, not a name a file resolves.** Choosing one
//! writes its *chords* into the song, so an `.asong` never refers to anything outside itself: sent
//! to somebody who has never seen your book, it still plays the piece you wrote. The alternative —
//! a `@myname` the parser has to look up on disk — would make a document that means one thing here
//! and nothing anywhere else, which is the property a portable format is for.
//!
//! It sits in `auris-session` rather than in `auris-core` for the ordinary reason: this is a file
//! on a disk, and the document model does no I/O.

use std::path::PathBuf;

use auris_core::theory::chart::{Chart, ChartMode, catalog};
use serde::{Deserialize, Serialize};

use crate::error::SessionError;
use crate::settings::config_dir;

/// One progression somebody wrote and kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProgression {
    /// What it is called in the picker.
    pub name: String,
    /// The chords, in the notation [`Chart::parse`] reads: `| IVmaj7 | III7 | vi7 | I7 |`.
    pub chart: String,
    /// Whether its degrees are written against a major or a minor tonic.
    ///
    /// The same question the built-in catalogue answers, and for the same reason: a progression
    /// asked for in the other mode follows its **chords** rather than its degrees, and this is
    /// what says which mode it was written in. `None` takes it at face value, which is what a
    /// chart nobody has said anything about deserves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
}

/// Which mode a saved progression is written against.
///
/// A spelling of [`ChartMode`] that serde can put in a file: the core type is a plain enum with no
/// derives, and giving it some would put a file format in the document model.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// `I` is the tonic.
    Major,
    /// `i` is the tonic.
    Minor,
}

impl From<Mode> for ChartMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Major => ChartMode::Major,
            Mode::Minor => ChartMode::Minor,
        }
    }
}

impl From<ChartMode> for Mode {
    fn from(mode: ChartMode) -> Self {
        match mode {
            ChartMode::Major => Mode::Major,
            ChartMode::Minor => Mode::Minor,
        }
    }
}

impl SavedProgression {
    /// The chart this entry describes, or nothing when the text stopped being a chart.
    ///
    /// A hand-edited file can say anything; a line that no longer parses is one entry missing from
    /// a picker, which is a great deal better than a startup that fails over a progression.
    pub fn chart(&self) -> Option<Chart> {
        let chart = Chart::parse(&self.chart)?;
        Some(match self.mode {
            Some(mode) => chart.written_in(mode.into()),
            None => chart,
        })
    }
}

/// Every progression this installation has been taught.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProgressionBook {
    entries: Vec<SavedProgression>,
}

impl ProgressionBook {
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

    /// Where the book lives.
    pub fn path() -> PathBuf {
        config_dir().join("progressions.json")
    }

    /// Loads the book, falling back to an empty one.
    ///
    /// A missing file is the ordinary case and a malformed one is logged and then also falls back:
    /// refusing to start over a list of chord progressions would be a poor trade.
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

    /// Writes the book, creating the configuration directory if needed.
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

    /// Everything in it, in the order it was kept.
    pub fn entries(&self) -> &[SavedProgression] {
        &self.entries
    }

    /// The chart kept under `name`, if there is one and it still parses.
    pub fn chart(&self, name: &str) -> Option<Chart> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .and_then(SavedProgression::chart)
    }

    /// Keeps `chart` under `name`, replacing an entry of the same name.
    ///
    /// A name the built-in catalogue already uses is refused. Two `@axis`es would be a picker with
    /// the same word in it twice, and a person choosing between them would have no way to tell
    /// which one they were about to get.
    pub fn keep(&mut self, name: &str, chart: &Chart, mode: Option<ChartMode>) -> bool {
        let name = name.trim();
        if name.is_empty() || catalog(name).is_some() {
            return false;
        }
        let saved = SavedProgression {
            name: name.to_string(),
            chart: chart.to_string(),
            mode: mode.map(Mode::from),
        };
        match self.entries.iter_mut().find(|entry| entry.name == name) {
            Some(existing) => *existing = saved,
            None => self.entries.push(saved),
        }
        true
    }

    /// Reloads, keeps one entry, and saves while holding a cross-process file lock.
    pub fn keep_saved(
        name: &str,
        chart: &Chart,
        mode: Option<ChartMode>,
    ) -> Result<bool, SessionError> {
        let _lock = Self::lock()?;
        let mut book = Self::load();
        let kept = book.keep(name, chart, mode);
        if kept {
            book.save()?;
        }
        Ok(kept)
    }

    /// Forgets the entry called `name`, reporting whether there was one.
    pub fn forget(&mut self, name: &str) -> bool {
        let name = name.trim();
        let before = self.entries.len();
        self.entries.retain(|entry| entry.name != name);
        self.entries.len() != before
    }

    /// Reloads, removes one entry, and saves while holding a cross-process file lock.
    pub fn forget_saved(name: &str) -> Result<bool, SessionError> {
        let _lock = Self::lock()?;
        let mut book = Self::load();
        let forgotten = book.forget(name);
        if forgotten {
            book.save()?;
        }
        Ok(forgotten)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(text: &str) -> Chart {
        Chart::parse(text).expect("the fixture is a chart")
    }

    #[test]
    fn a_builtin_alias_cannot_be_kept_as_a_custom_progression() {
        let mut book = ProgressionBook::default();
        assert!(!book.keep("丸サ", &chart("| I | V |"), None));
        assert!(book.entries().is_empty());
    }

    #[test]
    fn a_kept_progression_comes_back_as_the_chords_it_was() {
        let mut book = ProgressionBook::default();
        assert!(book.keep(
            "my turnaround",
            &chart("| IVmaj7 | III7 | vi7 | I7 |"),
            None
        ));
        assert_eq!(book.entries().len(), 1);

        let back = book.chart("my turnaround").expect("it is in the book");
        assert_eq!(back.bar_count(), 4);
        assert_eq!(back.to_string(), "| IVmaj7 | III7 | vi7 | I7 |");

        // Through the file, which is the whole point of keeping one.
        let text = serde_json::to_string(&book).unwrap();
        let read: ProgressionBook = serde_json::from_str(&text).unwrap();
        assert_eq!(read, book);
    }

    #[test]
    fn a_mode_travels_with_it_so_the_progression_can_be_quoted_into_the_other_one() {
        // The same rule the built-in catalogue follows: a progression written in minor degrees
        // and asked for in a major key follows its chords rather than its degrees.
        let mut book = ProgressionBook::default();
        book.keep(
            "my lament",
            &chart("| i | bVI | bIII | bVII |"),
            Some(ChartMode::Minor),
        );
        let back = book.chart("my lament").unwrap();
        assert_eq!(back.mode, Some(ChartMode::Minor));

        // And a chart nobody said anything about is taken at face value.
        book.keep("plain", &chart("| I | V |"), None);
        assert_eq!(book.chart("plain").unwrap().mode, None);
    }

    #[test]
    fn keeping_the_same_name_twice_replaces_rather_than_doubles() {
        let mut book = ProgressionBook::default();
        book.keep("mine", &chart("| I | V |"), None);
        book.keep("mine", &chart("| ii | V | I |"), None);
        assert_eq!(book.entries().len(), 1);
        assert_eq!(book.chart("mine").unwrap().bar_count(), 3);

        assert!(book.forget("mine"));
        assert!(!book.forget("mine"));
        assert!(book.chart("mine").is_none());
    }

    #[test]
    fn forgetting_uses_the_same_trimmed_name_as_keeping() {
        let mut book = ProgressionBook::default();
        assert!(book.keep(" mine ", &chart("| I | V |"), None));
        assert!(book.forget("  mine  "));
        assert!(book.entries().is_empty());
    }

    #[test]
    fn a_name_the_catalogue_already_uses_is_refused() {
        // Two `@axis`es would be a picker with the same word in it twice, and a person choosing
        // between them would have no way to tell which one they were about to get.
        let mut book = ProgressionBook::default();
        assert!(!book.keep("axis", &chart("| I | V |"), None));
        assert!(!book.keep("marusa", &chart("| I | V |"), None));
        assert!(!book.keep("   ", &chart("| I | V |"), None));
        assert!(book.entries().is_empty());
    }

    #[test]
    fn an_entry_that_stopped_being_a_chart_is_one_row_missing_rather_than_a_failure() {
        // The file is hand-editable, which means it can say anything.
        let book: ProgressionBook =
            serde_json::from_str(r#"[{"name":"broken","chart":"not a chart"}]"#).unwrap();
        assert_eq!(book.entries().len(), 1);
        assert!(book.chart("broken").is_none());
    }
}
