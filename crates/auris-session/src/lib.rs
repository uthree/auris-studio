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
//! # use auris_session::{Edit, Session, SessionOptions};
//! # use auris_core::time::Ticks;
//! # let mut session = Session::new(SessionOptions::headless())?;
//! # let clip = session.project().tracks.first().unwrap().id;
//! session.begin_transaction(Edit::MoveClip);
//! // ... many mutator calls as the pointer moves ...
//! let changed = session.end_transaction();
//! # Ok::<(), auris_session::SessionError>(())
//! ```
//!
//! One undo step is recorded for the whole gesture, and none at all when the pointer never
//! moved — which is what stops a selection click from quietly pushing real history off the end
//! of the stack. The transaction also batches the graph rebuild: structural edits inside it
//! set a flag and [`Session::end_transaction`] rebuilds once.
//!
//! # Where everything else is
//!
//! The workspace has no root crate, so [`guide`] carries the account of how the twelve of them
//! fit together — it lives here because this is the only crate that depends on every other, and
//! so the only one whose links to them all resolve. Start at [`guide::architecture`].

#![warn(missing_docs)]

pub mod error;
pub mod guide;
pub mod history;
pub mod param;
pub mod registry;
pub mod render;
pub mod session;
pub mod settings;

pub use error::SessionError;
pub use history::{Edit, History};
pub use param::ParamTarget;
pub use registry::{DEFAULT_INSTRUMENT, default_registry, plugin_catalogue};
pub use render::{ExportSummary, RenderJob};
pub use session::{AudioStatus, ComposeReport, SaveReport, Session, SessionOptions};
pub use settings::{AudioPreferences, CONFIG_DIR_VAR, Settings, config_dir, migrate_legacy_config};

/// File extension of a saved project.
pub use auris_io::PROJECT_EXTENSION;

/// File extension of a song specification.
pub use auris_compose::SPEC_EXTENSION;

/// Audio file extensions the importer accepts, for a file-picker filter.
pub fn supported_audio_extensions() -> &'static [&'static str] {
    auris_io::supported_extensions()
}

/// SoundFont extensions the importer accepts, for a file-picker filter.
pub fn supported_soundfont_extensions() -> &'static [&'static str] {
    auris_io::soundfont_extensions()
}

/// Re-exports of the backend types that appear in [`Session`]'s signatures, so a frontend can
/// depend on this crate alone.
///
/// [`Key`](auris_core::theory::key::Key) is re-exported as `MusicalKey`, because a frontend that
/// glob-imports this module almost certainly also imports `auris_i18n::Key` for its interface
/// text. Both names would compile — an explicit `use` beats a glob — but a reader would have to
/// know that rule to tell which `Key` a line means, and one of the two would be wrong silently.
pub mod prelude {
    pub use auris_compose::{Composition, SongSpec, SpecError, compose, default_instrument};
    pub use auris_core::harmony::{ChordMap, ChordPoint, Harmony, KeyMap, KeyPoint};
    pub use auris_core::param::{
        ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
    };
    pub use auris_core::plugin::{PluginCategory, PluginDescriptor, PluginKind};
    pub use auris_core::theory::chart::{CatalogEntry, Chart, HarmonicEvent};
    pub use auris_core::theory::chord::{Chord, Quality};
    pub use auris_core::theory::key::Key as MusicalKey;
    pub use auris_core::theory::numeral::Numeral;
    pub use auris_core::theory::pitch::PitchClass;
    pub use auris_core::theory::scale::ScaleId;
    pub use auris_core::time::{Seconds, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignature};
    pub use auris_core::{
        AudioBuffer, AudioClip, AudioSource, ClipId, ClipPreset, ClipRecipe, EffectSlotId,
        MidiClip, MixerStrip, Note, PluginRegistry, PresetRef, Project, SoundFontId, SoundFontRef,
        SourceId, Subdivision, Track, TrackId, TrackKind,
    };

    /// Every chord progression the composer knows by name.
    pub fn progression_catalog() -> &'static [CatalogEntry] {
        auris_core::theory::chart::CATALOG
    }

    pub use auris_compose::rhythm::Groove;

    /// Every drum groove the composer knows by name.
    ///
    /// A frontend needs this for the same reason it needs [`progression_catalog`]: a
    /// [`ClipRecipe`]'s groove is a name, and a picker cannot offer names it has no list of.
    pub fn groove_catalog() -> &'static [Groove] {
        auris_compose::rhythm::GROOVES
    }
    pub use auris_engine::{OfflineOptions, OutputDeviceInfo};
    pub use auris_gpu::WaveformPeaks;
    pub use auris_io::{SoundFontPreset, WavBitDepth, WavExportSettings};
    pub use auris_sampler::SAMPLER_ID;

    pub use crate::{
        AudioPreferences, ComposeReport, Edit, ExportSummary, ParamTarget, RenderJob, SaveReport,
        Session, SessionError, SessionOptions, Settings,
    };
}
