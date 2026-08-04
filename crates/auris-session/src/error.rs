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

    /// The clip holds notes rather than audio, so it has no gain or fades of its own.
    ///
    /// A note clip's loudness is its velocities; offering to fade one here would quietly do
    /// nothing, which is worse than saying which kind of clip was addressed.
    #[error("clip {0} is not an audio clip")]
    NotAudio(u64),

    /// A numeric setting arrived as NaN or infinity.
    ///
    /// Refused rather than clamped: a clamp picks the nearest value that exists, and NaN has
    /// no nearest anything — whatever produced it was a bug, and folding it into a real number
    /// would bury that.
    #[error("{0} is not a finite number")]
    NotFinite(f64),

    /// The clip was played rather than written, so there is no recipe to write it again from.
    ///
    /// An error rather than a quiet no-op: asking to regenerate a clip somebody performed is
    /// either a mistake or a misunderstanding, and answering "done" to it would be a lie about
    /// notes nobody touched.
    #[error("clip {0} was not written by the composer")]
    NotGenerated(u64),

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

    /// Saving here would replace a project that is already in that folder.
    ///
    /// Raised before anything is written, because the system save dialog cannot warn about it:
    /// it offers to replace the path the user typed, and a project goes into a folder named
    /// after that path instead. A host that has asked the user may call
    /// [`Session::save_as_replacing`](crate::Session::save_as_replacing) to go ahead.
    #[error("{} is already a project", .0.display())]
    WouldReplace(PathBuf),
}
