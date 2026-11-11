use super::*;
use crate::live_agent::Command;
use serde_json::Value;

fn fixture() -> (Session, ClipId) {
    let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
    let track = session.add_default_instrument_track("Melody").unwrap();
    let clip = session
        .add_midi_clip(track, "Motif", Ticks::QUARTER * 4, Ticks::QUARTER * 4)
        .unwrap();
    for (i, pitch) in [60, 64, 67, 71].into_iter().enumerate() {
        session
            .add_note(
                clip,
                Note::new(pitch, Ticks::QUARTER * i as i64, Ticks::QUARTER),
            )
            .unwrap();
    }
    session.forget_history();
    (session, clip)
}

fn request(clip: ClipId, start: u32, bars: u32) -> Request {
    serde_json::from_value(
        json!({"melody_clips":[clip.0],"start_bar":start,"bars":bars,"mood":"dreamy"}),
    )
    .unwrap()
}

#[test]
fn live_proposals_are_read_only_compact_and_include_literal_apply_arguments() {
    let (mut session, clip) = fixture();
    let before = session.project().clone();
    let revision = session.agent_document_revision().unwrap();
    let command = Command::SuggestHarmony {
        request: request(clip, 2, 1),
    };
    assert!(!command.is_mutating());
    let text = session.agent_command(command).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(session.project(), &before);
    assert!(!session.can_undo());
    assert_eq!(report["revision"], revision);
    assert_eq!(report["changed"], false);
    assert_eq!(report["protected_clips"], json!([clip.0]));
    assert_eq!(
        report["candidates"][0]["harmony"],
        json!({"start_bar":2,"bars":1,"key":"C major","chords":"| Imaj7 |"})
    );
    assert_eq!(
        report["candidates"][0]["bars"][0]["chords"],
        json!(["Cmaj7"])
    );
    assert!(
        report["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["plain_alternative"] == true)
    );
    assert!(!text.contains("\"notes\":"));
    assert!(text.len() < 10_000);
}

#[test]
fn windows_and_loops_are_expanded_without_editing_the_authored_clip() {
    let (mut session, clip) = fixture();
    session.set_clip_loop(clip, Ticks::QUARTER * 12).unwrap();
    let before = session.project().clone();
    let report: Value =
        serde_json::from_str(&session.agent_suggest_harmony(request(clip, 3, 2)).unwrap()).unwrap();
    assert_eq!(report["note_count"], 8);
    assert_eq!(
        report["candidates"][0]["harmony"]["chords"],
        "| Imaj7 | Imaj7 |"
    );
    assert_eq!(report["candidates"][0]["bars"][0]["bar"], 3);
    assert_eq!(session.project(), &before);
    assert!(
        session
            .agent_suggest_harmony(request(clip, 5, 1))
            .unwrap_err()
            .contains("no melody")
    );
}

#[test]
fn invalid_sources_intent_and_crossing_changes_are_rejected_without_mutation() {
    let (mut session, clip) = fixture();
    let mut duplicate = request(clip, 2, 1);
    duplicate.melody_clips.push(clip.0);
    assert!(session.agent_suggest_harmony(duplicate).is_err());
    for (key, mood) in [
        (Some("not a key"), "calm"),
        (None, "not a mood"),
        (Some("C pentatonic"), "calm"),
    ] {
        let mut bad = request(clip, 2, 1);
        bad.key = key.map(str::to_owned);
        bad.mood = mood.into();
        assert!(session.agent_suggest_harmony(bad).is_err());
    }
    assert!(
        session
            .agent_suggest_harmony(request(ClipId(u64::MAX), 2, 1))
            .is_err()
    );
    assert!(session.agent_suggest_harmony(request(clip, 0, 1)).is_err());
    assert!(session.agent_suggest_harmony(request(clip, 2, 17)).is_err());
    assert!(
        session
            .agent_suggest_harmony(request(clip, u32::MAX, 1))
            .is_err()
    );
    let drum = session
        .add_drum_track("Drums", crate::DEFAULT_INSTRUMENT)
        .unwrap();
    let drum_clip = session
        .add_midi_clip(drum, "Hit", Ticks::QUARTER * 4, Ticks::QUARTER * 4)
        .unwrap();
    session
        .add_note(drum_clip, Note::new(36, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    assert!(
        session
            .agent_suggest_harmony(request(drum_clip, 2, 1))
            .unwrap_err()
            .contains("not drums")
    );
    session.set_key(Ticks::QUARTER * 8, MusicalKey::parse("D minor").unwrap());
    let before = session.project().clone();
    assert!(
        session
            .agent_suggest_harmony(request(clip, 2, 2))
            .unwrap_err()
            .contains("key changes")
    );
    assert_eq!(session.project(), &before);
    session.remove_key(Ticks::QUARTER * 8);
    session.set_signature_point(Ticks::QUARTER * 8, TimeSignature::new(3, 4));
    assert!(
        session
            .agent_suggest_harmony(request(clip, 2, 2))
            .unwrap_err()
            .contains("meter")
    );
}
