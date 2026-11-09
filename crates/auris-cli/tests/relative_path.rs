//! A project named by a bare file name, from inside its own folder, still finds its audio.
//!
//! Issue #141: `auris render Song.auris` typed in the project folder resolved `Audio/…` against
//! the session's private working folder instead, warned that every take was missing, and rendered
//! the song without them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "auris-cli-relative-{}-{unique}",
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

/// A one-second 16-bit mono sine at 48 kHz, written by hand so the test needs no audio crate.
fn write_tone(path: &Path) {
    let rate = 48_000u32;
    let samples: Vec<i16> = (0..rate)
        .map(|frame| ((frame as f32 * 0.05).sin() * 12_000.0) as i16)
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_bare_document_name_resolves_its_audio_against_the_current_directory() {
    let fixture = Fixture::new();
    let tone = fixture.0.join("take.wav");
    write_tone(&tone);
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    session.import_audio(&tone, Ticks::ZERO).unwrap();
    session.save_as(&fixture.0.join("Song.auris")).unwrap();
    let document = session.path().unwrap().to_owned();
    drop(session);
    std::fs::remove_file(&tone).unwrap();
    let folder = document.parent().unwrap();
    assert!(
        folder.join("Audio").join("take.wav").is_file(),
        "Save As collects the take into the project's own Audio folder"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_auris"))
        .current_dir(folder)
        .arg("info")
        .arg(document.file_name().unwrap())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "info failed: {stderr}");
    assert!(
        !stderr.contains("take.wav"),
        "the take beside the document must not be reported missing: {stderr}"
    );
}
