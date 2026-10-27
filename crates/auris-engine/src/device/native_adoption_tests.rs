use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use super::*;
use crate::command::EngineCommand;
use crate::graph::{RETIRED_GRAPH_SLOTS, RenderGraph};
use crate::handle::Retired;
use crate::native::{
    NativeRenderInput, NativeRenderTarget, NodePlacedEffects, NodePlacedInstruments,
};
use crate::testkit;
use auris_core::pipeline::NodeId;
use auris_core::project::{AudioSourceBank, Project};

const SAMPLE_RATE: f64 = 48_000.0;

fn native_graph() -> Box<RenderGraph> {
    let input = NativeRenderInput::new(
        Project::default(),
        vec![(NodeId(1), NativeRenderTarget::Strip(None))],
        BTreeMap::new(),
    )
    .expect("native graph input");
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    Box::new(
        input
            .build(
                &AudioSourceBank::new(),
                &testkit::registry(),
                &mut effects,
                &mut instruments,
                256,
                SAMPLE_RATE,
            )
            .expect("native graph"),
    )
}

fn legacy_graph() -> Box<RenderGraph> {
    Box::new(RenderGraph::build(
        &Project::default(),
        &AudioSourceBank::new(),
        &testkit::registry(),
        256,
    ))
}

fn callback_engine(command_capacity: usize, return_capacity: usize) -> (AudioEngine, EngineHandle) {
    let (command_tx, command_rx) = crossbeam_channel::bounded(command_capacity);
    let (return_tx, return_rx) = crossbeam_channel::bounded(return_capacity);
    let adopted = Arc::new(AtomicU64::new(0));
    let engine = AudioEngine::new(
        command_rx,
        return_tx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(MeterBank::new(0)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::clone(&adopted),
        SAMPLE_RATE,
        2,
        256,
    );
    let handle = EngineHandle {
        commands: command_tx,
        returned_graphs: return_rx,
        playhead: Arc::clone(&engine.playhead),
        count_in: Arc::clone(&engine.count_in),
        meters: Arc::clone(&engine.meters),
        running: Arc::new(AtomicBool::new(true)),
        playing: Arc::clone(&engine.playing),
        latency_stale: Arc::clone(&engine.latency_stale),
        adopted_native_generation: adopted,
        stream_notices: Arc::new(AtomicU32::new(0)),
        clock: Arc::clone(&engine.clock),
        callbacks: Arc::clone(&engine.callbacks),
        sample_rate: SAMPLE_RATE,
        channel_count: 2,
        max_block: 256,
    };
    (engine, handle)
}

#[test]
fn native_generation_is_published_only_after_callback_adoption() {
    let (mut engine, handle) = callback_engine(8, 8);
    let graph = native_graph();
    let generation = graph.native_generation().expect("prepared generation");
    handle.set_graph(*graph).unwrap();
    assert_eq!(handle.adopted_native_generation(), None);
    engine.fill(&mut [0.0; 64]);
    assert_eq!(handle.adopted_native_generation(), Some(generation));
}

#[test]
fn pending_replacement_keeps_the_previous_generation_until_next_callback() {
    let (mut engine, handle) = callback_engine(8, 8);
    handle.set_graph(*native_graph()).unwrap();
    engine.fill(&mut [0.0; 64]);
    let first = handle.adopted_native_generation();
    let next = native_graph();
    let next_generation = next.native_generation();
    handle.set_graph(*next).unwrap();
    assert_eq!(handle.adopted_native_generation(), first);
    engine.fill(&mut [0.0; 64]);
    assert_eq!(handle.adopted_native_generation(), next_generation);
    assert_ne!(next_generation, first);
}

#[test]
fn legacy_replacement_clears_an_adopted_native_generation() {
    let (mut engine, handle) = callback_engine(8, 8);
    handle.set_graph(*native_graph()).unwrap();
    engine.fill(&mut [0.0; 64]);
    assert!(handle.adopted_native_generation().is_some());
    handle.set_graph(*legacy_graph()).unwrap();
    assert!(handle.adopted_native_generation().is_some());
    engine.fill(&mut [0.0; 64]);
    assert_eq!(handle.adopted_native_generation(), None);
}

#[test]
fn full_command_queue_does_not_publish_a_rejected_native_graph() {
    let (mut engine, handle) = callback_engine(1, 8);
    handle.set_graph(*native_graph()).unwrap();
    engine.fill(&mut [0.0; 64]);
    let old = handle.adopted_native_generation();
    handle.send(EngineCommand::Play).unwrap();
    assert!(matches!(
        handle.set_graph(*native_graph()),
        Err(EngineError::CommandQueueFull)
    ));
    engine.fill(&mut [0.0; 64]);
    assert_eq!(handle.adopted_native_generation(), old);
}

#[test]
fn full_retirement_stash_defers_native_replacement_and_keeps_old_generation() {
    let (mut engine, handle) = callback_engine(8, 1);
    handle.set_graph(*native_graph()).unwrap();
    engine.fill(&mut [0.0; 64]);
    let old = handle.adopted_native_generation();
    engine
        .returned_graphs
        .send(Retired::SoloResolution(Vec::new().into_boxed_slice()))
        .unwrap();
    for _ in 0..RETIRED_GRAPH_SLOTS {
        engine
            .retired
            .push(Retired::SoloResolution(Vec::new().into_boxed_slice()));
    }
    let next = native_graph();
    let next_generation = next.native_generation();
    handle.set_graph(*next).unwrap();
    assert_eq!(
        testkit::count_heap_operations(|| engine.fill(&mut [0.0; 64])),
        (0, 0)
    );
    assert_eq!(handle.adopted_native_generation(), old);
    assert!(engine.deferred_retiring.is_some());
    assert_eq!(handle.collect_garbage(), 1);
    engine.fill(&mut [0.0; 64]);
    assert_eq!(handle.adopted_native_generation(), next_generation);
}

#[test]
fn native_adoption_acknowledgment_does_not_allocate_in_the_callback() {
    let (mut engine, handle) = callback_engine(8, 8);
    handle.set_graph(*native_graph()).unwrap();
    let mut data = [0.0; 64];
    assert_eq!(
        testkit::count_heap_operations(|| engine.fill(&mut data)),
        (0, 0)
    );
    assert!(handle.adopted_native_generation().is_some());
}

#[test]
fn silent_engine_never_acknowledges_a_queued_native_graph() {
    let (mut device, handle) = start_silent(&AudioSettings::default());
    handle.set_graph(*native_graph()).unwrap();
    assert_eq!(handle.adopted_native_generation(), None);
    assert_eq!(device.discard_pending(), 1);
    assert_eq!(handle.adopted_native_generation(), None);
}
