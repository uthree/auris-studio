//! Preferences that outlive a session, and where they are kept.
//!
//! These are *application* settings, not document settings: which audio device to open, at what
//! rate. A project file never carries them, because the machine that opens the file is rarely
//! the machine that wrote it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::SessionError;

/// Folder name used under the platform's configuration directory.
const APP_FOLDER: &str = "AurisStudio";

/// Audio backend preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPreferences {
    /// Output device to open, by name. `None` follows the system default.
    pub device: Option<String>,
    /// Sample rate to request. `None` takes whatever the device prefers.
    pub sample_rate: Option<u32>,
    /// Callback size to request, in frames.
    pub block_frames: u32,
}

impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: None,
            // ~11 ms at 48 kHz: responsive enough to audition notes against, long enough that
            // per-block overhead stays small.
            block_frames: 512,
        }
    }
}

impl AudioPreferences {
    /// Buffer sizes offered in a settings panel, in frames.
    pub const BLOCK_CHOICES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

    /// Sample rates offered when a device does not advertise a usable list.
    pub const RATE_CHOICES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];

    /// Latency one block represents at `sample_rate`, in milliseconds.
    ///
    /// This is the number that actually means something to a musician; "512 frames" does not
    /// until it is divided by a rate.
    pub fn block_latency_ms(&self, sample_rate: f64) -> f64 {
        if sample_rate <= 0.0 {
            0.0
        } else {
            self.block_frames as f64 / sample_rate * 1000.0
        }
    }
}

/// Everything the application remembers between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Audio backend preferences.
    pub audio: AudioPreferences,
}

impl Settings {
    /// Where the settings file lives.
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    /// Loads the settings, falling back to defaults.
    ///
    /// A missing file is normal on a first run. A *malformed* file is logged and then also
    /// falls back, because refusing to start over a broken preference would be a poor trade.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(error) => {
                log::warn!("ignoring malformed {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the settings, creating the configuration directory if needed.
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
}

/// Directory this application keeps its configuration in.
///
/// Hand-rolled rather than pulled from a crate: it is one match on `cfg!`, and the rules have
/// not changed on any of these platforms in a decade.
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home()
            .join("Library")
            .join("Application Support")
            .join(APP_FOLDER)
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join("AppData").join("Roaming"))
            .join(APP_FOLDER)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join(APP_FOLDER.to_lowercase())
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_json() {
        let settings = Settings::default();
        let text = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_what_it_omits() {
        // Every field is `#[serde(default)]`, so a settings file written by an older build
        // still loads after new preferences are added.
        let settings: Settings = serde_json::from_str(r#"{"audio":{"block_frames":128}}"#).unwrap();
        assert_eq!(settings.audio.block_frames, 128);
        assert_eq!(settings.audio.device, None);
        assert_eq!(settings.audio.sample_rate, None);

        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Settings::default());
    }

    #[test]
    fn block_latency_is_reported_in_milliseconds() {
        let prefs = AudioPreferences {
            block_frames: 512,
            ..AudioPreferences::default()
        };
        assert!((prefs.block_latency_ms(48_000.0) - 10.666_667).abs() < 1e-4);
        // A nonsense rate must not produce a nonsense number.
        assert_eq!(prefs.block_latency_ms(0.0), 0.0);
    }

    #[test]
    fn the_config_path_is_under_the_platform_directory() {
        let path = Settings::path();
        assert!(path.ends_with("settings.json"));
        assert!(path.parent().is_some_and(|parent| {
            parent
                .to_string_lossy()
                .to_lowercase()
                .contains("aurisstudio")
        }));
    }
}
