//! The projection has one contract across live and saved execution.

use auris_session::{Session, SessionOptions, prelude::*};
use serde_json::{Value, json};

fn live(session: &mut Session, args: Value) -> Result<Value, String> {
    let command = crate::live_agent::command("pipeline_read", &args)?
        .ok_or("pipeline_read must be a canonical session command")?;
    assert!(!command.is_mutating());
    assert!(!command.needs_worker());
    let text = session.agent_command(command)?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
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
