use auris_core::project::{
    ClipId, ClipStage, CurvePoint, MidiClip, Note, NoteTransform, StageError, StageId,
};
use auris_core::{TICKS_PER_QUARTER, Ticks};

fn clip() -> MidiClip {
    let mut clip = MidiClip::new(
        ClipId(7),
        "stable",
        Ticks::ZERO,
        Ticks(TICKS_PER_QUARTER * 4),
    );
    clip.notes = vec![
        Note::new(60, Ticks(TICKS_PER_QUARTER), Ticks(TICKS_PER_QUARTER / 2)),
        Note::new(
            64,
            Ticks(TICKS_PER_QUARTER * 2),
            Ticks(TICKS_PER_QUARTER / 2),
        ),
    ];
    clip.notes[0].lyric = "la".to_owned();
    clip.bend = vec![CurvePoint {
        at: Ticks(TICKS_PER_QUARTER),
        value: 0.25,
    }];
    clip.controllers.insert(
        1,
        vec![CurvePoint {
            at: Ticks(TICKS_PER_QUARTER * 2),
            value: 0.75,
        }],
    );
    clip.muted = true;
    clip.length_is_explicit = true;
    clip.transforms = vec![
        NoteTransform::Mute { amount: 0.2 },
        NoteTransform::Humanize {
            amount: 0.3,
            seed: 1234,
        },
    ];
    clip.loop_end = Ticks(TICKS_PER_QUARTER * 8);
    clip
}

#[test]
fn stage_conversion_preserves_order_payload_and_performer_output() {
    let legacy = clip();
    let expected = legacy.sounding_notes(120.0).collect::<Vec<_>>();
    assert!(!expected.is_empty());
    let stable = legacy
        .clone()
        .with_stage_ids(vec![StageId(41), StageId(42)])
        .expect("valid stage IDs");
    assert_eq!(
        stable
            .transforms
            .iter()
            .map(|stage| stage.id)
            .collect::<Vec<_>>(),
        vec![StageId(41), StageId(42)]
    );
    assert_eq!(
        stable
            .performer_clip()
            .sounding_notes(120.0)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(stable.performer_clip(), legacy);
    let json = serde_json::to_string(&stable).expect("stable clip serializes");
    let decoded: auris_core::project::StableMidiClip =
        serde_json::from_str(&json).expect("stable clip deserializes");
    assert_eq!(decoded, stable);
    let legacy_json = serde_json::to_string(&legacy).expect("legacy clip serializes");
    let legacy_decoded: MidiClip =
        serde_json::from_str(&legacy_json).expect("legacy clip deserializes");
    assert_eq!(legacy_decoded, legacy);
}

#[test]
fn stage_reordering_keeps_identity_and_payload_attached() {
    let mut stable = clip()
        .with_stage_ids(vec![StageId(1), StageId(2)])
        .expect("valid stage IDs");
    assert!(
        stable
            .move_stage(StageId(2), -1)
            .expect("known stage moved")
    );
    assert_eq!(stable.transforms[0].id, StageId(2));
    assert_eq!(stable.transforms[1].id, StageId(1));
    assert_eq!(
        stable.transforms[0].transform,
        NoteTransform::Humanize {
            amount: 0.3,
            seed: 1234
        }
    );
    assert!(
        !stable
            .move_stage(StageId(2), -1)
            .expect("known stage is a no-op at start")
    );
}

#[test]
fn stage_conversion_and_mutation_reject_bad_identity_requests() {
    let legacy = clip();
    assert_eq!(
        legacy.clone().with_stage_ids(vec![StageId(1)]),
        Err(StageError::LengthMismatch {
            clip: ClipId(7),
            stages: 2,
            ids: 1
        })
    );
    assert_eq!(
        legacy.clone().with_stage_ids(vec![StageId(1), StageId(1)]),
        Err(StageError::DuplicateId(StageId(1)))
    );
    assert_eq!(
        legacy.with_stage_ids(vec![StageId(0), StageId(2)]),
        Err(StageError::ZeroId)
    );
    let mut stable = clip()
        .with_stage_ids(vec![StageId(1), StageId(2)])
        .expect("valid");
    assert_eq!(
        stable.remove_stage(StageId(9)),
        Err(StageError::UnknownId {
            clip: ClipId(7),
            stage: StageId(9)
        })
    );
    assert_eq!(stable.stage_index(StageId(0)), Err(StageError::ZeroId));
}

#[test]
fn invalid_existing_identity_set_is_never_partially_edited() {
    let mut stable = clip()
        .with_stage_ids(vec![StageId(1), StageId(2)])
        .expect("valid");
    stable.transforms[1].id = StageId(1);
    let before = stable.clone();
    assert_eq!(
        stable.move_stage(StageId(1), 1),
        Err(StageError::DuplicateId(StageId(1)))
    );
    assert_eq!(stable, before);
    assert_eq!(
        stable.replace_stage_transform(StageId(1), NoteTransform::Mute { amount: 0.9 }),
        Err(StageError::DuplicateId(StageId(1)))
    );
    assert_eq!(stable, before);
}

#[test]
fn append_and_replace_preserve_stable_identity() {
    let mut stable = clip()
        .with_stage_ids(vec![StageId(1), StageId(2)])
        .expect("valid");
    stable
        .append_stage(StageId(3), NoteTransform::Transpose { semitones: 7 })
        .expect("new ID appends");
    assert_eq!(stable.transforms[2].id, StageId(3));
    stable
        .replace_stage_transform(StageId(3), NoteTransform::Mute { amount: 0.4 })
        .expect("known ID replaces");
    assert_eq!(stable.transforms[2].id, StageId(3));
    assert_eq!(
        stable.transforms[2].transform,
        NoteTransform::Mute { amount: 0.4 }
    );
    let before = stable.clone();
    assert_eq!(
        stable.append_stage(StageId(3), NoteTransform::Transpose { semitones: 1 }),
        Err(StageError::DuplicateId(StageId(3)))
    );
    assert_eq!(stable, before);
}

#[test]
fn mapping_a_clip_keeps_all_document_fields() {
    let legacy = clip();
    let mapped = legacy.clone().map_transforms(|transform| ClipStage {
        id: StageId(100),
        transform,
    });
    assert_eq!(mapped.id, legacy.id);
    assert_eq!(mapped.name, legacy.name);
    assert_eq!(mapped.start, legacy.start);
    assert_eq!(mapped.length, legacy.length);
    assert_eq!(mapped.notes, legacy.notes);
    assert_eq!(mapped.recipe, legacy.recipe);
    assert_eq!(mapped.bend, legacy.bend);
    assert_eq!(mapped.controllers, legacy.controllers);
    assert_eq!(mapped.loop_end, legacy.loop_end);
    assert_eq!(mapped.transforms.len(), legacy.transforms.len());
}

#[test]
fn legacy_transform_wire_shape_and_missing_stack_default_are_unchanged() {
    let mut legacy = clip();
    legacy.transforms = vec![NoteTransform::Humanize {
        amount: 0.3,
        seed: 1234,
    }];
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert_eq!(
        encoded["transforms"],
        serde_json::to_value(&legacy.transforms).unwrap()
    );
    assert!(encoded["transforms"][0].get("id").is_none());
    let mut without_stack = encoded;
    without_stack.as_object_mut().unwrap().remove("transforms");
    let decoded: MidiClip = serde_json::from_value(without_stack.clone()).unwrap();
    assert!(decoded.transforms.is_empty());
    let stable: auris_core::project::StableMidiClip =
        serde_json::from_value(without_stack).unwrap();
    assert!(stable.transforms.is_empty());
    assert_eq!(stable.performer_clip(), decoded);
}

#[test]
fn removal_and_extreme_moves_preserve_remaining_stage_identities() {
    let mut stable = clip().with_stage_ids(vec![StageId(1), StageId(2)]).unwrap();
    assert!(stable.move_stage(StageId(1), isize::MAX).unwrap());
    assert_eq!(stable.stage_index(StageId(1)).unwrap(), 1);
    assert!(stable.move_stage(StageId(1), isize::MIN).unwrap());
    let removed = stable.remove_stage(StageId(1)).unwrap();
    assert_eq!(removed.id, StageId(1));
    assert_eq!(stable.stage_index(StageId(2)).unwrap(), 0);
    let before = stable.clone();
    assert_eq!(
        stable.append_stage(StageId(0), removed.transform),
        Err(StageError::ZeroId)
    );
    assert_eq!(stable, before);
}
