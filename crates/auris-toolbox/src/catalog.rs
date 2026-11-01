//! Shared wire schemas and on-demand help for model clients.
use super::*;

/// Explain a likely field mismatch after a saved-project tool rejects its arguments.
/// This is diagnostic help, not a substitute for deserialization or session validation.
pub fn argument_error_hint(name: &str, arguments: &serde_json::Value) -> Option<String> {
    let definition = tool_catalog().into_iter().find(|tool| tool.name == name)?;
    super::live_agent::argument_hint(&definition.parameters, arguments, "arguments")
}

/// A model-facing tool definition, shared by both transports.
pub struct ToolDefinition {
    /// Exact callable name.
    pub name: String,
    /// Instructions for choosing and using the tool.
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub parameters: serde_json::Value,
    /// Validated synchronous execution, shared by saved-project transports.
    pub call: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
}

/// Builds a self-contained argument schema without presentation-only metadata.
///
/// Small model tool templates often display only the properties, leaving references to
/// definitions unreadable. Inline those definitions while keeping required fields, tagged
/// unions, limits and nullability intact. Recursive types retain their necessary references.
pub fn parameter_schema<T: schemars::JsonSchema>() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .with_transform(schemars::transform::RecursiveTransform(
            |schema: &mut schemars::Schema| {
                schema.remove("title");
                schema.remove("$schema");
                // Rust numeric formats are annotations, not portable JSON Schema bounds.
                // Preserve string formats such as date-time and all numeric constraints.
                let numeric_type = match schema.get("type") {
                    Some(serde_json::Value::String(kind)) => {
                        matches!(kind.as_str(), "number" | "integer")
                    }
                    Some(serde_json::Value::Array(kinds)) => {
                        kinds
                            .iter()
                            .any(|kind| kind == "number" || kind == "integer")
                            && kinds
                                .iter()
                                .all(|kind| kind == "number" || kind == "integer" || kind == "null")
                    }
                    _ => false,
                };
                if numeric_type
                    && schema
                        .get("format")
                        .and_then(|value| value.as_str())
                        .is_some_and(|format| {
                            matches!(
                                format,
                                "float"
                                    | "double"
                                    | "int8"
                                    | "uint8"
                                    | "int16"
                                    | "uint16"
                                    | "int32"
                                    | "uint32"
                                    | "int64"
                                    | "uint64"
                                    | "int128"
                                    | "uint128"
                                    | "int"
                                    | "uint"
                                    | "isize"
                                    | "usize"
                            )
                        })
                {
                    schema.remove("format");
                }
                // Schemars keeps doc comments on unit enum variants by emitting oneOf/const.
                // A plain string enum is easier for tool templates and small models to follow.
                // Only collapse branches with no other validation constraints.
                let choices = schema
                    .get("oneOf")
                    .and_then(|value| value.as_array())
                    .and_then(|variants| {
                        if variants.is_empty() {
                            return None;
                        }
                        variants
                            .iter()
                            .map(|variant| {
                                let object = variant.as_object()?;
                                if object.keys().any(|key| {
                                    !matches!(
                                        key.as_str(),
                                        "const" | "type" | "description" | "title"
                                    )
                                }) || object.get("type").is_some_and(|value| value != "string")
                                {
                                    return None;
                                }
                                object.get("const")?.as_str().map(String::from)
                            })
                            .collect::<Option<Vec<_>>>()
                    });
                if let Some(choices) = choices {
                    schema.remove("oneOf");
                    schema.insert("type".into(), "string".into());
                    schema.insert("enum".into(), serde_json::json!(choices));
                }
                // Tool templates may ignore oneOf while deciding how to display a field.
                // Every branch already requires an object, so this adds no restriction.
                if schema.get("type").is_none()
                    && schema
                        .get("oneOf")
                        .and_then(|value| value.as_array())
                        .is_some_and(|variants| {
                            !variants.is_empty()
                                && variants.iter().all(|variant| {
                                    variant.get("type").is_some_and(|kind| kind == "object")
                                })
                        })
                {
                    schema.insert("type".into(), "object".into());
                }
            },
        ))
        .into_generator()
        .into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).expect("derived schemas serialize");
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
}

fn definition<T: schemars::JsonSchema + serde::de::DeserializeOwned + 'static>(
    name: &'static str,
    description: &'static str,
    run: fn(&T) -> Result<String, String>,
) -> ToolDefinition {
    let mut parameters = parameter_schema::<T>();
    if let Some(project) = parameters
        .get_mut("properties")
        .and_then(|p| p.get_mut("project"))
    {
        project["description"] =
            "Project path or project_id from create_project/open_project.".into();
    }
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
        call: Box::new(move |value| {
            let args = serde_json::from_value(value)
                .map_err(|error| format!("Invalid arguments for {name}: {error}"))?;
            run(&args)
        }),
    }
}

/// No arguments, said as a schema — for the reference and listing tools.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

/// Complete project-tool catalog. Transport tests compare their registrations to this list.
pub fn tool_catalog() -> Vec<ToolDefinition> {
    let mut tools = vec![
        definition::<open_project::Args>(
            open_project::NAME,
            open_project::DESCRIPTION,
            open_project::run,
        ),
        definition::<setup_tracks::Args>(
            setup_tracks::NAME,
            setup_tracks::DESCRIPTION,
            setup_tracks::run,
        ),
        definition::<discover_tools::Args>(
            discover_tools::NAME,
            discover_tools::DESCRIPTION,
            discover_tools::run,
        ),
        definition::<read_report::Args>(
            read_report::NAME,
            read_report::DESCRIPTION,
            read_report::run,
        ),
        definition::<create_project::Args>(
            create_project::NAME,
            create_project::DESCRIPTION,
            create_project::run,
        ),
        definition::<capabilities::Args>(
            capabilities::NAME,
            capabilities::DESCRIPTION,
            capabilities::run,
        ),
        definition::<analyze_music::Args>(
            analyze_music::NAME,
            analyze_music::DESCRIPTION,
            analyze_music::run,
        ),
        definition::<analyze_chords::Args>(
            analyze_chords::NAME,
            analyze_chords::DESCRIPTION,
            analyze_chords::run,
        ),
        definition::<analyze_audio::Args>(
            analyze_audio::NAME,
            analyze_audio::DESCRIPTION,
            analyze_audio::run,
        ),
        definition::<analyze_instruments::Args>(
            analyze_instruments::NAME,
            analyze_instruments::DESCRIPTION,
            analyze_instruments::run,
        ),
        definition::<inspect_composition::Args>(
            inspect_composition::NAME,
            inspect_composition::DESCRIPTION,
            inspect_composition::run,
        ),
        definition::<NoArgs>(spec_reference::NAME, spec_reference::DESCRIPTION, |_| {
            Ok(spec_reference::run())
        }),
        definition::<search_documentation::Args>(
            search_documentation::NAME,
            search_documentation::DESCRIPTION,
            search_documentation::run,
        ),
        definition::<check_spec::Args>(check_spec::NAME, check_spec::DESCRIPTION, check_spec::run),
        definition::<configure_plugin_paths::Args>(
            configure_plugin_paths::NAME,
            configure_plugin_paths::DESCRIPTION,
            configure_plugin_paths::run,
        ),
        definition::<preview_composition::Args>(
            preview_composition::NAME,
            preview_composition::DESCRIPTION,
            preview_composition::run,
        ),
        definition::<compose::Args>(compose::NAME, compose::DESCRIPTION, compose::run),
        definition::<analyze::Args>(analyze::NAME, analyze::DESCRIPTION, analyze::run),
        definition::<analyze_drum_kit::Args>(
            analyze_drum_kit::NAME,
            analyze_drum_kit::DESCRIPTION,
            analyze_drum_kit::run,
        ),
        definition::<section_gain::Args>(
            section_gain::NAME,
            section_gain::DESCRIPTION,
            section_gain::run,
        ),
        definition::<teach_progression::Args>(
            teach_progression::NAME,
            teach_progression::DESCRIPTION,
            teach_progression::run,
        ),
        definition::<forget_progression::Args>(
            forget_progression::NAME,
            forget_progression::DESCRIPTION,
            forget_progression::run,
        ),
        definition::<NoArgs>(
            list_progressions::NAME,
            list_progressions::DESCRIPTION,
            |_| Ok(list_progressions::run()),
        ),
        definition::<NoArgs>(list_presets::NAME, list_presets::DESCRIPTION, |_| {
            Ok(list_presets::run())
        }),
        definition::<accompany::Args>(accompany::NAME, accompany::DESCRIPTION, accompany::run),
        definition::<tool_help::Args>(tool_help::NAME, tool_help::DESCRIPTION, tool_help::run),
        definition::<convert_track_to_audio::Args>(
            convert_track_to_audio::NAME,
            convert_track_to_audio::DESCRIPTION,
            convert_track_to_audio::run,
        ),
    ];
    let common = crate::session_tools::definitions();
    tools.extend(common);
    tools
}

/// Exact tool arguments, fetched when a model needs to correct a call.
pub mod tool_help {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "tool_help";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Returns the compact argument schema and examples for one tool. Set operation to an action/kind name for its exact schema with field explanations. Use before an unfamiliar edit or after an argument error; copy the field names and nesting, replacing example paths and selectors with the current project's values. Does not change files.";
    /// The tool to explain.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Exact tool name, for example edit_clip, preview, automation or mixer_graph.
        pub name: String,
        /// Optional action/kind within the tool's operation field, for example set_strip.
        pub operation: Option<String>,
    }
    /// Returns only the requested tool's reference, keeping unrelated schemas out of context.
    pub fn run(args: &Args) -> Result<String, String> {
        let catalog = tool_catalog();
        let tool = catalog
            .iter()
            .find(|tool| tool.name == args.name)
            .ok_or_else(|| {
                format!(
                    "Unknown tool '{}'. Use discover_tools with a short query (for example edit_clip) to find its exact name.",
                    args.name.chars().take(80).collect::<String>()
                )
            })?;
        reference(args, &tool.name, &tool.description, &tool.parameters, false)
    }

    pub(crate) fn reference(
        args: &Args,
        name: &str,
        description: &str,
        parameters: &serde_json::Value,
        live: bool,
    ) -> Result<String, String> {
        let variants = parameters["properties"]["operation"]["oneOf"].as_array();
        let variant_name = |variant: &serde_json::Value| {
            ["action", "kind"]
                .into_iter()
                .find_map(|tag| variant["properties"][tag]["const"].as_str())
                .map(String::from)
        };
        let operations: Vec<_> = variants
            .into_iter()
            .flatten()
            .filter_map(variant_name)
            .collect();
        let mut schema = parameters.clone();
        if let Some(operation) = &args.operation {
            let selected = variants
                .into_iter()
                .flatten()
                .find(|variant| variant_name(variant).as_ref() == Some(operation))
                .ok_or_else(|| {
                    format!(
                        "Unknown operation for {name}; choose from {}",
                        operations.join(", ")
                    )
                })?;
            schema["properties"]["operation"] = selected.clone();
        } else {
            crate::tool_exposure::compact_help_parameters(&mut schema);
        }
        let project = "/absolute/path/Song/Song.auris";
        let mut examples = match name {
            "arrangement" => serde_json::json!([
                {"project":project,"operation":{"kind":"inspect","start_bar":1,"bars":8}},
                {"project":project,"operation":{"kind":"inspect_breathing","track":1,"role":"lead","start_tick":0,"end_tick":15360,"min_rest_ticks":480,"phrase_ends":[15360]}},
                {"project":project,"operation":{"kind":"validate","expected_revision":"copy-from-current-inspection","changes":[{"purpose":"Release the lead phrase before its answer","edit":{"kind":"breath","clips":[2],"rests":[{"start_tick":14880,"end_tick":15360}],"min_note_ticks":120,"protected_notes":[],"omit_notes":[]}}]}},
                {"project":project,"operation":{"kind":"validate","expected_revision":"copy-from-inspection","changes":[{"purpose":"Spread chorus guitar attacks","edit":{"kind":"performance","clip":2,"settings":{"strum":{"spread_ms":25}}}}]}},
                {"project":project,"operation":{"kind":"apply","plan_id":"copy-from-validation","expected_revision":"same-original-revision"}}
            ]),
            "mixer_graph" => serde_json::json!([
                {"project":project,"operation":{"action":"read"}},
                {"project":project,"operation":{"action":"set_strip","node":"track:1","gain_db":-6.0,"pan":0.25}},
                {"project":project,"operation":{"action":"connect","source":"track:1","target":"track:2","kind":"send"}},
                {"project":project,"operation":{"action":"parameters","node":"effect:3"}}
            ]),
            "add_track" => serde_json::json!([
                {"project":project,"name":"Reverb","kind":"bus"},
                {"project":project,"name":"Kit","kind":"drum"},
                {"project":project,"name":"Lead","kind":"instrument"}
            ]),
            "add_clip" => serde_json::json!([
                {"project":project,"track":1,"name":"Manual","start_bar":5,"bars":2}
            ]),
            "listen" => serde_json::json!([
                {"project":project,"start_bar":1,"bars":4,"focus":"Is the lead buried by the accompaniment?"},
                {"project":project,"start_bar":1,"bars":4,"compare_to":"audition:copy-from-listen","aspect":"balance"}
            ]),
            "edit_clip" => serde_json::json!([
                {"project":project,"clip":2,"operation":{"kind":"resize","end_tick":30720}},
                {"project":project,"clip":2,"operation":{"kind":"copy","track":1,"start_tick":30720,"transpose":12}},
                {"project":project,"clip":2,"operation":{"kind":"loop","end_tick":61440}}
            ]),
            "preview" => serde_json::json!([
                {"project":project,"range":{"start_bar":1,"bars":4}}
            ]),
            "automation" => serde_json::json!([
                {"project":project,"target":{"kind":"track_gain","track":1},"operation":{"kind":"read"}},
                {"project":project,"target":{"kind":"track_gain","track":1},"expected_revision":"copy-from-automation-read","operation":{"kind":"set","points":[{"at_tick":0,"value":-12}],"replace":false,"curve":"linear"}}
            ]),
            "history" => serde_json::json!([
                {"operation":{"kind":"read"}},
                {"operation":{"kind":"undo","group":1,"expected_revision":12}}
            ]),
            "read_notes" => serde_json::json!([
                {"project":project,"clips":[{"clip":1},{"clip":2,"offset":128}]}
            ]),
            "edit_notes" => serde_json::json!([
                {"project":project,"expected_revision":"copy-from-read_notes","input":{"kind":"inline","edits":[{"kind":"patch","clip":1,"add":[{"pitch":60,"bar":1,"beat":1,"length_beats":1,"velocity":0.75}]}]}}
            ]),
            _ => serde_json::json!([]),
        };
        if live {
            for example in examples.as_array_mut().unwrap() {
                example.as_object_mut().unwrap().remove("project");
            }
        }
        if let Some(operation) = &args.operation {
            examples.as_array_mut().unwrap().retain(|example| {
                let value = &example["operation"];
                value["action"].as_str().or_else(|| value["kind"].as_str()) == Some(operation)
            });
        }
        Ok(serde_json::json!({"name":name,"description":description,
            "parameters":schema,"operations":operations,"examples":examples})
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique_and_help_preserves_each_exact_schema() {
        let catalog = tool_catalog();
        let names: std::collections::BTreeSet<_> =
            catalog.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names.len(), catalog.len());
        for tool in catalog {
            let help: serde_json::Value = serde_json::from_str(
                &tool_help::run(&tool_help::Args {
                    name: tool.name.clone(),
                    operation: None,
                })
                .unwrap(),
            )
            .unwrap();
            let mut expected = tool.parameters.clone();
            crate::tool_exposure::compact_help_parameters(&mut expected);
            assert_eq!(help["parameters"], expected);
            if crate::session_tools::definitions()
                .iter()
                .any(|definition| definition.name == tool.name)
            {
                for example in help["examples"].as_array().unwrap() {
                    let mut example = example.clone();
                    example.as_object_mut().unwrap().remove("project");
                    assert!(
                        crate::live_agent::command(&tool.name, &example)
                            .unwrap()
                            .is_some(),
                        "{}",
                        tool.name
                    );
                }
            }
        }
    }

    #[test]
    fn operation_help_is_bounded_and_retains_exact_field_constraints() {
        let catalog = tool_catalog();
        let definition = catalog
            .iter()
            .find(|tool| tool.name == "mixer_graph")
            .unwrap();
        let request = tool_help::Args {
            name: "mixer_graph".into(),
            operation: Some("set_strip".into()),
        };
        let help = tool_help::run(&request).unwrap();
        assert!(help.len() < 6000, "{}", help.len());
        let help: serde_json::Value = serde_json::from_str(&help).unwrap();
        let original = definition.parameters["properties"]["operation"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["action"]["const"] == "set_strip")
            .unwrap();
        assert_eq!(help["parameters"]["properties"]["operation"], *original);
        assert_eq!(
            help["parameters"]["properties"]["project"],
            definition.parameters["properties"]["project"]
        );
        let invalid = tool_help::Args {
            operation: Some("guessed_action".into()),
            ..request
        };
        assert!(tool_help::run(&invalid).unwrap_err().contains("set_strip"));
    }

    #[test]
    fn canonical_creation_and_clip_edits_reject_guessed_fields() {
        for (name, args) in [
            ("add_track", serde_json::json!({"name":"Kit","drums":true})),
            ("add_clip", serde_json::json!({"track":1,"bars":1})),
            ("preview", serde_json::json!({"start_bar":1,"bars":1})),
            (
                "edit_clip",
                serde_json::json!({"clip":1,"operation":{"kind":"resize","end_tick":3840,"bar":2}}),
            ),
        ] {
            assert!(crate::live_agent::command(name, &args).is_err(), "{name}");
        }
        let root = crate::parameter_schema::<auris_session::live_agent::Command>();
        assert!(!root.to_string().contains("\"$ref\""));
    }
}
