//! Regression coverage for the singer take ownership boundary.

use auris_core::project::{Note, Project, SingerTake};
use auris_core::time::Ticks;
use auris_core::{AudioBuffer, SourceId};

use super::super::testkit;
use crate::{OfflineOptions, render_project};

fn singer_with_note() -> (Project, auris_core::TrackId) {
    let mut project = Project::new("singer take", 48_000.0);
    let track = project.add_singer_track("Voice", testkit::TONE_ID);
    let clip = project
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(2.0))
        .expect("singer clip");
    project
        .midi_clip_mut(clip)
        .expect("singer clip")
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(1.0)));
    (project, track)
}

fn render(project: &Project) -> AudioBuffer {
    render_project(
        project,
        &auris_core::project::AudioSourceBank::new(),
        &testkit::registry(),
        &OfflineOptions {
            end_frames: Some(2_048),
            block_frames: 128,
            ..OfflineOptions::default()
        },
    )
    .expect("offline render")
}

fn peak(buffer: &AudioBuffer) -> f32 {
    buffer
        .channel(0)
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0, f32::max)
}

#[test]
fn declared_take_without_audio_metadata_is_silence_and_never_falls_back_to_guide() {
    let (without_take, track) = singer_with_note();
    let guide_peak = peak(&render(&without_take));
    assert!(
        guide_peak > 1.0e-5,
        "guide preview should be audible without a take"
    );

    let mut missing_take = without_take;
    let singer = missing_take
        .track_mut(track)
        .unwrap()
        .kind
        .as_singer_mut()
        .unwrap();
    singer.take = Some(SingerTake {
        provenance: None,
        source: SourceId(900_000),
        fingerprint: 1,
        seed: 2,
        backend_pitch: None,
    });
    let take_peak = peak(&render(&missing_take));
    assert!(
        take_peak < 1.0e-7,
        "a declared take with no source metadata must remain silent, not audition the guide: {take_peak}"
    );
}
