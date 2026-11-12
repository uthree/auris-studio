//! Runtime-scoped discovery, help and dispatch from the same execution registry.

use crate::{ToolGroups, tool_catalog, tool_group};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn schema_hash(parameters: &Value) -> String {
    let mut parameters = parameters.clone();
    crate::compact_parameters(&mut parameters);
    hex::encode(Sha256::digest(parameters.to_string().as_bytes()))
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

/// Reference tools available through every live transport.
pub fn live_references() -> Vec<crate::live_agent::Definition> {
    [
        ("read_report", crate::read_report::DESCRIPTION, crate::parameter_schema::<crate::read_report::Args>()),
        ("search_documentation", crate::search_documentation::DESCRIPTION, crate::parameter_schema::<crate::search_documentation::Args>()),
        ("discover_tools", crate::discover_tools::DESCRIPTION, crate::parameter_schema::<crate::discover_tools::Args>()),
        ("tool_help", crate::tool_help::DESCRIPTION, crate::parameter_schema::<crate::tool_help::Args>()),
        ("capabilities", "Read the current live tool catalog and supported task groups. No project or file edits.", crate::parameter_schema::<Empty>()),
    ].into_iter().map(|(name, description, parameters)| crate::live_agent::Definition {
        name: name.into(), description: description.into(), parameters,
    }).collect()
}

/// Read a live reference without opening or changing a saved project.
pub fn call_live_reference(name: &str, args: Value) -> Option<Result<String, String>> {
    call_live_reference_in(name, args, &crate::live_agent::definitions())
}

/// Read reference metadata against exactly the host's available execution catalog.
pub fn call_live_reference_in(
    name: &str,
    args: Value,
    catalog: &[crate::live_agent::Definition],
) -> Option<Result<String, String>> {
    let run = || -> Result<String, String> {
        let invalid = |e: serde_json::Error| format!("Invalid arguments for {name}: {e}");
        match name {
            "read_report" => {
                crate::read_report::run(&serde_json::from_value(args).map_err(invalid)?)
            }
            "search_documentation" => {
                crate::search_documentation::run(&serde_json::from_value(args).map_err(invalid)?)
            }
            "capabilities" => {
                let _: Empty = serde_json::from_value(args).map_err(invalid)?;
                let audio_input_contract =
                    auris_session::audio_input::AudioInputContract::from_env()?;
                let names: Vec<_> = catalog.iter().map(|d| d.name.as_str()).collect();
                let schemas:Vec<_>=catalog.iter().map(|d|json!({"name":d.name,"group":tool_group(&d.name),"schema_sha256":schema_hash(&d.parameters)})).collect();
                let catalog_sha256 = schema_hash(&json!(schemas));
                // No feature flags: whether an optional tool is on is whether `tools` lists it.
                // A bare `"arrangement": false` read to a model as "arranging is unavailable",
                // and one stopped composing because of it.
                Ok(json!({"context":"open_document","tools":names,"tool_schemas":schemas,"catalog_sha256":catalog_sha256,"audio_input_contract":audio_input_contract,
                    "groups":catalog.iter().map(|d|tool_group(&d.name)).collect::<std::collections::BTreeSet<_>>()}).to_string())
            }
            "tool_help" => {
                let request: crate::tool_help::Args =
                    serde_json::from_value(args).map_err(invalid)?;
                let definition = catalog
                    .iter()
                    .find(|d| d.name == request.name)
                    .ok_or_else(|| format!("Unknown live tool: {}", request.name))?;
                crate::catalog::tool_help::reference(
                    &request,
                    &definition.name,
                    &definition.description,
                    &definition.parameters,
                    true,
                )
            }
            "discover_tools" => {
                let request: crate::discover_tools::Args =
                    serde_json::from_value(args).map_err(invalid)?;
                if request.group.as_deref().is_some_and(|g| {
                    ![
                        "common",
                        "manual",
                        "mix",
                        "transcription",
                        "vocals",
                        "composition",
                    ]
                    .contains(&g)
                }) {
                    return Err("Unknown task group".into());
                }
                let query = request.query.unwrap_or_default().to_lowercase();
                let definitions: Vec<_> = catalog
                    .iter()
                    .filter(|d| {
                        request
                            .group
                            .as_deref()
                            .is_none_or(|g| tool_group(&d.name) == g)
                            && query.split_whitespace().all(|word| {
                                format!("{} {}", d.name, d.description)
                                    .to_lowercase()
                                    .contains(word)
                            })
                    })
                    .collect();
                let tools: Vec<_> = definitions.iter().skip(request.offset).take(10).map(|d| json!({"name":d.name,"group":tool_group(&d.name),"description":crate::concise_description(&d.description)})).collect();
                let next = request.offset.saturating_add(tools.len());
                Ok(json!({"tools":tools,"total":definitions.len(),"next_offset":(next < definitions.len()).then_some(next)}).to_string())
            }
            _ => unreachable!(),
        }
    };
    matches!(
        name,
        "search_documentation" | "tool_help" | "discover_tools" | "capabilities" | "read_report"
    )
    .then(run)
}

/// Execute an enabled saved-project tool. Disabled and unknown names fail before dispatch.
pub fn call_saved(groups: &ToolGroups, name: &str, args: Value) -> Result<String, String> {
    let catalog = tool_catalog();
    let definition = catalog
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| format!("Unknown tool: {name}"))?;
    if !groups.allows(name) {
        return Err(format!(
            "Tool {name} is disabled in this server's startup groups"
        ));
    }
    match name {
        "capabilities" => {
            let request: crate::capabilities::Args =
                serde_json::from_value(args).map_err(|e| format!("Invalid arguments: {e}"))?;
            let mut value: Value = serde_json::from_str(&crate::capabilities::run(&request)?)
                .map_err(|e| e.to_string())?;
            value["tools"] = json!(
                catalog
                    .iter()
                    .filter(|tool| groups.allows(&tool.name))
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>()
            );
            value["context"] = "saved_project".into();
            value["audio_input_contract"] =
                json!(auris_session::audio_input::AudioInputContract::from_env()?);
            value["tool_schemas"] = json!(catalog.iter().filter(|tool|groups.allows(&tool.name)).map(|tool|json!({"name":tool.name,"group":tool_group(&tool.name),"schema_sha256":schema_hash(&tool.parameters)})).collect::<Vec<_>>());
            value["catalog_sha256"] = schema_hash(&value["tool_schemas"]).into();
            Ok(value.to_string())
        }
        "discover_tools" => discover_saved(
            groups,
            serde_json::from_value(args).map_err(|e| format!("Invalid arguments: {e}"))?,
        ),
        "tool_help" => {
            let request: crate::tool_help::Args =
                serde_json::from_value(args).map_err(|e| format!("Invalid arguments: {e}"))?;
            if !groups.allows(&request.name) {
                return Err(format!(
                    "Tool {} is disabled; enable group '{}' in AURIS_MCP_TOOL_GROUPS and restart",
                    request.name,
                    tool_group(&request.name)
                ));
            }
            crate::tool_help::run(&request)
        }
        _ => (definition.call)(args),
    }
}

fn discover_saved(
    groups: &ToolGroups,
    args: crate::discover_tools::Args,
) -> Result<String, String> {
    if args.group.as_deref().is_some_and(|g| {
        ![
            "common",
            "manual",
            "mix",
            "transcription",
            "vocals",
            "composition",
        ]
        .contains(&g)
    }) {
        return Err("Unknown task group".into());
    }
    let query = args.query.unwrap_or_default().to_lowercase();
    let matches: Vec<_> = tool_catalog()
        .into_iter()
        .filter(|tool| {
            groups.allows(&tool.name)
                && args
                    .group
                    .as_deref()
                    .is_none_or(|group| tool_group(&tool.name) == group)
                && query.split_whitespace().all(|word| {
                    format!("{} {}", tool.name, tool.description)
                        .to_lowercase()
                        .contains(word)
                })
        })
        .collect();
    let tools: Vec<_> = matches.iter().skip(args.offset).take(10).map(|tool| json!({"name":tool.name,"group":tool_group(&tool.name),"description":crate::concise_description(&tool.description)})).collect();
    let next = args.offset.saturating_add(tools.len());
    Ok(json!({"tools":tools,"total":matches.len(),"next_offset":(next < matches.len()).then_some(next)}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_hashes_match_the_transmitted_saved_and_live_schemas() {
        let live = crate::live_agent::definitions();
        let live_capabilities: Value = serde_json::from_str(
            &call_live_reference_in("capabilities", json!({}), &live)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        // An optional tool is on exactly when `tools` lists it; there is no separate flag for
        // a model to read as a missing feature.
        assert!(live_capabilities.get("feature_flags").is_none());
        let groups = ToolGroups::default();
        let saved_capabilities: Value =
            serde_json::from_str(&call_saved(&groups, "capabilities", json!({})).unwrap()).unwrap();
        for (report, catalog) in [
            (
                live_capabilities,
                live.into_iter()
                    .map(|tool| (tool.name, tool.parameters))
                    .collect::<Vec<_>>(),
            ),
            (
                saved_capabilities,
                tool_catalog()
                    .into_iter()
                    .map(|tool| (tool.name, tool.parameters))
                    .collect(),
            ),
        ] {
            for (name, mut transmitted) in catalog {
                // The MCP tools/list and native tool templates use this public wire projection.
                crate::compact_parameters(&mut transmitted);
                let expected = hex::encode(Sha256::digest(transmitted.to_string().as_bytes()));
                let contract = report["tool_schemas"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|tool| tool["name"] == name)
                    .unwrap();
                assert_eq!(contract["schema_sha256"], expected, "{name}");
            }
        }
    }

    #[test]
    fn optional_host_tools_are_shared_by_capabilities_discovery_and_full_help() {
        for enabled in [false, true] {
            let mut catalog = crate::live_agent::definitions();
            if enabled {
                catalog.push(crate::live_agent::Definition {
                    name: crate::internet_search::NAME.into(),
                    description: crate::internet_search::DESCRIPTION.into(),
                    parameters: crate::parameter_schema::<crate::internet_search::Args>(),
                });
            }
            let capabilities: Value = serde_json::from_str(
                &call_live_reference_in("capabilities", json!({}), &catalog)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                capabilities["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|name| name == "search_internet"),
                enabled
            );
            let discovery: Value = serde_json::from_str(
                &call_live_reference_in(
                    "discover_tools",
                    json!({"query":"search_internet"}),
                    &catalog,
                )
                .unwrap()
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                discovery["tools"].as_array().unwrap().len(),
                usize::from(enabled)
            );
            let help =
                call_live_reference_in("tool_help", json!({"name":"search_internet"}), &catalog)
                    .unwrap();
            assert_eq!(help.is_ok(), enabled);
        }
    }

    #[test]
    fn live_discovery_help_and_capabilities_use_the_live_definitions() {
        let report: Value = serde_json::from_str(
            &call_live_reference("capabilities", json!({}))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let names = report["tools"].as_array().unwrap();
        assert!(names.contains(&json!("search_documentation")));
        assert!(!names.contains(&json!("compose")));
        let help: Value = serde_json::from_str(
            &call_live_reference("tool_help", json!({"name":"set_performance"}))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(help["parameters"]["properties"].get("project").is_none());
        assert!(
            call_live_reference("tool_help", json!({"name":"compose"}))
                .unwrap()
                .is_err()
        );
        assert!(
            call_live_reference("capabilities", json!({"project":"/song.auris"}))
                .unwrap()
                .is_err()
        );
    }
}
