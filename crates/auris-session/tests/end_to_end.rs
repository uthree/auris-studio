//! End-to-end tests over the whole audio path.
//!
//! These build a project the way the UI does — registry, tracks, notes, effects — render it
//! through the offline renderer and write a real WAV file. They are the tests that would catch a
//! break in the seams *between* the crates, which each crate's own unit tests cannot see.

use std::fs;

use auris_core::param::gain_to_db;
use auris_core::time::{Seconds, Ticks};
use auris_core::{AudioSourceBank, Note, PluginRegistry, Project};
use auris_dsp::DspPack;
use auris_engine::{OfflineOptions, render_project, render_project_with_progress};
use auris_io::{WavBitDepth, WavExportSettings, import_audio_file, write_wav};
use auris_synth::SynthPack;

fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>();
    registry.install::<DspPack>();
    registry
}

/// A two-bar project: a chiptune lead with an EQ and a reverb, plus a master limiter.
fn demo_project() -> Project {
    let mut project = Project::new("End to end", 48_000.0);
    project.set_bpm(120.0);

    let lead = project.add_instrument_track("Lead", "auris.synth.chiptune");
    let bar = Ticks::from_beats(4.0);
    let clip = project
        .add_midi_clip(lead, "Riff", Ticks::ZERO, bar * 2)
        .expect("instrument track accepts MIDI clips");
    let midi = project.midi_clip_mut(clip).unwrap();
    for (index, pitch) in [60u8, 64, 67, 72].iter().enumerate() {
        midi.notes.push(Note::new(
            *pitch,
            Ticks::QUARTER * index as i64,
            Ticks::QUARTER,
        ));
    }

    project.add_effect(Some(lead), "auris.fx.eq");
    project.add_effect(Some(lead), "auris.fx.reverb");
    project.add_effect(None, "auris.fx.limiter");
    project
}

#[test]
fn a_project_renders_audible_audio() {
    let project = demo_project();
    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .expect("the demo project renders");

    assert_eq!(rendered.channel_count(), 2);
    assert!(
        rendered.peak() > 0.02,
        "rendered peak {:.4} is effectively silence",
        rendered.peak()
    );
    assert!(
        rendered.peak() <= 1.0,
        "the master limiter should keep the peak at or below full scale, got {}",
        rendered.peak()
    );
    assert!(
        rendered
            .iter_channels()
            .all(|c| c.iter().all(|s| s.is_finite())),
        "the render contains non-finite samples"
    );
}

#[test]
fn an_empty_project_renders_silence_rather_than_failing() {
    let project = Project::new("Empty", 48_000.0);
    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .expect("an empty project still renders");
    assert_eq!(rendered.peak(), 0.0);
}

#[test]
fn the_render_covers_the_arrangement_and_its_tail() {
    let project = demo_project();
    let arrangement_seconds = project.duration_seconds();

    let with_tail = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .unwrap();
    let without_tail = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions {
            include_tail: false,
            ..OfflineOptions::whole_project()
        },
    )
    .unwrap();

    // Two bars at 120 BPM is exactly 4 seconds.
    assert!((arrangement_seconds - 4.0).abs() < 1e-6);
    assert!((without_tail.duration_seconds() - arrangement_seconds).abs() < 0.01);
    assert!(
        with_tail.frame_count() > without_tail.frame_count(),
        "the reverb tail should extend the render"
    );
}

#[test]
fn block_size_does_not_change_the_result() {
    let project = demo_project();
    let render_at = |block_frames| {
        render_project(
            &project,
            &AudioSourceBank::new(),
            &registry(),
            &OfflineOptions::whole_project().with_block_frames(block_frames),
        )
        .unwrap()
    };

    let coarse = render_at(4_096);
    let odd = render_at(97);
    assert_eq!(coarse.frame_count(), odd.frame_count());
    for channel in 0..coarse.channel_count() {
        for (index, (a, b)) in coarse
            .channel(channel)
            .iter()
            .zip(odd.channel(channel))
            .enumerate()
        {
            assert!(
                (a - b).abs() < 1e-6,
                "channel {channel} frame {index} differs: {a} vs {b}"
            );
        }
    }
}

#[test]
fn progress_runs_from_zero_to_one() {
    let project = demo_project();
    let mut reported = Vec::new();
    render_project_with_progress(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
        &mut auris_engine::RenderProgress::reporting(&mut |fraction| reported.push(fraction)),
    )
    .unwrap();

    assert!(reported.len() > 2, "progress was barely reported");
    assert_eq!(reported.first().copied(), Some(0.0));
    assert_eq!(reported.last().copied(), Some(1.0));
    assert!(
        reported.windows(2).all(|pair| pair[1] >= pair[0]),
        "progress went backwards"
    );
}

#[test]
fn a_rendered_project_survives_a_wav_round_trip() {
    let project = demo_project();
    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .unwrap();

    let path = std::env::temp_dir().join("auris-end-to-end.wav");
    let settings = WavExportSettings {
        bit_depth: WavBitDepth::Float32,
        sample_rate: 48_000,
        dither: false,
    };
    write_wav(&path, &rendered, &settings).expect("the render is written");

    let reimported = import_audio_file(&path, 48_000.0).expect("the file reads back");
    assert_eq!(reimported.channel_count(), rendered.channel_count());
    assert_eq!(reimported.frame_count(), rendered.frame_count());
    assert!(
        (gain_to_db(reimported.peak()) - gain_to_db(rendered.peak())).abs() < 0.01,
        "the peak changed across the round trip"
    );

    fs::remove_file(&path).ok();
}

#[test]
fn muting_the_only_track_silences_the_render() {
    let mut project = demo_project();
    let track = project.tracks[0].id;
    project.track_mut(track).unwrap().mixer.mute = true;

    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .unwrap();
    assert_eq!(rendered.peak(), 0.0);
}

#[test]
fn a_project_with_a_missing_plugin_still_renders() {
    let mut project = Project::new("Missing", 48_000.0);
    let track = project.add_instrument_track("Ghost", "nobody.synth.missing");
    let clip = project
        .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    project
        .midi_clip_mut(clip)
        .unwrap()
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    project.add_effect(Some(track), "nobody.fx.missing");

    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions::whole_project(),
    )
    .expect("an unknown plugin id must not fail the render");
    assert_eq!(rendered.peak(), 0.0);
}

#[test]
fn tempo_sets_the_arrangement_length() {
    let mut project = demo_project();
    project.set_bpm(240.0);
    // Twice the tempo, half the wall-clock length.
    assert!((project.duration_seconds() - 2.0).abs() < 1e-6);

    let rendered = render_project(
        &project,
        &AudioSourceBank::new(),
        &registry(),
        &OfflineOptions {
            include_tail: false,
            ..OfflineOptions::whole_project()
        },
    )
    .unwrap();
    assert!((rendered.duration_seconds() - 2.0).abs() < 0.01);
    assert_eq!(
        project.tempo_map.ticks_to_seconds(project.end_tick()),
        Seconds(2.0)
    );
}
