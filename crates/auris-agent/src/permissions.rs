//! Every rig tool crosses the same fail-closed permission gate.
use super::*;
use auris_session::agent_policy::{Decision, Operation};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub(super) async fn authorize(
    bridge: Option<&Bridge>,
    tool: &str,
    args: &str,
) -> Result<(), String> {
    let correction = |error| {
        format!(
            "{error}. Check the tool schema for {tool} and correct its argument fields before trying again. Do not repeat unchanged arguments."
        )
    };
    let args: serde_json::Value =
        serde_json::from_str(args).map_err(|error| correction(error.to_string()))?;
    // Flat provider calls are approved as canonical commands, preserving saved rules and
    // the host's exact-command/revision permit checks.
    let normalized = toolbox::live_agent::command(tool, &args).map_err(correction)?;
    let (tool, args) = match normalized {
        Some(command) => ("edit_project", serde_json::json!({"command":command})),
        None => (tool, args),
    };
    let operation = Operation::parse(tool, &args).map_err(correction)?;
    let Some(bridge) = bridge else {
        return match auris_session::agent_policy::Policy::default().decide(&operation) {
            Decision::Allow if !operation.mutating => Ok(()),
            Decision::Deny(reason) => Err(reason),
            _ => Err("This operation needs the desktop Agent Panel.".into()),
        };
    };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let response = bridge
        .exchange(serde_json::json!({"event":"permission", "id":id, "tool":tool, "args":args}))
        .await?;
    if response["event"] != "permission_result" || response["id"] != id {
        return Err("The permission response did not match the pending request.".into());
    }
    if response["ok"] == true {
        Ok(())
    } else {
        Err(response["reason"]
            .as_str()
            .unwrap_or("The user denied this operation. Do not retry it.")
            .into())
    }
}
