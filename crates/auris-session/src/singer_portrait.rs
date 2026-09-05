//! Optional singer artwork, loaded independently of a voice's inference model.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use auris_core::TrackId;
use auris_singer::{BackendKind, PORTRAIT_MAX_BYTES, VoicePortrait, read_voice_portrait};
use serde::Deserialize;

use crate::VoiceSetupError;
use crate::voice_setup::read_voicevox_connection;

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

/// A track's saved voice identity, suitable for caching an asynchronous artwork request.
///
/// This contains no model, file contents or image data. Compare it with the current source
/// before displaying a worker's result so a previous speaker's portrait cannot replace it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingerPortraitSource {
    pub(crate) track: TrackId,
    pub(crate) path: PathBuf,
    pub(crate) backend: BackendKind,
    pub(crate) speaker: Option<String>,
}

impl SingerPortraitSource {
    /// The voice entry resolved against the project folder.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The singing backend whose artwork may be available.
    pub fn backend(&self) -> BackendKind {
        self.backend
    }

    /// The saved speaker selection, or `None` for the voice's first speaker.
    pub fn speaker(&self) -> Option<&str> {
        self.speaker.as_deref()
    }
}

/// An optional artwork request failed; this never changes synthesis or the saved voice.
#[derive(Debug, thiserror::Error)]
pub enum SingerPortraitError {
    /// The native voice metadata could not be read.
    #[error("{0}")]
    Voice(#[from] auris_singer::SingError),
    /// The VOICEVOX connection file could not be read.
    #[error("{0}")]
    Connection(#[from] VoiceSetupError),
    /// The Engine could not provide the artwork.
    #[error("{0}")]
    Request(String),
    /// The response could not describe supported artwork.
    #[error("{0}")]
    Invalid(String),
}

/// Loads a voice's optional portrait without loading a model or accessing a session.
///
/// Native voices provide `voice.portrait` in their ONNX metadata. VOICEVOX resolves the saved
/// decoding style through `/singers`, then requests `/singer_info` in URL format to avoid
/// downloading icons and voice samples. A style portrait takes precedence over the character
/// portrait. HTTP requests have finite timeouts, bounded bodies and no redirects; resource
/// URLs must belong to the configured Engine's origin. Run this blocking work off the UI thread.
pub fn load_singer_portrait(
    source: &SingerPortraitSource,
) -> Result<Option<VoicePortrait>, SingerPortraitError> {
    match source.backend {
        BackendKind::Auris => Ok(read_voice_portrait(&source.path)?),
        BackendKind::DiffSinger => Ok(None),
        BackendKind::Voicevox => {
            let connection =
                read_voicevox_connection(&source.path, source.speaker.as_deref(), source.track)?;
            fetch_voicevox_portrait(&connection.url, connection.decode_style_id)
        }
    }
}

#[derive(Deserialize)]
struct EngineSinger {
    speaker_uuid: String,
    styles: Vec<EngineStyle>,
}

#[derive(Deserialize)]
struct EngineStyle {
    id: u32,
}

#[derive(Deserialize)]
struct EngineSingerInfo {
    #[serde(default)]
    portrait: Option<String>,
    #[serde(default)]
    style_infos: Vec<EngineStyleInfo>,
}

#[derive(Deserialize)]
struct EngineStyleInfo {
    id: u32,
    #[serde(default)]
    portrait: Option<String>,
}

fn fetch_voicevox_portrait(
    root: &str,
    decode_style: u32,
) -> Result<Option<VoicePortrait>, SingerPortraitError> {
    let root = root.trim().trim_end_matches('/');
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .redirects(0)
        .build();
    let singers: Vec<EngineSinger> = read_json(agent.get(&format!("{root}/singers")))?;
    let Some(singer) = singers
        .iter()
        .find(|singer| singer.styles.iter().any(|style| style.id == decode_style))
    else {
        return Ok(None);
    };
    let info: EngineSingerInfo = read_json(
        agent
            .get(&format!("{root}/singer_info"))
            .query("speaker_uuid", &singer.speaker_uuid)
            .query("resource_format", "url"),
    )?;
    let selected = info
        .style_infos
        .iter()
        .find(|style| style.id == decode_style)
        .and_then(|style| style.portrait.as_deref());
    let mut last_error = None;
    for resource in selected.into_iter().chain(info.portrait.as_deref()) {
        if resource.is_empty() {
            continue;
        }
        match read_engine_image(&agent, root, resource) {
            Ok(Some(portrait)) => return Ok(Some(portrait)),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn read_engine_image(
    agent: &ureq::Agent,
    root: &str,
    resource: &str,
) -> Result<Option<VoicePortrait>, SingerPortraitError> {
    // Older Engines may ignore resource_format and keep returning an embedded PNG.
    if resource.starts_with("iVBOR") {
        return Ok(VoicePortrait::from_base64("image/png", resource).filter(is_png));
    }
    let engine = agent
        .get(&format!("{root}/"))
        .request_url()
        .map_err(|error| SingerPortraitError::Invalid(error.to_string()))?;
    let image = engine
        .as_url()
        .join(resource)
        .map_err(|error| SingerPortraitError::Invalid(error.to_string()))?;
    if image.origin() != engine.as_url().origin()
        || !matches!(image.scheme(), "http" | "https")
        || !image.username().is_empty()
        || image.password().is_some()
    {
        return Err(SingerPortraitError::Invalid(
            "VOICEVOX portrait URL must belong to the configured Engine".into(),
        ));
    }
    let bytes = read_response(agent.get(image.as_str()), PORTRAIT_MAX_BYTES)?;
    Ok(VoicePortrait::from_bytes("image/png", bytes).filter(is_png))
}

fn is_png(portrait: &VoicePortrait) -> bool {
    portrait.bytes().starts_with(b"\x89PNG\r\n\x1a\n")
}

fn read_json<T: serde::de::DeserializeOwned>(
    request: ureq::Request,
) -> Result<T, SingerPortraitError> {
    let bytes = read_response(request, MAX_JSON_BYTES)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| SingerPortraitError::Invalid(format!("VOICEVOX artwork: {error}")))
}

fn read_response(request: ureq::Request, limit: usize) -> Result<Vec<u8>, SingerPortraitError> {
    let response = request
        .call()
        .map_err(|error| SingerPortraitError::Request(format!("VOICEVOX artwork: {error}")))?;
    // ureq returns redirects as responses when following them is disabled.
    if !(200..300).contains(&response.status()) {
        return Err(SingerPortraitError::Request(format!(
            "VOICEVOX artwork returned status {}",
            response.status()
        )));
    }
    if response
        .header("Content-Length")
        .and_then(|length| length.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(SingerPortraitError::Invalid(
            "VOICEVOX artwork is too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| SingerPortraitError::Request(format!("VOICEVOX artwork: {error}")))?;
    if bytes.len() > limit {
        return Err(SingerPortraitError::Invalid(
            "VOICEVOX artwork is too large".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    const SINGERS: &str = r#"[
        {"speaker_uuid":"teacher","styles":[{"id":6000}]},
        {"speaker_uuid":"performer","styles":[{"id":3001},{"id":3003}]}
    ]"#;
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nselected style image";

    fn serve(responses: Vec<(String, Vec<u8>)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let mut paths = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            for (headers, body) in responses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "artwork request did not arrive");
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("artwork test server: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut chunk = [0; 1024];
                    let read = stream.read(&mut chunk).unwrap();
                    assert!(read > 0);
                    request.extend_from_slice(&chunk[..read]);
                }
                paths.push(
                    String::from_utf8(request)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_string(),
                );
                write!(stream, "HTTP/1.1 {headers}\r\nConnection: close\r\n\r\n").unwrap();
                let _ = stream.write_all(&body);
            }
            paths
        });
        (root, worker)
    }

    fn ok(body: impl AsRef<[u8]>) -> (String, Vec<u8>) {
        let body = body.as_ref().to_vec();
        (format!("200 OK\r\nContent-Length: {}", body.len()), body)
    }

    #[test]
    fn portrait_follows_the_decoding_style_and_downloads_only_its_image() {
        let (root, server) = serve(vec![
            ok(SINGERS),
            ok(r#"{"portrait":"/_resources/main","style_infos":[
                {"id":3001,"portrait":"/_resources/other"},
                {"id":3003,"portrait":"/_resources/selected","voice_samples":["unused"]}
            ]}"#),
            ok(PNG),
        ]);
        let portrait = fetch_voicevox_portrait(&root, 3003).unwrap().unwrap();
        assert_eq!(portrait.mime(), "image/png");
        assert_eq!(portrait.bytes().as_ref(), PNG);
        assert_eq!(
            server.join().unwrap(),
            [
                "GET /singers HTTP/1.1",
                "GET /singer_info?speaker_uuid=performer&resource_format=url HTTP/1.1",
                "GET /_resources/selected HTTP/1.1",
            ]
        );
    }

    #[test]
    fn absent_or_invalid_style_artwork_uses_the_character_portrait() {
        for selected in ["null", "\"/_resources/broken\""] {
            let mut responses = vec![
                ok(SINGERS),
                ok(format!(
                    r#"{{"portrait":"_resources/main","style_infos":[{{"id":3003,"portrait":{selected}}}]}}"#
                )),
            ];
            if selected != "null" {
                responses.push(ok(b"not an image"));
            }
            responses.push(ok(PNG));
            let (root, server) = serve(responses);
            assert_eq!(
                fetch_voicevox_portrait(&root, 3003)
                    .unwrap()
                    .unwrap()
                    .bytes()
                    .as_ref(),
                PNG
            );
            assert_eq!(
                server.join().unwrap().last().unwrap(),
                "GET /_resources/main HTTP/1.1"
            );
        }
    }

    #[test]
    fn a_missing_style_or_portrait_does_not_request_unrelated_artwork() {
        let (root, server) = serve(vec![ok(SINGERS)]);
        assert!(fetch_voicevox_portrait(&root, 3999).unwrap().is_none());
        assert_eq!(server.join().unwrap().len(), 1);

        let (root, server) = serve(vec![
            ok(SINGERS),
            ok(r#"{"portrait":null,"style_infos":[]}"#),
        ]);
        assert!(fetch_voicevox_portrait(&root, 3003).unwrap().is_none());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn artwork_urls_cannot_leave_the_configured_engine() {
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        for resource in [
            "http://example.com/portrait.png",
            "//example.com/portrait.png",
            "http://127.0.0.1:50022/portrait.png",
            "file:///portrait.png",
            "http://name:password@127.0.0.1:50021/portrait.png",
        ] {
            assert!(matches!(
                read_engine_image(&agent, "http://127.0.0.1:50021", resource),
                Err(SingerPortraitError::Invalid(_))
            ));
        }
    }

    #[test]
    fn redirects_and_bodies_over_the_limit_are_refused() {
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let (root, server) = serve(vec![(
            "302 Found\r\nLocation: http://example.com/portrait.png\r\nContent-Length: 0".into(),
            vec![],
        )]);
        assert!(matches!(
            read_response(agent.get(&root), 8),
            Err(SingerPortraitError::Request(_))
        ));
        server.join().unwrap();

        for headers in ["200 OK\r\nContent-Length: 9", "200 OK"] {
            let (root, server) = serve(vec![(headers.into(), vec![0; 9])]);
            assert!(matches!(
                read_response(agent.get(&root), 8),
                Err(SingerPortraitError::Invalid(_))
            ));
            server.join().unwrap();
        }
    }

    struct VoiceFile(PathBuf);

    impl VoiceFile {
        fn new(root: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "auris-portrait-{}-{}.voicevox.json",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(
                &path,
                serde_json::json!({
                    "format_version": 1, "name": "Test voice", "url": root,
                    "styles": [
                        {"name":"First", "query_style_id":6000, "decode_style_id":3001},
                        {"name":"Selected", "query_style_id":6000, "decode_style_id":3003}
                    ]
                })
                .to_string(),
            )
            .unwrap();
            Self(path)
        }
    }

    impl Drop for VoiceFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn saved_source_never_reads_the_connection_or_waits_for_inference() {
        let file = VoiceFile::new("http://127.0.0.1:1");
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Singer");
        assert!(session.singer_portrait_source(track).unwrap().is_none());
        session.set_singer_voice(track, Some(&file.0)).unwrap();
        let first = session.singer_portrait_source(track).unwrap().unwrap();
        session.set_singer_speaker(track, Some("Selected")).unwrap();
        let model = session.singer_voice_model(track).unwrap();
        let _inference = model.lock().unwrap();
        std::fs::remove_file(&file.0).unwrap();
        let selected = session.singer_portrait_source(track).unwrap().unwrap();
        assert_ne!(first, selected);
        assert_eq!(selected.speaker(), Some("Selected"));
        assert_eq!(selected.path(), file.0);
        assert_eq!(selected.backend(), BackendKind::Voicevox);
    }

    #[test]
    fn a_detached_worker_reads_the_saved_style_without_loading_a_model() {
        let (root, server) = serve(vec![
            ok(SINGERS),
            ok(r#"{"portrait":"/_resources/main","style_infos":[]}"#),
            ok(PNG),
        ]);
        let file = VoiceFile::new(&root);
        let source = SingerPortraitSource {
            track: TrackId(17),
            path: file.0.clone(),
            backend: BackendKind::Voicevox,
            speaker: Some("Selected".into()),
        };
        assert_eq!(
            load_singer_portrait(&source)
                .unwrap()
                .unwrap()
                .bytes()
                .as_ref(),
            PNG
        );
        assert!(server.join().unwrap()[1].contains("speaker_uuid=performer"));
    }
}
