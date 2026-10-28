//! Regression coverage for the ordinary Session persistence boundary.

use std::sync::atomic::AtomicBool;

use auris_core::NoteTransform;
use auris_core::automation::AutomationCurve;
use auris_core::param::{ParamId, ParamTarget};
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::Processor;
use auris_core::pipeline::content::{ContentLimits, ProcessorKind};
use auris_core::time::Ticks;
use auris_io::NATIVE_FORMAT_VERSION;

use super::fixtures::{Scratch, session};

fn format_version(path: &std::path::Path) -> u32 {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("the persisted document can be read"))
            .expect("the persisted document is JSON");
    value["format_version"]
        .as_u64()
        .expect("the persistence envelope has a format version") as u32
}

fn content_limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 128,
        stages: 512,
        processors: 512,
        groups: 128,
        tracks: 128,
        edges: 1024,
        external_ids: 128,
        automations: 128,
        automation_points: 1024,
        bindings: 128,
    }
}

#[test]
fn ordinary_save_and_open_preserve_real_audio_window_and_sample_offset() {
    let scratch = Scratch::new("native-audio-window");
    let source_file = scratch.tone("recorded.wav");
    let mut live = session();
    let clip = live
        .import_audio(&source_file, Ticks::ZERO)
        .expect("the real WAV imports");
    live.trim_clip_start(clip, Ticks::QUARTER)
        .expect("the audio window can be trimmed");
    let before = live
        .project()
        .audio_clip(clip)
        .expect("the imported clip exists")
        .clone();
    let source = live.project().audio_sources[&before.source].clone();

    let document = live
        .save_as(&scratch.join("Recorded.auris"))
        .expect("the ordinary Session Save As succeeds")
        .document;
    assert_eq!(format_version(&document), NATIVE_FORMAT_VERSION);

    let mut reopened = session();
    let missing = reopened
        .open(&document)
        .expect("the ordinary Session Open succeeds");
    assert!(
        missing.is_empty(),
        "the collected WAV is present: {missing:?}"
    );
    let after = reopened
        .project()
        .audio_clip(clip)
        .expect("the clip identity survives reopening");
    assert_eq!(after.source, before.source);
    assert_eq!(after.offset_frames, before.offset_frames);
    assert_eq!(after.length_frames, before.length_frames);
    assert_eq!(
        reopened.project().audio_sources[&after.source].frame_count,
        source.frame_count
    );
}

#[test]
fn ordinary_save_job_cancellation_and_staleness_never_publish_native_bytes() {
    let scratch = Scratch::new("native-save-guards");
    let mut live = session();
    live.add_default_instrument_track("Baseline").unwrap();
    let document = live
        .save_as(&scratch.join("Baseline.auris"))
        .unwrap()
        .document;
    assert_eq!(format_version(&document), NATIVE_FORMAT_VERSION);
    let before = std::fs::read(&document).unwrap();

    let cancelled = AtomicBool::new(true);
    assert!(
        live.begin_save_in_place()
            .unwrap()
            .run(&cancelled)
            .unwrap()
            .is_none(),
        "cancellation must stop before native publication"
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);

    let result = live
        .begin_save_in_place()
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap()
        .expect("the uncancelled worker stages a save");
    live.add_default_instrument_track("Newer edit").unwrap();
    assert!(
        live.continue_save(result).is_none(),
        "a stale worker cannot publish over a newer edit"
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);
    assert_eq!(format_version(&document), NATIVE_FORMAT_VERSION);
}

#[test]
fn native_decode_param_indices_do_not_trigger_a_false_external_change() {
    let scratch = Scratch::new("native-param-baseline");
    let mut live = session();
    let track = live.add_default_instrument_track("Automated").unwrap();
    let instrument = live
        .instrument_descriptors(track)
        .iter()
        .find(|descriptor| descriptor.id.0 != 0)
        .cloned()
        .expect("the built-in instrument has a nonzero parameter");
    assert!(live.set_automation_point(
        auris_core::ParamTarget::Instrument {
            track,
            param: instrument.id,
        },
        Ticks::ZERO,
        instrument.default,
    ));

    let slot = live
        .add_effect(Some(track), "auris.fx.compressor")
        .expect("the built-in compressor is available");
    let effect = live
        .effect_descriptors(slot)
        .iter()
        .find(|descriptor| descriptor.id.0 != 0)
        .cloned()
        .expect("the built-in effect has a nonzero parameter");
    assert!(live.set_automation_point(
        auris_core::ParamTarget::Effect {
            track: Some(track),
            slot,
            param: effect.id,
        },
        Ticks::ZERO,
        effect.default,
    ));

    let path = live
        .save_as(&scratch.join("Param baseline.auris"))
        .unwrap()
        .document;
    live.save_in_place()
        .expect("native decode must not report its synthetic ParamIds as external edits");
    let mut reopened = session();
    reopened.open(&path).unwrap();
    assert!(
        !reopened.is_dirty(),
        "resolving transient parameter positions is not an edit"
    );
    assert_eq!(reopened.automation(), live.automation());
    reopened.rename_track(track, "Changed").unwrap();
    reopened.undo().unwrap();
    assert!(
        !reopened.is_dirty(),
        "Undo compares against the resolved baseline"
    );
}

#[test]
fn native_source_and_stage_ids_survive_first_save_undo_and_new_objects() {
    let scratch = Scratch::new("native-history-identities");
    let mut live = session();
    let track = live.add_default_instrument_track("Before").unwrap();
    let clip = live
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    live.set_clip_transforms(
        clip,
        vec![
            NoteTransform::Lean { ticks: 1 },
            NoteTransform::Lean { ticks: 2 },
        ],
    )
    .unwrap();
    live.rename_track(track, "Saved").unwrap();
    let path = live
        .save_as(&scratch.join("History.auris"))
        .unwrap()
        .document;
    let original = auris_io::load_native(&path, content_limits()).unwrap();
    let transition = live.capture_native_transition(content_limits()).unwrap();
    assert_eq!(transition.editor().document().content, original.content);
    live.undo().unwrap();
    live.save_in_place().unwrap();
    let undone = auris_io::load_native(&path, content_limits()).unwrap();
    assert_eq!(
        undone.content, original.content,
        "pre-save history retains native identities"
    );
    live.set_clip_transforms(
        clip,
        vec![
            NoteTransform::Lean { ticks: 2 },
            NoteTransform::Lean { ticks: 1 },
        ],
    )
    .unwrap();
    live.save_in_place().unwrap();
    let reordered = auris_io::load_native(&path, content_limits()).unwrap();
    let stages = |document: &auris_core::pipeline::document::NativeDocument| {
        let auris_core::pipeline::content::SourceContent::Notes(clips) =
            &document.content.sources[0].content
        else {
            panic!("notes")
        };
        clips[0]
            .transforms
            .iter()
            .map(|stage| stage.id)
            .collect::<Vec<_>>()
    };
    let mut expected = stages(&original);
    expected.reverse();
    assert_eq!(stages(&reordered), expected);
    let added = live.add_default_instrument_track("New object").unwrap();
    assert!(added.0 >= reordered.content.next_id);
    live.undo().unwrap();
    let replacement = live.add_default_instrument_track("Replacement").unwrap();
    assert!(
        replacement.0 > added.0,
        "Undo cannot reuse a native identity"
    );
    live.save_in_place().unwrap();
    let mut reopened = session();
    reopened.open(&path).unwrap();
    assert_eq!(reopened.project(), live.project());
}

#[test]
fn unresolved_native_key_survives_reopen_without_colliding_with_a_real_parameter() {
    let scratch = Scratch::new("native-unresolved-automation");
    let mut source = session();
    let track = source.add_default_instrument_track("Automated").unwrap();
    let descriptor = source.instrument_descriptors(track)[0].clone();
    assert!(source.set_automation_point(
        ParamTarget::Instrument {
            track,
            param: descriptor.id,
        },
        Ticks::ZERO,
        descriptor.default,
    ));
    let path = source
        .save_as(&scratch.join("Unknown key.auris"))
        .unwrap()
        .document;

    let original_bytes = std::fs::read(&path).unwrap();
    let mut native = auris_io::load_native(&path, content_limits()).unwrap();
    let binding = native
        .content
        .automation_bindings
        .iter_mut()
        .find(|binding| binding.target.key == descriptor.key)
        .expect("the instrument automation binding exists");
    binding.target.key = "missing.parameter".into();
    let staged = auris_io::stage_native(&path, &native, content_limits()).unwrap();
    auris_io::publish_native_update(staged, &original_bytes, content_limits()).unwrap();

    let mut reopened = session();
    reopened.open(&path).unwrap();
    let unresolved = reopened
        .automation()
        .lanes()
        .iter()
        .find(|lane| lane.key() == Some("missing.parameter"))
        .expect("the unknown native lane is retained");
    assert_eq!(unresolved.target.param(), Some(ParamId(u32::MAX)));
    let actual = ParamTarget::Instrument {
        track,
        param: descriptor.id,
    };
    assert!(reopened.automation().lane(actual).is_none());
    assert!(reopened.set_automation_point(actual, Ticks::ZERO, descriptor.default));
    assert_eq!(reopened.automation().len(), 2);
    reopened.save_in_place().unwrap();

    let mut reopened_again = session();
    reopened_again.open(&path).unwrap();
    assert!(reopened_again.automation().lanes().iter().any(|lane| {
        lane.key() == Some("missing.parameter") && lane.target.param() == Some(ParamId(u32::MAX))
    }));
    assert!(reopened_again.automation().lane(actual).is_some());
    assert!(!reopened_again.is_dirty());
}

#[test]
fn persisted_baseline_rejects_removed_changed_key_and_changed_value_lanes() {
    let mut live = session();
    let track = live.add_default_instrument_track("Automated").unwrap();
    let target = ParamTarget::Instrument {
        track,
        param: ParamId(7),
    };
    let mut saved = live.project().clone();
    saved.automation.set_point(
        target,
        Some("filter.cutoff".into()),
        AutomationCurve::Linear,
        Ticks::ZERO,
        0.25,
    );

    let mut removed = saved.clone();
    assert!(removed.automation.remove_lane(target));
    assert!(!super::files::same_persisted_project(&removed, &saved));

    let mut changed_key = saved.clone();
    assert!(changed_key.automation.remove_lane(target));
    changed_key.automation.set_point(
        target,
        Some("filter.resonance".into()),
        AutomationCurve::Linear,
        Ticks::ZERO,
        0.25,
    );
    assert!(!super::files::same_persisted_project(&changed_key, &saved));

    let mut changed_value = saved.clone();
    assert!(changed_value.automation.set_point(
        target,
        Some("filter.cutoff".into()),
        AutomationCurve::Linear,
        Ticks::ZERO,
        0.75,
    ));
    assert!(!super::files::same_persisted_project(
        &changed_value,
        &saved
    ));
}

#[test]
fn legacy_open_then_ordinary_save_preserves_exact_backup_and_upgrades_in_place() {
    let scratch = Scratch::new("native-legacy-upgrade");
    let legacy = scratch.join("Legacy.auris");
    let mut source = session();
    source.add_default_instrument_track("Migrated").unwrap();
    let mut project = source.project().clone();
    let legacy_bytes = {
        auris_io::save_project(&legacy, &mut project).expect("the legacy fixture saves");
        std::fs::read(&legacy).unwrap()
    };

    let mut live = session();
    live.open(&legacy).expect("legacy files remain readable");
    live.save_in_place()
        .expect("ordinary Save upgrades the file");
    assert_eq!(format_version(&legacy), NATIVE_FORMAT_VERSION);
    let backup = legacy.with_file_name("Legacy.auris.legacy-backup");
    assert_eq!(
        std::fs::read(backup).expect("migration leaves a backup"),
        legacy_bytes,
        "the backup is byte-for-byte the original legacy source"
    );
}

#[test]
fn ordinary_checkpoint_uses_native_format_and_can_be_reopened() {
    let scratch = Scratch::new("native-checkpoint");
    let mut live = session();
    live.add_default_instrument_track("Checkpointed").unwrap();
    live.save_as(&scratch.join("Song.auris")).unwrap();

    let checkpoint = live
        .create_checkpoint("before_edit")
        .expect("the ordinary checkpoint command succeeds");
    assert_eq!(format_version(&checkpoint), NATIVE_FORMAT_VERSION);

    let mut reopened = session();
    reopened
        .open(&checkpoint)
        .expect("a native checkpoint can be opened through Session");
    assert!(
        reopened
            .project()
            .tracks
            .iter()
            .any(|track| track.name == "Checkpointed")
    );
}

#[test]
fn unsupported_native_graph_is_rejected_without_replacing_the_live_document() {
    let scratch = Scratch::new("native-unsupported-open");
    let mut source = session();
    source
        .add_default_instrument_track("Unsupported source")
        .unwrap();
    let mut native = auris_io::project_to_native(source.project(), content_limits())
        .expect("the conventional project converts to native");
    let transform_id = NodeId(native.content.next_id);
    native.content.next_id += 1;
    native.content.processors.push(Processor {
        id: transform_id,
        group: Some(native.content.master),
        kind: ProcessorKind::Transform(NoteTransform::Lean { ticks: 0 }),
    });
    let unsupported = scratch.join("Unsupported.auris");
    auris_io::save_native(&unsupported, &native, content_limits())
        .expect("the unsupported native graph still has a valid envelope");

    let mut live = session();
    live.add_default_instrument_track("Keep live").unwrap();
    live.save_as(&scratch.join("Live.auris")).unwrap();
    let before = live.project().clone();
    let error = live
        .open(&unsupported)
        .expect_err("a transform cannot be silently lowered to the conventional model");
    assert!(error.to_string().contains("represent") || error.to_string().contains("native"));
    assert_eq!(
        live.project(),
        &before,
        "rejected Open preserves the live document"
    );
}
