//! Idempotent authored-score replacement, including file-backed batches.
use super::*;
use std::io::Read;

/// The tool's wire name.
pub const NAME: &str = "replace_notes";
/// The tool's model-facing description.
pub const DESCRIPTION: &str = "Replaces all authored notes in one clip. Supply exactly one of notes (an array) or source (an absolute path to a UTF-8 JSON array on the MCP server). Use source for script-generated scores instead of printing and copying large arrays into tool calls. Notes use pitch (60, \"60\", or \"C4\"), song-relative 1-based bar and beat, beats for duration, and optional velocity 0-1 (default 0.75). All notes are validated before changing the clip. Identical retries do not duplicate notes or create checkpoints; an empty array clears notes. Preserves clip length, curves, transforms and recipe; regeneration can overwrite authored notes. Maximum 65536 notes and 16 MiB per file. Saves with a checkpoint.";

const MAX_NOTES: usize = 65_536;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const EXAMPLE: &str = r#"Example note: {"pitch":60,"bar":1,"beat":1,"beats":1,"velocity":0.75}"#;

/// Arguments for replacing a clip's complete authored note sequence.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("oneOf" = [
    {"required":["notes"],"properties":{"notes":{"type":"array"},"source":{"type":"null"}}},
    {"required":["source"],"properties":{"source":{"type":"string"},"notes":{"type":"null"}}}
]))]
pub struct Args {
    /// Absolute path of the existing saved .auris project.
    pub project: String,
    /// Track name or stable id:N selector from describe.
    pub track: String,
    /// One-based clip number from describe.
    #[schemars(range(min = 1))]
    pub clip: usize,
    /// Complete note array, including [] to clear; omit when using source.
    #[serde(default, deserialize_with = "deserialize_optional_notes")]
    #[schemars(length(max = 65536))]
    pub notes: Option<Vec<edit_notes::NoteSpec>>,
    /// Absolute path on the MCP server to a UTF-8 JSON array, not a wrapper object.
    /// Omit when using notes. The file is read once and is not retained as a project asset.
    pub source: Option<String>,
}

pub(crate) fn deserialize_optional_notes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<edit_notes::NoteSpec>>, D::Error> {
    use serde::Deserialize;
    Option::<serde_json::Value>::deserialize(d)?
        .map(parse_notes)
        .transpose()
        .map_err(serde::de::Error::custom)
}

fn parse_notes(value: serde_json::Value) -> Result<Vec<edit_notes::NoteSpec>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("notes must be a JSON array. {EXAMPLE}"))?;
    if values.len() > MAX_NOTES {
        return Err(format!("notes must contain at most {MAX_NOTES} entries"));
    }
    let schema = parameter_schema::<edit_notes::NoteSpec>();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            serde_json::from_value(value.clone()).map_err(|error| {
                let path = format!("notes[{index}]");
                let hint = super::live_agent::argument_hint(&schema, value, &path).unwrap_or(path);
                format!("{hint}: {error}. {EXAMPLE}")
            })
        })
        .collect()
}

fn read_notes(source: &str) -> Result<Vec<edit_notes::NoteSpec>, String> {
    let path = std::path::Path::new(source);
    if !path.is_absolute() {
        return Err("source must be an absolute path on the MCP server".into());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("source: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("source must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("source: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("source exceeds 16 MiB".into());
    }
    // Windows editors commonly emit a UTF-8 BOM.
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let value = serde_json::from_slice(bytes)
        .map_err(|e| format!("source JSON: {e}. Expected an array. {EXAMPLE}"))?;
    parse_notes(value)
}

/// Validate, replace and checkpoint; never save partial input or unchanged retries.
pub fn run(args: &Args) -> Result<String, String> {
    let loaded;
    let notes = match (&args.notes, &args.source) {
        (Some(notes), None) => notes.as_slice(),
        (None, Some(source)) => {
            loaded = read_notes(source)?;
            &loaded
        }
        _ => {
            return Err(
                "Supply exactly one of notes or source; [] explicitly clears the clip".into(),
            );
        }
    };
    if notes.len() > MAX_NOTES {
        return Err(format!("notes must contain at most {MAX_NOTES} entries"));
    }
    let mut session = opened(&args.project)?;
    let track = track_by_name(session.project(), &args.track)?.id;
    let (id, clip) = clip_by_number(session.project(), track, args.clip)?;
    let placed = edit_notes::prepare(&session, clip, notes)?;
    if clip.notes == placed {
        return Ok(format!(
            "Unchanged: clip already holds these {} notes; no checkpoint created.",
            notes.len()
        ));
    }
    session
        .replace_notes(id, placed)
        .map_err(|e| e.to_string())?;
    session.save_with_checkpoint().map_err(|e| e.to_string())?;
    Ok(format!(
        "Replaced authored notes: clip now holds {} notes. Saved.",
        notes.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_and_inline_replacement_are_atomic_and_retryable() {
        let root = tempfile::tempdir().unwrap();
        let mut session = headless().unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::QUARTER * 4, Ticks::QUARTER * 8)
            .unwrap();
        session.save_as(&root.path().join("Song.auris")).unwrap();
        let project = session.path().unwrap().to_str().unwrap().to_owned();
        let source = root.path().join("notes.json");
        let batch = json!([
            {"pitch":36,"bar":2,"beat":1,"beats":0.5},
            {"pitch":"C4","bar":2,"beat":2,"beats":0.5},
            {"pitch":"64","bar":3,"beat":1,"beats":1}
        ]);
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend(serde_json::to_vec(&batch).unwrap());
        std::fs::write(&source, bytes).unwrap();
        let file_args: Args = serde_json::from_value(
            json!({"project":project,"track":"Lead","clip":1,"source":source}),
        )
        .unwrap();
        run(&file_args).unwrap();
        let saved = std::fs::read(&project).unwrap();
        let opened = opened(&project).unwrap();
        let actual = &opened.project().midi_clip(clip).unwrap().1.notes;
        assert_eq!(
            actual.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![36, 60, 64]
        );
        assert_eq!(actual[0].start, Ticks::ZERO);
        assert_eq!(actual[2].start, Ticks::QUARTER * 4);
        assert!(run(&file_args).unwrap().starts_with("Unchanged"));
        let inline: Args = serde_json::from_value(
            json!({"project":project,"track":"Lead","clip":1,"notes":batch}),
        )
        .unwrap();
        assert!(run(&inline).unwrap().starts_with("Unchanged"));
        assert_eq!(std::fs::read(&project).unwrap(), saved);

        for bad in [
            json!([{"pitch":60,"bar":2,"beat":1,"beats":1},{"pitch":61,"bar":3,"beat":4,"beats":2}]),
            json!([{"pitch":60,"bar":2,"beat":1,"length":1}]),
            json!([{"pitch":60,"bar":2,"beat":1,"beats":0.00000001}]),
            json!([{"pitch":128,"bar":2,"beat":1,"beats":1}]),
        ] {
            std::fs::write(&source, serde_json::to_vec(&bad).unwrap()).unwrap();
            let error = run(&file_args).unwrap_err();
            assert!(
                error.contains("notes[") && error.contains("Example"),
                "{error}"
            );
            assert_eq!(std::fs::read(&project).unwrap(), saved);
        }
        for invalid in [b"[".as_slice(), b"{\"notes\":[]}", b"[] []"] {
            std::fs::write(&source, invalid).unwrap();
            assert!(run(&file_args).is_err());
            assert_eq!(std::fs::read(&project).unwrap(), saved);
        }
        std::fs::write(&source, b"[]").unwrap();
        run(&file_args).unwrap();
        assert!(
            super::opened(&project)
                .unwrap()
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
        assert!(run(&file_args).unwrap().starts_with("Unchanged"));
    }

    #[test]
    fn source_limits_and_exclusive_inputs_are_enforced() {
        assert!(
            read_notes("relative.json")
                .unwrap_err()
                .contains("absolute")
        );
        let root = tempfile::tempdir().unwrap();
        assert!(read_notes(root.path().to_str().unwrap()).is_err());
        let path = root.path().join("large.json");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(
            read_notes(path.to_str().unwrap())
                .unwrap_err()
                .contains("16 MiB")
        );
        assert!(
            parse_notes(json!(vec![json!({}); MAX_NOTES + 1]))
                .unwrap_err()
                .contains("65536")
        );
        for value in [json!({}), json!({"notes":[],"source":"/notes.json"})] {
            let mut value = value;
            value["project"] = json!("/song.auris");
            value["track"] = json!("Lead");
            value["clip"] = json!(1);
            let args: Args = serde_json::from_value(value).unwrap();
            assert!(run(&args).unwrap_err().contains("exactly one"));
        }
        let error = serde_json::from_value::<edit_notes::Args>(json!({"project":"/song.auris","track":"Lead","clip":1,"add":[{"pitch":60,"bar":1,"beat":1,"length":1}]})).unwrap_err().to_string();
        assert!(
            error.contains("notes[0].beats") && error.contains("Example"),
            "{error}"
        );
    }
}
