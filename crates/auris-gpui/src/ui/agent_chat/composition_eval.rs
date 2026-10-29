//! Opt-in, multi-turn composition trials through the real application agent hosts.

use super::*;
use auris_session::{
    agent_policy::{Mode, Policy},
    prelude::*,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;

#[test]
#[ignore = "writes a frozen prompt; requires AURIS_COMPOSITION_PROMPT_SNAPSHOT"]
fn snapshot_composition_prompt() {
    let path = std::env::var_os("AURIS_COMPOSITION_PROMPT_SNAPSHOT")
        .expect("explicit prompt snapshot destination");
    fs::write(path, auris_agent::default_system_prompt()).expect("write prompt snapshot");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    #[serde(default = "codex_backend")]
    backend: String,
    #[serde(default = "context_tokens")]
    context_tokens: u32,
    model: String,
    output: PathBuf,
    timeout_seconds: u64,
    fixture: String,
    prompts: Vec<String>,
    #[serde(default)]
    prompt_condition: Option<PromptCondition>,
    #[serde(default)]
    seed_project: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptCondition {
    expected_prefix: String,
    replacement_prefix: String,
    skill_context: String,
}

impl PromptCondition {
    fn apply(&self, current: &str) -> Result<String, &'static str> {
        // Git checkouts may use CRLF; compare the content rather than checkout encoding.
        let current = current.replace("\r\n", "\n");
        let expected = self.expected_prefix.replace("\r\n", "\n");
        if expected.trim().is_empty() || self.replacement_prefix.trim().is_empty() {
            return Err("Prompt condition prefixes must be nonempty");
        }
        let suffix = current
            .strip_prefix(&expected)
            .ok_or("Frozen prompt prefix does not match this evaluation binary")?;
        let mut prompt = self.replacement_prefix.replace("\r\n", "\n");
        prompt.push_str(suffix);
        if !self.skill_context.trim().is_empty() {
            prompt.push_str("\n\n<composition_skill_context>\n");
            prompt.push_str(&self.skill_context.replace("\r\n", "\n"));
            prompt.push_str("\n</composition_skill_context>\n");
        }
        Ok(prompt)
    }
}

#[test]
fn prompt_condition_preserves_operational_suffix_and_freezes_skill_context() {
    let condition = PromptCondition {
        expected_prefix: "Current composition\n".into(),
        replacement_prefix: "Baseline composition\n".into(),
        skill_context: "Frozen skill\r\nReference".into(),
    };
    assert_eq!(
        condition
            .apply("Current composition\r\nTool and safety contract")
            .unwrap(),
        "Baseline composition\nTool and safety contract\n\n<composition_skill_context>\nFrozen skill\nReference\n</composition_skill_context>\n"
    );
}

#[test]
fn prompt_condition_rejects_stale_binary_or_empty_prefix() {
    let mut condition = PromptCondition {
        expected_prefix: "Current composition".into(),
        replacement_prefix: "Baseline composition".into(),
        skill_context: String::new(),
    };
    assert!(condition.apply("Different binary").is_err());
    condition.expected_prefix.clear();
    assert!(condition.apply("Current composition").is_err());
}

fn codex_backend() -> String {
    "codex".into()
}
fn context_tokens() -> u32 {
    65536
}

fn validate_seed_project(path: &Path) -> Result<PathBuf, &'static str> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("auris"))
    {
        return Err("Seed project must be an absolute .auris path");
    }
    let canonical = fs::canonicalize(path).map_err(|_| "Seed project cannot be read")?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| "Seed project cannot be read")?;
    if !metadata.is_file() {
        return Err("Seed project must be a file");
    }
    let stem = canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("Seed project has no valid name")?;
    if canonical
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        != Some(stem)
    {
        return Err("Seed project must be in the project-named folder");
    }
    let audio = canonical.parent().unwrap().join("Audio");
    if audio.exists() {
        let metadata =
            fs::symlink_metadata(&audio).map_err(|_| "Seed project Audio cannot be read")?;
        if !metadata.is_dir() {
            return Err("Seed project Audio must be a directory");
        }
    }
    Ok(canonical)
}

#[test]
fn seed_project_validation_requires_a_real_project_named_folder() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("SeedSong");
    fs::create_dir(&folder).unwrap();
    let path = folder.join("SeedSong.auris");
    fs::write(&path, b"{}").unwrap();
    assert_eq!(
        validate_seed_project(&path).unwrap(),
        fs::canonicalize(&path).unwrap()
    );

    let ambiguous = root.path().join("song.auris");
    fs::write(&ambiguous, b"{}").unwrap();
    assert_eq!(
        validate_seed_project(&ambiguous),
        Err("Seed project must be in the project-named folder")
    );
}

fn melody_fixture(quarter: Ticks) -> (Ticks, Vec<Note>) {
    let notes = (0..8)
        .flat_map(|bar| {
            let pitches = [[69, 72, 76], [67, 71, 74], [65, 69, 72], [64, 68, 71]][bar % 4];
            pitches.into_iter().enumerate().map(move |(i, pitch)| {
                let mut note = Note::new(
                    pitch,
                    Ticks((bar as i64 * 3 + i as i64) * quarter.0),
                    Ticks(quarter.0 * 7 / 8),
                );
                note.velocity = 0.61 + i as f32 * 0.07;
                note
            })
        })
        .collect();
    (Ticks(24 * quarter.0), notes)
}

#[test]
fn versioned_melody_fixture_spans_eight_bars_without_changing_legacy_data() {
    let signature: TimeSignature = "6/8".parse().unwrap();
    let (length, notes) = melody_fixture(Ticks::QUARTER);
    assert_eq!(length, signature.ticks_per_bar() * 8);
    assert_eq!(length.as_beats() * 60.0 / 90.0, 16.0);
    assert_eq!(notes.len(), 24);
    assert_eq!(notes[2].start, Ticks(1920));
    assert_eq!(notes[2].length, Ticks(840));
    assert_eq!(notes[2].pitch, 76);
    let (legacy_length, legacy_notes) = melody_fixture(Ticks(480));
    assert_eq!(legacy_length, Ticks(11520));
    assert_eq!(legacy_notes[2].start, Ticks(960));
    assert_eq!(legacy_notes[2].length, Ticks(420));
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

fn wait_for_agent_conversation(app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext) {
    let started = Instant::now();
    loop {
        app.update(cx, |this, cx| this.drain_agent(cx));
        cx.run_until_parked();
        let ready = app.read_with(cx, |this, _| {
            !this.agent_chat.conversations.unavailable()
                && !this.agent_operation_busy()
                && this.agent_chat.history_load.is_none()
                && this.agent_chat.history_error_project.is_none()
                && (this.session.path().is_none() || this.agent_chat.loaded_history.is_some())
        });
        if ready {
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            panic!("agent conversation preflight did not become ready for saved seed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
fn saved_project_agent_conversation_preflight_loads_without_starting_model(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut seed = Session::new(SessionOptions::headless()).unwrap();
    let path = seed
        .save_as(&directory.path().join("Preflight.auris"))
        .unwrap()
        .document;
    let (app, cx) = crate::harness::open(cx);
    app.update(cx, |this, _| {
        this.session.open(&path).unwrap();
        this.panels.show(crate::dock::Panel::Agent);
    });
    crate::harness::paint(&app, cx);
    wait_for_agent_conversation(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(this.agent_chat.loaded_history.is_some());
        assert!(!this.agent_operation_busy());
        assert!(this.agent_chat.link.is_none());
    });
}

#[gpui::test]
#[ignore = "real model inference; requires AURIS_COMPOSITION_EVAL config"]
fn codex_composes_and_revises_the_open_document(cx: &mut gpui::TestAppContext) {
    let path = std::env::var_os("AURIS_COMPOSITION_EVAL").expect("explicit evaluation config");
    let config: Evaluation = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(config.output.is_absolute());
    assert!(!config.prompts.is_empty());
    assert!(matches!(
        config.backend.as_str(),
        "codex" | "claude" | "rig"
    ));
    let seed_project = config.seed_project.as_deref().map(|path| {
        assert_eq!(
            config.fixture, "blank",
            "seed projects only support the blank fixture"
        );
        validate_seed_project(path).expect("frozen seed project must be valid")
    });
    fs::create_dir(&config.output).expect("a fresh evidence folder");
    let system_prompt = config.prompt_condition.as_ref().map(|condition| {
        condition
            .apply(&auris_agent::default_system_prompt())
            .expect("an explicitly frozen prompt condition matching this binary")
    });
    fs::write(
        config.output.join("system-prompt.txt"),
        system_prompt
            .clone()
            .unwrap_or_else(auris_agent::default_system_prompt),
    )
    .unwrap();
    let (app, cx) = crate::harness::open(cx);
    app.update(cx, |this, _| {
        if let Some(path) = seed_project.as_deref() {
            this.session.open(path).expect("seed project opens");
        } else {
            this.session.new_project();
        }
        match (seed_project.is_some(), config.fixture.as_str()) {
            (true, "blank") => {}
            (false, "blank") => {}
            (false, "melody_six_eight" | "melody_six_eight_eight_bars") => {
                // Keep v1's four-bar seed unchanged for frozen paired comparisons.
                let quarter = if config.fixture == "melody_six_eight" {
                    Ticks(480)
                } else {
                    Ticks::QUARTER
                };
                let (length, notes) = melody_fixture(quarter);
                this.session.set_tempo_at(Ticks::ZERO, 90.0);
                this.session
                    .set_signature_at(Ticks::ZERO, "6/8".parse().unwrap());
                let track = this
                    .session
                    .add_default_instrument_track("手書きのメロディー")
                    .unwrap();
                let clip = this
                    .session
                    .add_midi_clip(track, "残したい旋律", Ticks::ZERO, length)
                    .unwrap();
                this.session.replace_notes(clip, notes).unwrap();
                this.session
                    .set_curve_point(clip, ClipCurve::Controller(1), Ticks(333), 0.37);
            }
            (_, other) => panic!("unknown fixture {other}"),
        }
        this.settings.agent = AgentPreferences {
            backend: config.backend.clone(),
            provider: "ollama".into(),
            context_tokens: Some(config.context_tokens),
            model: config.model.clone(),
            system_prompt: system_prompt.clone().unwrap_or_default(),
            // Fixture-only trials isolate tool selection from interactive approval timing.
            policy: Policy {
                mode: Mode::Bypass,
                ..Default::default()
            },
            ..Default::default()
        };
        this.agent_chat
            .load_preferences(&this.settings.agent.clone());
        this.panels.show(crate::dock::Panel::Agent);
        fs::write(
            config.output.join("project-00.json"),
            serde_json::to_vec_pretty(this.session.project()).unwrap(),
        )
        .unwrap();
    });
    crate::harness::paint(&app, cx);
    // Opening a saved seed causes the painted Agent panel to initialize conversation storage and
    // load the project's history asynchronously.  A first Send while that work is pending is
    // intentionally ignored by the live UI; this evaluation must wait for the same readiness
    // boundary before submitting its first prompt, otherwise no model worker is ever spawned.
    wait_for_agent_conversation(&app, cx);
    let started = Instant::now();
    let mut turns = Vec::new();
    for (index, prompt) in config.prompts.iter().enumerate() {
        let offset = app.update(cx, |this, _| {
            let offset = this.agent_chat.entries.len();
            this.agent_chat.input =
                TextField::new(prompt.replace("{output}", &config.output.to_string_lossy()));
            this.agent_send();
            offset
        });
        let turn_started = Instant::now();
        let mut snapshot = Instant::now() - Duration::from_secs(5);
        let failure = loop {
            app.update(cx, |this, cx| this.drain_agent(cx));
            cx.run_until_parked();
            let (finished, error, pending) = app.read_with(cx, |this, _| {
                let entries = &this.agent_chat.entries[offset..];
                (
                    !this.agent_chat.busy
                        && entries.iter().any(|e| matches!(e, ChatEntry::Agent(_))),
                    entries.iter().find_map(|e| {
                        if let ChatEntry::Error(text) = e {
                            Some(text.clone())
                        } else {
                            None
                        }
                    }),
                    this.agent_chat.has_pending_approval(),
                )
            });
            if snapshot.elapsed() >= Duration::from_secs(5) {
                let entries = app.read_with(cx, |this, _| transcript(&this.agent_chat.entries));
                fs::write(
                    config.output.join("transcript.json"),
                    serde_json::to_vec_pretty(&entries).unwrap(),
                )
                .unwrap();
                eprintln!(
                    "turn {}: {:.1}s, {} entries",
                    index + 1,
                    turn_started.elapsed().as_secs_f64(),
                    entries.len()
                );
                snapshot = Instant::now();
            }
            if error.is_some() {
                break error;
            }
            if pending {
                break Some("Unexpected approval in isolated bypass fixture".into());
            }
            if finished {
                break None;
            }
            if turn_started.elapsed() > Duration::from_secs(config.timeout_seconds) {
                break Some("Real-model turn timed out".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        app.read_with(cx, |this, _| {
            fs::write(
                config.output.join(format!("project-{:02}.json", index + 1)),
                serde_json::to_vec_pretty(this.session.project()).unwrap(),
            )
            .unwrap();
            fs::write(
                config.output.join("transcript.json"),
                serde_json::to_vec_pretty(&transcript(&this.agent_chat.entries)).unwrap(),
            )
            .unwrap();
        });
        let usage = app.read_with(cx, |this, _| json!({
            "last_reported_input_context_tokens":this.agent_chat.tokens_in,
            "conversation_reported_output_tokens":this.agent_chat.tokens_out,
            "interpretation":"Backend-reported counters; input is the last reported context, not cumulative billable input. Zero may mean unavailable."}));
        turns.push(json!({"turn":index+1,"elapsed_seconds":turn_started.elapsed().as_secs_f64(),"failure":failure,"usage":usage}));
        if failure.is_some() {
            break;
        }
    }
    let report = app.update(cx, |this, cx| {
        this.agent_stop(cx);
        let entries = transcript(&this.agent_chat.entries);
        let calls = entries.iter().filter(|e|e["kind"]=="tool").collect::<Vec<_>>();
        let wav = config.output.join("result.wav");
        let midi = config.output.join("result.mid");
        let midi_notes = {
            let mut check = Session::new(auris_session::SessionOptions::headless()).unwrap();
            check.import_midi(&midi).ok().map(|report|report.notes)
        };
        json!({"model":config.model,"backend":config.backend,"prompt_profile":if std::env::var("AURIS_COMPOSITION_WORKFLOWS").is_ok_and(|value| value == "1") {"modular"} else {"legacy"},"fixture":config.fixture,"elapsed_seconds":started.elapsed().as_secs_f64(),
            "policy":"isolated generated fixture, explicit bypass; real application host",
            "turns":turns,"unsaved":this.session.path().is_none(),"midi_notes":midi_notes,
            "encoded":auris_session::inspect_export(&wav).ok(),
            "argument_or_tool_errors":calls.iter().filter(|e|e["ok"]==false).count(),"transcript":entries})
    });
    fs::write(
        config.output.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    assert!(
        report["turns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|turn| turn["failure"].is_null()),
        "see evaluation report: {}",
        config.output.display()
    );
}
