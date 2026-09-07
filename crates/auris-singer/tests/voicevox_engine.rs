//! Optional smoke test against a running VOICEVOX Engine.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use auris_singer::{Acceleration, BackendKind, VoiceModel};
use auris_vocal::{SingerFrames, SingerNote, SingerScore};

#[test]
fn a_running_voicevox_engine_sings_a_score() {
    let Ok(url) = std::env::var("AURIS_VOICEVOX_TEST_URL") else {
        eprintln!("set AURIS_VOICEVOX_TEST_URL to run the VOICEVOX Engine smoke test");
        return;
    };
    let path = connection_path();
    std::fs::write(
        &path,
        format!(
            r#"{{"format_version":1,"name":"VOICEVOX smoke test","url":"{url}","styles":[{{"name":"波音リツ / ノーマル","query_style_id":6000,"decode_style_id":3009}},{{"name":"ずんだもん / ツンツン","query_style_id":6000,"decode_style_id":3007}}]}}"#
        ),
    )
    .unwrap();

    let mut model = VoiceModel::load(&path, Acceleration::Auto).unwrap();
    assert_eq!(model.backend_kind(), BackendKind::Voicevox);
    for events in [
        vec![(25, "ラ")],
        vec![(25, "コ"), (25, "ー"), (25, "ヒ"), (25, "ー")],
        vec![(25, "シュ"), (25, "ー"), (25, "テ"), (25, "ー")],
        vec![(25, " ｶﾞ "), (25, "ｰ"), (25, "か\u{3099}"), (25, " ー ")],
        vec![(25, "ッ"), (25, "カ"), (25, "ン"), (25, "ア")],
        vec![(2, "ア"), (25, "カ")],
        vec![(25, "ア"), (2, ""), (25, "カ")],
        vec![(1, "ア"), (25, "ア")],
        vec![(25, "")],
    ] {
        let frames = 30
            + events
                .iter()
                .map(|(length, _)| *length as usize)
                .sum::<usize>();
        let mut f0_hz = vec![0.0; frames];
        let mut energy = vec![0.0; frames];
        let mut offset = 15;
        for (length, lyric) in &events {
            let end = offset + *length as usize;
            if !lyric.is_empty() {
                f0_hz[offset..end].fill(261.625_55);
                energy[offset..end].fill(0.8);
            }
            offset = end;
        }
        let curves = SingerFrames {
            hop_seconds: model.info().hop_seconds(),
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: energy
                .iter()
                .map(|energy| u32::from(*energy > 0.0))
                .collect(),
            f0_hz,
            energy,
        };
        let rest = SingerNote {
            key: None,
            frame_length: 15,
            lyric: String::new(),
        };
        let mut score = SingerScore {
            notes: vec![rest.clone()],
        };
        score
            .notes
            .extend(events.iter().map(|(length, lyric)| SingerNote {
                key: (!lyric.is_empty()).then_some(60),
                frame_length: *length,
                lyric: (*lyric).into(),
            }));
        score.notes.push(rest);
        let original = score.clone();
        for speaker in [0, 1] {
            let samples = model.sing_score(&curves, &score, speaker, 0).unwrap();
            assert_eq!(samples.len(), frames * model.info().hop_length as usize);
            assert!(samples.iter().all(|sample| sample.is_finite()));
            if events.iter().any(|(_, lyric)| !lyric.is_empty()) {
                assert!(
                    samples.iter().any(|sample| sample.abs() > 0.001),
                    "the Engine returned silence for {events:?}"
                );
            }
        }
        assert_eq!(score, original);
    }
    std::fs::remove_file(path).unwrap();
}

fn connection_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "auris-voicevox-smoke-{}-{unique}.voicevox.json",
        std::process::id()
    ))
}
