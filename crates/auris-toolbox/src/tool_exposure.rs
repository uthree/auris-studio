//! Task-specific catalogs and concise wire presentation; full help stays on demand.
use super::*;

/// Startup selection; omitted configuration preserves the complete catalog.
#[derive(Clone, Debug, Default)]
pub struct ToolGroups(Vec<String>);
impl ToolGroups {
    /// Parses comma-separated manual, mix, transcription, vocals, composition, or all.
    pub fn parse(value: &str) -> Result<Self, String> {
        let groups = value
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if groups.iter().any(|s| {
            !matches!(
                s.as_str(),
                "all" | "manual" | "mix" | "transcription" | "vocals" | "composition"
            )
        }) {
            return Err("AURIS_MCP_TOOL_GROUPS must contain manual,mix,transcription,vocals,composition or all".into());
        }
        Ok(Self(groups))
    }
    /// Whether a callable is enabled. Shared inspection and help are always available.
    pub fn allows(&self, name: &str) -> bool {
        self.0.is_empty()
            || self.0.iter().any(|g| g == "all" || g == tool_group(name))
            || (matches!(name, "analyze_lyrics" | "diagnose_song")
                && self.0.iter().any(|g| g == "composition"))
            || (matches!(name, "accompaniment" | "suggest_harmony" | "candidates")
                && self.0.iter().any(|g| g == "manual"))
            || tool_group(name) == "common"
    }
    /// Workflow text for only the selected groups.
    pub fn instructions(&self) -> String {
        if self.0.is_empty() || self.0.iter().any(|g| g == "all") {
            return format!(
                "{INSTRUCTIONS}{}{}{}{}{}",
                crate::MELODY_PHRASING_INSTRUCTIONS,
                crate::ACCOMPANIMENT_INSTRUCTIONS,
                crate::PERFORMANCE_INSTRUCTIONS,
                crate::arrangement_instructions(),
                crate::composition_workflows::VOCAL_SONG_FORM
            );
        }
        let mut text = String::from(
            "Control saved Auris projects. Respond in the user's language. Use project_id from create_project/open_project in project arguments; paths also work. Make dependent edits sequentially. Use tool_help for exact fields and examples after an error; correct arguments before retrying. Use inspect_project for stable numeric track/clip IDs. Read large reports with read_report instead of repeating a write. Never invent successful results. ",
        );
        for group in &self.0 {
            text.push_str(match group.as_str() {
                "manual" => "For SFX, audio_assets finds local/Commons audio, imports URLs and crops/reverses clips with preserved source metadata; import_audio copies local files. For ordinary composition, author leads/vocals, counterlines, distinctive bass and fills note by note; do not substitute procedural note generators or frozen recipes. Rule writers are optional only for plain chords/held pads, simple arpeggios and basic drum pulses. Establish vocal lyrics, section chords/form, instrument clips, purposeful handoffs/SFX, then effects and volume balance with the actual sounds; verify the final full mix. search_instruments selects sound_id; source/library restrict candidates and sound.library indexes libraries. setup_tracks creates tracks/sounds/clips atomically; edit_notes writes exact tick-based notes. ",
                "mix" => "For mixing, read mixer_graph for stable track/effect nodes and outgoing edges; follow next_offset. Track nodes contain fader controls; inserts still process before the fader. Edit graph connections and inserts through mixer_graph; all output/send/sidechain cycles are rejected. Parallel effects use bus branches. Inspect mixer parameters in native dB/pan units, and measure the same rendered range before and after edits. After sound changes remeasure loudness; normalize_mix moves source faders toward the target (default -14 LUFS, -1 dBTP) and leans on a master auris.fx.limiter kept last in the chain. Render inspects the encoded WAV; use verify_render for target, peak and ending checks. ",
                "transcription" => "For transcription, analyze the source and fetch tool_help before applying recognized notes. ",
                "vocals" => "For vocals, analyze_lyrics establishes readings and diagnose_song checks shared phrases. Read notes before repair_lyrics, keep the template and keywords fixed, preview at most three alternatives and apply only a pass with the current revision. Recheck related phrases and meaning, then sing and confirm a current take. ",
                "composition" => "For explicitly requested automatic preset compositions, fetch spec_reference, validate syntax with check_spec and lyric fit with diagnose_song before compose; regeneration is an explicit edit. ",
                _ => "",
            });
        }
        if self.allows("arrangement") {
            text.push_str(crate::arrangement_instructions());
        }
        if self.allows("accompaniment") {
            text.push_str(crate::ACCOMPANIMENT_INSTRUCTIONS);
        }
        if self.allows("set_performance") {
            text.push_str(crate::PERFORMANCE_INSTRUCTIONS);
        }
        if self
            .0
            .iter()
            .any(|g| matches!(g.as_str(), "manual" | "vocals" | "composition"))
        {
            text.push_str(crate::composition_workflows::VOCAL_SONG_FORM);
        }
        if self
            .0
            .iter()
            .any(|g| matches!(g.as_str(), "manual" | "composition"))
        {
            text.push_str(crate::MELODY_PHRASING_INSTRUCTIONS);
        }
        text
    }
}
/// Primary task group for a saved-project callable.
pub fn tool_group(name: &str) -> &'static str {
    match name {
        "analyze_audio" | "analyze_chords" | "analyze_instruments" | "import_transcription" => {
            "transcription"
        }
        "write_lyrics" | "edit_lyrics" | "sing" | "list_voices" | "inspect_singer"
        | "set_singer_voice" | "edit_vocal_notes" | "analyze_lyrics" | "diagnose_song"
        | "repair_lyrics" => "vocals",
        "mixer_graph"
        | "automation"
        | "normalize_mix"
        | "section_gain"
        | "set_instrument_params"
        | "convert_track_to_audio" => "mix",
        "spec_reference"
        | "check_spec"
        | "preview_composition"
        | "compose"
        | "edit_recipe"
        | "teach_progression"
        | "forget_progression"
        | "list_progressions"
        | "list_presets"
        | "generate_phrase"
        | "accompaniment"
        | "candidates"
        | "suggest_harmony"
        | "accompany" => "composition",
        "arrangement"
        | "phrase_variation"
        | "create_project"
        | "audio_assets"
        | "import_audio"
        | "import_midi"
        | "setup_tracks"
        | "add_track"
        | "rename_project"
        | "rename_track"
        | "remove_track"
        | "layer"
        | "set_instrument"
        | "add_clip"
        | "edit_clip"
        | "read_notes"
        | "edit_clip_curve"
        | "edit_timeline"
        | "inspect_performance"
        | "set_performance"
        | "edit_notes"
        | "edit_harmony"
        | "set_drum_assignment" => "manual",
        _ => "common",
    }
}
/// Shortens explanatory prose while retaining sentences with units, limits and cautions.
pub fn concise_description(text: &str) -> String {
    text.split(". ")
        .enumerate()
        .filter(|(index, sentence)| {
            *index == 0
                || sentence.chars().any(|c| c.is_ascii_digit())
                || [
                    "must", "require", "only", "not ", "never", "omit", "default", "beat", "tick",
                    "decibel", "BPM", "expire", "clear", "replace", "limit", "bound", "error",
                    "retry",
                ]
                .iter()
                .any(|word| sentence.to_lowercase().contains(&word.to_lowercase()))
        })
        .map(|(_, sentence)| sentence)
        .collect::<Vec<_>>()
        .join(". ")
}
/// Remove prose and default annotations from transmitted schemas; help retains defaults.
/// Only JSON Schema positions are visited, preserving literal const payloads and properties
/// whose names happen to be description or default. Validation is unchanged.
pub fn compact_parameters(value: &mut serde_json::Value) {
    compact_annotations(value, false);
}

pub(crate) fn compact_help_parameters(value: &mut serde_json::Value) {
    compact_annotations(value, true);
}

fn compact_annotations(value: &mut serde_json::Value, keep_defaults: bool) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    schema.remove("description");
    if !keep_defaults {
        schema.remove("default");
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "dependentSchemas",
    ] {
        if let Some(entries) = schema
            .get_mut(key)
            .and_then(serde_json::Value::as_object_mut)
        {
            for child in entries.values_mut() {
                compact_annotations(child, keep_defaults);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "if",
        "then",
        "else",
        "not",
        "contains",
        "propertyNames",
        "contentSchema",
    ] {
        if let Some(child) = schema.get_mut(key) {
            compact_annotations(child, keep_defaults);
        }
    }
    for key in ["oneOf", "allOf", "anyOf", "prefixItems"] {
        if let Some(branches) = schema
            .get_mut(key)
            .and_then(serde_json::Value::as_array_mut)
        {
            for child in branches {
                compact_annotations(child, keep_defaults);
            }
        }
    }
}
/// Bounded discovery without loading every tool schema.
pub mod discover_tools {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "discover_tools";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Find tool names by text and optional task group, without full schemas. Fetch tool_help for one result. Startup AURIS_MCP_TOOL_GROUPS controls which groups are callable.";
    /// Bounded discovery arguments.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Optional case-insensitive words in the tool name or description.
        pub query: Option<String>,
        /// Optional group: common, manual, mix, transcription, vocals, composition.
        pub group: Option<String>,
        /// First match, zero-based.
        #[serde(default)]
        pub offset: usize,
    }
    /// Returns at most ten short entries.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.group.as_deref().is_some_and(|g| {
            !matches!(
                g,
                "common" | "manual" | "mix" | "transcription" | "vocals" | "composition"
            )
        }) {
            return Err(
                "Unknown group; use common,manual,mix,transcription,vocals,composition".into(),
            );
        }
        let catalog = tool_catalog();
        let words = args.query.as_deref().unwrap_or("").to_lowercase();
        let matches = catalog
            .iter()
            .filter(|t| {
                args.group
                    .as_deref()
                    .is_none_or(|g| g == tool_group(&t.name))
                    && words.split_whitespace().all(|w| {
                        format!("{} {}", t.name, t.description)
                            .to_lowercase()
                            .contains(w)
                    })
            })
            .collect::<Vec<_>>();
        if args.offset > matches.len() {
            return Err("offset exceeds tool count".into());
        }
        let page = matches
            .iter()
            .skip(args.offset)
            .take(10)
            .map(|t| serde_json::json!({"name":t.name,"group":tool_group(&t.name)}))
            .collect::<Vec<_>>();
        let next = args.offset + page.len();
        Ok(serde_json::json!({"tools":page,"total":matches.len(),"next_offset":(next<matches.len()).then_some(next)}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn without_descriptions(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(o) => {
                o.remove("description");
                o.remove("default");
                for child in o.values_mut() {
                    without_descriptions(child);
                }
            }
            serde_json::Value::Array(a) => {
                for child in a {
                    without_descriptions(child);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn compact_schema_preserves_named_description_fields_and_literal_payloads() {
        let literal =
            serde_json::json!({"description":"literal user data","default":"literal default data"});
        let mut schema = serde_json::json!({"type":"object","description":"schema prose","default":{},"properties":{"description":{"type":"string","description":"field prose","default":"annotation"},"default":{"type":"integer"},"payload":{"const":literal}},"required":["description"]});
        let mut help = schema.clone();
        compact_help_parameters(&mut help);
        assert_eq!(help["properties"]["description"]["default"], "annotation");
        compact_parameters(&mut schema);
        assert!(schema.get("default").is_none());
        assert_eq!(
            schema["properties"]["default"],
            serde_json::json!({"type":"integer"})
        );
        assert!(schema.get("description").is_none());
        assert_eq!(
            schema["properties"]["description"],
            serde_json::json!({"type":"string"})
        );
        assert_eq!(schema["properties"]["payload"]["const"], literal);
        assert_eq!(schema["required"], serde_json::json!(["description"]));
    }

    #[test]
    fn compact_schemas_keep_all_validation_and_required_units() {
        for tool in tool_catalog() {
            let mut original = tool.parameters.clone();
            let mut compact = original.clone();
            compact_parameters(&mut compact);
            assert!(
                compact.to_string().len() <= original.to_string().len() + 100,
                "{}",
                tool.name
            );
            without_descriptions(&mut original);
            without_descriptions(&mut compact);
            assert_eq!(original, compact, "{}", tool.name);
        }
        assert_eq!(
            concise_description(
                "Edit notes. Starts are beats. Maximum 256 notes. Do not repeat errors."
            ),
            "Edit notes. Starts are beats. Maximum 256 notes. Do not repeat errors."
        );
        assert!(ToolGroups::parse("typo").is_err());
        let manual = ToolGroups::parse("manual").unwrap();
        assert!(manual.allows("tool_help") && manual.allows("edit_notes"));
        assert!(!manual.allows("compose"));
        assert!(!manual.instructions().contains("spec_reference"));
        assert!(manual.allows("accompaniment"));
        assert!(manual.allows("suggest_harmony"));
        assert!(
            manual
                .instructions()
                .contains(crate::PERFORMANCE_INSTRUCTIONS)
        );
        assert!(
            ToolGroups::default()
                .instructions()
                .contains(crate::PERFORMANCE_INSTRUCTIONS)
        );
        assert!(
            manual
                .instructions()
                .contains(crate::ACCOMPANIMENT_INSTRUCTIONS)
        );
        let composition = ToolGroups::parse("composition").unwrap();
        assert!(composition.allows("accompaniment"));
        assert!(composition.allows("suggest_harmony"));
        assert!(
            composition
                .instructions()
                .contains(crate::ACCOMPANIMENT_INSTRUCTIONS)
        );
        let mix = ToolGroups::parse("mix").unwrap();
        for groups in [
            ToolGroups::default(),
            manual,
            composition,
            ToolGroups::parse("vocals").unwrap(),
        ] {
            assert!(
                groups
                    .instructions()
                    .contains(crate::composition_workflows::VOCAL_SONG_FORM)
            );
        }
        assert!(
            !mix.instructions()
                .contains(crate::composition_workflows::VOCAL_SONG_FORM)
        );
        assert!(!mix.allows("accompaniment"));
        assert!(!mix.allows("suggest_harmony"));
        assert!(
            !mix.instructions()
                .contains(crate::ACCOMPANIMENT_INSTRUCTIONS)
        );
    }
    #[test]
    fn discovery_and_unknown_help_are_bounded() {
        let page: serde_json::Value = serde_json::from_str(
            &discover_tools::run(&discover_tools::Args {
                query: None,
                group: None,
                offset: 0,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["tools"].as_array().unwrap().len(), 10);
        assert_eq!(page["next_offset"], 10);
        let error = tool_help::run(&tool_help::Args {
            name: "x".repeat(10000),
            operation: None,
        })
        .unwrap_err();
        assert!(error.len() < 256);
    }
}
