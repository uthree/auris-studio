//! Agent-host reading of the conversation's own earlier turns; the host owns the history.
//!
//! A command-line provider that starts a fresh session is handed only a bounded selection of the
//! conversation, with the omitted turns named by number. This tool returns them.

/// Stable callable name.
pub const NAME: &str = "read_conversation";
/// Shared runtime-specific tool guidance.
pub const DESCRIPTION: &str = "Read earlier turns of this conversation by number, when the context you were handed says turns were omitted or cut. Returns the user's words and your answers, oldest first, a page at a time. Earlier answers describe the document as it was; inspect the project for its current state.";

/// An inclusive range of turn numbers, as the handed-over context numbers them.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// First turn to read, counting from 1.
    #[schemars(range(min = 1))]
    pub from: usize,
    /// Last turn to read, inclusive; omit to read to the newest.
    pub to: Option<usize>,
}
