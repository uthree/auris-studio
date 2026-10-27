//! Negative representability tests for the M2 conventional lowering bridge.
//!
//! These cases keep the native ownership model valid where possible, then add a graph shape that
//! the one-chain legacy renderer cannot represent.  The bridge must reject the candidate without
//! mutating it.

use auris_core::pipeline::content::{
    CapabilityProvider, ContentLimits, PluginCapabilities, PluginInstance, Processor, ProcessorKind,
};
use auris_core::pipeline::document::NativeTrackKind;
use auris_core::pipeline::{EdgeId, NodeId, PipelineLimits, PortId};
use auris_core::project::{EffectSlotId, Project};
use auris_core::{PluginKind, PluginState};

struct NoPlugins;

impl CapabilityProvider for NoPlugins {
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>> {
        match &processor.kind {
            ProcessorKind::Effect { plugin, .. } if plugin.plugin_id == "synthetic-effect" => {
                Some(PluginCapabilities {
                    plugin_id: "synthetic-effect",
                    kind: PluginKind::Effect,
                    parameters: &[],
                    sidechain: false,
                })
            }
            _ => None,
        }
    }
}

struct InstrumentsOnly;

impl CapabilityProvider for InstrumentsOnly {
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>> {
        let ProcessorKind::Instrument(instance) = &processor.kind else {
            return None;
        };
        Some(PluginCapabilities {
            plugin_id: if instance.plugin_id == "fixture.singer" {
                "fixture.singer"
            } else {
                return None;
            },
            kind: PluginKind::Instrument,
            parameters: &[],
            sidechain: false,
        })
    }
}

fn content_limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 1024,
        stages: 4096,
        processors: 4096,
        groups: 256,
        tracks: 256,
        edges: 8192,
        external_ids: 1024,
        automations: 256,
        automation_points: 8192,
        bindings: 1024,
    }
}

fn graph_limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 4096,
        edges: 8192,
        ports: 16384,
        parameter_keys: 1024,
        text_bytes: 1 << 20,
    }
}

fn empty_native() -> auris_core::pipeline::document::NativeDocument {
    auris_io::project_to_native(&Project::new("shape", 48_000.0), content_limits()).unwrap()
}

fn assert_rejected_unchanged(document: auris_core::pipeline::document::NativeDocument) {
    let before = document.clone();
    document
        .content
        .resolve_pipeline(
            &document.external_ids(),
            content_limits(),
            graph_limits(),
            &NoPlugins,
        )
        .expect("native shape should resolve before representability check");
    assert!(matches!(
        super::lower_native_conventional(&document, &NoPlugins, content_limits(), graph_limits()),
        Err(super::NativeConventionalError::Unsupported(_))
    ));
    assert_eq!(document, before);
}

#[test]
fn disconnected_master_output_is_not_a_conventional_render() {
    let mut document = empty_native();
    let master_strip = document
        .content
        .groups
        .iter()
        .find(|group| group.id == document.content.master)
        .unwrap()
        .strip;
    let output = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(processor.kind, ProcessorKind::Output))
        .unwrap()
        .id;
    document.content.edges.retain(|edge| {
        !(edge.from == (master_strip, PortId::new("audio_out"))
            && edge.to == (output, PortId::new("audio_in")))
    });
    document
        .validate_ownership(content_limits())
        .expect("ownership remains valid; this is a topology shape");
    assert_rejected_unchanged(document);
}

#[test]
fn two_master_outputs_are_not_silently_collapsed() {
    let mut document = empty_native();
    let master_strip = document
        .content
        .groups
        .iter()
        .find(|group| group.id == document.content.master)
        .unwrap()
        .strip;
    let output = NodeId(document.content.next_id);
    document.content.next_id += 1;
    document.content.processors.push(Processor {
        id: output,
        group: Some(document.content.master),
        kind: ProcessorKind::Output,
    });
    let mut duplicate = document
        .content
        .edges
        .iter()
        .find(|edge| edge.from == (master_strip, PortId::new("audio_out")))
        .cloned()
        .unwrap();
    duplicate.id = EdgeId(document.content.next_id);
    document.content.next_id += 1;
    duplicate.to = (output, PortId::new("audio_in"));
    document.content.edges.push(duplicate);
    document
        .validate_ownership(content_limits())
        .expect("ownership remains valid; duplicate output is a topology shape");
    assert_rejected_unchanged(document);
}

#[test]
fn an_extra_disconnected_sum_is_not_dropped() {
    let mut document = empty_native();
    let id = NodeId(document.content.next_id);
    document.content.next_id += 1;
    document.content.processors.push(Processor {
        id,
        group: Some(document.content.master),
        kind: ProcessorKind::Sum,
    });
    document
        .validate_ownership(content_limits())
        .expect("ownership remains valid; orphan processor is representability-only");
    assert_rejected_unchanged(document);
}

#[test]
fn an_orphan_effect_with_stable_identity_is_not_dropped() {
    let mut document = empty_native();
    let id = NodeId(document.content.next_id);
    document.content.next_id += 1;
    let slot = EffectSlotId(document.content.next_id);
    document.content.next_id += 1;
    document.content.processors.push(Processor {
        id,
        group: Some(document.content.master),
        kind: ProcessorKind::Effect {
            slot,
            plugin: PluginInstance {
                plugin_id: "synthetic-effect".into(),
                state: PluginState::default(),
                file: None,
            },
            enabled: true,
        },
    });
    document
        .validate_ownership(content_limits())
        .expect("ownership remains valid; orphan effect is representability-only");
    assert_rejected_unchanged(document);
}

#[test]
fn singer_graph_mislabeled_as_instrument_is_not_lowered_as_plain_instrument() {
    let mut project = Project::new("singer shape", 48_000.0);
    let track = project.add_singer_track("Singer", "fixture.singer");
    project
        .add_midi_clip(
            track,
            "Notes",
            auris_core::Ticks::ZERO,
            auris_core::Ticks::QUARTER,
        )
        .expect("singer clip");
    let mut document = auris_io::project_to_native(&project, content_limits()).unwrap();
    document
        .track_presentation
        .iter_mut()
        .find(|presentation| presentation.id == track)
        .expect("singer presentation")
        .kind = NativeTrackKind::Instrument;
    document
        .validate_ownership(content_limits())
        .expect("presentation kind is ownership-valid");
    document
        .content
        .resolve_pipeline(
            &document.external_ids(),
            content_limits(),
            graph_limits(),
            &InstrumentsOnly,
        )
        .expect("singer topology resolves before representability check");
    let before = document.clone();
    assert!(matches!(
        super::lower_native_conventional(
            &document,
            &InstrumentsOnly,
            content_limits(),
            graph_limits(),
        ),
        Err(super::NativeConventionalError::Unsupported(_))
    ));
    assert_eq!(document, before);
}

#[test]
fn orphan_singer_on_audio_track_is_not_counted_as_consumed() {
    let mut project = Project::new("audio shape", 48_000.0);
    project.add_audio_track("Audio");
    let mut document = auris_io::project_to_native(&project, content_limits()).unwrap();
    let group = document.content.tracks[0].group;
    let singer = NodeId(document.content.next_id);
    document.content.next_id += 1;
    document.content.processors.push(Processor {
        id: singer,
        group: Some(group),
        kind: ProcessorKind::Singer(Box::new(auris_core::pipeline::content::SingerInstance {
            frame_hop: 0.01,
            voice: None,
            take: None,
        })),
    });
    document
        .validate_ownership(content_limits())
        .expect("orphan singer ownership is valid");
    document
        .content
        .resolve_pipeline(
            &document.external_ids(),
            content_limits(),
            graph_limits(),
            &NoPlugins,
        )
        .expect("orphan singer topology resolves before representability check");
    let before = document.clone();
    assert!(matches!(
        super::lower_native_conventional(&document, &NoPlugins, content_limits(), graph_limits()),
        Err(super::NativeConventionalError::Unsupported(_))
    ));
    assert_eq!(document, before);
}
