//! Per-request Auris tool allowance, independent of transport and failed-call recovery.

const PREFIX: &str = "Auris tool call limit reached (";

/// Recover the limit from the stable diagnostic stored in transcripts and candidates.
/// Unknown errors are left to their existing presentation path.
pub fn limit_from_error(error: &str) -> Option<usize> {
    error.strip_prefix(PREFIX)?.strip_suffix(").")?.parse().ok()
}

pub(super) fn limit_error(limit: usize) -> String {
    format!("{PREFIX}{limit}).")
}

pub(super) struct Budget {
    limit: usize,
    calls: usize,
    exhausted: bool,
}

impl Budget {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            limit,
            calls: 0,
            exhausted: false,
        }
    }

    pub(super) fn reset(&mut self) {
        self.calls = 0;
        self.exhausted = false;
    }

    pub(super) fn reserve(&mut self) -> Result<(), String> {
        if self.calls >= self.limit {
            self.exhausted = true;
            return Err(limit_error(self.limit));
        }
        self.calls += 1;
        Ok(())
    }

    pub(super) fn error(&self) -> Option<String> {
        self.exhausted.then(|| limit_error(self.limit))
    }

    /// Calls left, once few enough that a model should plan around them: a tenth of the
    /// allowance, and never fewer than twenty.
    pub(super) fn low(&self) -> Option<usize> {
        let left = self.limit.saturating_sub(self.calls);
        (left <= (self.limit / 10).max(20)).then_some(left)
    }
}

/// `text` with the remaining allowance attached, for a model that cannot see it being spent.
///
/// An object result gains a `tool_budget` field, so a model that parses it still can; anything
/// else, an error included, gets a trailing line.
pub(super) fn with_remaining(text: &str, left: usize) -> String {
    let note = serde_json::json!({"calls_left":left,
        "advice":"Running low: finish with what is left, combine the remaining edits into \
            whole-section calls and skip optional polishing."});
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(mut fields)) => {
            fields.insert("tool_budget".into(), note);
            serde_json::Value::Object(fields).to_string()
        }
        _ => format!("{text}\n\ntool_budget: {note}"),
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(auris_session::AgentPreferences::default().tool_call_limit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_the_exact_allowance_and_resets_only_for_a_new_request() {
        let mut budget = Budget::new(2);
        budget.reserve().unwrap();
        budget.reserve().unwrap();
        assert_eq!(limit_from_error(&budget.reserve().unwrap_err()), Some(2));
        assert!(budget.reserve().is_err());
        budget.reset();
        budget.reserve().unwrap();
    }

    #[test]
    fn the_remaining_allowance_is_reported_only_once_it_runs_low() {
        let mut budget = Budget::new(1000);
        for _ in 0..899 {
            budget.reserve().unwrap();
        }
        assert_eq!(budget.low(), None);
        budget.reserve().unwrap();
        assert_eq!(budget.low(), Some(100));
        // A small allowance still warns twenty calls ahead.
        let mut small = Budget::new(100);
        for _ in 0..79 {
            small.reserve().unwrap();
        }
        assert_eq!(small.low(), None);
        small.reserve().unwrap();
        assert_eq!(small.low(), Some(20));

        let object: serde_json::Value =
            serde_json::from_str(&with_remaining(r#"{"track":1}"#, 5)).unwrap();
        assert_eq!(object["track"], 1);
        assert_eq!(object["tool_budget"]["calls_left"], 5);
        let error = with_remaining("query must contain search words", 3);
        assert!(error.starts_with("query must contain search words\n\ntool_budget: "));
        assert!(error.contains(r#""calls_left":3"#));
    }
}
