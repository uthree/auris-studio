//! The projection has one contract across live and saved execution.

use auris_session::{
    Session, SessionOptions,
    live_agent::automation::{ControlTarget, Interpolation, Operation, Point},
    prelude::*,
};
use serde_json::{Value, json};

fn live(session: &mut Session, args: Value) -> Result<Value, String> {
    let command = crate::live_agent::command("pipeline_read", &args)?
        .ok_or("pipeline_read must be a canonical session command")?;
    assert!(!command.is_mutating());
    assert!(!command.needs_worker());
    let text = session.agent_command(command)?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn edit(session: &mut Session, args: Value) -> Result<Value, String> {
    let command = crate::live_agent::command("pipeline_edit", &args)?
        .ok_or("pipeline_edit must be a canonical session command")?;
    assert!(command.is_mutating());
    assert!(!command.needs_worker());
    let text = session.agent_command(command)?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn staged_session() -> (Session, ClipId) {
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session.add_default_instrument_track("Lead").unwrap();
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    session
        .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
        .unwrap();
    session
        .set_clip_transforms(
            clip,
            vec![
                NoteTransform::Humanize {
                    amount: 0.1,
                    seed: 11,
                },
                NoteTransform::Humanize {
                    amount: 0.2,
                    seed: 22,
                },
                NoteTransform::Gate { amount: 0.8 },
            ],
        )
        .unwrap();
    (session, clip)
}

fn clip_revision_and_stage(session: &mut Session, clip: ClipId) -> (String, String) {
    let page = live(
        session,
        json!({"scope":format!("clip:{}",clip.0),"limit":64,"edge_limit":128}),
    )
    .unwrap();
    let stage = page["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == format!("clip:{}/stage:1", clip.0))
        .unwrap();
    (
        page["revision"].as_str().unwrap().into(),
        stage["id"].as_str().unwrap().into(),
    )
}

#[test]
fn pipeline_read_has_the_same_live_and_saved_contract_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Pipeline.auris");
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session.add_default_instrument_track("Lead").unwrap();
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    session
        .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
        .unwrap();
    session.save(&path).unwrap();
    let before = session.project().clone();
    let history_revision = session.revision();
    let bytes = std::fs::read(&path).unwrap();
    let args = json!({"limit":1});
    let first = live(&mut session, args.clone()).unwrap();
    let groups = crate::ToolGroups::parse("mix").unwrap();
    let saved = |mut args: Value| {
        args["project"] = json!(path);
        let text = crate::runtime::call_saved(&groups, "pipeline_read", args).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()
    };
    assert_eq!(saved(args), first);
    let next = json!({
        "offset":first["next_offset"], "limit":1,
        "expected_revision":first["revision"]
    });
    let second = live(&mut session, next.clone()).unwrap();
    assert_eq!(saved(next), second);
    assert_eq!(second["revision"], first["revision"]);
    assert_ne!(second["nodes"][0]["id"], first["nodes"][0]["id"]);
    let focus = json!({"scope":format!("clip:{}", clip.0),"limit":64,"edge_limit":128});
    let focused = live(&mut session, focus.clone()).unwrap();
    assert_eq!(focused["scope"], format!("clip:{}", clip.0));
    assert_eq!(saved(focus), focused);
    assert!(
        focused["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "clip_stage" || node["kind"] == "note_source")
    );
    assert_eq!(session.project(), &before);
    assert_eq!(session.revision(), history_revision);
    assert!(!session.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(
        crate::opened(path.to_str().unwrap())
            .unwrap()
            .checkpoints()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pipeline_edit_is_flat_bounded_and_uses_the_pipeline_content_revision() {
    let definition = crate::live_agent::definitions()
        .into_iter()
        .find(|tool| tool.name == "pipeline_edit")
        .expect("pipeline_edit is in the shared live catalog");
    assert_eq!(crate::tool_group(&definition.name), "mix");
    assert!(
        definition.parameters["properties"]["action"]["enum"]
            .as_array()
            .is_some_and(|actions| actions.len() == 3)
    );
    assert!(
        crate::live_agent::command(
            "pipeline_edit",
            &json!({"node":"clip:2/stage:0","expected_revision":"hash","action":"remove"})
        )
        .unwrap()
        .is_some()
    );
    assert!(crate::live_agent::command(
        "pipeline_edit",
        &json!({"node":"clip:2/stage:0","expected_revision":"hash","action":"remove","operation":"remove"})
    )
    .is_err());
}

#[test]
fn pipeline_read_detail_is_bounded_revision_bound_and_shared_by_live_and_saved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("PipelineDetail.auris");
    let (mut session, clip) = staged_session();
    session.save(&path).unwrap();
    let before = session.project().clone();
    let history_revision = session.revision();
    let bytes = std::fs::read(&path).unwrap();
    let page = live(
        &mut session,
        json!({
            "scope":format!("clip:{}/stage:1", clip.0), "detail":true, "at_tick":0
        }),
    )
    .unwrap();
    assert_eq!(page["scope"], format!("clip:{}/stage:1", clip.0));
    assert_eq!(page["node"], format!("clip:{}/stage:1", clip.0));
    assert_eq!(page["detail"]["kind"], "stage");
    assert_eq!(page["detail"]["state"]["stage_index"], 1);
    assert!(page["detail"]["state"]["input"].as_array().unwrap().len() <= 32);
    assert!(page["detail"]["state"]["output"].as_array().unwrap().len() <= 32);
    assert_eq!(page["at_tick"], 0);
    assert!(page["revision"].as_str().is_some());

    let mut args = json!({
        "project": path,
        "scope": format!("clip:{}/stage:1", clip.0),
        "detail": true,
        "at_tick": 0
    });
    let saved: Value = serde_json::from_str(
        &crate::runtime::call_saved(
            &crate::ToolGroups::parse("mix").unwrap(),
            "pipeline_read",
            args.take(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["scope"], page["scope"]);
    assert_eq!(saved["detail"], page["detail"]);
    assert_eq!(saved["revision"], page["revision"]);
    assert_eq!(session.project(), &before);
    assert_eq!(session.revision(), history_revision);
    assert!(!session.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn pipeline_read_detail_rejects_unscoped_and_paged_requests() {
    let (mut session, clip) = staged_session();
    assert!(live(&mut session, json!({"detail":true})).is_err());
    assert!(
        live(
            &mut session,
            json!({
                "scope": format!("clip:{}/stage:1", clip.0), "detail":true, "offset":1
            })
        )
        .is_err()
    );
    assert!(
        live(
            &mut session,
            json!({
                "scope": format!("clip:{}/stage:1", clip.0), "detail":true, "limit":1
            })
        )
        .is_err()
    );
    assert!(
        live(
            &mut session,
            json!({
                "scope": format!("clip:{}/stage:1", clip.0), "detail":true, "at_tick":-1
            })
        )
        .is_err()
    );
    assert!(live(&mut session, json!({"at_tick":1})).is_err());
}

#[test]
fn pipeline_read_detail_rejects_stale_revision_and_bounds_malicious_work() {
    let (mut session, clip) = staged_session();
    let page = live(
        &mut session,
        json!({"scope":format!("clip:{}/stage:1", clip.0)}),
    )
    .unwrap();
    let revision = page["revision"].as_str().unwrap().to_owned();
    session
        .set_clip_transforms(clip, vec![NoteTransform::Gate { amount: 0.5 }])
        .unwrap();
    assert!(
        live(
            &mut session,
            json!({
                "scope":format!("clip:{}/stage:1", clip.0), "detail":true,
                "expected_revision":revision
            })
        )
        .is_err()
    );

    let (mut session, clip) = staged_session();
    let detail = live(
        &mut session,
        json!({
            "scope":format!("clip:{}/stage:1", clip.0), "detail":true, "at_tick":i64::MAX
        }),
    )
    .unwrap();
    assert_eq!(detail["detail"]["kind"], "stage");
    assert!(detail["detail"]["state"]["input"].as_array().unwrap().len() <= 32);
    assert!(
        detail["detail"]["state"]["output"]
            .as_array()
            .unwrap()
            .len()
            <= 32
    );
}

#[test]
fn pipeline_read_detail_exposes_fader_and_automation_values_at_absolute_tick() {
    let (mut session, _) = staged_session();
    let track = session.project().tracks.first().unwrap().id;
    let graph = live(
        &mut session,
        json!({"scope":format!("track:{}", track.0), "limit":64}),
    )
    .unwrap();
    let fader = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "fader" && node["id"] == format!("fader:track:{}", track.0))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fader_detail = live(
        &mut session,
        json!({"scope":fader, "detail":true, "at_tick":960}),
    )
    .unwrap();
    assert_eq!(fader_detail["at_tick"], 960);
    assert_eq!(fader_detail["detail"]["kind"], "parameters");
    assert_eq!(fader_detail["detail"]["values"][0]["effective_value"], 0.0);

    let revision = session.agent_document_revision().unwrap();
    session
        .agent_automation(
            ControlTarget::TrackGain { track: track.0 },
            Operation::Set {
                points: vec![Point {
                    at_tick: 960,
                    value: -12.0,
                }],
                replace: true,
                curve: Some(Interpolation::Hold),
            },
            Some(revision),
        )
        .unwrap();
    let graph = live(
        &mut session,
        json!({"scope":format!("track:{}", track.0), "limit":64}),
    )
    .unwrap();
    let automation = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "automation")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let automation_detail = live(
        &mut session,
        json!({"scope":automation, "detail":true, "at_tick":960}),
    )
    .unwrap();
    assert_eq!(automation_detail["detail"]["kind"], "parameters");
    assert_eq!(
        automation_detail["detail"]["values"][0]["automation_value"],
        -12.0
    );
    assert_eq!(
        automation_detail["detail"]["values"][0]["effective_value"],
        -12.0
    );
}

#[test]
fn pipeline_read_detail_resolves_a_high_fan_in_node_beyond_the_first_page() {
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let mut sources = Vec::new();
    for _ in 0..70 {
        sources.push(session.add_bus_track("Source"));
    }
    let hub = session.add_bus_track("Hub");
    for source in sources {
        session.set_track_output(source, Output::Bus(hub)).unwrap();
    }
    let node = format!("track:{}", hub.0);
    let page = live(&mut session, json!({"scope":node.clone(), "limit":64})).unwrap();
    assert!(page["total_nodes"].as_u64().unwrap() > 64);
    assert_eq!(page["nodes"].as_array().unwrap().len(), 64);
    assert!(
        !page["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["id"] == node)
    );
    let detail = live(&mut session, json!({"scope":node.clone(), "detail":true})).unwrap();
    assert_eq!(detail["scope"], detail["node"]);
    assert_eq!(detail["node"], node);
    assert_eq!(detail["detail"]["kind"], "unavailable");
}

#[test]
fn pipeline_edit_moves_exact_duplicate_stage_and_undoes_each_live_request() {
    let (mut session, clip) = staged_session();
    let authored = session.midi_clip(clip).unwrap().notes.clone();
    let (revision, node) = clip_revision_and_stage(&mut session, clip);
    edit(
        &mut session,
        json!({"node":node,"expected_revision":revision,"action":"move_earlier"}),
    )
    .unwrap();
    assert!(matches!(
        session.clip_transforms(clip).unwrap()[0],
        NoteTransform::Humanize { seed: 22, .. }
    ));
    assert!(matches!(
        session.clip_transforms(clip).unwrap()[1],
        NoteTransform::Humanize { seed: 11, .. }
    ));
    let (revision, _) = clip_revision_and_stage(&mut session, clip);
    edit(
        &mut session,
        json!({"node":format!("clip:{}/stage:1",clip.0),"expected_revision":revision,"action":"remove"}),
    )
    .unwrap();
    assert_eq!(session.clip_transforms(clip).unwrap().len(), 2);
    session.undo();
    session.undo();
    assert!(matches!(
        session.clip_transforms(clip).unwrap()[0],
        NoteTransform::Humanize { seed: 11, .. }
    ));
    assert!(matches!(
        session.clip_transforms(clip).unwrap()[1],
        NoteTransform::Humanize { seed: 22, .. }
    ));
    assert!(matches!(
        session.clip_transforms(clip).unwrap()[2],
        NoteTransform::Gate { .. }
    ));
    assert_eq!(session.midi_clip(clip).unwrap().notes, authored);
}

#[test]
fn pipeline_edit_saved_live_parity_and_checkpoint_only_on_change() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("PipelineEdit.auris");
    let (mut session, clip) = staged_session();
    session.save(&path).unwrap();
    let (revision, node) = clip_revision_and_stage(&mut session, clip);
    let args = json!({"node":node,"expected_revision":revision,"action":"move_later"});
    let live_result = edit(&mut session, args.clone()).unwrap();
    let mut saved_args = args;
    saved_args["project"] = json!(path);
    let saved_result: Value = serde_json::from_str(
        &crate::runtime::call_saved(
            &crate::ToolGroups::parse("mix").unwrap(),
            "pipeline_edit",
            saved_args,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(live_result["changed"], true);
    assert_eq!(saved_result["changed"], true);
    assert_eq!(saved_result["node"], live_result["node"]);
    assert_eq!(saved_result["revision"], live_result["revision"]);
    let saved = crate::opened(path.to_str().unwrap()).unwrap();
    assert_eq!(saved.project(), session.project());
    assert!(
        !crate::opened(path.to_str().unwrap())
            .unwrap()
            .checkpoints()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pipeline_edit_rejects_stale_or_invalid_targets_without_writes_and_boundary_is_noop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("PipelineEditNoop.auris");
    let (mut session, clip) = staged_session();
    session.save(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let revision = clip_revision_and_stage(&mut session, clip).0;
    assert!(edit(&mut session, json!({"node":format!("clip:{}/stage:0",clip.0),"expected_revision":"stale","action":"remove"})).is_err());
    assert!(
        edit(
            &mut session,
            json!({"node":"track:1","expected_revision":revision,"action":"remove"})
        )
        .is_err()
    );
    let mut stale_args = json!({"node":format!("clip:{}/stage:0",clip.0),"expected_revision":"stale","action":"remove","project":path});
    assert!(
        crate::runtime::call_saved(
            &crate::ToolGroups::parse("mix").unwrap(),
            "pipeline_edit",
            stale_args.take()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(
        crate::opened(path.to_str().unwrap())
            .unwrap()
            .checkpoints()
            .unwrap()
            .is_empty()
    );

    for node in ["track:1".to_owned(), format!("clip:{}/stage:999", clip.0)] {
        assert!(
            crate::runtime::call_saved(
                &crate::ToolGroups::parse("mix").unwrap(),
                "pipeline_edit",
                json!({"node":node,"expected_revision":revision,"action":"remove","project":path})
            )
            .is_err()
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(
        crate::opened(path.to_str().unwrap())
            .unwrap()
            .checkpoints()
            .unwrap()
            .is_empty()
    );

    let mut args = json!({"node":format!("clip:{}/stage:0",clip.0),"expected_revision":revision,"action":"move_earlier","project":path});
    let result = crate::runtime::call_saved(
        &crate::ToolGroups::parse("mix").unwrap(),
        "pipeline_edit",
        args.take(),
    )
    .unwrap();
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result["changed"], false);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(
        crate::opened(path.to_str().unwrap())
            .unwrap()
            .checkpoints()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pipeline_continuation_requires_a_revision_and_rejects_stale_saved_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Revision.auris");
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    session.add_bus_track("A");
    session.save(&path).unwrap();
    let first = live(&mut session, json!({"limit":1})).unwrap();
    assert!(live(&mut session, json!({"offset":1,"limit":1})).is_err());
    session.add_bus_track("B");
    session.save(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let args = json!({"offset":1,"limit":1,"expected_revision":first["revision"]});
    let live_error = live(&mut session, args.clone()).unwrap_err();
    let diagnostic: Value = serde_json::from_str(&live_error).unwrap();
    assert_eq!(diagnostic["code"], "revision_mismatch");
    assert!(diagnostic["message"].as_str().unwrap().contains("restart"));
    let mut saved_args = args;
    saved_args["project"] = json!(path);
    let saved_error = crate::runtime::call_saved(
        &crate::ToolGroups::parse("mix").unwrap(),
        "pipeline_read",
        saved_args,
    )
    .unwrap_err();
    assert_eq!(saved_error, live_error);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(live(&mut session, json!({"limit":1})).is_ok());
}

#[test]
fn pipeline_help_and_discovery_use_the_execution_schema() {
    let definitions = crate::live_agent::definitions();
    let definition = definitions
        .iter()
        .find(|tool| tool.name == "pipeline_read")
        .unwrap();
    assert_eq!(crate::tool_group(&definition.name), "mix");
    assert!(definition.parameters["properties"]["scope"].is_object());
    assert!(crate::live_agent::command("pipeline_read", &json!({"unknown":true})).is_err());
    assert!(
        crate::live_agent::command("pipeline_read", &json!({"scope":"track:1"}))
            .unwrap()
            .is_some()
    );
    let help: Value = serde_json::from_str(
        &crate::runtime::call_live_reference("tool_help", json!({"name":"pipeline_read"}))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(help["name"], "pipeline_read");
    let discover: Value = serde_json::from_str(
        &crate::runtime::call_live_reference(
            "discover_tools",
            json!({"group":"mix","query":"pipeline_read","offset":0}),
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    assert!(
        discover["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "pipeline_read")
    );
}
