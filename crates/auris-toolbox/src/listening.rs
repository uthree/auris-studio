//! Rendered audio sent to a separate audio-capable critic for iterative editing.
use super::*;

/// A short audition reviewed by an audio model, accessible from either transport.
pub mod listen {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "listen";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Renders a short project excerpt and sends its actual WAV to the configured audio critic. Use start_bar and bars (default first four bars), or section and optional instance. Keep focus to a short question about the sound. compare_to accepts an earlier audio_path from this project's listen/preview. Review a supported edit by listening to the same range again; leave the mix unchanged when no correction is supported. Returns audio delivery status, fallible observations and separate measurements. Configure AURIS_AUDIO_MODEL and AURIS_AUDIO_URL for a music-capable server. Does not edit the project.";
    /// The excerpt and listening question.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute path to the project to audition.
        pub project: String,
        /// First bar, 1-based. Defaults to 1 unless section is supplied.
        pub start_bar: Option<u32>,
        /// Number of bars. Defaults to 4 unless section is supplied. Keep excerpts short.
        pub bars: Option<u32>,
        /// Section label instead of start_bar/bars.
        pub section: Option<String>,
        /// Section occurrence, 1-based; only with section.
        pub instance: Option<usize>,
        /// Optional short question about the audible instrumentation, balance or phrasing.
        pub focus: Option<String>,
        /// Previous audio_path returned by listen/preview in this project, for comparison.
        pub compare_to: Option<String>,
    }

    fn range(args: &Args) -> Result<RenderRange, String> {
        if args.section.is_some() && (args.start_bar.is_some() || args.bars.is_some()) {
            return Err("use section or start_bar/bars, never both".into());
        }
        if args.instance.is_some() && args.section.is_none() {
            return Err("instance requires section".into());
        }
        Ok(RenderRange {
            start_bar: args.section.is_none().then(|| args.start_bar.unwrap_or(1)),
            bars: args.section.is_none().then(|| args.bars.unwrap_or(4)),
            section: args.section.clone(),
            instance: args.instance,
            include_tail: Some(false),
        })
    }

    fn fit_implicit_range(args: &Args, range: &mut RenderRange, project: &Project) {
        if args.section.is_none()
            && args.start_bar.is_none()
            && args.bars.is_none()
            && project.end_tick() <= project.signatures.bar_start(5)
        {
            // A short manually written phrase can end between bar lines. Whole-song preview
            // preserves that exact end instead of asking the range renderer for nonexistent bars.
            range.start_bar = None;
            range.bars = None;
        }
    }

    fn previous_audio(project: &str, path: &str) -> Result<PathBuf, String> {
        let document = resolve_project(project)?;
        let folder = document
            .parent()
            .ok_or("project has no folder")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let previews = folder
            .join(".auris-previews")
            .canonicalize()
            .map_err(|_| "this project has no previous previews")?;
        let previous = Path::new(path);
        if !previous.is_absolute() {
            return Err("compare_to must be the absolute audio_path from listen/preview".into());
        }
        let previous = previous.canonicalize().map_err(|e| e.to_string())?;
        if !previews.starts_with(&folder)
            || !previous.starts_with(&previews)
            || previous
                .extension()
                .and_then(|s| s.to_str())
                .is_none_or(|s| !s.eq_ignore_ascii_case("wav"))
        {
            return Err(
                "compare_to must name a WAV inside this project's .auris-previews folder".into(),
            );
        }
        Ok(previous)
    }

    /// Renders once and asks the audio critic; the controller remains responsible for edits.
    pub fn run(args: &Args) -> Result<String, String> {
        let options = auris_session::audio_review::AudioReviewOptions::from_env()?;
        run_with_options(args, &options)
    }

    fn run_with_options(
        args: &Args,
        options: &auris_session::audio_review::AudioReviewOptions,
    ) -> Result<String, String> {
        let mut range = range(args)?;
        let previous = args
            .compare_to
            .as_deref()
            .map(|path| previous_audio(&args.project, path))
            .transpose()?;
        let focus = args.focus.as_deref().unwrap_or(
            "Describe the instrumentation and balance in this recording. Suggest one small mix adjustment only if the sound supports it, and explain why.",
        );
        if focus.len() > 4000 {
            return Err("focus must be at most 4000 UTF-8 bytes".into());
        }
        if args.section.is_none() && args.start_bar.is_none() && args.bars.is_none() {
            let session = opened(&args.project)?;
            fit_implicit_range(args, &mut range, session.project());
        }
        let preview = preview::create(&preview::Args {
            project: args.project.clone(),
            range,
        })?;
        let mut audio = Vec::new();
        if let Some(path) = &previous {
            audio.push(("Before", path.as_path()));
        }
        audio.push(("Current", preview.path.as_path()));
        // A same-WAV local trial recovered musical descriptions when the long repeated
        // caveats were replaced by a short focus. The session supplies grounding once.
        let review = auris_session::audio_review::review_audio(&audio,focus,options)
            .map_err(|error|format!("Audio review failed: {error}. Preview remains at {}. No project edits were made.",preview.path.display()))?;
        Ok(serde_json::json!({
            "audio_path":preview.path,"compare_to":previous,"model":review.model,
            "audio_sent":review.audio_sent,"review":review.review,"measurements":preview.text,
            "next_step":"Treat the critic's observations as fallible evidence. If it could not hear or judge the music, report that limitation. Speech absence alone is not a music assessment. Make one targeted edit only when supported; otherwise keep the mix unchanged. After an edit, verify the saved state and listen to the same range again; pass this audio_path as compare_to when useful."
        }).to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use base64::Engine;
        use serde_json::{Value, json};
        use std::io::{BufRead, Read, Write};
        use std::time::{Duration, Instant};

        fn scripted_critic() -> (String, std::thread::JoinHandle<Vec<Value>>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/v1", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let mut captured = Vec::new();
                for reply in ["Scripted initial review.", "Scripted comparison review."] {
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let mut socket = loop {
                        match listener.accept() {
                            Ok((socket, _)) => break socket,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "critic request did not arrive");
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("critic accept failed: {error}"),
                        }
                    };
                    // Windows can inherit nonblocking mode from the listener.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut reader = std::io::BufReader::new(&mut socket);
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    assert!(first.starts_with("POST /v1/chat/completions "), "{first}");
                    let mut length = None;
                    let mut header_bytes = first.len();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        header_bytes += line.len();
                        assert!(header_bytes < 16_384, "unexpectedly large request headers");
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(value) =
                            line.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = Some(value.trim().parse::<usize>().unwrap());
                        }
                    }
                    let length = length.expect("request has a Content-Length");
                    assert!(
                        length <= 4 * 1024 * 1024,
                        "test excerpts should remain small"
                    );
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    drop(reader);
                    captured.push(serde_json::from_slice(&body).unwrap());
                    let reply = json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":reply}}]}).to_string();
                    write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
                }
                captured
            });
            (url, worker)
        }

        fn submitted_wavs(request: &Value) -> Vec<Vec<u8>> {
            assert!(request.get("tools").is_none());
            request["messages"][1]["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|part| part["type"] == "input_audio")
                .map(|part| {
                    assert_eq!(part["input_audio"]["format"], "wav");
                    base64::engine::general_purpose::STANDARD
                        .decode(part["input_audio"]["data"].as_str().unwrap())
                        .unwrap()
                })
                .collect()
        }

        #[test]
        fn listening_after_a_saved_edit_submits_distinct_audio_and_the_immutable_before() {
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join("Song.auris");
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let track = session.add_default_instrument_track("Lead").unwrap();
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            let written_notes = session.midi_clip(clip).unwrap().notes.clone();
            session.save(&project).unwrap();
            let original_document = std::fs::read(&project).unwrap();

            let (url, worker) = scripted_critic();
            let options = auris_session::audio_review::AudioReviewOptions {
                url,
                model: "scripted-audio-critic".into(),
                api_key: None,
                ollama_url: None,
            };
            // The omitted range also exercises the default on a phrase shorter than four bars.
            let first_args: Args = serde_json::from_value(json!({"project":project})).unwrap();
            let first: Value =
                serde_json::from_str(&run_with_options(&first_args, &options).unwrap()).unwrap();
            assert_eq!(first["review"], "Scripted initial review.");
            assert_eq!(first["audio_sent"], true);
            assert_eq!(std::fs::read(&project).unwrap(), original_document);
            let first_path = PathBuf::from(first["audio_path"].as_str().unwrap());
            let original_audio = std::fs::read(&first_path).unwrap();

            set_level::run(
                &serde_json::from_value(json!({
                    "project":project,"track":format!("id:{}",track.0),"gain_db":-12.0
                }))
                .unwrap(),
            )
            .unwrap();
            let edited_document = std::fs::read(&project).unwrap();
            let second_args: Args = serde_json::from_value(json!({
                "project":project,"compare_to":first_path,
                "focus":"Compare the level before and after the fader change."
            }))
            .unwrap();
            let second: Value =
                serde_json::from_str(&run_with_options(&second_args, &options).unwrap()).unwrap();
            assert_eq!(second["review"], "Scripted comparison review.");
            assert_eq!(second["audio_sent"], true);
            let second_path = PathBuf::from(second["audio_path"].as_str().unwrap());
            assert_ne!(first_path, second_path);
            assert_eq!(
                PathBuf::from(second["compare_to"].as_str().unwrap()),
                first_path.canonicalize().unwrap()
            );
            assert_eq!(std::fs::read(&project).unwrap(), edited_document);
            assert_eq!(std::fs::read(&first_path).unwrap(), original_audio);

            let captured = worker.join().unwrap();
            assert_eq!(captured.len(), 2);
            let first_upload = submitted_wavs(&captured[0]);
            let comparison = submitted_wavs(&captured[1]);
            assert_eq!(first_upload.len(), 1);
            assert_eq!(comparison.len(), 2);
            assert_eq!(first_upload[0], original_audio);
            assert_eq!(comparison[0], original_audio);
            assert_eq!(comparison[1], std::fs::read(&second_path).unwrap());
            assert_ne!(comparison[0], comparison[1]);
            let labels = captured[1]["messages"][1]["content"]
                .as_array()
                .unwrap()
                .first()
                .unwrap()["text"]
                .as_str()
                .unwrap();
            assert!(labels.contains("1. Before\n2. Current"));

            let before = auris_session::decode_audio(&first_path, 24_000.0).unwrap();
            let after = auris_session::decode_audio(&second_path, 24_000.0).unwrap();
            assert_eq!(before.frame_count(), after.frame_count());
            assert_eq!(before.channel_count(), after.channel_count());
            let before_rms = before.channel_rms(0);
            assert!(before_rms > 0.0001, "the fixture must produce actual sound");
            let ratio = after.channel_rms(0) / before_rms;
            assert!(
                (ratio - 10.0_f32.powf(-12.0 / 20.0)).abs() < 0.01,
                "measured amplitude ratio: {ratio}"
            );
            let reopened = opened(project.to_str().unwrap()).unwrap();
            assert_eq!(reopened.midi_clip(clip).unwrap().notes, written_notes);
            assert_eq!(
                reopened.project().track(track).unwrap().mixer.gain_db,
                -12.0
            );
        }

        #[test]
        fn listening_defaults_to_a_short_range_and_refuses_ambiguous_ranges() {
            let args = |extra: serde_json::Value| {
                let mut value = serde_json::json!({"project":"/song.auris"});
                value
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                serde_json::from_value::<Args>(value).unwrap()
            };
            let default = range(&args(serde_json::json!({}))).unwrap();
            assert_eq!((default.start_bar, default.bars), (Some(1), Some(4)));
            let section = range(&args(serde_json::json!({"section":"verse"}))).unwrap();
            assert_eq!((section.start_bar, section.bars), (None, None));
            assert!(range(&args(serde_json::json!({"section":"verse","bars":4}))).is_err());
            assert!(range(&args(serde_json::json!({"instance":1}))).is_err());
            let short = Project::new("Short phrase", 48_000.0);
            let implicit = args(serde_json::json!({}));
            let mut chosen = range(&implicit).unwrap();
            fit_implicit_range(&implicit, &mut chosen, &short);
            assert_eq!((chosen.start_bar, chosen.bars), (None, None));
            let explicit = args(serde_json::json!({"start_bar":1,"bars":4}));
            let mut chosen = range(&explicit).unwrap();
            fit_implicit_range(&explicit, &mut chosen, &short);
            assert_eq!((chosen.start_bar, chosen.bars), (Some(1), Some(4)));
        }

        #[test]
        fn comparison_accepts_only_saved_previews_of_the_same_project() {
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join("Song.auris");
            std::fs::write(&project, b"path-only fixture").unwrap();
            let folder = root.path().join(".auris-previews");
            std::fs::create_dir(&folder).unwrap();
            let previous = folder.join("before.wav");
            std::fs::write(&previous, b"WAV").unwrap();
            let project = project.to_str().unwrap();
            assert_eq!(
                previous_audio(project, previous.to_str().unwrap()).unwrap(),
                previous.canonicalize().unwrap()
            );
            let outside = root.path().join("other.wav");
            std::fs::write(&outside, b"WAV").unwrap();
            assert!(previous_audio(project, outside.to_str().unwrap()).is_err());
            assert!(previous_audio(project, ".auris-previews/before.wav").is_err());
        }
    }
}
