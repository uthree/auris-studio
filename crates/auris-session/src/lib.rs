//! The headless editing session — everything a frontend needs, and nothing a frontend is.
//!
//! [`Session`] owns the document, the plugin registry, the audio engine and the undo history,
//! and exposes one method per user-level command. It has no dependency on any UI toolkit, so
//! the same commands drive the gpui application, the command line tool, and anything else
//! (an MCP server, a script host) that arrives later.
//!
//! # Why this crate exists
//!
//! Keeping the *rendering* backend UI-free is easy; the part that usually leaks is the
//! orchestration around it — building the registry, rebuilding the render graph after an edit,
//! deciding which changes need a whole new graph and which fit in a command, tracking undo,
//! resolving a parameter to the plugin that owns it. That work lives here so a second frontend
//! reuses it instead of reimplementing it slightly differently.
//!
//! # Transactions
//!
//! Every mutator records its own undo step. A GUI drag would then push one step per pointer
//! move, so a gesture wraps itself in a transaction:
//!
//! ```no_run
//! # use auris_session::{Session, SessionOptions};
//! # use auris_core::time::Ticks;
//! # let mut session = Session::new(SessionOptions::headless())?;
//! # let clip = session.project().tracks.first().unwrap().id;
//! session.begin_transaction("Move clip");
//! // ... many mutator calls as the pointer moves ...
//! let changed = session.end_transaction();
//! # Ok::<(), auris_session::SessionError>(())
//! ```
//!
//! One undo step is recorded for the whole gesture, and none at all when the pointer never
//! moved — which is what stops a selection click from quietly pushing real history off the end
//! of the stack. The transaction also batches the graph rebuild: structural edits inside it
//! set a flag and [`Session::end_transaction`] rebuilds once.

#![warn(missing_docs)]

pub mod error;
pub mod history;
pub mod param;
pub mod registry;
pub mod render;
pub mod session;

pub use error::SessionError;
pub use history::History;
pub use param::ParamTarget;
pub use registry::default_registry;
pub use render::{ExportSummary, RenderJob};
pub use session::{AudioStatus, Session, SessionOptions};

/// File extension of a saved project.
pub use auris_io::PROJECT_EXTENSION;

/// Audio file extensions the importer accepts, for a file-picker filter.
pub fn supported_audio_extensions() -> &'static [&'static str] {
    auris_io::supported_extensions()
}

/// Re-exports of the backend types that appear in [`Session`]'s signatures, so a frontend can
/// depend on this crate alone.
pub mod prelude {
    pub use auris_core::param::{
        ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
    };
    pub use auris_core::plugin::{PluginCategory, PluginDescriptor, PluginKind};
    pub use auris_core::time::{Seconds, TICKS_PER_QUARTER, Ticks, TimeSignature};
    pub use auris_core::{
        AudioBuffer, AudioClip, AudioSource, ClipId, EffectSlotId, MidiClip, MixerStrip, Note,
        PluginRegistry, Project, SourceId, Track, TrackId, TrackKind,
    };
    pub use auris_engine::OfflineOptions;
    pub use auris_gpu::WaveformPeaks;
    pub use auris_io::{WavBitDepth, WavExportSettings};

    pub use crate::{ExportSummary, ParamTarget, RenderJob, Session, SessionError, SessionOptions};
}
