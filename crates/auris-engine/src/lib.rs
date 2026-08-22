#![warn(missing_docs)]

//! Realtime and offline rendering for Auris Studio.
//!
//! # How the pieces fit
//!
//! The UI owns the [`Project`](auris_core::project::Project) — the editable document. The audio
//! thread owns a [`RenderGraph`], which is that document flattened into something that can be
//! rendered without a single allocation: live plugin instances, note events already converted to
//! absolute sample positions, and every scratch buffer pre-sized.
//!
//! ```text
//!        UI thread                                    audio callback
//!   ┌──────────────────┐   EngineCommand::SetGraph   ┌────────────────────┐
//!   │ Project (edited) │ ──────────────────────────► │ RenderGraph        │
//!   │ RenderGraph::build                             │ Transport          │
//!   │                  │ ◄────────────────────────── │ render_block()     │
//!   └──────────────────┘   retired graph (dropped)   └────────────────────┘
//!         collect_garbage()                                 meters ▲
//!                                                        playhead ▲
//! ```
//!
//! A structural edit rebuilds the graph off the audio thread and sends it down a bounded queue;
//! the audio thread swaps it in and pushes the old one back so its destructor — which frees
//! plugins and sample buffers — runs on the UI thread. Small changes such as a fader move travel
//! as cheap [`EngineCommand`]s instead.
//!
//! # Rendering
//!
//! [`render_block`] is the single implementation of "produce N frames". Realtime playback calls
//! it from the device callback; [`render_project`] calls it in a loop as fast as the CPU allows.
//! Because there is only one path, an export matches what was heard, sample for sample.
//!
//! # Audio going the other way
//!
//! [`start_capture`] opens an input device and hands its blocks to whatever is writing them down.
//! It is a *second* stream on a *second* clock — cpal offers nothing else — so [`capture`] is
//! where the consequences of that are set out: where a take lands on the timeline, and what the
//! two crystals drifting apart does and does not cost.
//!
//! Those blocks have a second reader. [`monitor`] is the ring that carries them back to the output
//! callback so the player hears themselves through the mix, and it answers the two-clock problem
//! the other way round from [`capture`]: what is being listened to and not kept may be re-seated
//! when the clocks drift, and what is being kept may not.
//!
//! # Realtime rules
//!
//! Everything reachable from [`render_block`] is allocation-free, lock-free and panic-free, and
//! so is everything the capture callback touches. Allocation happens in [`RenderGraph::build`]
//! and in the capture's buffer pool; levels reach the UI through the lock-free [`MeterBank`]; the
//! playhead is a single relaxed atomic.
//!
//! # Example
//!
//! ```no_run
//! use auris_core::project::{AudioSourceBank, Project};
//! use auris_core::registry::PluginRegistry;
//! use auris_engine::{AudioSettings, EngineCommand, RenderGraph, start_audio};
//!
//! let project = Project::default();
//! let bank = AudioSourceBank::new();
//! let registry = PluginRegistry::new();
//!
//! let (_device, engine) = start_audio(&AudioSettings::default())?;
//! let graph = RenderGraph::build_at(
//!     &project,
//!     &bank,
//!     &registry,
//!     engine.max_block(),
//!     engine.sample_rate(),
//! );
//! engine.set_graph(graph)?;
//! engine.send(EngineCommand::Play)?;
//! # Ok::<(), auris_engine::EngineError>(())
//! ```

pub mod capture;
pub mod command;
pub mod device;
pub mod error;
pub mod graph;
pub mod handle;
pub mod meter;
pub mod metronome;
pub mod monitor;
pub mod offline;
pub mod renderer;
pub mod scope;
pub mod transport;

#[cfg(test)]
mod testkit;

pub use capture::{
    Capture, CaptureReader, CaptureSettings, MAX_METERED_CHANNELS, input_devices, start_capture,
};
pub use command::EngineCommand;
pub use device::{
    AudioDevice, AudioDeviceInfo, AudioSettings, output_devices, start_audio, start_silent,
};
pub use error::EngineError;
pub use graph::{
    PlacedEffects, PlacedInstruments, RENDER_CHANNELS, RenderAudioClip, RenderGraph, RenderSource,
    RenderStrip, RenderTrack, ScheduledEvent, SmoothedGain,
};
pub use handle::EngineHandle;
pub use meter::MeterBank;
pub use metronome::{Click, Metronome};
pub use monitor::MonitorRing;
pub use offline::{
    OfflineOptions, OfflineRender, RenderProgress, render_project, render_project_using,
    render_project_with_progress,
};
pub use renderer::render_block;
pub use scope::{SCOPE_WINDOW, Scope, ScopeSource};
pub use transport::{CountIn, Transport};
