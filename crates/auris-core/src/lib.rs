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

#![warn(missing_docs)]

pub mod asset;
pub mod buffer;
pub mod error;
pub mod param;
pub mod plugin;
pub mod project;
pub mod registry;
pub mod time;

pub use asset::AssetPath;
pub use buffer::AudioBuffer;
pub use error::{CoreError, Result};
pub use param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve};
pub use plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginKind,
    PluginState, PrepareContext, ProcessContext,
};
pub use project::{
    AudioClip, AudioSource, AudioSourceBank, AudioTrack, ClipId, Color, EffectSlot, EffectSlotId,
    InstrumentTrack, MidiClip, MixerStrip, Note, PresetRef, Project, SoundFontId, SoundFontRef,
    SourceId, Track, TrackId, TrackKind,
};
pub use registry::{PluginPack, PluginRegistry};
pub use time::{Beats, Samples, Seconds, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignature};

/// Convenience import for code that implements plugins.
pub mod prelude {
    pub use crate::buffer::AudioBuffer;
    pub use crate::param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve};
    pub use crate::plugin::{
        Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginKind,
        PluginState, PrepareContext, ProcessContext,
    };
    pub use crate::registry::PluginRegistry;
}
