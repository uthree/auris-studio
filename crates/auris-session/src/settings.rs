//! Preferences that outlive a session, and where they are kept.
//!
//! These are *application* settings, not document settings: which audio device to open, at what
//! rate. A project file never carries them, because the machine that opens the file is rarely
//! the machine that wrote it.

use std::path::{Path, PathBuf};

use auris_i18n::Language;
use auris_io::{WavBitDepth, WavExportSettings};
use serde::{Deserialize, Serialize};

use crate::error::SessionError;

/// Folder name used under the user's configuration directory.
const APP_FOLDER: &str = "auris-studio";

/// Folder name earlier builds used, under the platform's own application-data directory.
const LEGACY_APP_FOLDER: &str = "AurisStudio";

/// Environment variable naming the configuration directory outright.
pub const CONFIG_DIR_VAR: &str = "AURIS_CONFIG_DIR";

/// Audio backend preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPreferences {
    /// Output device to open, by name. `None` follows the system default.
    pub device: Option<String>,
    /// Input device to record from, by name. `None` follows the system default.
    ///
    /// Its own field rather than a shared one: recording through an interface while listening on
    /// the laptop's own output is the ordinary arrangement, not the exotic one. The rate and
    /// block size are not repeated — a take asks for the project's rate and the same block size
    /// as playback, and a second pair of controls for numbers nobody would set differently would
    /// be two more things to get wrong.
    pub input_device: Option<String>,
    /// Sample rate to request. `None` takes whatever the device prefers.
    pub sample_rate: Option<u32>,
    /// Callback size to request, in frames.
    pub block_frames: u32,
}

impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            device: None,
            input_device: None,
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

/// How a bounce is written.
///
/// Kept with the settings rather than in the document: the depth somebody masters at is a fact
/// about them and their delivery, not about the song, and a project handed to somebody else
/// should be exported the way *they* export. It is also why there is no dialog in front of the
/// save sheet — an export that asks three questions every time is an export people stop using
/// for a quick listen.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPreferences {
    /// Sample format written to the file.
    pub bit_depth: WavBitDepth,
    /// Add TPDF dither before quantising. Only ever applied at an integer depth — see
    /// [`Self::dither_applies`].
    pub dither: bool,
    /// Rate to render and write at. `None` uses the project's own rate.
    ///
    /// Rendering at a rate is not the same as writing one into the header: the render is done at
    /// this rate, so asking for 44.1 from a 48 kHz project resamples the whole mix rather than
    /// mislabelling it.
    pub sample_rate: Option<u32>,
}

impl ExportPreferences {
    /// Whether dither can do anything at the chosen depth.
    ///
    /// A float file stores what the render produced, so there is nothing to dither *to*. The
    /// switch is shown greyed rather than hidden, because a control that disappears when a
    /// neighbour moves reads as a bug in the window.
    pub fn dither_applies(&self) -> bool {
        self.bit_depth.is_integer()
    }

    /// The settings a WAV writer should be given, at the rate the render actually ran at.
    ///
    /// The rate is passed in rather than read from here because those two can disagree: a render
    /// that could not be run at the asked-for rate must not be labelled with it.
    pub fn wav_settings(&self, rendered_rate: f64) -> WavExportSettings {
        WavExportSettings {
            bit_depth: self.bit_depth,
            sample_rate: rendered_rate.round().max(1.0) as u32,
            dither: self.dither && self.dither_applies(),
        }
    }
}

/// Everything the application remembers between runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Audio backend preferences.
    pub audio: AudioPreferences,
    /// Interface language. `None` follows the system.
    ///
    /// The *window's* language, and no other frontend's: `auris` prints English whatever this
    /// says, because a terminal cannot promise to render anything else. It stays down here rather
    /// than in `auris-gpui` because it is a preference like the sample rate — a fact about the
    /// installation, kept where every frontend can read it — and because a second frontend with a
    /// window of its own should find it already answered.
    pub language: Option<Language>,
    /// Write the document back over itself as it changes, once it has been saved somewhere.
    ///
    /// On unless turned off. What it costs when it is on — "close without saving" stops being a
    /// way to undo an afternoon — is set out beside
    /// [`should_autosave`](crate::session::should_autosave), and is the reason this is a setting
    /// rather than simply how the application behaves.
    pub autosave: bool,
    /// How a bounce is written.
    pub export: ExportPreferences,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio: AudioPreferences::default(),
            language: None,
            // Written out rather than derived, because `bool`'s own default is the wrong one here
            // and a settings file written before this field existed is filled in from exactly
            // this value.
            autosave: true,
            export: ExportPreferences::default(),
        }
    }
}

impl Settings {
    /// The language to use, resolving "follow the system" against the environment.
    pub fn language(&self) -> Language {
        Language::resolve(self.language)
    }

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
/// `~/.config/auris-studio` on every platform, including the two that have a convention of their
/// own. That is deliberate: these files are small, hand-editable and worth version-controlling,
/// and the people who do that keep a dotfiles repository checked out over `~/.config`. A
/// configuration in `%APPDATA%` or in `~/Library/Application Support` cannot join it.
///
/// [`CONFIG_DIR_VAR`] overrides the answer outright, and `XDG_CONFIG_HOME` moves the parent —
/// so a dotfiles setup that already relocates one can relocate this too.
pub fn config_dir() -> PathBuf {
    resolve_config_dir(
        std::env::var_os(CONFIG_DIR_VAR).map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        &home(),
    )
}

/// [`config_dir`] with the environment passed in, so it can be tested without setting variables.
///
/// An empty variable counts as unset. A shell that exports `XDG_CONFIG_HOME=` would otherwise
/// put the configuration in `/auris-studio`.
fn resolve_config_dir(override_dir: Option<PathBuf>, xdg: Option<PathBuf>, home: &Path) -> PathBuf {
    let named = |dir: PathBuf| (!dir.as_os_str().is_empty()).then_some(dir);
    if let Some(dir) = override_dir.and_then(named) {
        return dir;
    }
    xdg.and_then(named)
        .unwrap_or_else(|| home.join(".config"))
        .join(APP_FOLDER)
}

/// Directory builds before the move to [`config_dir`] kept their configuration in.
///
/// Read once, by [`migrate_legacy_config`], and never written to.
fn legacy_config_dir() -> PathBuf {
    let home = home();
    if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join(LEGACY_APP_FOLDER)
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"))
            .join(LEGACY_APP_FOLDER)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join(LEGACY_APP_FOLDER.to_lowercase())
    }
}

/// Carries a configuration written before the move into the directory this build reads.
///
/// Call once at start-up, before anything is loaded. Returns what was carried across.
///
/// Every file is taken, not a list of the ones this crate knows about: `keymap.json` and
/// `appearance.json` belong to the desktop frontend, and nothing at this level may name them.
pub fn migrate_legacy_config() -> Vec<PathBuf> {
    migrate_config(&legacy_config_dir(), &config_dir())
}

/// [`migrate_legacy_config`] with both directories passed in, so it can be tested.
///
/// Copies rather than moves, and never over a file that is already there. An older build left
/// running keeps working, and running this twice does nothing the second time — which matters,
/// because it runs on every start-up rather than behind a flag saying it has happened.
fn migrate_config(from: &Path, to: &Path) -> Vec<PathBuf> {
    if from == to {
        return Vec::new();
    }
    // Nothing to carry is the ordinary case, not a failure: it is what a first run on a new
    // machine finds, and what every run after the first one finds too.
    let Ok(entries) = std::fs::read_dir(from) else {
        return Vec::new();
    };
    let pending: Vec<(PathBuf, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|source| source.is_file())
        .filter_map(|source| {
            let destination = to.join(source.file_name()?);
            (!destination.exists()).then_some((source, destination))
        })
        .collect();
    if pending.is_empty() {
        return Vec::new();
    }
    if let Err(error) = std::fs::create_dir_all(to) {
        log::warn!("could not create {}: {error}", to.display());
        return Vec::new();
    }

    let mut carried = Vec::new();
    for (source, destination) in pending {
        match std::fs::copy(&source, &destination) {
            Ok(_) => {
                log::info!("carried {} to {}", source.display(), destination.display());
                carried.push(destination);
            }
            Err(error) => log::warn!("could not carry {}: {error}", source.display()),
        }
    }
    carried
}

/// The user's home directory.
///
/// `USERPROFILE` before `HOME` on Windows, where nothing sets `HOME` unless a Unix-flavoured
/// shell has been installed — and where this is only reached at all when `APPDATA` is missing,
/// which is already a strange enough machine to be worth landing somewhere sensible on.
fn home() -> PathBuf {
    #[cfg(target_os = "windows")]
    let names: [&str; 2] = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let names: [&str; 1] = ["HOME"];

    names
        .into_iter()
        .find_map(std::env::var_os)
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
    fn the_config_path_is_the_same_dotfile_path_on_every_platform() {
        // The point of the whole arrangement: a dotfiles repository checked out over `~/.config`
        // finds the file at the same place on a Mac and on Windows.
        let path = Settings::path();
        assert!(path.ends_with("settings.json"));
        assert!(path.parent().is_some_and(|parent| {
            parent.ends_with(Path::new(".config").join(APP_FOLDER))
                || std::env::var_os(CONFIG_DIR_VAR).is_some()
                || std::env::var_os("XDG_CONFIG_HOME").is_some()
        }));
    }

    #[test]
    fn the_environment_can_move_the_configuration_and_an_empty_variable_cannot() {
        let home = Path::new("/home/somebody");

        assert_eq!(
            resolve_config_dir(None, None, home),
            home.join(".config").join(APP_FOLDER)
        );
        // The override names the directory itself rather than its parent — the point is to say
        // "read exactly this", which a symlinked dotfiles checkout wants to be able to do.
        assert_eq!(
            resolve_config_dir(Some(PathBuf::from("/dotfiles/auris")), None, home),
            PathBuf::from("/dotfiles/auris")
        );
        assert_eq!(
            resolve_config_dir(None, Some(PathBuf::from("/elsewhere")), home),
            Path::new("/elsewhere").join(APP_FOLDER)
        );
        // `export XDG_CONFIG_HOME=` is a shell being unhelpful, not a request to write to the
        // root of the filesystem.
        assert_eq!(
            resolve_config_dir(Some(PathBuf::new()), Some(PathBuf::new()), home),
            home.join(".config").join(APP_FOLDER)
        );
    }

    #[test]
    fn an_earlier_configuration_is_carried_across_once_and_never_over_a_newer_one() {
        let root = std::env::temp_dir().join(format!(
            "auris-migrate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let (from, to) = (root.join("legacy"), root.join("config"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("settings.json"), "old settings").unwrap();
        // A file belonging to a frontend, which this crate must carry without naming.
        std::fs::write(from.join("keymap.json"), "old keymap").unwrap();
        // Already answered for in the new place: the old one must not win.
        std::fs::write(from.join("appearance.json"), "old scheme").unwrap();
        std::fs::write(to.join("appearance.json"), "chosen since").unwrap();

        let mut carried = migrate_config(&from, &to);
        carried.sort();
        assert_eq!(
            carried,
            vec![to.join("keymap.json"), to.join("settings.json")]
        );
        assert_eq!(
            std::fs::read_to_string(to.join("settings.json")).unwrap(),
            "old settings"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("appearance.json")).unwrap(),
            "chosen since"
        );
        // Copied, not moved: an older build left running keeps its own configuration.
        assert!(from.join("settings.json").exists());

        // Every start-up runs this, so the second time must be a no-op rather than a restore of
        // whatever has been changed since.
        std::fs::write(to.join("settings.json"), "changed since").unwrap();
        assert!(migrate_config(&from, &to).is_empty());
        assert_eq!(
            std::fs::read_to_string(to.join("settings.json")).unwrap(),
            "changed since"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn there_is_nothing_to_carry_when_the_old_directory_is_the_new_one() {
        // What every Linux machine that already used `~/.config` will hit, and a directory
        // walked into itself would be at best pointless.
        let same = std::env::temp_dir().join("auris-same");
        assert!(migrate_config(&same, &same).is_empty());
        // A directory that was never there is the ordinary first run, not an error.
        assert!(migrate_config(&same.join("missing"), &same.join("also-missing")).is_empty());
        assert!(!same.join("also-missing").exists());
    }

    #[test]
    fn dither_is_dropped_at_a_depth_that_cannot_use_it() {
        // Asked for and impossible: a float file stores what the render produced, so there is
        // nothing to dither *to*. The preference is kept as it was — moving to float and back
        // must not silently turn the switch off — and simply not applied.
        let float = ExportPreferences {
            bit_depth: WavBitDepth::Float32,
            dither: true,
            sample_rate: None,
        };
        assert!(!float.dither_applies());
        assert!(!float.wav_settings(48_000.0).dither);
        assert!(float.dither, "the preference itself is not rewritten");

        let sixteen = ExportPreferences {
            bit_depth: WavBitDepth::Int16,
            ..float
        };
        assert!(sixteen.wav_settings(48_000.0).dither);
    }

    #[test]
    fn the_file_is_labelled_with_the_rate_it_was_rendered_at() {
        // Not with the one that was asked for. A render that could not run at 44.1 must not
        // produce a file claiming it did — the samples would play back at the wrong speed.
        let asked = ExportPreferences {
            bit_depth: WavBitDepth::Int24,
            dither: false,
            sample_rate: Some(44_100),
        };
        assert_eq!(asked.wav_settings(48_000.0).sample_rate, 48_000);
    }
}
