//! Release-build baseline for the document-to-render pipeline.
//!
//! This is intentionally a small executable rather than a benchmark harness.  It reports the
//! costs that matter to the M0 pipeline contract: cloning the editable document, flattening it
//! into a render graph, and running already-prepared callback blocks.  Run it with
//! `cargo run -p auris-session --release --example pipeline_baseline`.

use auris_core::time::Ticks;
use auris_core::{AudioBuffer, AudioSourceBank, Note, PluginRegistry, Project};
use auris_dsp::DspPack;
use auris_engine::{RenderGraph, Transport, render_block};
use auris_synth::SynthPack;
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: all operations delegate to the system allocator; counters are independent atomics.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.load(Ordering::Relaxed) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(
                new_size.saturating_sub(layout.size()) as u64,
                Ordering::Relaxed,
            );
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct AllocationWindow;

impl AllocationWindow {
    fn start() -> Self {
        ALLOCATIONS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        DEALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        Self
    }

    fn result(&self) -> (u64, u64, u64) {
        (
            ALLOCATIONS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
            DEALLOCATIONS.load(Ordering::Relaxed),
        )
    }
}

impl Drop for AllocationWindow {
    fn drop(&mut self) {
        COUNTING.store(false, Ordering::Relaxed);
    }
}

fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>();
    registry.install::<DspPack>();
    registry
}

fn project(track_count: usize) -> Project {
    let mut project = Project::new("M0 pipeline baseline", 48_000.0);
    project.set_bpm(120.0);
    for track_index in 0..track_count {
        let track =
            project.add_instrument_track(format!("Part {track_index:02}"), "auris.synth.chiptune");
        let clip = project
            .add_midi_clip(track, "Pattern", Ticks::ZERO, Ticks::from_beats(64.0))
            .expect("instrument track accepts MIDI clips");
        let midi = project.midi_clip_mut(clip).expect("MIDI clip exists");
        for step in 0..256 {
            let pitch = [48, 55, 60, 67][step % 4];
            midi.notes.push(Note::new(
                pitch + (track_index % 5) as u8,
                Ticks::from_beats(step as f64 * 0.25),
                Ticks::from_beats(0.75),
            ));
        }
    }
    project.add_effect(None, "auris.fx.limiter");
    project
}

fn render_summary(project: &Project, registry: &PluginRegistry) -> (f32, f64, Vec<f32>) {
    let bank = AudioSourceBank::new();
    let mut graph = RenderGraph::build(project, &bank, registry, 256);
    assert!(graph.resource_error().is_none(), "graph resource error");
    let mut transport = Transport::playing_from(0);
    let mut block = AudioBuffer::new(2, 256, 48_000.0);
    let mut samples = Vec::with_capacity(256 * 2 * 128);
    for _ in 0..128 {
        render_block(&mut graph, &mut transport, &mut block, false);
        samples.extend_from_slice(block.channel(0));
        samples.extend_from_slice(block.channel(1));
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0, f32::max);
    let sum = samples.iter().map(|sample| f64::from(*sample)).sum();
    (peak, sum, samples)
}

fn measure_workload(track_count: usize, registry: &PluginRegistry) -> serde_json::Value {
    let project = project(track_count);
    let bank = AudioSourceBank::new();

    let mut clone_samples = Vec::new();
    let mut build_samples = Vec::new();
    for _ in 0..3 {
        let clone_start = Instant::now();
        let snapshot = project.clone();
        clone_samples.push(clone_start.elapsed().as_secs_f64() * 1_000_000.0);
        std::hint::black_box(snapshot);

        let build_start = Instant::now();
        let graph = RenderGraph::build(&project, &bank, registry, 256);
        build_samples.push(build_start.elapsed().as_secs_f64() * 1_000_000.0);
        std::hint::black_box(graph);
    }
    clone_samples.sort_by(f64::total_cmp);
    build_samples.sort_by(f64::total_cmp);

    let mut graph = RenderGraph::build(&project, &bank, registry, 256);
    assert!(graph.resource_error().is_none(), "graph resource error");
    let mut transport = Transport::playing_from(0);
    let mut block = AudioBuffer::new(2, 256, 48_000.0);
    let first_start = Instant::now();
    let first_window = AllocationWindow::start();
    render_block(&mut graph, &mut transport, &mut block, false);
    let first_elapsed_micros = first_start.elapsed().as_secs_f64() * 1_000_000.0;
    let (first_allocations, first_bytes, first_deallocations) = first_window.result();
    drop(first_window);

    let steady_start = Instant::now();
    let steady_window = AllocationWindow::start();
    for _ in 0..127 {
        render_block(&mut graph, &mut transport, &mut block, false);
    }
    let steady_elapsed_micros = steady_start.elapsed().as_secs_f64() * 1_000_000.0;
    let (steady_allocations, steady_bytes, steady_deallocations) = steady_window.result();
    drop(steady_window);

    let seek_loop_start = Instant::now();
    let seek_loop_window = AllocationWindow::start();
    for _ in 0..128 {
        transport.loop_enabled = true;
        transport.loop_start_frames = 1_024;
        transport.loop_end_frames = 2_048;
        transport.seek(1_900);
        render_block(&mut graph, &mut transport, &mut block, false);
        assert_eq!(transport.position_frames, 1_132);
    }
    let seek_loop_elapsed_micros = seek_loop_start.elapsed().as_secs_f64() * 1_000_000.0;
    let (seek_loop_allocations, seek_loop_bytes, seek_loop_deallocations) =
        seek_loop_window.result();
    drop(seek_loop_window);

    let (peak_a, sum_a, samples_a) = render_summary(&project, registry);
    let (peak_b, sum_b, samples_b) = render_summary(&project, registry);
    assert_eq!(
        samples_a, samples_b,
        "same-build render was not deterministic"
    );
    assert_eq!(peak_a, peak_b);
    assert_eq!(sum_a, sum_b);
    assert!(peak_a.is_finite() && sum_a.is_finite());
    assert!(peak_a > 0.0 && samples_a.iter().any(|sample| *sample != 0.0));
    assert!(samples_a.iter().all(|sample| sample.is_finite()));

    json!({
        "tracks": track_count,
        "notes_per_track": 256,
        "snapshot_clone": { "repetitions": 3, "median_microseconds": clone_samples[1] },
        "graph_build": { "repetitions": 3, "median_microseconds": build_samples[1] },
        "first_callback": { "microseconds": first_elapsed_micros, "allocations": first_allocations, "bytes": first_bytes, "deallocations": first_deallocations },
        "steady_callback": { "blocks": 127, "microseconds": steady_elapsed_micros, "allocations": steady_allocations, "bytes": steady_bytes, "deallocations": steady_deallocations },
        "seek_loop_callback": { "blocks": 128, "microseconds": seek_loop_elapsed_micros, "allocations": seek_loop_allocations, "bytes": seek_loop_bytes, "deallocations": seek_loop_deallocations, "wrap_start": 1900, "wrap_end": 2048, "wrapped_position": 1132 },
        "determinism": { "frames": samples_a.len() / 2, "peak": peak_a, "sum": sum_a, "nonzero": samples_a.iter().any(|sample| *sample != 0.0), "finite": samples_a.iter().all(|sample| sample.is_finite()) }
    })
}

fn main() {
    let registry = registry();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "auris.render_pipeline_baseline.v1",
            "sample_rate": 48000,
            "block_frames": 256,
            "instrument_id": "auris.synth.chiptune",
            "effect_id": "auris.fx.limiter",
            "workloads": [measure_workload(8, &registry), measure_workload(64, &registry)]
        }))
        .expect("baseline JSON serialization")
    );
}
