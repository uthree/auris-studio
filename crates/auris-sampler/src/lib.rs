//! SoundFont playback for Auris Studio.
//!
//! A SoundFont is a bank of recorded samples together with the regions, envelopes and filters
//! that turn them back into an instrument. Reading one is `auris-io`'s job; playing one is this
//! crate's, and the two meet at an `Arc<SoundFont>` — which is why the library that parses and
//! renders them is named once, in the workspace manifest, rather than twice.
//!
//! Two pieces:
//!
//! * [`SoundFontBank`] — every font this session has loaded, by the id the project names it
//!   with. The document stores paths; this stores what those paths decoded to.
//! * [`Sampler`] — an [`Instrument`](auris_core::Instrument) that plays one preset of one font.
//!
//! # Why a bank at all
//!
//! A plugin registry builds instruments from factories that take no arguments, and a
//! [`PluginState`](auris_core::PluginState) is a map of `f32`. Neither can carry two hundred
//! megabytes of samples. So the sampler is registered by [`register_sampler`], whose closure
//! captures the bank, and a track names the sound it wants by id — the same shape
//! [`AudioSourceBank`](auris_core::AudioSourceBank) uses to keep decoded audio out of the
//! document.
//!
//! ```
//! use auris_core::{Instrument, PrepareContext, registry::PluginRegistry};
//! use auris_sampler::{SAMPLER_ID, SoundFontBank, register_sampler};
//!
//! let fonts = SoundFontBank::shared();
//! let mut registry = PluginRegistry::new();
//! register_sampler(&mut registry, fonts.clone());
//!
//! // No fonts are loaded, so the sampler builds and prepares — and renders silence.
//! let mut sampler = registry.create_instrument(SAMPLER_ID).expect("registered");
//! sampler.prepare(&PrepareContext::new(48_000.0, 512, 2));
//! assert_eq!(sampler.active_voices(), 0);
//! ```
//!
//! # Velocity, and a deliberate disagreement with the format
//!
//! A SoundFont synthesiser makes amplitude proportional to the *square* of MIDI velocity, which
//! is what the format says and what every other player does. Auris is linear everywhere else —
//! a built-in instrument multiplies its voice by the velocity — so a part written for one came
//! out of the other with twice its dynamic range in decibels. [`Sampler`] pre-compensates for
//! the squaring rather than living with two meanings for one word, and the price is that a
//! velocity written here will not sound the same in another player. That trade is taken
//! knowingly; see [`Sampler`] for what it costs in layer switching, and for what a chord peaks
//! at now that the sampler is calibrated against the built-in instruments rather than against
//! the library's idea of unity.
//!
//! # Realtime behaviour
//!
//! Everything expensive happens in [`Instrument::prepare`](auris_core::Instrument::prepare):
//! resolving the font, building the synthesiser, sizing its buffers. `process` renders through
//! them and never locks the bank, allocates or touches a file. The velocity correction is one
//! square root per note-on, which is arithmetic and costs nothing per sample.

#![warn(missing_docs)]

pub mod bank;
pub mod sampler;

#[cfg(test)]
mod test_support;

pub use bank::{SharedSoundFonts, SoundFontBank};
pub use sampler::{
    ENVELOPE_KEY as SAMPLER_ENVELOPE_KEY, SAMPLER_ID, Sampler, register_sampler, store_preset,
    stored_preset,
};
