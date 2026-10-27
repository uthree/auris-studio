//! The same DAG operations for saved projects that the live agent uses in memory.

use super::*;
use auris_session::mixer_graph::commands::GraphAction;

/// The tool's wire name.
pub const NAME: &str = "mixer_graph";
/// The model-facing description shared with the MCP registration.
pub const DESCRIPTION: &str = "Reads or edits the mixer DAG using stable track/effect node IDs. Start with operation {\"action\":\"read\"}; follow next_offset for more nodes and outgoing edges. Track nodes contain fader controls; inserts still process before the fader. Connect output/send to bus track nodes (output also accepts track:master), or sidechain to an effect. All cycles are rejected before mutation. Parallel effects use bus branches. Set track fader/pan/mute/solo, add/remove/move/enable effects, read/set effect parameters, and set existing send levels/taps with operation objects. Reordering preserves node IDs. Output connections add unity post-fader branches, preserving other destinations. Disconnect removes only that edge; removing all outputs leaves the track disconnected. Changed operations are checkpointed and saved; reads and unchanged retries do not save.";

/// A saved project and one shared graph operation.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Project handle or absolute .auris document path.
    pub project: String,
    /// Object with action read, connect, disconnect, add_effect, remove_effect, move_effect,
    /// enable_effect, set_strip, set_send, parameters or set_parameter.
    pub operation: GraphAction,
}

/// Apply the shared session operation and persist only actual document changes.
pub fn run(args: &Args) -> Result<String, String> {
    let mut session = opened(&args.project)?;
    let mut report = session.apply_mixer_graph(&args.operation)?;
    if report["changed"] == true {
        save_checkpointed(&mut session)?;
        report["saved"] = true.into();
    }
    Ok(report.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn file_and_live_tools_share_the_graph_and_failed_calls_do_not_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Song.auris");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        session.save(&path).unwrap();
        let project = path.to_string_lossy().into_owned();
        let request = |operation: Value| -> Result<Value, String> {
            run(&serde_json::from_value(json!({"project":project,"operation":operation})).unwrap())
                .map(|reply| serde_json::from_str(&reply).unwrap())
        };
        let before = std::fs::read(&path).unwrap();
        request(json!({"action":"read"})).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(opened(&project).unwrap().checkpoints().unwrap().is_empty());
        let operation = json!({"action":"connect","source":format!("track:{}",a.0),"target":format!("track:{}",b.0),"kind":"send"});
        let live_before = session.project().clone();
        let live = session
            .agent_command(auris_session::live_agent::Command::MixerGraph {
                operation: serde_json::from_value(operation.clone()).unwrap(),
            })
            .unwrap();
        let live_after = session.project().clone();
        session.undo();
        assert_eq!(session.project(), &live_before);
        session.redo();
        assert_eq!(session.project(), &live_after);
        let file = request(operation.clone()).unwrap();
        assert_eq!(
            file["graph"],
            serde_json::from_str::<Value>(&live).unwrap()["graph"]
        );
        assert_eq!(file["saved"], true);
        let checkpoints = opened(&project).unwrap().checkpoints().unwrap().len();
        let saved = std::fs::read(&path).unwrap();
        assert_eq!(request(operation).unwrap()["changed"], false);
        assert!(request(json!({"action":"connect","source":format!("track:{}",b.0),"target":format!("track:{}",a.0),"kind":"output"})).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert_eq!(
            opened(&project).unwrap().checkpoints().unwrap().len(),
            checkpoints
        );
        request(json!({"action":"connect","source":format!("track:{}",a.0),"target":format!("track:{}",b.0),"kind":"output"})).unwrap();
        assert_eq!(
            opened(&project)
                .unwrap()
                .project()
                .track(a)
                .unwrap()
                .outputs,
            vec![Output::Master, Output::Bus(b)]
        );
        for target in ["track:master".to_string(), format!("track:{}", b.0)] {
            request(json!({"action":"disconnect","source":format!("track:{}",a.0),"target":target,"kind":"output"})).unwrap();
        }
        assert!(
            opened(&project)
                .unwrap()
                .project()
                .track(a)
                .unwrap()
                .outputs
                .is_empty()
        );
    }

    #[test]
    fn saved_workflow_replay_measures_graph_automation_and_performance_contracts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Workflow.auris");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = session.add_bus_track("Source");
        let destination = session.add_bus_track("Destination");
        let performer = session.add_default_instrument_track("Performer").unwrap();
        let clip = session
            .add_midi_clip(
                performer,
                "Phrase",
                auris_session::prelude::Ticks::ZERO,
                auris_session::prelude::Ticks(3840),
            )
            .unwrap();
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                    Note::new(64, Ticks::QUARTER, Ticks::QUARTER),
                ],
            )
            .unwrap();
        session.save(&path).unwrap();
        let project = path.to_string_lossy().into_owned();
        let mut call_count = 0usize;
        let mut request_bytes_total = 0usize;
        let mut response_bytes_total = 0usize;
        let mut error_count = 0usize;
        let mut call = |name: &str, args: Value| -> Result<(Value, usize), String> {
            let bytes = serde_json::to_vec(&args).unwrap().len();
            call_count += 1;
            request_bytes_total += bytes;
            let reply = match crate::runtime::call_saved(&crate::ToolGroups::default(), name, args)
            {
                Ok(reply) => reply,
                Err(error) => {
                    error_count += 1;
                    response_bytes_total += error.len();
                    return Err(error);
                }
            };
            response_bytes_total += reply.len();
            Ok((
                serde_json::from_str(&reply).unwrap_or_else(|_| Value::String(reply.clone())),
                bytes,
            ))
        };

        let before_read = std::fs::read(&path).unwrap();
        let (read, read_bytes) = call(
            "mixer_graph",
            json!({"project":project,"operation":{"action":"read","offset":0,"limit":64}}),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before_read);
        assert_eq!(opened(&project).unwrap().checkpoints().unwrap().len(), 0);
        let nodes = read["result"]["nodes"]
            .as_array()
            .or_else(|| read["nodes"].as_array())
            .unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node["id"] == format!("track:{}", source.0))
        );
        assert!(
            nodes
                .iter()
                .all(|node| node["id"].as_str().unwrap().starts_with("track:")
                    || node["id"].as_str().unwrap().starts_with("effect:"))
        );

        let send_request = json!({"project":project,"operation":{"action":"connect","source":format!("track:{}",source.0),"target":format!("track:{}",destination.0),"kind":"send"}});
        let (connected, connect_bytes) = call("mixer_graph", send_request.clone()).unwrap();
        assert_eq!(connected["changed"], true);
        let saved_after_connect = std::fs::read(&path).unwrap();
        let checkpoints_after_connect = opened(&project).unwrap().checkpoints().unwrap().len();
        let (duplicate, _) = call("mixer_graph", send_request).unwrap();
        assert_eq!(duplicate["changed"], false);
        assert_eq!(std::fs::read(&path).unwrap(), saved_after_connect);
        assert_eq!(
            opened(&project).unwrap().checkpoints().unwrap().len(),
            checkpoints_after_connect
        );
        let saved_before_cycle = std::fs::read(&path).unwrap();
        let checkpoints_before_cycle = opened(&project).unwrap().checkpoints().unwrap().len();
        assert!(call(
            "mixer_graph",
            json!({"project":project,"operation":{"action":"connect","source":format!("track:{}",destination.0),"target":format!("track:{}",source.0),"kind":"output"}}),
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved_before_cycle);
        assert_eq!(
            opened(&project).unwrap().checkpoints().unwrap().len(),
            checkpoints_before_cycle
        );
        let (recovered, _) = call(
            "mixer_graph",
            json!({"project":project,"operation":{"action":"disconnect","source":format!("track:{}",source.0),"target":format!("track:{}",destination.0),"kind":"send"}}),
        )
        .unwrap();
        assert_eq!(recovered["changed"], true);

        let (automation_read, automation_read_bytes) = call(
            "automation",
            json!({"project":project,"target":{"kind":"track_gain","track":performer.0},"operation":{"kind":"read"}}),
        )
        .unwrap();
        let revision = automation_read["revision"].as_str().unwrap().to_owned();
        let (automation_set, automation_set_bytes) = call(
            "automation",
            json!({"project":project,"target":{"kind":"track_gain","track":performer.0},"expected_revision":revision,"operation":{"kind":"set","points":[{"at_tick":0,"value":-12.0}],"replace":false,"curve":"linear"}}),
        )
        .unwrap();
        assert_eq!(automation_set["automated"], true);

        let (performance, performance_bytes) = call(
            "set_performance",
            json!({"project":project,"clip":clip.0,"settings":{"gate":0.5}}),
        )
        .unwrap();
        assert!(performance["clip"].is_number() || performance["settings"].is_object());
        let (performance_read, performance_read_bytes) = call(
            "inspect_performance",
            json!({"project":project,"clip":clip.0}),
        )
        .unwrap();
        assert_eq!(performance_read["settings"]["gate"], 0.5);
        let (notes_after, _) = call(
            "read_notes",
            json!({"project":project,"clips":[{"clip":clip.0}]}),
        )
        .unwrap();
        assert_eq!(
            notes_after["clips"][0]["notes"].as_array().unwrap().len(),
            2
        );
        assert_eq!(notes_after["clips"][0]["notes"][0]["pitch"], 60);
        assert_eq!(notes_after["clips"][0]["notes"][1]["pitch"], 64);

        println!(
            "workflow-baseline bytes: read request={}, read response={}, connect request={}, connect response={}, automation read request={}, automation set request={}, performance request={}, performance read request={}",
            read_bytes,
            serde_json::to_vec(&read).unwrap().len(),
            connect_bytes,
            serde_json::to_vec(&connected).unwrap().len(),
            automation_read_bytes,
            automation_set_bytes,
            performance_bytes,
            performance_read_bytes,
        );
        println!(
            "workflow-baseline calls: count={}, errors={}, request_bytes_total={}, response_bytes_total={}",
            call_count, error_count, request_bytes_total, response_bytes_total
        );
    }
}
