//! Bounded recovery for failed Auris tool calls.

use crate::{Bridge, tool_failures::ToolFailures};
use serde_json::json;

/// Per-turn recovery state for one ACP session.
#[derive(Default)]
pub(crate) struct Recovery {
    failures: ToolFailures,
    last_failure: Option<(String, String)>,
    fresh_failure: bool,
    continuations: usize,
    limit: usize,
}

impl Recovery {
    /// Create recovery state with the configured continuation limit.
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    /// Clear the per-turn markers before reading a new ACP turn.
    pub(crate) fn begin_turn(&mut self) {
        self.fresh_failure = false;
    }

    /// Record an ACP tool result. Native tools are deliberately excluded from Auris recovery.
    pub(crate) fn observe(&mut self, tool: &str, args: &str, ok: bool, auris_tool: bool) {
        self.last_failure = if !ok && auris_tool {
            self.failures.record(tool, args, true);
            self.fresh_failure = true;
            Some((tool.to_owned(), args.to_owned()))
        } else {
            self.failures.record(tool, args, false);
            None
        };
    }

    /// Ask the ACP peer to continue after a recoverable Auris tool error.
    ///
    /// Returns the continuation prompt, or `None` when no retry is allowed.
    pub(crate) fn continuation(&mut self, bridge: &Bridge) -> Result<Option<String>, String> {
        if !self.fresh_failure || !bridge.can_recover_tools() {
            return Ok(None);
        }
        let Some((tool, args)) = &self.last_failure else {
            return Ok(None);
        };
        if self.failures.exhausted(tool, args, self.limit)
            || self.continuations >= self.limit.saturating_sub(1)
        {
            return Err(format!(
                "ACP tool recovery reached the configured limit of {}. Earlier edits remain in the document. Send a new instruction to continue.",
                self.limit
            ));
        }
        self.continuations += 1;
        let prompt = "Auris runtime recovery: the last Auris tool call failed. Continue only the unfinished original user request. Read tool_help and inspect the current document as needed, then correct the arguments or choose a valid alternative. Earlier successful edits remain applied: do not blindly replay them. Tool argument errors do not consume the musical polishing budget. Do not bypass any refusal or expand permissions. If user input or an unavailable resource is required, explain that and finish.".to_owned();
        bridge.emit(json!({"event":"notice","message":"ACP ended immediately after a tool error; asking it to correct the call and continue the unfinished request."}));
        Ok(Some(prompt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auris_failure_produces_a_continuation() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mut recovery = Recovery::new(3);
        recovery.observe("add_track", r#"{"kind":1}"#, false, true);
        assert!(recovery.continuation(&bridge).unwrap().is_some());
    }

    #[test]
    fn native_failure_does_not_produce_a_continuation() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mut recovery = Recovery::new(5);
        recovery.observe("Bash", "{}", false, false);
        assert!(recovery.continuation(&bridge).unwrap().is_none());
    }

    #[test]
    fn denial_disables_recovery() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        bridge.block_tool_recovery();
        let mut recovery = Recovery::new(5);
        recovery.observe("add_track", "{}", false, true);
        assert!(recovery.continuation(&bridge).unwrap().is_none());
    }

    #[test]
    fn changing_arguments_cannot_extend_the_global_continuation_limit() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mut recovery = Recovery::new(3);
        for kind in 1..=3 {
            recovery.begin_turn();
            recovery.observe("add_track", &format!(r#"{{"kind":{kind}}}"#), false, true);
            if kind < 3 {
                assert!(recovery.continuation(&bridge).unwrap().is_some());
            } else {
                assert!(
                    recovery
                        .continuation(&bridge)
                        .unwrap_err()
                        .contains("limit of 3")
                );
            }
        }
    }

    #[test]
    fn repeated_normalized_arguments_exhaust_the_tool_budget() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mut recovery = Recovery::new(2);
        recovery.observe("edit", r#"{"b":2,"a":1}"#, false, true);
        assert!(recovery.continuation(&bridge).unwrap().is_some());
        recovery.begin_turn();
        recovery.observe("edit", r#"{"a":1,"b":2}"#, false, true);
        assert!(
            recovery
                .continuation(&bridge)
                .unwrap_err()
                .contains("limit of 2")
        );
    }
}
