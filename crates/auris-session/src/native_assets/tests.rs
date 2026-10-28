use super::*;
use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::document::NativeDocument;
use auris_core::project::Project;
use auris_core::{AssetPath, Ticks};
use std::sync::Arc;
use tempfile::TempDir;

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 16,
        clips: 32,
        stages: 32,
        processors: 64,
        groups: 16,
        tracks: 8,
        edges: 64,
        external_ids: 32,
        automations: 16,
        automation_points: 64,
        bindings: 16,
    }
}

fn wav() -> (TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("take.wav");
    let mut recorder = auris_io::WavRecorder::create(&path, 44_100.0, 1).expect("wav");
    let samples: Vec<f32> = (0..4096)
        .map(|index| if index % 4 < 2 { 0.25 } else { -0.25 })
        .collect();
    recorder.write(&samples).expect("samples");
    recorder.finish().expect("finish");
    (directory, path)
}

fn native(path: &std::path::Path, follows_tempo: bool) -> (NativeDocument, auris_core::SourceId) {
    let mut project = Project::new("assets", 48_000.0);
    let track = project.add_audio_track("Audio");
    let source = project.add_audio_source("take", AssetPath::external(path), 4, 44_100.0, 1);
    let clip = project
        .add_audio_clip(track, source, Ticks::ZERO)
        .expect("clip");
    if follows_tempo {
        project.audio_clip_mut(clip).expect("clip").follows_tempo = true;
        project.audio_clip_mut(clip).expect("clip").source_bpm = Some(60.0);
    }
    (
        auris_io::project_to_native(&project, limits()).expect("native"),
        source,
    )
}

#[test]
fn native_preparation_decodes_and_resamples_actual_wav() {
    let (_directory, path) = wav();
    let (document, source) = native(&path, false);
    let prepared = prepare_native_audio_assets(&document, None, 48_000.0, None).expect("prepare");
    let buffer = prepared.bank.get(source).expect("decoded source");
    assert_eq!(buffer.sample_rate(), 48_000.0);
    assert!(buffer.frame_count() >= 4);
    assert!(buffer.channel(0).iter().any(|sample| sample.abs() > 0.1));
}

#[test]
fn native_preparation_reports_missing_assets_without_failing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("missing.wav");
    let (document, source) = native(&path, false);
    let prepared =
        prepare_native_audio_assets(&document, None, 48_000.0, None).expect("diagnostic");
    assert!(prepared.bank.get(source).is_none());
    assert_eq!(prepared.missing.len(), 1);
    assert_eq!(prepared.missing[0].source, source);
}

#[test]
fn native_preparation_populates_requested_stretch_cache_and_reuses_source() {
    let (_directory, path) = wav();
    let (document, source) = native(&path, true);
    let first = prepare_native_audio_assets(&document, None, 48_000.0, None).expect("prepare");
    assert_eq!(first.bank.stretches().count(), 1);
    let second =
        prepare_native_audio_assets(&document, None, 48_000.0, Some((&document, &first.bank)))
            .expect("reuse");
    assert!(second.bank.get(source).is_some());
    assert_eq!(second.bank.stretches().count(), 1);
    assert!(Arc::ptr_eq(
        first.bank.get(source).unwrap(),
        second.bank.get(source).unwrap()
    ));
}

#[test]
fn native_preparation_does_not_reuse_stretch_at_a_different_sample_rate() {
    let (_directory, path) = wav();
    let (document, source) = native(&path, true);
    let first = prepare_native_audio_assets(&document, None, 48_000.0, None).expect("prepare");
    let (_, key) = first.bank.stretches().next().expect("stretch key");
    let second =
        prepare_native_audio_assets(&document, None, 44_100.0, Some((&document, &first.bank)))
            .expect("prepare at changed rate");
    assert_eq!(second.bank.get(source).unwrap().sample_rate(), 44_100.0);
    assert_ne!(
        second.bank.stretched(source, key).unwrap().sample_rate(),
        first.bank.stretched(source, key).unwrap().sample_rate()
    );
    assert!(!Arc::ptr_eq(
        first.bank.stretched(source, key).unwrap(),
        second.bank.stretched(source, key).unwrap()
    ));
}
