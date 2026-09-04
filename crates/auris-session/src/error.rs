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

    /// A third-party plugin could not be loaded, instantiated or driven.
    #[error(transparent)]
    Clap(#[from] auris_clap::ClapError),

    /// A VST3 plugin could not be discovered, loaded, or driven.
    #[error(transparent)]
    Vst3(#[from] auris_vst3::Vst3Error),

    /// The requested track does not exist.
    #[error("no track with id {0}")]
    UnknownTrack(u64),

    /// The project has no SoundFont with this id.
    #[error("no soundfont with id {0}")]
    UnknownSoundFont(u64),

    /// The shipped sound library is not installed, so a General MIDI sound cannot be adopted.
    #[error("the shipped sound library is not installed; General MIDI sounds are unavailable")]
    LibraryMissing,

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

    /// The clip exists but holds no note at that index.
    ///
    /// Most many-note commands quietly skip indices a stale selection no longer covers; the
    /// commands that write *words* refuse instead, because a lyric that silently landed nowhere
    /// is a lyric somebody typed and lost.
    #[error("clip {clip} has no note {index}")]
    UnknownNote {
        /// The clip that was addressed.
        clip: u64,
        /// The index that named nothing.
        index: usize,
    },

    /// The requested send does not exist on that track.
    #[error("track {track} has no send with id {send}")]
    UnknownSend {
        /// The track that was addressed.
        track: u64,
        /// The send that is not on it.
        send: u64,
    },

    /// Audio can only be routed into a bus, and this track is not one.
    #[error("track {0} is not a bus, so nothing can be routed into it")]
    NotABus(u64),

    /// The routing would have made a signal loop back on itself.
    ///
    /// An error rather than something to untangle later: a loop has no order it can be rendered
    /// in — every bus in it is waiting for itself — so there is nothing sensible to do with a
    /// document that holds one. The one that arrives in a *file* is repaired instead, because
    /// refusing to open it would be a worse answer than breaking one edge.
    #[error("routing track {from} into track {to} would loop back on itself")]
    RoutingLoop {
        /// The track being routed.
        from: u64,
        /// Where it was aimed.
        to: u64,
    },

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

    /// The clip has no notes in it, so there is no melody to read a harmony off.
    ///
    /// An error rather than an empty accompaniment: a bass line and a comp written from a tune
    /// nobody played would be an accompaniment to nothing, and answering "done" to that is worse
    /// than saying which clip was empty.
    #[error("clip {0} has no notes to accompany")]
    NothingToAccompany(u64),

    /// A lyric could not be turned into phonemes, or the Japanese dictionary failed.
    #[error(transparent)]
    Vocal(#[from] auris_vocal::VocalError),

    /// A voice model could not be loaded, or refused to sing.
    #[error(transparent)]
    Sing(#[from] auris_singer::SingError),

    /// A singer command needs a voice model and the track names none.
    #[error("track {0} names no voice model; choose one first")]
    NoVoice(u64),

    /// A speaker was named that the track's voice model does not have.
    #[error("the voice has no speaker called {name}; it offers {}", offered.join(", "))]
    NoSuchSpeaker {
        /// The name that was asked for.
        name: String,
        /// The names the model does have, in id order.
        offered: Vec<String>,
    },

    /// Singing was asked for on a project that has no folder to keep the take in.
    ///
    /// The same shape as [`Self::RecordingNeedsFolder`], for the same reason: the remedy is a
    /// save *before* the thing the user wanted, not instead of it.
    #[error("a sung take needs a project folder to live in; save the project first")]
    SingingNeedsFolder,

    /// A singer track with no notes was asked to sing.
    #[error("track {0} has no notes to sing")]
    NothingToSing(u64),

    /// Composing from lyrics was asked with nothing to sing.
    ///
    /// The [`Self::NothingToAccompany`] reasoning: a melody written from no words would be a
    /// melody to nothing, and answering "done" to that is worse than saying so.
    #[error("the lyrics contain nothing to sing")]
    NoLyrics,

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

    /// Recording was asked for on a project that has no folder to write the take into.
    ///
    /// Separate from [`Self::NoPath`] because the remedy reads differently: a save is being asked
    /// for *before* the thing the user wanted, not instead of it.
    #[error("a recording needs a project folder to write into; save the project first")]
    RecordingNeedsFolder,

    /// Recording was asked for with neither an armed track nor an audio track selected.
    #[error("select an audio track to record onto, or arm one")]
    NothingToRecordOnto,

    /// Monitoring was asked for on more tracks at once than there are rings to carry them.
    ///
    /// A limit rather than a queue: every ring is made when the device opens, because the input
    /// callback may not allocate one while it is running.
    #[error("no more than {limit} tracks can be monitored at once")]
    TooManyMonitors {
        /// How many may be monitored at once.
        limit: usize,
    },

    /// A crossfade was asked for between two clips that do not overlap on one track.
    ///
    /// Nothing is moved to make them: a crossfade shapes a join somebody has already made by
    /// dragging one clip over another, and a command that dragged clips about on its own would be
    /// a command that undid an arrangement to fix a fade.
    #[error("those clips do not overlap on one track; drag one over the other first")]
    NotOverlapping,

    /// A second take was started while one was already running.
    #[error("a recording is already running")]
    AlreadyRecording,

    /// Stems were asked for from a project with no track that makes a sound of its own.
    ///
    /// Buses are not stems — what comes out of one is already in the stems of the tracks feeding
    /// it — so a project of nothing but buses has nothing to take apart.
    #[error("there is nothing to export as stems; add a track that makes a sound")]
    NothingToStem,

    /// Something was asked for that cannot be done while a take is being written.
    ///
    /// Separate from [`Self::AlreadyRecording`] because it is the other way round: that one
    /// refuses a *recording* because one is running, and this refuses everything else. Changing
    /// the audio device is the case it exists for — the take is stamped in the frames of the
    /// engine being replaced.
    #[error("stop the recording before changing the audio device")]
    RecordingInProgress,

    /// A take was stopped when none was running.
    #[error("no recording is running")]
    NotRecording,

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

impl SessionError {
    /// Whether this is a render that was stopped on purpose rather than one that broke.
    ///
    /// Asked here rather than matched at each frontend, because both of them have to get the
    /// same answer and only one of them is looked at every day. A cancellation reported in the
    /// colour of a failure sends somebody looking for what went wrong with an export they
    /// themselves stopped.
    pub fn is_cancellation(&self) -> bool {
        matches!(
            self,
            SessionError::Engine(auris_engine::EngineError::RenderCancelled)
                | SessionError::Sing(auris_singer::SingError::Cancelled)
        )
    }
}
