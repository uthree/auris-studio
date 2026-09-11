//! Flat model-facing operations, translated to the same permission-checked live commands.
use auris_session::live_agent::Command;
use auris_session::prelude::*;
use serde_json::{Value, json};

/// A small, flat tool definition derived from the live command contract.
pub struct Definition {
    /// Provider-facing operation name.
    pub name: String,
    /// Operation-specific instructions.
    pub description: String,
    /// Flat JSON object schema, without action discriminators or nested command objects.
    pub parameters: Value,
}

/// A song starts from a valid arrangement; the model supplies only musical choices.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Song {
    /// Starting arrangement from list_presets. Preserves valid parts, roles and sections.
    preset: String,
    /// Optional song title.
    title: Option<String>,
    /// Optional tempo in BPM, 20 through 300.
    tempo: Option<f64>,
    /// Optional key, for example D minor or C major.
    key: Option<String>,
    /// Optional calm-to-driving energy, 0 through 1.
    energy: Option<f32>,
    /// Optional harmonic tension, 0 through 1.
    tension: Option<f32>,
    /// Optional dark-to-bright character, 0 through 1.
    brightness: Option<f32>,
    /// Make the ending return to the opening for background music. Default false.
    #[serde(default)]
    looped: bool,
    /// Replace existing tracks only when the user explicitly requests replacement. Default false.
    #[serde(default)]
    replace: bool,
}

fn inline(value: &Value, root: &Value) -> Value {
    if let Some(reference) = value.get("$ref").and_then(Value::as_str)
        && let Some(target) = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
    {
        return inline(target, root);
    }
    match value {
        Value::Array(values) => Value::Array(values.iter().map(|v| inline(v, root)).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "$defs" | "$schema"))
                .map(|(key, value)| (key.clone(), inline(value, root)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Small per-action schemas replace the model-facing tagged union. The advanced TOML command
/// remains an internal command, but is not offered to the model.
pub fn definitions() -> Vec<Definition> {
    let root = serde_json::to_value(schemars::schema_for!(Command)).expect("serializable schema");
    let mut tools = Vec::new();
    if let Some(branches) = root["oneOf"].as_array() {
        for branch in branches {
            let Some(action) = branch["properties"]["action"]["const"].as_str() else {
                continue;
            };
            if action == "compose" {
                continue;
            }
            let mut parameters = inline(branch, &root);
            let description = parameters["description"]
                .as_str()
                .unwrap_or(action)
                .to_owned();
            parameters.as_object_mut().unwrap().remove("description");
            parameters["properties"]
                .as_object_mut()
                .unwrap()
                .remove("action");
            if let Some(required) = parameters["required"].as_array_mut() {
                required.retain(|value| value != "action");
            }
            tools.push(Definition {
                name: if action == "inspect" {
                    "inspect_project".into()
                } else {
                    action.into()
                },
                description,
                parameters,
            });
        }
    }
    let root = serde_json::to_value(schemars::schema_for!(Song)).expect("serializable schema");
    let mut parameters = inline(&root, &root);
    parameters["properties"]["preset"]["enum"] =
        json!(PRESETS.iter().map(|p| p.name).collect::<Vec<_>>());
    tools.push(Definition {
        name: "compose_song".into(),
        description: "Compose a complete song into the open document using a preset and a few musical choices. No TOML, part definitions or file paths. Inspect first; do not add empty tracks before composing. Existing tracks require explicit user-requested replacement.".into(),
        parameters,
    });
    tools
}

/// Translate a flat tool into a validated canonical command. `None` identifies a reference tool.
/// Unknown fields and attempts to supply the action discriminator are rejected.
pub fn command(tool: &str, args: &Value) -> Result<Option<Command>, String> {
    if tool == "compose_song" {
        let song: Song = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        let mut spec = preset(&song.preset)
            .ok_or("Unknown preset; choose one from list_presets")?
            .spec();
        if let Some(title) = song.title {
            spec.title = title;
        }
        if let Some(tempo) = song.tempo {
            if !(20.0..=300.0).contains(&tempo) {
                return Err("tempo must be 20..300 BPM".into());
            }
            spec.tempo = tempo;
            for section in spec.sections.values_mut() {
                section.tempo = None;
            }
        }
        if let Some(key) = song.key {
            spec.key = MusicalKey::parse(&key).ok_or("key must look like D minor or C major")?;
        }
        for (name, value, target) in [
            ("energy", song.energy, &mut spec.mood.energy),
            ("tension", song.tension, &mut spec.mood.tension),
            ("brightness", song.brightness, &mut spec.mood.brightness),
        ] {
            if let Some(value) = value {
                if !(0.0..=1.0).contains(&value) {
                    return Err(format!("{name} must be 0..1"));
                }
                *target = value;
            }
        }
        if song.looped {
            spec.ending = Ending::Loop;
        }
        return Ok(Some(Command::Compose {
            spec: Some(spec.to_toml()),
            preset: None,
            replace: song.replace,
        }));
    }
    let action = match tool {
        "inspect_project" => "inspect",
        "read_notes" | "add_track" | "rename_track" | "remove_track" | "set_instrument"
        | "set_level" | "set_track_state" | "add_clip" | "add_note" | "remove_notes" => tool,
        _ => return Ok(None),
    };
    let mut object = args
        .as_object()
        .ok_or("Expected a flat argument object")?
        .clone();
    if object.contains_key("action") || object.contains_key("command") {
        return Err("Pass the fields directly, without action or command wrappers".into());
    }
    object.insert("action".into(), action.into());
    serde_json::from_value(Value::Object(object))
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_catalog_covers_commands_without_a_tagged_union_or_file_destinations() {
        let tools = definitions();
        assert_eq!(tools.len(), 12);
        for tool in tools {
            assert_eq!(tool.parameters["type"], "object");
            assert!(tool.parameters.get("oneOf").is_none());
            assert!(tool.parameters["properties"].get("action").is_none());
            assert!(tool.parameters["properties"].get("command").is_none());
            assert!(tool.parameters["properties"].get("spec").is_none());
            assert!(tool.parameters["properties"].get("path").is_none());
        }
    }
    #[test]
    fn short_orchestral_request_is_valid_loopable_undoable_and_never_replaces_implicitly() {
        let args = json!({"preset":"orchestral", "tempo":144, "key":"D minor", "energy":0.9, "tension":0.8, "looped":true});
        let cmd = command("compose_song", &args).unwrap().unwrap();
        let Command::Compose {
            spec: Some(source), ..
        } = &cmd
        else {
            panic!()
        };
        let spec = SongSpec::parse(source).unwrap();
        assert_eq!(spec.tempo, 144.0);
        assert_eq!(spec.ending, Ending::Loop);
        assert_eq!(spec.key.to_text(), "D minor");
        let mut session = auris_session::Session::new(
            auris_session::SessionOptions::headless().with_balance(false),
        )
        .unwrap();
        let before = session.project().clone();
        session.agent_command(cmd).unwrap();
        assert!(!session.project().tracks.is_empty());
        let composed = session.project().clone();
        assert!(
            session
                .agent_command(command("compose_song", &args).unwrap().unwrap())
                .is_err()
        );
        assert_eq!(session.project(), &composed);
        session.undo();
        assert_eq!(session.project(), &before);
        for bad in [
            json!({"preset":"orchestral", "spec":{}}),
            json!({"preset":"orchestral", "energy":2}),
            json!({"preset":"missing"}),
        ] {
            assert!(command("compose_song", &bad).is_err());
        }
    }
}
