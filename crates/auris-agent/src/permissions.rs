//! Every rig tool crosses the same fail-closed permission gate.
use super::*;
use auris_session::agent_policy::{Decision, Operation};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub(super) async fn authorize(tool: &str, args: &str) -> Result<(), String> {
    let correction = |error| {
        format!(
            "{error}. Check the tool schema for {tool} and correct its argument fields before trying again. Do not repeat unchanged arguments."
        )
    };
    let args: serde_json::Value =
        serde_json::from_str(args).map_err(|error| correction(error.to_string()))?;
    let operation = Operation::parse(tool, &args).map_err(correction)?;
    if std::env::var_os("AURIS_AGENT_LIVE_SESSION").is_none() {
        return match auris_session::Settings::load()
            .agent
            .policy
            .decide(&operation)
        {
            Decision::Allow => Ok(()),
            Decision::Deny(reason) => Err(reason),
            Decision::Ask => {
                Err("This operation needs confirmation in the desktop Agent Panel.".into())
            }
        };
    }
    let _guard = LIVE_REQUEST.lock().await;
    let tool = tool.to_string();
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    tokio::task::spawn_blocking(move || {
        let mut output = std::io::stdout().lock();
        let request = serde_json::json!({"event":"permission", "id":id, "tool":tool, "args":args});
        writeln!(output, "{request}")
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())?;
        drop(output);
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let response: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
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
    })
    .await
    .map_err(|e| e.to_string())?
}
