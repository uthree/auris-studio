//! Configuration and lifecycle commands for external singing backends.
//!
//! These are session-level commands rather than gpui helpers so every frontend can create the
//! same files, validate the same fields and start the same engine without duplicating policy.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::library::VOICES_FOLDER;
use crate::settings::config_dir;

/// A VOICEVOX Engine connection that can be written as an Auris voice entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoicevoxSetup {
    /// Name shown on the voice shelf.
    pub name: String,
    /// Base URL of the running Engine.
    pub url: String,
    /// Sample rate returned by frame synthesis.
    pub sample_rate: u32,
    /// Frames per second used by the Engine.
    pub frame_rate: f64,
    /// Name shown for this pair of singing styles.
    pub style_name: String,
    /// Style used by `/sing_frame_audio_query`.
    pub query_style_id: u32,
    /// Style used by `/frame_synthesis`.
    pub decode_style_id: u32,
}

impl Default for VoicevoxSetup {
    fn default() -> Self {
        Self {
            name: "VOICEVOX singer".into(),
            url: "http://127.0.0.1:50021".into(),
            sample_rate: 24_000,
            frame_rate: 93.75,
            style_name: "Singer / normal".into(),
            query_style_id: 6000,
            decode_style_id: 3001,
        }
    }
}

/// One named singing style advertised by a VOICEVOX Engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicevoxStyle {
    /// Engine style ID, written to the connection file after a name is chosen.
    pub id: u32,
    /// Singer's display name.
    pub singer: String,
    /// Style's display name within that singer.
    pub name: String,
}

/// A named pair of Engine styles that can be selected for a singer track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoicevoxSpeakerChoice {
    /// The singer and singing style shown to the user.
    pub name: String,
    /// Style used by `/sing_frame_audio_query`.
    pub query_style_id: u32,
    /// Style used by `/frame_synthesis`.
    pub decode_style_id: u32,
}

/// An unchanged connection and speaker selection against which an Engine request was made.
///
/// Reading this snapshot never contacts the Engine or loads a neural model. Its private source
/// bytes let a later selection refuse an outdated catalogue instead of overwriting a newer file.
#[derive(Clone, Debug, PartialEq)]
pub struct VoicevoxConnection {
    /// The connection file resolved against the project folder.
    pub path: PathBuf,
    /// Connection name shown on the voice shelf.
    pub name: String,
    /// Base URL of the Engine to query.
    pub url: String,
    /// Sample rate returned by frame synthesis.
    pub sample_rate: u32,
    /// Frames per second used by the Engine.
    pub frame_rate: f64,
    /// The saved style name, or the first style's name for the default speaker.
    pub speaker: String,
    /// The selected style used by `/sing_frame_audio_query`.
    pub query_style_id: u32,
    /// The selected style used by `/frame_synthesis`.
    pub decode_style_id: u32,
    pub(crate) track: auris_core::TrackId,
    saved_speaker: Option<String>,
    raw: Vec<u8>,
    styles: Vec<VoicevoxSpeakerChoice>,
}

impl VoicevoxStyle {
    /// The singer and style together, so identically named styles remain distinguishable.
    pub fn label(&self) -> String {
        format!("{} / {}", self.singer, self.name)
    }
}

/// A connected Engine's version and the two kinds of singing styles it supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicevoxCatalog {
    /// Engine version reported by `/version`.
    pub version: String,
    /// Styles accepted by `/sing_frame_audio_query`.
    pub query: Vec<VoicevoxStyle>,
    /// Styles accepted by `/frame_synthesis`.
    pub decode: Vec<VoicevoxStyle>,
}

impl VoicevoxCatalog {
    /// Offers every singing voice by name, retaining the selected melody style when valid.
    ///
    /// Engines may expose one singing teacher for many decoding voices. If the saved teacher
    /// disappeared, prefer a query style belonging to the selected singer, then the first query
    /// style the Engine advertises. Existing connection entries keep their names and positions.
    pub fn speaker_choices(&self, connection: &VoicevoxConnection) -> Vec<VoicevoxSpeakerChoice> {
        let mut choices = Vec::new();
        for decode in &self.decode {
            let query = self
                .query
                .iter()
                .find(|style| style.id == connection.query_style_id)
                .or_else(|| {
                    self.query
                        .iter()
                        .find(|style| style.singer == decode.singer)
                })
                .or_else(|| self.query.first());
            let Some(query) = query else { continue };
            let label = decode.label();
            let mut name = label.clone();
            let mut suffix = 0;
            while connection.styles.iter().chain(&choices).any(|style| {
                style.name == name
                    && (style.query_style_id != query.id || style.decode_style_id != decode.id)
            }) {
                suffix += 1;
                name = match suffix {
                    1 => format!("{label} ({})", decode.id),
                    _ => format!("{label} ({}; {suffix})", decode.id),
                };
            }
            let choice = VoicevoxSpeakerChoice {
                name,
                query_style_id: query.id,
                decode_style_id: decode.id,
            };
            if !choices.contains(&choice) {
                choices.push(choice);
            }
        }
        choices
    }

    /// Checks a configured pair against the Engine that supplied this catalogue.
    pub fn validate_styles(&self, setup: &VoicevoxSetup) -> Result<(), VoiceSetupError> {
        if !self
            .query
            .iter()
            .any(|style| style.id == setup.query_style_id)
            || !self
                .decode
                .iter()
                .any(|style| style.id == setup.decode_style_id)
        {
            return Err(VoiceSetupError::Connection(format!(
                "VOICEVOX styles were not found (query {}, decode {})",
                setup.query_style_id, setup.decode_style_id
            )));
        }
        Ok(())
    }
}

/// The supported part of a DiffSinger deployment configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffSingerSetup {
    /// Voicebank folder in which `dsconfig.yaml` is written.
    #[serde(skip)]
    pub folder: PathBuf,
    /// Phoneme vocabulary, relative to the voicebank folder.
    pub phonemes: String,
    /// Acoustic ONNX model, relative to the voicebank folder.
    pub acoustic: String,
    /// Vocoder folder, relative to the voicebank folder.
    pub vocoder: String,
    /// Audio sample rate.
    pub sample_rate: u32,
    /// Samples represented by one acoustic frame.
    pub hop_size: u32,
    /// Number of mel bins produced by the acoustic model.
    pub num_mel_bins: usize,
    /// Mel logarithm base, either `10` or `e`.
    pub mel_base: String,
    /// Whether the acoustic model accepts a continuous diffusion step count.
    pub use_continuous_acceleration: bool,
    /// Whether the acoustic model accepts a variable diffusion depth.
    pub use_variable_depth: bool,
    /// Whether the model accepts the neutral key-shift input.
    pub use_key_shift_embed: bool,
    /// Whether the model accepts the neutral speed input.
    pub use_speed_embed: bool,
}

impl Default for DiffSingerSetup {
    fn default() -> Self {
        Self {
            folder: PathBuf::new(),
            phonemes: "phonemes.txt".into(),
            acoustic: "acoustic.onnx".into(),
            vocoder: "dsvocoder".into(),
            sample_rate: 44_100,
            hop_size: 512,
            num_mel_bins: 128,
            mel_base: "10".into(),
            use_continuous_acceleration: false,
            use_variable_depth: false,
            use_key_shift_embed: false,
            use_speed_embed: false,
        }
    }
}

/// A problem preparing or contacting an external singing backend.
#[derive(Debug, thiserror::Error)]
pub enum VoiceSetupError {
    /// A field does not describe a usable configuration.
    #[error("{0}")]
    Invalid(String),
    /// A configuration or executable could not be read or written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The VOICEVOX Engine could not be contacted or rejected the request.
    #[error("{0}")]
    Connection(String),
    /// A configuration could not be encoded.
    #[error("{0}")]
    Encode(String),
}

pub(crate) fn read_voicevox_connection(
    path: &Path,
    speaker: Option<&str>,
    track: auris_core::TrackId,
) -> Result<VoicevoxConnection, VoiceSetupError> {
    let raw = std::fs::read(path)?;
    let file: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
    if file
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(VoiceSetupError::Invalid(
            "Unsupported VOICEVOX connection format".into(),
        ));
    }
    let styles: Vec<VoicevoxSpeakerChoice> = serde_json::from_value(file["styles"].clone())
        .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
    let style = match speaker {
        Some(name) => styles.iter().find(|style| style.name == name),
        None => styles.first(),
    }
    .ok_or_else(|| VoiceSetupError::Invalid("The selected VOICEVOX style is missing".into()))?;
    let sample_rate = match file.get("sample_rate") {
        Some(value) => value.as_u64().and_then(|rate| u32::try_from(rate).ok()),
        None => Some(24_000),
    }
    .ok_or_else(|| VoiceSetupError::Invalid("Invalid VOICEVOX sample rate".into()))?;
    let frame_rate = match file.get("frame_rate") {
        Some(value) => value.as_f64(),
        None => Some(93.75),
    }
    .ok_or_else(|| VoiceSetupError::Invalid("Invalid VOICEVOX frame rate".into()))?;
    let text = |key: &str, default: &str| -> Result<String, VoiceSetupError> {
        match file.get(key) {
            Some(value) => value.as_str().map(str::to_string),
            None => Some(default.to_string()),
        }
        .ok_or_else(|| VoiceSetupError::Invalid(format!("Invalid VOICEVOX {key}")))
    };
    let connection = VoicevoxConnection {
        path: path.to_path_buf(),
        name: text("name", "VOICEVOX")?,
        url: text("url", "http://127.0.0.1:50021")?
            .trim_end_matches('/')
            .into(),
        sample_rate,
        frame_rate,
        speaker: style.name.clone(),
        query_style_id: style.query_style_id,
        decode_style_id: style.decode_style_id,
        track,
        saved_speaker: speaker.map(str::to_string),
        raw,
        styles,
    };
    validate_voicevox_url(&connection.url)?;
    if sample_rate == 0 || !frame_rate.is_finite() || frame_rate <= 0.0 {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX sample rate and frame rate must be positive".into(),
        ));
    }
    Ok(connection)
}

pub(crate) fn append_voicevox_speaker(
    connection: &VoicevoxConnection,
    choice: &VoicevoxSpeakerChoice,
) -> Result<(), VoiceSetupError> {
    if choice.name.trim().is_empty() {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX style name cannot be empty".into(),
        ));
    }
    if std::fs::read(&connection.path)? != connection.raw {
        return Err(VoiceSetupError::Invalid(
            "The VOICEVOX connection changed; fetch the singers again".into(),
        ));
    }
    if let Some(existing) = connection
        .styles
        .iter()
        .find(|style| style.name == choice.name)
    {
        return if existing == choice {
            Ok(())
        } else {
            Err(VoiceSetupError::Invalid(
                "The VOICEVOX style name already belongs to a different voice".into(),
            ))
        };
    }
    let mut file: serde_json::Value = serde_json::from_slice(&connection.raw)
        .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
    file["styles"]
        .as_array_mut()
        .expect("the snapshot validated the styles")
        .push(
            serde_json::to_value(choice)
                .map_err(|error| VoiceSetupError::Encode(error.to_string()))?,
        );
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| VoiceSetupError::Encode(error.to_string()))?;
    replace_voicevox_file(&connection.path, &bytes, Some(&connection.raw))
}

fn replace_voicevox_file(
    path: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
) -> Result<(), VoiceSetupError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

    let temporary = path.with_file_name(format!(
        ".auris-voicevox-{}-{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = (|| -> Result<(), VoiceSetupError> {
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        // Check again after the write: the catalogue may have been open for several minutes.
        let unchanged = match expected {
            Some(original) => std::fs::read(path)? == original,
            None => !path.try_exists()?,
        };
        if !unchanged {
            return Err(VoiceSetupError::Invalid(
                "The VOICEVOX connection changed; fetch the singers again".into(),
            ));
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Writes a VOICEVOX connection into the application's managed Voices folder.
///
/// Updating an existing connection retains all other named styles. A selected style with the
/// same name is updated in place; a new name is appended, so track selections remain valid.
pub fn write_voicevox_connection(setup: &VoicevoxSetup) -> Result<PathBuf, VoiceSetupError> {
    let folder = config_dir().join(VOICES_FOLDER);
    write_voicevox_connection_in(setup, &folder)
}

fn write_voicevox_connection_in(
    setup: &VoicevoxSetup,
    folder: &Path,
) -> Result<PathBuf, VoiceSetupError> {
    validate_voicevox(setup)?;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(format!("{}.voicevox.json", safe_name(&setup.name)));
    let original = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut styles: Vec<VoicevoxSpeakerChoice> = match &original {
        Some(bytes) => {
            // A corrupt or newer-format connection must not be silently replaced with defaults.
            // VOICEVOX loading only reads JSON; it neither contacts the Engine nor runs inference.
            auris_singer::VoiceModel::load(&path, auris_singer::Acceleration::Auto)
                .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
            let file: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
            serde_json::from_value(file["styles"].clone())
                .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?
        }
        None => Vec::new(),
    };
    let selected = VoicevoxSpeakerChoice {
        name: setup.style_name.trim().into(),
        query_style_id: setup.query_style_id,
        decode_style_id: setup.decode_style_id,
    };
    match styles.iter_mut().find(|style| style.name == selected.name) {
        Some(existing) => *existing = selected,
        None => styles.push(selected),
    }
    let file = serde_json::json!({
        "format_version": 1,
        "name": setup.name.trim(),
        "url": setup.url.trim().trim_end_matches('/'),
        "sample_rate": setup.sample_rate,
        "frame_rate": setup.frame_rate,
        "styles": styles,
    });
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| VoiceSetupError::Encode(error.to_string()))?;
    replace_voicevox_file(&path, &bytes, original.as_deref())?;
    Ok(path)
}

/// Contacts VOICEVOX and verifies that both configured singing style IDs are advertised.
pub fn check_voicevox_connection(setup: &VoicevoxSetup) -> Result<String, VoiceSetupError> {
    validate_voicevox(setup)?;
    let catalog = fetch_voicevox_catalog(&setup.url)?;
    catalog.validate_styles(setup)?;
    Ok(catalog.version)
}

/// Fetches named singing styles without requiring a caller to know any Engine style IDs.
///
/// This is a blocking HTTP operation; a graphical frontend must run it on a worker thread.
pub fn fetch_voicevox_catalog(url: &str) -> Result<VoicevoxCatalog, VoiceSetupError> {
    let root = url.trim().trim_end_matches('/');
    validate_voicevox_url(root)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    let version = agent
        .get(&format!("{root}/version"))
        .call()
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /version: {error}")))?
        .into_string()
        .map_err(|error| VoiceSetupError::Connection(error.to_string()))?;
    let singers: Vec<EngineSinger> = agent
        .get(&format!("{root}/singers"))
        .call()
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /singers: {error}")))?
        .into_json()
        .map_err(|error| VoiceSetupError::Connection(error.to_string()))?;
    let mut catalog = VoicevoxCatalog {
        version: version.trim_matches(['"', '\n', '\r']).to_string(),
        query: Vec::new(),
        decode: Vec::new(),
    };
    for singer in singers {
        for style in singer.styles {
            let destination = match style.kind.as_str() {
                "sing" | "singing_teacher" => &mut catalog.query,
                "frame_decode" => &mut catalog.decode,
                _ => continue,
            };
            if singer.name.trim().is_empty() || style.name.trim().is_empty() {
                return Err(VoiceSetupError::Connection(
                    "VOICEVOX advertised an unnamed singing style".into(),
                ));
            }
            destination.push(VoicevoxStyle {
                id: style.id,
                singer: singer.name.clone(),
                name: style.name,
            });
        }
    }
    if catalog.query.is_empty() || catalog.decode.is_empty() {
        return Err(VoiceSetupError::Connection(
            "VOICEVOX did not advertise both melody and singing styles".into(),
        ));
    }
    Ok(catalog)
}

#[derive(Deserialize)]
struct EngineSinger {
    name: String,
    styles: Vec<EngineStyle>,
}

#[derive(Deserialize)]
struct EngineStyle {
    id: u32,
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

/// Starts a user-selected VOICEVOX Engine executable and returns its child handle.
pub fn start_voicevox_engine(executable: &Path) -> Result<Child, VoiceSetupError> {
    if !executable.is_file() {
        return Err(VoiceSetupError::Invalid(format!(
            "VOICEVOX executable was not found: {}",
            executable.display()
        )));
    }
    let mut command = Command::new(executable);
    if let Some(folder) = executable.parent() {
        command.current_dir(folder);
    }
    hide_child_window(&mut command);
    command.spawn().map_err(VoiceSetupError::Io)
}

#[cfg(target_os = "windows")]
fn hide_child_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_child_window(_: &mut Command) {}

/// Validates and writes `dsconfig.yaml` into a DiffSinger voicebank folder.
pub fn write_diffsinger_config(setup: &DiffSingerSetup) -> Result<PathBuf, VoiceSetupError> {
    if !setup.folder.is_dir() {
        return Err(VoiceSetupError::Invalid(
            "Choose an existing DiffSinger voicebank folder".into(),
        ));
    }
    for (label, value) in [
        ("phonemes", setup.phonemes.as_str()),
        ("acoustic", setup.acoustic.as_str()),
        ("vocoder", setup.vocoder.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(VoiceSetupError::Invalid(format!("{label} cannot be empty")));
        }
    }
    for (label, path) in [
        ("phonemes", setup.folder.join(&setup.phonemes)),
        ("acoustic model", setup.folder.join(&setup.acoustic)),
        (
            "vocoder configuration",
            setup.folder.join(&setup.vocoder).join("vocoder.yaml"),
        ),
    ] {
        if !path.is_file() {
            return Err(VoiceSetupError::Invalid(format!(
                "DiffSinger {label} was not found: {}",
                path.display()
            )));
        }
    }
    if setup.sample_rate == 0 || setup.hop_size == 0 || setup.num_mel_bins == 0 {
        return Err(VoiceSetupError::Invalid(
            "DiffSinger audio dimensions must be positive".into(),
        ));
    }
    if !matches!(setup.mel_base.as_str(), "10" | "e") {
        return Err(VoiceSetupError::Invalid(
            "DiffSinger mel base must be 10 or e".into(),
        ));
    }
    let path = setup.folder.join("dsconfig.yaml");
    let text = serde_yaml_ng::to_string(setup)
        .map_err(|error| VoiceSetupError::Encode(error.to_string()))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

fn validate_voicevox(setup: &VoicevoxSetup) -> Result<(), VoiceSetupError> {
    if setup.name.trim().is_empty() || setup.style_name.trim().is_empty() {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX name and style name cannot be empty".into(),
        ));
    }
    validate_voicevox_url(setup.url.trim())?;
    if setup.sample_rate == 0 || !setup.frame_rate.is_finite() || setup.frame_rate <= 0.0 {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX sample rate and frame rate must be positive".into(),
        ));
    }
    Ok(())
}

fn validate_voicevox_url(url: &str) -> Result<(), VoiceSetupError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX URL must begin with http:// or https://".into(),
        ));
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    let safe: String = name
        .trim()
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();
    if safe.is_empty() {
        "VOICEVOX".into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::{Read, Write};

    #[test]
    fn singer_choices_keep_a_valid_teacher_and_resolve_legacy_names_by_ids() {
        let connection = VoicevoxConnection {
            path: PathBuf::from("legacy.voicevox.json"),
            name: "VOICEVOX singer".into(),
            url: "http://127.0.0.1:50021".into(),
            sample_rate: 24_000,
            frame_rate: 93.75,
            speaker: "Singer / normal".into(),
            query_style_id: 6000,
            decode_style_id: 3001,
            track: auris_core::TrackId(1),
            saved_speaker: None,
            raw: Vec::new(),
            styles: vec![VoicevoxSpeakerChoice {
                name: "Singer / normal".into(),
                query_style_id: 6000,
                decode_style_id: 3001,
            }],
        };
        let catalog = VoicevoxCatalog {
            version: "0.25.2".into(),
            query: vec![
                VoicevoxStyle {
                    id: 6000,
                    singer: "波音リツ".into(),
                    name: "ノーマル".into(),
                },
                VoicevoxStyle {
                    id: 6010,
                    singer: "ずんだもん".into(),
                    name: "先生".into(),
                },
            ],
            decode: vec![
                VoicevoxStyle {
                    id: 3001,
                    singer: "ずんだもん".into(),
                    name: "あまあま".into(),
                },
                VoicevoxStyle {
                    id: 3003,
                    singer: "ずんだもん".into(),
                    name: "ノーマル".into(),
                },
            ],
        };
        let choices = catalog.speaker_choices(&connection);
        assert_eq!(choices[0].name, "ずんだもん / あまあま");
        assert_eq!(choices[0].decode_style_id, connection.decode_style_id);
        assert_eq!(choices[1].name, "ずんだもん / ノーマル");
        assert!(choices.iter().all(|choice| choice.query_style_id == 6000));

        let mut outdated = connection.clone();
        outdated.query_style_id = 9999;
        assert!(
            catalog
                .speaker_choices(&outdated)
                .iter()
                .all(|choice| choice.query_style_id == 6010)
        );
        let mut one_teacher = catalog.clone();
        one_teacher.query.truncate(1);
        assert!(
            one_teacher
                .speaker_choices(&outdated)
                .iter()
                .all(|choice| choice.query_style_id == 6000)
        );

        let mut collision = connection;
        collision.styles.push(VoicevoxSpeakerChoice {
            name: "ずんだもん / あまあま".into(),
            query_style_id: 6000,
            decode_style_id: 9999,
        });
        assert_eq!(
            catalog.speaker_choices(&collision)[0].name,
            "ずんだもん / あまあま (3001)"
        );
    }

    fn catalog_server(body: &'static str) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                let path = request.split_whitespace().nth(1).unwrap().to_string();
                let response = if path == "/version" {
                    r#""0.25.0""#
                } else {
                    body
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                paths.push(path);
            }
            paths
        });
        (format!("http://{address}"), server)
    }

    #[test]
    fn the_catalog_preserves_singer_names_and_filters_styles_by_singing_role() {
        let (url, server) = catalog_server(
            r#"[
            {"name":"波音リツ","styles":[{"id":71,"name":"通常","type":"sing"},{"id":81,"name":"通常","type":"frame_decode"},{"id":1,"name":"話す","type":"talk"}]},
            {"name":"先生","styles":[{"id":72,"name":"ガイド","type":"singing_teacher"}]},
            {"name":"別の歌手","styles":[{"id":82,"name":"通常","type":"frame_decode"}]}
        ]"#,
        );
        let catalog = fetch_voicevox_catalog(&format!(" {url}/ ")).unwrap();
        assert_eq!(catalog.version, "0.25.0");
        assert_eq!(
            catalog
                .query
                .iter()
                .map(|style| style.id)
                .collect::<Vec<_>>(),
            [71, 72]
        );
        assert_eq!(
            catalog
                .decode
                .iter()
                .map(|style| style.id)
                .collect::<Vec<_>>(),
            [81, 82]
        );
        assert_eq!(catalog.decode[0].label(), "波音リツ / 通常");
        assert_eq!(catalog.decode[1].label(), "別の歌手 / 通常");
        let mut setup = VoicevoxSetup {
            query_style_id: 72,
            decode_style_id: 82,
            ..VoicevoxSetup::default()
        };
        assert!(catalog.validate_styles(&setup).is_ok());
        setup.decode_style_id = 72;
        assert!(
            catalog.validate_styles(&setup).is_err(),
            "a teacher cannot be selected as a decoder"
        );
        assert_eq!(server.join().unwrap(), ["/version", "/singers"]);
    }

    #[test]
    fn malformed_or_incomplete_singer_catalogs_are_reported() {
        for body in [
            r#"{"styles":[]}"#,
            r#"[{"name":"Singer","styles":[{"id":71,"name":"Normal","type":"sing"}]}]"#,
            r#"[{"name":"Singer","styles":[{"id":71,"name":"","type":"sing"},{"id":81,"name":"Normal","type":"frame_decode"}]}]"#,
        ] {
            let (url, server) = catalog_server(body);
            assert!(
                fetch_voicevox_catalog(&url).is_err(),
                "unusable catalog: {body}"
            );
            assert_eq!(server.join().unwrap(), ["/version", "/singers"]);
        }
    }

    #[test]
    fn connection_names_are_safe_on_every_platform() {
        assert_eq!(safe_name("波音/normal:*"), "波音_normal__");
    }

    #[test]
    fn defaults_describe_the_standard_local_engine() {
        let setup = VoicevoxSetup::default();
        assert_eq!(setup.url, "http://127.0.0.1:50021");
        assert!(validate_voicevox(&setup).is_ok());
    }

    #[test]
    fn a_voicevox_connection_round_trips_through_the_backend_shape() {
        let folder = std::env::temp_dir().join(format!(
            "auris-voicevox-setup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let setup = VoicevoxSetup {
            name: "波音リツ".into(),
            style_name: "通常".into(),
            query_style_id: 6000,
            decode_style_id: 3009,
            ..VoicevoxSetup::default()
        };

        let path = write_voicevox_connection_in(&setup, &folder).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["format_version"], 1);
        assert_eq!(value["name"], "波音リツ");
        assert_eq!(value["styles"][0]["query_style_id"], 6000);
        assert_eq!(value["styles"][0]["decode_style_id"], 3009);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn saving_connection_settings_preserves_added_singers_and_rejects_corrupt_files() {
        let folder = std::env::temp_dir().join(format!(
            "auris-voicevox-preserve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut setup = VoicevoxSetup::default();
        let path = write_voicevox_connection_in(&setup, &folder).unwrap();
        let snapshot = read_voicevox_connection(&path, None, auris_core::TrackId(1)).unwrap();
        let added = VoicevoxSpeakerChoice {
            name: "ずんだもん / ノーマル".into(),
            query_style_id: 6000,
            decode_style_id: 3003,
        };
        append_voicevox_speaker(&snapshot, &added).unwrap();
        setup.url = "http://127.0.0.1:50022".into();
        setup.sample_rate = 48_000;
        setup.frame_rate = 100.0;
        setup.query_style_id = 6010;
        write_voicevox_connection_in(&setup, &folder).unwrap();
        let updated: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(updated["styles"].as_array().unwrap().len(), 2);
        assert_eq!(updated["styles"][0]["name"], "Singer / normal");
        assert_eq!(updated["styles"][0]["query_style_id"], 6010);
        assert_eq!(updated["styles"][0]["decode_style_id"], 3001);
        assert_eq!(updated["styles"][1], serde_json::to_value(&added).unwrap());
        assert_eq!(updated["url"], setup.url);
        assert_eq!(updated["sample_rate"], 48_000);
        assert_eq!(updated["frame_rate"], 100.0);

        setup.style_name = "別の歌手 / 通常".into();
        setup.decode_style_id = 3010;
        write_voicevox_connection_in(&setup, &folder).unwrap();
        let appended: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(appended["styles"].as_array().unwrap().len(), 3);
        assert_eq!(appended["styles"][0], updated["styles"][0]);
        assert_eq!(appended["styles"][1], updated["styles"][1]);
        assert_eq!(appended["styles"][2]["decode_style_id"], 3010);

        for corrupt in [
            "not JSON",
            r#"{"format_version":2,"styles":[]}"#,
            r#"{"format_version":1,"styles":[]}"#,
            r#"{"format_version":1,"styles":[{"name":"Old","query_style_id":6000,"decode_style_id":3001}],"unknown":true}"#,
        ] {
            std::fs::write(&path, corrupt).unwrap();
            assert!(write_voicevox_connection_in(&setup, &folder).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_diffsinger_setup_writes_the_supported_deployment_fields() {
        let folder = std::env::temp_dir().join(format!(
            "auris-diffsinger-setup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(folder.join("dsvocoder")).unwrap();
        std::fs::write(folder.join("phonemes.txt"), "SP\na\n").unwrap();
        std::fs::write(folder.join("acoustic.onnx"), []).unwrap();
        std::fs::write(
            folder.join("dsvocoder/vocoder.yaml"),
            "model: vocoder.onnx\n",
        )
        .unwrap();
        let setup = DiffSingerSetup {
            folder: folder.clone(),
            use_key_shift_embed: true,
            ..DiffSingerSetup::default()
        };

        let path = write_diffsinger_config(&setup).unwrap();
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["acoustic"], "acoustic.onnx");
        assert_eq!(value["vocoder"], "dsvocoder");
        assert_eq!(value["use_key_shift_embed"], true);
        assert!(value.get("folder").is_none());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_connection_check_verifies_the_engines_singing_styles() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let body = if request.starts_with("GET /version ") {
                    r#""0.24.0""#
                } else {
                    r#"[{"name":"Ritsu","styles":[{"id":6000,"name":"Normal","type":"sing"},{"id":3009,"name":"Normal","type":"frame_decode"}]}]"#
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let setup = VoicevoxSetup {
            url: format!("http://{address}"),
            query_style_id: 6000,
            decode_style_id: 3009,
            ..VoicevoxSetup::default()
        };

        assert_eq!(check_voicevox_connection(&setup).unwrap(), "0.24.0");
        server.join().unwrap();
    }
}
