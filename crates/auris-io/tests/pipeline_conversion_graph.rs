use std::collections::BTreeMap;

use auris_core::pipeline::content::*;
use auris_core::pipeline::{ControlDomain, EdgeRole, NodeId, PipelineLimits};
use auris_core::{ParamDescriptor, ParamUnit, PluginKind, Project, TrackId};
use auris_io::{PipelineConversionError, project_to_native};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 1024,
        clips: 1024,
        stages: 1024,
        processors: 1024,
        groups: 1024,
        tracks: 1024,
        edges: 1024,
        external_ids: 1024,
        automations: 1024,
        automation_points: 1024,
        bindings: 1024,
    }
}

fn fixture() -> Project {
    serde_json::from_str(include_str!("fixtures/project_formats/v32.auris")).unwrap()
}

fn group(content: &PipelineContent, track: Option<TrackId>) -> &ProcessingGroup {
    let id = track.map_or(content.master, |track| {
        content
            .tracks
            .iter()
            .find(|binding| binding.track == track)
            .unwrap()
            .group
    });
    content.groups.iter().find(|group| group.id == id).unwrap()
}

fn sum(content: &PipelineContent, track: Option<TrackId>) -> NodeId {
    let group = group(content, track);
    content
        .processors
        .iter()
        .find(|node| node.group == Some(group.id) && matches!(node.kind, ProcessorKind::Sum))
        .unwrap()
        .id
}

#[test]
fn routes_inserts_sends_and_master_sidechains_keep_exact_endpoints_and_order() {
    let mut project = fixture();
    project.master.effects[0].sidechain = Some(project.tracks[0].id);
    let native = project_to_native(&project, limits()).unwrap();
    let content = &native.content;
    let commands = content
        .command_bindings(&native.external_ids(), limits())
        .unwrap();
    for (owner, strip) in std::iter::once((None, &project.master)).chain(
        project
            .tracks
            .iter()
            .map(|track| (Some(track.id), &track.mixer)),
    ) {
        let mut predecessor = sum(content, owner);
        for effect in &strip.effects {
            let node = commands.effect(owner, effect.id).unwrap();
            assert_eq!(
                content
                    .edges
                    .iter()
                    .filter(|edge| edge.to.0 == node && edge.to.1.0 == "audio_in")
                    .map(|edge| edge.from.0)
                    .collect::<Vec<_>>(),
                vec![predecessor]
            );
            if let Some(source) = effect.sidechain {
                assert!(
                    content
                        .edges
                        .iter()
                        .any(|edge| edge.from.0 == commands.strip(Some(source)).unwrap()
                            && edge.to.0 == node
                            && edge.role == EdgeRole::Sidechain)
                );
            }
            predecessor = node;
        }
        assert!(content.edges.iter().any(|edge| edge.from.0 == predecessor
            && edge.to.0 == group(content, owner).strip
            && edge.to.1.0 == "audio_in"));
    }
    for track in &project.tracks {
        let strip = commands.strip(Some(track.id)).unwrap();
        let expected: Vec<_> = track
            .outputs
            .iter()
            .map(|output| sum(content, output.bus()))
            .collect();
        let actual: Vec<_> = content
            .edges
            .iter()
            .filter(|edge| {
                edge.from.0 == strip && edge.from.1.0 == "audio_out" && edge.role == EdgeRole::Main
            })
            .filter(|edge| {
                content
                    .processors
                    .iter()
                    .any(|node| node.id == edge.to.0 && matches!(node.kind, ProcessorKind::Sum))
            })
            .map(|edge| edge.to.0)
            .collect();
        assert_eq!(actual, expected, "outputs for {:?}", track.id);
        for send in &track.sends {
            let node = commands.send(track.id, send.id).unwrap();
            assert!(content.edges.iter().any(|edge| edge.from.0 == strip
                && edge.from.1.0
                    == if send.pre_fader {
                        "pre_fader"
                    } else {
                        "audio_out"
                    }
                && edge.to.0 == node));
            assert!(content.edges.iter().any(|edge| edge.from.0 == node && edge.to.0 == sum(content, Some(send.target))));
        }
    }
    let output: Vec<_> = content
        .processors
        .iter()
        .filter(|node| matches!(node.kind, ProcessorKind::Output))
        .collect();
    assert_eq!(output.len(), 1);
    assert!(
        content
            .edges
            .iter()
            .any(|edge| edge.from.0 == group(content, None).strip && edge.to.0 == output[0].id)
    );
}

#[test]
fn instrument_and_singer_payloads_and_all_automation_bindings_survive() {
    let project = fixture();
    let native = project_to_native(&project, limits()).unwrap();
    let commands = native
        .content
        .command_bindings(&native.external_ids(), limits())
        .unwrap();
    for track in &project.tracks {
        let instrument = match &track.kind {
            auris_core::project::TrackKind::Instrument(inner)
            | auris_core::project::TrackKind::Drum(inner) => Some((
                &inner.instrument_id,
                &inner.instrument_state,
                inner.file.as_ref(),
            )),
            auris_core::project::TrackKind::Singer(inner) => {
                let node = native
                    .content
                    .processors
                    .iter()
                    .find(|node| {
                        node.group == Some(group(&native.content, Some(track.id)).id)
                            && matches!(node.kind, ProcessorKind::Singer(_))
                    })
                    .unwrap();
                let ProcessorKind::Singer(saved) = &node.kind else {
                    unreachable!()
                };
                assert_eq!(saved.frame_hop, inner.frame_hop);
                assert_eq!(saved.voice, inner.voice);
                assert_eq!(saved.take, inner.take);
                Some((&inner.instrument_id, &inner.instrument_state, None))
            }
            _ => None,
        };
        if let Some((id, state, file)) = instrument {
            let node = commands.instrument(track.id).unwrap();
            let ProcessorKind::Instrument(saved) = &native
                .content
                .processors
                .iter()
                .find(|p| p.id == node)
                .unwrap()
                .kind
            else {
                unreachable!()
            };
            assert_eq!(&saved.plugin_id, id);
            assert_eq!(&saved.state, state);
            assert_eq!(saved.file.as_ref(), file);
        }
    }
    assert_eq!(
        native.content.automation_bindings.len(),
        project.automation.lanes().len()
    );
    for (lane, binding) in project
        .automation
        .lanes()
        .iter()
        .zip(&native.content.automation_bindings)
    {
        let expected = commands.parameter(lane.target, lane.key()).unwrap();
        assert_eq!(binding.target, expected);
        assert!(binding.enabled);
        let node = native
            .content
            .processors
            .iter()
            .find(|node| node.id == binding.source)
            .unwrap();
        let ProcessorKind::Automation(id) = node.kind else {
            unreachable!()
        };
        let curve = native
            .content
            .automation
            .iter()
            .find(|curve| curve.id == id)
            .unwrap();
        assert_eq!(curve.points(), lane.points());
        assert_eq!(curve.curve, lane.curve);
    }
}

struct Metadata {
    id: String,
    kind: PluginKind,
    parameters: Vec<PluginParameter>,
}
struct Provider(BTreeMap<NodeId, Metadata>);
impl CapabilityProvider for Provider {
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>> {
        self.0
            .get(&processor.id)
            .map(|metadata| PluginCapabilities {
                plugin_id: &metadata.id,
                kind: metadata.kind,
                parameters: &metadata.parameters,
                sidechain: true,
            })
    }
}

#[test]
fn converted_graph_resolves_with_explicit_synthetic_fixture_capabilities() {
    let mut native = project_to_native(&fixture(), limits()).unwrap();
    // Test metadata proves topology, not availability or parameter ranges of real plugins.
    let domain = ControlDomain::Native {
        unit: ParamUnit::Plain,
        min: -100_000.0,
        max: 100_000.0,
    };
    for curve in &mut native.content.automation {
        if curve.domain.is_none() {
            curve.resolve_domain(domain).unwrap();
        }
    }
    let mut metadata = BTreeMap::new();
    for node in &native.content.processors {
        let (instance, kind) = match &node.kind {
            ProcessorKind::Instrument(instance) => (instance, PluginKind::Instrument),
            ProcessorKind::Effect { plugin, .. } => (plugin, PluginKind::Effect),
            _ => continue,
        };
        let parameters = native
            .content
            .automation_bindings
            .iter()
            .filter(|binding| binding.target.node == node.id)
            .enumerate()
            .map(|(index, binding)| PluginParameter {
                descriptor: {
                    let mut descriptor = ParamDescriptor::new(
                        index as u32,
                        "fixture",
                        "Fixture",
                        -100_000.0,
                        100_000.0,
                        0.0,
                    );
                    descriptor.key = binding.target.key.clone().into();
                    descriptor
                },
                domain,
                policy: auris_core::pipeline::AutomationPolicy::Continuous,
            })
            .collect();
        metadata.insert(
            node.id,
            Metadata {
                id: instance.plugin_id.clone(),
                kind,
                parameters,
            },
        );
    }
    let provider = Provider(metadata);
    let resolved = native
        .content
        .resolve_pipeline(
            &native.external_ids(),
            limits(),
            PipelineLimits {
                nodes: 1024,
                edges: 1024,
                ports: 4096,
                parameter_keys: 4096,
                text_bytes: 65536,
            },
            &provider,
        )
        .unwrap();
    assert_eq!(
        resolved.topological_order().len(),
        native.content.processors.len()
    );
    assert_eq!(
        resolved.pipeline().edges.len(),
        native.content.edges.len() + native.content.automation_bindings.len()
    );
}

#[test]
fn missing_plugin_key_and_exhaustion_preserve_the_original_input() {
    let original = serde_json::to_value(fixture()).unwrap();
    let mut missing = original.clone();
    missing["automation"][1]
        .as_object_mut()
        .unwrap()
        .remove("key");
    let project: Project = serde_json::from_value(missing).unwrap();
    let before = project.clone();
    assert!(matches!(
        project_to_native(&project, limits()),
        Err(PipelineConversionError::MissingPluginKey { .. })
    ));
    assert_eq!(project, before);
    let mut exhausted = original;
    exhausted["next_id"] = serde_json::json!(u64::MAX - 2);
    let project: Project = serde_json::from_value(exhausted).unwrap();
    let before = project.clone();
    assert!(matches!(
        project_to_native(&project, limits()),
        Err(PipelineConversionError::IdExhausted)
    ));
    assert_eq!(project, before);
}

#[test]
fn stale_counter_repairs_in_candidate_and_deleted_identity_history_is_retained() {
    for next in [0, 10_000] {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["next_id"] = serde_json::json!(next);
        let project: Project = serde_json::from_value(value).unwrap();
        let before = project.clone();
        let native = project_to_native(&project, limits()).unwrap();
        native.validate_ownership(limits()).unwrap();
        assert!(
            native
                .content
                .processors
                .iter()
                .all(|node| node.id.0 >= project.allocation_start().unwrap())
        );
        assert_eq!(project, before);
    }
}

#[test]
fn exact_preflight_budgets_cover_every_allocated_native_object() {
    let project = fixture();
    let native = project_to_native(&project, limits()).unwrap();
    let mut exact = limits();
    exact.processors = native.content.processors.len();
    exact.edges = native.content.edges.len();
    exact.bindings = native.content.automation_bindings.len();
    assert_eq!(project_to_native(&project, exact).unwrap(), native);
    for field in ["processors", "edges", "bindings"] {
        let mut too_small = exact;
        match field {
            "processors" => too_small.processors -= 1,
            "edges" => too_small.edges -= 1,
            _ => too_small.bindings -= 1,
        }
        assert!(
            matches!(project_to_native(&project, too_small), Err(PipelineConversionError::Ownership(ContentError::Limit(name))) if name == field)
        );
    }
}
