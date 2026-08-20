#![warn(missing_docs)]

//! File I/O for Auris Studio: audio import, audio export and project persistence.
//!
//! This crate is the only place that talks to third-party codec libraries, and the only place
//! that reads or writes a *project's own* files — the document, the audio it owns, the MIDI and
//! SoundFonts it names. That is what keeps Symphonia, Hound and the rest out of the engine and
//! the interface.
//!
//! It is not the only place that talks to the filesystem, and does not try to be: the settings,
//! the keymap, the colour scheme and the panel layout are each read and written by whoever owns
//! them, in `auris-session` and in the frontend, with plain `std::fs` over a serde type. Those are
//! preferences about the application rather than parts of a song, they belong to no project
//! folder, and routing them through here would buy nothing but a longer call.
//!
//! Nothing here is realtime-safe — every function allocates and blocks — so calls belong on a
//! worker thread.
//!
//! * [`import`] decodes any container Symphonia understands into an
//!   [`AudioBuffer`](auris_core::AudioBuffer), optionally resampling to the project rate.
//! * [`export`] writes a rendered buffer out as 16-bit, 24-bit or 32-bit float WAV.
//! * [`record`] writes a WAV that does not exist yet, block by block, for as long as a take runs.
//! * [`project_file`] saves and loads the [`Project`](auris_core::Project) document as JSON.
//! * [`assets`] copies the files a project refers to into its folder, and finds them again when
//!   they have moved.

pub mod assets;
pub mod error;
pub mod export;
pub mod import;
pub mod midi;
pub mod project_file;
pub mod record;
pub mod soundfont;

pub use assets::{byte_size, copy_into, find_named};
pub use error::{IoError, Result};
pub use export::{WavBitDepth, WavExportSettings, write_wav};
pub use import::{
    DecodedAudio, decode_audio_file, import_audio_file, resample_buffer, supported_extensions,
};
pub use midi::{
    MidiImport, MidiTrack, midi_extensions, read_midi_bytes, read_midi_file, write_midi_bytes,
    write_midi_file,
};
pub use project_file::{
    AUDIO_DIR, PROJECT_EXTENSION, document_in_folder, load_project, project_folder, save_project,
};
pub use record::WavRecorder;
pub use soundfont::{
    SoundFontPreset, font_name, load_soundfont, preset_count, presets, soundfont_extensions,
};

/// A parsed SoundFont, as [`load_soundfont`] returns it.
///
/// Re-exported because a caller that holds one has to be able to name it — and because the type
/// crosses a crate boundary to `auris-sampler`, so both sides naming the library themselves would
/// be two resolved copies and two unrelated types.
pub use rustysynth::SoundFont;

#[cfg(test)]
mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A path in the system temp directory that deletes itself when the test ends.
    ///
    /// Tests run concurrently within a process and several test binaries may run at once, so the
    /// name mixes the process id, a clock reading and a counter to stay unique.
    pub(crate) struct TempFile {
        path: PathBuf,
    }

    impl TempFile {
        pub(crate) fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "auris-io-{}-{nanos}-{unique}-{name}",
                std::process::id()
            ));
            Self { path }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            // Either kind: a recorder is handed a path inside a folder it has to create, so some
            // of these names end up being directories with a take in them.
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
