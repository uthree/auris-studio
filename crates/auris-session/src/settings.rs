//! Preferences that outlive a session, and where they are kept.
//!
//! These are *application* settings, not document settings: which audio device to open, at what
//! rate. A project file never carries them, because the machine that opens the file is rarely
//! the machine that wrote it.

use std::path::{Path, PathBuf};

use auris_i18n::Language;
use auris_io::{
    AudioExportFormat, AudioExportSettings, Mp3Bitrate, WavBitDepth, WavExportSettings,
};
use serde::{Deserialize, Serialize};

use crate::error::SessionError;

/// Folder name used under the user's configuration directory.
const APP_FOLDER: &str = "auris-studio";

/// Environment variable naming the configuration directory outright.
pub const CONFIG_DIR_VAR: &str = "AURIS_CONFIG_DIR";

/// Audio backend preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPreferences {
    /// Audio backend name. `None` selects the platform default.
    pub host: Option<String>,
    /// Output device to open, by name. `None` follows the system default.
    pub device: Option<String>,
    /// Input device to record from, by name. `None` follows the system default.
    ///
    /// ASIO uses the output's driver; the session normalizes this field to match `device`.
    /// Other hosts keep their own field: recording through an interface while listening on
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
            host: None,
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
    /// Whether input and output must share one ASIO driver.
    pub fn uses_asio(&self) -> bool {
        self.host
            .as_deref()
            .is_some_and(|host| host.eq_ignore_ascii_case("ASIO"))
    }

    /// Buffer sizes offered in a settings panel, in frames.
    pub const BLOCK_CHOICES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

    /// Sample rates offered when a device does not advertise a usable list.
    pub const RATE_CHOICES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];

    /// Latency one block represents at `sample_rate`, in milliseconds.
    ///
    /// This is the requested duration of one buffer, not measured device or round-trip latency.
    pub fn block_latency_ms(&self, sample_rate: f64) -> f64 {
        if sample_rate <= 0.0 {
            0.0
        } else {
            self.block_frames as f64 / sample_rate * 1000.0
        }
    }
}

/// Where the window was when it was last put away.
///
/// Plain numbers rather than a toolkit's rectangle, because nothing at this level may name a UI
/// toolkit — and because the file is meant to be readable by a person who has opened it to fix
/// something. The size is what the window restores to, so a maximised window that is unmaximised
/// after being reopened lands back where it was rather than filling the screen for ever.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPlacement {
    /// Distance from the left of the desktop, in logical pixels.
    pub x: f32,
    /// Distance from the top of the desktop.
    pub y: f32,
    /// Width of the window.
    pub width: f32,
    /// Height of the window.
    pub height: f32,
    /// Whether it was maximised. The rectangle above is then the restore size.
    pub maximized: bool,
}

/// How a bounce is written.
///
/// Kept with the settings rather than in the document: the depth somebody masters at is a fact
/// about them and their delivery, not about the song, and a project handed to somebody else
/// should be exported the way *they* export. The desktop copies these values into its export
/// dialog and saves the confirmed choices, so a quick bounce starts with the last delivery's
/// settings without making them part of the project.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPreferences {
    /// Container and codec used for the export.
    pub format: AudioExportFormat,
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
    /// Constant bitrate used for MP3 output.
    pub mp3_bitrate: Mp3Bitrate,
}

impl ExportPreferences {
    /// Common high-quality rates offered for MP3 output.
    pub const MP3_RATE_CHOICES: [u32; 3] = [32_000, 44_100, 48_000];

    /// Whether dither can do anything at the chosen depth.
    ///
    /// A float file stores what the render produced, so there is nothing to dither *to*. The
    /// switch is shown greyed rather than hidden, because a control that disappears when a
    /// neighbour moves reads as a bug in the window.
    pub fn dither_applies(&self) -> bool {
        !matches!(self.format, AudioExportFormat::Mp3) && self.bit_depth.is_integer()
    }

    /// Makes dependent choices valid after changing the output format.
    pub fn normalize_for_project_rate(&mut self, project_rate: f64) {
        if matches!(self.format, AudioExportFormat::Flac)
            && matches!(self.bit_depth, WavBitDepth::Float32)
        {
            self.bit_depth = WavBitDepth::Int24;
        }
        let effective_rate = self
            .sample_rate
            .unwrap_or_else(|| project_rate.round().max(1.0) as u32);
        if !self.format.supports_sample_rate(effective_rate) {
            self.sample_rate = Some(44_100);
        }
    }

    /// Settings for the selected encoder, at the rate the render actually ran at.
    pub fn audio_settings(&self, rendered_rate: f64) -> AudioExportSettings {
        AudioExportSettings {
            format: self.format,
            bit_depth: self.bit_depth,
            sample_rate: rendered_rate.round().max(1.0) as u32,
            dither: self.dither && self.dither_applies(),
            mp3_bitrate: self.mp3_bitrate,
        }
    }

    /// The settings a WAV writer should be given, at the rate the render actually ran at.
    ///
    /// The rate is passed in rather than read from here because those two can disagree: a render
    /// that could not be run at the asked-for rate must not be labelled with it.
    pub fn wav_settings(&self, rendered_rate: f64) -> WavExportSettings {
        let settings = self.audio_settings(rendered_rate);
        WavExportSettings {
            bit_depth: settings.bit_depth,
            sample_rate: settings.sample_rate,
            dither: settings.dither,
        }
    }
}

/// How the built-in agent dials a language model.
///
/// A fact about the machine, like a plugin folder: which server answers, and as which model.
/// `auris-agent` reads these as its flag defaults and the desktop's agent panel reads them as
/// its whole configuration, so setting them once points both doors at the same place. Strings
/// rather than richer types on purpose — the *agent* is where a provider name is validated,
/// and a preference file should not go stale because that list grew.
///
/// The API key is *named*, never stored: `api_key_env` is which environment variable to read,
/// and the value stays in the environment where it was put.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentPreferences {
    /// Live agent mode and operation allow/deny rules. MCP is unaffected.
    pub policy: crate::agent_policy::Policy,
    /// Auto-compaction threshold in percent; absent means 85, zero disables it.
    pub auto_compact_percent: Option<u8>,
    /// Ollama request context window. Absent uses 32768 tokens, independently of server defaults.
    pub context_tokens: Option<u32>,
    /// Ollama output limit per response. Absent uses 4096 tokens.
    pub output_tokens: Option<u32>,
    /// Ollama thinking override; absent keeps the model's default.
    pub thinking: Option<bool>,
    /// The API dialect: "ollama", or "openai" for any OpenAI-compatible endpoint. Empty means
    /// ollama.
    pub provider: String,
    /// The model to ask for, in the provider's own naming. Empty means not configured.
    pub model: String,
    /// Base URL override. Empty uses the provider's default.
    pub url: String,
    /// Environment variable holding the API key. Empty means no key.
    pub api_key_env: String,
}

impl AgentPreferences {
    /// Whether enough is set for the agent to place a call at all.
    ///
    /// The model is the one field with no sensible default — everything else falls back.
    pub fn is_configured(&self) -> bool {
        !self.model.trim().is_empty()
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
    /// Keep a recovery snapshot in private working storage as the document changes.
    ///
    /// On unless turned off. The snapshot never replaces the user-chosen project file; see
    /// [`should_autosave`](crate::session::should_autosave).
    pub autosave: bool,
    /// Snap a note's duration to the editing grid while its right edge is dragged.
    ///
    /// On by default. This is an editing preference rather than project data: two people can
    /// shape the same notes with different pointer behaviour without changing the file merely by
    /// opening it.
    pub snap_note_lengths: bool,
    /// How a bounce is written.
    pub export: ExportPreferences,
    /// Where the window was when it was last put away. `None` on a first run.
    pub window: Option<WindowPlacement>,
    /// Projects opened lately, most recent first.
    ///
    /// Capped at [`Settings::RECENT`]. A path rather than a handle, so a project moved or
    /// deleted since simply fails to open and says so — checking the disk to draw a menu would
    /// mean waking a sleeping network share every time somebody looked at File.
    pub recent: Vec<PathBuf>,
    /// Extra places to look for CLAP plugins, on top of the conventional folders.
    ///
    /// Each is a `.clap` taken as it stands or a directory walked for them. Kept here rather
    /// than in the document because a plugin folder is a fact about the machine: a project
    /// carried to another one names the plugins it uses, and where *that* machine keeps them is
    /// that machine's business.
    pub plugin_paths: Vec<PathBuf>,
    /// Folder holding a compiled Japanese dictionary — a prebuilt `naist-jdic` — for reading
    /// kanji lyrics on a singer track.
    ///
    /// A fact about the machine for the reason a plugin folder is: kana lyrics need nothing
    /// installed, and a document written with kanji opens fine on a machine without this — only
    /// the command that turns *new* kanji into phonemes asks for it, and it names this setting
    /// when it is missing.
    pub japanese_dictionary: Option<PathBuf>,
    /// Where a singer voice's inference runs: on the GPU when one is offered, or on the CPU.
    ///
    /// Auto by default, which sings on the GPU wherever the runtime has one to offer —
    /// DirectML on Windows, Core ML on macOS — and on the CPU everywhere else. A machine
    /// fact like the device above it: the same project renders through whichever of these
    /// the machine that opens it prefers, and a frozen take keeps what it was sung with.
    pub singer_acceleration: auris_singer::Acceleration,
    /// Extra folders holding singer voice models, on top of the `Voices` library folders.
    ///
    /// Each is a directory searched for supported voice entries by
    /// [`crate::library::installed_voices_in`] — the [`Self::plugin_paths`] arrangement,
    /// and a fact about the machine for the same reason: a voice is hundreds of
    /// megabytes somebody keeps where they keep it, and registering it in the library means
    /// remembering where it lies, never copying it.
    pub voice_paths: Vec<PathBuf>,
    /// How the built-in agent dials a language model.
    pub agent: AgentPreferences,
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
            snap_note_lengths: true,
            export: ExportPreferences::default(),
            window: None,
            recent: Vec::new(),
            plugin_paths: Vec::new(),
            japanese_dictionary: None,
            singer_acceleration: auris_singer::Acceleration::default(),
            voice_paths: Vec::new(),
            agent: AgentPreferences::default(),
        }
    }
}

impl Settings {
    /// How many recently opened projects are remembered.
    ///
    /// Ten. Long enough to hold a week of work on two or three pieces, short enough that the
    /// list is still something an eye takes in at once rather than a second file dialog.
    pub const RECENT: usize = 10;

    /// Puts `path` at the head of the recent list, without letting it appear twice.
    ///
    /// Called on opening and on saving under a new name — the two moments at which a path
    /// becomes the one being worked on. Saving over the same file does not reorder anything,
    /// because it was already at the top.
    pub fn remember_recent(&mut self, path: &Path) {
        self.recent.retain(|kept| kept != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(Self::RECENT);
    }

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
        match serde_json::from_str::<Self>(&text) {
            Ok(mut settings) => {
                settings.recent.truncate(Self::RECENT);
                settings.audio.sample_rate = settings.audio.sample_rate.map(|rate| rate.max(8_000));
                settings.audio.block_frames = settings.audio.block_frames.max(1);
                settings
            }
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

/// The user's home directory.
///
/// `USERPROFILE` before `HOME` on Windows, where nothing sets `HOME` unless a Unix-flavoured
/// shell has been installed.
fn home() -> PathBuf {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };

    names
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
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
        // A hand-written settings file can specify only the preferences it overrides.
        let settings: Settings = serde_json::from_str(r#"{"audio":{"block_frames":128}}"#).unwrap();
        assert_eq!(settings.audio.block_frames, 128);
        assert_eq!(settings.audio.device, None);
        assert_eq!(settings.audio.host, None);
        assert_eq!(settings.audio.sample_rate, None);
        assert!(settings.snap_note_lengths);

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
    fn an_explicit_host_survives_a_settings_round_trip() {
        let settings: Settings = serde_json::from_str(
            r#"{"audio":{"host":"ASIO","device":"Interface","block_frames":128}}"#,
        )
        .unwrap();
        assert!(settings.audio.uses_asio());
        let saved = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap(), settings);
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
    fn dither_is_dropped_at_a_depth_that_cannot_use_it() {
        // Asked for and impossible: a float file stores what the render produced, so there is
        // nothing to dither *to*. The preference is kept as it was — moving to float and back
        // must not silently turn the switch off — and simply not applied.
        let float = ExportPreferences {
            bit_depth: WavBitDepth::Float32,
            dither: true,
            sample_rate: None,
            ..ExportPreferences::default()
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
            ..ExportPreferences::default()
        };
        assert_eq!(asked.wav_settings(48_000.0).sample_rate, 48_000);
    }

    #[test]
    fn changing_format_normalizes_only_incompatible_choices() {
        let mut flac = ExportPreferences {
            format: AudioExportFormat::Flac,
            bit_depth: WavBitDepth::Float32,
            ..ExportPreferences::default()
        };
        flac.normalize_for_project_rate(96_000.0);
        assert_eq!(flac.bit_depth, WavBitDepth::Int24);
        assert_eq!(flac.sample_rate, None);

        let mut mp3 = ExportPreferences {
            format: AudioExportFormat::Mp3,
            sample_rate: None,
            dither: true,
            ..ExportPreferences::default()
        };
        mp3.normalize_for_project_rate(96_000.0);
        assert_eq!(mp3.sample_rate, Some(44_100));
        assert!(!mp3.audio_settings(44_100.0).dither);
    }

    #[test]
    fn the_recent_list_holds_each_project_once_and_the_newest_first() {
        let mut settings = Settings::default();
        settings.remember_recent(Path::new("/songs/One.auris"));
        settings.remember_recent(Path::new("/songs/Two.auris"));
        // Opening the first one again moves it back to the top rather than listing it twice.
        settings.remember_recent(Path::new("/songs/One.auris"));
        assert_eq!(
            settings.recent,
            vec![
                PathBuf::from("/songs/One.auris"),
                PathBuf::from("/songs/Two.auris")
            ]
        );
    }

    #[test]
    fn the_recent_list_stops_at_a_length_an_eye_can_take_in() {
        let mut settings = Settings::default();
        for n in 0..Settings::RECENT + 5 {
            settings.remember_recent(&PathBuf::from(format!("/songs/{n}.auris")));
        }
        assert_eq!(settings.recent.len(), Settings::RECENT);
        // And it is the oldest that fell off, not the newest.
        assert_eq!(
            settings.recent.first(),
            Some(&PathBuf::from(format!(
                "/songs/{}.auris",
                Settings::RECENT + 4
            )))
        );
    }
}
