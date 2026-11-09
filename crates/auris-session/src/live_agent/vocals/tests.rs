use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

fn session() -> Session {
    Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap()
}

fn voice_fixture_dir() -> tempfile::TempDir {
    // macOS spells its temp directory through /var, a symlink to /private/var. Resolve that
    // system alias before creating a fixture; automatic voice loading still rejects redirects
    // inside the fixture itself. Leave Windows paths alone so they keep their local spelling.
    let root = std::env::temp_dir();
    let root = if cfg!(target_os = "macos") {
        root.canonicalize().unwrap()
    } else {
        root
    };
    tempfile::Builder::new().tempdir_in(root).unwrap()
}

fn call(s: &mut Session, value: serde_json::Value) -> Result<serde_json::Value, String> {
    let text = s.agent_command(serde_json::from_value(value).unwrap())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn voice(folder: &Path, url: &str) -> std::path::PathBuf {
    let path = folder.join("test.voicevox.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "format_version":1,"name":"Test singer","url":url,
            "styles":[{"name":"First","query_style_id":6000,"decode_style_id":3001},
                      {"name":"Second","query_style_id":6001,"decode_style_id":3002}]
        })
        .to_string(),
    )
    .unwrap();
    path
}

fn run(s: &mut Session, command: Command) -> Result<String, String> {
    let mut job = s.begin_agent_vocal(command, crate::Settings::default())?;
    loop {
        // The real load/render/staging payload must be Send and never borrow the owner.
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()?;
        match s.continue_agent_vocal(result)? {
            VocalStep::Work(next) => job = *next,
            VocalStep::Done(text) => return Ok(text),
        }
    }
}

#[test]
fn written_lyrics_with_explicit_reading_survive_without_a_dictionary() {
    let mut s = session();
    let track = s.add_singer_track("Singer");
    let clip = s
        .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    for beat in [2, 0, 1, 3] {
        s.add_note(clip, Note::new(60, Ticks::QUARTER * beat, Ticks::QUARTER))
            .unwrap();
    }
    let before = s.project().clone();
    let response = call(
        &mut s,
        serde_json::json!({
            "action":"write_lyrics", "clip":clip.0, "text":"月の夜", "reading":"つきのよ"
        }),
    )
    .unwrap();
    assert_eq!(response["clip_name"], "月の夜");
    assert_eq!(
        s.project().lyrics.text_at(Ticks::ZERO),
        Some("月の夜"),
        "the song-wide lyrics follow"
    );
    assert_eq!(s.project().lyrics.text_at(Ticks::QUARTER * 4), None);
    let notes = &s.midi_clip(clip).unwrap().notes;
    assert_eq!(
        notes.iter().map(|n| n.lyric.as_str()).collect::<Vec<_>>(),
        ["+", "月の夜", "+", "+"]
    );
    assert!(notes.iter().all(|n| !n.phonemes.is_empty()));
    let after = s.project().clone();
    for reading in ["", "漢字", "つきのよるに"] {
        assert!(
            call(
                &mut s,
                serde_json::json!({
                    "action":"write_lyrics", "clip":clip.0, "text":"月の夜", "reading":reading
                })
            )
            .is_err()
        );
        assert_eq!(s.project(), &after);
    }
    s.undo();
    assert_eq!(s.project(), &before);
}

#[test]
fn lyrics_follow_time_order_and_preserve_unfilled_notes_in_one_undo_step() {
    let mut s = session();
    let track = s.add_singer_track("Singer");
    let clip = s
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    for beat in [2, 0, 1, 3] {
        s.add_note(clip, Note::new(60, Ticks::QUARTER * beat, Ticks::QUARTER))
            .unwrap();
    }
    s.set_note_lyric(clip, 3, "の").unwrap();
    let before = s.project().clone();
    let response = call(
        &mut s,
        serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"さくら"}),
    )
    .unwrap();
    assert_eq!(response["filled"], 3);
    let notes = &s.project().midi_clip(clip).unwrap().1.notes;
    assert_eq!(
        notes.iter().map(|n| n.lyric.as_str()).collect::<Vec<_>>(),
        ["ら", "さ", "く", "の"]
    );
    assert!(notes[..3].iter().all(|n| !n.phonemes.is_empty()));
    let after = s.project().clone();
    assert!(
        call(
            &mut s,
            serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"あ","from":4})
        )
        .is_err()
    );
    assert_eq!(s.project(), &after);
    s.undo();
    assert_eq!(s.project(), &before);
}

#[test]
fn voice_selection_validates_speaker_and_rejects_stale_or_cancelled_loads() {
    let folder = voice_fixture_dir();
    let path = voice(folder.path(), "http://127.0.0.1:1");
    let mut s = session();
    let track = s.add_singer_track("Singer");
    let select = |speaker: &str| Command::SetSingerVoice {
        track: track.0,
        voice: path.display().to_string(),
        speaker: Some(speaker.into()),
    };
    let before = s.project().clone();
    assert!(
        run(&mut s, select("Unknown"))
            .unwrap_err()
            .contains("First")
    );
    assert_eq!(s.project(), &before);
    let result: serde_json::Value =
        serde_json::from_str(&run(&mut s, select("Second")).unwrap()).unwrap();
    assert_eq!(result["voice"]["speaker"], "Second");
    assert_eq!(
        result["voice"]["speakers"],
        serde_json::json!(["First", "Second"])
    );
    s.undo();
    assert_eq!(s.project(), &before);

    let job = s
        .begin_agent_vocal(select("First"), crate::Settings::default())
        .unwrap();
    let result = job.run(&AtomicBool::new(false)).unwrap();
    s.set_bpm(99.0);
    let revised = s.project().clone();
    assert!(s.continue_agent_vocal(result).is_err());
    assert_eq!(s.project(), &revised);
    let job = s
        .begin_agent_vocal(select("First"), crate::Settings::default())
        .unwrap();
    assert!(job.run(&AtomicBool::new(true)).is_err());
    assert_eq!(s.project(), &revised);
    let path = voice(folder.path(), "https://example.invalid");
    assert!(
        run(
            &mut s,
            Command::SetSingerVoice {
                track: track.0,
                voice: path.display().to_string(),
                speaker: None
            }
        )
        .is_err()
    );
    assert_eq!(s.project(), &revised);
}

#[test]
fn voice_discovery_pages_configured_libraries_without_loading_or_editing() {
    let folder = voice_fixture_dir();
    let mut expected = Vec::new();
    for index in 0..20 {
        let path = folder
            .path()
            .join(format!("Agent Fixture {index:02}.voicevox.json"));
        std::fs::write(&path,serde_json::json!({"format_version":1,"name":format!("Agent Fixture {index:02}"),"url":"https://example.invalid","styles":[{"name":"Test","query_style_id":6000,"decode_style_id":3001}]}).to_string()).unwrap();
        expected.push(path);
    }
    let mut s = session();
    let before = s.project().clone();
    let settings = crate::Settings {
        voice_paths: vec![folder.path().to_path_buf()],
        ..Default::default()
    };
    let mut found = Vec::new();
    let mut offset = 0;
    loop {
        let job = s
            .begin_agent_vocal(Command::ListVoices { offset }, settings.clone())
            .unwrap();
        let VocalStep::Done(text) = s
            .continue_agent_vocal(job.run(&AtomicBool::new(false)).unwrap())
            .unwrap()
        else {
            panic!();
        };
        let page: serde_json::Value = serde_json::from_str(&text).unwrap();
        let voices = page["voices"].as_array().unwrap();
        assert!(voices.len() <= 16);
        for voice in voices {
            assert_eq!(voice["validated"], false);
            found.push(std::path::PathBuf::from(voice["voice"].as_str().unwrap()));
        }
        let Some(next) = page["next_offset"].as_u64() else {
            break;
        };
        assert!(next as usize > offset);
        offset = next as usize;
    }
    assert!(expected.iter().all(|path| found.contains(path)));
    assert_eq!(s.project(), &before);
}

fn request(listener: &TcpListener) -> (TcpStream, String, serde_json::Value) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "singing request did not arrive");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let end = loop {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(at) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..end]);
    let path = header.split_whitespace().nth(1).unwrap().to_string();
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() < end + length {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&bytes[end..end + length]).unwrap();
    (stream, path, body)
}

fn reply(mut stream: TcpStream, kind: &str, body: &[u8]) {
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
    stream.write_all(body).unwrap();
}

fn engine() -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (stream, query_path, score) = request(&listener);
        let count: usize = score["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["frame_length"].as_u64().unwrap() as usize)
            .sum();
        reply(stream,"application/json",serde_json::json!({"f0":vec![261.6256;count],"volume":vec![0.8;count],"phonemes":[{"phoneme":"a","frame_length":count}]}).to_string().as_bytes());
        let (stream, synth_path, query) = request(&listener);
        let frames = query["f0"].as_array().unwrap().len();
        let mut wav = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 24000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..frames * 256 {
            writer
                .write_sample(
                    ((i as f64 * std::f64::consts::TAU * 261.6256 / 24000.0).sin() * 8000.0) as i16,
                )
                .unwrap();
        }
        writer.finalize().unwrap();
        reply(stream, "audio/wav", &wav.into_inner());
        vec![query_path, synth_path]
    });
    (url, worker)
}

#[test]
fn live_vocal_workflow_publishes_real_audio_checks_staleness_and_preserves_undo() {
    let folder = voice_fixture_dir();
    let (url, server) = engine();
    let path = voice(folder.path(), &url);
    let mut s = session();
    let track = s.add_singer_track("Singer");
    let clip = s
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    s.add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    run(
        &mut s,
        Command::SetSingerVoice {
            track: track.0,
            voice: path.display().to_string(),
            speaker: Some("Second".into()),
        },
    )
    .unwrap();
    call(
        &mut s,
        serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"あ"}),
    )
    .unwrap();
    let before = s.project().clone();
    let result: serde_json::Value = serde_json::from_str(
        &run(
            &mut s,
            Command::Sing {
                track: track.0,
                seed: Some(17),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(result["seconds"].as_f64().unwrap() > 0.4);
    assert_eq!(
        s.singer_take_state(track).unwrap(),
        crate::SingerTakeState::Current
    );
    let take = s
        .project()
        .track(track)
        .unwrap()
        .kind
        .as_singer()
        .unwrap()
        .take
        .as_ref()
        .unwrap();
    assert_eq!(take.seed, 17);
    let source = &s.project().audio_sources[&take.source];
    let audio_path = source.path.resolve(s.project_folder()).unwrap();
    let mut wav = hound::WavReader::open(audio_path).unwrap();
    assert!(
        wav.samples::<i32>()
            .map(|x| x.unwrap().abs())
            .max()
            .unwrap()
            > 0
    );
    assert_eq!(
        server.join().unwrap(),
        [
            "/sing_frame_audio_query?speaker=6001",
            "/frame_synthesis?speaker=3002"
        ]
    );
    let after = s.project().clone();
    call(
        &mut s,
        serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"い"}),
    )
    .unwrap();
    assert_eq!(
        s.singer_take_state(track).unwrap(),
        crate::SingerTakeState::Behind
    );
    s.undo();
    assert_eq!(s.project(), &after);
    s.undo();
    assert_eq!(s.project(), &before);
    s.redo();
    assert_eq!(s.project(), &after);

    let (url, server) = engine();
    let path = voice(folder.path(), &url);
    run(
        &mut s,
        Command::SetSingerVoice {
            track: track.0,
            voice: path.display().to_string(),
            speaker: Some("Second".into()),
        },
    )
    .unwrap();
    let load = s
        .begin_agent_vocal(
            Command::Sing {
                track: track.0,
                seed: Some(18),
            },
            crate::Settings::default(),
        )
        .unwrap();
    let VocalStep::Work(render) = s
        .continue_agent_vocal(load.run(&AtomicBool::new(false)).unwrap())
        .unwrap()
    else {
        panic!();
    };
    let result = (*render).run(&AtomicBool::new(false)).unwrap();
    server.join().unwrap();
    let before_tempo = s.project().clone();
    s.set_bpm(99.0);
    let revised = s.project().clone();
    assert!(s.continue_agent_vocal(result).is_err());
    assert_eq!(s.project(), &revised);
    // The fixture Engine has stopped: a failed synthesis must retain the prior take and Undo.
    assert!(
        run(
            &mut s,
            Command::Sing {
                track: track.0,
                seed: Some(19)
            }
        )
        .is_err()
    );
    assert_eq!(s.project(), &revised);
    s.undo();
    assert_eq!(s.project(), &before_tempo);
}

#[test]
fn written_lines_become_one_annotation_line_each_and_undo_with_the_words() {
    let mut s = session();
    let track = s.add_singer_track("Singer");
    let clip = s
        .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 8)
        .unwrap();
    for beat in 0..5 {
        s.add_note(clip, Note::new(60, Ticks::QUARTER * beat, Ticks::QUARTER))
            .unwrap();
    }
    let response = call(
        &mut s,
        serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"さくら\nはる"}),
    )
    .unwrap();
    assert_eq!(response["lyric_annotation"].as_array().unwrap().len(), 2);
    let lyrics = &s.project().lyrics;
    assert_eq!(lyrics.text_at(Ticks::ZERO), Some("さくら"));
    assert_eq!(lyrics.text_at(Ticks::QUARTER * 3), Some("はる"));
    assert_eq!(lyrics.text_at(Ticks::QUARTER * 5), None);
    s.undo().unwrap();
    assert!(s.project().lyrics.is_empty(), "one step with the words");
    assert!(
        s.midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .all(|n| n.lyric.is_empty())
    );
}
