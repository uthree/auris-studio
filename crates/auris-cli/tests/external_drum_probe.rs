//! Opt-in checks against an installed instrument with an audible initial patch.
//!
//! These exercise source snapshotting and the actual isolated probe executable. A synthesized
//! initial patch can remain unassigned: plugin names and note addresses supply no drum labels.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auris_session::prelude::PluginKind;
use auris_session::{DrumKitAnalysis, DrumScanOptions, Session, SessionOptions};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "auris-external-drum-probe-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires AURIS_CLAP_DRUM_SMOKE_PLUGIN to name an installed CLAP instrument"]
fn an_external_clap_snapshot_produces_real_acoustic_evidence() {
    external_probe("AURIS_CLAP_DRUM_SMOKE_PLUGIN", false);
}

#[test]
#[ignore = "requires AURIS_VST3_SMOKE_PLUGIN to name an installed VST3 instrument"]
fn an_external_vst3_snapshot_produces_real_acoustic_evidence() {
    external_probe("AURIS_VST3_SMOKE_PLUGIN", true);
}

fn external_probe(variable: &str, vst3: bool) {
    let path = PathBuf::from(std::env::var_os(variable).expect("the plugin path is required"));
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session
        .add_default_instrument_track("External probe")
        .unwrap();
    if vst3 {
        let info = session
            .vst3_plugins_in(&path)
            .unwrap()
            .into_iter()
            .find(|info| info.kind == PluginKind::Instrument)
            .expect("the selected bundle must contain an instrument");
        eprintln!("probing {} {} ({})", info.vendor, info.name, info.class_id);
        session
            .set_vst3_instrument(track, &path, &info.class_id)
            .unwrap();
    } else {
        let info = session
            .hosted_plugins_in(&path)
            .unwrap()
            .into_iter()
            .find(|info| info.kind == PluginKind::Instrument)
            .expect("the selected library must contain an instrument");
        eprintln!("probing {} {} ({})", info.vendor, info.name, info.clap_id);
        session
            .set_hosted_instrument(track, &path, &info.clap_id)
            .unwrap();
    }
    session.poll();
    let before = session.project().clone();
    let options = DrumScanOptions {
        notes: vec![36, 60, 84],
        velocities: vec![0.4, 0.9],
        repetitions: 2,
        seconds_per_note: 0.75,
        timeout_seconds: 30,
        ..DrumScanOptions::default()
    };
    let request = session.drum_probe_request(track, &options).unwrap();
    assert!(
        request
            .state
            .hosted_bytes()
            .is_some_and(|state| !state.is_empty())
    );
    let fixture = Fixture::new();
    let input = fixture.0.join("request.json");
    let output = fixture.0.join("response.json");
    std::fs::write(&input, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_auris"));
    command
        .arg("--drum-probe-worker")
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut worker = Worker(command.spawn().unwrap());
    let started = Instant::now();
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(status.success(), "the external worker failed: {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45),
            "external probe timed out"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let report: Result<DrumKitAnalysis, String> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let report = report.expect("the real plugin should restore and render in the worker");
    report.validate().unwrap();
    assert_eq!(report.source_fingerprint, request.source_fingerprint);
    assert_eq!(report.voices.len(), 3);
    assert!(report.voices.iter().all(|voice| voice.samples.len() == 4));
    assert!(
        report.voices.iter().any(|voice| !voice.silent),
        "use an instrument whose initial patch sounds at a tested address"
    );
    assert_eq!(
        session.project(),
        &before,
        "measurement changed the live project"
    );
    session
        .apply_drum_map(&report, false)
        .expect("the unchanged live source should accept its measured evidence");
    for voice in report.voices {
        let sound = &voice.samples[0].acoustics;
        eprintln!(
            "address {}: silent={}, centroid={:.1}Hz, energy duration={:.3}s, fitness={:?}",
            voice.note,
            voice.silent,
            sound.spectrum.centroid_hz,
            sound.energy_duration_seconds,
            voice.fitness
        );
    }
    eprintln!("measured proposal: {:?}", report.proposed_map.voices);
}
