//! Errors a session command can return.

use std::path::PathBuf;

use thiserror::Error;

/// Anything that can go wrong while driving a session.
#[derive(Debug, Error)]
pub enum SessionError {
    /// A file could not be read, written, decoded or encoded.
    #[error(transparent)]
    Io(#[from] auris_io::IoError),

    /// The rendering engine refused the request.
    #[error(transparent)]
    Engine(#[from] auris_engine::EngineError),

    /// A core invariant was violated.
    #[error(transparent)]
    Core(#[from] auris_core::CoreError),

    /// No plugin is registered under this id.
    #[error("no plugin is registered as `{0}`")]
    UnknownPlugin(String),

    /// The requested track does not exist.
    #[error("no track with id {0}")]
    UnknownTrack(u64),

    /// The project has no SoundFont with this id.
    #[error("no soundfont with id {0}")]
    UnknownSoundFont(u64),

    /// Nothing in the catalogue answers to that name.
    ///
    /// An error rather than a clamp, unlike the settings a session quietly corrects: a grid of
    /// zero has an obvious nearest right answer and a misspelt progression has none. Writing
    /// nothing and saying nothing would be the worst of the three.
    #[error("no chord progression is named `{0}`")]
    UnknownProgression(String),

    /// The requested clip does not exist.
    #[error("no clip with id {0}")]
    UnknownClip(u64),

    /// The clip cannot be divided at the requested position.
    #[error("clip {0} cannot be split there")]
    CannotSplit(u64),

    /// The operation only applies to one kind of track.
    #[error("track {id} is {actual}, but this needs {expected}")]
    WrongTrackKind {
        /// Id of the track that was addressed.
        id: u64,
        /// What the track actually is.
        actual: &'static str,
        /// What the operation needed.
        expected: &'static str,
    },

    /// The document has never been saved, so there is no path to save it back to.
    #[error("the project has no path yet; save it somewhere first")]
    NoPath,

    /// The application settings file could not be written.
    #[error("could not write {path}: {source}")]
    SettingsWrite {
        /// File that could not be written.
        path: PathBuf,
        /// Why it failed.
        #[source]
        source: std::io::Error,
    },

    /// Reopening the audio device failed.
    #[error("could not switch audio device: {0}")]
    AudioRestart(String),

    /// A project referenced audio files that could not be loaded.
    ///
    /// The project itself opened; these clips will be silent until the files come back.
    #[error("{} audio file(s) could not be loaded", .0.len())]
    MissingAudio(Vec<PathBuf>),
}
