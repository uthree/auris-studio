use super::*;
use crate::{SessionOptions, live_agent::Command, prelude::*};

fn session() -> Session {
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    session.japanese = None;
    session
}

#[test]
fn analysis_fits_phrases_to_note_counts() {
    let session = session();
    let text = "さあ いっしょに ゆこー\nさあ ゆこう";
    let fits = [
        PhraseFit {
            phrase: 1,
            notes: 6,
        },
        PhraseFit {
            phrase: 2,
            notes: 6,
        },
    ];
    let analysis = session.analyze_lyrics_fitted(text, &[], &fits).unwrap();
    let first = &analysis.phrases[0];
    assert_eq!((first.min_notes, first.max_notes), (6, 8));
    assert_eq!(first.moras[1].slot, MoraSlot::Hold);
    assert_eq!(first.moras[3].slot, MoraSlot::Rest);
    let fit = first.fit.as_ref().unwrap();
    assert_eq!((fit.verdict, fit.excess), (FitVerdict::Fits, 0));
    assert_eq!(
        first.note_reading.as_deref(),
        Some("さあ| いっ|しょ|に| ゆ|こー")
    );
    // ゆこう spelled in kana is う, not a held o: 5 moras, one hold, too few for 6 notes.
    let second = &analysis.phrases[1];
    assert_eq!((second.min_notes, second.max_notes), (4, 5));
    let fit = second.fit.as_ref().unwrap();
    assert_eq!((fit.verdict, fit.excess), (FitVerdict::TooFewMoras, -1));
    assert_eq!(second.note_reading.as_deref(), Some("さ|あ| ゆ|こ|う"));

    assert!(
        session
            .analyze_lyrics_fitted(
                text,
                &[],
                &[PhraseFit {
                    phrase: 3,
                    notes: 4
                }]
            )
            .is_err()
    );
}

#[test]
fn analysis_keeps_original_unicode_locations_and_special_moras() {
    let session = session();
    let text = " きゃっと、\r\n  コーヒー！\n漢字";
    let before = session.project().clone();
    let analysis = session.analyze_lyrics(text, &[]).unwrap();
    assert!(!analysis.complete);
    assert!(!analysis.dictionary_available);
    assert_eq!(
        analysis
            .phrases
            .iter()
            .map(|p| p.moras.len())
            .collect::<Vec<_>>(),
        [3, 4, 0]
    );
    assert_eq!(
        analysis.phrases.iter().map(|p| p.line).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for phrase in &analysis.phrases {
        assert_eq!(
            text.chars()
                .skip(phrase.start_char)
                .take(phrase.end_char - phrase.start_char)
                .collect::<String>(),
            phrase.text
        );
    }
    assert!(analysis.phrases[0].moras[1].closure);
    assert!(analysis.phrases[1].moras[1].long_vowel_mark);
    let explicit = session
        .analyze_lyrics(
            text,
            &[PhraseReading {
                phrase: 3,
                kana: "かんじ".into(),
            }],
        )
        .unwrap();
    assert!(explicit.complete);
    assert_eq!(explicit.phrases[2].text, "漢字");
    assert_eq!(explicit.phrases[2].reading_source, ReadingSource::Explicit);
    assert!(explicit.phrases[2].moras[1].nasal);
    assert_eq!(session.project(), &before);
}

#[test]
fn readings_reject_ambiguous_addresses_and_phrase_breaks() {
    let session = session();
    for readings in [
        vec![PhraseReading {
            phrase: 0,
            kana: "あ".into(),
        }],
        vec![PhraseReading {
            phrase: 2,
            kana: "あ".into(),
        }],
        vec![PhraseReading {
            phrase: 1,
            kana: "あ、い".into(),
        }],
        vec![PhraseReading {
            phrase: 1,
            kana: "漢字".into(),
        }],
        vec![
            PhraseReading {
                phrase: 1,
                kana: "あ".into(),
            },
            PhraseReading {
                phrase: 1,
                kana: "い".into(),
            },
        ],
    ] {
        assert!(session.analyze_lyrics("漢字", &readings).is_err());
    }
}

#[test]
fn a_zero_based_phrase_is_refused_with_the_numbering_spelt_out() {
    // Models number from zero more often than not; the refusal has to say which entry was
    // wrong and where numbering starts, or the retry guesses again.
    let session = session();
    let reading = |phrase| PhraseReading {
        phrase,
        kana: "あ".into(),
    };
    let error = session
        .analyze_lyrics("あ\nい", &[reading(1), reading(0)])
        .unwrap_err();
    assert!(error.contains("readings[1].phrase is 0"), "{error}");
    assert!(error.contains("first line is phrase 1"), "{error}");
    let error = session
        .analyze_lyrics_fitted(
            "あ\nい",
            &[],
            &[PhraseFit {
                phrase: 3,
                notes: 1,
            }],
        )
        .unwrap_err();
    assert!(error.contains("fit[0].phrase is 3"), "{error}");
    assert!(error.contains("2 phrases"), "{error}");
}

fn singer_pair(session: &mut Session) -> (NoteSpan, NoteSpan) {
    let track = session.add_singer_track("Vocal");
    let mut spans = Vec::new();
    for (i, text) in ["ずっときみと", "きみとあえる"].iter().enumerate() {
        let clip = session
            .add_midi_clip(track, "Verse", Ticks(i as i64 * 7680), Ticks(7680))
            .unwrap();
        let notes = (0..6)
            .map(|i| Note::new(60 + i % 3, Ticks(i as i64 * 960), Ticks(800)))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        session
            .write_lyrics(clip, &[0, 1, 2, 3, 4, 5], text)
            .unwrap();
        spans.push(NoteSpan {
            clip: clip.0,
            notes: vec![0, 1, 2, 3, 4, 5],
        });
    }
    (spans.remove(0), spans.remove(0))
}

#[test]
fn repair_previews_then_changes_only_target_words_in_one_undo() {
    let mut session = session();
    let (reference, target) = singer_pair(&mut session);
    for id in [reference.clip, target.clip] {
        let notes = &mut session.project.midi_clip_mut(ClipId(id)).unwrap().notes;
        notes[0].scoop = Some(Scoop {
            depth: 1.5,
            seconds: 0.1,
        });
        notes[0].phoneme_seconds = vec![0.1, 0.2];
        notes[1].phoneme_seconds = vec![0.1];
    }
    let before = session.project().clone();
    let revision = session.agent_document_revision().unwrap();
    let request = RepairRequest {
        target: target.clone(),
        reference: Some(reference.clone()),
        expected_revision: revision.clone(),
        candidates: vec![
            LyricCandidate {
                text: "きみとあえる".into(),
                reading: None,
            },
            LyricCandidate {
                text: "きっとあえる".into(),
                reading: None,
            },
        ],
        required_keywords: vec!["あえる".into()],
        apply: false,
    };
    let preview = session.repair_lyrics(&request).unwrap();
    assert_eq!(preview["selected_candidate"], 2);
    assert_eq!(preview["applied"], false);
    assert_eq!(session.project(), &before);
    assert_eq!(session.agent_document_revision().unwrap(), revision);
    let command = Command::RepairLyrics {
        request: RepairRequest {
            apply: true,
            ..request
        },
    };
    assert!(command.is_mutating());
    let result: serde_json::Value =
        serde_json::from_str(&session.agent_command(command).unwrap()).unwrap();
    assert_eq!(result["applied"], true);
    let mut expected = before.clone();
    expected.midi_clip_mut(ClipId(target.clip)).unwrap().name = "きっとあえる".into();
    let moras = split_kana_lyric("きっとあえる").unwrap();
    for (note, (text, phonemes)) in expected
        .midi_clip_mut(ClipId(target.clip))
        .unwrap()
        .notes
        .iter_mut()
        .zip(moras)
    {
        if note.lyric != text || note.phonemes != phonemes {
            note.lyric = text;
            note.phonemes = phonemes;
            note.phoneme_seconds.clear();
        }
    }
    assert_eq!(session.project(), &expected);
    assert_eq!(
        session
            .diagnose_clip_lyrics(&[ClipComparison { reference, target }])
            .unwrap()
            .status,
        "pass"
    );
    session.undo().unwrap();
    assert_eq!(session.project(), &before);
}

#[test]
fn rejected_stale_and_keyword_breaking_repairs_leave_the_whole_project_unchanged() {
    let mut session = session();
    let (reference, target) = singer_pair(&mut session);
    let before = session.project().clone();
    let mut request = RepairRequest {
        target,
        reference: Some(reference),
        expected_revision: session.agent_document_revision().unwrap(),
        candidates: vec![LyricCandidate {
            text: "きっとあえる".into(),
            reading: None,
        }],
        required_keywords: vec!["きみ".into()],
        apply: true,
    };
    let result = session.repair_lyrics(&request).unwrap();
    assert_eq!(result["applied"], false);
    assert_eq!(session.project(), &before);
    request.required_keywords.clear();
    request.expected_revision = "stale".into();
    assert!(session.repair_lyrics(&request).is_err());
    assert_eq!(session.project(), &before);
}

#[test]
fn explicit_readings_and_long_vowels_are_applied_without_losing_phonemes() {
    let mut session = session();
    let track = session.add_singer_track("Vocal");
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(7680))
        .unwrap();
    session
        .replace_notes(
            clip,
            (0..4)
                .map(|i| Note::new(60, Ticks(i * 960), Ticks(800)))
                .collect(),
        )
        .unwrap();
    session
        .write_lyrics(clip, &[0, 1, 2, 3], "あいうえ")
        .unwrap();
    let request = RepairRequest {
        target: NoteSpan {
            clip: clip.0,
            notes: vec![3, 2, 1, 0],
        },
        reference: None,
        expected_revision: session.agent_document_revision().unwrap(),
        candidates: vec![LyricCandidate {
            text: "珈琲".into(),
            reading: Some("コーヒー".into()),
        }],
        required_keywords: vec!["珈琲".into()],
        apply: true,
    };
    assert_eq!(session.repair_lyrics(&request).unwrap()["applied"], true);
    assert_eq!(session.midi_clip(clip).unwrap().name, "珈琲");
    let notes = &session.midi_clip(clip).unwrap().notes;
    assert_eq!(notes[1].phonemes, ["o"]);
    assert_eq!(notes[3].phonemes, ["i"]);
    assert_eq!(notes[0].lyric, "珈琲");
    assert_eq!(notes[1].lyric, "+");
    let span = request.target.clone();
    let report = session
        .diagnose_clip_lyrics(&[ClipComparison {
            reference: span.clone(),
            target: span,
        }])
        .unwrap();
    assert_eq!(report.status, "pass");
    assert_eq!(report.sections[0].analysis.phrases[0].text, "珈琲");
    assert_eq!(
        report.sections[0].analysis.phrases[0].reading.as_deref(),
        Some("こおひい")
    );
    let before = session.project().clone();
    let mut again = request;
    again.expected_revision = session.agent_document_revision().unwrap();
    assert_eq!(session.repair_lyrics(&again).unwrap()["applied"], false);
    assert_eq!(session.project(), &before);
}

#[test]
fn diagnosis_and_repair_do_not_hide_musical_or_alignment_mismatches() {
    let mut session = session();
    let (reference, target) = singer_pair(&mut session);
    session.set_note_lyric(ClipId(target.clip), 0, "").unwrap();
    let report = session
        .diagnose_clip_lyrics(&[ClipComparison {
            reference: reference.clone(),
            target: target.clone(),
        }])
        .unwrap();
    assert_eq!(report.status, "incomplete");
    session
        .project
        .midi_clip_mut(ClipId(target.clip))
        .unwrap()
        .notes[0]
        .pitch += 1;
    let report = session
        .diagnose_clip_lyrics(&[ClipComparison {
            reference: reference.clone(),
            target: target.clone(),
        }])
        .unwrap();
    assert_eq!(report.status, "fail");
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "musical_slot_mismatch")
    );
    let before = session.project().clone();
    let request = RepairRequest {
        target,
        reference: Some(reference),
        expected_revision: session.agent_document_revision().unwrap(),
        candidates: vec![LyricCandidate {
            text: "きっとあえる".into(),
            reading: None,
        }],
        required_keywords: Vec::new(),
        apply: true,
    };
    assert!(session.repair_lyrics(&request).is_err());
    assert_eq!(session.project(), &before);
}

#[test]
fn stored_one_mora_phonemes_establish_slots_for_original_text_and_plus_labels() {
    let mut session = session();
    let (reference, target) = singer_pair(&mut session);
    session
        .write_lyrics(ClipId(target.clip), &[0, 1, 2, 3, 4, 5], "きっとあえる")
        .unwrap();
    let clip = session
        .project
        .midi_clip_mut(ClipId(reference.clip))
        .unwrap();
    clip.notes[0].lyric = "ずっと君と".into();
    for note in &mut clip.notes[1..] {
        note.lyric = "+".into();
    }
    let report = session
        .diagnose_clip_lyrics(&[ClipComparison { reference, target }])
        .unwrap();
    assert_eq!(report.status, "pass");
    assert_eq!(report.sections[0].analysis.phrases[0].text, "ずっと君と");
    assert_eq!(report.sections[0].analysis.phrases[0].moras.len(), 6);
}

#[test]
fn analysis_shows_the_dictionary_accent_and_nothing_without_it() {
    let analysis = session().analyze_lyrics("箸を持つ", &[]).unwrap();
    assert_eq!(analysis.phrases[0].accent, None, "no dictionary, no guess");
    let Some(folder) = std::env::var_os("AURIS_JAPANESE_DICTIONARY") else {
        return;
    };
    let mut session = session();
    session
        .set_japanese_dictionary(Some(std::path::Path::new(&folder)))
        .unwrap();
    let analysis = session.analyze_lyrics("箸を持つ、橋を渡る", &[]).unwrap();
    let accents: Vec<_> = analysis
        .phrases
        .iter()
        .map(|phrase| phrase.accent.clone().unwrap())
        .collect();
    assert!(
        accents[0].starts_with("は＼し"),
        "箸 is 頭高: {}",
        accents[0]
    );
    assert!(
        accents[1].starts_with("はし＼"),
        "橋 is 尾高: {}",
        accents[1]
    );
    // The notation reads back as the same accent phrases.
    let parsed = auris_vocal::parse_accent_notation(&accents[1]).unwrap();
    assert_eq!(
        auris_vocal::accent_notation(&parsed).as_deref(),
        Some(accents[1].as_str())
    );
    let explicit = session
        .analyze_lyrics(
            "橋",
            &[PhraseReading {
                phrase: 1,
                kana: "はし".into(),
            }],
        )
        .unwrap();
    assert_eq!(explicit.phrases[0].accent, None);
}

#[test]
fn a_misfit_offers_the_melody_side_ways_to_close_it() {
    assert_eq!(
        melody_options(2),
        [
            MelodyOption::AddPickup { notes: 2 },
            MelodyOption::Split { notes: 2 }
        ]
    );
    assert_eq!(melody_options(-1), [MelodyOption::Merge { notes: 1 }]);
    assert!(melody_options(0).is_empty());
    assert_eq!(
        serde_json::to_value(MelodyOption::AddPickup { notes: 1 }).unwrap(),
        serde_json::json!({"kind": "add_pickup", "notes": 1})
    );
}
