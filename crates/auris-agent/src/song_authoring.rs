//! Isolated startup-song authoring through the same tool worker as the agent panel.

use crate::Worker;
use auris_session::{
    AgentPreferences, Session, Settings, SingerTakeState, SongCandidate, SongIdea, SongRequest,
    SongSinging,
    agent_policy::{Decision, Operation},
    live_agent::{AgentStep, Command},
};
use serde_json::{Value, json};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::TryRecvError,
    },
    time::{Duration, Instant},
};

/// Author, mix and render one candidate without exposing the open project or conversation.
/// Failing or cancelled model work is never replaced with a rule-generated song.
pub fn generate(
    mut prefs: AgentPreferences,
    idea: SongIdea,
    brief: &str,
    request: &SongRequest,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<SongCandidate, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let mut session = request.authoring_session(&idea)?;
    // Replacement here affects only this new, disposable candidate. Explicit denials and
    // read-only/plan modes still win; no file, network or process permissions are added.
    prefs.policy.allow.push("edit_project.edit_notes".into());
    let settings = Settings {
        plugin_paths: request.plugin_paths.clone(),
        ..Settings::load()
    };
    let mut worker = Worker::spawn(prefs.clone(), None, true, None)?;
    let sharing = match request.singing {
        SongSinging::Auto => "alternate lyrical sections; join together in the final section",
        SongSinging::Alternate => "alternate lyrical sections between the selected singers",
        SongSinging::Harmony => {
            "sing together with deliberately authored complementary harmony lines"
        }
    };
    let prompt = format!(
        "Create this complete song in the isolated candidate document using Auris tools. \
         Inspect it first: selected instruments and singers are already installed, with no notes. \
         Preserve those exact choices. Use the same note-authoring and production workflow as the agent panel. \
         Write expressive parts yourself with add_clip/read_notes/edit_notes, including the lead, \
         motif developments, countermelodies, distinctive bass lines and fills. Do not use compose, \
         accompaniment, candidates, phrase_variation, arrangement or recipe edits here; use generate_phrase \
         only for plain chord/pad backing, simple arpeggios or basic drum pulses on supporting tracks. \
         Write lyrics (supplied below), section chords, notes and arrangement, then purposeful handoffs/SFX, \
         then mix using mixer_graph and supported effects. Synthesize all selected vocal parts with sing \
         and check their current takes. Before answering, inspect_audio the full mix after the last edit, \
         covering the whole song in bounded windows if needed. The host renders and retains the final \
         candidate; do not save/export files, run shell commands, install tools, use external evaluators \
         or request external downloads. Available local sounds can supply SFX. \
         The preset is a style reference, not an instruction to generate notes. Density is not a note quota. \
         Finish the requested form and length, not just a motif.\nDirection: {}\nDuration: {:?} seconds \
         (None means choose a suitable complete form). Singer sharing: {}.\nLyrics: {}\nBrief: {}",
        serde_json::to_string(&idea).map_err(|e| e.to_string())?,
        request.seconds,
        sharing,
        serde_json::to_string(&request.lyrics).map_err(|e| e.to_string())?,
        serde_json::to_string(brief).map_err(|e| e.to_string())?,
    );
    worker.send(&json!({"say":prompt,"policy":prefs.policy}).to_string())?;
    progress("compose");
    let deadline = Instant::now() + Duration::from_secs(1200);
    let mut evidence = Evidence::default();
    let mut repairs = 0;
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            worker.cancel();
            return Err(if cancel.load(Ordering::Acquire) {
                "Cancelled"
            } else {
                "Song authoring timed out"
            }
            .into());
        }
        let event = match worker.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(TryRecvError::Disconnected) => return Err("Song authoring worker stopped".into()),
        };
        match event["event"].as_str().unwrap_or_default() {
            "permission" => {
                let allowed =
                    Operation::parse(event["tool"].as_str().unwrap_or_default(), &event["args"])
                        .is_ok_and(|op| {
                            matches!(prefs.policy.decide(&op), Decision::Allow)
                                && !matches!(
                                    op.name.as_str(),
                                    "listen"
                                        | "cli_file_read"
                                        | "cli_file_change"
                                        | "cli_command"
                                        | "search_internet"
                                        | "export_audio_attachment"
                                )
                        });
                worker.send(&json!({"event":"permission_result","id":event["id"],"ok":allowed,
                    "reason":"Candidate authoring uses approved local Auris edits only; operations needing interactive approval are unavailable here."}).to_string())?;
            }
            "edit" => {
                let result = serde_json::from_value::<Command>(event["command"].clone())
                    .map_err(|e| e.to_string())
                    .and_then(|command| {
                        if matches!(command, Command::Sing { .. }) {
                            progress("sing");
                        }
                        execute(&mut session, command, &settings, cancel, &mut evidence)
                    });
                let response = result
                    .unwrap_or_else(|error| json!({"event":"edit_result","ok":false,"text":error}));
                worker.send(&response.to_string())?;
            }
            "answer" => match evidence.validate(&session, request) {
                Ok(()) => break,
                Err(error) if repairs < 2 => {
                    repairs += 1;
                    worker.send(&json!({"say":format!("The candidate is unfinished: {error}. Complete the missing work using tools before answering."),"policy":prefs.policy}).to_string())?;
                }
                Err(error) => return Err(error),
            },
            "error" | "ended" => return Err(format!("Song authoring stopped: {event}")),
            _ => {}
        }
    }
    SongCandidate::from_authored_session(idea, brief, &mut session, cancel, progress)
}

#[derive(Default)]
struct Evidence {
    authored: bool,
    mixed: bool,
    inspected_revision: Option<u64>,
    inspected_ranges: Vec<(u32, u32)>,
}

impl Evidence {
    fn validate(&self, session: &Session, request: &SongRequest) -> Result<(), String> {
        let has_notes = session.project().tracks.iter().any(|track| {
            track.kind.note_clips().is_some_and(|clips| {
                clips
                    .iter()
                    .any(|clip| clip.recipe.is_none() && !clip.notes.is_empty())
            })
        });
        if !self.authored || !has_notes {
            return Err(
                "Author the expressive parts with edit_notes; no authored notes are present".into(),
            );
        }
        let mut through = 1;
        let mut ranges = self.inspected_ranges.clone();
        ranges.sort_unstable();
        for (start, end) in ranges {
            if start > through {
                break;
            }
            through = through.max(end);
        }
        if !self.mixed
            || self.inspected_revision != Some(session.revision())
            || session.project().signatures.bar_start(through) < session.project().end_tick()
        {
            return Err("Review/adjust the mixer and inspect_audio the current full mix after the final edit".into());
        }
        if let Some(seconds) = request.seconds {
            let project = session.project();
            let actual = project.tempo_map.ticks_to_seconds(project.end_tick()).0;
            let bar_seconds = project
                .tempo_map
                .ticks_to_seconds(project.signatures.bar_start(2))
                .0;
            if (actual - f64::from(seconds)).abs() > bar_seconds.max(f64::from(seconds) * 0.1) {
                return Err(format!(
                    "Complete the requested duration: expected about {seconds}s, score is {actual:.1}s"
                ));
            }
        }
        let singers: Vec<_> = session
            .project()
            .tracks
            .iter()
            .filter(|t| t.kind.as_singer().is_some())
            .collect();
        if singers.len() < request.singers.len() {
            return Err("Restore the selected singer tracks and finish their parts".into());
        }
        for track in singers {
            if track
                .kind
                .note_clips()
                .is_none_or(|clips| clips.iter().all(|c| c.notes.is_empty()))
                || !matches!(
                    session.singer_take_state(track.id),
                    Ok(SingerTakeState::Current)
                )
            {
                return Err(format!(
                    "Finish and synthesize the current vocal score for {}",
                    track.name
                ));
            }
        }
        Ok(())
    }
}

fn execute(
    session: &mut Session,
    command: Command,
    settings: &Settings,
    cancel: &AtomicBool,
    evidence: &mut Evidence,
) -> Result<Value, String> {
    match &command {
        Command::Compose { .. } | Command::Accompaniment { .. } | Command::Candidates { .. }
        | Command::PhraseVariation { .. } | Command::Arrangement { .. } | Command::EditRecipe { .. } =>
            return Err("Write expressive notes directly with edit_notes. Only simple backing may use generate_phrase.".into()),
        Command::GeneratePhrase { part, .. } if !matches!(part.as_str(), "chords" | "pad" | "arp" | "drums" | "kick" | "snare" | "hat") =>
            return Err("Author lead and bass notes directly with edit_notes".into()),
        Command::Listen { .. } | Command::Render { .. } | Command::Preview { .. } | Command::ExportMidi { .. } =>
            return Err("Use inspect_audio; the host retains the candidate and renders its audition".into()),
        _ => {}
    }
    if let Command::InspectAudio {
        start_bar,
        bars,
        track,
    } = command
    {
        let report = session
            .audio_inspection_job(start_bar, bars, track)
            .map_err(|e| e.to_string())?
            .run(cancel)
            .map_err(|e| e.to_string())?;
        if track.is_none() {
            if evidence.inspected_revision != Some(session.revision()) {
                evidence.inspected_ranges.clear();
            }
            evidence.inspected_revision = Some(session.revision());
            evidence
                .inspected_ranges
                .push((start_bar, start_bar + bars));
        }
        return Ok(json!({"event":"edit_result","ok":true,"inspection":report}));
    }
    let authored = matches!(command, Command::EditNotes { .. });
    let mixed = matches!(command, Command::MixerGraph { .. });
    let text = if command.needs_worker() {
        let mut job = session.begin_agent_job(command, settings.clone())?;
        loop {
            match session.continue_agent_job(job.run(cancel)?, cancel)? {
                AgentStep::Work(next) => job = next,
                AgentStep::Done(text) => break text,
            }
        }
    } else {
        session.agent_command_with_plugin_paths(command, &settings.plugin_paths)?
    };
    evidence.authored |= authored;
    evidence.mixed |= mixed;
    Ok(json!({"event":"edit_result","ok":true,"text":text}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::{SongSourceChoice, prelude::*};

    fn idea() -> SongIdea {
        SongIdea {
            title: "Authored test".into(),
            description: "A short motif".into(),
            preset: "pop-band".into(),
            tempo: 120.0,
            key: "C major".into(),
            density: 0.5,
            lead: "synth".into(),
        }
    }

    fn request() -> SongRequest {
        SongRequest {
            sources: vec![SongSourceChoice {
                role: "lead".into(),
                instrument: "auris.synth.fm2".into(),
                source: None,
            }],
            ..Default::default()
        }
    }

    fn call(name: &str, args: Value) -> String {
        crate::tests::completion(
            &json!({"role":"assistant","tool_calls":[{
                "id":name,"type":"function","function":{"name":name,"arguments":args.to_string()}
            }]})
            .to_string(),
            "tool_calls",
        )
    }

    #[test]
    fn configured_worker_authors_notes_mixes_and_renders_the_candidate() {
        let request = request();
        let mut expected = request.authoring_session(&idea()).unwrap();
        let clip_args = json!({"track":1,"name":"Hook","start_bar":1,"bars":2});
        expected
            .agent_command(Command::AddClip {
                track: 1,
                name: "Hook".into(),
                start_bar: 1,
                bars: 2,
            })
            .unwrap();
        let revision = expected.agent_document_revision().unwrap();
        // Read back rather than assumed: the new document's master limiter takes an id too.
        let clip = expected.project().tracks[0]
            .kind
            .as_instrument()
            .unwrap()
            .clips[0]
            .id
            .0;
        let edits = json!([{"kind":"replace","clip":clip,"notes":[
            {"pitch":64,"bar":1,"beat":1,"length_beats":0.5,"velocity":0.7},
            {"pitch":67,"bar":1,"beat":1.75,"length_beats":0.25,"velocity":0.6},
            {"pitch":69,"bar":1,"beat":2,"length_beats":1,"velocity":0.8}
        ]}]);
        let (url, seen) = crate::tests::mock_server(vec![
            call("add_clip", clip_args),
            call("read_notes", json!({"clips":[{"clip":clip}]})),
            call(
                "edit_notes",
                json!({"expected_revision":revision,"input":{"kind":"inline","edits":edits}}),
            ),
            call(
                "mixer_graph",
                json!({"operation":{"action":"set_strip","node":"track:1","gain_db":-9.0}}),
            ),
            call("inspect_audio", json!({"start_bar":1,"bars":2})),
            crate::tests::completion(r#"{"role":"assistant","content":"Finished"}"#, "stop"),
        ]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let candidate = generate(
            prefs,
            idea(),
            "A short instrumental test",
            &request,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(candidate.part_names(), vec!["lead"]);
        assert!(candidate.audio.peak() > 1e-8);
        let folder = tempfile::tempdir().unwrap();
        let mut adopted = Session::new(auris_session::SessionOptions::headless()).unwrap();
        adopted
            .save_as(&folder.path().join("Adopted.auris"))
            .unwrap();
        adopted.adopt_song_candidate(&candidate).unwrap();
        let track = &adopted.project().tracks[0];
        assert_eq!(track.mixer.gain_db, -9.0);
        let clip = &track.kind.note_clips().unwrap()[0];
        assert!(clip.recipe.is_none());
        assert_eq!(
            clip.notes
                .iter()
                .map(|note| (note.pitch, note.start.raw(), note.length.raw()))
                .collect::<Vec<_>>(),
            vec![(64, 0, 480), (67, 720, 240), (69, 960, 960)]
        );
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 6, "unexpected repair or tool failure");
        let body: Value = serde_json::from_str(&requests[0]).unwrap();
        assert!(
            body["messages"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(crate::MUSICAL_DESIGN_INSTRUCTIONS)
        );
    }

    #[test]
    fn refuses_rule_written_lead_and_requires_fresh_complete_mix_coverage() {
        let request = request();
        let mut session = request.authoring_session(&idea()).unwrap();
        let mut evidence = Evidence::default();
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let baseline = session.project().clone();
        let error = execute(
            &mut session,
            Command::GeneratePhrase {
                track: 1,
                part: "lead".into(),
                start_bar: 1,
                bars: 2,
                seed: 0,
            },
            &settings,
            &cancel,
            &mut evidence,
        )
        .unwrap_err();
        assert!(error.contains("Author lead"));
        assert_eq!(session.project(), &baseline);
        let clip = session
            .add_midi_clip(
                TrackId(1),
                "Long phrase",
                Ticks::ZERO,
                Ticks::from_beats(36.0),
            )
            .unwrap();
        let revision = session.agent_document_revision().unwrap();
        let command: Command = serde_json::from_value(json!({"action":"edit_notes","expected_revision":revision,
            "edits":[{"kind":"patch","clip":clip.0,"add":[{"pitch":64,"bar":1,"beat":1,"length_beats":1,"velocity":0.6}]}]})).unwrap();
        execute(&mut session, command, &settings, &cancel, &mut evidence).unwrap();
        let mixer: Command =
            serde_json::from_value(json!({"action":"mixer_graph","operation":{"action":"read"}}))
                .unwrap();
        execute(&mut session, mixer, &settings, &cancel, &mut evidence).unwrap();
        execute(
            &mut session,
            Command::InspectAudio {
                start_bar: 1,
                bars: 8,
                track: None,
            },
            &settings,
            &cancel,
            &mut evidence,
        )
        .unwrap();
        assert!(evidence.validate(&session, &request).is_err());
        execute(
            &mut session,
            Command::InspectAudio {
                start_bar: 9,
                bars: 1,
                track: None,
            },
            &settings,
            &cancel,
            &mut evidence,
        )
        .unwrap();
        evidence.validate(&session, &request).unwrap();
        session
            .add_note(clip, Note::new(67, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();
        assert!(evidence.validate(&session, &request).is_err());
    }

    #[test]
    fn answer_without_tools_never_falls_back_to_automatic_composition() {
        let answer =
            crate::tests::completion(r#"{"role":"assistant","content":"Finished"}"#, "stop");
        let (url, _) = crate::tests::mock_server(vec![answer.clone(), answer.clone(), answer]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let result = generate(
            prefs,
            idea(),
            "Instrumental",
            &request(),
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(result.err().unwrap().contains("no authored notes"));
        let result = generate(
            AgentPreferences::default(),
            idea(),
            "Cancelled",
            &request(),
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert_eq!(result.err().unwrap(), "Cancelled");
    }

    #[test]
    #[ignore = "uses the user's configured model and may incur provider usage"]
    fn configured_provider_authors_a_short_startup_candidate() {
        let request = SongRequest {
            seconds: Some(30),
            ..request()
        };
        let candidate = generate(Settings::load().agent, idea(),
            "Create a 30-second instrumental pop sketch with a memorable authored lead, a recognizable varied return, simple quiet chord backing and a resolved ending. Use the installed built-in instruments. Finish the mix.",
            &request, &AtomicBool::new(false), &mut |stage| eprintln!("candidate stage: {stage}"))
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut session = Session::new(auris_session::SessionOptions::headless()).unwrap();
        session
            .save_as(&folder.path().join("Candidate.auris"))
            .unwrap();
        session.adopt_song_candidate(&candidate).unwrap();
        let clips: Vec<_> = session
            .project()
            .tracks
            .iter()
            .flat_map(|t| t.kind.note_clips().into_iter().flatten())
            .collect();
        assert!(
            clips
                .iter()
                .any(|clip| clip.recipe.is_none() && !clip.notes.is_empty())
        );
        eprintln!(
            "Candidate: {:.1}s, {} clips ({} without recipes), peak {:.5}",
            session.project().duration_seconds(),
            clips.len(),
            clips.iter().filter(|c| c.recipe.is_none()).count(),
            candidate.audio.peak()
        );
    }
}
