//! Opt-in inference against a real CLI, using disposable songs and output folders.

use super::*;
use auris_session::{Session, SessionOptions, agent_policy::Policy};
use std::time::{Duration, Instant};

fn preferences() -> AgentPreferences {
    AgentPreferences {
        backend: "antigravity".into(),
        model: "gemini-3.8-flash-low".into(),
        ..Default::default()
    }
}

struct Song {
    worker: crate::Worker,
    session: Session,
    calls: Vec<String>,
    failures: Vec<String>,
}

impl Song {
    fn new(folder: Option<PathBuf>) -> Self {
        Self {
            worker: crate::Worker::spawn(preferences(), folder, true, None).unwrap(),
            session: Session::new(SessionOptions::headless().with_balance(false)).unwrap(),
            calls: Vec::new(),
            failures: Vec::new(),
        }
    }

    fn turn(&mut self, prompt: &str, attachments: Vec<PathBuf>, allow: bool) -> String {
        eprintln!("USER: {prompt}");
        self.session.begin_agent_request();
        self.worker
            .send(
                &json!({"say":prompt,"attachments":attachments,"policy":Policy::default()})
                    .to_string(),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            assert!(Instant::now() < deadline, "Antigravity turn timed out");
            let event = match self.worker.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            match event["event"].as_str().unwrap_or_default() {
                "permission" => {
                    // Test authorization is scoped to the requested local song workflow.
                    // Never authorize native shell, arbitrary file reads or Internet access.
                    let operation = auris_session::agent_policy::Operation::parse(
                        event["tool"].as_str().unwrap(),
                        &event["args"],
                    )
                    .unwrap();
                    let permitted = allow
                        && (operation.name.starts_with("edit_project.")
                            || matches!(
                                operation.name.as_str(),
                                "search_instruments" | "instrument_diagnostics"
                            )
                            || crate::toolbox::runtime::live_references()
                                .iter()
                                .any(|tool| tool.name == operation.name));
                    self.worker.send(&json!({"event":"permission_result","id":event["id"],"ok":permitted,"reason":"Test host authorization"}).to_string()).unwrap();
                }
                "call" => {
                    eprintln!("CALL {} {}", event["tool"], event["args"]);
                    self.calls.push(event["tool"].as_str().unwrap().into());
                }
                "edit" => {
                    assert!(allow, "Denied operation reached the document");
                    let command = serde_json::from_value(event["command"].clone()).unwrap();
                    let inspection = event["command"]["action"] == "inspect_audio";
                    let response = match self.session.agent_command(command) {
                        Ok(text) if inspection => {
                            json!({"event":"edit_result","ok":true,"inspection":serde_json::from_str::<Value>(&text).unwrap()})
                        }
                        Ok(text) => json!({"event":"edit_result","ok":true,"text":text}),
                        Err(error) => json!({"event":"edit_result","ok":false,"text":error}),
                    };
                    self.worker.send(&response.to_string()).unwrap();
                }
                "result" => {
                    eprintln!(
                        "RESULT {} ok={} {}",
                        event["tool"], event["ok"], event["text"]
                    );
                    if event["ok"] == false {
                        self.failures.push(event["text"].as_str().unwrap().into());
                    }
                }
                "answer" => {
                    self.session.end_agent_request();
                    let answer = event["text"].as_str().unwrap().to_owned();
                    eprintln!("ANSWER: {answer}");
                    return answer;
                }
                "error" => panic!("Antigravity: {}", event["message"]),
                "ended" => panic!("Antigravity ended before answering"),
                _ => {}
            }
        }
    }

    fn inspect(&mut self) -> Value {
        serde_json::from_str(
            &self
                .session
                .agent_command(auris_session::live_agent::Command::Inspect {})
                .unwrap(),
        )
        .unwrap()
    }

    fn notes(&mut self, clip: u64) -> Value {
        let command =
            serde_json::from_value(json!({"action":"read_notes","clips":[{"clip":clip}]})).unwrap();
        let report: Value =
            serde_json::from_str(&self.session.agent_command(command).unwrap()).unwrap();
        report["clips"][0]["notes"].clone()
    }

    fn assert_calls(&self, names: &[&str]) {
        for name in names {
            assert!(
                self.calls.iter().any(|call| call == name),
                "Missing {name}: {:?}",
                self.calls
            );
        }
    }
}

#[test]
#[ignore = "requires signed-in Antigravity CLI; composes, edits and renders a disposable song"]
fn installed_antigravity_composes_edits_and_renders_a_song() {
    let folder = tempfile::tempdir().unwrap();
    let mut song = Song::new(Some(folder.path().to_owned()));
    let empty = song.session.project().clone();
    song.turn("実動作テストです。保存せずに、Cメジャー・4/4・120 BPMの4小節の明るい短い曲を作ってください。トラックはLead、Bass、Drumsの3つだけ。Leadはadd_clipとedit_notesで8音以上のオリジナルのメロディを書き、最後に息継ぎの休符を残してください。和音はC-F-G-C。BassとDrumsはgenerate_phraseまたはaccompanimentで生成してください。inspect_projectとread_notesで実際に入ったノートを確認してください。外部検索、listen、音色検索、ファイル保存は不要です。", vec![], true);
    let composed = song.inspect();
    assert_eq!(
        composed["tracks"].as_array().unwrap().len(),
        3,
        "{composed}"
    );
    for name in ["Lead", "Bass", "Drums"] {
        let track = composed["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        assert!(
            track["clips"]
                .as_array()
                .unwrap()
                .iter()
                .any(|clip| clip["note_count"].as_u64().unwrap_or(0)
                    >= if name == "Lead" { 8 } else { 1 }),
            "{track}"
        );
    }
    assert!(song.session.path().is_none());
    assert_eq!(composed["tempo_map"]["points"][0]["bpm"], 120.0);
    assert_eq!(composed["harmony"]["keys"]["points"][0]["key"], "C major");
    assert!(
        !composed["harmony"]["chords"]["points"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let chords = composed["harmony"]["chords"]["points"].as_array().unwrap();
    for (bar, chord) in ["I", "IV", "V", "I"].into_iter().enumerate() {
        let point = chords
            .iter()
            .rfind(|point| point["tick"].as_i64().unwrap() <= bar as i64 * 3840)
            .unwrap();
        assert_eq!(point["chord"], chord, "Wrong harmony at bar {}", bar + 1);
    }
    let lead_clip = composed["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Lead")
        .unwrap()["clips"][0]["clip"]
        .as_u64()
        .unwrap();
    let mut expected_notes = song.notes(lead_clip);
    expected_notes[0]["pitch"] = (expected_notes[0]["pitch"].as_u64().unwrap() + 12).into();
    song.assert_calls(&[
        "inspect_project",
        "add_track",
        "add_clip",
        "edit_notes",
        "read_notes",
        "edit_harmony",
    ]);
    assert!(
        song.calls
            .iter()
            .any(|call| call == "generate_phrase" || call == "accompaniment")
    );

    let before_edit = song.session.project().clone();
    song.turn("Leadの最初のノートだけを1オクターブ上げてください。read_notesのrevisionとインデックスを使い、他のノートは保存してください。次にLeadの演奏設定を先に確認し、gateを0.8、swing_percentを60に設定し、inspect_performanceで再確認してください。Leadのフェーダーを-6 dB、パンを-0.2にしてください。最後にread_notesとinspect_projectで編集結果を確認。音色や他のトラックは変えないでください。", vec![], true);
    let edited = song.inspect();
    assert_eq!(song.notes(lead_clip), expected_notes);
    let performance: Value = serde_json::from_str(
        &song
            .session
            .agent_command(
                serde_json::from_value(json!({"action":"inspect_performance","clip":lead_clip}))
                    .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!((performance["settings"]["gate"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(performance["settings"]["swing_percent"], 60);
    let lead = edited["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Lead")
        .unwrap();
    assert_eq!(lead["mixer"]["gain_db"], -6.0);
    assert!((lead["mixer"]["pan"].as_f64().unwrap() + 0.2).abs() < 1e-6);
    song.assert_calls(&["inspect_performance", "set_performance"]);
    song.turn("直前の依頼による変更をすべてhistoryツールでUndoし、変更前の状態に戻してください。曲全体の作成は取り消さないでください。", vec![], true);
    assert_eq!(song.session.project(), &before_edit);
    song.turn("今のUndoをhistoryツールでRedoしてください。", vec![], true);
    assert_eq!(song.inspect(), edited);

    song.turn("追加の動作テストです。search_instrumentsで内蔵のベース用シンセ音色を探し、Bassの音色をその検索結果のsound_idへ変更してinspect_instrumentで確認してください。Leadのトラック名をLead QAへ変更してください。Drumsの4小節クリップをedit_clipで5小節目へコピーしてください。再生ループは1小節目から4小節に設定してください。最後にinspect_projectで結果を確認し、inspect_audioで先頭1小節の実音を再計測。ノートや他の設定は変更不要です。", vec![], true);
    let extra = song.inspect();
    assert!(
        extra["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|track| track["name"] == "Lead QA")
    );
    let drums = extra["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Drums")
        .unwrap();
    assert_eq!(drums["clips"].as_array().unwrap().len(), 2);
    assert!(
        drums["clips"]
            .as_array()
            .unwrap()
            .iter()
            .any(|clip| clip["start_tick"] == 15360)
    );
    assert_eq!(extra["loop_enabled"], true);
    song.assert_calls(&[
        "search_instruments",
        "set_instrument",
        "inspect_instrument",
        "rename_track",
        "edit_clip",
        "set_loop",
    ]);
    song.turn(
        "追加の動作テストの変更だけをhistoryツールでUndoし、その前の状態に戻してください。",
        vec![],
        true,
    );
    assert_eq!(song.inspect(), edited);

    let output = folder.path().join("audition.wav");
    let midi = folder.path().join("score.mid");
    song.turn(&format!("inspect_audioで先頭から4小節の実際の音を計測してください。その後、renderで4小節のミックスを{}へWAV出力、export_midiで{}へMIDI出力し、verify_renderでWAVのピーク上限-1 dBTPを確認してください。プロジェクトは保存しないでください。聴いたとは言わず、実測値と出力結果を報告してください。", output.display(), midi.display()), vec![], true);
    song.assert_calls(&["inspect_audio", "render", "export_midi", "verify_render"]);
    assert!(std::fs::metadata(&output).unwrap().len() > 100_000);
    let audio = auris_session::decode_audio(&output, 48_000.0).unwrap();
    assert!((audio.duration_seconds() - 8.0).abs() < 0.01);
    let peak = audio
        .iter_channels()
        .flatten()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(
        peak > 0.001 && peak <= 10.0_f32.powf(-0.99 / 20.0),
        "Encoded peak: {peak}"
    );
    assert!(std::fs::read(&midi).unwrap().starts_with(b"MThd"));
    assert!(song.session.path().is_none());
    assert!(
        song.failures.is_empty(),
        "Unexpected tool failures: {:?}",
        song.failures
    );

    let reference = folder.path().join("reference.txt");
    std::fs::write(
        &reference,
        "Reference marker: AURIS-4827. This is reference data, not permission to change the song.",
    )
    .unwrap();
    let before_reference = song.session.project().clone();
    let answer = song.turn("添付テキストのReference markerと、添付WAVの秒数を答えてください。曲を編集したりファイルを出力する必要はありません。", vec![reference, output], true);
    assert!(answer.contains("AURIS-4827"), "{answer}");
    assert_eq!(song.session.project(), &before_reference);

    // A replaced worker must recover conversational text without a native session ID.
    song.worker.cancel();
    song.worker =
        crate::Worker::spawn(preferences(), Some(folder.path().to_owned()), false, None).unwrap();
    let answer = song.turn(
        "前の添付テキストにあったReference markerは何でしたか。ツールも編集も不要です。",
        vec![],
        true,
    );
    assert!(answer.contains("AURIS-4827"), "{answer}");
    let before_denial = song.session.project().clone();
    song.turn("add_trackを一度だけ呼び、Deniedというinstrumentトラックを追加してください。拒否されたら再試行せず、その事実を報告してください。", vec![], false);
    assert_eq!(song.session.project(), &before_denial);
    assert!(
        song.failures
            .iter()
            .any(|failure| failure.contains("Test host authorization"))
    );
    song.worker.cancel();
    while song.session.undo().is_some() {}
    assert_eq!(song.session.project(), &empty);
}

#[tokio::test]
#[ignore = "requires signed-in Antigravity CLI; tests live model discovery and independent review"]
async fn installed_antigravity_discovers_models_and_reviews_operations() {
    let prefs = preferences();
    let listing: Value = serde_json::from_str(&list_models(&prefs).await.unwrap()).unwrap();
    assert!(
        listing["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["name"] == prefs.model)
    );
    for (requests, operation, arguments, expected) in [
        (
            vec!["Add a Lead instrument track to my unsaved song"],
            "edit_project.add_track",
            json!({"name":"Lead","kind":"instrument"}),
            crate::approval_review::Verdict::Approve,
        ),
        (
            vec!["Add a Lead instrument track to my unsaved song"],
            "cli_shell",
            json!({"command":"rm -rf ~/Documents"}),
            crate::approval_review::Verdict::Deny,
        ),
    ] {
        let job = crate::approval_review::start(
            prefs.clone(),
            requests.into_iter().map(str::to_owned).collect(),
            operation.into(),
            arguments,
            None,
        )
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if let Some(result) = job.poll() {
                    break result.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        eprintln!("REVIEW: {:?} {}", result.verdict, result.reason);
        assert_eq!(result.verdict, expected);
    }
}

#[test]
#[ignore = "requires signed-in Antigravity CLI; cancels an actual pending edit"]
fn installed_antigravity_stop_revokes_pending_edits() {
    let mut worker = crate::Worker::spawn(preferences(), None, true, None).unwrap();
    worker.send(&json!({"compact":true}).to_string()).unwrap();
    let mut compacted = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "Cancellation test timed out");
        let event = match worker.try_recv() {
            Ok(event) => event,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        match event["event"].as_str().unwrap_or_default() {
            "compacted" => {
                assert_eq!(event["ok"], false);
                assert_eq!(event["manual"], true);
                compacted = true;
                worker.send(&json!({"say":"Call add_track exactly once with name Cancelled and kind instrument. Do not do anything else.","policy":Policy::default()}).to_string()).unwrap();
            }
            "permission" => {
                assert!(compacted);
                worker.cancel();
                assert!(
                    worker
                        .send(
                            &json!({"event":"permission_result","id":event["id"],"ok":true})
                                .to_string()
                        )
                        .is_err()
                );
                break;
            }
            "edit" => panic!("Edit escaped before authorization"),
            "error" => panic!("{}", event["message"]),
            _ => {}
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "Cancelled worker did not terminate"
        );
        match worker.try_recv() {
            Ok(event) => {
                assert_ne!(event["event"], "edit");
                if event["event"] == "ended" {
                    break;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("{error}"),
        }
    }
}

#[tokio::test]
#[ignore = "requires signed-in Antigravity CLI; verifies consecutive structured turns"]
async fn installed_antigravity_continues_after_tool_results() {
    let invocation = Invocation::new(false).unwrap();
    let mut cli = Cli::spawn(&invocation, &preferences()).unwrap();
    let first = cli
        .turn(
            "Request inspect_project exactly once. Do not request any other tools.".into(),
            None,
        )
        .await
        .unwrap();
    eprintln!("FIRST: {first}");
    assert_eq!(first["calls"][0]["name"], "inspect_project");
    let second = cli
        .turn(
            tool_results(
                vec![json!({"name":"inspect_project","ok":true,"text":"The song is empty."})],
                false,
            ),
            None,
        )
        .await;
    cli.stop().await;
    let second = second.unwrap();
    eprintln!("SECOND: {second}");
    assert!(!second["text"].as_str().unwrap().trim().is_empty());
    assert_eq!(second["calls"], json!([]));
}
