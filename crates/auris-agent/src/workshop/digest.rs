//! Background summaries that keep a long song conversation and a long preference history
//! within the director's context.
//!
//! Both run on the configured model as tool-free text requests, started only while the director
//! is idle, and both are applied only against the exact state they were written from: a
//! conversation summary names the prefix it replaces, a preference summary the choices it folds.
//! A failed summary changes nothing and is not retried until there is more to summarize.

use auris_session::AgentPreferences;
use auris_session::workshop::{Document, Preferences, Selection};
use serde_json::json;
use std::time::Duration;

use crate::completion::{self, CompletionJob};

const TIMEOUT: Duration = Duration::from_secs(120);

/// A conversation summary in flight, replacing `messages[from..to]` plus the earlier summary.
pub(super) struct ConversationDigest {
    from: usize,
    to: usize,
    job: CompletionJob,
}

impl ConversationDigest {
    /// Start summarizing when the document's model-facing tail has outgrown its budget.
    pub(super) fn start(prefs: &AgentPreferences, document: &Document) -> Option<Self> {
        let to = document.digest_due()?;
        let from = document.digested.min(to);
        let input = json!({
            "earlier_summary": document.digest,
            "messages": document.messages[from..to].iter().map(|m| json!({
                "role": if m.user { "user" } else { "director" },
                "text": m.text,
                "references": m.references.iter().map(|r| &r.text).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        let job = completion::start(
            prefs.clone(),
            auris_toolbox::workshop::CONVERSATION_DIGEST.into(),
            input.to_string(),
            2048,
            TIMEOUT,
        )
        .map_err(|e| log::warn!("conversation summary not started: {e}"))
        .ok()?;
        Some(Self { from, to, job })
    }

    /// The summarized range that this job would cover; used to avoid retrying a failed one.
    pub(super) fn to(&self) -> usize {
        self.to
    }

    /// Apply a finished summary; `None` while it is still running.
    pub(super) fn poll(&self, document: &mut Document) -> Option<Result<(), String>> {
        let text = self.job.poll()?;
        Some(text.and_then(|text| document.apply_digest(self.from, self.to, &text)))
    }
}

/// A preference summary in flight, folding exactly `folded` into the earlier summary.
pub(super) struct PreferenceDigest {
    folded: Vec<Selection>,
    job: CompletionJob,
}

impl PreferenceDigest {
    /// Start summarizing when the stored choices have outgrown their budget.
    pub(super) fn start(prefs: &AgentPreferences, preferences: &Preferences) -> Option<Self> {
        let folded = preferences.digest_due()?.to_vec();
        let input = json!({
            "earlier_summary": preferences.digest,
            "choices": folded.iter().map(|s| json!({
                "request": s.context,
                "chosen": s.chosen,
                "alternatives": s.alternatives.iter()
                    .map(|d| json!({"title": d.title, "description": d.description}))
                    .collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        let job = completion::start(
            prefs.clone(),
            auris_toolbox::workshop::PREFERENCE_DIGEST.into(),
            input.to_string(),
            1024,
            TIMEOUT,
        )
        .map_err(|e| log::warn!("preference summary not started: {e}"))
        .ok()?;
        Some(Self { folded, job })
    }

    /// How many choices were stored when this summary was requested.
    pub(super) fn folded(&self) -> usize {
        self.folded.len()
    }

    /// Store a finished summary under the preference lock; `None` while it is still running.
    pub(super) fn poll(&self) -> Option<Result<Preferences, String>> {
        let text = self.job.poll()?;
        Some(text.and_then(|text| auris_session::workshop::digest_preferences(&self.folded, &text)))
    }
}
