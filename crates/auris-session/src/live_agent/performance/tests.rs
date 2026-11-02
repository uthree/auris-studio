use super::*;
use serde_json::{Value, json};

#[test]
fn sparse_long_ghost_clips_do_not_trigger_unbounded_grid_scans_during_inspection() {
    let mut clip = MidiClip::new(ClipId(1), "Sparse", Ticks::ZERO, Ticks::QUARTER * 256);
    clip.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    let result = effects::measure(
        &clip,
        &[NoteTransform::Ghost {
            settings: GhostNotes {
                density: 1.0,
                velocity: 0.3,
                preserve_rests: false,
                ..Default::default()
            },
        }],
        None,
        &Project::new("Probe", 48000.0),
    );
    assert_eq!(result["complete"], false);
    assert_eq!(result["stages"][0]["measured"], false);
}

#[test]
fn arpeggio_reports_ineffective_strum_and_effective_expression_without_changing_the_score() {
    let (mut session, clip) = fixture();
    let before = session.project().midi_clip(clip).unwrap().1.notes.clone();
    let result = edit(&mut session, clip, json!({"strum":{"spread_ms":24,"direction":"low_to_high","clock":"attacks"},"expression":{"timing":0.1,"velocity":0.08,"seed":73}}), None).unwrap();
    let stages = &result["note_effects"]["stages"];
    assert_eq!(result["note_effects"]["complete"], true);
    assert_eq!(stages[0]["eligible_chord_groups"], 0);
    assert_eq!(stages[0]["note_events_unchanged"], true);
    assert!(!stages[0]["warnings"].as_array().unwrap().is_empty());
    let expression = stages
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "expression")
        .unwrap();
    assert_eq!(expression["changed_velocities"], 2, "{result}");
    assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, before);
}

#[test]
fn chord_strum_reports_actual_onset_changes_and_readback_agrees() {
    let (mut session, clip) = fixture();
    session
        .replace_notes(
            clip,
            [60, 64, 67]
                .map(|pitch| Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .to_vec(),
        )
        .unwrap();
    let result = edit(
        &mut session,
        clip,
        json!({"strum":{"spread_ms":24,"direction":"low_to_high"}}),
        None,
    )
    .unwrap();
    assert_eq!(
        result["note_effects"]["stages"][0]["eligible_chord_groups"],
        1
    );
    assert_eq!(result["note_effects"]["stages"][0]["changed_onsets"], 2);
    let read: Value =
        serde_json::from_str(&session.agent_inspect_performance(clip, None).unwrap()).unwrap();
    assert_eq!(read["note_effects"], result["note_effects"]);
}

#[test]
fn performance_measurements_are_bounded_and_pitch_curves_are_not_called_inaudible() {
    let (mut session, clip) = fixture();
    session
        .replace_notes(
            clip,
            (0..2050)
                .map(|i| Note::new(60, Ticks(i), Ticks(2)))
                .collect(),
        )
        .unwrap();
    let result = edit(&mut session, clip, json!({"pitch":{"vibrato":0.2}}), None).unwrap();
    assert_eq!(result["note_effects"]["sampled_authored_notes"], 2048);
    assert_eq!(result["note_effects"]["truncated"], true);
    assert!(
        result["note_effects"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["warnings"].as_array().unwrap().is_empty())
    );
}

#[test]
fn slide_and_glide_report_effective_performed_changes_separately() {
    let (mut session, clip) = fixture();
    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(67, Ticks::QUARTER, Ticks::QUARTER),
            ],
        )
        .unwrap();
    let result = edit(&mut session, clip, json!({"slide":1.0}), None).unwrap();
    let stages = result["note_effects"]["stages"].as_array().unwrap();
    let slide = stages
        .iter()
        .find(|stage| stage["stage"] == "slide")
        .unwrap();
    assert_eq!(slide["slide"]["added_notes"], 1);
    assert_eq!(slide["slide"]["effective"], true);
    let result = edit(
        &mut session,
        clip,
        json!({"reset":["slide"],"pitch":{"glide_ms":100.0}}),
        None,
    )
    .unwrap();
    let pitch = result["note_effects"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "pitch")
        .unwrap();
    assert_eq!(pitch["pitch"]["effective"], true);
    assert!(pitch["pitch"]["max_semitone_delta"].as_f64().unwrap() > 0.0);
}

#[test]
fn repeated_slide_and_gapped_glide_report_why_they_did_nothing() {
    let (mut session, clip) = fixture();
    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(60, Ticks::QUARTER, Ticks::QUARTER),
            ],
        )
        .unwrap();
    let repeated = edit(&mut session, clip, json!({"slide":1.0}), None).unwrap();
    let slide = repeated["note_effects"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "slide")
        .unwrap();
    assert_eq!(slide["slide"]["added_notes"], 0);
    assert_eq!(slide["slide"]["ineligible_reasons"]["same_or_semitone"], 1);

    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(67, Ticks::QUARTER * 2, Ticks::QUARTER),
            ],
        )
        .unwrap();
    let gapped = edit(
        &mut session,
        clip,
        json!({"reset":["slide"],"pitch":{"glide_ms":100.0}}),
        None,
    )
    .unwrap();
    let pitch = gapped["note_effects"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "pitch")
        .unwrap();
    assert_eq!(pitch["pitch"]["effective"], false);
    assert_eq!(pitch["pitch"]["eligibility"]["eligible_transitions"], 0);
    assert_eq!(pitch["pitch"]["eligibility"]["maximum_gap_ms"], 500.0);
}

#[test]
fn authored_bend_does_not_mask_an_ineffective_glide() {
    let (mut session, clip) = fixture();
    session.set_curve_point(clip, auris_core::project::ClipCurve::Bend, Ticks(200), 0.7);
    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(67, Ticks::QUARTER * 2, Ticks::QUARTER),
            ],
        )
        .unwrap();
    let result = edit(
        &mut session,
        clip,
        json!({"pitch":{"glide_ms":100.0}}),
        None,
    )
    .unwrap();
    let pitch = result["note_effects"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "pitch")
        .unwrap();
    assert_eq!(pitch["pitch"]["effective"], false);
    assert_eq!(pitch["pitch"]["max_semitone_delta"], 0.0);
}

#[test]
fn same_pitch_glide_is_reported_as_ineffective_with_a_reason() {
    let (mut session, clip) = fixture();
    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(60, Ticks::QUARTER, Ticks::QUARTER),
            ],
        )
        .unwrap();
    let result = edit(
        &mut session,
        clip,
        json!({"pitch":{"glide_ms":100.0}}),
        None,
    )
    .unwrap();
    let pitch = result["pitch_effects"][0]["report"].clone();
    assert_eq!(pitch["effective"], false);
    assert_eq!(pitch["eligibility"]["ineligible_reasons"]["same_pitch"], 1);
}

#[test]
fn pitch_measurement_counts_hidden_notes_before_applying_the_note_bound() {
    let mut clip = MidiClip::new(ClipId(9), "Hidden", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes = (0..2050)
        .map(|index| Note::new(60, Ticks::QUARTER * 4 + Ticks(index), Ticks::QUARTER))
        .collect();
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                glide_ms: 100.0,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["measured"], false);
}

#[test]
fn pitch_measurement_stops_before_a_clip_longer_than_sixty_four_beats() {
    let clip = MidiClip::new(ClipId(10), "Long", Ticks::ZERO, Ticks::QUARTER * 65);
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                glide_ms: 100.0,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["measured"], false);
}

#[test]
fn dense_authored_curve_is_reported_unmeasured_instead_of_ineffective() {
    let mut clip = MidiClip::new(ClipId(11), "Dense", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes = vec![
        Note::new(60, Ticks::ZERO, Ticks::QUARTER),
        Note::new(67, Ticks::QUARTER, Ticks::QUARTER),
    ];
    clip.bend = (0..2050)
        .map(|index| auris_core::project::CurvePoint {
            at: Ticks(index),
            value: 0.0,
        })
        .collect();
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                glide_ms: 100.0,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["measured"], false);
    assert!(report["effective"].is_null());
}

#[test]
fn whole_drum_scope_does_not_claim_to_measure_nested_writer_stages() {
    let mut clip = MidiClip::new(ClipId(12), "Kit", Ticks::ZERO, Ticks::QUARTER * 4);
    let mut note = Note::new(36, Ticks::ZERO, Ticks::QUARTER);
    note.drum_voice = "kick".into();
    clip.notes.push(note);
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::ForDrumVoice {
            voice: "kick".into(),
            transforms: vec![NoteTransform::Gate { amount: 0.5 }],
        }],
        None,
        &project,
    );
    assert_eq!(result["complete"], false);
    assert_eq!(result["stages"][0]["stage"], "for_drum_voice");
    assert!(
        result["stages"][0]["warnings"].as_array().unwrap()[0]
            .as_str()
            .unwrap()
            .contains("Select drum_voice")
    );
}

#[test]
fn modulation_reports_an_effect_even_when_glide_is_zero() {
    let mut clip = MidiClip::new(ClipId(13), "Modulation", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes
        .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2));
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                modulation: 0.5,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["effective"], false);
    assert_eq!(report["generated_curves"]["modulation"]["effective"], true);
}

#[test]
fn short_notes_do_not_report_a_volume_swell_from_the_default_full_volume() {
    let mut clip = MidiClip::new(ClipId(16), "Short volume", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes.push(Note::new(60, Ticks::ZERO, Ticks(480)));
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                volume_swell: 0.8,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    assert_eq!(
        result["pitch_effects"][0]["report"]["generated_curves"]["volume_swell"]["effective"],
        false
    );
}

#[test]
fn pitch_time_and_generated_sample_bounds_remain_unknown() {
    for (beats, bpm) in [(64, 60.0), (32, 120.0)] {
        let length = Ticks::QUARTER * beats;
        let mut clip = MidiClip::new(ClipId(17), "Bounded", Ticks::ZERO, length);
        let half = Ticks(length.raw() / 2);
        clip.notes.push(Note::new(60, Ticks::ZERO, half));
        clip.notes.push(Note::new(67, half, half));
        let mut project = Project::new("Probe", 48_000.0);
        project.tempo_map.set_point(Ticks::ZERO, bpm);
        let result = effects::measure(
            &clip,
            &[NoteTransform::Pitch {
                settings: PitchPerformance {
                    glide_ms: 120.0,
                    ..Default::default()
                },
            }],
            None,
            &project,
        );
        let report = &result["pitch_effects"][0]["report"];
        assert_eq!(report["measured"], false, "{result}");
        assert!(report["effective"].is_null(), "{result}");
        assert_eq!(result["complete"], false);
    }
}

#[test]
fn same_pitch_glide_stays_ineffective_with_authored_bend_and_vibrato() {
    let mut clip = MidiClip::new(ClipId(14), "Same pitch", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes = vec![
        Note::new(60, Ticks::ZERO, Ticks::QUARTER),
        Note::new(60, Ticks::QUARTER, Ticks::QUARTER),
    ];
    clip.bend.push(auris_core::project::CurvePoint {
        at: Ticks(120),
        value: 0.7,
    });
    let project = Project::new("Probe", 48_000.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                glide_ms: 100.0,
                vibrato: 0.4,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["effective"], false);
    assert_eq!(report["eligibility"]["ineligible_reasons"]["same_pitch"], 1);
}

#[test]
fn glide_eligibility_uses_tempo_map_seconds_at_each_note_boundary() {
    let mut clip = MidiClip::new(ClipId(15), "Tempo", Ticks::ZERO, Ticks::QUARTER * 4);
    clip.notes = vec![
        Note::new(60, Ticks::ZERO, Ticks(480)),
        Note::new(67, Ticks(600), Ticks(960)),
    ];
    let mut project = Project::new("Probe", 48_000.0);
    project.tempo_map.set_point(Ticks::ZERO, 120.0);
    project.tempo_map.set_point(Ticks(480), 600.0);
    let result = effects::measure(
        &clip,
        &[NoteTransform::Pitch {
            settings: PitchPerformance {
                glide_ms: 100.0,
                ..Default::default()
            },
        }],
        None,
        &project,
    );
    let report = &result["pitch_effects"][0]["report"];
    assert_eq!(report["eligibility"]["eligible_transitions"], 1);
    assert_eq!(report["effective"], true);
}

fn fixture() -> (Session, ClipId) {
    let mut session = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
    let track = session.add_default_instrument_track("Lead").unwrap();
    session
        .agent_edit_harmony(
            1,
            4,
            Some("C major"),
            Some("| I | IV | V | I |"),
            false,
            None,
        )
        .unwrap();
    let clip = session
        .generate_clip(
            track,
            Ticks::ZERO,
            Ticks::QUARTER * 16,
            ClipRecipe::new(ClipPreset::Lead, 42),
        )
        .unwrap();
    session.set_clip_transforms(clip, Vec::new()).unwrap();
    session
        .replace_notes(
            clip,
            vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                Note::new(64, Ticks(240), Ticks::QUARTER),
            ],
        )
        .unwrap();
    (session, clip)
}

fn edit(
    session: &mut Session,
    clip: ClipId,
    settings: Value,
    voice: Option<&str>,
) -> Result<Value, String> {
    let command = serde_json::from_value(
        json!({"action":"set_performance","clip":clip.0,"drum_voice":voice,"settings":settings}),
    )
    .map_err(|e| e.to_string())?;
    let text = session.agent_command(command)?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[test]
fn edits_change_the_performance_preserve_the_complete_score_and_undo_separately() {
    let (mut session, clip) = fixture();
    session.set_curve_point(clip, auris_core::project::ClipCurve::Bend, Ticks(200), 0.7);
    let contour = VolumeContour::new(vec![auris_core::project::CurvePoint {
        at: Ticks(2300),
        value: 0.22,
    }]);
    session
        .set_clip_transforms(
            clip,
            vec![
                NoteTransform::Pitch {
                    settings: PitchPerformance {
                        vibrato: 0.4,
                        volume_contour: contour.clone(),
                        ..Default::default()
                    },
                },
                NoteTransform::Expression {
                    settings: Expression {
                        group: 99,
                        seed: 777,
                        ..Default::default()
                    },
                },
            ],
        )
        .unwrap();
    let before = session.project().clone();
    let patch = json!({"gate":0.5,"transpose":12,"lean_ticks":24,"expression":{"velocity":0.4},"pitch":{"scoop":1.5}});
    let result = edit(&mut session, clip, patch.clone(), None).unwrap();
    assert_eq!(result["settings"]["expression"]["group"], 99);
    assert_eq!(result["settings"]["expression"]["seed"], 777);
    let actual = session.project().midi_clip(clip).unwrap().1;
    let mut expected = before.midi_clip(clip).unwrap().1.clone();
    expected.transforms = actual.transforms.clone();
    assert_eq!(
        actual, &expected,
        "notes, recipe, source curves, placement and loop state are preserved"
    );
    let held_pitch = actual
        .transforms
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Pitch { settings } => Some(settings),
            _ => None,
        })
        .unwrap();
    assert_eq!(held_pitch.volume_contour, contour);
    assert_eq!(held_pitch.vibrato, 0.4);
    let performed = auris_core::performed_notes(
        actual.playable_notes().collect(),
        &actual.transforms,
        auris_core::PerformanceContext {
            bpm: 120.0,
            pass: 0,
            start: actual.start,
            length: actual.length,
            signatures: &session.project().signatures,
        },
    );
    assert_eq!(performed[0].pitch, 72);
    assert_eq!(performed[0].length, Ticks(480));
    assert_eq!(performed[0].start, Ticks(24));
    let after_first = session.project().clone();
    edit(&mut session, clip, patch, None).unwrap();
    assert_eq!(
        session.project(),
        &after_first,
        "an identical retry is a no-op"
    );
    edit(&mut session, clip, json!({"gate":0.25}), None).unwrap();
    session.undo();
    assert_eq!(
        session.project(),
        &after_first,
        "successive tool calls are separate Undo steps"
    );
    session.undo();
    assert_eq!(session.project(), &before);
    assert!(session.path().is_none());
}

#[test]
fn partial_edits_upgrade_legacy_stages_without_losing_their_controls_or_order() {
    let (mut session, clip) = fixture();
    let before = vec![
        NoteTransform::Humanize {
            amount: 0.2,
            seed: 9,
        },
        NoteTransform::Transpose { semitones: 7 },
        NoteTransform::Stroke {
            spread_ms: 23.0,
            direction: StrokeDirection::HighToLow,
        },
        NoteTransform::Brush { amount: 0.3 },
        NoteTransform::Gate { amount: 0.8 },
    ];
    session.set_clip_transforms(clip, before.clone()).unwrap();
    let result = edit(
        &mut session,
        clip,
        json!({"expression":{"timing":0.4},"strum":{"up_velocity":0.6},"ghost":{"length_ms":30.0}}),
        None,
    )
    .unwrap();
    assert_eq!(result["settings"]["expression"]["velocity"], json!(0.2f32));
    assert_eq!(result["settings"]["expression"]["seed"], 9);
    assert_eq!(result["settings"]["strum"]["spread_ms"], 23.0);
    assert_eq!(result["settings"]["strum"]["direction"], "high_to_low");
    assert_eq!(result["settings"]["strum"]["clock"], "attacks");
    assert_eq!(result["settings"]["ghost"]["density"], 1.0);
    assert_eq!(result["settings"]["ghost"]["pattern"], "sixteenths");
    assert_eq!(result["settings"]["ghost"]["preserve_rests"], false);
    assert!((result["settings"]["ghost"]["velocity"].as_f64().unwrap() - 0.09).abs() < 1e-6);
    let actual = session.clip_transforms(clip).unwrap();
    assert_eq!(actual[1], before[1]);
    assert_eq!(actual[4], before[4]);
    assert_eq!(
        actual.iter().filter_map(family).collect::<Vec<_>>(),
        before.iter().filter_map(family).collect::<Vec<_>>()
    );
}

#[test]
fn invalid_batches_and_unknown_fields_leave_document_and_history_untouched() {
    let (mut session, clip) = fixture();
    let before = session.project().clone();
    for settings in [
        json!({}),
        json!({"gate":0.5,"strum":{}}),
        json!({"reset":vec!["gate";14]}),
        json!({"gate":0.5,"reset":["expression"],"ghost":{"length_ms":500}}),
        json!({"swing_percent":49}),
        json!({"pitch":{"vibrato_hz":12}}),
        json!({"expression":{"typo":0.5}}),
        json!({"strum":{"direction":"sideways"}}),
        json!({"ghost":{"target_pitch":128}}),
        json!({"ghost":{"target_pitch":42,"clear_target_pitch":true}}),
        json!({"groove":{"reference_clip":u64::MAX}}),
        json!({"groove":{"timing":0.5}}),
    ] {
        assert!(
            edit(&mut session, clip, settings.clone(), None).is_err(),
            "{settings}"
        );
        assert_eq!(session.project(), &before);
    }
    let patch = PerformancePatch {
        gate: Some(f32::NAN),
        ..Default::default()
    };
    assert!(session.agent_set_performance(clip, None, patch).is_err());
    assert_eq!(session.project(), &before);
    session.undo();
    assert_ne!(
        session.project(),
        &before,
        "failed calls added no empty Undo step"
    );
}

#[test]
fn scoped_drum_edits_preserve_other_writers_global_stages_and_stored_notes() {
    let mut session = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
    let track = session.add_default_drum_track("Beat").unwrap();
    let clip = session
        .add_midi_clip(track, "Kit", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    let mut kick = Note::new(36, Ticks::ZERO, Ticks::QUARTER);
    kick.drum_voice = "kick".into();
    let mut hat = Note::new(42, Ticks::ZERO, Ticks::QUARTER);
    hat.drum_voice = "hat".into();
    session.replace_notes(clip, vec![kick, hat]).unwrap();
    let kick_stage = NoteTransform::ForDrumVoice {
        voice: "kick".into(),
        transforms: vec![NoteTransform::Gate { amount: 0.9 }],
    };
    let global = NoteTransform::Lean { ticks: 7 };
    session
        .set_clip_transforms(
            clip,
            vec![
                kick_stage.clone(),
                global.clone(),
                NoteTransform::ForDrumVoice {
                    voice: "hat".into(),
                    transforms: vec![NoteTransform::Humanize {
                        amount: 0.1,
                        seed: 33,
                    }],
                },
            ],
        )
        .unwrap();
    let before = session.project().clone();
    let result = edit(
        &mut session,
        clip,
        json!({"gate":0.5,"expression":{"timing":0.2}}),
        Some("hat"),
    )
    .unwrap();
    assert_eq!(result["settings"]["expression"]["seed"], 33);
    assert_eq!(result["settings"]["gate"], 0.5);
    assert_eq!(session.clip_transforms(clip).unwrap()[0], kick_stage);
    assert_eq!(session.clip_transforms(clip).unwrap()[1], global);
    assert_eq!(
        session.project().midi_clip(clip).unwrap().1.notes,
        before.midi_clip(clip).unwrap().1.notes
    );
    let after = session.project().clone();
    assert!(edit(&mut session, clip, json!({"gate":0.8}), Some("unknown")).is_err());
    assert!(
        edit(
            &mut session,
            clip,
            json!({"pitch":{"scoop":1.0}}),
            Some("hat")
        )
        .is_err()
    );
    assert_eq!(session.project(), &after);
    let result = edit(
        &mut session,
        clip,
        json!({"reset":["expression","gate"]}),
        Some("hat"),
    )
    .unwrap();
    assert_eq!(result["settings"]["gate"], 1.0);
    assert!(result["settings"]["expression"].is_null());
    assert_eq!(session.clip_transforms(clip).unwrap()[0], kick_stage);
    session.undo();
    assert_eq!(session.project(), &after);
}

#[test]
fn groove_capture_uses_the_performed_reference_and_partial_edits_keep_the_snapshot() {
    let (mut session, target) = fixture();
    let track = session.project().midi_clip(target).unwrap().0;
    let source = session
        .add_midi_clip(track, "Reference", Ticks::QUARTER * 16, Ticks::QUARTER * 4)
        .unwrap();
    session
        .add_note(source, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    session
        .set_clip_transforms(source, vec![NoteTransform::Lean { ticks: 60 }])
        .unwrap();
    edit(
        &mut session,
        target,
        json!({"groove":{"reference_clip":source.0,"timing":0.8}}),
        None,
    )
    .unwrap();
    let held = session
        .clip_transforms(target)
        .unwrap()
        .iter()
        .find_map(|s| match s {
            NoteTransform::Groove { template, .. } => Some(template.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(held.points[0].offset, Ticks(60));
    session
        .replace_notes(source, vec![Note::new(72, Ticks(200), Ticks::QUARTER)])
        .unwrap();
    let result = edit(
        &mut session,
        target,
        json!({"groove":{"velocity":0.2}}),
        None,
    )
    .unwrap();
    assert_eq!(result["settings"]["groove"]["timing"], json!(0.8f32));
    let actual = session
        .clip_transforms(target)
        .unwrap()
        .iter()
        .find_map(|s| match s {
            NoteTransform::Groove { template, .. } => Some(template),
            _ => None,
        })
        .unwrap();
    assert_eq!(actual, &held);
}

#[test]
fn performance_inspection_is_read_only_and_exposes_panel_units() {
    let (mut session, clip) = fixture();
    session.set_clip_transforms(clip, Vec::new()).unwrap();
    let before = session.project().clone();
    let result: Value = serde_json::from_str(
        &session
            .agent_command(Command::InspectPerformance {
                clip: clip.0,
                drum_voice: None,
            })
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["settings"]["swing_percent"], 50);
    assert_eq!(result["settings"]["gate"], 1.0);
    assert_eq!(result["units"]["times"], "milliseconds");
    assert_eq!(session.project(), &before);
}

#[test]
fn sustain_holds_releases_reads_back_and_resets_to_nothing() {
    let (mut session, clip) = fixture();
    let result = edit(&mut session, clip, json!({"sustain_ticks":960}), None).unwrap();
    assert_eq!(result["settings"]["sustain_ticks"], 960);
    let target = session.project().midi_clip(clip).unwrap().1.clone();
    assert_eq!(
        target.transforms,
        vec![NoteTransform::Sustain { ticks: 960 }]
    );
    let heard: Vec<_> = target.sounding_notes(120.0).collect();
    assert_eq!(heard[0].length, Ticks(1920));
    assert_eq!(heard[1].length, Ticks(1920));
    assert_eq!(
        target.notes[0].length,
        Ticks::QUARTER,
        "the score is untouched"
    );
    assert!(edit(&mut session, clip, json!({"sustain_ticks":4000}), None).is_err());
    edit(&mut session, clip, json!({"reset":["sustain"]}), None).unwrap();
    assert!(
        session
            .project()
            .midi_clip(clip)
            .unwrap()
            .1
            .transforms
            .is_empty()
    );
    edit(&mut session, clip, json!({"sustain_ticks":480}), None).unwrap();
    edit(&mut session, clip, json!({"sustain_ticks":0}), None).unwrap();
    assert!(
        session
            .project()
            .midi_clip(clip)
            .unwrap()
            .1
            .transforms
            .is_empty()
    );
}
