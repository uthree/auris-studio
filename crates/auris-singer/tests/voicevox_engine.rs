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
    for lyrics in [
        vec!["ラ"],
        vec!["コ", "ー", "ヒ", "ー"],
        vec!["シュ", "ー", "テ", "ー"],
    ] {
        let frames = 30 + lyrics.len() * 25;
        let mut f0_hz = vec![0.0; frames];
        let mut energy = vec![0.0; frames];
        f0_hz[15..frames - 15].fill(261.625_55);
        energy[15..frames - 15].fill(0.8);
        let curves = SingerFrames {
            hop_seconds: model.info().hop_seconds(),
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: (0..frames)
                .map(|frame| u32::from((15..frames - 15).contains(&frame)))
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
        score.notes.extend(lyrics.iter().map(|lyric| SingerNote {
            key: Some(60),
            frame_length: 25,
            lyric: (*lyric).into(),
        }));
        score.notes.push(rest);
        for speaker in [0, 1] {
            let samples = model.sing_score(&curves, &score, speaker, 0).unwrap();
            assert_eq!(samples.len(), frames * model.info().hop_length as usize);
            assert!(samples.iter().all(|sample| sample.is_finite()));
            assert!(
                samples.iter().any(|sample| sample.abs() > 0.001),
                "the Engine returned silence"
            );
        }
        assert_eq!(score.notes[1].lyric, lyrics[0]);
        if lyrics.contains(&"ー") {
            assert_eq!(score.notes[2].lyric, "ー");
        }
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
