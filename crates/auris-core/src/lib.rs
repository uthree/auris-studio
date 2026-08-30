//! Core types shared by every part of Auris Studio.
//!
//! This crate deliberately has no audio-backend, GPU or UI dependencies. It defines:
//!
//! * [`buffer::AudioBuffer`] — the planar sample container passed through the whole engine.
//! * [`time`] — musical/wall-clock time conversions driven by a [`time::TempoMap`].
//! * [`param`] — a plugin parameter description model.
//! * [`plugin`] — the [`plugin::Instrument`] and [`plugin::Effect`] traits that make the
//!   application extensible with new sound sources and processors.
//! * [`registry::PluginRegistry`] — the runtime lookup table those plugins register into.
//! * [`project`] — the serialisable document model (tracks, clips, notes, mixer state).
//! * [`asset`] — how that document refers to the files it is too small to contain.
//! * [`theory`] — keys, scales, chords and roman numerals: music as it would be true without a
//!   computer.
//! * [`harmony`] — that theory laid out over the timeline, the way [`time::TempoMap`] lays out
//!   tempo.
//! * [`structure`] — the song's sections laid out the same way: イントロ, Aメロ, サビ, and where
//!   each begins.
//!
//! The last three are here rather than in the composer because the document holds a key, a chord
//! progression and a structure of its own, and the document model may not depend on anything
//! above it.

#![warn(missing_docs)]

pub mod asset;
pub mod automation;
pub mod buffer;
pub mod error;
pub mod harmony;
pub mod param;
pub mod plugin;
pub mod project;
pub mod registry;
pub mod rng;
pub mod structure;
pub mod theory;
pub mod time;

pub use asset::AssetPath;
pub use automation::{Automation, AutomationCurve, AutomationLane, AutomationPoint};
pub use buffer::AudioBuffer;
pub use error::{CoreError, Result};
pub use harmony::{ChordMap, ChordPoint, Harmony, KeyMap, KeyPoint};
pub use param::{ParamDescriptor, ParamId, ParamTarget, ParamUnit, ParamValueCurve};
pub use plugin::{
    CC_EXPRESSION, CC_MODULATION, CONTROLLER_MAX, Effect, Instrument, NoteEvent, Parameterized,
    PluginCategory, PluginDescriptor, PluginKind, PluginState, PrepareContext, ProcessContext,
};
pub use project::{
    AudioClip, AudioSource, AudioSourceBank, AudioTrack, AuxSend, ClipId, ClipPreset, ClipRecipe,
    Color, EffectSlot, EffectSlotId, FadeCurve, InstrumentTrack, MidiClip, MixerStrip, Note,
    NoteTransform, Output, PresetRef, Project, SendId, SingerTake, SingerTrack, SoundFontId,
    SoundFontRef, SourceId, Subdivision, Track, TrackId, TrackKind, default_frame_hop,
    default_loop_end, loop_passes, notes_digest, notes_trimmed_from_front, performed,
    sounding_length,
};
pub use registry::{PluginPack, PluginRegistry};
pub use structure::{SectionMap, SectionPoint, SectionSpan};
pub use time::{
    Beats, Samples, Seconds, SignatureMap, SignaturePoint, SignatureSpan, TICKS_PER_QUARTER,
    TempoMap, Ticks, TimeSignature,
};

/// Convenience import for code that implements plugins.
pub mod prelude {
    pub use crate::buffer::AudioBuffer;
    pub use crate::param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve};
    pub use crate::plugin::{
        CC_EXPRESSION, CC_MODULATION, CONTROLLER_MAX, Effect, Instrument, NoteEvent, Parameterized,
        PluginCategory, PluginDescriptor, PluginKind, PluginState, PrepareContext, ProcessContext,
    };
    pub use crate::registry::PluginRegistry;
}
