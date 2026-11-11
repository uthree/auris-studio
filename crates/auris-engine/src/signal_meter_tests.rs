use super::*;
use crate::{RENDER_CHANNELS, RenderGraph, Transport, render_block, testkit};
use auris_core::{
    AudioBuffer, AudioSourceBank, Destination, Note, Output, ParamId, Tap, Ticks, param::db_to_gain,
};

const RATE: f64 = 48_000.0;

struct Fixture {
    project: Project,
    source: TrackId,
    bus: TrackId,
    aux: TrackId,
    first: EffectSlotId,
    second: EffectSlotId,
    keyed: EffectSlotId,
    master: EffectSlotId,
}

fn fixture() -> Fixture {
    let mut project = Project::new("Signal meters", RATE);
    let source = project.add_instrument_track("Source", testkit::TONE_ID);
    let clip = project
        .add_midi_clip(source, "Tone", Ticks::ZERO, Ticks::from_beats(8.))
        .unwrap();
    project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
        60,
        Ticks::ZERO,
        Ticks::from_beats(8.),
    ));
    let bus = project.add_bus_track("Bus");
    let aux = project.add_bus_track("Aux");
    let first = project.add_effect(Some(source), testkit::GAIN_ID).unwrap();
    let second = project.add_effect(Some(source), testkit::GAIN_ID).unwrap();
    let keyed = project.add_effect(Some(bus), testkit::KEYED_ID).unwrap();
    project.track_mut(bus).unwrap().mixer.effects[0].sidechain = Some(source);
    let master = project.add_effect(None, testkit::GAIN_ID).unwrap();
    let track = project.track_mut(source).unwrap();
    track.mixer.gain_db = -12.;
    track.outputs = vec![
        Output::master(),
        Output::to_bus(bus),
        Output {
            to: Destination::Bus(bus),
            gain_db: -6.,
            tap: Tap::PreFader,
        },
        Output {
            to: Destination::Bus(aux),
            gain_db: -12.,
            tap: Tap::PostFader,
        },
        Output {
            to: Destination::Bus(aux),
            gain_db: f32::NEG_INFINITY,
            tap: Tap::PreFader,
        },
    ];
    Fixture {
        project,
        source,
        bus,
        aux,
        first,
        second,
        keyed,
        master,
    }
}

fn build(project: &Project, block: usize) -> RenderGraph {
    RenderGraph::build(
        project,
        &AudioSourceBank::new(),
        &testkit::registry(),
        block,
    )
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn every_connection_reads_its_actual_tap_without_allocating_or_changing_audio() {
    let f = fixture();
    let mut observed = build(&f.project, 256);
    let mut baseline = build(&f.project, 256);
    for graph in [&mut observed, &mut baseline] {
        graph.tracks_mut()[0]
            .strip_mut()
            .set_effect_param(0, ParamId(0), 0.5);
    }
    let meters = observed.signal_meters();
    meters.watch(true);
    let mut transport = Transport::playing_from(0);
    let mut baseline_transport = transport;
    let mut actual = AudioBuffer::new(RENDER_CHANNELS, 256, RATE);
    let mut expected = actual.clone();
    let heap = testkit::count_heap_operations(|| {
        render_block(&mut observed, &mut transport, &mut actual, false);
    });
    assert_eq!(heap, (0, 0));
    render_block(&mut baseline, &mut baseline_transport, &mut expected, false);
    assert_eq!(actual.channels(), expected.channels());

    let post = 0.25 * db_to_gain(-12.);
    let pre_send = 0.25 * db_to_gain(-6.);
    let post_send = post * db_to_gain(-12.);
    near(
        meters.peak(SignalRoute::InsertInput(Some(f.source), f.first)),
        0.5,
    );
    near(
        meters.peak(SignalRoute::InsertInput(Some(f.source), f.second)),
        0.25,
    );
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PostFader,
        )),
        post,
    );
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.bus),
            Tap::PostFader,
        )),
        post,
    );
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.bus),
            Tap::PreFader,
        )),
        pre_send,
    );
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.aux),
            Tap::PostFader,
        )),
        post_send,
    );
    assert_eq!(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.aux),
            Tap::PreFader
        )),
        0.
    );
    near(
        meters.peak(SignalRoute::Sidechain(Some(f.bus), f.keyed)),
        post,
    );
    let bus_input = post + pre_send;
    near(
        meters.peak(SignalRoute::InsertInput(Some(f.bus), f.keyed)),
        bus_input,
    );
    near(
        meters.peak(SignalRoute::InsertInput(None, f.master)),
        post + bus_input * post + post_send,
    );

    // Exercise moving gains as well as the flat path. Metering must preserve every sample.
    for block in 0..8 {
        if block == 1 {
            for graph in [&mut observed, &mut baseline] {
                graph.set_output_gain_db(0, 3, -3.);
                graph.tracks_mut()[0].strip_mut().set_gain_db(-24.);
            }
        }
        render_block(&mut baseline, &mut baseline_transport, &mut expected, false);
        assert_eq!(
            testkit::count_heap_operations(|| {
                render_block(&mut observed, &mut transport, &mut actual, false);
            }),
            (0, 0)
        );
        assert_eq!(actual.channels(), expected.channels());
    }
    assert!(
        baseline
            .signal_meters()
            .levels
            .iter()
            .all(|slot| slot.load(Ordering::Relaxed) == 0)
    );
}

#[test]
fn mute_bypass_and_insert_processing_change_only_the_connections_they_affect() {
    let f = fixture();
    let mut graph = build(&f.project, 256);
    let meters = graph.signal_meters();
    meters.watch(true);
    let mut transport = Transport::playing_from(0);
    let mut out = AudioBuffer::new(RENDER_CHANNELS, 256, RATE);
    render_block(&mut graph, &mut transport, &mut out, false);
    graph.tracks[1].strip.enabled[0] = false;
    graph.tracks_mut()[0].strip_mut().set_gain_db(-120.);
    render_block(&mut graph, &mut transport, &mut out, false);
    meters.reset();
    render_block(&mut graph, &mut transport, &mut out, false);
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.bus),
            Tap::PreFader,
        )),
        0.5 * db_to_gain(-6.),
    );
    assert!(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.aux),
            Tap::PostFader
        )) < 1e-6
    );
    assert_eq!(
        meters.peak(SignalRoute::Sidechain(Some(f.bus), f.keyed)),
        0.
    );
    assert!(
        meters.peak(SignalRoute::Output(
            f.bus,
            Destination::Master,
            Tap::PostFader
        )) > 0.2
    );

    graph.tracks_mut()[0]
        .strip_mut()
        .set_effect_param(0, ParamId(0), 0.0);
    meters.reset();
    render_block(&mut graph, &mut transport, &mut out, false);
    near(
        meters.peak(SignalRoute::InsertInput(Some(f.source), f.first)),
        0.5,
    );
    assert_eq!(
        meters.peak(SignalRoute::InsertInput(Some(f.source), f.second)),
        0.
    );
    assert_eq!(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Bus(f.bus),
            Tap::PreFader
        )),
        0.
    );

    graph.tracks_mut()[0].strip_mut().set_mute(true);
    render_block(&mut graph, &mut transport, &mut out, false);
    meters.reset();
    render_block(&mut graph, &mut transport, &mut out, false);
    assert!(
        meters
            .levels
            .iter()
            .all(|slot| slot.load(Ordering::Relaxed) == 0)
    );
}

#[test]
fn compensated_output_peaks_wait_for_the_signal_to_reach_the_destination() {
    let mut f = fixture();
    f.project.track_mut(f.source).unwrap().mixer.gain_db = 0.;
    f.project.track_mut(f.source).unwrap().outputs.truncate(2);
    f.project.track_mut(f.bus).unwrap().mixer.effects.clear();
    f.project
        .add_effect(Some(f.bus), testkit::LOOKAHEAD_ID)
        .unwrap();
    let mut graph = build(&f.project, 16);
    let meters = graph.signal_meters();
    meters.watch(true);
    let mut transport = Transport::playing_from(0);
    let mut out = AudioBuffer::new(RENDER_CHANNELS, 16, RATE);
    for _ in 0..testkit::LOOKAHEAD_FRAMES / 16 {
        render_block(&mut graph, &mut transport, &mut out, false);
        assert_eq!(
            meters.peak(SignalRoute::Output(
                f.source,
                Destination::Master,
                Tap::PostFader
            )),
            0.
        );
        near(
            meters.peak(SignalRoute::Output(
                f.source,
                Destination::Bus(f.bus),
                Tap::PostFader,
            )),
            0.5,
        );
    }
    render_block(&mut graph, &mut transport, &mut out, false);
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PostFader,
        )),
        0.5,
    );
}

#[test]
fn callback_segments_hold_short_peaks_and_offline_rendering_publishes_nothing() {
    let mut f = fixture();
    let track = f.project.track_mut(f.source).unwrap();
    track.mixer.gain_db = 0.;
    track.mixer.effects.clear();
    track.outputs = vec![Output::master()];
    f.project.tracks[0].kind.note_clips_mut().unwrap()[0].notes[0].length = Ticks(1);
    let mut graph = build(&f.project, 256);
    let meters = graph.signal_meters();
    meters.watch(true);
    let mut transport = Transport::playing_from(0);
    transport.loop_enabled = true;
    transport.loop_start_frames = 128;
    transport.loop_end_frames = 144;
    let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, RATE);
    render_block(&mut graph, &mut transport, &mut out, false);
    near(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PostFader,
        )),
        0.5,
    );
    assert!(out.channel(0)[144..].iter().all(|sample| *sample == 0.));
    graph.panic();
    assert_eq!(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PostFader
        )),
        0.
    );
    transport = Transport::playing_from(0);
    render_block(&mut graph, &mut transport, &mut out, true);
    assert!(out.peak() > 0.);
    assert_eq!(
        meters.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PostFader
        )),
        0.
    );
}

#[test]
fn connection_ballistics_reset_and_new_graphs_do_not_reuse_readings() {
    let f = fixture();
    let meters = SignalMeters::for_project(&f.project);
    let route = SignalRoute::Output(f.source, Destination::Bus(f.bus), Tap::PreFader);
    meters.watch(true);
    meters.observe(meters.slot(route), 0.5);
    meters.observe(meters.slot(route), f32::NAN);
    meters.observe(meters.slot(route), f32::INFINITY);
    near(meters.peak(route), 0.5);
    near(meters.peak(route), 0.5);
    for _ in 0..100 {
        meters.begin_block(480, RATE);
    }
    near(meters.peak(route), 0.05);
    meters.watch(false);
    assert_eq!(meters.peak(route), 0.);
    meters.watch(true);
    assert_eq!(meters.peak(route), 0.);
    meters.observe(meters.slot(route), 0.8);
    let fresh = build(&f.project, 256).signal_meters();
    fresh.watch(true);
    assert_eq!(fresh.peak(route), 0.);
    assert_eq!(
        fresh.peak(SignalRoute::Output(
            f.source,
            Destination::Master,
            Tap::PreFader
        )),
        0.
    );
}
