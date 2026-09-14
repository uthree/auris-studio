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
            || tool_group(name) == "common"
    }
    /// Workflow text for only the selected groups.
    pub fn instructions(&self) -> String {
        if self.0.is_empty() || self.0.iter().any(|g| g == "all") {
            return INSTRUCTIONS.into();
        }
        let mut text = String::from(
            "Control saved Auris projects. Respond in the user's language. Use project_id from create_project/open_project in project arguments; paths also work. Make dependent edits sequentially. Use tool_help for exact fields and examples after an error; correct arguments before retrying. Use describe for track IDs and clip numbers; refresh clip numbers after arrangement edits. Read large reports with read_report instead of repeating a write. Never invent successful results. ",
        );
        for group in &self.0 {
            text.push_str(match group.as_str() {
                "manual" => "For manual composition, search_instruments selects sound_id; source/library restrict candidates and sound.library indexes libraries. setup_tracks creates tracks/sounds/clips atomically; edit_notes or replace_notes writes notes. ",
                "mix" => "For mixing, inspect mixer, use native dB/pan units, and measure or listen to the same range before and after edits. ",
                "transcription" => "For transcription, analyze the source and fetch tool_help before applying recognized notes. ",
                "vocals" => "For vocals, inspect singer availability and write lyrics before singing. ",
                "composition" => "For generated compositions, fetch spec_reference, validate with check_spec and compose; regeneration is an explicit edit. ",
                _ => "",
            });
        }
        text
    }
}
/// Primary task group for a saved-project callable.
pub fn tool_group(name: &str) -> &'static str {
    match name {
        "analyze_audio"
        | "analyze_chords"
        | "analyze_instruments"
        | "transcribe_audio"
        | "transcribe_mixture" => "transcription",
        "write_lyrics" | "sing" | "compose_lyrics" => "vocals",
        "effects"
        | "automation"
        | "mixer"
        | "set_level"
        | "set_effect"
        | "section_gain"
        | "routing"
        | "set_instrument_param"
        | "convert_track_to_audio" => "mix",
        "spec_reference" | "check_spec" | "compose" | "edit_recipe" | "regenerate_clips"
        | "teach_progression" | "forget_progression" | "list_progressions" | "list_presets"
        | "add_part" | "accompany" => "composition",
        "create_project"
        | "import_audio"
        | "import_midi"
        | "setup_tracks"
        | "add_track"
        | "rename_track"
        | "remove_track"
        | "set_instrument"
        | "add_clip"
        | "edit_clip"
        | "notes"
        | "edit_notes"
        | "replace_notes"
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
/// Compact only presentation metadata; schema validation constraints are unchanged.
pub fn compact_parameters(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(serde_json::Value::String(text)) = object.get_mut("description") {
                *text = concise_description(text);
            }
            if let Some(properties) = object.get_mut("properties").and_then(|v| v.as_object_mut())
                && let Some(project) = properties.get_mut("project")
            {
                project["description"] =
                    "Project path or project_id from create_project/open_project.".into();
            }
            for child in object.values_mut() {
                compact_parameters(child);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                compact_parameters(child);
            }
        }
        _ => {}
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
                    .is_none_or(|g| g == tool_group(t.name))
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
            .map(|t| serde_json::json!({"name":t.name,"group":tool_group(t.name)}))
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
        })
        .unwrap_err();
        assert!(error.len() < 256);
    }
}
