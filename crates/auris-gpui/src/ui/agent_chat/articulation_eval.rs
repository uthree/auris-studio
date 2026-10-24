//! Opt-in end-to-end evaluation of natural-language articulation choices.
//!
//! This deliberately lives beside the other real-agent evaluations.  It is ignored by default:
//! running it calls the configured Codex or Claude installation, and therefore belongs to an
//! explicit developer run rather than the ordinary test suite.

use super::*;
use auris_session::agent_policy::Policy;
use auris_session::live_agent::Command;
use auris_session::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::AtomicBool;

const BAR: Ticks = Ticks(3_840);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    backend: String,
    model: String,
    output: PathBuf,
    timeout_seconds: u64,
}

/// The prose intentionally describes the musical operation, not the wire property names.  This
/// keeps the evaluation honest: an agent that merely copies `slide` or `glide_ms` from a hint has
/// not demonstrated that it can choose between a fretboard slide and a continuous legato bend.
fn articulation_prompt(clips: &str) -> String {
    format!(
        "次の未保存プロジェクトで、演奏の非破壊アレンジだけを行ってください。対象クリップは {clips} です。\n\n\
ギターの同時発音コードには、弦を低い音から高い音へ順に弾くストロークを設定してください。\n\
ベースの、音符が途切れず隣接していて音程が変わるフレーズには、途中の音を一つ挿入する短い音程移動を設定してください。\n\
別のベースクリップでは、音符の境目をまたいで滑らかに音程が連続して変わる奏法を設定してください。\n\
休符で分断された音符や同じ音程を繰り返す音符には既存の設定があります。設定を変えずに実際に効く条件か検査し、効かなければその理由を報告してください。\n\
まず各クリップの音符、音域、休符、既存の演奏設定を読み取ってから、必要な設定だけを一つの変更にまとめてください。\n\
変更後は各クリップを独立して再読取し、設定値だけでなく、実際に生成された発音列とピッチ曲線が変化したかを報告してください。\n\
元の音符、手描きカーブ、生成レシピは保持してください。今回はローカルの演奏診断と音声計測で確認し、外部サービスへの音声送信や音楽的な良し悪しの批評は行わないでください。保存やファイル操作は不要です。",
    )
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

fn successful_tool(entries: &[Value], name: &str) -> usize {
    entries
        .iter()
        .filter(|entry| entry["name"] == name && entry["ok"] == true)
        .count()
}

fn independent_clip_readbacks(entries: &[Value], clips: &[ClipId]) -> bool {
    clips.iter().all(|clip| {
        let belongs = |entry: &Value| {
            serde_json::from_str::<Value>(entry["args"].as_str().unwrap_or("{}"))
                .ok()
                .is_some_and(|args| args["clip"].as_u64() == Some(clip.0))
        };
        let first_edit = entries.iter().position(|entry| {
            entry["name"] == "set_performance" && entry["ok"] == true && belongs(entry)
        });
        if let Some(first) = first_edit
            && !entries[..first].iter().any(|entry| {
                entry["name"] == "inspect_performance" && entry["ok"] == true && belongs(entry)
            })
        {
            return false;
        }
        let last_edit = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry["name"] == "set_performance" && entry["ok"] == true)
            .filter_map(|(index, entry)| {
                let args: Value = serde_json::from_str(entry["args"].as_str()?).ok()?;
                (args["clip"].as_u64() == Some(clip.0)).then_some(index)
            })
            .next_back();
        let after = last_edit.unwrap_or(0);
        entries.iter().enumerate().any(|(index, entry)| {
            if index <= after || entry["name"] != "inspect_performance" || entry["ok"] != true {
                return false;
            }
            serde_json::from_str::<Value>(entry["args"].as_str().unwrap_or("{}"))
                .ok()
                .and_then(|args| args["clip"].as_u64())
                == Some(clip.0)
        })
    })
}

fn find_stage<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object(fields) => {
            if fields.get("stage").and_then(Value::as_str) == Some(name) {
                return Some(value);
            }
            fields.values().find_map(|child| find_stage(child, name))
        }
        Value::Array(values) => values.iter().find_map(|child| find_stage(child, name)),
        _ => None,
    }
}

fn no_op_inspection(value: &Value, kind: &str) -> bool {
    let Some(stage) = find_stage(value, kind) else {
        return false;
    };
    if kind == "pitch" {
        return stage["pitch"]["measured"] == Value::Bool(true)
            && stage["pitch"]["effective"] == Value::Bool(false)
            && stage["pitch"]["eligibility"]["ineligible_reasons"]["gap_over_60ms"]
                .as_u64()
                .unwrap_or(0)
                > 0;
    }
    stage["slide"]["effective"] == Value::Bool(false)
        && stage["slide"]["ineligible_reasons"]["same_or_semitone"]
            .as_u64()
            .unwrap_or(0)
            > 0
}

fn preinspect_no_op(entries: &[Value], clip: ClipId, kind: &str) -> bool {
    let first_edit = entries.iter().position(|entry| {
        entry["name"] == "set_performance"
            && entry["ok"] == true
            && serde_json::from_str::<Value>(entry["args"].as_str().unwrap_or("{}"))
                .ok()
                .is_some_and(|args| args["clip"].as_u64() == Some(clip.0))
    });
    entries.iter().enumerate().any(|(index, entry)| {
        if first_edit.is_some_and(|edit| index >= edit)
            || entry["name"] != "inspect_performance"
            || entry["ok"] != true
        {
            return false;
        }
        let args = serde_json::from_str::<Value>(entry["args"].as_str().unwrap_or("{}"))
            .ok()
            .and_then(|args| args["clip"].as_u64());
        if args != Some(clip.0) {
            return false;
        }
        serde_json::from_str::<Value>(entry["detail"].as_str().unwrap_or("{}"))
            .ok()
            .is_some_and(|detail| no_op_inspection(&detail, kind))
    })
}

fn render(session: &mut Session, start: Ticks) -> AudioBuffer {
    let options = session
        .render_range_options(start, start + BAR, false)
        .unwrap();
    session
        .render_job()
        .render(&options, &mut RenderProgress::default())
        .unwrap()
}

fn rms_difference(before: &AudioBuffer, after: &AudioBuffer) -> f64 {
    assert_eq!(before.frame_count(), after.frame_count());
    let sum = before
        .channels()
        .iter()
        .flatten()
        .zip(after.channels().iter().flatten())
        .map(|(left, right)| f64::from(left - right).powi(2))
        .sum::<f64>();
    (sum / (before.frame_count() * before.channel_count()) as f64).sqrt()
}

#[test]
fn articulation_readback_must_follow_its_own_edit_and_have_a_prior_inspection() {
    let inspect = |clip, ok| json!({"name":"inspect_performance","ok":ok,"args":json!({"clip":clip}).to_string()});
    let edit = json!({"name":"set_performance","ok":true,"args":"{\"clip\":12}"});
    let mut entries = vec![inspect(12, true), edit.clone(), inspect(13, true)];
    assert!(!independent_clip_readbacks(&entries, &[ClipId(12)]));
    entries.push(inspect(12, false));
    assert!(!independent_clip_readbacks(&entries, &[ClipId(12)]));
    entries.push(inspect(12, true));
    assert!(independent_clip_readbacks(
        &entries,
        &[ClipId(12), ClipId(13)]
    ));
    assert!(!independent_clip_readbacks(
        &[edit, inspect(12, true)],
        &[ClipId(12)]
    ));
}

#[test]
fn articulation_no_op_evidence_rejects_unknown_and_unrelated_effects() {
    let mut value = json!({"note_effects":{"stages":[{"stage":"pitch","pitch":{
        "measured":false,"effective":null,
        "eligibility":{"ineligible_reasons":{"gap_over_60ms":1}},
        "generated_curves":{"modulation":{"effective":false}}
    }}]}});
    assert!(!no_op_inspection(&value, "pitch"));
    value["note_effects"]["stages"][0]["pitch"]["measured"] = json!(true);
    value["note_effects"]["stages"][0]["pitch"]["effective"] = json!(false);
    assert!(no_op_inspection(&value, "pitch"));
}

#[gpui::test]
#[ignore = "real Claude/Codex inference; requires an explicit AURIS_ARTICULATION_EVAL configuration"]
fn installed_cli_selects_slide_or_glide_and_reports_effective_performance(
    cx: &mut gpui::TestAppContext,
) {
    let config_path =
        std::env::var_os("AURIS_ARTICULATION_EVAL").expect("explicit evaluation config");
    let config: Evaluation = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
    assert!(matches!(
        config.backend.as_str(),
        "codex" | "claude" | "opencode"
    ));
    assert!(config.output.is_absolute());
    fs::create_dir(&config.output).expect("a fresh evaluation output directory");

    let (app, cx) = crate::harness::open(cx);
    let (clips, before, before_audio) = app.update(cx, |this, _| {
        this.session.new_project();
        this.session.set_tempo_point(Ticks::ZERO, 120.0);
        let guitar = this
            .session
            .add_instrument_track("Physical Guitar", "auris.physical.guitar")
            .unwrap();
        let bass = this
            .session
            .add_instrument_track("Physical Bass", "auris.physical.bass")
            .unwrap();

        let mut make = |track: TrackId, bar: i64, name: &str, notes: Vec<Note>| {
            let clip = this
                .session
                .generate_clip(
                    track,
                    BAR * (bar - 1),
                    BAR,
                    ClipRecipe::new(
                        if track == guitar {
                            ClipPreset::Chords
                        } else {
                            ClipPreset::Bass
                        },
                        bar as u64,
                    ),
                )
                .unwrap();
            this.session.rename_clip(clip, name).unwrap();
            this.session.replace_notes(clip, notes).unwrap();
            // Generated presets carry their own feel.  Start from a neutral stack so the
            // measured delta belongs to the agent's articulation, not inherited humanisation.
            this.session.set_clip_transforms(clip, Vec::new()).unwrap();
            clip
        };
        let guitar_clip = make(
            guitar,
            1,
            "Guitar A: simultaneous chord",
            vec![
                Note::new(52, Ticks::ZERO, BAR),
                Note::new(59, Ticks::ZERO, BAR),
                Note::new(64, Ticks::ZERO, BAR),
                Note::new(67, Ticks::ZERO, BAR),
            ],
        );
        let discrete = make(
            bass,
            2,
            "Bass A: brief passing pitch",
            vec![
                Note::new(36, Ticks::ZERO, Ticks::QUARTER),
                Note::new(43, Ticks::QUARTER, Ticks::QUARTER),
            ],
        );
        let glide = make(
            bass,
            3,
            "Bass B: connected phrase",
            vec![
                Note::new(40, Ticks::ZERO, Ticks::QUARTER * 2),
                Note::new(47, Ticks::QUARTER * 2, Ticks::QUARTER * 2),
            ],
        );
        let gapped = make(
            bass,
            4,
            "Bass C: phrase with a rest",
            vec![
                Note::new(36, Ticks::ZERO, Ticks::QUARTER),
                Note::new(43, Ticks::QUARTER * 3, Ticks::QUARTER),
            ],
        );
        let repeated = make(
            bass,
            5,
            "Bass D: repeated pitch",
            vec![
                Note::new(36, Ticks::ZERO, Ticks::QUARTER),
                Note::new(36, Ticks::QUARTER, Ticks::QUARTER),
            ],
        );
        this.session
            .set_curve_point(guitar_clip, ClipCurve::Controller(64), Ticks::ZERO, 0.5);
        this.session
            .set_curve_point(discrete, ClipCurve::Bend, Ticks(480), 0.1);
        this.session
            .set_curve_point(discrete, ClipCurve::Controller(1), Ticks::QUARTER, 0.2);
        this.session
            .set_curve_point(glide, ClipCurve::Bend, Ticks(480), -0.1);
        this.session
            .set_clip_transforms(
                gapped,
                vec![NoteTransform::Pitch {
                    settings: PitchPerformance {
                        glide_ms: 120.0,
                        ..Default::default()
                    },
                }],
            )
            .unwrap();
        this.session
            .set_clip_transforms(repeated, vec![NoteTransform::Slide { amount: 0.8 }])
            .unwrap();
        let clips = [guitar_clip, discrete, glide, gapped, repeated];
        let before = this.session.project().clone();
        let before_audio: Vec<_> = clips
            .iter()
            .map(|clip| render(&mut this.session, before.midi_clip(*clip).unwrap().1.start))
            .collect();
        this.settings.agent = AgentPreferences {
            backend: config.backend.clone(),
            model: config.model.clone(),
            policy: Policy::default(),
            ..Default::default()
        };
        this.agent_chat
            .load_preferences(&this.settings.agent.clone());
        this.panels.show(crate::dock::Panel::Agent);
        this.agent_chat.input = TextField::new(articulation_prompt(
            &clips
                .iter()
                .map(|clip| clip.0.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ));
        this.agent_send();
        (clips, before, before_audio)
    });

    crate::harness::paint(&app, cx);
    let started = Instant::now();
    let mut last_snapshot = Instant::now() - Duration::from_secs(3);
    let mut failure = None;
    let mut readback_followup = false;
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
                        .any(|entry| matches!(entry, ChatEntry::Agent(_))),
                this.agent_chat
                    .entries
                    .iter()
                    .find_map(|entry| match entry {
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
        if finished && !readback_followup {
            let missing = app.read_with(cx, |this, _| {
                let entries = transcript(&this.agent_chat.entries);
                successful_tool(&entries, "set_performance") > 0
                    && !independent_clip_readbacks(&entries, &clips)
            });
            if missing {
                readback_followup = true;
                app.update(cx, |this, _| {
                    this.agent_chat.input = TextField::new(format!(
                        "設定変更の結果は保持してください。対象クリップ {} に対して inspect_performance を個別に実行し、変更後の演奏設定と実効性を独立して再読取してください。set_performance の応答や read_notes だけでは演奏設定の再読取になりません。追加の編集、保存、外部送信は不要です。",
                        clips.iter().map(|clip| clip.0.to_string()).collect::<Vec<_>>().join(", ")
                    ));
                    this.agent_send();
                });
                continue;
            }
        }
        if finished || started.elapsed() > Duration::from_secs(config.timeout_seconds) {
            if !finished {
                failure = Some("Real-model evaluation timed out".into());
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let report = app.update(cx, |this, cx| {
        let entries = transcript(&this.agent_chat.entries);
        this.agent_stop(cx);
        let after = this.session.project().clone();
        let mut readbacks = Vec::new();
        for clip in &clips {
            readbacks.push(
                serde_json::from_str::<Value>(
                    &this
                        .session
                        .agent_command(Command::InspectPerformance {
                            clip: clip.0,
                            drum_voice: None,
                        })
                        .unwrap(),
                )
                .unwrap(),
            );
        }
        let after_audio: Vec<_> = clips
            .iter()
            .map(|clip| render(&mut this.session, after.midi_clip(*clip).unwrap().1.start))
            .collect();
        for (index, (before, after)) in before_audio.iter().zip(&after_audio).enumerate() {
            let cancel = AtomicBool::new(false);
            auris_session::audio_audition::write_audition_wav(
                &config.output.join(format!("clip-{index}-before.wav")),
                before,
                &cancel,
            )
            .unwrap();
            auris_session::audio_audition::write_audition_wav(
                &config.output.join(format!("clip-{index}-after.wav")),
                after,
                &cancel,
            )
            .unwrap();
        }
        let audio_differences: Vec<_> = before_audio
            .iter()
            .zip(after_audio.iter())
            .map(|(before, after)| rms_difference(before, after))
            .collect();
        let mut expected = before.clone();
        for clip in &clips {
            expected.midi_clip_mut(*clip).unwrap().transforms =
                after.midi_clip(*clip).unwrap().1.transforms.clone();
        }
        let source_preserved = after == expected;
        let edit_count = successful_tool(&entries, "set_performance");
        let readback = independent_clip_readbacks(&entries, &clips);
        let strum = find_stage(&readbacks[0], "strum")
            .or_else(|| find_stage(&readbacks[0], "stroke"))
            .and_then(|stage| stage["changed_onsets"].as_u64())
            .is_some_and(|count| count > 0);
        let discrete_slide = find_stage(&readbacks[1], "slide")
            .and_then(|stage| stage["added_notes"].as_u64())
            .is_some_and(|count| count > 0)
            && readbacks[1]["settings"]["pitch"]["glide_ms"]
                .as_f64()
                .unwrap_or(0.0)
                == 0.0;
        let glide = find_stage(&readbacks[2], "pitch")
            .and_then(|stage| stage["pitch"]["effective"].as_bool())
            .unwrap_or(false)
            && find_stage(&readbacks[2], "slide").is_none()
            && find_stage(&readbacks[2], "pitch")
                .is_some_and(|stage| stage["note_events_unchanged"] == true);
        let effective = usize::from(strum) + usize::from(discrete_slide) + usize::from(glide);
        let ineffective = usize::from(preinspect_no_op(&entries, clips[3], "pitch"))
            + usize::from(preinspect_no_op(&entries, clips[4], "slide"));
        let dirty = this.session.is_dirty();
        let mut undo_steps = 0;
        while undo_steps < edit_count && this.session.project() != &before {
            this.session.undo();
            undo_steps += 1;
        }
        let undo_restored = this.session.project() == &before;
        for _ in 0..undo_steps {
            this.session.redo();
        }
        let redo_restored = this.session.project() == &after;
        fs::write(
            config.output.join("transcript.json"),
            serde_json::to_vec_pretty(&entries).unwrap(),
        )
        .unwrap();
        json!({
            "backend": config.backend,
            "model": config.model,
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "failure": failure,
            "successful_edit_calls": edit_count,
            "undo_steps": undo_steps,
            "independent_readback": readback,
            "readback_followup": readback_followup,
            "source_preserved": source_preserved,
            "effective_readback_count": effective,
            "ineffective_readback_count": ineffective,
            "audio_rms_difference": audio_differences,
            "dirty": dirty,
            "undo_restored": undo_restored,
            "redo_restored": redo_restored,
            "readbacks": readbacks,
            "entries": entries,
        })
    });
    fs::write(
        config.output.join("result.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();

    assert!(report["failure"].is_null(), "{report}");
    assert!(
        report["successful_edit_calls"].as_u64().unwrap_or(0) >= 1,
        "{report}"
    );
    assert_eq!(report["independent_readback"], true, "{report}");
    assert_eq!(report["source_preserved"], true, "{report}");
    assert!(
        report["effective_readback_count"].as_u64().unwrap_or(0) >= 3,
        "{report}"
    );
    assert!(
        report["ineffective_readback_count"].as_u64().unwrap_or(0) >= 2,
        "{report}"
    );
    assert_eq!(report["dirty"], true, "{report}");
    assert_eq!(report["undo_restored"], true, "{report}");
    assert_eq!(report["redo_restored"], true, "{report}");
    let differences = report["audio_rms_difference"].as_array().unwrap();
    assert!(differences[0].as_f64().unwrap_or(0.0) > 1e-6, "{report}");
    assert!(differences[1].as_f64().unwrap_or(0.0) > 1e-6, "{report}");
    assert!(differences[2].as_f64().unwrap_or(0.0) > 1e-6, "{report}");
    assert!(differences[3].as_f64().unwrap_or(0.0) < 1e-9, "{report}");
    assert!(differences[4].as_f64().unwrap_or(0.0) < 1e-9, "{report}");
}
