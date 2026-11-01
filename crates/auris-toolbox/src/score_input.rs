//! Bounded score input for the shared note editor.

use auris_session::live_agent::{Command, note_editing::NoteEdit};
use std::io::Read;

/// Exact score edits with one explicit, typed input mode.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Content revision returned by read_notes or inspect_project.
    pub expected_revision: String,
    /// Choose inline edits or an absolute local JSON file; never both.
    pub input: Input,
}

/// Mutually exclusive score input, represented completely in every provider schema.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    /// Up to 16 clip edits and 4096 inline note changes.
    Inline {
        /// Typed clip patches or replacements, not JSON strings.
        #[schemars(length(min = 1, max = 16))]
        edits: Vec<NoteEdit>,
    },
    /// Read a local UTF-8 JSON array of clip edits, up to 16 MiB and 65536 changes.
    File {
        /// Absolute local file path, never a provenance description.
        #[schemars(length(min = 1))]
        source_path: String,
    },
}

impl Args {
    /// Local file input requiring host authorization before loading; inline input returns None.
    pub fn source_path(&self) -> Option<&str> {
        match &self.input {
            Input::File { source_path } => Some(source_path),
            Input::Inline { .. } => None,
        }
    }

    /// Load and validate the bounded input before requesting a document edit.
    pub fn command(self) -> Result<Command, String> {
        let edits = match self.input {
            Input::Inline { edits } => {
                if note_count(&edits) > 4096 {
                    return Err(
                        "At most 4096 inline note changes; use source_path for larger scores"
                            .into(),
                    );
                }
                edits
            }
            Input::File {
                source_path: source,
            } => {
                let path = std::path::Path::new(&source);
                if !path.is_absolute() || source.starts_with("\\\\") || source.starts_with("//") {
                    return Err("source_path must be an absolute local file path".into());
                }
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err("source_path must be a regular file".into());
                }
                let mut bytes = Vec::new();
                file.take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err("source_path exceeds 16 MiB".into());
                }
                let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
                serde_json::from_slice(bytes).map_err(|e| format!("source JSON: {e}"))?
            }
        };
        if edits.is_empty() || edits.len() > 16 || note_count(&edits) > 65_536 {
            return Err("Supply 1..16 clip edits and at most 65536 note changes".into());
        }
        Ok(Command::EditNotes {
            expected_revision: self.expected_revision,
            edits,
        })
    }
}

fn note_count(edits: &[NoteEdit]) -> usize {
    edits
        .iter()
        .map(|edit| match edit {
            NoteEdit::Replace { notes, .. } => notes.len(),
            NoteEdit::Patch {
                add,
                remove,
                update,
                ..
            } => add.len() + remove.len() + update.len(),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_input_is_exclusive_and_preserves_note_types_in_provider_schemas() {
        assert!(serde_json::from_value::<Args>(json!({"expected_revision":"r","input":{"kind":"inline","edits":[],"source_path":"missing.json"}})).is_err());
        assert!(
            serde_json::from_value::<Args>(
                json!({"expected_revision":"r","input":{"kind":"file","source":"provenance"}})
            )
            .is_err()
        );
        let mut schema = crate::parameter_schema::<Args>();
        crate::tool_exposure::compact_parameters(&mut schema);
        assert_eq!(schema["properties"]["expected_revision"]["type"], "string");
        let inline = &schema["properties"]["input"]["oneOf"][0];
        assert_eq!(inline["properties"]["edits"]["type"], "array");
        assert!(inline["properties"]["edits"]["items"].is_object());
        assert!(inline["properties"].get("source_path").is_none());
        assert!(schema.get("oneOf").is_none());
    }

    #[test]
    fn local_note_file_uses_the_same_atomic_validator_as_inline_edits() {
        let mut session = crate::headless().unwrap();
        let track = session.add_default_instrument_track("Test").unwrap();
        let clip = session
            .add_midi_clip(
                track,
                "Test",
                auris_session::prelude::Ticks::ZERO,
                auris_session::prelude::Ticks::QUARTER,
            )
            .unwrap();
        let before = session.project().clone();
        let file = tempfile::NamedTempFile::new().unwrap();
        let edits = json!([{"kind":"replace","clip":clip.0,"notes":[{"pitch":60,"bar":1,"beat":1,"length_beats":1,"velocity":94}]}]);
        std::fs::write(file.path(), serde_json::to_vec(&edits).unwrap()).unwrap();
        let request:Args=serde_json::from_value(json!({"expected_revision":session.agent_document_revision().unwrap(),"input":{"kind":"file","source_path":file.path()}})).unwrap();
        assert!(
            session
                .agent_command(request.command().unwrap())
                .unwrap_err()
                .contains("velocity")
        );
        assert_eq!(session.project(), &before);
        assert!(!session.can_redo());
    }
}
