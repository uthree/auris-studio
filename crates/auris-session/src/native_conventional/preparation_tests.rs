//! Integration checks for the session→engine native render preparation boundary.

use super::preparation::prepare_native_conventional;
use super::tests::{
    builtin_provider, content_limits, fixture_registry, graph_limits, instrument_fixture,
};
use auris_core::pipeline::AutomationPolicy;
use auris_core::project::AudioSourceBank;
use auris_engine::native::{NativeRenderTarget, NodePlacedEffects, NodePlacedInstruments};
use auris_engine::{OfflineOptions, RenderProgress, render_project};

#[test]
fn preparation_exposes_mutable_addresses_without_exposing_project() {
    let project = instrument_fixture();
    let document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    let provider = builtin_provider(&document);
    let prepared =
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .expect("native preparation");
    for binding in &document.content.tracks {
        let group = document
            .content
            .groups
            .iter()
            .find(|group| group.id == binding.group)
            .expect("track group");
        assert!(prepared.addresses().address(group.strip).is_some());
        if let Some(instrument) = group.primary_instrument {
            assert!(prepared.addresses().address(instrument).is_some());
        }
    }
}

#[test]
fn preparation_build_consumes_only_validated_node_placements() {
    let project = instrument_fixture();
    let document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    let provider = builtin_provider(&document);
    let prepared =
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .expect("native preparation");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    let track = project.tracks[0].id;
    let instrument_node = prepared
        .addresses()
        .node_for_target(NativeRenderTarget::Instrument(track))
        .expect("instrument address");
    instruments.insert(
        instrument_node,
        fixture_registry()
            .create_instrument(auris_synth::Chiptune::ID)
            .expect("fixture instrument"),
    );
    let graph = prepared
        .build(
            &AudioSourceBank::new(),
            &fixture_registry(),
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .expect("native graph");
    let _ = graph;
    assert!(effects.is_empty());
    assert!(instruments.is_empty());
}

#[test]
fn prepared_offline_render_matches_the_legacy_renderer_sample_for_sample() {
    let project = instrument_fixture();
    let document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    let provider = builtin_provider(&document);
    let prepared =
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .expect("native preparation");
    let options = OfflineOptions {
        end_frames: Some(4_096),
        include_tail: false,
        block_frames: 128,
        ..OfflineOptions::default()
    };
    let legacy = render_project(
        &project,
        &AudioSourceBank::new(),
        &fixture_registry(),
        &options,
    )
    .expect("legacy render");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    let mut native = prepared
        .offline(
            &AudioSourceBank::new(),
            &fixture_registry(),
            &mut effects,
            &mut instruments,
            &options,
        )
        .expect("native render");
    let mut buffer = native.buffer().expect("native buffer");
    let mut progress = RenderProgress::default();
    native
        .render(&mut buffer, &mut progress)
        .expect("native pass");
    assert_eq!(legacy.frame_count(), buffer.frame_count());
    for channel in 0..2 {
        assert!(legacy.channel_peak(channel) > 0.0);
        assert_eq!(
            legacy.channel(channel),
            buffer.channel(channel),
            "native parity channel {channel}"
        );
    }
}

#[test]
fn reordered_native_presentation_keeps_stable_node_addresses() {
    let mut project = instrument_fixture();
    project.add_instrument_track("Second", auris_synth::Chiptune::ID);
    let mut document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    document.track_presentation.reverse();
    let provider = builtin_provider(&document);
    let prepared =
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .expect("native preparation");
    for (index, presentation) in document.track_presentation.iter().enumerate() {
        let binding = document
            .content
            .tracks
            .iter()
            .find(|binding| binding.track == presentation.id)
            .expect("binding");
        let strip = document
            .content
            .groups
            .iter()
            .find(|group| group.id == binding.group)
            .expect("group")
            .strip;
        assert_eq!(
            prepared.addresses().address(strip),
            Some(auris_engine::native::RuntimeNodeAddress::Strip { track: Some(index) })
        );
    }
}

#[test]
fn failed_preparation_does_not_mutate_the_native_document() {
    let project = instrument_fixture();
    let mut document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    document.track_presentation[0].id = auris_core::TrackId(999_999);
    let before = document.clone();
    let provider = builtin_provider(&document);
    assert!(
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .is_err()
    );
    assert_eq!(document, before);
}

#[test]
fn preparation_preserves_plugin_automation_policy_at_the_engine_boundary() {
    let project = instrument_fixture();
    let track = project.tracks[0].id;
    let document =
        auris_io::project_to_native(&project, content_limits()).expect("native conversion");
    let mut provider = builtin_provider(&document);
    let instrument_node = document
        .content
        .groups
        .iter()
        .find_map(|group| {
            group
                .primary_instrument
                .map(|instrument| (instrument, group.id))
        })
        .and_then(|(instrument, group)| {
            document
                .content
                .tracks
                .iter()
                .find(|binding| binding.track == track && binding.group == group)
                .map(|_| instrument)
        })
        .expect("instrument node");
    let key = provider
        .set_policy_for_node(instrument_node, AutomationPolicy::Continuous)
        .expect("instrument parameter");
    let continuous =
        prepare_native_conventional(&document, &provider, content_limits(), graph_limits())
            .expect("continuous preparation");
    let node = continuous
        .addresses()
        .node_for_target(NativeRenderTarget::Instrument(track))
        .expect("instrument target");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    let graph = continuous
        .build(
            &AudioSourceBank::new(),
            &fixture_registry(),
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .expect("continuous graph");
    assert!(graph.native_parameter(node, &key).is_some());

    let mut preparation_only = builtin_provider(&document);
    let key = preparation_only
        .set_policy_for_node(instrument_node, AutomationPolicy::PreparationOnly)
        .expect("instrument parameter");
    let prepared = prepare_native_conventional(
        &document,
        &preparation_only,
        content_limits(),
        graph_limits(),
    )
    .expect("preparation-only preparation");
    let node = prepared
        .addresses()
        .node_for_target(NativeRenderTarget::Instrument(track))
        .expect("instrument target");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    let graph = prepared
        .build(
            &AudioSourceBank::new(),
            &fixture_registry(),
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .expect("preparation-only graph");
    assert!(graph.native_parameter(node, &key).is_none());
}
