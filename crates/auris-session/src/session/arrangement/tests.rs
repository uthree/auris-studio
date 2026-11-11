use super::*;
use crate::session::fixtures::session;
use auris_core::{Note, TrackId};

static SERIAL: Mutex<()> = Mutex::new(());
pub(super) fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fixture() -> (Session, TrackId, ClipId, ClipId) {
    let mut session = session();
    let track = session
        .add_instrument_track("Keys", "auris.physical.piano")
        .unwrap();
    let verse = session
        .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    let chorus = session
        .add_midi_clip(track, "Chorus", Ticks::QUARTER * 4, Ticks::QUARTER * 4)
        .unwrap();
    for clip in [verse, chorus] {
        for pitch in [60, 64, 67] {
            session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
    }
    session.forget_history();
    (session, track, verse, chorus)
}

fn validate(session: &mut Session, changes: Value) -> Value {
    let revision = session.agent_document_revision().unwrap();
    let changes = serde_json::from_value(changes).unwrap();
    let result = session
        .begin_arrangement(revision, changes, &[])
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    serde_json::from_str(
        &session
            .continue_arrangement(result, &AtomicBool::new(false))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn validation_is_read_only_and_atomic_apply_is_retry_safe_and_one_undo() {
    let _serial = serial();
    let (mut session, track, verse, chorus) = fixture();
    let before = session.project().clone();
    let report = validate(
        &mut session,
        json!([
            {"purpose":"Give the chorus a chord stroke","edit":{"kind":"performance","clip":chorus.0,"settings":{"strum":{"spread_ms":30.0}}}},
            {"purpose":"Build energy inside the verse","edit":{"kind":"automation","target":{"kind":"track_gain","track":track.0},"start_tick":0,"end_tick":3840,"points":[{"at_tick":960,"value":-12.0},{"at_tick":3000,"value":-3.0}]}},
            {"purpose":"Mark the chorus arrival","edit":{"kind":"transition","transition":"crash","at_tick":3840,"duration_ticks":960,"name":"Chorus crash","gain_db":-12.0}}
        ]),
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    let response = session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["changed"],
        true
    );
    let after = session.project().clone();
    assert_eq!(
        after.midi_clip(verse).unwrap(),
        before.midi_clip(verse).unwrap()
    );
    assert_eq!(
        after.midi_clip(chorus).unwrap().1.notes,
        before.midi_clip(chorus).unwrap().1.notes
    );
    assert_eq!(after.tracks.len(), before.tracks.len() + 1);
    let retry = session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&retry).unwrap()["replayed"],
        true
    );
    assert_eq!(session.project(), &after);
    session.undo().unwrap();
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    assert!(
        session
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap()
            )
            .is_err()
    );
    session.redo().unwrap();
    assert_eq!(session.project(), &after);
}

#[test]
fn inspection_is_bounded_and_does_not_modify_history_or_score() {
    let _serial = serial();
    let (mut session, _, _, _) = fixture();
    let before = session.project().clone();
    let info: Value = serde_json::from_str(&session.inspect_arrangement(1, 2, 0).unwrap()).unwrap();
    assert_eq!(info["tracks"][0]["capabilities"]["family"], "keys");
    assert_eq!(
        info["tracks"][0]["capabilities"]["cc64_sustain"],
        "supported"
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
}

#[test]
fn revoice_moves_only_selected_section_and_preserves_score_curves_and_source_automation() {
    let _serial = serial();
    let (mut session, track, verse, chorus) = fixture();
    assert!(session.set_curve_point(
        chorus,
        auris_core::project::ClipCurve::Controller(64),
        Ticks::ZERO,
        1.0
    ));
    let descriptor = session.instrument_descriptors(track)[0].clone();
    let target = ParamTarget::Instrument {
        track,
        param: descriptor.id,
    };
    session.set_automation_point(target, Ticks::ZERO, descriptor.default);
    session.forget_history();
    let sound = session.arrangement_test_sound("guitar");
    let before = session.project().clone();
    let report = validate(
        &mut session,
        json!([{"purpose":"Give only the chorus a guitar voice","edit":{"kind":"revoice","clips":[chorus.0],"sound_id":sound,"name":"Chorus guitar","settings":{"strum":{"spread_ms":25.0}}}}]),
    );
    assert_eq!(session.project(), &before);
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let destination = TrackId(report["changes"][0]["result"]["track"].as_u64().unwrap());
    assert_ne!(destination, track);
    assert_eq!(session.project().midi_clip(chorus).unwrap().0, destination);
    assert_eq!(
        session.project().midi_clip(verse).unwrap(),
        before.midi_clip(verse).unwrap()
    );
    let copied = session.project().midi_clip(chorus).unwrap().1;
    let original = before.midi_clip(chorus).unwrap().1;
    assert_eq!(copied.notes, original.notes);
    assert_eq!(copied.controllers, original.controllers);
    assert_eq!(copied.recipe, original.recipe);
    assert_eq!(
        session.project().automation.lane(target),
        before.automation.lane(target)
    );
    assert_eq!(
        session.project().outputs(destination),
        before.outputs(track)
    );
    session.undo().unwrap();
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
}

#[test]
fn layering_keeps_source_and_reissues_every_copied_clip_identity() {
    let _serial = serial();
    let (mut session, track, verse, chorus) = fixture();
    let sound = session.arrangement_test_sound("guitar");
    let before = session.project().clone();
    let report = validate(
        &mut session,
        json!([{"purpose":"Layer the last chorus","edit":{"kind":"revoice","clips":[chorus.0],"sound_id":sound,"name":"Guitar layer","layer":true}}]),
    );
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(session.project().track(track), before.track(track));
    assert_eq!(session.project().midi_clip(verse), before.midi_clip(verse));
    let new_id = ClipId(report["changes"][0]["result"]["clips"][0].as_u64().unwrap());
    assert_ne!(new_id, chorus);
    assert_eq!(
        session.project().midi_clip(new_id).unwrap().1.notes,
        before.midi_clip(chorus).unwrap().1.notes
    );
}

#[test]
fn scoped_automation_preserves_values_before_and_after_including_existing_linear_ramps() {
    let _serial = serial();
    let (mut session, track, _, _) = fixture();
    let target = ParamTarget::TrackGain(track);
    session.set_automation_point(target, Ticks::ZERO, -20.0);
    session.set_automation_point(target, Ticks(10000), 0.0);
    session.forget_history();
    let before = session.project().automation.lane(target).unwrap().clone();
    let report = validate(
        &mut session,
        json!([{"purpose":"Swell inside the chorus only","edit":{"kind":"automation","target":{"kind":"track_gain","track":track.0},"start_tick":3000,"end_tick":6000,"points":[{"at_tick":4500,"value":-1.0}]}}]),
    );
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let after = session.project().automation.lane(target).unwrap();
    for tick in [0, 500, 2999, 3000, 6000, 6001, 9999, 12000] {
        assert!(
            (before.value_at(Ticks(tick)) - after.value_at(Ticks(tick))).abs() < 1e-5,
            "changed outside passage at {tick}"
        );
    }
    assert_eq!(after.value_at(Ticks(4500)), -1.0);
}

#[test]
fn stale_wrong_owner_cancelled_and_repeated_targets_cannot_edit_the_document() {
    let _serial = serial();
    let (mut session, _, verse, _) = fixture();
    let changes = json!([{"purpose":"Shorter articulation","edit":{"kind":"performance","clip":verse.0,"settings":{"gate":0.5}}}]);
    let revision = session.agent_document_revision().unwrap();
    let result = session
        .begin_arrangement(
            revision.clone(),
            serde_json::from_value(changes.clone()).unwrap(),
            &[],
        )
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    let (mut other, _, _, _) = fixture();
    assert!(
        other
            .continue_arrangement(result, &AtomicBool::new(false))
            .is_err()
    );
    let job = session
        .begin_arrangement(
            revision,
            serde_json::from_value(changes.clone()).unwrap(),
            &[],
        )
        .unwrap();
    assert!(job.run(&AtomicBool::new(true)).is_err());
    let report = validate(&mut session, changes.clone());
    let before = session.project().clone();
    assert!(
        other
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap()
            )
            .is_err()
    );
    session
        .rename_track(session.project().tracks[0].id, "Human edit")
        .unwrap();
    let edited = session.project().clone();
    assert!(
        session
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap()
            )
            .is_err()
    );
    assert_eq!(session.project(), &edited);
    let repeated = json!([changes[0].clone(), changes[0].clone()]);
    assert!(
        session
            .begin_arrangement(
                session.agent_document_revision().unwrap(),
                serde_json::from_value(repeated).unwrap(),
                &[]
            )
            .is_err()
    );
    assert_ne!(session.project(), &before);
}

#[test]
fn source_budget_counts_the_actual_device_rate_buffer() {
    let scratch = tempfile::tempdir().unwrap();
    let path = crate::session::fixtures::write_tone(&scratch.path().join("effect.wav"), 4800);
    let buffer = crate::decode_audio(&path, 48_000.0).unwrap();
    let channels = buffer.channel_count();
    let source_bytes = 4800 * channels * std::mem::size_of::<f32>();
    let same = super::super::prepare_audio_source(buffer.clone(), 48_000.0).unwrap();
    let converted = super::super::prepare_audio_source(buffer, 192_000.0).unwrap();
    assert!(same.retained_bytes() >= source_bytes);
    assert!(converted.retained_bytes() >= source_bytes * 5);
    assert!(converted.retained_bytes() > same.retained_bytes() * 2);
}

#[test]
fn audio_sfx_is_frozen_aligned_and_prepared_without_touching_the_source_or_score() {
    let _serial = serial();
    let scratch = tempfile::tempdir().unwrap();
    let path = crate::session::fixtures::write_tone(&scratch.path().join("impact.wav"), 4800);
    let bytes = std::fs::read(&path).unwrap();
    let (mut session, _, _, _) = fixture();
    let before = session.project().clone();
    let report = validate(
        &mut session,
        json!([{"purpose":"Land the recording at the chorus","edit":{"kind":"audio_sfx","path":path,"at_tick":3840,"alignment":"end","name":"Recorded impact","gain_db":-9.0,"fade_in_ms":2.0,"fade_out_ms":5.0}}]),
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    std::fs::remove_file(&path).unwrap();
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let id = auris_core::SourceId(report["changes"][0]["result"]["source"].as_u64().unwrap());
    let source = &session.project().audio_sources[&id];
    let retained = source.path.resolve(session.project_folder()).unwrap();
    assert_eq!(std::fs::read(retained).unwrap(), bytes);
    assert_eq!(session.bank().get(id).unwrap().frame_count(), 4800);
    assert!(
        report["changes"][0]["result"]["alignment_error_ms"]
            .as_f64()
            .unwrap()
            .abs()
            < 1.0
    );
    let after = session.project().clone();
    session.undo().unwrap();
    assert_eq!(session.project(), &before);
    session.redo().unwrap();
    assert_eq!(session.project(), &after);
}

#[test]
fn a_late_failure_discards_private_sfx_and_never_adds_history() {
    let _serial = serial();
    let scratch = tempfile::tempdir().unwrap();
    let path = crate::session::fixtures::write_tone(&scratch.path().join("impact.wav"), 4800);
    let (mut session, _, _, _) = fixture();
    let before = session.project().clone();
    let directory = session.project_folder().unwrap().join(auris_io::AUDIO_DIR);
    let changes = json!([
        {"purpose":"Prepare an effect","edit":{"kind":"audio_sfx","path":path,"at_tick":3840,"alignment":"start","name":"Effect","gain_db":-9.0,"fade_in_ms":0.0,"fade_out_ms":0.0}},
        {"purpose":"Reject an impossible riser","edit":{"kind":"transition","transition":"riser","at_tick":0,"duration_ticks":960,"name":"Impossible","gain_db":-12.0}}
    ]);
    let result = session
        .begin_arrangement(
            session.agent_document_revision().unwrap(),
            serde_json::from_value(changes).unwrap(),
            &[],
        )
        .unwrap()
        .run(&AtomicBool::new(false));
    assert!(result.is_err());
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

#[test]
fn unchanged_proposals_do_not_add_history_or_advance_the_session_revision() {
    let _serial = serial();
    let (mut session, _, verse, _) = fixture();
    let report = validate(
        &mut session,
        json!([{"purpose":"Keep the phrase audible","edit":{"kind":"clip_mute","clip":verse.0,"muted":false}}]),
    );
    let revision = session.revision();
    let response: Value = serde_json::from_str(
        &session
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["changed"], false);
    assert_eq!(session.revision(), revision);
    assert!(!session.can_undo());
}

#[test]
fn saved_session_reopening_retains_plan_identity_and_exact_retry_receipts() {
    let _serial = serial();
    let scratch = tempfile::tempdir().unwrap();
    let (mut original, _, verse, _) = fixture();
    original
        .save_as(&scratch.path().join("Song.auris"))
        .unwrap();
    let path = original.path().unwrap().to_owned();
    let report = validate(
        &mut original,
        json!([{"purpose":"Shorten verse articulation","edit":{"kind":"performance","clip":verse.0,"settings":{"gate":0.5}}}]),
    );
    let disk_before = std::fs::read(&path).unwrap();
    drop(original);
    let mut reopened = session();
    reopened.open(&path).unwrap();
    let response: Value = serde_json::from_str(
        &reopened
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["changed"], true);
    reopened.save(&path).unwrap();
    assert_ne!(std::fs::read(&path).unwrap(), disk_before);
    drop(reopened);
    let mut again = session();
    again.open(&path).unwrap();
    let response: Value = serde_json::from_str(
        &again
            .apply_arrangement(
                report["plan_id"].as_str().unwrap(),
                report["revision"].as_str().unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["replayed"], true);
    assert!(!again.is_dirty());
    assert!(!again.can_undo());
}

#[test]
fn expired_plans_and_open_gestures_leave_the_score_untouched() {
    let _serial = serial();
    let (mut session, _, verse, _) = fixture();
    let report = validate(
        &mut session,
        json!([{"purpose":"Shorter verse","edit":{"kind":"performance","clip":verse.0,"settings":{"gate":0.5}}}]),
    );
    let id = report["plan_id"].as_str().unwrap();
    let revision = report["revision"].as_str().unwrap();
    let before = session.project().clone();
    session.begin_transaction(crate::Edit::ExternalChanges);
    assert!(session.apply_arrangement(id, revision).is_err());
    session.end_transaction();
    {
        let cache = PLANS.get_or_init(Default::default).lock().unwrap();
        let entry = cache
            .iter()
            .find(|entry| entry.lock().unwrap().id == id)
            .unwrap();
        entry.lock().unwrap().created = Instant::now() - TTL - Duration::from_secs(1);
    }
    assert!(session.apply_arrangement(id, revision).is_err());
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
}

#[test]
fn controller_changes_are_measured_even_when_note_events_do_not_change() {
    let _serial = serial();
    let (mut session, _, verse, _) = fixture();
    session.remove_notes(verse, &[0, 1, 2]).unwrap();
    session
        .add_note(verse, Note::new(60, Ticks::ZERO, Ticks::QUARTER * 4))
        .unwrap();
    let report = validate(
        &mut session,
        json!([{"purpose":"Swell a held phrase","edit":{"kind":"performance","clip":verse.0,"settings":{"pitch":{"volume_swell":0.8}}}}]),
    );
    let delta = &report["changes"][0]["result"]["delta"];
    assert_eq!(delta["complete"], true);
    assert_eq!(delta["note_events_changed"], false);
    assert!(
        delta["curves"]
            .as_array()
            .unwrap()
            .iter()
            .any(|curve| curve["points_changed"] == true)
    );
}

#[test]
fn real_render_changes_after_revoice_and_preserves_the_authored_phrase() {
    let _serial = serial();
    let (mut session, _, _, chorus) = fixture();
    let options = crate::prelude::OfflineOptions {
        start_frames: 0,
        end_frames: Some(192000),
        include_tail: false,
        ..Default::default()
    };
    let before = session
        .render_job()
        .render(&options, &mut crate::prelude::RenderProgress::default())
        .unwrap();
    let sound = session.arrangement_test_sound("guitar");
    let report = validate(
        &mut session,
        json!([{"purpose":"Change chorus tone and stroke","edit":{"kind":"revoice","clips":[chorus.0],"sound_id":sound,"name":"Guitar","settings":{"strum":{"spread_ms":25.0}}}}]),
    );
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let after = session
        .render_job()
        .render(&options, &mut crate::prelude::RenderProgress::default())
        .unwrap();
    assert_eq!(before.frame_count(), after.frame_count());
    let delta = before
        .channels()
        .iter()
        .zip(after.channels())
        .flat_map(|(a, b)| a.iter().zip(b))
        .map(|(a, b)| f64::from(a - b).powi(2))
        .sum::<f64>();
    assert!(
        delta > 1e-6,
        "revoicing must change actual rendered samples"
    );
    assert!(
        after
            .channels()
            .iter()
            .flatten()
            .all(|value| value.is_finite())
    );
}
