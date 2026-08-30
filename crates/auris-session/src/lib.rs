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
pub mod library;
pub mod param;
pub mod progressions;
pub mod registry;
pub mod render;
pub mod session;
pub mod settings;

pub use error::SessionError;
pub use history::{Edit, History};
pub use library::{GENERAL_MIDI, LIBRARY_DIR_VAR, LIBRARY_FOLDER, ShippedFont};
pub use param::ParamTarget;
pub use registry::{DEFAULT_INSTRUMENT, default_registry, plugin_catalogue};
pub use render::{ExportSummary, RenderJob, StemSummary, stem_tracks};
pub use session::{
    AccompanyReport, Arm, AudioStatus, BalanceReport, CEILING_DB, Clipboard, ComposeReport,
    CopiedClip, CopiedContent, DEFAULT_OCTAVE, DEFAULT_PARTS, DEFAULT_VELOCITY, InputChannels,
    LAYOUT, LIMITER_ALLOWANCE_DB, LoadedFont, MixAnalysis, MusicalTyping, OCTAVE_RANGE, Played,
    PluginWindow, Quantize, RecordingReport, RecordingStatus, Release, SaveReport, SectionLoudness,
    Session, SessionOptions, Struck, TARGET_LUFS, TYPING_BEND, TakeReport, TrackLevel,
    TrackLoudness, TypingRole, VELOCITY_STEP, WHEEL_STEPS, decode_audio, fader_for, faders_lift_db,
    input_level_of, master_gain_db, quantized, read_soundfont, shadows_musical_typing,
};
pub use settings::{
    AgentPreferences, AudioPreferences, CONFIG_DIR_VAR, ExportPreferences, Settings,
    WindowPlacement, config_dir, migrate_legacy_config,
};

/// What a `.clap` file says is inside it, for a frontend listing one.
///
/// Re-exported so a frontend can name the type without depending on [`auris_clap`], which it may
/// not do — a frontend depends on this crate and its own toolkit and nothing else.
pub use auris_clap::ClapPluginInfo;

/// The platform's own handle for a window, and the trait a toolkit hands one out through.
///
/// Re-exported for the same reason: a frontend naming the window its plugin windows should float
/// above must be able to say what kind of thing that is. This is not a UI toolkit — it is the
/// vocabulary two of them use to talk about the same window.
pub use auris_clap::{HasWindowHandle, RawWindowHandle};

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

/// Standard MIDI File extensions, for a file-picker filter.
pub fn midi_extensions() -> &'static [&'static str] {
    auris_io::midi_extensions()
}

/// Re-exports of the backend types that appear in [`Session`]'s signatures, so a frontend can
/// depend on this crate alone.
///
/// [`Key`](auris_core::theory::key::Key) is re-exported as `MusicalKey`, because a frontend that
/// glob-imports this module almost certainly also imports `auris_i18n::Key` for its interface
/// text. Both names would compile — an explicit `use` beats a glob — but a reader would have to
/// know that rule to tell which `Key` a line means, and one of the two would be wrong silently.
pub mod prelude {
    /// General MIDI: the programs a part can ask for, and the kits a drum part can.
    ///
    /// A whole module rather than the type alone, because a picker needs the name table beside
    /// it — a frontend cannot list what it has no list of.
    pub use auris_compose::gm;
    /// Reads a motif field — `"0 2 4 2"` — the way a specification does, so a prompt that
    /// takes one refuses exactly what the file would refuse.
    pub use auris_compose::spec::parse_motif;
    pub use auris_compose::{
        Composition, Ending, Mood, PRESETS, PartSpec, Role, SectionSpec, SongPreset, SongSpec,
        SpecError, compose, default_instrument, motif_of, preset,
    };
    pub use auris_core::automation::{
        Automation, AutomationCurve, AutomationLane, AutomationPoint,
    };
    pub use auris_core::harmony::{ChordMap, ChordPoint, Harmony, KeyMap, KeyPoint};
    pub use auris_core::param::{
        ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
    };
    pub use auris_core::plugin::{PluginCategory, PluginDescriptor, PluginKind};
    pub use auris_core::theory::chart::{
        CatalogEntry, Chart, ChartMode, ChartOrigin, HarmonicEvent,
    };
    pub use auris_core::theory::chord::{Chord, Quality};
    pub use auris_core::theory::key::Key as MusicalKey;
    pub use auris_core::theory::numeral::Numeral;
    pub use auris_core::theory::pitch::PitchClass;
    pub use auris_core::theory::scale::ScaleId;
    pub use auris_core::time::{
        Seconds, SignatureMap, SignaturePoint, SignatureSpan, TICKS_PER_QUARTER, TempoMap,
        TempoPoint, Ticks, TimeSignature,
    };
    pub use auris_core::{
        AudioBuffer, AudioClip, AudioSource, AuxSend, ClipId, ClipPreset, ClipRecipe, Color,
        EffectSlotId, FadeCurve, MidiClip, MixerStrip, Note, NoteTransform, Output, PluginRegistry,
        PresetRef, Project, SectionMap, SectionPoint, SectionSpan, SendId, SingerTrack,
        SoundFontId, SoundFontRef, SourceId, Subdivision, Track, TrackId, TrackKind,
        default_frame_hop, default_loop_end, loop_passes, sounding_length,
    };
    /// The equalizer's band table, the settings a display reads out of one, and the curve those
    /// settings make.
    ///
    /// A frontend that drew the response itself would be a second implementation of the cookbook
    /// filters, and the two would agree until the day one of them was corrected — so the graph on
    /// screen comes from the same crate the audio does. [`EQUALIZER_ID`] is here for the same
    /// reason: an editor has to recognise the one effect that has a curve.
    pub use auris_dsp::{
        EQ_BAND_COUNT, EQ_LAYOUT, EQUALIZER_ID, EqBandKind, EqBandLayout, EqBandSetting,
        eq_response_db,
    };
    /// What a singer track stores and what its voice model is fed — see [`auris_vocal`] and
    /// the singer commands on [`Session`].
    pub use auris_vocal::{SILENCE, SingerFrames, VocalError, split_kana_lyric, split_kana_moras};
    // The curves a clip carries, and how far each may go. A frontend drawing one has to know the
    // range it is drawing against, and may not reach past this crate to find out.
    pub use auris_core::plugin::{CC_MODULATION, CONTROLLER_MAX};
    pub use auris_core::project::{BEND_LIMIT, CONTROLLER_LIMIT, ClipCurve, CurvePoint};

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
    /// `MeterBank` is here for its ballistics rather than for itself: a frontend reaches the bank
    /// through [`Session::meters`](crate::Session::meters) without naming it, but a meter it fills
    /// from somewhere else — the input peak, which is handed over once and forgotten — has to fall
    /// at the same rate as the ones beside it or it reads as a different instrument.
    pub use auris_engine::{AudioDeviceInfo, MeterBank, OfflineOptions, RenderProgress};
    pub use auris_gpu::WaveformPeaks;
    pub use auris_io::{SoundFontPreset, WavBitDepth, WavExportSettings};
    pub use auris_sampler::{SAMPLER_ENVELOPE_KEY, SAMPLER_ID};

    pub use crate::{
        AccompanyReport, Arm, AudioPreferences, Clipboard, ComposeReport, CopiedClip,
        CopiedContent, DEFAULT_PARTS, Edit, ExportPreferences, ExportSummary, InputChannels,
        LoadedFont, MusicalTyping, ParamTarget, Quantize, RecordingReport, RecordingStatus,
        RenderJob, SaveReport, Session, SessionError, SessionOptions, Settings, StemSummary,
        TakeReport, decode_audio, input_level_of, read_soundfont,
    };
}
