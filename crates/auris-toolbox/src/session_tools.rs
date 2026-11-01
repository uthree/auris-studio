//! Saved execution of the canonical session contracts; no legacy argument translation.

use crate::{ToolDefinition, live_agent};

#[cfg(test)]
const REMOVED_TOOLS: &[&str] = &[
    "add_note",
    "add_notes",
    "remove_notes",
    "replace_notes",
    "replace_notes_batch",
    "mixer",
    "effects",
    "set_effect",
    "routing",
    "set_level",
    "set_track_state",
    "set_instrument_param",
    "add_part",
    "describe",
    "notes",
    "similar_instruments",
    "list_instruments",
    "regenerate_clips",
    "set_tempo",
    "create_piano_cover",
    "transcribe_mixture",
];

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    live_agent::definitions()
        .into_iter()
        .filter(|tool| {
            !matches!(
                tool.name.as_str(),
                "history"
                    | "execution_trace"
                    | "production_record"
                    | "capabilities"
                    | "discover_tools"
                    | "tool_help"
                    | "search_documentation"
                    | "export_audio_attachment"
                    | "read_report"
            )
        })
        .map(|tool| {
            let name = tool.name;
            let optional_project = matches!(name.as_str(), "analyze_lyrics" | "diagnose_song");
            let mut parameters = tool.parameters;
            parameters["properties"].as_object_mut().expect("session object schema").insert("project".into(), serde_json::json!({"type":"string","description":"Project path or project_id from open_project/create_project."}));
            if parameters.get("required").is_none() { parameters["required"] = serde_json::json!([]); }
            if !optional_project {
                parameters["required"].as_array_mut().expect("required session fields").push("project".into());
            }
            let action = name.clone();
            ToolDefinition {
                name,
                description: tool.description,
                parameters,
                call: Box::new(move |mut args| {
                    let object = args.as_object_mut().ok_or("Expected argument object")?;
                    let project = object.remove("project").map(|value| value.as_str().map(str::to_owned).ok_or("project must be a string")).transpose()?;
                    if project.is_none() && !optional_project {
                        return Err("Invalid arguments: project is required".into());
                    }
                    let command = if action == "edit_notes" {
                        serde_json::from_value::<crate::score_input::Args>(args)
                            .map_err(|e| format!("Invalid arguments: {e}"))?
                            .command()?
                    } else if action == "import_transcription" {
                        serde_json::from_value::<live_agent::transcription_import::Args>(args)
                            .map_err(|e| format!("Invalid arguments: {e}"))?
                            .command(&auris_session::AnalysisControl::cancelled_by(
                                crate::cancellation::current().flag_handle(),
                            ))?
                    } else {
                        live_agent::command(&action, &args)?.ok_or("Unknown session tool")?
                    };
                    let mut session = match project {
                        Some(project) => crate::opened(&project)?,
                        None => crate::headless()?,
                    };
                    if let auris_session::live_agent::Command::InspectAudio {
                        start_bar,
                        bars,
                        track,
                    } = command
                    {
                        let report = session
                            .audio_inspection_job(start_bar, bars, track)?
                            .run(crate::cancellation::current().flag())?;
                        return serde_json::to_string(&report).map_err(|e| e.to_string());
                    }
                    if let auris_session::live_agent::Command::Listen {
                        start_bar,
                        bars,
                        focus,
                        compare_to,
                        aspect,
                    } = command
                    {
                        return crate::saved_listening::run(
                            &mut session,
                            start_bar,
                            bars,
                            focus.as_deref(),
                            compare_to.as_deref(),
                            aspect,
                        );
                    }
                    let before = command.is_mutating().then(|| session.project().clone());
                    let text = if command.needs_worker() {
                        let cancellation = crate::cancellation::current();
                        let mut job =
                            session.begin_agent_job(command, auris_session::Settings::load())?;
                        loop {
                            let result = job.run(cancellation.flag())?;
                            match session.continue_agent_job(result, cancellation.flag())? {
                                auris_session::live_agent::AgentStep::Done(text) => break text,
                                auris_session::live_agent::AgentStep::Work(next) => job = next,
                            }
                        }
                    } else {
                        session.agent_command_with_plugin_paths(
                            command,
                            &crate::sound_search::configured_plugin_paths(),
                        )?
                    };
                    if before
                        .as_ref()
                        .is_some_and(|before| session.project() != before)
                    {
                        crate::save_checkpointed(&mut session)?;
                    }
                    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text)
                        && let Some(object) = value.as_object_mut()
                        && object.contains_key("unsaved")
                    {
                        object.insert("unsaved".into(), session.is_dirty().into());
                        return Ok(value.to_string());
                    }
                    if matches!(action.as_str(), "analyze_lyrics" | "diagnose_song" | "repair_lyrics" | "arrangement" | "candidates" | "phrase_variation") {
                        return crate::present_domain_report(&text);
                    }
                    Ok(text)
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::prelude::*;
    use serde_json::{Value, json};

    #[test]
    fn saved_candidate_rounds_preserve_files_until_exact_adoption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Candidates.auris");
        let mut session = crate::headless().unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        session.save(&path).unwrap();
        let revision = session.agent_document_revision().unwrap();
        let before = std::fs::read(&path).unwrap();
        let call = |operation| {
            crate::runtime::call_saved(
                &crate::ToolGroups::parse("manual").unwrap(),
                "candidates",
                json!({"project":path,"operation":operation}),
            )
        };
        let round:Value=serde_json::from_str(&call(json!({"kind":"create","expected_revision":revision,"candidates":[{"purpose":"Answer on the dominant","edit":{"kind":"notes","edits":[{"kind":"patch","clip":clip.0,"update":[{"index":0,"pitch":67}]}]}}]})).unwrap()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let read: Value = serde_json::from_str(
            &call(json!({"kind":"read","round_id":round["round_id"],"candidate":1,"clip":clip.0}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(read["notes"][0]["after"]["pitch"], 67);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        call(json!({"kind":"apply","round_id":round["round_id"],"candidate":1,"expected_revision":revision})).unwrap();
        let after = crate::opened(path.to_str().unwrap()).unwrap();
        let mut expected = session.project().clone();
        expected.midi_clip_mut(clip).unwrap().notes[0].pitch = 67;
        assert_eq!(after.project(), &expected);
        call(json!({"kind":"apply","round_id":round["round_id"],"candidate":1,"expected_revision":revision})).unwrap();
        assert_eq!(
            crate::opened(path.to_str().unwrap()).unwrap().project(),
            &expected
        );
    }

    #[test]
    fn saved_accompaniment_preserves_melody_and_selective_regeneration_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Backing.auris");
        let mut session = crate::headless().unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let lead = session
            .add_midi_clip(track, "Melody", Ticks::ZERO, Ticks::QUARTER * 16)
            .unwrap();
        for (i, pitch) in [60, 64, 67, 71].into_iter().enumerate() {
            session
                .add_note(
                    lead,
                    Note::new(pitch, Ticks::QUARTER * i as i64, Ticks::QUARTER),
                )
                .unwrap();
        }
        let bass = session.add_default_instrument_track("Bass").unwrap();
        let pad = session.add_default_instrument_track("Pad").unwrap();
        session.stamp_progression(&Chart::parse("| I | IV | V | I |").unwrap(), Ticks::ZERO, 4);
        session.save(&path).unwrap();
        let revision = crate::opened(path.to_str().unwrap())
            .unwrap()
            .agent_document_revision()
            .unwrap();
        let args = json!({"project":path,"expected_revision":revision,"protected_clips":[lead.0],"foreground_clips":[lead.0],"operation":{"kind":"generate","parts":[
            {"track":bass.0,"name":"Bass","part":"bass","start_bar":1,"bars":4,"seed":23},
            {"track":pad.0,"name":"Pad","part":"pad","start_bar":1,"bars":4,"seed":42}
        ]}});
        let groups = crate::ToolGroups::parse("manual").unwrap();
        let result = crate::runtime::call_saved(&groups, "accompaniment", args.clone()).unwrap();
        let report: Value = serde_json::from_str(&result).unwrap();
        assert!(!result.contains("\"notes\":"));
        let before = crate::opened(path.to_str().unwrap()).unwrap();
        assert_eq!(before.midi_clip(lead), session.midi_clip(lead));
        assert_eq!(
            report["revision"],
            before.agent_document_revision().unwrap()
        );
        let bytes = std::fs::read(&path).unwrap();
        assert!(crate::runtime::call_saved(&groups, "accompaniment", args).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let id = ClipId(report["clips"][0]["clip"].as_u64().unwrap());
        let untouched = ClipId(report["clips"][1]["clip"].as_u64().unwrap());
        let suggestions: Value = serde_json::from_str(&crate::runtime::call_saved(&groups,"suggest_harmony",json!({
            "project":path,"request":{"melody_clips":[lead.0],"start_bar":1,"bars":4,"mood":"dreamy"}
        })).unwrap()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(suggestions["revision"], report["revision"]);
        assert_eq!(
            suggestions["candidates"][0]["harmony"]["chords"],
            "| Imaj7 | Imaj7 | Imaj7 | Imaj7 |"
        );
        let apply = json!({"project":path,"expected_revision":suggestions["revision"],"protected_clips":suggestions["protected_clips"],"foreground_clips":suggestions["foreground_clips"],"operation":{
            "kind":"regenerate","clips":[id.0],"harmony":suggestions["candidates"][0]["harmony"]
        }});
        crate::runtime::call_saved(&groups, "accompaniment", apply.clone()).unwrap();
        assert!(crate::runtime::call_saved(&groups, "accompaniment", apply).is_err());
        let after = crate::opened(path.to_str().unwrap()).unwrap();
        assert_eq!(after.midi_clip(lead), session.midi_clip(lead));
        assert_eq!(after.midi_clip(untouched), before.midi_clip(untouched));
        assert_ne!(
            after.midi_clip(id).unwrap().notes,
            before.midi_clip(id).unwrap().notes
        );
        assert_eq!(
            after.midi_clip(id).unwrap().recipe.as_ref().unwrap().seed,
            23
        );
    }

    #[test]
    fn lyric_drafts_are_diagnosed_without_a_project_and_large_reports_are_paged() {
        let call = |name, args| {
            crate::runtime::call_saved(&crate::ToolGroups::default(), name, args).unwrap()
        };
        let analysis: Value = serde_json::from_str(&call(
            "analyze_lyrics",
            json!({"text":"きゃっと、コーヒー"}),
        ))
        .unwrap();
        assert_eq!(analysis["complete"], true);
        let spec = "form='verse verse2'\n[section.verse]\nlyrics='ずっときみと'\n[section.verse2]\nmelody_from='verse'\nlyrics='きみとあえる'";
        let report: Value =
            serde_json::from_str(&call("diagnose_song", json!({"spec":spec}))).unwrap();
        assert_eq!(report["status"], "fail");
        assert!(
            report["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "closure_position_mismatch")
        );
        let paged: Value = serde_json::from_str(&call(
            "analyze_lyrics",
            json!({"text":"あいうえお\n".repeat(100)}),
        ))
        .unwrap();
        assert!(paged["report_id"].is_string());
    }

    #[test]
    fn saved_lyric_preview_never_saves_and_repair_preserves_musical_notes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Lyrics.auris");
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Vocal");
        let mut ids = Vec::new();
        for (i, text) in ["ずっときみと", "きみとあえる"].iter().enumerate() {
            let clip = session
                .add_midi_clip(track, "Verse", Ticks(i as i64 * 7680), Ticks(7680))
                .unwrap();
            session
                .replace_notes(
                    clip,
                    (0..6)
                        .map(|i| Note::new(60, Ticks(i * 960), Ticks(800)))
                        .collect(),
                )
                .unwrap();
            session
                .write_lyrics(clip, &[0, 1, 2, 3, 4, 5], text)
                .unwrap();
            ids.push(clip);
        }
        session.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let call = |name, args| -> Value {
            serde_json::from_str(
                &crate::runtime::call_saved(&crate::ToolGroups::default(), name, args).unwrap(),
            )
            .unwrap()
        };
        let report = call(
            "diagnose_song",
            json!({"project":path,"comparisons":[{
                "reference":{"clip":ids[0].0,"notes":[0,1,2,3,4,5]},
                "target":{"clip":ids[1].0,"notes":[0,1,2,3,4,5]}
            }]}),
        );
        let mut args = json!({"project":path,"target":{"clip":ids[1].0,"notes":[0,1,2,3,4,5]},
            "reference":{"clip":ids[0].0,"notes":[0,1,2,3,4,5]},"expected_revision":report["revision"],
            "candidates":[{"text":"きっとあえる"}],"apply":false});
        let preview = call("repair_lyrics", args.clone());
        assert_eq!(preview["selected_candidate"], 1);
        assert_eq!(preview["applied"], false);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        args["apply"] = true.into();
        assert_eq!(call("repair_lyrics", args)["applied"], true);
        let reopened = crate::opened(path.to_str().unwrap()).unwrap();
        for id in ids {
            let original = &session.midi_clip(id).unwrap().notes;
            let repaired = &reopened.midi_clip(id).unwrap().notes;
            for (a, b) in original.iter().zip(repaired) {
                assert_eq!(
                    (a.pitch, a.start, a.length, a.velocity),
                    (b.pitch, b.start, b.length, b.velocity)
                );
            }
        }
    }

    #[test]
    fn saved_creation_ids_are_direct_inputs_to_the_shared_editors() {
        let directory = tempfile::tempdir().unwrap();
        let call = |name, args| {
            serde_json::from_str::<Value>(
                &crate::runtime::call_saved(&crate::ToolGroups::default(), name, args).unwrap(),
            )
            .unwrap()
        };
        let created = call(
            "create_project",
            json!({"output":directory.path().join("Manual.auris"),"tempo":10,"meter":"6/8"}),
        );
        assert_eq!(created["tempo"].as_f64(), Some(10.0));
        assert!(created["tracks"][0]["track"].is_u64());
        let clip = call(
            "add_clip",
            json!({"project":created["project_id"],"track":created["tracks"][0]["track"],"name":"Phrase","start_bar":1,"bars":1}),
        );
        let read = call(
            "read_notes",
            json!({"project":created["project_id"],"clips":[{"clip":clip["clip"]}]}),
        );
        assert_eq!(read["clips"][0]["total"], 0);
        assert_eq!(read["clips"][0]["bars"], json!([1, 1]));
    }

    #[test]
    fn saved_inspection_uses_the_common_range_and_never_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Inspection.auris");
        let mut session = crate::headless().unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks(997))])
            .unwrap();
        session.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let result = crate::runtime::call_saved(
            &crate::ToolGroups::default(),
            "inspect_audio",
            json!({"project":path,"start_bar":1,"bars":1}),
        )
        .unwrap();
        let report: auris_session::audio_inspection::Inspection =
            serde_json::from_str(&result).unwrap();
        let visual = crate::audio_inspection::present(&report).unwrap();
        assert!(!visual.png.is_empty());
        assert!(report.columns > 0);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn saved_and_live_contracts_match_and_removed_names_have_no_dispatch() {
        let saved = crate::tool_catalog();
        for live in live_agent::definitions() {
            if let Some(definition) = definitions()
                .into_iter()
                .find(|tool| tool.name == live.name)
            {
                let mut schema = definition.parameters;
                schema["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("project");
                schema["required"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|value| value != "project");
                let mut expected = live.parameters;
                if expected.get("required").is_none() {
                    expected["required"] = json!([]);
                }
                assert_eq!(schema, expected, "{}", live.name);
            }
        }
        for name in REMOVED_TOOLS {
            assert!(!saved.iter().any(|tool| tool.name == *name), "{name}");
            assert!(
                crate::runtime::call_saved(&crate::ToolGroups::default(), name, json!({}))
                    .unwrap_err()
                    .contains("Unknown tool")
            );
            assert!(live_agent::command(name, &json!({})).unwrap().is_none());
        }
    }

    #[test]
    fn exact_score_and_performance_round_trip_through_saved_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Song.auris");
        let mut session = crate::headless().unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
            .unwrap();
        session.save(&path).unwrap();
        let call = |name, args| {
            crate::runtime::call_saved(&crate::ToolGroups::default(), name, args).unwrap()
        };
        let read = || {
            serde_json::from_str::<Value>(&call(
                "read_notes",
                json!({"project":path,"clips":[{"clip":clip.0}]}),
            ))
            .unwrap()
        };
        let initial = read();
        let edits = json!([{"kind":"replace","clip":clip.0,"notes":[{"pitch":60,"bar":1,"beat":2.75,"length_beats":0.125,"velocity":0.73125}]}]);
        call(
            "edit_notes",
            json!({"project":path,"expected_revision":initial["revision"],"input":{"kind":"inline","edits":edits}}),
        );
        let current = read();
        assert_eq!(current["clips"][0]["notes"][0]["beat"], 2.75);
        assert_eq!(current["clips"][0]["notes"][0]["length_beats"], 0.125);
        let bytes = std::fs::read(&path).unwrap();
        call(
            "edit_notes",
            json!({"project":path,"expected_revision":current["revision"],"input":{"kind":"inline","edits":edits}}),
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        call(
            "set_performance",
            json!({"project":path,"clip":clip.0,"settings":{"gate":0.5}}),
        );
        let performance: Value = serde_json::from_str(&call(
            "inspect_performance",
            json!({"project":path,"clip":clip.0}),
        ))
        .unwrap();
        assert_eq!(performance["settings"]["gate"], 0.5);
        assert_eq!(read()["clips"][0]["notes"], current["clips"][0]["notes"]);
    }
    #[test]
    fn saved_audio_material_processing_reopens_sources_and_persists_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("fixture.wav");
        let mut session = crate::headless().unwrap();
        session
            .render_job()
            .render_to_wav(
                &wav,
                &Default::default(),
                &auris_session::prelude::OfflineOptions {
                    end_frames: Some(4800),
                    include_tail: false,
                    ..Default::default()
                },
                &mut Default::default(),
            )
            .unwrap();
        let project = directory.path().join("Materials.auris");
        session.save(&project).unwrap();
        let call = |name, args| {
            serde_json::from_str::<Value>(
                &crate::runtime::call_saved(&crate::ToolGroups::default(), name, args).unwrap(),
            )
            .unwrap()
        };
        let import = call(
            "import_audio",
            json!({"project":project,"path":wav,"at_tick":0}),
        );
        let before = std::fs::read(&project).unwrap();
        let inspect = call(
            "audio_assets",
            json!({"project":project,"operation":{"kind":"inspect","clip":import["clip"]}}),
        );
        assert_eq!(std::fs::read(&project).unwrap(), before);
        assert_eq!(
            inspect["source"]["provenance"]["sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        let processed = call(
            "audio_assets",
            json!({"project":project,"operation":{"kind":"process","clip":import["clip"],"expected_revision":inspect["revision"],"reverse":true}}),
        );
        let inspect = call(
            "audio_assets",
            json!({"project":project,"operation":{"kind":"inspect","clip":import["clip"]}}),
        );
        assert_eq!(inspect["source"]["id"], processed["source"]);
        assert_ne!(inspect["source"]["id"], import["source"]);
        assert!(inspect["source"]["provenance"]["parent_sha256"].is_string());
    }
}
