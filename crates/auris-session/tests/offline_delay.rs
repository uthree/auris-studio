//! Numerical regressions for tempo-dependent tails across the real DSP and render graph.

use std::sync::Arc;

use auris_core::{AssetPath, AudioBuffer, AudioSourceBank, PluginRegistry, Project, Ticks};
use auris_dsp::DspPack;
use auris_engine::{OfflineOptions, OfflineRender, RenderProgress, render_project};

const SAMPLE_RATE: f64 = 48_000.0;

fn delayed_impulse(
    bpm: f64,
    start: Ticks,
    frames: usize,
) -> (Project, AudioSourceBank, PluginRegistry) {
    let mut project = Project::new("Tempo-dependent tail", SAMPLE_RATE);
    project.tempo_map.set_initial_bpm(bpm);
    let track = project.add_audio_track("Impulse");
    let source = project.add_audio_source(
        "Impulse",
        AssetPath::inside("Audio/impulse.wav"),
        frames as u64,
        SAMPLE_RATE,
        1,
    );
    project.add_audio_clip(track, source, start).unwrap();
    project.add_effect(Some(track), "auris.fx.delay").unwrap();
    let parameters = &mut project.tracks[0].mixer.effects[0].state.params;
    parameters.insert("sync".into(), 5.0); // Quarter note.
    parameters.insert("mix".into(), 1.0);
    parameters.insert("feedback".into(), 0.0);

    let mut samples = vec![0.0; frames];
    samples[0] = 1.0;
    let mut bank = AudioSourceBank::new();
    bank.insert(
        source,
        Arc::new(AudioBuffer::from_planar(vec![samples], SAMPLE_RATE).unwrap()),
    );
    let mut registry = PluginRegistry::new();
    registry.install::<DspPack>();
    (project, bank, registry)
}

fn assert_echo(buffer: &AudioBuffer, frames: usize, echo: usize) {
    assert_eq!(buffer.frame_count(), frames);
    assert!(
        buffer.channel(0)[echo] > 0.99,
        "the delayed impulse is missing"
    );
    assert!(buffer.peak() <= 1.0);
}

#[test]
fn a_synced_delay_keeps_its_full_tail_in_buffered_streamed_and_repeated_exports() {
    let (project, bank, registry) = delayed_impulse(60.0, Ticks::ZERO, 4_800);
    for block_frames in [7, 1_024] {
        let mut render = OfflineRender::new(
            &project,
            &bank,
            &registry,
            &mut Default::default(),
            &mut Default::default(),
            &OfflineOptions::default().with_block_frames(block_frames),
        )
        .unwrap();
        let mut first = render.buffer().unwrap();
        render
            .render(&mut first, &mut RenderProgress::default())
            .unwrap();
        assert_echo(&first, 52_800, 48_000);
        assert!(
            first.channel(0)[..48_000]
                .iter()
                .all(|sample| *sample == 0.0)
        );

        let mut streamed = Vec::new();
        render
            .render_streamed(&mut RenderProgress::default(), |block| {
                streamed.extend_from_slice(block.channel(0));
                Ok::<_, std::convert::Infallible>(())
            })
            .unwrap();
        assert!(
            streamed == first.channel(0),
            "streaming changed the samples"
        );
        let mut repeated = render.buffer().unwrap();
        render
            .render(&mut repeated, &mut RenderProgress::default())
            .unwrap();
        assert!(
            first == repeated,
            "repeating the render changed the samples"
        );
    }
}

#[test]
fn a_ranged_tail_uses_the_range_tempo_and_ignores_later_changes() {
    let (mut project, bank, registry) = delayed_impulse(120.0, Ticks::QUARTER, 4_800);
    project.tempo_map.set_point(Ticks::QUARTER, 60.0);
    project.tempo_map.set_point(Ticks::from_beats(2.0), 10.0);
    let output = render_project(
        &project,
        &bank,
        &registry,
        &OfflineOptions::default().with_range(24_000, 28_800),
    )
    .unwrap();
    assert_echo(&output, 52_800, 48_000);
}

#[test]
fn a_full_export_accounts_for_tempo_changes_during_ring_out_but_not_hours_later() {
    let (mut project, bank, registry) = delayed_impulse(120.0, Ticks::ZERO, 4_800);
    // The arrangement ends at 0.1 s; the delay slows down at 0.125 s, during its tail.
    project.tempo_map.set_point(Ticks::from_beats(0.25), 60.0);
    project
        .tempo_map
        .set_point(Ticks::from_beats(10_000.0), 10.0);
    let output = render_project(&project, &bank, &registry, &OfflineOptions::default()).unwrap();
    assert_echo(&output, 52_800, 48_000);
}

#[test]
fn a_cycle_warms_up_for_the_actual_synced_delay_without_appending_a_tail() {
    let (mut project, bank, registry) = delayed_impulse(60.0, Ticks::ZERO, 4_800);
    // A tempo outside the cycle must not control its warm-up or the captured repetition.
    project.tempo_map.set_point(Ticks::QUARTER, 10.0);
    for block_frames in [7, 1_024] {
        let output = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions {
                end_frames: Some(14_400),
                looping: true,
                block_frames,
                ..OfflineOptions::default()
            },
        )
        .unwrap();
        assert_echo(&output, 14_400, 4_800);
    }
}

#[test]
fn repeated_passes_start_at_the_same_tempo_after_a_tempo_changing_render() {
    let (mut project, bank, registry) = delayed_impulse(120.0, Ticks::ZERO, 48_000);
    project.tempo_map.set_point(Ticks::from_beats(1.5), 60.0);
    let mut render = OfflineRender::new(
        &project,
        &bank,
        &registry,
        &mut Default::default(),
        &mut Default::default(),
        &OfflineOptions::default(),
    )
    .unwrap();
    let mut first = render.buffer().unwrap();
    render
        .render(&mut first, &mut RenderProgress::default())
        .unwrap();
    assert!(first.channel(0)[24_000] > 0.99);
    let mut second = render.buffer().unwrap();
    render
        .render(&mut second, &mut RenderProgress::default())
        .unwrap();
    assert!(
        first == second,
        "the second pass started from the preceding pass's tempo"
    );
}
