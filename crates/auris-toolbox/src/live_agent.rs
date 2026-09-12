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

/// Small per-action schemas replace the model-facing tagged union. The advanced TOML command
/// remains an internal command, but is not offered to the model.
pub fn definitions() -> Vec<Definition> {
    // Keep enum simplification, inlining and numeric annotations identical to the MCP catalog.
    let root = super::parameter_schema::<Command>();
    let mut tools = Vec::new();
    if let Some(branches) = root["oneOf"].as_array() {
        for branch in branches {
            let Some(action) = branch["properties"]["action"]["const"].as_str() else {
                continue;
            };
            if action == "compose" {
                continue;
            }
            let mut parameters = branch.clone();
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
        | "set_level" | "set_track_state" | "add_clip" | "add_note" | "remove_notes"
        | "replace_notes" => tool,
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
        .map_err(|error| {
            let Some(definition) = definitions().into_iter().find(|item| item.name == tool) else {
                return error.to_string();
            };
            let hint = argument_hint(&definition.parameters, args, "arguments")
                .unwrap_or_else(|| format!("Expected arguments: {}", definition.parameters));
            format!("Invalid arguments for {tool}: {hint}. {error}")
        })
}

// Serde's internally tagged enum buffers its fields, losing their paths on failure. These
// schema-derived hints run only after deserialization fails; they are not a schema validator
// or an alternative acceptance gate. Serde and the session remain authoritative.
pub(crate) fn argument_hint(schema: &Value, value: &Value, path: &str) -> Option<String> {
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array)
        && branches
            .iter()
            .all(|branch| argument_hint(branch, value, path).is_some())
    {
        return Some(format!("{path} must match one of {}", schema["anyOf"]));
    }
    if let Some(kind) = schema.get("type") {
        let matches = |kind: &Value| match kind.as_str() {
            Some("string") => value.is_string(),
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("integer") => value.is_u64() || value.is_i64(),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            _ => true,
        };
        let valid = kind
            .as_array()
            .map_or_else(|| matches(kind), |kinds| kinds.iter().any(matches));
        if !valid {
            let choices = schema
                .get("enum")
                .map(|choices| format!("; choose one of {choices}"))
                .unwrap_or_default();
            return Some(format!("{path} must be {kind}{choices}"));
        }
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return Some(format!("{path} must be one of {}", schema["enum"]));
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Some(format!(
                        "{path}.{field} is required; expected {}",
                        schema["properties"][field]
                    ));
                }
            }
        }
        if let Some(properties) = schema["properties"].as_object() {
            for (field, value) in object {
                match properties.get(field) {
                    Some(field_schema) => {
                        if let Some(hint) =
                            argument_hint(field_schema, value, &format!("{path}.{field}"))
                        {
                            return Some(hint);
                        }
                    }
                    None if schema["additionalProperties"] == false => {
                        return Some(format!(
                            "{path}.{field} is not accepted; allowed fields: {}",
                            properties
                                .keys()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    None => {}
                }
            }
        }
    }
    if let Some(values) = value.as_array()
        && let Some(items) = schema.get("items")
    {
        for (index, value) in values.iter().enumerate() {
            if let Some(hint) = argument_hint(items, value, &format!("{path}[{index}]")) {
                return Some(hint);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_kind_is_a_string_enum_and_array_errors_explain_the_field() {
        let tool = definitions()
            .into_iter()
            .find(|tool| tool.name == "add_track")
            .unwrap();
        assert_eq!(tool.parameters["properties"]["kind"]["type"], "string");
        assert_eq!(
            tool.parameters["properties"]["kind"]["enum"],
            serde_json::json!(["instrument", "drum", "singer", "audio", "bus"])
        );
        assert!(
            command(
                "add_track",
                &serde_json::json!({"name":"Drums", "kind":"drum"})
            )
            .unwrap()
            .is_some()
        );
        let error = command(
            "add_track",
            &serde_json::json!({"name":"Drums", "kind":["drum"]}),
        )
        .unwrap_err();
        assert!(
            error.contains("kind") && error.contains("string") && error.contains("drum"),
            "{error}"
        );
    }

    #[test]
    fn nested_note_errors_identify_the_index_and_expected_type() {
        let error = command("add_notes", &serde_json::json!({"clip":1, "notes":[{"pitch":[60], "start_beat":0, "duration_beats":1, "velocity":0.8}]})).unwrap_err();
        assert!(
            error.contains("notes[0].pitch") && error.contains("integer"),
            "{error}"
        );
    }

    #[test]
    fn live_schemas_publish_runtime_limits() {
        let tools = definitions();
        let schema = |name: &str| {
            &tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap()
                .parameters
        };
        assert_eq!(schema("set_tempo")["properties"]["bpm"]["minimum"], 20.0);
        assert_eq!(schema("set_tempo")["properties"]["bpm"]["maximum"], 300.0);
        assert_eq!(schema("add_notes")["properties"]["notes"]["minItems"], 1);
        assert_eq!(schema("add_notes")["properties"]["notes"]["maxItems"], 256);
        assert_eq!(
            schema("add_notes")["properties"]["notes"]["items"]["properties"]["pitch"]["anyOf"][0]
                ["maximum"],
            127
        );
        assert_eq!(schema("inspect_audio")["properties"]["bars"]["maximum"], 8);
        for tool in ["add_clip", "set_loop"] {
            assert_eq!(schema(tool)["properties"]["start_bar"]["minimum"], 1);
            assert_eq!(schema(tool)["properties"]["bars"]["minimum"], 1);
            assert_eq!(schema(tool)["properties"]["bars"]["maximum"], 1024);
        }
        for (field, min, max) in [("gain_db", -60.0, 12.0), ("pan", -1.0, 1.0)] {
            assert_eq!(schema("set_level")["properties"][field]["minimum"], min);
            assert_eq!(schema("set_level")["properties"][field]["maximum"], max);
        }
        for (note, start, duration) in [
            (&schema("add_note")["properties"], "start", "beats"),
            (
                &schema("add_notes")["properties"]["notes"]["items"]["properties"],
                "start_beat",
                "duration_beats",
            ),
        ] {
            assert_eq!(note["pitch"]["anyOf"][0]["maximum"], 127);
            assert_eq!(note[start]["minimum"], 0.0);
            assert_eq!(note[duration]["exclusiveMinimum"], 0);
            assert_eq!(note["velocity"]["minimum"], 0.0);
            assert_eq!(note["velocity"]["maximum"], 1.0);
        }
    }

    #[test]
    fn every_live_operation_has_a_round_tripping_flat_contract() {
        let fixtures = [
            ("inspect_project", serde_json::json!({})),
            (
                "inspect_audio",
                serde_json::json!({"start_bar":1,"bars":1,"track":null}),
            ),
            (
                "list_instruments",
                serde_json::json!({"query":null,"offset":0,"refresh":false}),
            ),
            ("read_notes", serde_json::json!({"clip":1,"offset":0})),
            (
                "add_track",
                serde_json::json!({"name":"Drums","kind":"drum"}),
            ),
            ("rename_track", serde_json::json!({"track":1,"name":"Bass"})),
            ("remove_track", serde_json::json!({"track":1})),
            (
                "set_instrument",
                serde_json::json!({"track":1,"instrument":"auris.synth.drumkit"}),
            ),
            (
                "set_level",
                serde_json::json!({"track":1,"gain_db":-6.0,"pan":0.0}),
            ),
            (
                "set_track_state",
                serde_json::json!({"track":1,"mute":false,"solo":false}),
            ),
            (
                "add_clip",
                serde_json::json!({"track":1,"name":"Phrase","start_bar":1,"bars":4}),
            ),
            (
                "add_note",
                serde_json::json!({"clip":1,"pitch":60,"start":0.0,"beats":1.0,"velocity":0.5}),
            ),
            (
                "add_notes",
                serde_json::json!({"clip":1,"notes":[{"pitch":60,"start_beat":0.0,"duration_beats":1.0,"velocity":0.5}]}),
            ),
            ("set_tempo", serde_json::json!({"bpm":120.0})),
            ("set_loop", serde_json::json!({"start_bar":1,"bars":4})),
            ("remove_notes", serde_json::json!({"clip":1,"indices":[0]})),
            ("replace_notes", serde_json::json!({"clip":1,"notes":[]})),
        ];
        let tools = definitions();
        assert_eq!(tools.len(), fixtures.len());
        for (name, args) in fixtures {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            let properties = tool.parameters["properties"].as_object().unwrap();
            assert_eq!(
                properties.keys().collect::<Vec<_>>(),
                args.as_object().unwrap().keys().collect::<Vec<_>>(),
                "{name}"
            );
            assert_eq!(tool.parameters["additionalProperties"], false, "{name}");
            assert!(!tool.parameters.to_string().contains("\"$ref\""), "{name}");
            let parsed = command(name, &args).unwrap().unwrap();
            let mut wire = serde_json::to_value(parsed).unwrap();
            wire.as_object_mut().unwrap().remove("action");
            assert_eq!(wire, args, "{name}");
        }
    }

    #[test]
    fn missing_unknown_and_wrongly_wrapped_fields_are_actionable() {
        for (args, expected) in [
            (
                serde_json::json!({"name":"Drums"}),
                "arguments.kind is required",
            ),
            (
                serde_json::json!({"name":"Drums","kind":"drum","instrument":"kit"}),
                "arguments.instrument is not accepted",
            ),
            (
                serde_json::json!({"name":"Drums","kind":"percussion"}),
                "kind must be one of",
            ),
            (
                serde_json::json!({"command":{"name":"Drums","kind":"drum"}}),
                "without action or command wrappers",
            ),
            (
                serde_json::json!([{"name":"Drums","kind":"drum"}]),
                "flat argument object",
            ),
        ] {
            let error = command("add_track", &args).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
        assert!(
            command("list_instruments", &serde_json::json!({}))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn flat_catalog_covers_commands_without_a_tagged_union_or_file_destinations() {
        let tools = definitions();
        assert_eq!(tools.len(), 17);
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
