//! Hosting third-party [CLAP](https://cleveraudio.org) plugins.
//!
//! # Why this is a crate of its own
//!
//! CLAP mandates a split between a *main thread*, which owns the plugin and answers questions
//! about it, and an *audio thread*, which does nothing but render. `clack_host` enforces that
//! split in the type system: a [`PluginInstance`](clack_host::prelude::PluginInstance) is not
//! [`Send`], while the audio processor it hands out is.
//!
//! Auris Studio has the same two threads and the same split, so the two models fit — but they
//! fit at a seam that runs *between* existing crates. The render graph in `auris-engine` holds
//! `Box<dyn Effect>` and must stay `Send`; the thing that can answer "what are your parameters"
//! cannot travel there. So a hosted plugin is two objects:
//!
//! * [`ClapPlugin`] — the main-thread half. The session owns it, asks it for parameters and
//!   state, and it is the only thing that may talk to the plugin about anything but audio.
//! * [`ClapEffect`] — the audio-thread half. It implements [`Effect`](auris_core::plugin::Effect)
//!   and goes into the graph like any built-in effect.
//!
//! Neither `auris-core` nor `auris-engine` knows this crate exists. The engine drives a hosted
//! plugin through exactly the same trait it drives a biquad through.
//!
//! # What is *not* here
//!
//! * **No `PluginRegistry` entry.** A registry factory is `Fn() -> Box<dyn Effect>`, which cannot
//!   produce the main-thread half a CLAP plugin also needs. A hosted plugin is placed by the
//!   session, not built by the registry.
//! * **No instrument.** [`ClapPlugin::activate`] produces an effect. Note input needs the note
//!   ports extension and a translation from [`NoteEvent`](auris_core::plugin::NoteEvent), which
//!   is a separate piece of work.
//! * **No GUI.** A hosted plugin is edited through the generic parameter panel.
//!
//! # Safety
//!
//! Loading a `.clap` runs third-party machine code in this process. [`ClapLibrary::load`] is
//! `unsafe` and honest about it: nothing this crate does can stop a bad plugin from corrupting
//! memory or crashing the application. The realtime rules in
//! [`auris_core::plugin`] cannot be enforced on a binary either — a hosted plugin may well
//! allocate on the audio thread, and the only remedy is not to load it.

#![warn(missing_docs)]

mod effect;
mod error;
mod host;
mod library;
mod plugin;
mod ports;

#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

#[cfg(test)]
mod tests;

pub use effect::ClapEffect;
pub use error::ClapError;
pub use library::{ClapLibrary, ClapPluginInfo, classify};
pub use plugin::{ClapPlugin, PendingRequests};
pub use ports::{PortLayout, main_port};
