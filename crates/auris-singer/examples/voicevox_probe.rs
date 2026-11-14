//! Renders kana phrases through Auris's VOICEVOX adapter and directly through the Engine.
//!
//! `cargo run -p auris-singer --example voicevox_probe -- <out-dir> [decode-style] [note-ticks]` writes
//! `<n>-auris<velocity>.wav` and `<n>-direct.wav` per phrase plus `phrases.json`, for
//! `tools/eval/voicevox_intelligibility.py` to compare. `AURIS_VOICEVOX_TEST_URL` names the
//! Engine (default `http://127.0.0.1:50021`).

use std::path::{Path, PathBuf};

use auris_core::{ClipId, MidiClip, Note, PluginState, SingerTrack, TempoMap, Ticks};
use auris_singer::{Acceleration, VoiceModel};
use auris_vocal::{SingerNote, render_expression_frames, render_score};
use serde_json::{Value, json};

const PHRASES: &[&str] = &[
    "さくらさくら",
    "かきくけこ",
    "さしすせそ",
    "たちつてと",
    "なにぬねの",
    "はひふへほ",
    "まみむめも",
    "らりるれろ",
    "がぎぐげご",
    "ざじずぜぞ",
    "ぱぴぷぺぽ",
    "ばびぶべぼ",
    "ありがとう",
    "こんにちは",
    "おはよう",
    "ともだち",
    "あたらしい",
    "さよなら",
    "きらきらひかる",
    "しあわせ",
    "はなびがひかる",
    "ほしのふね",
    "へやのぴあの",
    "ぽすとのぺん",
];

/// Note velocities rendered through Auris; the Engine's own path has no velocity.
const VELOCITIES: &[f32] = &[1.0, 0.8, 0.5];

const PADDING: u32 = 94;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .expect("usage: voicevox_probe <out-dir> [decode-style] [note-ticks]"),
    );
    let decode: u32 = args.next().map_or(3002, |style| style.parse().unwrap());
    let length: i64 = args.next().map_or(480, |ticks| ticks.parse().unwrap());
    let url = std::env::var("AURIS_VOICEVOX_TEST_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:50021".into());
    std::fs::create_dir_all(&out).unwrap();
    let connection = out.join("probe.voicevox.json");
    std::fs::write(
        &connection,
        serde_json::to_vec(&json!({
            "format_version": 1,
            "name": "probe",
            "url": url,
            "styles": [{"name": "probe", "query_style_id": 6000, "decode_style_id": decode}],
        }))
        .unwrap(),
    )
    .unwrap();
    let mut model = VoiceModel::load(&connection, Acceleration::Cpu).unwrap();
    let tempo = TempoMap::constant(100.0);
    let mut manifest = Vec::new();
    for (index, phrase) in PHRASES.iter().enumerate() {
        let moras = auris_vocal::split_kana_lyric(phrase).unwrap();
        let stem = format!("{index:02}");
        let mut score = None;
        for velocity in VELOCITIES {
            let track = track(&moras, *velocity, length, model.info().hop_seconds());
            let notes = render_score(&track, &tempo, &auris_core::Tuning::standard());
            let frames = render_expression_frames(&track, &tempo, &auris_core::Tuning::standard());
            let auris = model.sing_score(&frames, &notes, 0, 0).unwrap();
            write_wav(&out.join(format!("{stem}-auris{velocity:.1}.wav")), &auris);
            score = Some(notes);
        }
        let direct = direct(&url, decode, &score.unwrap().notes);
        write_wav(&out.join(format!("{stem}-direct.wav")), &direct);
        manifest.push(json!({"id": stem, "kana": phrase}));
        eprintln!("{stem} {phrase}");
    }
    std::fs::write(
        out.join("phrases.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

/// One mora to a note, at 100 bpm eighth notes over a stepwise melody.
fn track(moras: &[(String, Vec<String>)], velocity: f32, length: i64, hop: f64) -> SingerTrack {
    let mut clip = MidiClip::new(ClipId(1), "probe", Ticks::ZERO, Ticks(480 * 32));
    let melody = [60, 62, 64, 65, 67, 65, 64, 62];
    for (at, (mora, phonemes)) in moras.iter().enumerate() {
        let mut note = Note::new(
            melody[at % melody.len()],
            Ticks(480 + 480 * at as i64),
            Ticks(length),
        );
        note.velocity = velocity;
        note.lyric = mora.clone();
        note.phonemes = phonemes.clone();
        clip.notes.push(note);
    }
    SingerTrack {
        instrument_id: "auris.synth.vocal".into(),
        instrument_state: PluginState::empty(),
        clips: vec![clip],
        frame_hop: hop,
        voice: None,
        take: None,
    }
}

/// The Engine's own pipeline: one query, one synthesis, nothing adjusted in between.
fn direct(url: &str, decode: u32, notes: &[SingerNote]) -> Vec<f32> {
    let mut padded = vec![json!({"key": null, "frame_length": PADDING, "lyric": ""})];
    padded.extend(notes.iter().map(|note| json!(note)));
    padded.push(json!({"key": null, "frame_length": PADDING, "lyric": ""}));
    let mut query: Value = ureq::post(&format!("{url}/sing_frame_audio_query?speaker=6000"))
        .send_json(json!({"notes": padded}))
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    query["outputSamplingRate"] = json!(24_000);
    query["outputStereo"] = json!(false);
    let bytes = ureq::post(&format!("{url}/frame_synthesis?speaker={decode}"))
        .send_json(query)
        .unwrap()
        .body_mut()
        .read_to_vec()
        .unwrap();
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
    let scale = f32::from(i16::MAX);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / scale)
        .collect();
    let hop = 256;
    let start = PADDING as usize * hop;
    samples[start..samples.len() - start].to_vec()
}

fn write_wav(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
}
