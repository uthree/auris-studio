//! Which colour scheme the window is drawn in, and where that choice is kept.
//!
//! Its own preferences file rather than a field on [`Settings`](auris_session::Settings), where
//! the interface language lives. The language is kept down there because the command line
//! frontend answers in it too, and being told twice is how the two come to disagree. A colour
//! scheme has no such second reader: nothing at or below `auris-session` has a window to paint.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::{DEFAULT_SCHEME, Theme, scheme};

/// Everything in `appearance.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Id of the chosen colour scheme.
    pub scheme: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            scheme: DEFAULT_SCHEME.to_string(),
        }
    }
}

impl Appearance {
    /// Where the file lives.
    pub fn path() -> PathBuf {
        auris_session::config_dir().join("appearance.json")
    }

    /// Loads the file, falling back to the defaults.
    ///
    /// A missing file is a first run. A malformed one is logged and then also falls back, and so
    /// is a scheme this build does not have — the file is user-editable text that outlives the
    /// build that wrote it, and neither case is worth refusing to start over.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(appearance) if scheme(&appearance.scheme).is_some() => appearance,
            Ok(appearance) => {
                log::warn!("ignoring unknown colour scheme `{}`", appearance.scheme);
                Self::default()
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

    /// The palette this choice describes.
    pub fn theme(&self) -> Theme {
        Theme::named(&self.scheme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_or_unreadable_file_is_the_default_scheme() {
        let empty: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Appearance::default());
        assert_eq!(empty.theme().scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn a_scheme_this_build_does_not_have_costs_its_colour_and_nothing_else() {
        // The file outlives the build that wrote it, so this is reachable without anybody doing
        // anything wrong — and starting in the default palette beats not starting.
        let stored: Appearance = serde_json::from_str(r#"{"scheme":"chartreuse"}"#).unwrap();
        assert_eq!(stored.theme().scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn a_chosen_scheme_round_trips() {
        let chosen = Appearance {
            scheme: "daylight".to_string(),
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert_eq!(serde_json::from_str::<Appearance>(&text).unwrap(), chosen);
        assert_eq!(chosen.theme().scheme, "daylight");
    }
}
