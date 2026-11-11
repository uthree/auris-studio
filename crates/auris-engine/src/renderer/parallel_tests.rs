//! Exactness and release benchmarks for offline graph parallelism.
//!
//! These tests intentionally use only the engine test fixtures plus a local stateful effect. They
//! do not pull in `auris-dsp` or `auris-synth`, so they exercise graph scheduling rather than a
//! particular production plug-in implementation.

use std::hint::black_box;
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{
    Effect, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_core::project::{AudioSourceBank, Destination, Note, Output, Project, Tap, TrackId};
use auris_core::registry::PluginRegistry;
use auris_core::time::Ticks;
use rayon::ThreadPoolBuilder;

use super::RenderGraph;
use crate::graph::RENDER_CHANNELS;
use crate::testkit;
use crate::transport::Transport;

const SAMPLE_RATE: f64 = 48_000.0;
const FILTER_ID: &str = "test.parallel-filter-bank";
const AFFINITY_ID: &str = "test.parallel-thread-affinity";
const BLOCK: usize = 512;

/// A deterministic stateful workload large enough to make graph scheduling measurable.
///
/// Each lane is a one-pole recurrence. There is no transcendental arithmetic or random state, so
/// serial and parallel output can be compared bit-for-bit while still exercising plugin memory.
struct FilterBank {
    state: [f32; 16],
}

/// Test-only stand-in for a caller-placed/native plugin whose process method must stay on its
/// owning thread. The engine's plugin traits are `Send`, but that must not silently turn a
/// thread-affine host object into a Rayon job.
struct ThreadAffineEffect {
    owner: ThreadId,
}

impl auris_core::plugin::Parameterized for ThreadAffineEffect {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for ThreadAffineEffect {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            AFFINITY_ID,
            "Thread-affine test effect",
            "Test-only thread-affinity sentinel",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        assert_eq!(
            thread::current().id(),
            self.owner,
            "thread-affine plugin was dispatched away from its owning thread"
        );
        // Preserve the signal while ensuring the buffer was actually handed to the plugin.
        black_box(buffer.frame_count());
    }
}

impl FilterBank {
    fn new() -> Self {
        Self { state: [0.0; 16] }
    }

    fn apply(&mut self, buffer: &mut AudioBuffer, key: Option<&AudioBuffer>) {
        for frame in 0..buffer.frame_count() {
            let side = key
                .and_then(|buffer| buffer.channel(0).get(frame))
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            for channel in buffer.channels_mut() {
                let input = channel[frame];
                let mut output = 0.0;
                for (lane, state) in self.state.iter_mut().enumerate() {
                    let coefficient = 0.02 + lane as f32 * 0.001;
                    *state = *state * (0.97 - coefficient) + input * (coefficient + side * 0.002);
                    output += *state * (0.015 + lane as f32 * 0.0005);
                }
                channel[frame] = output;
            }
        }
    }
}

impl auris_core::plugin::Parameterized for FilterBank {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for FilterBank {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            FILTER_ID,
            "Deterministic Filter Bank",
            "Stateful benchmark-only filter bank",
            PluginCategory::Equalizer,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {
        self.state = [0.0; 16];
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        self.apply(buffer, None);
    }

    fn wants_sidechain(&self) -> bool {
        true
    }

    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: &AudioBuffer,
        _ctx: &ProcessContext,
    ) {
        self.apply(buffer, Some(sidechain));
    }
}

fn registry() -> PluginRegistry {
    let mut registry = testkit::registry();
    registry.register_effect(|| Box::new(FilterBank::new()));
    registry
}

fn fixture(track_count: usize, nested_buses: usize) -> (Project, PluginRegistry) {
    fixture_with_filters(track_count, nested_buses, true)
}

fn light_fixture(track_count: usize, nested_buses: usize) -> (Project, PluginRegistry) {
    fixture_with_filters(track_count, nested_buses, false)
}

fn fixture_with_filters(
    track_count: usize,
    nested_buses: usize,
    with_filters: bool,
) -> (Project, PluginRegistry) {
    let mut project = Project::new("Parallel render fixture", SAMPLE_RATE);
    project.master.gain_db = -20.0 * (track_count.max(1) as f32).log10();
    let buses: Vec<TrackId> = (0..nested_buses)
        .map(|index| project.add_bus_track(format!("Bus {index}")))
        .collect();
    let mut first_source = None;
    for index in 0..track_count {
        let track = project.add_instrument_track(format!("Part {index}"), testkit::TONE_ID);
        first_source.get_or_insert(track);
        let clip = project
            .add_midi_clip(
                track,
                format!("Clip {index}"),
                Ticks::ZERO,
                Ticks::from_beats(16.0),
            )
            .expect("fixture clip");
        project
            .midi_clip_mut(clip)
            .expect("fixture clip exists")
            .notes
            .push(Note::new(
                36 + (index % 48) as u8,
                Ticks::ZERO,
                Ticks::from_beats(16.0),
            ));
        if with_filters {
            project.add_effect(Some(track), FILTER_ID);
        }
        if !buses.is_empty() {
            let bus = buses[index % buses.len()];
            // A unity output to the bus and a second, quieter copy of the same signal beside it,
            // the way a send used to sit beside a track's output.
            project.routing.set_outputs(
                track,
                &[
                    Output::to_bus(bus),
                    Output {
                        to: Destination::Bus(bus),
                        gain_db: -6.0,
                        tap: Tap::PreFader,
                    },
                ],
            );
        }
    }
    for pair in buses.windows(2) {
        project
            .routing
            .set_outputs(pair[0], &[Output::to_bus(pair[1])]);
    }
    if with_filters && let (Some(&source_id), Some(&last)) = (first_source.as_ref(), buses.last()) {
        project
            .track_mut(last)
            .expect("fixture bus")
            .mixer
            .effects
            .push(auris_core::project::EffectSlot::new(
                auris_core::project::EffectSlotId(9_000),
                FILTER_ID,
            ));
        // The first source keys the final bus effect, forcing an explicit dependency edge.
        project
            .routing
            .set_key(auris_core::project::EffectSlotId(9_000), Some(source_id));
    }
    if with_filters {
        project.add_effect(None, FILTER_ID);
    }
    (project, registry())
}

fn render_once(
    project: &Project,
    registry: &PluginRegistry,
    frames: usize,
    threads: usize,
    parallel: bool,
) -> AudioBuffer {
    render_once_with_loop(project, registry, frames, threads, parallel, false)
}

fn render_once_with_loop(
    project: &Project,
    registry: &PluginRegistry,
    frames: usize,
    threads: usize,
    parallel: bool,
    looped: bool,
) -> AudioBuffer {
    let mut graph = RenderGraph::build(project, &AudioSourceBank::new(), registry, BLOCK);
    let mut transport = if looped {
        Transport {
            playing: true,
            position_frames: 0,
            loop_enabled: true,
            loop_start_frames: 0,
            loop_end_frames: (frames / 2).max(1) as u64,
            count_in: None,
        }
    } else {
        Transport::playing_from(0)
    };
    let mut out = AudioBuffer::new(RENDER_CHANNELS, frames, SAMPLE_RATE);
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("pool");
    let mut block = AudioBuffer::new(RENDER_CHANNELS, BLOCK, SAMPLE_RATE);
    pool.install(|| render_prepared(&mut graph, &mut transport, &mut block, &mut out, parallel));
    out
}

fn render_prepared(
    graph: &mut RenderGraph,
    transport: &mut Transport,
    block: &mut AudioBuffer,
    out: &mut AudioBuffer,
    parallel: bool,
) {
    let frames = out.frame_count();
    let mut offset = 0;
    while offset < frames {
        let count = (frames - offset).min(BLOCK);
        block.set_frame_count(count);
        super::render_block_impl(graph, transport, block, true, parallel);
        for channel in 0..RENDER_CHANNELS {
            out.channel_mut(channel)[offset..offset + count]
                .copy_from_slice(block.channel(channel));
        }
        offset += count;
    }
}

fn assert_bits(left: &AudioBuffer, right: &AudioBuffer) {
    assert_eq!(left.frame_count(), right.frame_count());
    assert_eq!(left.channel_count(), right.channel_count());
    for (channel, (a, b)) in left.channels().iter().zip(right.channels()).enumerate() {
        for (frame, (a, b)) in a.iter().zip(b).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "channel {channel}, frame {frame}: {a} != {b}"
            );
        }
    }
}

fn assert_close(left: &AudioBuffer, right: &AudioBuffer) -> f32 {
    assert_eq!(left.frame_count(), right.frame_count());
    assert_eq!(left.channel_count(), right.channel_count());
    let mut max_error = 0.0_f32;
    for (a, b) in left
        .channels()
        .iter()
        .flatten()
        .zip(right.channels().iter().flatten())
    {
        assert!(a.is_finite() && b.is_finite());
        let error = (a - b).abs();
        // Eight f32 epsilons at unity, with a relative term for values outside normal PCM range.
        assert!(
            error <= 8.0 * f32::EPSILON * (1.0 + a.abs()),
            "{a} != {b}, error {error:e}"
        );
        max_error = max_error.max(error);
    }
    max_error
}

#[test]
fn parallel_flat_mix_preserves_bits() {
    let (project, registry) = fixture(32, 0);
    let serial = render_once(&project, &registry, 24_000, 1, false);
    for threads in [1, 2, 4] {
        let parallel = render_once(&project, &registry, 24_000, threads, true);
        assert_bits(&serial, &parallel);
    }
}

#[test]
fn parallel_routed_mix_is_deterministic_and_numerically_close() {
    let (mut project, registry) = fixture(32, 4);
    // Independent bus branches interleave leaves and buses in DFS order. Grouping them by
    // generation deliberately changes the summation order, unlike a single shared bus chain.
    for bus in project.tracks[..4]
        .iter()
        .map(|bus| bus.id)
        .collect::<Vec<_>>()
    {
        project.routing.set_outputs(bus, &[Output::master()]);
    }
    let sources: Vec<_> = project.tracks[4..].iter().map(|track| track.id).collect();
    for (index, track) in sources.into_iter().enumerate() {
        let strip = &mut project.track_mut(track).unwrap().mixer;
        strip.gain_db = -((index % 7) as f32) * 1.3;
        strip.pan = (index % 11) as f32 / 5.0 - 1.0;
        let mut outputs = project.outputs(track);
        outputs[1].tap = if index % 2 == 0 {
            Tap::PreFader
        } else {
            Tap::PostFader
        };
        if index % 3 == 0 {
            outputs.push(Output::master());
        }
        project.routing.set_outputs(track, &outputs);
    }
    project.tracks.rotate_left(4);
    let graph = RenderGraph::build(&project, &AudioSourceBank::new(), &registry, BLOCK);
    assert_ne!(
        graph.order,
        graph
            .offline_plan
            .generations
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>()
    );
    let serial = render_once(&project, &registry, 24_000, 1, false);
    let mut reference = None;
    for threads in [1, 2, 4] {
        let parallel = render_once(&project, &registry, 24_000, threads, true);
        let max_error = assert_close(&serial, &parallel);
        let rms = serial
            .channels()
            .iter()
            .flatten()
            .zip(parallel.channels().iter().flatten())
            .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
            .sum::<f64>();
        let rms = (rms / (serial.frame_count() * serial.channel_count()) as f64).sqrt();
        eprintln!("interleaved buses, {threads} workers: max={max_error:e}, rms={rms:e}");
        if let Some(reference) = &reference {
            assert_bits(reference, &parallel);
        } else {
            reference = Some(parallel);
        }
    }
}

#[test]
fn thread_affine_plugins_are_not_dispatched_to_worker_threads() {
    let (mut project, _) = fixture(8, 0);
    let track_id = project.tracks[0].id;
    let slot = project.add_effect(Some(track_id), AFFINITY_ID).unwrap();
    let pool = ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("pool");
    pool.install(|| {
        // The native stand-in enters through the same placed map as CLAP/VST3. Its seven
        // independent registry-built peers still make this generation wide enough for Rayon.
        let registry = registry();
        let mut placed = crate::graph::PlacedEffects::new();
        placed.insert(
            slot,
            Box::new(ThreadAffineEffect {
                owner: thread::current().id(),
            }),
        );
        let mut graph = RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &registry,
            &mut placed,
            &mut crate::graph::PlacedInstruments::new(),
            BLOCK,
            SAMPLE_RATE,
        );
        assert!(!graph.tracks[0].parallel_safe);
        assert!(graph.tracks[1..].iter().all(|track| track.parallel_safe));
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, BLOCK, SAMPLE_RATE);
        for _ in 0..8 {
            super::render_block_impl(&mut graph, &mut transport, &mut out, true, true);
        }
    });
}

#[test]
fn parallel_render_covers_mute_latency_loop_and_partial_block() {
    let (mut project, registry) = fixture(16, 2);
    project.tracks[2].mixer.mute = true;
    let track = project.tracks[3].id;
    project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
    project.add_effect(Some(project.tracks[0].id), testkit::LOOKAHEAD_ID);
    let mut outputs = project.outputs(track);
    outputs[1].tap = Tap::PreFader;
    outputs.push(Output::master());
    project.routing.set_outputs(track, &outputs);
    let quieter = outputs[1];
    let targets = [
        auris_core::param::ParamTarget::TrackGain(track),
        auris_core::param::ParamTarget::Output {
            track,
            to: quieter.to,
            tap: quieter.tap,
        },
    ];
    for target in targets {
        for (tick, value) in [(Ticks::ZERO, -20.0), (Ticks::from_beats(0.05), -3.0)] {
            project.automation.set_point(
                target,
                None,
                auris_core::AutomationCurve::Linear,
                tick,
                value,
            );
        }
    }
    project.add_effect(None, testkit::LOOKAHEAD_ID);
    let serial = render_once_with_loop(&project, &registry, BLOCK * 3 + 17, 1, false, true);
    for threads in [1, 2, 4] {
        let parallel =
            render_once_with_loop(&project, &registry, BLOCK * 3 + 17, threads, true, true);
        assert_close(&serial, &parallel);
    }
}

#[test]
fn parallel_tracks_keep_state_through_mute_seek_audition_and_stopped_tails() {
    let (project, registry) = fixture(16, 3);
    let render = |parallel: bool, threads| {
        let pool = ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut graph = RenderGraph::build(&project, &AudioSourceBank::new(), &registry, BLOCK);
            let mut transport = Transport::playing_from(0);
            let mut block = AudioBuffer::new(2, BLOCK, SAMPLE_RATE);
            let mut output = AudioBuffer::new(2, BLOCK * 12, SAMPLE_RATE);
            let mut shot = AudioBuffer::new(2, BLOCK * 2, SAMPLE_RATE);
            shot.channel_mut(0).fill(0.03125);
            let _ = graph.tracks[5].play_one_shot(std::sync::Arc::new(shot));
            for step in 0..12 {
                match step {
                    1 => graph.set_track_mute(4, true),
                    3 => graph.tracks[4].note_on(70, 0.5),
                    5 => graph.set_track_mute(4, false),
                    7 => transport.position_frames = 123,
                    9 => transport.playing = false,
                    _ => {}
                }
                super::render_block_impl(&mut graph, &mut transport, &mut block, true, parallel);
                for channel in 0..2 {
                    output.channel_mut(channel)[step * BLOCK..(step + 1) * BLOCK]
                        .copy_from_slice(block.channel(channel));
                }
            }
            (
                output,
                transport.position_frames,
                graph
                    .tracks
                    .iter()
                    .map(|track| track.continued_from)
                    .collect::<Vec<_>>(),
            )
        })
    };
    let serial = render(false, 1);
    for threads in [1, 2, 4] {
        let parallel = render(true, threads);
        assert_close(&serial.0, &parallel.0);
        assert_eq!(serial.1, parallel.1);
        assert_eq!(serial.2, parallel.2);
    }
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn benchmark_case(name: &str, tracks: usize, buses: usize, frames: usize, with_filters: bool) {
    let (project, registry) = if with_filters {
        fixture(tracks, buses)
    } else {
        light_fixture(tracks, buses)
    };
    // Warm the worker threads before collecting timings. Graph construction, output allocation,
    // block allocation, and pool creation are deliberately outside the timed region.
    let mut warm_graph = RenderGraph::build(&project, &AudioSourceBank::new(), &registry, BLOCK);
    let mut warm_transport = Transport::playing_from(0);
    let mut warm_output = AudioBuffer::new(RENDER_CHANNELS, frames.min(BLOCK), SAMPLE_RATE);
    let mut block = AudioBuffer::new(RENDER_CHANNELS, BLOCK, SAMPLE_RATE);
    render_prepared(
        &mut warm_graph,
        &mut warm_transport,
        &mut block,
        &mut warm_output,
        true,
    );
    let expected = render_once(&project, &registry, frames, 1, false);
    let mut max_error = 0.0_f32;
    let mut serial = Vec::with_capacity(9);
    let mut parallel = Vec::with_capacity(9);
    for trial in 0..9 {
        let first_parallel = trial % 2 == 1;
        for is_parallel in [first_parallel, !first_parallel] {
            let mut graph = RenderGraph::build(&project, &AudioSourceBank::new(), &registry, BLOCK);
            let mut transport = Transport::playing_from(0);
            let mut output = AudioBuffer::new(RENDER_CHANNELS, frames, SAMPLE_RATE);
            let start = Instant::now();
            render_prepared(
                &mut graph,
                &mut transport,
                &mut block,
                &mut output,
                is_parallel,
            );
            let elapsed = start.elapsed();
            max_error = max_error.max(assert_close(&expected, &output));
            black_box(&output);
            if is_parallel {
                parallel.push(elapsed);
            } else {
                serial.push(elapsed);
            }
        }
        black_box(trial);
    }
    eprintln!(
        "{name}: serial median {:?}, parallel median {:?}, max error {max_error:e}",
        median(serial),
        median(parallel)
    );
}

#[test]
#[ignore = "release benchmark; run explicitly"]
fn benchmark_offline_parallel_render_variants() {
    benchmark_case("small-2-track", 2, 0, 2 * 48_000, true);
    benchmark_case("wide-32-track", 32, 0, 5 * 48_000, true);
    benchmark_case("wide-128-track", 128, 0, 5 * 48_000, true);
    benchmark_case("nested-32-track", 32, 8, 5 * 48_000, true);
    benchmark_case("nested-128-track", 128, 32, 5 * 48_000, true);
    benchmark_case("light-128-track-control", 128, 0, 5 * 48_000, false);
}
