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
//! * [`ClapEffect`] or [`ClapInstrument`] — the audio-thread half. It implements
//!   [`Effect`](auris_core::plugin::Effect) or [`Instrument`](auris_core::plugin::Instrument) and
//!   goes into the graph like any built-in one.
//!
//! CLAP itself has only one kind of plugin; effect and instrument are habits, not types. Which
//! half [`ClapPlugin`] hands out is therefore the caller's choice, and the plugin's declared
//! features — read into [`ClapPluginInfo::kind`] — are what the caller should choose by.
//!
//! Neither `auris-core` nor `auris-engine` knows this crate exists. The engine drives a hosted
//! plugin through exactly the same trait it drives a biquad through.
//!
//! # What is *not* here
//!
//! * **No `PluginRegistry` entry.** A registry factory is `Fn() -> Box<dyn Effect>`, which cannot
//!   produce the main-thread half a CLAP plugin also needs. A hosted plugin is placed by the
//!   session, not built by the registry.
//! * **No sidechain.** Every audio port a plugin declares is handed to it, but only the main one
//!   carries anything; nothing in Auris can route a second track into a plugin yet.
//! * **No *embedded* window.** A plugin's own interface opens in a window of its own, floating
//!   above the application, and never inside a panel of it. [`gui`] gives the account.
//!
//! # Safety
//!
//! Loading a `.clap` runs third-party machine code in this process. [`ClapLibrary::load`] is
//! `unsafe` and honest about it: nothing this crate does can stop a bad plugin from corrupting
//! memory or crashing the application. The realtime rules in
//! [`auris_core::plugin`] cannot be enforced on a binary either — a hosted plugin may well
//! allocate on the audio thread, and the only remedy is not to load it.

#![warn(missing_docs)]

mod bridge;
mod effect;
mod error;
pub mod gui;
mod host;
mod instrument;
mod library;
pub mod notes;
mod plugin;
mod ports;
pub mod timers;

#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

#[cfg(test)]
mod tests;

pub use effect::ClapEffect;
pub use error::ClapError;
pub use gui::{window_plan, window_title};
pub use instrument::ClapInstrument;
pub use library::{ClapLibrary, ClapPluginInfo, classify};
pub use notes::{NoteLanguage, language_for};
pub use plugin::{ClapPlugin, PendingRequests};
pub use ports::{PortLayout, main_port};
// Re-exported so a frontend naming a parent window and a session passing one along agree on the
// type without either taking its own dependency on the crate that defines it.
pub use raw_window_handle::{HasWindowHandle, RawWindowHandle};
