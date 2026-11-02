//! Which of the music linter's findings a project has asked to stop hearing about.

use serde::{Deserialize, Serialize};

use super::{ClipId, TrackId};

/// What a suppression covers.
///
/// Stored with the document, because "the pad holds that minor second on purpose" is a fact about
/// this song, not about the person's installation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LintScope {
    /// Everywhere in the song.
    Song,
    /// One track's findings, and mix-wide findings never.
    Track {
        /// The track.
        track: TrackId,
    },
    /// Findings that involve notes of one clip.
    Clip {
        /// The clip.
        clip: ClipId,
    },
    /// Findings that start inside any section with this label, every time it comes round.
    Section {
        /// The section's label, as the structure lane shows it.
        label: String,
    },
}

/// One "stop telling me about this".
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LintSuppression {
    /// The rule's wire name (`harsh_interval`, `silent_track`, …), or `None` for every rule.
    ///
    /// A name rather than a type: the rules live in `auris-session`, which this crate may not
    /// name, and a name nobody recognises any more simply suppresses nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Where it applies.
    pub scope: LintScope,
    /// Why it was ignored, in the words of whoever ignored it; empty when nobody said.
    ///
    /// The comment habit again, applied to the one place a score most needs one: an ignored
    /// warning with no reason is a warning the next reader un-ignores to find out. Not part of
    /// what the rule *is* — [`Self::same_rule`] compares without it — so retyping the reason
    /// never makes a second rule.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl LintSuppression {
    /// A rule with no reason written yet.
    pub fn new(rule: Option<String>, scope: LintScope) -> Self {
        Self {
            rule,
            scope,
            reason: String::new(),
        }
    }

    /// Whether `other` ignores the same thing in the same place, whatever either says about why.
    pub fn same_rule(&self, other: &Self) -> bool {
        self.rule == other.rule && self.scope == other.scope
    }
}
