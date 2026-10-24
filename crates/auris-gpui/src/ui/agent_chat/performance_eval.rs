//! Explicitly opted-in real-model performance edits through the application's agent panel.

use super::*;
use auris_session::agent_policy::Policy;
use auris_session::live_agent::Command;
use auris_session::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    backend: String,
    model: String,
    output: PathBuf,
    timeout_seconds: u64,
}

fn transcript(entries: &[ChatEntry]) -> Vec<Value> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            ChatEntry::You { text, attachments } => {
                Some(json!({"kind":"user","text":text,"attachments":attachments}))
            }
            ChatEntry::Agent(text) => Some(json!({"kind":"assistant","text":text})),
            ChatEntry::Error(text) => Some(json!({"kind":"error","text":text})),
            ChatEntry::Tool {
                name,
                args,
                ok,
                detail,
                ..
            } => Some(json!({"kind":"tool","name":name,"args":args,"ok":ok,"detail":detail})),
            _ => None,
        })
        .collect()
}

fn performance_readback(entries: &[Value]) -> (bool, bool) {
    let Some(index) = entries
        .iter()
        .position(|e| e["name"] == "set_performance" && e["ok"] == true)
    else {
        return (false, false);
    };
    let inspected = |entries: &[Value]| {
        entries
            .iter()
            .any(|e| e["name"] == "inspect_performance" && e["ok"] == true)
    };
    (
        inspected(&entries[..index]),
        inspected(&entries[index + 1..]),
    )
}

#[test]
fn performance_readback_requires_successful_inspection_after_the_edit() {
    let inspect = json!({"name":"inspect_performance","ok":true});
    let edit = json!({"name":"set_performance","ok":true});
    for (entries, expected) in [
        (vec![], (false, false)),
        (vec![inspect.clone()], (false, false)),
        (vec![inspect.clone(), edit.clone()], (true, false)),
        (
            vec![
                inspect.clone(),
                edit.clone(),
                json!({"name":"inspect_performance","ok":false}),
            ],
            (true, false),
        ),
        (
            vec![inspect.clone(), edit.clone(), inspect.clone()],
            (true, true),
        ),
        (vec![edit, inspect], (false, true)),
    ] {
        assert_eq!(performance_readback(&entries), expected);
    }
}

fn render(session: &mut Session) -> AudioBuffer {
    let options = session
        .render_range_options(Ticks::ZERO, Ticks::QUARTER * 4, false)
        .unwrap();
    session
        .render_job()
        .render(&options, &mut RenderProgress::default())
        .unwrap()
}

#[gpui::test]
#[ignore = "real Claude/Codex inference; requires an explicit AURIS_PERFORMANCE_EVAL configuration"]
fn installed_cli_edits_performance_in_the_window(cx: &mut gpui::TestAppContext) {
    let config = std::env::var_os("AURIS_PERFORMANCE_EVAL").expect("explicit evaluation config");
    let config: Evaluation = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    assert!(matches!(
        config.backend.as_str(),
        "codex" | "claude" | "opencode"
    ));
    assert!(config.output.is_absolute());
    fs::create_dir(&config.output).expect("a fresh evaluation output directory");
    let (app, cx) = crate::harness::open(cx);
    let (clip, before, before_audio) = app.update(cx, |this, _| {
        this.session.new_project();
        this.session.set_tempo_point(Ticks::ZERO, 120.0);
        this.session.agent_command(serde_json::from_value(json!({
            "action":"edit_harmony","start_bar":1,"bars":1,
            "key":"C major","chords":"| I |"
        })).unwrap()).unwrap();
        let track = this.session.add_default_instrument_track("Performance trial").unwrap();
        let clip = this.session.generate_clip(
            track, Ticks::ZERO, Ticks::QUARTER * 4, ClipRecipe::new(ClipPreset::Lead, 42)
        ).unwrap();
        this.session.replace_notes(clip, vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(64, Ticks(240), Ticks::QUARTER),
            Note::new(67, Ticks::QUARTER, Ticks::QUARTER),
        ]).unwrap();
        this.session.set_curve_point(clip, ClipCurve::Bend, Ticks(200), 0.1);
        this.session.set_clip_transforms(clip, vec![
            NoteTransform::Expression { settings: Expression {
                timing: 0.0, velocity: 0.1, swell: 0.2, group: 99, seed: 777,
                ..Default::default()
            } },
            NoteTransform::Strum { settings: Strum {
                spread_ms: 23.0, direction: StrokeDirection::HighToLow,
                ..Default::default()
            } },
            NoteTransform::Pitch { settings: PitchPerformance {
                vibrato: 0.05,
                volume_contour: VolumeContour::new(vec![CurvePoint {at:Ticks(2300), value:0.22}]),
                ..Default::default()
            } },
        ]).unwrap();
        let before = this.session.project().clone();
        let audio = render(&mut this.session);
        this.settings.agent = AgentPreferences {
            backend: config.backend.clone(), model: config.model.clone(),
            policy: Policy::default(), ..Default::default()
        };
        this.agent_chat.load_preferences(&this.settings.agent.clone());
        this.panels.show(crate::dock::Panel::Agent);
        this.agent_chat.input = TextField::new(format!(
            "この未保存プロジェクトのクリップ ID {} の「演奏」を調整してください。まず現在値を読み取り、スイングを16分音符の62%、ゲートを50%、移調を+12半音、Expressionのベロシティの揺れを0.3、ストラムの幅を30 ms、ビブラートの深さを0.2半音にしてください。変更はset_performanceの1回の呼び出しにまとめてください。他の設定、音符、生成レシピ、手描きカーブは保持してください。変更後にinspect_performanceで実際の値を読み戻して報告してください。保存・ファイル操作・別の編集・音声解析は不要です。",
            clip.0
        ));
        this.agent_send();
        (clip, before, audio)
    });
    crate::harness::paint(&app, cx);
    let started = Instant::now();
    let mut last_snapshot = Instant::now() - Duration::from_secs(3);
    let mut failure = None;
    let mut requested_readback = false;
    loop {
        app.update(cx, |this, cx| this.drain_agent(cx));
        cx.run_until_parked();
        let (finished, error, pending) = app.read_with(cx, |this, _| {
            (
                !this.agent_chat.busy
                    && this
                        .agent_chat
                        .entries
                        .iter()
                        .any(|e| matches!(e, ChatEntry::Agent(_))),
                this.agent_chat.entries.iter().find_map(|e| match e {
                    ChatEntry::Error(text) => Some(text.clone()),
                    _ => None,
                }),
                this.agent_chat.has_pending_approval(),
            )
        });
        if last_snapshot.elapsed() >= Duration::from_secs(3) {
            let entries = app.read_with(cx, |this, _| transcript(&this.agent_chat.entries));
            fs::write(
                config.output.join("transcript.json"),
                serde_json::to_vec_pretty(&entries).unwrap(),
            )
            .unwrap();
            eprintln!(
                "{} {:.1}s, {} transcript entries",
                config.backend,
                started.elapsed().as_secs_f64(),
                entries.len()
            );
            last_snapshot = Instant::now();
        }
        if let Some(error) = error {
            failure = Some(error);
            break;
        }
        if pending {
            failure = Some("The application requested approval; no approval was bypassed".into());
            break;
        }
        if finished {
            let needs_readback = app.read_with(cx, |this, _| {
                let entries = transcript(&this.agent_chat.entries);
                !performance_readback(&entries).1
                    && entries
                        .iter()
                        .any(|e| e["name"] == "set_performance" && e["ok"] == true)
            });
            if needs_readback && !requested_readback {
                requested_readback = true;
                app.update(cx, |this, _| {
                    this.agent_chat.input = TextField::new(format!(
                        "先ほどの変更を保持したまま、クリップ ID {} に対して inspect_performance を実行して、6項目の現在値を読み戻してください。set_performance の応答だけでなく、独立した再読取を確認します。追加の編集や保存は不要です。",
                        clip.0
                    ));
                    this.agent_send();
                });
                continue;
            }
            break;
        }
        if started.elapsed() > Duration::from_secs(config.timeout_seconds) {
            failure = Some("Real-model evaluation timed out".into());
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = app.update(cx, |this, cx| {
        let entries = transcript(&this.agent_chat.entries);
        this.agent_stop(cx);
        let after = this.session.project().clone();
        let settings: Value = serde_json::from_str(&this.session.agent_command(
            Command::InspectPerformance { clip:clip.0, drum_voice:None }
        ).unwrap()).unwrap();
        let mut expected = before.clone();
        expected.midi_clip_mut(clip).unwrap().transforms = after.midi_clip(clip).unwrap().1.transforms.clone();
        let source_preserved = after == expected;
        let target = after.midi_clip(clip).unwrap().1;
        let performed: Vec<_> = target.sounding_notes_with_meter(120.0, after.signatures.clone()).collect();
        let after_audio = render(&mut this.session);
        assert_eq!(before_audio.frame_count(), after_audio.frame_count());
        let samples = before_audio.channels().iter().flatten().zip(after_audio.channels().iter().flatten());
        let mut squared_difference = 0.0;
        for (a,b) in samples { squared_difference += f64::from(a-b).powi(2); }
        let rms_difference = (squared_difference / (after_audio.frame_count()*after_audio.channel_count()) as f64).sqrt();
        let (inspected_before, inspected_after) = performance_readback(&entries);
        let edit_count = entries.iter().filter(|e|e["name"]=="set_performance" && e["ok"]==true).count();
        let shape_before: Vec<_> = before.midi_clip(clip).unwrap().1.transforms.iter().filter_map(|s|match s {NoteTransform::Pitch{settings}=>Some(&settings.volume_contour),_=>None}).collect();
        let shape_after: Vec<_> = target.transforms.iter().filter_map(|s|match s {NoteTransform::Pitch{settings}=>Some(&settings.volume_contour),_=>None}).collect();
        let contour_preserved = shape_before == shape_after;
        let dirty = this.session.is_dirty();
        if edit_count > 0 { this.session.undo(); }
        let undo_restored = this.session.project() == &before;
        if edit_count > 0 { this.session.redo(); }
        let redo_restored = this.session.project() == &after;
        for (name,project) in [("project-before.json", &before),("project-after.json", &after)] {
            fs::write(config.output.join(name), serde_json::to_vec_pretty(project).unwrap()).unwrap();
        }
        json!({"backend":config.backend,"model":config.model,"elapsed_seconds":started.elapsed().as_secs_f64(),
            "failure":failure,"policy":"edit; actual application permission pipeline",
            "settings":settings,"inspected_before":inspected_before,"inspected_after":inspected_after,"readback_followup":requested_readback,
            "successful_edit_calls":edit_count,"source_preserved":source_preserved,"contour_preserved":contour_preserved,
            "dirty":dirty,"unsaved":this.session.path().is_none(),"undo_restored":undo_restored,"redo_restored":redo_restored,
            "performed_notes":performed,"audio":{"frames":after_audio.frame_count(),"before_peak":before_audio.peak(),
            "after_peak":after_audio.peak(),"rms_difference":rms_difference},"entries":entries})
    });
    fs::write(
        config.output.join("result.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(report["successful_edit_calls"], 1, "{report}");
    for flag in [
        "inspected_before",
        "inspected_after",
        "source_preserved",
        "contour_preserved",
        "dirty",
        "unsaved",
        "undo_restored",
        "redo_restored",
    ] {
        assert_eq!(report[flag], true, "{flag}: {report}");
    }
    let settings = &report["settings"]["settings"];
    assert_eq!(settings["swing_percent"], 62);
    assert_eq!(settings["swing_grid"], "sixteenth");
    assert_eq!(settings["gate"], 0.5);
    assert_eq!(settings["transpose"], 12);
    assert!((settings["expression"]["velocity"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(settings["expression"]["timing"], 0.0);
    assert_eq!(settings["expression"]["group"], 99);
    assert_eq!(settings["expression"]["seed"], 777);
    assert!((settings["expression"]["swell"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    assert_eq!(settings["strum"]["spread_ms"], 30.0);
    assert_eq!(settings["strum"]["direction"], "high_to_low");
    assert!((settings["pitch"]["vibrato"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    let performed = report["performed_notes"].as_array().unwrap();
    assert_eq!(performed.len(), 3);
    assert_eq!(performed[0]["pitch"], 72);
    assert_eq!(performed[0]["length"], 480);
    assert_eq!(performed[1]["start"], 298);
    assert!(report["audio"]["rms_difference"].as_f64().unwrap() > 1e-6);
    assert!(report["audio"]["before_peak"].as_f64().unwrap() > 1e-6);
    assert!(report["audio"]["after_peak"].as_f64().unwrap() > 1e-6);
}
