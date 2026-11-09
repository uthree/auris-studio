use super::*;
use crate::{SessionOptions, live_agent::Command};

fn run(session: &mut Session, operation: Operation) -> serde_json::Value {
    serde_json::from_str(
        &session
            .agent_command(Command::AudioAssets { operation })
            .unwrap(),
    )
    .unwrap()
}
fn fixture() -> (Session, tempfile::TempDir, PathBuf, ClipId) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Metal impact.wav");
    let data = AudioBuffer::from_planar(
        vec![
            (0..4800).map(|i| i as f32 / 10000.0).collect(),
            (0..4800).map(|i| -(i as f32) / 20000.0).collect(),
        ],
        48000.0,
    )
    .unwrap();
    auris_io::write_wav(
        &path,
        &data,
        &WavExportSettings {
            bit_depth: WavBitDepth::Float32,
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let response: serde_json::Value = serde_json::from_str(
        &session
            .agent_command(Command::ImportAudio {
                path: path.display().to_string(),
                at_tick: 960,
                name: None,
            })
            .unwrap(),
    )
    .unwrap();
    (
        session,
        directory,
        path,
        ClipId(response["clip"].as_u64().unwrap()),
    )
}
#[test]
fn reverse_crops_each_channel_preserves_original_and_round_trips_undo_save() {
    let (mut session, directory, path, clip) = fixture();
    let original = std::fs::read(&path).unwrap();
    let source = session.project.audio_clip(clip).unwrap().source;
    let before = session.project.clone();
    let expected = session.bank.get(source).unwrap().slice(960, 2400);
    let revision = session.agent_document_revision().unwrap();
    run(
        &mut session,
        Operation::Process {
            clip: clip.0,
            expected_revision: revision,
            reverse: true,
            start_seconds: Some(0.02),
            end_seconds: Some(0.07),
        },
    );
    let changed = session.project.clone();
    let processed = session.project.audio_clip(clip).unwrap();
    assert_eq!(processed.start, Ticks(960));
    assert_ne!(processed.source, source);
    let result = Arc::clone(session.bank.get(processed.source).unwrap());
    for channel in 0..2 {
        let mut reversed = expected.channel(channel).to_vec();
        reversed.reverse();
        assert_eq!(result.channel(channel), reversed);
    }
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(session.bank.get(source).unwrap().frame_count(), 4800);
    assert!(session.undo().is_some());
    assert_eq!(session.project, before);
    assert!(session.redo().is_some());
    assert_eq!(session.project, changed);
    session
        .save_as(&directory.path().join("Sfx.auris"))
        .unwrap();
    let saved = session.path().unwrap().to_owned();
    let mut reopened = Session::new(SessionOptions::headless()).unwrap();
    assert!(reopened.open(&saved).unwrap().is_empty());
    let source = reopened.project.audio_clip(clip).unwrap().source;
    assert_eq!(
        reopened.bank.get(source).unwrap().channel(0),
        result.channel(0)
    );
    assert!(
        reopened.project.audio_sources[&source]
            .provenance
            .contains_key("source_path")
    );
    assert!(
        reopened.project.audio_sources[&source]
            .provenance
            .contains_key("sha256")
    );
}
#[test]
fn stale_cancelled_and_wrong_owner_results_publish_nothing() {
    let (mut session, _directory, _path, clip) = fixture();
    let command = || Operation::Process {
        clip: clip.0,
        expected_revision: session.agent_document_revision().unwrap(),
        reverse: true,
        start_seconds: None,
        end_seconds: None,
    };
    let result = session
        .begin_audio_assets(command())
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    let file = match &result.outcome {
        Outcome::Audio { file, .. } => file.path().to_owned(),
        _ => panic!(),
    };
    session.set_tempo_at(Ticks::ZERO, 123.0);
    let before = session.project.clone();
    assert!(
        session
            .continue_audio_assets(result, &AtomicBool::new(false))
            .is_err()
    );
    assert!(!file.exists());
    assert_eq!(session.project, before);
    let op = Operation::Process {
        clip: clip.0,
        expected_revision: session.agent_document_revision().unwrap(),
        reverse: true,
        start_seconds: None,
        end_seconds: None,
    };
    let result = session
        .begin_audio_assets(op)
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    assert!(
        session
            .continue_audio_assets(result, &AtomicBool::new(true))
            .is_err()
    );
    assert_eq!(session.project, before);
    let op = Operation::Inspect { clip: clip.0 };
    let result = session
        .begin_audio_assets(op)
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    let mut other = Session::new(SessionOptions::headless()).unwrap();
    assert!(
        other
            .continue_audio_assets(result, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        session
            .begin_audio_assets(Operation::Process {
                clip: clip.0,
                expected_revision: "stale".into(),
                reverse: true,
                start_seconds: None,
                end_seconds: None
            })
            .is_err()
    );
}
#[test]
fn filename_search_is_bounded_paged_and_does_not_touch_the_document() {
    let (mut session, directory, _, _) = fixture();
    std::fs::write(
        directory.path().join("Metal second.WAV"),
        b"not decoded by search",
    )
    .unwrap();
    std::fs::write(directory.path().join("Metal secret.txt"), b"not audio").unwrap();
    let before = session.project.clone();
    let search = |offset| Operation::Search {
        root: Some(directory.path().display().to_string()),
        query: "metal".into(),
        recursive: true,
        offset,
        limit: Some(1),
    };
    let first = run(&mut session, search(0));
    let second = run(&mut session, search(1));
    assert_eq!(first["scan_complete"], true);
    assert_eq!(first["matches_in_scan"], 2);
    assert_eq!(first["next_offset"], 1);
    assert_ne!(first["files"][0]["path"], second["files"][0]["path"]);
    assert_eq!(session.project, before);
}
#[test]
fn download_redirect_keeps_provenance_and_rejects_pages_and_credentials() {
    use std::net::TcpListener;
    let (_, directory, path, _) = fixture();
    let bytes = std::fs::read(path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/redirect", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for (i, stream) in listener.incoming().take(2).enumerate() {
            let mut stream = stream.unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            if i == 0 {
                write!(stream,"HTTP/1.1 302 Found\r\nLocation: /sample.wav\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
        }
    });
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let before = session.project.clone();
    let response = run(
        &mut session,
        Operation::ImportUrl {
            url: url.clone(),
            at_tick: 0,
            name: "Impact".into(),
            source_page: Some("https://example.test/sound".into()),
            attribution: Some("Test fixture".into()),
            license: Some("CC0 test fixture".into()),
        },
    );
    worker.join().unwrap();
    let source = &session.project.audio_sources[&SourceId(response["source"].as_u64().unwrap())];
    assert_eq!(source.provenance["source_url"], url);
    assert!(source.provenance["resolved_url"].ends_with("/sample.wav"));
    assert_eq!(source.provenance["sha256"].len(), 64);
    assert!(source.path.resolve(session.project_folder()).is_some());
    session.undo();
    assert_eq!(session.project, before);
    for invalid in [
        "file:///C:/secret.wav",
        "https://user:pass@example.test/a.wav",
        "https://example.test/a.wav#fragment",
    ] {
        assert!(audio_url(invalid).is_err());
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops").unwrap();
    });
    assert!(download(&url, directory.path(), &AtomicBool::new(false)).is_err());
    worker.join().unwrap();
}
#[test]
fn policy_distinguishes_discovery_processing_and_network_import() {
    use crate::agent_policy::{Decision, Mode, Operation as Permission, Policy};
    for (operation, mutating, confirm) in [
        (json!({"kind":"search","query":"wind"}), false, false),
        (
            json!({"kind":"search","root":"/home/someone","query":""}),
            false,
            true,
        ),
        (json!({"kind":"search_online","query":"wind"}), false, true),
        (
            json!({"kind":"process","clip":1,"expected_revision":"x","reverse":true}),
            true,
            false,
        ),
        (
            json!({"kind":"import_url","url":"https://example.test/wind.wav","at_tick":0,"name":"Wind"}),
            true,
            true,
        ),
    ] {
        let permission = Permission::parse(
            "edit_project",
            &json!({"command":{"action":"audio_assets","operation":operation}}),
        )
        .unwrap();
        assert_eq!(permission.mutating, mutating);
        assert_eq!(permission.confirm, confirm);
        let mut policy = Policy::default();
        assert_eq!(
            policy.decide(&permission),
            if confirm {
                Decision::Ask
            } else {
                Decision::Allow
            }
        );
        policy.mode = Mode::ReadOnly;
        if mutating {
            assert!(matches!(policy.decide(&permission), Decision::Deny(_)));
        }
        policy
            .set_rule("edit_project.audio_assets", Some(false))
            .unwrap();
        policy.mode = Mode::Bypass;
        assert!(matches!(policy.decide(&permission), Decision::Deny(_)));
    }
}

#[test]
fn commons_results_keep_credits_order_and_continuation_without_treating_html_as_instructions() {
    let response = json!({"query":{"pages":{
        "2":{"index":2,"title":"File:Second.ogg","imageinfo":[{"mediatype":"AUDIO","url":"https://example.test/b.ogg"}]},
        "3":{"index":3,"title":"File:Image.png","imageinfo":[{"mediatype":"BITMAP","url":"https://example.test/c.png"}]},
        "1":{"index":1,"title":"File:First.wav","imageinfo":[{"mediatype":"AUDIO","url":"https://example.test/a.wav","descriptionurl":"https://example.test/page","size":1234,"mime":"audio/wav","extmetadata":{"Artist":{"value":"<a href=\"x\">Creator</a>"},"LicenseShortName":{"value":"CC0"}}}]}
    }},"continue":{"gsroffset":3}});
    let parsed: serde_json::Value =
        serde_json::from_str(&commons_results(&response, "door").unwrap()).unwrap();
    assert_eq!(parsed["files"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["files"][0]["attribution"], "Creator");
    assert_eq!(parsed["files"][0]["license"], "CC0");
    assert_eq!(parsed["next_offset"], 3);
    assert!(commons_results(&json!({"error":{"info":"rate limit"}}), "door").is_err());
}

#[test]
fn old_audio_source_loads_without_provenance_and_bad_crops_do_not_edit() {
    let (mut session, _directory, _path, clip) = fixture();
    let before = session.project.clone();
    let source = &before.audio_sources[&before.audio_clip(clip).unwrap().source];
    let mut legacy = serde_json::to_value(source).unwrap();
    legacy.as_object_mut().unwrap().remove("provenance");
    let legacy: auris_core::AudioSource = serde_json::from_value(legacy).unwrap();
    assert!(legacy.provenance.is_empty());
    for (start, end) in [(0.05, 0.04), (-1.0, 0.05), (0.0, 0.2), (0.0, f64::NAN)] {
        assert!(
            session
                .begin_audio_assets(Operation::Process {
                    clip: clip.0,
                    expected_revision: session.agent_document_revision().unwrap(),
                    reverse: true,
                    start_seconds: Some(start),
                    end_seconds: Some(end),
                })
                .is_err()
        );
    }
    assert_eq!(session.project, before);
    // A second clip sharing this source must retain its original samples after processing.
    let track = session
        .project
        .tracks
        .iter()
        .find(|t| {
            t.kind
                .as_audio()
                .is_some_and(|a| a.clips.iter().any(|c| c.id == clip))
        })
        .unwrap()
        .id;
    session.agent_command(serde_json::from_value(json!({"action":"edit_clip","clip":clip.0,"operation":{"kind":"copy","track":track.0,"start_tick":3840}})).unwrap()).unwrap();
    let revision = session.agent_document_revision().unwrap();
    run(
        &mut session,
        Operation::Process {
            clip: clip.0,
            expected_revision: revision,
            reverse: true,
            start_seconds: None,
            end_seconds: None,
        },
    );
    let other = session
        .project
        .tracks
        .iter()
        .filter_map(|t| t.kind.as_audio())
        .flat_map(|a| &a.clips)
        .find(|c| c.id != clip)
        .unwrap();
    assert_eq!(other.source, source.id);
    assert_eq!(session.bank.get(other.source).unwrap().frame_count(), 4800);
}

fn mixer(session: &mut Session, operation: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(
        &session
            .agent_command(
                serde_json::from_value(json!({"action":"mixer_graph","operation":operation}))
                    .unwrap(),
            )
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn agent_eq_changes_rendered_audio_without_rewriting_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("two-tones.wav");
    let rate = 48000.0;
    let data = AudioBuffer::from_planar(
        vec![
            (0..96000)
                .map(|i| {
                    let t = i as f32 / rate as f32;
                    0.2 * ((std::f32::consts::TAU * 100.0 * t).sin()
                        + (std::f32::consts::TAU * 4000.0 * t).sin())
                })
                .collect(),
        ],
        rate,
    )
    .unwrap();
    auris_io::write_wav(
        &path,
        &data,
        &WavExportSettings {
            bit_depth: WavBitDepth::Float32,
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let imported: serde_json::Value = serde_json::from_str(
        &session
            .agent_command(Command::ImportAudio {
                path: path.display().to_string(),
                at_tick: 0,
                name: None,
            })
            .unwrap(),
    )
    .unwrap();
    let options = OfflineOptions {
        sample_rate: Some(rate),
        end_frames: Some(96000),
        include_tail: false,
        ..Default::default()
    };
    let before = session
        .render_job()
        .render(&options, &mut RenderProgress::default())
        .unwrap();
    let added = mixer(
        &mut session,
        json!({"action":"add_effect","input":format!("track:{}",imported["track"]),"effect":"auris.fx.eq"}),
    );
    let node = &added["result"]["node"];
    mixer(
        &mut session,
        json!({"action":"set_parameter","node":node,"param":"hp_enabled","value":1.0}),
    );
    mixer(
        &mut session,
        json!({"action":"set_parameter","node":node,"param":"hp_freq","value":1000.0}),
    );
    let after = session
        .render_job()
        .render(&options, &mut RenderProgress::default())
        .unwrap();
    let magnitude = |buffer: &AudioBuffer, hz: f64| {
        let samples = &buffer.channel(0)[48000..96000];
        let (re, im) = samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &x)| {
                let phase = std::f64::consts::TAU * hz * i as f64 / rate;
                (
                    re + f64::from(x) * phase.cos(),
                    im + f64::from(x) * phase.sin(),
                )
            });
        re.hypot(im)
    };
    assert!(magnitude(&after, 100.0) < magnitude(&before, 100.0) * 0.03);
    assert!(magnitude(&after, 4000.0) > magnitude(&before, 4000.0) * 0.9);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
#[ignore = "real Commons network request; set AURIS_AUDIO_ASSETS_EVAL to a fresh evidence folder"]
fn online_sfx_smoke() {
    let output = PathBuf::from(std::env::var_os("AURIS_AUDIO_ASSETS_EVAL").unwrap());
    assert!(output.is_absolute());
    std::fs::create_dir(&output).unwrap();
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let found = run(
        &mut session,
        Operation::SearchOnline {
            query: "door creak".into(),
            limit: Some(5),
            offset: 0,
        },
    );
    std::fs::write(
        output.join("search.json"),
        serde_json::to_vec_pretty(&found).unwrap(),
    )
    .unwrap();
    let candidate = found["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| {
            file["bytes"].as_u64().is_some_and(|b| b < MAX_BYTES)
                && file["license"].as_str().is_some_and(|s| !s.is_empty())
        })
        .expect("audio candidate with credit metadata");
    let imported = run(
        &mut session,
        Operation::ImportUrl {
            url: candidate["url"].as_str().unwrap().into(),
            at_tick: 0,
            name: "Commons door".into(),
            source_page: candidate["source_page"].as_str().map(str::to_owned),
            attribution: candidate["attribution"].as_str().map(str::to_owned),
            license: candidate["license"].as_str().map(str::to_owned),
        },
    );
    let clip = imported["clip"].as_u64().unwrap();
    let inspection = run(&mut session, Operation::Inspect { clip });
    std::fs::write(
        output.join("inspection.json"),
        serde_json::to_vec_pretty(&inspection).unwrap(),
    )
    .unwrap();
    let revision = session.agent_document_revision().unwrap();
    run(
        &mut session,
        Operation::Process {
            clip,
            expected_revision: revision,
            reverse: true,
            start_seconds: None,
            end_seconds: Some(inspection["source_seconds"].as_f64().unwrap().min(3.0)),
        },
    );
    session.save_as(&output.join("CommonsSfx.auris")).unwrap();
    let audio = session
        .render_job()
        .render(
            &OfflineOptions {
                include_tail: false,
                ..Default::default()
            },
            &mut RenderProgress::default(),
        )
        .unwrap();
    assert!(audio.peak() > 0.001);
    auris_io::write_wav(
        &output.join("reversed.wav"),
        &audio,
        &WavExportSettings {
            bit_depth: WavBitDepth::Float32,
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn bad_downloads_leave_no_asset_or_document_changes() {
    use std::net::TcpListener;
    for response in [
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",MAX_BYTES+1),
        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops".into(),
        "HTTP/1.1 302 Found\r\nLocation: file:///secret.wav\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
    ] {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let url=format!("http://{}/bad.wav",listener.local_addr().unwrap());
        let worker=std::thread::spawn(move || {
            let (mut stream,_)=listener.accept().unwrap();
            let mut request=[0;4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream.write_all(response.as_bytes()).unwrap();
        });
        let mut session=Session::new(SessionOptions::headless()).unwrap();
        let before=session.project.clone();
        let result=session.agent_command(Command::AudioAssets {operation:Operation::ImportUrl {
            url,at_tick:0,name:"Bad".into(),source_page:None,attribution:None,license:None,
        }});
        worker.join().unwrap();
        assert!(result.is_err());
        assert_eq!(session.project,before);
        let folder=session.project_folder().unwrap().join(auris_io::AUDIO_DIR);
        if folder.exists() { assert_eq!(std::fs::read_dir(folder).unwrap().count(),0); }
    }
}

#[test]
fn processed_wav_preserves_nondefault_sample_rate_after_reopening() {
    let (_, directory, path, _) = fixture();
    let mut session = Session::new(SessionOptions::headless().with_sample_rate(44100.0)).unwrap();
    session.project.sample_rate = 44100.0;
    let imported: serde_json::Value = serde_json::from_str(
        &session
            .agent_command(Command::ImportAudio {
                path: path.display().to_string(),
                at_tick: 0,
                name: None,
            })
            .unwrap(),
    )
    .unwrap();
    let clip = imported["clip"].as_u64().unwrap();
    let revision = session.agent_document_revision().unwrap();
    run(
        &mut session,
        Operation::Process {
            clip,
            expected_revision: revision,
            reverse: true,
            start_seconds: None,
            end_seconds: None,
        },
    );
    let source =
        &session.project.audio_sources[&session.project.audio_clip(ClipId(clip)).unwrap().source];
    let decoded =
        auris_io::decode_audio_file(&source.path.resolve(session.project_folder()).unwrap())
            .unwrap();
    assert_eq!(decoded.source_sample_rate, 44100.0);
    assert_eq!(decoded.buffer.frame_count(), 4410);
    let expected = session.bank.get(source.id).unwrap().channel(0).to_vec();
    assert_eq!(decoded.buffer.channel(0), expected);
    session
        .save_as(&directory.path().join("Rate.auris"))
        .unwrap();
    let mut reopened = Session::new(SessionOptions::headless()).unwrap();
    assert!(reopened.open(session.path().unwrap()).unwrap().is_empty());
    let source = reopened.project.audio_clip(ClipId(clip)).unwrap().source;
    assert_eq!(reopened.bank.get(source).unwrap().channel(0), expected);
}

#[test]
fn a_redirect_leaves_the_approved_origin_only_for_a_public_https_host() {
    let approved = url::Url::parse("https://commons.example.test/a.wav").unwrap();
    let to = |value: &str| url::Url::parse(value).unwrap();
    assert!(redirect_allowed(
        &approved,
        &to("https://commons.example.test/b.wav")
    ));
    assert!(redirect_allowed(
        &approved,
        &to("https://upload.example.test/b.wav")
    ));
    for refused in [
        "http://upload.example.test/b.wav",
        "https://localhost/b.wav",
        "https://media.localhost./b.wav",
        "https://127.0.0.1/b.wav",
        "https://192.168.1.1/admin.wav",
        "https://10.0.0.5/b.wav",
        "https://169.254.169.254/latest/meta-data",
        "https://100.100.100.100/b.wav",
        "https://[::1]/b.wav",
        "https://[fd00::1]/b.wav",
        "https://[fe80::1]/b.wav",
        "https://[::ffff:127.0.0.1]/b.wav",
    ] {
        assert!(!redirect_allowed(&approved, &to(refused)), "{refused}");
    }
    // A server on this machine that the person chose themselves may redirect within itself.
    let local = to("http://127.0.0.1:8000/a.wav");
    assert!(redirect_allowed(&local, &to("http://127.0.0.1:8000/b.wav")));
    assert!(!redirect_allowed(
        &local,
        &to("http://127.0.0.1:9000/b.wav")
    ));
}
