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
    /// enable_effect, set_strip, set_outputs, add_output, set_output, set_sidechain, parameters
    /// or set_parameter.
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
        let operation = json!({"action":"connect","source":format!("track:{}",a.0),"target":format!("track:{}",b.0),"kind":"output"});
        let live = session
            .agent_command(auris_session::live_agent::Command::MixerGraph {
                operation: serde_json::from_value(operation.clone()).unwrap(),
            })
            .unwrap();
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
            opened(&project).unwrap().project().outputs(a),
            vec![Output::master(), Output::to_bus(b)]
        );
        for target in ["track:master".to_string(), format!("track:{}", b.0)] {
            request(json!({"action":"disconnect","source":format!("track:{}",a.0),"target":target,"kind":"output"})).unwrap();
        }
        assert!(opened(&project).unwrap().project().outputs(a).is_empty());
    }
}
