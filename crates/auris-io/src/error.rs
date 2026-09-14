//! Errors produced by file import, export and project persistence.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// The bounded class of decoded MIDI objects that exhausted its import budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiImportResource {
    /// Events decoded from one source track.
    TrackEvents,
    /// Events decoded across every source track.
    FileEvents,
    /// Notes retained by the import.
    Notes,
    /// Pitch-bend and controller points retained by the import.
    AutomationPoints,
    /// Notes, automation, tempo and meter events retained together.
    OutputEvents,
}

impl std::fmt::Display for MidiImportResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TrackEvents => "events in one MIDI track",
            Self::FileEvents => "events across the MIDI file",
            Self::Notes => "MIDI notes",
            Self::AutomationPoints => "MIDI automation points",
            Self::OutputEvents => "retained MIDI events",
        })
    }
}

/// The bounded class of expanded MIDI data that exhausted its export budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiExportResource {
    /// Performed notes in one exported instrument track.
    TrackNotes,
    /// Performed notes across every exported instrument track.
    FileNotes,
    /// Sampled bend and controller events in one exported instrument track.
    TrackCurveEvents,
    /// Sampled bend and controller events across the exported file.
    FileCurveEvents,
    /// Wire events, including note-off and metadata, in one exported track.
    TrackEvents,
    /// Wire events across the complete exported file.
    FileEvents,
}

impl std::fmt::Display for MidiExportResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TrackNotes => "performed notes in one MIDI track",
            Self::FileNotes => "performed notes across the MIDI file",
            Self::TrackCurveEvents => "curve events in one MIDI track",
            Self::FileCurveEvents => "curve events across the MIDI file",
            Self::TrackEvents => "events in one MIDI track",
            Self::FileEvents => "events across the MIDI file",
        })
    }
}

/// Everything that can go wrong while reading or writing files.
///
/// Every variant carries enough context to be shown to the user directly: the failing path, or
/// the message the underlying library produced. `auris-io` never logs-and-swallows a failure,
/// because a silently truncated import or export is worse than a dialog.
#[derive(Debug, Error)]
pub enum IoError {
    /// The requested file does not exist, or is not readable.
    #[error("file not found: {0}")]
    FileNotFound(PathBuf),

    /// The container or codec is not one Symphonia was built to handle.
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),

    /// The file was recognised but its audio data could not be decoded.
    #[error("failed to decode audio: {0}")]
    Decode(String),

    /// Sample rate conversion failed, or could not be set up for the requested ratio.
    #[error("failed to resample audio: {0}")]
    Resample(String),

    /// Writing a WAV file failed.
    #[error("failed to write WAV file: {0}")]
    WavWrite(String),

    /// Writing a FLAC file failed.
    #[error("failed to write FLAC file: {0}")]
    FlacWrite(String),

    /// Writing an MP3 file failed.
    #[error("failed to write MP3 file: {0}")]
    Mp3Write(String),

    /// An audio export was stopped by its caller.
    #[error("audio export cancelled")]
    ExportCancelled,

    /// A no-replace export found that another writer had already claimed its destination.
    #[error("audio export destination already exists: {}", .0.display())]
    ExportDestinationExists(PathBuf),

    /// A file was offered as a Standard MIDI File and would not parse as one.
    #[error("failed to read MIDI file: {0}")]
    MidiParse(String),

    /// A Standard MIDI File exceeds the bounded in-memory importer size.
    #[error(
        "MIDI file is too large to import: {} is at least {observed} bytes; the limit is {limit} bytes",
        path.display()
    )]
    MidiFileTooLarge {
        /// File that was offered for import.
        path: PathBuf,
        /// Size observed either from the open handle or by the bounded read.
        observed: u64,
        /// Largest file this importer will hold in memory.
        limit: u64,
    },

    /// An in-memory Standard MIDI File byte slice exceeds the importer size bound.
    #[error(
        "MIDI data is too large to import: {observed} bytes were supplied; the limit is {limit} bytes"
    )]
    MidiDataTooLarge {
        /// Size of the supplied byte slice.
        observed: u64,
        /// Largest byte slice this importer will parse.
        limit: u64,
    },

    /// A parsed MIDI file expands into more events than the importer can retain safely.
    #[error("MIDI import contains too many {resource}: at least {observed}; the limit is {limit}")]
    MidiImportTooLarge {
        /// Kind of decoded object whose budget was exhausted.
        resource: MidiImportResource,
        /// First count known to exceed the budget.
        observed: u64,
        /// Largest count this importer will process or retain.
        limit: u64,
    },

    /// A project would expand into more Standard MIDI File events than can be held safely.
    #[error("MIDI export contains too many {resource}: at least {observed}; the limit is {limit}")]
    MidiExportTooLarge {
        /// Kind of expanded object whose budget was exhausted.
        resource: MidiExportResource,
        /// Conservative count that exceeded the budget.
        observed: u64,
        /// Largest count this exporter will process or retain.
        limit: u64,
    },

    /// A project contains a MIDI value the Standard MIDI File representation cannot hold.
    #[error("failed to write MIDI file: {0}")]
    MidiWrite(String),

    /// The file counts time in SMPTE frames rather than in beats.
    ///
    /// Not a defect in the file — it is a legal division — but a different kind of thing. Frames
    /// of real time have no beats, so they have no bars, and putting one on a musical timeline
    /// would mean choosing a tempo on the file's behalf.
    #[error(
        "this MIDI file counts time in SMPTE frames ({fps} fps, {subframe} subframes) rather \
         than in beats, so it has no musical positions to import"
    )]
    MidiTimecode {
        /// Frames per second the file counts in.
        fps: f32,
        /// Subdivisions of each frame.
        subframe: u8,
    },

    /// A SoundFont exceeds the bounded in-memory importer size.
    #[error(
        "SoundFont is too large to import: {} is at least {observed} bytes; the limit is {limit} bytes",
        path.display()
    )]
    SoundFontFileTooLarge {
        /// File that was offered for import.
        path: PathBuf,
        /// Size observed either from the open handle or by the bounded read.
        observed: u64,
        /// Largest file this importer will hold in memory.
        limit: u64,
    },

    /// A project document exceeds the bounded in-memory JSON reader size.
    #[error(
        "project file is too large to open: {} is at least {observed} bytes; the limit is {limit} bytes",
        path.display()
    )]
    ProjectFileTooLarge {
        /// File that was offered as an Auris project.
        path: PathBuf,
        /// Size observed either from the open handle or by the bounded read.
        observed: u64,
        /// Largest project document this reader will hold in memory.
        limit: u64,
    },

    /// A project file could not be parsed as JSON, or a project could not be serialised.
    #[error("project JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The project file uses a different schema version.
    #[error(
        "project format version {found} does not match the supported version {supported}; \
         open this project with a build supporting its format"
    )]
    ProjectVersionMismatch {
        /// Version recorded in the file.
        found: u32,
        /// Format version this build understands.
        supported: u32,
    },

    /// The document contains the largest possible object id, leaving no id for future edits.
    #[error("project object ids have exhausted their supported range")]
    ProjectIdsExhausted,

    /// Two document objects claim the same id, or a keyed object disagrees with its map key.
    #[error("project object id {0} is duplicated or inconsistent")]
    ProjectIdConflict(u64),

    /// The decoded document violates a bounded core-model invariant.
    #[error(transparent)]
    Core(#[from] auris_core::CoreError),

    /// Any other filesystem failure, such as a permission or disk-full error.
    #[error("I/O error on {path}: {source}")]
    Filesystem {
        /// File the operation was working on.
        path: PathBuf,
        /// Underlying operating system error.
        source: std::io::Error,
    },
}

impl IoError {
    /// Classifies a filesystem error against the path it happened on.
    ///
    /// A missing file is by far the most common failure and deserves its own message, so it is
    /// split out from the generic case here rather than at every call site.
    pub fn from_fs(path: &Path, source: std::io::Error) -> Self {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::FileNotFound(path.to_path_buf())
        } else {
            IoError::Filesystem {
                path: path.to_path_buf(),
                source,
            }
        }
    }
}

/// Result alias used throughout this crate.
pub type Result<T, E = IoError> = std::result::Result<T, E>;
