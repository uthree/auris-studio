//! Flat model-facing operations, translated to the same permission-checked live commands.
use auris_session::live_agent::Command;
use serde_json::Value;

/// A small, flat tool definition derived from the live command contract.
pub struct Definition {
    /// Provider-facing operation name.
    pub name: String,
    /// Operation-specific instructions.
    pub description: String,
    /// Flat JSON object schema, without action discriminators or nested command objects.
    pub parameters: Value,
}

/// List exact live instrument identifiers without saved-project-only arguments.
pub fn instruments() -> String {
    match super::headless() {
        Ok(session) => {
            let mut lines =
                vec!["Instrument IDs for set_instrument's instrument field:".to_string()];
            lines.extend(
                session
                    .registry()
                    .instruments()
                    .map(|instrument| format!("{}: {}", instrument.id, instrument.name)),
            );
            lines.join("\n")
        }
        Err(error) => format!("Cannot list instruments: {error}"),
    }
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
    tools
}

/// Translate a flat tool into a validated canonical command. `None` identifies a reference tool.
/// Unknown fields and attempts to supply the action discriminator are rejected.
pub fn command(tool: &str, args: &Value) -> Result<Option<Command>, String> {
    let action = match tool {
        "inspect_project" => "inspect",
        "list_instruments" | "inspect_audio" | "read_notes" | "add_track" | "rename_track"
        | "remove_track" | "set_instrument" | "add_notes" | "set_tempo" | "set_loop"
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
        assert_eq!(tools.len(), 16);
        assert!(
            command("compose_song", &serde_json::json!({}))
                .unwrap()
                .is_none()
        );
        for tool in tools {
            assert_eq!(tool.parameters["type"], "object");
            assert!(tool.parameters.get("oneOf").is_none());
            assert!(tool.parameters["properties"].get("action").is_none());
            assert!(tool.parameters["properties"].get("command").is_none());
            assert!(tool.parameters["properties"].get("spec").is_none());
            assert!(tool.parameters["properties"].get("path").is_none());
        }
    }
}
