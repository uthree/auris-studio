//! Exercise the real executable's private worker and public analysis command together.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use auris_session::prelude::*;
use auris_session::{DrumKitAnalysis, Session, SessionOptions};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("auris-cli-drum-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn subprocess_measurement_is_read_only_until_applied_and_survives_reload() {
    let fixture = Fixture::new();
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session
        .add_instrument_track("Unknown source", "auris.synth.drumkit")
        .unwrap();
    let clip = session
        .generate_clip(
            track,
            Ticks::ZERO,
            Ticks::QUARTER * 4,
            ClipRecipe::new(ClipPreset::Kick, 42),
        )
        .unwrap();
    session.save_as(&fixture.0.join("Song.auris")).unwrap();
    let path = session.path().unwrap().to_owned();
    let before = std::fs::read(&path).unwrap();
    let run = |apply: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_auris"));
        command.arg("analyze-drums").arg(&path).args([
            "--track",
            "Unknown source",
            "--first-note",
            "35",
            "--last-note",
            "36",
        ]);
        if apply {
            command.args(["--apply", "--remap-clips"]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<DrumKitAnalysis>(&output.stdout).unwrap()
    };
    let measured = run(false);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(measured.voices.len(), 2);
    assert!(measured.voices.iter().all(|voice| voice.samples.len() == 6));
    let kick = measured.proposed_map.voices[&DrumRole::Kick];
    assert!([35, 36].contains(&kick));
    assert!(!measured.proposed_map.voices.contains_key(&DrumRole::Snare));
    let applied = run(true);
    session.open(&path).unwrap();
    let state = &session
        .project()
        .track(track)
        .unwrap()
        .kind
        .as_instrument()
        .unwrap()
        .instrument_state;
    assert_eq!(DrumMap::load(state), Some(applied.proposed_map));
    assert!(
        session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .all(|note| note.pitch == kick)
    );
    session.regenerate_clip(clip).unwrap();
    assert!(
        session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .all(|note| note.pitch == kick)
    );
}

#[test]
fn private_worker_rejects_an_invalid_request_without_starting_a_scan() {
    let fixture = Fixture::new();
    let input = fixture.0.join("request.json");
    let output = fixture.0.join("response.json");
    std::fs::write(&input, b"{}").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_auris"))
        .arg("--drum-probe-worker")
        .arg(input)
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    let report: Result<DrumKitAnalysis, String> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert!(report.unwrap_err().contains("missing field"));
}
