//! What the tests of every file here are written against.
//!
//! `#[cfg(test)]` and nothing else. Nearly every test in this module opens a headless session and
//! then asks one question of it, and a good few need a track with a clip on it, a font the
//! document names, a progression already stamped, or a directory that cleans itself up — so those
//! live where each file's own tests can reach them rather than being copied into eleven.
//!
//! [`undo_depth`] is the odd one out and worth knowing about: it counts the history by walking it
//! to the bottom and back, so it is only safe as the *last* question a test asks about steps.

use std::path::{Path, PathBuf};

use auris_core::theory::numeral::Numeral;
use auris_core::time::Ticks;
use auris_core::{AssetPath, AudioBuffer, ClipId, Note, SoundFontId, TrackId};

use super::{Session, SessionOptions};

pub(super) fn session() -> Session {
    Session::new(SessionOptions::headless()).expect("a headless session always opens")
}

/// How many steps deep the undo stack is, counted by walking it.
pub(super) fn undo_depth(session: &mut Session) -> usize {
    let mut depth = 0;
    while session.undo().is_some() {
        depth += 1;
    }
    while session.redo().is_some() {}
    depth
}

/// Registers a font in the document without a file behind it.
///
/// Enough to exercise every command that decides what a track *plays*; what it *sounds* like
/// needs a real SoundFont, which is somebody's 200 MB file rather than a test fixture.
pub(super) fn named_font(session: &mut Session, name: &str) -> SoundFontId {
    session.project.add_soundfont(
        name,
        AssetPath::external(format!("/fonts/{name}.sf2")),
        1024,
    )
}

/// A directory under the system temp area that deletes itself when the test ends.
pub(super) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(super) fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "auris-session-{}-{unique}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("a temp directory can be made");
        Self { path }
    }

    pub(super) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Writes a short tone so `import_audio` has a real file to decode.
    pub(super) fn tone(&self, name: &str) -> PathBuf {
        write_tone(&self.join(name), 480)
    }
}

/// Writes a decodable tone of `frames` frames wherever it is asked to.
///
/// The length is a parameter because the tests about a file that moved turn on two files of
/// the same name being different files, and the length is what makes them different sizes.
pub(super) fn write_tone(path: &Path, frames: usize) -> PathBuf {
    let mut buffer = AudioBuffer::new(2, frames, 48_000.0);
    for channel in 0..2 {
        for (frame, sample) in buffer.channel_mut(channel).iter_mut().enumerate() {
            *sample = (frame as f32 * 0.01).sin() * 0.5;
        }
    }
    auris_io::write_wav(path, &buffer, &auris_io::WavExportSettings::default())
        .expect("a WAV file writes");
    path.to_path_buf()
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A session with one instrument track holding a one-bar clip of two notes.
pub(super) fn session_with_clip() -> (Session, TrackId, ClipId) {
    let mut session = session();
    let track = session.add_default_instrument_track("Lead").unwrap();
    let clip = session
        .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
        .unwrap();
    session
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    session
        .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
        .unwrap();
    (session, track, clip)
}

/// One bar of 4/4, which is what every harmony test below counts in.
pub(super) const BAR: Ticks = Ticks(3840);

pub(super) fn numeral(text: &str) -> Numeral {
    Numeral::parse(text).expect("a numeral the test wrote itself")
}

/// A session with four bars of the axis progression and a track to put a part on.
pub(super) fn with_a_progression() -> (Session, TrackId) {
    let mut session = session();
    let track = session.add_default_instrument_track("Bass").unwrap();
    session
        .stamp_named_progression("axis", Ticks::ZERO, 4)
        .unwrap();
    session.forget_history();
    (session, track)
}
