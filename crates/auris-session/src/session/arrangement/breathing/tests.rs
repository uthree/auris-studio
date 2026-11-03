use super::*;
use crate::session::fixtures::session;
use auris_core::{Note, TrackId};

fn lead() -> (Session, TrackId, ClipId) {
    let mut session = session();
    let track = session
        .add_instrument_track("Lead", "auris.synth.chiptune")
        .unwrap();
    let clip = session
        .add_midi_clip(track, "Lead phrase", Ticks::ZERO, Ticks(15360))
        .unwrap();
    for index in 0..16 {
        session
            .add_note(
                clip,
                Note::new(60 + (index % 5) as u8, Ticks(index * 960), Ticks(960)),
            )
            .unwrap();
    }
    session.forget_history();
    (session, track, clip)
}

fn read(session: &Session, track: TrackId, threshold: i64) -> Value {
    inspect(
        session.project(),
        track.0,
        MelodicRole::Lead,
        0,
        15360,
        threshold,
        &[7680, 15360],
    )
    .unwrap()
}

fn proposal(session: &mut Session, clip: ClipId, extra: Value) -> Result<Value, String> {
    let mut edit = json!({"kind":"breath","clips":[clip.0],"rests":[{"start_tick":14880,"end_tick":15360}],"min_note_ticks":120});
    edit.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let changes = serde_json::from_value(
        json!([{"purpose":"Release the final phrase before its answer","edit":edit}]),
    )
    .unwrap();
    let result = session
        .begin_arrangement(session.agent_document_revision().unwrap(), changes, &[])?
        .run(&AtomicBool::new(false))?;
    serde_json::from_str(&session.continue_arrangement(result, &AtomicBool::new(false))?)
        .map_err(|e| e.to_string())
}

#[test]
fn held_arrival_replaces_only_explicit_approach_and_remains_one_undo() {
    let (mut session, track, clip) = lead();
    let original = session.project().clone();
    let plan = proposal(&mut session, clip, json!({"landings":[{"note":{"clip":clip.0,"index":15},"start_tick":13440,"replace_notes":[14]}]})).unwrap();
    assert_eq!(session.project(), &original);
    session
        .apply_arrangement(
            plan["plan_id"].as_str().unwrap(),
            plan["revision"].as_str().unwrap(),
        )
        .unwrap();
    let mut expected = original.clone();
    let notes = &mut expected.midi_clip_mut(clip).unwrap().notes;
    notes[15].start = Ticks(13440);
    notes[15].length = Ticks(1440);
    notes.remove(14);
    assert_eq!(session.project(), &expected);
    assert_eq!(read(&session, track, 480)["performed"]["rest_count"], 1);
    session.undo();
    assert_eq!(session.project(), &original);
    assert!(!session.can_undo());
}

#[test]
fn unsafe_landings_refuse_without_publishing_a_partial_candidate() {
    for extra in [
        json!({"protected_notes":[{"clip":1,"index":14}]}),
        json!({"lyric":true}),
        json!({"loop":true}),
        json!({"overlap":true}),
        json!({"replace_notes":[13]}),
        json!({"replace_notes":[14,14]}),
    ] {
        let (mut session, _, clip) = lead();
        let mut edit = json!({"landings":[{"note":{"clip":clip.0,"index":15},"start_tick":13440,"replace_notes":[14]}]});
        if extra.get("protected_notes").is_some() {
            edit["protected_notes"] = json!([{"clip":clip.0,"index":14}]);
        }
        if extra["lyric"] == true {
            session.project.midi_clip_mut(clip).unwrap().notes[14].lyric = "protected".into();
        }
        if extra["loop"] == true {
            session.project.midi_clip_mut(clip).unwrap().loop_end = Ticks(30720);
        }
        if extra["overlap"] == true {
            session
                .project
                .midi_clip_mut(clip)
                .unwrap()
                .notes
                .push(Note::new(70, Ticks(13440), Ticks(960)));
        }
        if let Some(indices) = extra.get("replace_notes") {
            edit["landings"][0]["replace_notes"] = indices.clone();
        }
        let before = session.project().clone();
        assert!(proposal(&mut session, clip, edit).is_err(), "{extra}");
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }
}

#[test]
fn earlier_landing_can_release_before_its_old_attack_to_allow_a_long_tail() {
    let (mut session, track, clip) = lead();
    let plan = proposal(&mut session, clip, json!({"rests":[{"start_tick":12000,"end_tick":15360}],"landings":[{"note":{"clip":clip.0,"index":15},"start_tick":9600,"replace_notes":[10,11,12,13,14]}]})).unwrap();
    session
        .apply_arrangement(
            plan["plan_id"].as_str().unwrap(),
            plan["revision"].as_str().unwrap(),
        )
        .unwrap();
    let notes = &session.project().midi_clip(clip).unwrap().1.notes;
    assert_eq!(notes.len(), 11);
    assert_eq!(notes[10], Note::new(60, Ticks(9600), Ticks(2400)));
    assert_eq!(read(&session, track, 3360)["performed"]["rest_count"], 1);
}

#[test]
fn the_audio_sampler_lists_its_envelope_and_a_soundfont_does_not() {
    let mut session = session();
    let audio = session
        .add_instrument_track("Sampled", auris_sampler::AUDIO_SAMPLER_ID)
        .unwrap();
    let controls = envelope_controls(session.registry(), session.project(), audio.0);
    let entries = controls.as_array().unwrap();
    let keys: Vec<_> = entries.iter().map(|c| c["key"].clone()).collect();
    assert_eq!(keys, ["attack", "decay", "sustain", "release"]);
    assert!((entries[3]["base_value"].as_f64().unwrap() - 0.1).abs() < 1e-5);
    for entry in entries {
        let address: automation::ControlTarget =
            serde_json::from_value(entry["target"].clone()).unwrap();
        assert!(session.agent_control_target(&address).is_ok());
    }

    let font = session
        .add_instrument_track("Font", auris_sampler::SAMPLER_ID)
        .unwrap();
    assert!(envelope_controls(session.registry(), session.project(), font.0).is_null());
}

#[test]
fn ending_controls_use_registered_defaults_and_distinguish_instrument_decay() {
    for (id, family, release) in [
        ("auris.synth.chiptune", "synth", 0.15),
        ("auris.physical.piano", "keys", 0.2),
        ("auris.physical.clarinet", "winds", 0.24),
        ("auris.physical.violin", "bowed_strings", 1.5),
    ] {
        let mut session = session();
        let track = session.add_instrument_track("Lead", id).unwrap();
        let controls = envelope_controls(session.registry(), session.project(), track.0);
        let value = controls
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == "release")
            .unwrap();
        let address: automation::ControlTarget =
            serde_json::from_value(value["target"].clone()).unwrap();
        assert!(session.agent_control_target(&address).is_ok());
        assert!((value["base_value"].as_f64().unwrap() - release).abs() < 1e-5);
        assert_eq!(
            endings::guidance(session.project(), session.project().track(track).unwrap())["family"],
            family
        );
    }
}

#[test]
fn continuous_and_merely_detached_notes_have_no_phrase_breath() {
    let (mut session, track, clip) = lead();
    let before = session.project().clone();
    let continuous = read(&session, track, 480);
    assert_eq!(continuous["performed"]["occupancy"], 1.0);
    assert_eq!(
        continuous["performed"]["longest_without_phrase_rest"]["ticks"],
        15360
    );
    assert_eq!(continuous["performed"]["rest_count"], 0);
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    session
        .set_clip_transforms(clip, vec![NoteTransform::Gate { amount: 0.9 }])
        .unwrap();
    let detached = read(&session, track, 480);
    assert_eq!(detached["authored"]["occupancy"], 1.0);
    assert!(detached["performed"]["occupancy"].as_f64().unwrap() < 1.0);
    assert_eq!(detached["performed"]["rest_count"], 0);
    assert!(
        detached["performed"]["longest_without_phrase_rest"]["ticks"]
            .as_i64()
            .unwrap()
            > 15000
    );
}

#[test]
fn overlaps_are_unioned_and_clip_joins_and_loop_passes_are_continuous() {
    let (mut session, track, clip) = lead();
    session
        .add_note(clip, Note::new(72, Ticks::ZERO, Ticks(15360)))
        .unwrap();
    assert_eq!(read(&session, track, 480)["performed"]["occupancy"], 1.0);
    let mut project = session.project().clone();
    project.midi_clip_mut(clip).unwrap().loop_end = Ticks(30720);
    let repeated = inspect(
        &project,
        track.0,
        MelodicRole::Lead,
        0,
        30720,
        480,
        &[15360, 30720],
    )
    .unwrap();
    assert_eq!(repeated["performed"]["longest_continuous"]["ticks"], 30720);
    project.midi_clip_mut(clip).unwrap().loop_end = Ticks::ZERO;
    let next = project
        .add_midi_clip(track, "Continuation", Ticks(15360), Ticks(15360))
        .unwrap();
    project
        .midi_clip_mut(next)
        .unwrap()
        .notes
        .push(Note::new(62, Ticks::ZERO, Ticks(15360)));
    let joined = inspect(
        &project,
        track.0,
        MelodicRole::Lead,
        0,
        30720,
        480,
        &[15360, 30720],
    )
    .unwrap();
    assert_eq!(joined["performed"]["longest_continuous"]["ticks"], 30720);
    project.midi_clip_mut(next).unwrap().muted = true;
    let muted = inspect(
        &project,
        track.0,
        MelodicRole::Lead,
        0,
        30720,
        480,
        &[15360, 30720],
    )
    .unwrap();
    assert_eq!(muted["performed"]["occupied_ticks"], 15360);
}

#[test]
fn tempo_changes_and_texture_role_do_not_invent_a_universal_rest_quota() {
    let (mut session, track, clip) = lead();
    session.set_tempo_point(Ticks::ZERO, 120.0);
    session.set_tempo_point(Ticks(7680), 60.0);
    let info = read(&session, track, 480);
    assert_eq!(info["performed"]["longest_continuous"]["seconds"], 12.0);
    let texture = inspect(
        session.project(),
        track.0,
        MelodicRole::Texture,
        0,
        15360,
        480,
        &[7680, 15360],
    )
    .unwrap();
    assert!(texture["suggestions"].as_array().unwrap().is_empty());
    let mut project = session.project().clone();
    project
        .midi_clip_mut(clip)
        .unwrap()
        .notes
        .last_mut()
        .unwrap()
        .length = Ticks(480);
    let breathed = inspect(
        &project,
        track.0,
        MelodicRole::Lead,
        0,
        15360,
        480,
        &[15360],
    )
    .unwrap();
    assert_eq!(
        breathed["performed"]["phrase_endings"][0]["preceding_space"]["seconds"],
        0.5
    );
    assert_eq!(
        breathed["performed"]["phrase_endings"][0]["has_phrase_rest"],
        true
    );
}

#[test]
fn a_previewed_release_is_exact_retry_safe_and_one_undo() {
    let _serial = super::super::tests::serial();
    let (mut session, track, clip) = lead();
    let before = session.project().clone();
    assert!(proposal(&mut session,clip,json!({"rests":[{"start_tick":14880,"end_tick":15120},{"start_tick":15120,"end_tick":15360}]})).is_ok());
    let report = proposal(&mut session, clip, json!({})).unwrap();
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    let result = &report["changes"][0]["result"];
    assert_eq!(result["note_changes"][0]["index"], 15);
    assert_eq!(result["note_changes"][0]["after_length_ticks"], 480);
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let after = session.project().clone();
    let mut expected = before.clone();
    expected.midi_clip_mut(clip).unwrap().notes[15].length = Ticks(480);
    assert_eq!(after, expected);
    assert_eq!(read(&session, track, 480)["performed"]["rest_count"], 1);
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
    session.undo().unwrap();
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    session.redo().unwrap();
    assert_eq!(session.project(), &after);
}

#[test]
fn pickups_require_exact_omission_indices_and_protected_notes_are_never_deleted() {
    let _serial = super::super::tests::serial();
    let (mut session, _, clip) = lead();
    let before = session.project().clone();
    let rest = json!([{"start_tick":14400,"end_tick":15360}]);
    assert!(
        proposal(&mut session, clip, json!({"rests":rest}))
            .unwrap_err()
            .contains("omit_notes")
    );
    assert_eq!(session.project(), &before);
    let report = proposal(
        &mut session,
        clip,
        json!({"rests":rest,"omit_notes":[{"clip":clip.0,"index":15}]}),
    )
    .unwrap();
    assert_eq!(
        report["changes"][0]["result"]["note_changes"][0]["kind"],
        "omit"
    );
    assert!(proposal(&mut session,clip,json!({"rests":rest,"protected_notes":[{"clip":clip.0,"index":15}],"omit_notes":[{"clip":clip.0,"index":15}]})).is_err());
    assert!(
        proposal(
            &mut session,
            clip,
            json!({"protected_notes":[{"clip":clip.0,"index":15}]})
        )
        .is_err()
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let mut expected = before;
    expected.midi_clip_mut(clip).unwrap().notes.pop();
    assert_eq!(session.project(), &expected);
}

#[test]
fn lyrics_ties_pedal_hidden_pickups_and_unselected_layers_cannot_be_silently_rewritten() {
    let _serial = super::super::tests::serial();
    for condition in [
        "lyrics",
        "tie",
        "pedal",
        "layer",
        "loop",
        "tiny",
        "unused_index",
    ] {
        let (mut session, track, clip) = lead();
        let mut extra = json!({});
        match condition {
            "lyrics" => {
                session.project.midi_clip_mut(clip).unwrap().notes[15].lyric = "keep".into()
            }
            "tie" => session.project.midi_clip_mut(clip).unwrap().notes[14].pitch = 60,
            "pedal" => {
                session
                    .project
                    .track_mut(track)
                    .unwrap()
                    .kind
                    .as_instrument_mut()
                    .unwrap()
                    .instrument_id = "auris.physical.piano".into();
                session.set_curve_point(clip, ClipCurve::Controller(64), Ticks::ZERO, 1.0);
            }
            "layer" => {
                let layer = session
                    .add_midi_clip(track, "Answer", Ticks(14880), Ticks(480))
                    .unwrap();
                session
                    .add_note(layer, Note::new(72, Ticks::ZERO, Ticks(480)))
                    .unwrap();
            }
            "loop" => session.project.midi_clip_mut(clip).unwrap().loop_end = Ticks(30720),
            "tiny" => extra = json!({"min_note_ticks":960}),
            "unused_index" => extra = json!({"omit_notes":[{"clip":clip.0,"index":1}]}),
            _ => unreachable!(),
        }
        session.forget_history();
        let before = session.project().clone();
        assert!(proposal(&mut session, clip, extra).is_err(), "{condition}");
        assert_eq!(session.project(), &before, "{condition}");
        assert!(!session.can_undo(), "{condition}");
    }
}

#[test]
fn neighboring_clip_ties_and_generated_brushes_cannot_fill_a_claimed_breath() {
    let _serial = super::super::tests::serial();
    let (mut session, track, clip) = lead();
    let next = session
        .add_midi_clip(track, "Tied continuation", Ticks(15360), Ticks(960))
        .unwrap();
    session
        .add_note(next, Note::new(60, Ticks::ZERO, Ticks(960)))
        .unwrap();
    session.forget_history();
    assert!(
        proposal(&mut session, clip, json!({}))
            .unwrap_err()
            .contains("same-pitch")
    );
    session.project.midi_clip_mut(next).unwrap().muted = true;
    session
        .set_clip_transforms(clip, vec![NoteTransform::Brush { amount: 1.0 }])
        .unwrap();
    session.forget_history();
    let before = session.project().clone();
    assert!(
        proposal(&mut session, clip, json!({}))
            .unwrap_err()
            .contains("Performed notes still fill")
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
}

#[test]
fn a_loop_repair_requires_explicit_rest_windows_for_every_repeated_pass() {
    let _serial = super::super::tests::serial();
    let (mut session, track, clip) = lead();
    // Make the loop join a pitch change rather than a possible tie.
    session.project.midi_clip_mut(clip).unwrap().notes[0].pitch = 72;
    session.project.midi_clip_mut(clip).unwrap().loop_end = Ticks(30720);
    session
        .project
        .track_mut(track)
        .unwrap()
        .kind
        .as_instrument_mut()
        .unwrap()
        .instrument_id = "auris.physical.piano".into();
    session.set_curve_point(clip, ClipCurve::Controller(64), Ticks::ZERO, 0.0);
    let before = session.project().clone();
    assert!(
        proposal(&mut session, clip, json!({}))
            .unwrap_err()
            .contains("unrequested loop pass")
    );
    let report=proposal(&mut session,clip,json!({"rests":[{"start_tick":14880,"end_tick":15360},{"start_tick":30240,"end_tick":30720}]})).unwrap();
    assert_eq!(session.project(), &before);
    session
        .apply_arrangement(
            report["plan_id"].as_str().unwrap(),
            report["revision"].as_str().unwrap(),
        )
        .unwrap();
    let mut expected = before;
    expected.midi_clip_mut(clip).unwrap().notes[15].length = Ticks(480);
    assert_eq!(session.project(), &expected);
    let info = inspect(
        session.project(),
        track.0,
        MelodicRole::Lead,
        0,
        30720,
        480,
        &[15360, 30720],
    )
    .unwrap();
    assert_eq!(info["performed"]["rest_count"], 2);
    session.set_curve_point(clip, ClipCurve::Controller(64), Ticks(15000), 1.0);
    let before_refusal = session.project().clone();
    assert!(proposal(&mut session,clip,json!({"rests":[{"start_tick":14880,"end_tick":15360},{"start_tick":30240,"end_tick":30720}]})).unwrap_err().contains("Sustain"));
    assert_eq!(session.project(), &before_refusal);
}

#[test]
fn already_notated_space_with_a_supported_pedal_requires_controller_repair() {
    let _serial = super::super::tests::serial();
    let (mut session, track, clip) = lead();
    session.project.midi_clip_mut(clip).unwrap().notes[15].length = Ticks(480);
    session.set_curve_point(clip, ClipCurve::Controller(64), Ticks::ZERO, 1.0);
    // Chiptune ignores sustain: the authored lane alone does not prove an audible pedal.
    assert!(proposal(&mut session, clip, json!({})).is_ok());
    session
        .project
        .track_mut(track)
        .unwrap()
        .kind
        .as_instrument_mut()
        .unwrap()
        .instrument_id = "auris.physical.piano".into();
    session.forget_history();
    let before = session.project().clone();
    let info = read(&session, track, 480);
    assert_eq!(
        info["authored"]["phrase_endings"][1]["has_phrase_rest"],
        true
    );
    assert_eq!(info["audibility"]["sustain_data_present"], true);
    assert!(
        proposal(&mut session, clip, json!({}))
            .unwrap_err()
            .contains("Sustain")
    );
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    // A linear pedal rise can cross its activation threshold between stored points.
    session.set_curve_point(clip, ClipCurve::Controller(64), Ticks::ZERO, 0.0);
    session.set_curve_point(clip, ClipCurve::Controller(64), Ticks(30000), 1.0);
    assert!(
        proposal(&mut session, clip, json!({}))
            .unwrap_err()
            .contains("Sustain")
    );
}

#[test]
fn releasing_a_foreground_phrase_changes_rendered_energy_without_silencing_the_mix() {
    let _serial = super::super::tests::serial();
    let (mut session, track, clip) = lead();
    let options = crate::prelude::OfflineOptions {
        end_frames: Some(384000),
        include_tail: false,
        ..Default::default()
    };
    let before = session
        .render_job()
        .render(&options, &mut crate::prelude::RenderProgress::default())
        .unwrap();
    let report = proposal(&mut session, clip, json!({})).unwrap();
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
    let energy = |buffer: &auris_core::AudioBuffer| {
        buffer
            .channels()
            .iter()
            .flat_map(|c| &c[380000..384000])
            .map(|s| f64::from(*s).powi(2))
            .sum::<f64>()
    };
    let before_energy = energy(&before);
    let after_energy = energy(&after);
    eprintln!("breathing rest energy: before={before_energy}, after={after_energy}");
    assert!(before_energy > 1e-5);
    assert!(
        after_energy < before_energy * 0.5,
        "{after_energy} vs {before_energy}"
    );
    let pad = session
        .add_instrument_track("Sustained response", "auris.synth.chiptune")
        .unwrap();
    let pad_clip = session
        .add_midi_clip(pad, "Texture", Ticks::ZERO, Ticks(15360))
        .unwrap();
    session
        .add_note(pad_clip, Note::new(48, Ticks::ZERO, Ticks(15360)))
        .unwrap();
    let mix = session
        .render_job()
        .render(&options, &mut crate::prelude::RenderProgress::default())
        .unwrap();
    assert!(energy(&mix) > after_energy + 1e-5);
    assert_eq!(read(&session, track, 480)["performed"]["rest_count"], 1);
    assert!(mix.channels().iter().flatten().all(|s| s.is_finite()));
}

#[test]
#[ignore = "sends real audio to the configured critic; requires a fresh AURIS_BREATHING_REVIEW_OUTPUT directory"]
fn installed_audio_critic_compares_breathing_stem_and_mix() {
    let _serial = super::super::tests::serial();
    let folder = PathBuf::from(
        std::env::var_os("AURIS_BREATHING_REVIEW_OUTPUT").expect("explicit audio review output"),
    );
    assert!(folder.is_absolute() && !folder.exists());
    std::fs::create_dir_all(&folder).unwrap();
    let (mut session, _, clip) = lead();
    let original = session.project().clone();
    let options = crate::prelude::OfflineOptions {
        start_frames: 96000,
        end_frames: Some(192000),
        sample_rate: Some(24000.0),
        include_tail: false,
        ..Default::default()
    };
    let write = |session: &mut Session, name: &str| {
        let buffer = session
            .render_job()
            .render(&options, &mut crate::prelude::RenderProgress::default())
            .unwrap();
        let path = folder.join(format!("{name}.wav"));
        auris_io::write_wav(&path, &buffer, &auris_io::WavExportSettings::default()).unwrap();
        path
    };
    let stem_before = write(&mut session, "stem-before");
    let proposal = proposal(&mut session, clip, json!({})).unwrap();
    session
        .apply_arrangement(
            proposal["plan_id"].as_str().unwrap(),
            proposal["revision"].as_str().unwrap(),
        )
        .unwrap();
    let repaired = session.project().clone();
    let stem_after = write(&mut session, "stem-after");
    let pad = session
        .add_instrument_track("Intentional pad", "auris.synth.chiptune")
        .unwrap();
    let pad_clip = session
        .add_midi_clip(pad, "Keep the texture", Ticks::ZERO, Ticks(15360))
        .unwrap();
    session
        .add_note(pad_clip, Note::new(48, Ticks::ZERO, Ticks(15360)))
        .unwrap();
    session.set_param(ParamTarget::TrackGain(pad), -18.0);
    let mix_after = write(&mut session, "mix-after");
    session.project.midi_clip_mut(clip).unwrap().notes =
        original.midi_clip(clip).unwrap().1.notes.clone();
    let mix_before = write(&mut session, "mix-before");
    let settings = crate::audio_review::AudioReviewOptions::from_env().unwrap();
    let mut reviews = Vec::new();
    for (label, before, after) in [
        ("foreground", stem_before, stem_after),
        ("mix with unchanged pad", mix_before, mix_after),
    ] {
        let result = crate::audio_review::review_audio(
            &[("Before", before.as_path()), ("After", after.as_path())],
            "Compare these two recordings of the same four-second phrase. Discuss whether the ending of the foreground melody has breathing space, whether the repeated motif stays recognizable, and whether the change sounds like unwanted staccato. State uncertainty. In a mix a sustained background may continue intentionally; do not equate breathing with total silence. Do not assume the edited version is better.",
            &settings,
        );
        let failed = result.is_err();
        reviews.push(match result {Ok(review)=>json!({"scope":label,"model":review.model,"audio_sent":review.audio_sent,"review":review.review}),Err(error)=>json!({"scope":label,"error":error})});
        if failed {
            break;
        }
    }
    std::fs::write(folder.join("review.json"),serde_json::to_vec_pretty(&json!({"reviews":reviews,"limitation":"Untrusted separate model observations, not a human listening judgment or a release gate","source_score":original,"repaired_score":repaired})).unwrap()).unwrap();
    assert_eq!(reviews.len(), 2, "see {}", folder.display());
    assert!(
        reviews.iter().all(|r| r["audio_sent"] == true),
        "see {}",
        folder.display()
    );
}

#[test]
#[ignore = "renders four native instruments and sends paired audio; requires fresh AURIS_BREATHING_REVIEW_OUTPUT"]
fn installed_audio_critic_compares_natural_phrase_endings() {
    let _serial = super::super::tests::serial();
    let folder =
        PathBuf::from(std::env::var_os("AURIS_BREATHING_REVIEW_OUTPUT").expect("explicit output"));
    assert!(folder.is_absolute() && !folder.exists());
    std::fs::create_dir_all(&folder).unwrap();
    let settings = crate::audio_review::AudioReviewOptions::from_env().unwrap();
    let mut records = Vec::new();
    for (name, id, bowed) in [
        ("synth", "auris.synth.chiptune", false),
        ("piano", "auris.physical.piano", false),
        ("clarinet", "auris.physical.clarinet", false),
        ("violin", "auris.physical.violin", true),
    ] {
        let (mut session, track, clip) = lead();
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_id = id.into();
        session.project.midi_clip_mut(clip).unwrap().length = Ticks(19200);
        session
            .add_note(clip, Note::new(67, Ticks(15360), Ticks(960)))
            .unwrap();
        let original = session.project().clone();
        let controls = envelope_controls(session.registry(), &original, track.0);
        let options = crate::prelude::OfflineOptions {
            start_frames: 96000,
            end_frames: Some(240000),
            sample_rate: Some(24000.0),
            include_tail: false,
            ..Default::default()
        };
        let write = |session: &mut Session, variant: &str| {
            let buffer = session
                .render_job()
                .render(&options, &mut crate::prelude::RenderProgress::default())
                .unwrap();
            let path = folder.join(format!("{name}-{variant}.wav"));
            auris_io::write_wav(&path, &buffer, &auris_io::WavExportSettings::default()).unwrap();
            // Time bins retain the real decay trajectory, without gain normalization.
            let bins: Vec<f64> = buffer.channels()[0]
                .chunks(1200)
                .map(|samples| {
                    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                        / samples.len() as f64)
                        .sqrt()
                })
                .collect();
            (path, bins)
        };
        let continuous = write(&mut session, "continuous-stem");
        let hard = proposal(&mut session, clip, json!({})).unwrap();
        session
            .apply_arrangement(
                hard["plan_id"].as_str().unwrap(),
                hard["revision"].as_str().unwrap(),
            )
            .unwrap();
        let hard_project = session.project().clone();
        let hard_stem = write(&mut session, "shortened-stem");
        session.undo();
        assert_eq!(session.project(), &original);
        let edits = if bowed {
            json!({"rests":[{"start_tick":13920,"end_tick":15360}],"landings":[{"note":{"clip":clip.0,"index":15},"start_tick":12480,"replace_notes":[13,14]}]})
        } else {
            json!({"landings":[{"note":{"clip":clip.0,"index":15},"start_tick":13440,"replace_notes":[14]}]})
        };
        let mut edit = json!({"kind":"breath","clips":[clip.0],"rests":[{"start_tick":14880,"end_tick":15360}],"min_note_ticks":120});
        edit.as_object_mut()
            .unwrap()
            .extend(edits.as_object().unwrap().clone());
        let mut changes =
            vec![json!({"purpose":"Hold the arrival and allow its release","edit":edit})];
        // Sustained synth/wind voices receive an explicit taper. Piano resonance keeps
        // its own decay, while the bowed string gets a longer release budget.
        if name == "synth" || name == "clarinet" {
            changes.push(json!({"purpose":"Ease the sustained ending before release","edit":{"kind":"automation","target":{"kind":"track_gain","track":track.0},"start_tick":13920,"end_tick":15360,"points":[{"at_tick":14880,"value":-9.0},{"at_tick":15120,"value":-18.0},{"at_tick":15359,"value":-18.0}]}}));
        }
        let prepared = session
            .begin_arrangement(
                session.agent_document_revision().unwrap(),
                serde_json::from_value(json!(changes)).unwrap(),
                &[],
            )
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let plan: Value = serde_json::from_str(
            &session
                .continue_arrangement(prepared, &AtomicBool::new(false))
                .unwrap(),
        )
        .unwrap();
        session
            .apply_arrangement(
                plan["plan_id"].as_str().unwrap(),
                plan["revision"].as_str().unwrap(),
            )
            .unwrap();
        let natural_project = session.project().clone();
        let natural_stem = write(&mut session, "held-stem");
        assert_eq!(natural_stem.1.len(), 120);
        assert!(natural_stem.1.iter().all(|v| v.is_finite()));
        assert!(
            natural_stem.1[80] > 1e-6,
            "following entry was lost: {name}"
        );
        if name == "synth" || name == "clarinet" {
            assert!(
                natural_stem.1[74] < hard_stem.1[74] * 0.6,
                "ending did not ease before release: {name}"
            );
        }
        if bowed {
            assert!(
                natural_stem.1[79] < hard_stem.1[79] * 0.01,
                "bow tail still fills the next entry"
            );
        }
        let pad = session
            .add_instrument_track("Unchanged background", "auris.synth.chiptune")
            .unwrap();
        let pad_clip = session
            .add_midi_clip(pad, "Texture", Ticks::ZERO, Ticks(19200))
            .unwrap();
        session
            .add_note(pad_clip, Note::new(48, Ticks::ZERO, Ticks(19200)))
            .unwrap();
        session.set_param(ParamTarget::TrackGain(pad), -18.0);
        let natural_mix = write(&mut session, "held-mix");
        session.project.midi_clip_mut(clip).unwrap().notes =
            hard_project.midi_clip(clip).unwrap().1.notes.clone();
        session.project.automation = hard_project.automation.clone();
        let hard_mix = write(&mut session, "shortened-mix");
        let mut reviews = Vec::new();
        for (scope, a, b) in [
            ("stem", &hard_stem.0, &natural_stem.0),
            ("mix", &hard_mix.0, &natural_mix.0),
        ] {
            eprintln!("Reviewing {name} {scope}");
            let reviewed = crate::audio_review::review_audio(
                &[("A", a.as_path()), ("B", b.as_path())],
                "Compare the melody endings. Which sounds more settled? Describe audible decay or abruptness briefly. State uncertainty.",
                &settings,
            );
            reviews.push(match reviewed { Ok(r) => json!({"scope":scope,"model":r.model,"audio_sent":r.audio_sent,"review":r.review}), Err(e) => json!({"scope":scope,"error":e}) });
        }
        records.push(json!({"instrument":id,"name":name,"envelope_controls":controls,"edits":edits,"plan":plan,"original_score":original,"held_score":natural_project,
            "rms_50ms":{"continuous_stem":continuous.1,"shortened_stem":hard_stem.1,"held_stem":natural_stem.1,"shortened_mix":hard_mix.1,"held_mix":natural_mix.1},"reviews":reviews}));
        std::fs::write(folder.join("review.json"), serde_json::to_vec_pretty(&json!({"comparison":"A = prior shortening-only repair; B = held arrival and instrument-tail budget","limitation":"Separate model observations, not a human listening judgment. Common gain; no normalization or limiting.","records":records})).unwrap()).unwrap();
    }
    assert!(
        records.iter().all(|r| r["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["audio_sent"] == true)),
        "see {}",
        folder.display()
    );
}

#[test]
fn bounded_observation_never_claims_a_verified_rest_from_partial_data() {
    let (mut session, track, clip) = lead();
    for _ in 0..2049 {
        session
            .project
            .midi_clip_mut(clip)
            .unwrap()
            .notes
            .push(Note::new(50, Ticks::ZERO, Ticks(1)));
    }
    let info = read(&session, track, 480);
    assert_eq!(info["complete"], false);
    assert!(info["suggestions"].as_array().unwrap().is_empty());
    assert!(proposal(&mut session, clip, json!({})).is_err());
}

#[test]
fn existing_generated_phrase_breaths_are_observed_without_regeneration() {
    let mut session = session();
    session
        .agent_command(crate::live_agent::Command::EditHarmony {
            start_bar: 1,
            bars: 4,
            key: Some("C major".into()),
            chords: Some("| I | IV | V | I |".into()),
            section: None,
            clear_chords: false,
            clear_key: false,
            clear_section: false,
        })
        .unwrap();
    let track = session
        .add_instrument_track("Generated lead", "auris.physical.clarinet")
        .unwrap();
    let result = session
        .agent_command(crate::live_agent::Command::GeneratePhrase {
            track: track.0,
            part: "melody".into(),
            start_bar: 1,
            bars: 4,
            seed: 42,
        })
        .unwrap();
    let result: Value = serde_json::from_str(&result).unwrap();
    let clip = ClipId(result["clip"].as_u64().unwrap());
    assert!(
        session
            .project()
            .midi_clip(clip)
            .unwrap()
            .1
            .recipe
            .is_some()
    );
    let before = session.project().clone();
    let info = inspect(&before, track.0, MelodicRole::Lead, 0, 15360, 240, &[15360]).unwrap();
    assert_eq!(info["complete"], true);
    assert_eq!(
        info["authored"]["phrase_endings"][0]["has_phrase_rest"], true,
        "{info}"
    );
    assert_eq!(session.project(), &before);
}
