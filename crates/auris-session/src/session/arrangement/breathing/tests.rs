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
    let route = envelope_route(session.project(), font.0);
    assert!(route.as_str().unwrap().contains("load_sample"));
    assert!(envelope_route(session.project(), audio.0).is_null());
}

#[test]
fn ending_controls_use_registered_defaults_and_distinguish_instrument_decay() {
    for (id, family, release) in [
        ("auris.synth.chiptune", "synth", 0.15),
        ("auris.physical.piano", "keys", 0.2),
        ("auris.physical.clarinet", "winds", 0.12),
        ("auris.physical.violin", "bowed_strings", 0.35),
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
