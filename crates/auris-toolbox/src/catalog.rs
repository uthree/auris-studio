//! Shared wire schemas and on-demand help for model clients.
use super::*;

/// A model-facing tool definition, shared by both transports.
pub struct ToolDefinition {
    /// Exact callable name.
    pub name: &'static str,
    /// Instructions for choosing and using the tool.
    pub description: &'static str,
    /// JSON Schema for the tool's arguments.
    pub parameters: serde_json::Value,
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

fn definition<T: schemars::JsonSchema>(
    name: &'static str,
    description: &'static str,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        parameters: parameter_schema::<T>(),
    }
}

/// No arguments, said as a schema — for the reference and listing tools.
#[derive(schemars::JsonSchema)]
struct NoArgs {}

/// Complete project-tool catalog. Transport tests compare their registrations to this list.
pub fn tool_catalog() -> Vec<ToolDefinition> {
    vec![
        definition::<listen::Args>(listen::NAME, listen::DESCRIPTION),
        definition::<create_project::Args>(create_project::NAME, create_project::DESCRIPTION),
        definition::<import_audio::Args>(import_audio::NAME, import_audio::DESCRIPTION),
        definition::<import_midi::Args>(import_midi::NAME, import_midi::DESCRIPTION),
        definition::<export_midi::Args>(export_midi::NAME, export_midi::DESCRIPTION),
        definition::<capabilities::Args>(capabilities::NAME, capabilities::DESCRIPTION),
        definition::<effects::Args>(effects::NAME, effects::DESCRIPTION),
        definition::<automation::Args>(automation::NAME, automation::DESCRIPTION),
        definition::<analyze_music::Args>(analyze_music::NAME, analyze_music::DESCRIPTION),
        definition::<analyze_chords::Args>(analyze_chords::NAME, analyze_chords::DESCRIPTION),
        definition::<analyze_audio::Args>(analyze_audio::NAME, analyze_audio::DESCRIPTION),
        definition::<analyze_instruments::Args>(
            analyze_instruments::NAME,
            analyze_instruments::DESCRIPTION,
        ),
        definition::<transcribe_audio::Args>(transcribe_audio::NAME, transcribe_audio::DESCRIPTION),
        definition::<transcribe_mixture::Args>(
            transcribe_mixture::NAME,
            transcribe_mixture::DESCRIPTION,
        ),
        definition::<inspect_composition::Args>(
            inspect_composition::NAME,
            inspect_composition::DESCRIPTION,
        ),
        definition::<edit_harmony::Args>(edit_harmony::NAME, edit_harmony::DESCRIPTION),
        definition::<edit_recipe::Args>(edit_recipe::NAME, edit_recipe::DESCRIPTION),
        definition::<edit_clip::Args>(edit_clip::NAME, edit_clip::DESCRIPTION),
        definition::<checkpoints::Args>(checkpoints::NAME, checkpoints::DESCRIPTION),
        definition::<NoArgs>(spec_reference::NAME, spec_reference::DESCRIPTION),
        definition::<search_documentation::Args>(
            search_documentation::NAME,
            search_documentation::DESCRIPTION,
        ),
        definition::<check_spec::Args>(check_spec::NAME, check_spec::DESCRIPTION),
        definition::<compose::Args>(compose::NAME, compose::DESCRIPTION),
        definition::<render::Args>(render::NAME, render::DESCRIPTION),
        definition::<preview::Args>(preview::NAME, preview::DESCRIPTION),
        definition::<describe::Args>(describe::NAME, describe::DESCRIPTION),
        definition::<analyze::Args>(analyze::NAME, analyze::DESCRIPTION),
        definition::<analyze_drum_kit::Args>(analyze_drum_kit::NAME, analyze_drum_kit::DESCRIPTION),
        definition::<set_drum_assignment::Args>(
            set_drum_assignment::NAME,
            set_drum_assignment::DESCRIPTION,
        ),
        definition::<mixer::Args>(mixer::NAME, mixer::DESCRIPTION),
        definition::<set_level::Args>(set_level::NAME, set_level::DESCRIPTION),
        definition::<set_effect::Args>(set_effect::NAME, set_effect::DESCRIPTION),
        definition::<section_gain::Args>(section_gain::NAME, section_gain::DESCRIPTION),
        definition::<regenerate_clips::Args>(regenerate_clips::NAME, regenerate_clips::DESCRIPTION),
        definition::<teach_progression::Args>(
            teach_progression::NAME,
            teach_progression::DESCRIPTION,
        ),
        definition::<forget_progression::Args>(
            forget_progression::NAME,
            forget_progression::DESCRIPTION,
        ),
        definition::<NoArgs>(list_progressions::NAME, list_progressions::DESCRIPTION),
        definition::<NoArgs>(list_presets::NAME, list_presets::DESCRIPTION),
        definition::<NoArgs>(list_instruments::NAME, list_instruments::DESCRIPTION),
        definition::<add_track::Args>(add_track::NAME, add_track::DESCRIPTION),
        definition::<add_part::Args>(add_part::NAME, add_part::DESCRIPTION),
        definition::<set_instrument::Args>(set_instrument::NAME, set_instrument::DESCRIPTION),
        definition::<rename_track::Args>(rename_track::NAME, rename_track::DESCRIPTION),
        definition::<remove_track::Args>(remove_track::NAME, remove_track::DESCRIPTION),
        definition::<add_clip::Args>(add_clip::NAME, add_clip::DESCRIPTION),
        definition::<notes::Args>(notes::NAME, notes::DESCRIPTION),
        definition::<edit_notes::Args>(edit_notes::NAME, edit_notes::DESCRIPTION),
        definition::<replace_notes::Args>(replace_notes::NAME, replace_notes::DESCRIPTION),
        definition::<accompany::Args>(accompany::NAME, accompany::DESCRIPTION),
        definition::<write_lyrics::Args>(write_lyrics::NAME, write_lyrics::DESCRIPTION),
        definition::<sing::Args>(sing::NAME, sing::DESCRIPTION),
        definition::<compose_lyrics::Args>(compose_lyrics::NAME, compose_lyrics::DESCRIPTION),
        definition::<tool_help::Args>(tool_help::NAME, tool_help::DESCRIPTION),
        definition::<routing::Args>(routing::NAME, routing::DESCRIPTION),
        definition::<set_track_state::Args>(set_track_state::NAME, set_track_state::DESCRIPTION),
        definition::<convert_track_to_audio::Args>(
            convert_track_to_audio::NAME,
            convert_track_to_audio::DESCRIPTION,
        ),
        definition::<set_instrument_param::Args>(
            set_instrument_param::NAME,
            set_instrument_param::DESCRIPTION,
        ),
    ]
}

/// Exact tool arguments, fetched when a model needs to correct a call.
pub mod tool_help {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "tool_help";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Returns the exact argument schema and examples for one tool. Use before an unfamiliar edit or after an argument error; copy the field names and nesting, replacing example paths and selectors with the current project's values. Does not change files.";
    /// The tool to explain.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Exact tool name, for example edit_clip, preview, automation or routing.
        pub name: String,
    }
    /// Returns only the requested tool's reference, keeping unrelated schemas out of context.
    pub fn run(args: &Args) -> Result<String, String> {
        let catalog = tool_catalog();
        let tool = catalog
            .iter()
            .find(|tool| tool.name == args.name)
            .ok_or_else(|| {
                format!(
                    "Unknown tool '{}'. Available tools: {}",
                    args.name,
                    catalog
                        .iter()
                        .map(|tool| tool.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let project = "/absolute/path/Song/Song.auris";
        let examples = match tool.name {
            "add_track" => serde_json::json!([
                {"project":project,"name":"Reverb","kind":"bus"},
                {"project":project,"name":"Kit","kind":"drum"},
                {"project":project,"name":"Lead","kind":"instrument","instrument":"auris.synth.chiptune"}
            ]),
            "add_clip" => serde_json::json!([
                {"project":project,"track":"Lead","name":"Manual","start_bar":5,"bars":2}
            ]),
            "listen" => serde_json::json!([
                {"project":project,"start_bar":1,"bars":4,"focus":"Is the lead buried by the accompaniment?"},
                {"project":project,"start_bar":1,"bars":4,"compare_to":"/absolute/path/Song/.auris-previews/preview-before.wav","focus":"Did the balance improve?"}
            ]),
            "edit_clip" => serde_json::json!([
                {"project":project,"track":"Lead","clip":1,"action":{"kind":"resize","end_bar":9}},
                {"project":project,"track":"Lead","clip":1,"action":{"kind":"copy","destination":"Flute","bar":9,"transpose":12}}
            ]),
            "preview" => serde_json::json!([
                {"project":project,"start_bar":1,"bars":4},
                {"project":project,"section":"chorus","instance":1}
            ]),
            "effects" => serde_json::json!([
                {"project":project,"track":"Bass","operation":{"action":"list"}},
                {"project":project,"track":"Bass","operation":{"action":"add","effect":"auris.fx.compressor"}},
                {"project":project,"track":"Bass","operation":{"action":"sidechain","slot":1,"source":"Kick"}}
            ]),
            "set_effect" => serde_json::json!([
                {"project":project,"track":"Bass","slot":1,"param":"threshold_db","value":-18},
                {"project":project,"track":"master","slot":1,"effect":"auris.fx.limiter","param":"input_db","value":-3}
            ]),
            "automation" => serde_json::json!([
                {"project":project,"track":"Bass","target":{"kind":"effect","slot":1},"operation":{"action":"read"}},
                {"project":project,"track":"Bass","target":{"kind":"effect","slot":1},"operation":{"action":"set","param":"threshold_db","points":[{"beat":0,"value":-6},{"beat":16,"value":-24}],"curve":"linear"}}
            ]),
            "routing" => serde_json::json!([
                {"project":project,"track":"Lead","operation":"add_send","destination":"Reverb","level_db":-12},
                {"project":project,"track":"Lead","operation":"send_level","send_id":4,"level_db":-18},
                {"project":project,"track":"Lead","operation":"output","destination":"master"}
            ]),
            "regenerate_clips" => serde_json::json!([
                {"project":project,"track":"Lead","clip":1,"take":{"kind":"same"}},
                {"project":project,"track":"Lead","take":{"kind":"next"}},
                {"project":project,"track":"Lead","clip":1,"take":{"kind":"seed","seed":42}}
            ]),
            "edit_notes" => serde_json::json!([
                {"project":project,"track":"Lead","clip":1,"add":[{"pitch":"C4","bar":1,"beat":1,"beats":1,"velocity":0.75}]}
            ]),
            "replace_notes" => serde_json::json!([
                {"project":project,"track":"Lead","clip":1,"source":"/absolute/path/lead.json"},
                {"project":project,"track":"Lead","clip":1,"notes":[{"pitch":60,"bar":1,"beat":1,"beats":1}]},
                {"project":project,"track":"Lead","clip":1,"notes":[]}
            ]),
            _ => serde_json::json!([]),
        };
        Ok(
            serde_json::json!({"name":tool.name,"description":tool.description,
            "parameters":tool.parameters,"examples":examples})
            .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creation_requires_intended_track_kind_and_clip_name() {
        let track = serde_json::json!({"project":"/song.auris","name":"Reverb"});
        assert!(serde_json::from_value::<add_track::Args>(track.clone()).is_err());
        let clip = serde_json::json!({"project":"/song.auris","track":"Lead","bars":2});
        assert!(serde_json::from_value::<add_clip::Args>(clip.clone()).is_err());
        let mut named_bus = track;
        named_bus["kind"] = serde_json::json!("bus");
        assert_eq!(
            serde_json::from_value::<add_track::Args>(named_bus)
                .unwrap()
                .kind,
            add_track::Kind::Bus
        );
        let schema = parameter_schema::<add_track::Args>();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("kind"))
        );
        assert_eq!(
            schema["properties"]["kind"]["enum"],
            serde_json::json!(["instrument", "drum", "singer", "audio", "bus"])
        );
        let schema = parameter_schema::<add_clip::Args>();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("name"))
        );
    }

    #[test]
    fn explicit_drum_kind_saves_tracks_for_the_drum_editor() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Song.auris");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let instrument = session
            .registry()
            .default_instrument_id()
            .unwrap()
            .to_string();
        session.save(&path).unwrap();
        assert!(
            serde_json::from_value::<add_track::Args>(serde_json::json!({
                "project":path,"name":"Kit","kind":"instrument","drums":true
            }))
            .is_err()
        );
        for (name, kind, instrument) in [
            ("Default kit", "drum", None),
            ("Explicit kit", "drum", Some(instrument)),
            ("Melody", "instrument", None),
        ] {
            let args = serde_json::from_value::<add_track::Args>(serde_json::json!({
                "project":path,"name":name,"kind":kind,
                "instrument":instrument
            }))
            .unwrap();
            add_track::run(&args).unwrap();
            let reopened = opened(path.to_str().unwrap()).unwrap();
            let track = track_by_name(reopened.project(), name).unwrap();
            assert_eq!(track.kind.is_drum(), kind == "drum", "{name}");
            assert!(track.kind.as_instrument().unwrap().clips.is_empty());
        }
    }

    #[test]
    fn regeneration_requires_one_complete_seed_policy() {
        for take in [
            serde_json::json!({"kind":"same"}),
            serde_json::json!({"kind":"next"}),
            serde_json::json!({"kind":"seed","seed":42}),
        ] {
            assert!(
                serde_json::from_value::<regenerate_clips::Args>(serde_json::json!({
                    "project":"/song.auris","track":"Lead","take":take
                }))
                .is_ok()
            );
        }
        for take in [
            serde_json::Value::Null,
            serde_json::json!("same"),
            serde_json::json!({"kind":"same","seed":42}),
            serde_json::json!({"kind":"next","seed":42}),
            serde_json::json!({"kind":"seed"}),
            serde_json::json!({"kind":"seed","seed":-1}),
        ] {
            assert!(
                serde_json::from_value::<regenerate_clips::Args>(serde_json::json!({
                    "project":"/song.auris","track":"Lead","take":take
                }))
                .is_err()
            );
        }
        let schema = parameter_schema::<regenerate_clips::Args>();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("take"))
        );
        assert_eq!(
            schema["properties"]["take"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn nested_operations_are_readable_without_definitions_and_keep_requirements() {
        let schema = parameter_schema::<edit_clip::Args>();
        assert!(!schema.to_string().contains("\"$ref\""));
        let variants = schema["properties"]["action"]["oneOf"].as_array().unwrap();
        let resize = variants
            .iter()
            .find(|v| v["properties"]["kind"]["const"] == "resize")
            .unwrap();
        assert!(
            resize["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("end_bar"))
        );
        assert!(
            !resize["properties"]
                .as_object()
                .unwrap()
                .contains_key("bar")
        );
    }

    #[test]
    fn unit_operations_are_plain_string_enums_but_tagged_objects_stay_structured() {
        let schema = parameter_schema::<routing::Args>();
        let operation = &schema["properties"]["operation"];
        assert_eq!(operation["type"], "string");
        assert_eq!(
            operation["enum"],
            serde_json::json!([
                "list",
                "output",
                "add_send",
                "remove_send",
                "send_mode",
                "send_level"
            ])
        );
        assert!(operation.get("oneOf").is_none());
        let effects = parameter_schema::<effects::Args>();
        assert_eq!(effects["properties"]["operation"]["type"], "object");
        assert!(effects["properties"]["operation"]["oneOf"].is_array());
        assert!(
            serde_json::from_value::<routing::Args>(serde_json::json!({
                "project":"/song.auris","track":"Lead","operation":[["list",null]]
            }))
            .is_err()
        );
    }

    #[test]
    fn tagged_operations_declare_objects_and_refuse_numeric_placeholders() {
        let automation = parameter_schema::<automation::Args>();
        for field in ["target", "operation"] {
            assert_eq!(automation["properties"][field]["type"], "object");
            assert!(automation["properties"][field]["oneOf"].is_array());
        }
        let set = automation["properties"]["operation"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["action"]["const"] == "set")
            .unwrap();
        assert!(
            set["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("points"))
        );
        assert_eq!(set["properties"]["points"]["minItems"], 1);
        for operation in [serde_json::json!(-1), serde_json::json!("list")] {
            assert!(
                serde_json::from_value::<effects::Args>(serde_json::json!({
                    "project":"/song.auris", "track":"Bass", "operation":operation
                }))
                .is_err()
            );
            assert!(serde_json::from_value::<automation::Args>(serde_json::json!({
                "project":"/song.auris", "track":"Bass", "target":{"kind":"mixer"}, "operation":operation
            })).is_err());
        }
    }

    #[test]
    fn numeric_annotation_cleanup_preserves_bounds_nullability_and_string_formats() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Metadata {
            #[schemars(range(min = -6, max = 6))]
            gain: f32,
            count: u32,
            optional: Option<f64>,
            #[schemars(extend("format" = "date-time"))]
            timestamp: String,
            #[schemars(extend("format" = "float"))]
            textual_format: String,
        }
        let schema = parameter_schema::<Metadata>();
        let fields = &schema["properties"];
        for name in ["gain", "count", "optional"] {
            assert!(
                fields[name].get("format").is_none(),
                "{name}: {}",
                fields[name]
            );
        }
        assert_eq!(fields["gain"]["minimum"].as_f64(), Some(-6.0));
        assert_eq!(fields["gain"]["maximum"].as_f64(), Some(6.0));
        assert_eq!(fields["count"]["minimum"].as_f64(), Some(0.0));
        assert!(
            fields["optional"]["type"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("null"))
        );
        assert_eq!(fields["timestamp"]["format"], "date-time");
        assert_eq!(fields["textual_format"]["format"], "float");
    }

    #[test]
    fn effect_slots_are_required_in_schema_and_arguments() {
        let schema = parameter_schema::<set_effect::Args>();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("slot"))
        );
        assert_eq!(schema["properties"]["slot"]["type"], "integer");
        assert_eq!(schema["properties"]["slot"]["minimum"].as_f64(), Some(1.0));
        assert!(serde_json::from_value::<set_effect::Args>(serde_json::json!({
            "project":"/song.auris", "track":"Bass", "effect":"compressor", "param":"threshold_db", "value":-18
        })).is_err());
        let primary: set_effect::Args = serde_json::from_value(serde_json::json!({
            "project":"/song.auris", "track":"Bass", "slot":1, "param":"threshold_db", "value":-18
        }))
        .unwrap();
        assert_eq!(primary.slot, 1);
        assert_eq!(primary.effect, None);
    }
    #[test]
    fn catalog_has_unique_names_and_help_preserves_the_exact_schema() {
        let catalog = tool_catalog();
        let names: std::collections::BTreeSet<_> = catalog.iter().map(|tool| tool.name).collect();
        assert_eq!(names.len(), catalog.len());
        for tool in catalog {
            let help: serde_json::Value = serde_json::from_str(
                &tool_help::run(&tool_help::Args {
                    name: tool.name.into(),
                })
                .unwrap(),
            )
            .unwrap();
            assert_eq!(help["parameters"], tool.parameters);
        }
        assert!(
            tool_help::run(&tool_help::Args {
                name: "invented_tool".into()
            })
            .unwrap_err()
            .contains("edit_clip")
        );
    }
    #[test]
    fn help_examples_deserialize_as_the_actual_arguments() {
        fn check<T: serde::de::DeserializeOwned>(name: &str) {
            let help: serde_json::Value = serde_json::from_str(
                &tool_help::run(&tool_help::Args { name: name.into() }).unwrap(),
            )
            .unwrap();
            for example in help["examples"].as_array().unwrap() {
                serde_json::from_value::<T>(example.clone())
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
        check::<edit_clip::Args>("edit_clip");
        check::<preview::Args>("preview");
        check::<effects::Args>("effects");
        check::<set_effect::Args>("set_effect");
        check::<automation::Args>("automation");
        check::<routing::Args>("routing");
        check::<listen::Args>("listen");
        check::<edit_notes::Args>("edit_notes");
        check::<replace_notes::Args>("replace_notes");
        check::<add_track::Args>("add_track");
        check::<add_clip::Args>("add_clip");
    }

    #[test]
    fn preview_rejects_misnested_ranges_instead_of_previewing_the_whole_song() {
        let valid: preview::Args = serde_json::from_value(serde_json::json!({
            "project":"Song.auris","start_bar":3,"bars":2
        }))
        .unwrap();
        assert_eq!(valid.range.start_bar, Some(3));
        assert_eq!(valid.range.bars, Some(2));
        for invalid in [
            serde_json::json!({"project":"Song.auris","range":{"start_bar":3,"bars":2}}),
            serde_json::json!({"project":"Song.auris","start_bar":3,"bars":2,"end_bar":5}),
        ] {
            assert!(serde_json::from_value::<preview::Args>(invalid).is_err());
        }
    }

    #[test]
    fn clip_resize_refuses_guessed_position_fields_even_with_a_valid_end() {
        for action in [
            serde_json::json!({"kind":"resize","bar":9}),
            serde_json::json!({"kind":"resize","end_bar":9,"bar":9}),
        ] {
            assert!(
                serde_json::from_value::<edit_clip::Args>(serde_json::json!({
                    "project":"Song.auris","track":"Lead","clip":1,"action":action
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn clip_positions_in_compound_meter_use_eighth_note_beats() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Song.auris");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_signature_at(Ticks::ZERO, TimeSignature::new(6, 8));
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 3)
            .unwrap();
        session.save(&path).unwrap();
        let args: edit_clip::Args = serde_json::from_value(serde_json::json!({
            "project":path,"track":"Lead","clip":1,
            "action":{"kind":"move","bar":2,"beat":3}
        }))
        .unwrap();
        edit_clip::run(&args).unwrap();
        let reopened = opened(path.to_str().unwrap()).unwrap();
        // One 6/8 bar is three quarter notes; beat three is two eighth notes into it.
        assert_eq!(reopened.clip_start(clip), Some(Ticks::QUARTER * 4));
    }
}
