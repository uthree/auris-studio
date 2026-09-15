//! Configuration and lifecycle commands for external singing backends.
//!
//! These are session-level commands rather than gpui helpers so every frontend can create the
//! same files, validate the same fields and start the same engine without duplicating policy.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::library::VOICES_FOLDER;
use crate::settings::config_dir;

/// Largest connection or voicebank text file accepted by the setup workflow.
///
/// These files contain a few paths, labels and numeric settings. A MiB leaves generous room for
/// third-party catalogues while ensuring a selected or replaced file cannot be copied without a
/// bound before JSON or YAML validation begins.
const MAX_VOICE_CONFIG_BYTES: usize = 1024 * 1024;

/// Largest `/version` response accepted from a VOICEVOX Engine.
const MAX_VERSION_BYTES: usize = 1024;

/// Largest `/singers` response accepted from a VOICEVOX Engine.
const MAX_SINGER_CATALOG_BYTES: usize = 1024 * 1024;

/// Most singing styles retained from one Engine catalogue or connection file.
const MAX_VOICEVOX_STYLES: usize = 4_096;

/// Largest human-facing name accepted from a configuration or Engine response.
const MAX_VOICE_LABEL_BYTES: usize = 512;

/// Leaves room for `.voicevox.json` within a portable 255-unit path component.
const MAX_VOICE_FILE_NAME_BYTES: usize = 200;

/// Largest configured Engine URL.
const MAX_VOICE_URL_BYTES: usize = 2_048;

/// Audio clocks supported by the rest of the singer pipeline.
const MIN_VOICE_SAMPLE_RATE: u32 = 8_000;
const MAX_VOICE_SAMPLE_RATE: u32 = 192_000;
const MIN_VOICE_FRAME_RATE: f64 = 10.0;
const MAX_VOICE_FRAME_RATE: f64 = 1_000.0;

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

/// Reads one small configuration through a single open handle and enforces the bound again while
/// reading. The second check is authoritative when another process grows the file after metadata
/// was observed.
fn read_voice_config(path: &Path) -> Result<Vec<u8>, VoiceSetupError> {
    read_voice_config_after_metadata(path, || {})
}

fn read_voice_config_after_metadata(
    path: &Path,
    after_metadata: impl FnOnce(),
) -> Result<Vec<u8>, VoiceSetupError> {
    let file = File::open(path)?;
    let observed = file.metadata()?.len();
    if observed > MAX_VOICE_CONFIG_BYTES as u64 {
        return Err(VoiceSetupError::Invalid(format!(
            "Voice configuration is too large: {} is {observed} bytes; the limit is {MAX_VOICE_CONFIG_BYTES} bytes",
            path.display()
        )));
    }
    after_metadata();

    let mut bytes = Vec::new();
    bytes.try_reserve_exact(observed as usize).map_err(|_| {
        VoiceSetupError::Invalid("Not enough memory to read the voice configuration".into())
    })?;
    file.take(MAX_VOICE_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_VOICE_CONFIG_BYTES {
        return Err(VoiceSetupError::Invalid(format!(
            "Voice configuration is too large: {} is at least {} bytes; the limit is {MAX_VOICE_CONFIG_BYTES} bytes",
            path.display(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// Reads a bounded HTTP body even when Content-Length is absent, false, or bypassed by chunking.
fn read_voicevox_response(
    response: ureq::Response,
    endpoint: &str,
    limit: usize,
) -> Result<Vec<u8>, VoiceSetupError> {
    let length = response
        .header("Content-Length")
        .and_then(|length| length.parse::<usize>().ok());
    if length.is_some_and(|length| length > limit) {
        return Err(VoiceSetupError::Connection(format!(
            "VOICEVOX {endpoint} response is too large: {} bytes; the limit is {limit} bytes",
            length.unwrap_or_default()
        )));
    }
    read_voicevox_body_with_capacity(
        response.into_reader(),
        endpoint,
        limit,
        length.unwrap_or_default(),
    )
}

#[cfg(test)]
fn read_voicevox_body(
    reader: impl Read,
    endpoint: &str,
    limit: usize,
) -> Result<Vec<u8>, VoiceSetupError> {
    read_voicevox_body_with_capacity(reader, endpoint, limit, 0)
}

fn read_voicevox_body_with_capacity(
    reader: impl Read,
    endpoint: &str,
    limit: usize,
    capacity: usize,
) -> Result<Vec<u8>, VoiceSetupError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity.min(limit)).map_err(|_| {
        VoiceSetupError::Connection(format!(
            "Not enough memory to read the VOICEVOX {endpoint} response"
        ))
    })?;
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX {endpoint}: {error}")))?;
    if bytes.len() > limit {
        return Err(VoiceSetupError::Connection(format!(
            "VOICEVOX {endpoint} response exceeds the {limit}-byte limit"
        )));
    }
    Ok(bytes)
}

fn label_is_valid(label: &str) -> bool {
    !label.trim().is_empty()
        && label.len() <= MAX_VOICE_LABEL_BYTES
        && !label.chars().any(char::is_control)
}

fn voice_clock_is_valid(sample_rate: u32, frame_rate: f64) -> bool {
    if !(MIN_VOICE_SAMPLE_RATE..=MAX_VOICE_SAMPLE_RATE).contains(&sample_rate)
        || !frame_rate.is_finite()
        || !(MIN_VOICE_FRAME_RATE..=MAX_VOICE_FRAME_RATE).contains(&frame_rate)
    {
        return false;
    }
    let hop = f64::from(sample_rate) / frame_rate;
    hop >= 1.0 && (hop - hop.round()).abs() <= 1.0e-8
}

fn validate_voicevox_styles(styles: &[VoicevoxSpeakerChoice]) -> Result<(), VoiceSetupError> {
    if styles.is_empty() || styles.len() > MAX_VOICEVOX_STYLES {
        return Err(VoiceSetupError::Invalid(format!(
            "VOICEVOX connections need 1..={MAX_VOICEVOX_STYLES} singing styles"
        )));
    }
    let mut names = std::collections::HashSet::with_capacity(styles.len());
    if styles
        .iter()
        .any(|style| !label_is_valid(&style.name) || !names.insert(&style.name))
    {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX style names must be unique, nonempty, printable, and no more than 512 bytes"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn read_voicevox_connection(
    path: &Path,
    speaker: Option<&str>,
    track: auris_core::TrackId,
) -> Result<VoicevoxConnection, VoiceSetupError> {
    let raw = read_voice_config(path)?;
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
    validate_voicevox_styles(&styles)?;
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
    if !voice_clock_is_valid(sample_rate, frame_rate) {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX needs an 8000..=192000 Hz sample rate and a 10..=1000 Hz frame rate that divide into an integer sample hop".into(),
        ));
    }
    if !label_is_valid(&connection.name)
        || connection.url.len() > MAX_VOICE_URL_BYTES
        || connection.url.chars().any(char::is_control)
    {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX names and URLs are empty, too long, or contain control characters".into(),
        ));
    }
    Ok(connection)
}

pub(crate) fn append_voicevox_speaker(
    connection: &VoicevoxConnection,
    choice: &VoicevoxSpeakerChoice,
) -> Result<(), VoiceSetupError> {
    if !label_is_valid(&choice.name) {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX style name must be nonempty, printable, and no more than 512 bytes".into(),
        ));
    }
    if read_voice_config(&connection.path)? != connection.raw {
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
    if connection.styles.len() >= MAX_VOICEVOX_STYLES {
        return Err(VoiceSetupError::Invalid(format!(
            "VOICEVOX connections cannot contain more than {MAX_VOICEVOX_STYLES} styles"
        )));
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
    replace_config_file(
        &connection.path,
        &bytes,
        Some(&connection.raw),
        "The VOICEVOX connection changed; fetch the singers again",
    )
}

fn replace_config_file(
    path: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
    changed: &str,
) -> Result<(), VoiceSetupError> {
    replace_config_file_after_check(path, bytes, expected, changed, || {})
}

fn replace_config_file_after_check(
    path: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
    changed: &str,
    after_check: impl FnOnce(),
) -> Result<(), VoiceSetupError> {
    use std::io::Write;

    if bytes.len() > MAX_VOICE_CONFIG_BYTES {
        return Err(VoiceSetupError::Invalid(format!(
            "Voice configuration is too large to save: {} bytes; the limit is {MAX_VOICE_CONFIG_BYTES} bytes",
            bytes.len()
        )));
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Serialize cooperating setup windows across processes. The expected-byte check stays under
    // this lock, so two writers that started from one catalogue cannot both publish it.
    let lock_path = parent.join(".auris-voice-config.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock()?;
    // A random, exclusively created sibling cannot collide with debris from an earlier process
    // whose PID has since been reused. Keeping it beside the destination also keeps publication
    // on one filesystem, where `persist` can atomically replace an existing connection on both
    // desktop platforms.
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;

    // Check again after the write: the catalogue may have been open for several minutes.
    let unchanged = match expected {
        Some(original) => read_voice_config(path)? == original,
        None => !path.try_exists()?,
    };
    if !unchanged {
        return Err(VoiceSetupError::Invalid(changed.into()));
    }
    after_check();
    let published = match expected {
        Some(_) => staged.persist(path),
        // The destination was absent when this writer started. Refuse a non-cooperating writer
        // that creates it after our check instead of silently replacing its new file.
        None => staged.persist_noclobber(path),
    };
    published.map(drop).map_err(|error| {
        if expected.is_none() && error.error.kind() == std::io::ErrorKind::AlreadyExists {
            VoiceSetupError::Invalid(changed.into())
        } else {
            VoiceSetupError::Io(error.error)
        }
    })?;
    crate::settings::sync_config_parent(parent)?;
    Ok(())
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
    let original = match read_voice_config(&path) {
        Ok(bytes) => Some(bytes),
        Err(VoiceSetupError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut styles: Vec<VoicevoxSpeakerChoice> = match &original {
        Some(bytes) => {
            // A corrupt or newer-format connection must not be silently replaced with defaults.
            // VOICEVOX loading only reads JSON; it neither contacts the Engine nor runs inference.
            auris_singer::VoiceModel::load(&path, auris_singer::Acceleration::Auto)
                .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
            let file: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|error| VoiceSetupError::Invalid(error.to_string()))?;
            let existing_name = file
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("VOICEVOX");
            if existing_name != setup.name.trim() {
                return Err(VoiceSetupError::Invalid(format!(
                    "A different VOICEVOX connection named '{existing_name}' already uses this portable file name"
                )));
            }
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
    validate_voicevox_styles(&styles)?;
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
    replace_config_file(
        &path,
        &bytes,
        original.as_deref(),
        "The VOICEVOX connection changed while it was being saved; retry",
    )?;
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
    let version_response = agent
        .get(&format!("{root}/version"))
        .call()
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /version: {error}")))?;
    let version_bytes = read_voicevox_response(version_response, "/version", MAX_VERSION_BYTES)?;
    let version: String = serde_json::from_slice(&version_bytes)
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /version: {error}")))?;
    if !label_is_valid(&version) {
        return Err(VoiceSetupError::Connection(
            "VOICEVOX advertised an invalid version label".into(),
        ));
    }

    let singers_response = agent
        .get(&format!("{root}/singers"))
        .call()
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /singers: {error}")))?;
    let singers_bytes =
        read_voicevox_response(singers_response, "/singers", MAX_SINGER_CATALOG_BYTES)?;
    let singers: Vec<EngineSinger> = serde_json::from_slice(&singers_bytes)
        .map_err(|error| VoiceSetupError::Connection(format!("VOICEVOX /singers: {error}")))?;
    let mut catalog = VoicevoxCatalog {
        version,
        query: Vec::new(),
        decode: Vec::new(),
    };
    for singer in singers {
        for style in singer.styles {
            let query = match style.kind.as_str() {
                "sing" | "singing_teacher" => true,
                "frame_decode" => false,
                _ => continue,
            };
            if !label_is_valid(&singer.name) || !label_is_valid(&style.name) {
                return Err(VoiceSetupError::Connection(
                    "VOICEVOX advertised an empty, oversized, or unprintable singing style".into(),
                ));
            }
            if catalog.query.len() + catalog.decode.len() >= MAX_VOICEVOX_STYLES {
                return Err(VoiceSetupError::Connection(format!(
                    "VOICEVOX advertised more than {MAX_VOICEVOX_STYLES} singing styles"
                )));
            }
            let destination = if query {
                &mut catalog.query
            } else {
                &mut catalog.decode
            };
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
        if value.trim().is_empty() || value.len() > 4_096 || value.chars().any(char::is_control) {
            return Err(VoiceSetupError::Invalid(format!(
                "{label} must be a printable path no longer than 4096 bytes"
            )));
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
    let hop_seconds = f64::from(setup.hop_size) / f64::from(setup.sample_rate);
    if !(MIN_VOICE_SAMPLE_RATE..=MAX_VOICE_SAMPLE_RATE).contains(&setup.sample_rate)
        || setup.hop_size == 0
        || !(0.001..=0.100).contains(&hop_seconds)
        || setup.num_mel_bins == 0
        || setup.num_mel_bins > 4_096
    {
        return Err(VoiceSetupError::Invalid(
            "DiffSinger needs an 8000..=192000 Hz sample rate, a 1..=100 ms hop, and 1..=4096 mel bins".into(),
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
    let original = match read_voice_config(&path) {
        Ok(bytes) => Some(bytes),
        Err(VoiceSetupError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    replace_config_file(
        &path,
        text.as_bytes(),
        original.as_deref(),
        "The DiffSinger configuration changed while it was being saved; retry",
    )?;
    Ok(path)
}

fn validate_voicevox(setup: &VoicevoxSetup) -> Result<(), VoiceSetupError> {
    if !label_is_valid(&setup.name)
        || setup.name.len() > MAX_VOICE_FILE_NAME_BYTES
        || !label_is_valid(&setup.style_name)
    {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX names must be nonempty and printable; the connection name is limited to 200 bytes and the style name to 512 bytes".into(),
        ));
    }
    validate_voicevox_url(setup.url.trim())?;
    if !voice_clock_is_valid(setup.sample_rate, setup.frame_rate) {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX needs an 8000..=192000 Hz sample rate and a 10..=1000 Hz frame rate that divide into an integer sample hop".into(),
        ));
    }
    Ok(())
}

fn validate_voicevox_url(url: &str) -> Result<(), VoiceSetupError> {
    if url.is_empty() || url.len() > MAX_VOICE_URL_BYTES || url.chars().any(char::is_control) {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX URL must be a printable http:// or https:// URL no longer than 2048 bytes"
                .into(),
        ));
    }
    let parsed = ureq::get(url).request_url().map_err(|_| {
        VoiceSetupError::Invalid(
            "VOICEVOX URL must be a complete http:// or https:// Engine base URL".into(),
        )
    })?;
    let parsed = parsed.as_url();
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(VoiceSetupError::Invalid(
            "VOICEVOX URL must be an http:// or https:// Engine base URL without credentials, a query, or a fragment".into(),
        ));
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if character.is_control() => '_',
            other => other,
        })
        .collect();
    let safe = cleaned
        .trim()
        .trim_end_matches(|character: char| character == '.' || character.is_whitespace());
    if safe.is_empty() {
        return "VOICEVOX".into();
    }
    // Windows treats these as devices even when another extension follows the name. Prefixing
    // rather than replacing keeps the singer recognizable in the managed Voices folder.
    let device = safe
        .split('.')
        .next()
        .unwrap_or(safe)
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(
        device.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || device
        .strip_prefix("COM")
        .or_else(|| device.strip_prefix("LPT"))
        .is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    match reserved {
        true => format!("_{safe}"),
        false => safe.to_string(),
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
    fn voice_configuration_reads_are_bounded_before_and_after_metadata() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("oversized.voicevox.json");
        std::fs::write(&path, vec![b' '; MAX_VOICE_CONFIG_BYTES + 1]).unwrap();
        assert!(matches!(
            read_voice_config(&path),
            Err(VoiceSetupError::Invalid(message)) if message.contains("too large")
        ));

        std::fs::write(&path, vec![b' '; MAX_VOICE_CONFIG_BYTES]).unwrap();
        let result = read_voice_config_after_metadata(&path, || {
            let mut append = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            append.write_all(b"x").unwrap();
            append.flush().unwrap();
        });
        assert!(matches!(
            result,
            Err(VoiceSetupError::Invalid(message)) if message.contains("too large")
        ));
    }

    #[test]
    fn a_new_voice_configuration_never_clobbers_a_late_competing_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("new.voicevox.json");
        let error =
            replace_config_file_after_check(&path, b"ours", None, "configuration changed", || {
                std::fs::write(&path, b"theirs").unwrap()
            })
            .unwrap_err();

        assert!(matches!(error, VoiceSetupError::Invalid(_)));
        assert_eq!(std::fs::read(path).unwrap(), b"theirs");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }

    #[test]
    fn voicevox_streamed_responses_stop_at_the_byte_limit() {
        const LIMIT: usize = 32;
        assert_eq!(
            read_voicevox_body(std::io::Cursor::new(vec![0_u8; LIMIT]), "/fixture", LIMIT)
                .unwrap()
                .len(),
            LIMIT
        );
        assert!(matches!(
            read_voicevox_body(
                std::io::Cursor::new(vec![0_u8; LIMIT + 1]),
                "/fixture",
                LIMIT
            ),
            Err(VoiceSetupError::Connection(message)) if message.contains("32-byte limit")
        ));
    }

    #[test]
    fn voicevox_configuration_limits_match_the_singer_pipeline() {
        let mut setup = VoicevoxSetup::default();
        for (rate, frames) in [
            (7_999, 93.75),
            (192_001, 93.75),
            (24_000, 9.0),
            (24_000, 1_001.0),
            (44_100, 93.75),
        ] {
            setup.sample_rate = rate;
            setup.frame_rate = frames;
            assert!(
                validate_voicevox(&setup).is_err(),
                "{rate} Hz at {frames} fps"
            );
        }
        setup.sample_rate = 192_000;
        setup.frame_rate = 1_000.0;
        assert!(validate_voicevox(&setup).is_ok());

        let mut styles = (0..MAX_VOICEVOX_STYLES)
            .map(|index| VoicevoxSpeakerChoice {
                name: format!("Singer {index}"),
                query_style_id: 1,
                decode_style_id: index as u32,
            })
            .collect::<Vec<_>>();
        assert!(validate_voicevox_styles(&styles).is_ok());
        styles.push(styles[0].clone());
        assert!(validate_voicevox_styles(&styles).is_err());
    }

    #[test]
    fn connection_names_are_safe_on_every_platform() {
        assert_eq!(safe_name("波音/normal:*"), "波音_normal__");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("CONIN$"), "_CONIN$");
        assert_eq!(safe_name("conout$.json"), "_conout$.json");
        assert_eq!(safe_name("lpt9.demo"), "_lpt9.demo");
        assert_eq!(safe_name("COM¹"), "_COM¹");
        assert_eq!(safe_name("CON .demo"), "_CON .demo");
        assert_eq!(safe_name("COM10"), "COM10");
        assert_eq!(safe_name(". ."), "VOICEVOX");
        assert_eq!(safe_name("voice. ."), "voice");
        assert_eq!(safe_name("voice\nname"), "voice_name");
    }

    #[test]
    fn defaults_describe_the_standard_local_engine() {
        let setup = VoicevoxSetup::default();
        assert_eq!(setup.url, "http://127.0.0.1:50021");
        assert!(validate_voicevox(&setup).is_ok());

        for url in [
            "http://",
            "file:///voicevox",
            "http://name@127.0.0.1:50021",
            "http://127.0.0.1:50021?engine=voicevox",
            "http://127.0.0.1:50021#engine",
        ] {
            assert!(validate_voicevox_url(url).is_err(), "{url}");
        }
        let oversized_name = VoicevoxSetup {
            name: "x".repeat(MAX_VOICE_FILE_NAME_BYTES + 1),
            ..VoicevoxSetup::default()
        };
        assert!(validate_voicevox(&oversized_name).is_err());
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
    fn portable_name_collisions_never_replace_another_connection() {
        let folder = tempfile::tempdir().unwrap();
        let first = VoicevoxSetup {
            name: "Singer/Normal".into(),
            ..VoicevoxSetup::default()
        };
        let path = write_voicevox_connection_in(&first, folder.path()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let colliding = VoicevoxSetup {
            name: "Singer:Normal".into(),
            ..VoicevoxSetup::default()
        };

        let error = write_voicevox_connection_in(&colliding, folder.path()).unwrap_err();

        assert!(error.to_string().contains("different VOICEVOX connection"));
        assert_eq!(std::fs::read(path).unwrap(), before);
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
