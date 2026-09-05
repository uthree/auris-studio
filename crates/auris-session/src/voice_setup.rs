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

#[derive(Serialize)]
struct VoicevoxFile<'a> {
    format_version: u32,
    name: &'a str,
    url: &'a str,
    sample_rate: u32,
    frame_rate: f64,
    styles: [VoicevoxStyleFile<'a>; 1],
}

#[derive(Serialize)]
struct VoicevoxStyleFile<'a> {
    name: &'a str,
    query_style_id: u32,
    decode_style_id: u32,
}

/// Writes a VOICEVOX connection into the application's managed Voices folder.
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
    let file = VoicevoxFile {
        format_version: 1,
        name: setup.name.trim(),
        url: setup.url.trim().trim_end_matches('/'),
        sample_rate: setup.sample_rate,
        frame_rate: setup.frame_rate,
        styles: [VoicevoxStyleFile {
            name: setup.style_name.trim(),
            query_style_id: setup.query_style_id,
            decode_style_id: setup.decode_style_id,
        }],
    };
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| VoiceSetupError::Encode(error.to_string()))?;
    std::fs::write(&path, bytes)?;
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
