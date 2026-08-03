//! How Auris Studio fits together.
//!
//! The workspace has no root crate — `cargo doc` on it produces a list of eleven crates and no
//! account of how they relate. This module is that account. It lives here because `auris-session`
//! is the only crate that depends on every other backend crate, which is what lets the links below
//! actually resolve; a page anywhere else could name its neighbours but not point at them.
//!
//! Nothing here is code. Read it alongside the crates it describes rather than instead of them:
//! each crate's own front page explains what that crate is for, and this one explains why the
//! boundaries between them are where they are.
//!
//! # The smallest complete program
//!
//! ```
//! use auris_session::{Session, SessionOptions};
//!
//! // No window and no audio device — the same session the command line tool drives, and the
//! // same one the desktop application drives with a device attached.
//! let mut session = Session::new(SessionOptions::headless())?;
//! let track = session.add_default_instrument_track("Lead")?;
//!
//! assert_eq!(session.project().tracks.len(), 1);
//! assert!(session.can_undo());
//! # Ok::<(), auris_session::SessionError>(())
//! ```
//!
//! * [`architecture`] — the crates, the direction they depend in, and the two threads.
//! * [`realtime`] — what may and may not happen on the audio callback thread.
//! * [`plugins`] — writing an instrument or an effect, with a worked example.
//! * [`composition`] — the song specification and how a piece is written from it.
//! * [`platforms`] — where macOS and Windows differ, and the rules that keep both alive.

pub mod architecture {
    //! The crates, the direction they depend in, and the two threads.
    //!
    //! # The dependency rule
    //!
    //! Dependencies run strictly downhill, and the boundary is enforced by what each crate is
    //! *allowed to name* rather than by convention:
    //!
    //! ```text
    //! BACKEND — no UI dependency of any kind
    //!   auris-core      types, plugin traits, project model — no local dependencies at all
    //!   auris-dsp       effects and DSP primitives
    //!   auris-synth     built-in instruments
    //!   auris-engine    render graph, transport, cpal output, offline renderer
    //!   auris-io        audio file import/export, project save/load
    //!   auris-gpu       optional wgpu compute for offline analysis
    //!   auris-compose   score-based automatic composition
    //!   auris-i18n      interface text in every language
    //!   auris-session   the document, the engine and every command a frontend needs
    //!
    //! FRONTEND
    //!   auris-gpui      the desktop application  (binary: auris-studio)
    //!   auris-cli       the command line tool    (binary: auris)
    //! ```
    //!
    //! Three rules carry most of the weight.
    //!
    //! **[`auris_core`] depends on nothing local.** It holds [`AudioBuffer`](auris_core::AudioBuffer),
    //! the [`Instrument`](auris_core::plugin::Instrument) and [`Effect`](auris_core::plugin::Effect)
    //! traits, the parameter model, the [`PluginRegistry`](auris_core::registry::PluginRegistry) and
    //! the serialisable [`Project`](auris_core::project::Project). Everything else is downstream of
    //! it, so a change here is a change to the whole system and is made carefully.
    //!
    //! **[`auris_engine`] does not depend on [`auris_dsp`] or [`auris_synth`].** It drives plugins
    //! purely through the `auris-core` traits. That is what keeps the plugin system honest: if the
    //! engine could reach for a concrete effect it would eventually do so, and the traits would
    //! quietly stop being sufficient for anyone else's. The binary is what installs those packs
    //! into the registry — see [`crate::default_registry`].
    //!
    //! **A frontend depends on [`crate::Session`] and its own toolkit, and on nothing else in the
    //! workspace.** If `auris-gpui` ever needs `auris-engine` directly, something that belongs in
    //! the session layer has leaked into the UI. Move it down rather than adding the dependency.
    //!
    //! # Why a session layer exists
    //!
    //! Keeping the *rendering* backend UI-free is the easy part. The piece that usually leaks is
    //! the orchestration around it: building the registry, rebuilding the render graph after an
    //! edit, deciding which changes need a whole new graph and which fit in a command, tracking
    //! undo, resolving a parameter to the plugin that owns it. All of that is [`crate::Session`],
    //! so a second frontend reuses it instead of reimplementing it slightly differently.
    //!
    //! The command line tool exists as much to keep the split honest as to be useful: it drives
    //! the identical session with no window and no audio device, so anything that leaks into the
    //! UI stops compiling there.
    //!
    //! **New work that is a *command* — anything a user could ask for — goes in `auris-session` so
    //! every frontend gets it. New work that is *presentation* stays in the frontend.**
    //!
    //! # The two threads
    //!
    //! The UI thread owns the [`Project`](auris_core::project::Project), which is the editable
    //! document. The audio thread owns an [`auris_engine::RenderGraph`], which is that document
    //! flattened into something renderable without a single allocation: live plugin instances,
    //! note events already converted to absolute sample positions, every scratch buffer pre-sized.
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
    //! A structural edit rebuilds the graph off the audio thread and sends it down a bounded
    //! queue; the audio thread swaps it in and pushes the old one back so its destructor — which
    //! frees plugins and sample buffers — runs on the UI thread. Small changes such as a fader
    //! move travel as cheap [`auris_engine::EngineCommand`]s instead. See [`super::realtime`].
    //!
    //! # Transactions
    //!
    //! Every mutator on [`crate::Session`] records its own undo step, so a pointer drag would
    //! push one step per pointer move. A gesture therefore wraps itself in a transaction with
    //! [`Session::begin_transaction`](crate::Session::begin_transaction) and
    //! [`Session::end_transaction`](crate::Session::end_transaction): one undo step for the whole
    //! gesture, and none at all when the drag changed nothing — which is what stops a selection
    //! click quietly pushing real history off the end of the stack. The transaction also batches
    //! the graph rebuild, so a structural edit inside one sets a flag and the close rebuilds once.
    //!
    //! # Where the audio actually goes
    //!
    //! [`auris_engine::render_block`] is the single implementation of "produce N frames".
    //! Realtime playback calls it from the device callback; [`auris_engine::render_project`] calls
    //! it in a loop as fast as the CPU allows. Because there is only one path, an export matches
    //! what was heard, sample for sample — there is no second implementation that could drift.
}

pub mod realtime {
    //! What may and may not happen on the audio callback thread.
    //!
    //! # The contract
    //!
    //! [`Instrument::process`](auris_core::plugin::Instrument::process) and
    //! [`Effect::process`](auris_core::plugin::Effect::process) run on the audio callback thread.
    //! In those functions there is **no allocation, no locking, no blocking and no I/O**. A missed
    //! deadline is not a slow frame that nobody notices; it is an audible click in every listener's
    //! monitors.
    //!
    //! Anything expensive goes in
    //! [`prepare`](auris_core::plugin::Instrument::prepare), which the engine calls from a normal
    //! thread and hands a [`PrepareContext`](auris_core::plugin::PrepareContext) carrying the
    //! sample rate, the largest block that will ever arrive, and the channel count. Size every
    //! buffer for that block; `process` may then index into them freely.
    //!
    //! Indexing counts too. Every slice index reachable from `process` is derived from a
    //! `min`-clamped length, because a panic on the audio thread takes the stream down.
    //!
    //! # How the two sides talk
    //!
    //! | Direction | Mechanism | Cost |
    //! | --- | --- | --- |
    //! | UI → audio, structural | a whole new [`auris_engine::RenderGraph`] down a bounded queue | one allocation-free swap |
    //! | UI → audio, small | [`auris_engine::EngineCommand`] | a few words |
    //! | audio → UI, graphs | the retired graph, returned to be dropped | a refcount |
    //! | audio → UI, levels | [`auris_engine::MeterBank`], lock-free | a relaxed atomic per track |
    //! | audio → UI, playhead | one relaxed atomic | free |
    //!
    //! The graph handover is the important one. Freeing a graph means dropping plugin instances
    //! and sample buffers, which allocates; doing that inside the callback would risk an xrun. So
    //! the audio thread pushes the old graph back and the UI thread drops it, on its next call to
    //! [`EngineHandle::collect_garbage`](auris_engine::EngineHandle::collect_garbage).
    //!
    //! # Ramps, and why they are counted in frames
    //!
    //! A value that jumps between blocks is a step in the waveform, and a step is a click. Faders,
    //! pan and mute all slide instead. The slides are counted in *frames*, never in blocks, so
    //! splitting a callback in two and rendering the halves separately lands on exactly the same
    //! samples — a property the engine's tests assert on directly. Deriving each sample's gain
    //! from its position rather than accumulating it matters for the same reason: an accumulating
    //! ramp drifts by a rounding error per frame, and the drift depends on where the block
    //! boundaries happened to fall.
    //!
    //! # Latency and tails
    //!
    //! An effect that looks ahead reports it through
    //! [`Effect::latency_frames`](auris_core::plugin::Effect::latency_frames). The engine holds
    //! every other track back to the longest chain in the graph so the parts stay in step, and an
    //! export renders the resulting lead-in and drops it so the file still starts on the timeline.
    //!
    //! [`Effect::tail_frames`](auris_core::plugin::Effect::tail_frames) says how long the effect
    //! keeps sounding after its input stops. Tails along a chain **add up** rather than overlap,
    //! because a delay feeding a reverb keeps feeding it for the whole of its own decay; the
    //! offline renderer uses the total to decide how far past the last clip to keep going.
    //!
    //! Both figures come from plugins, so the arithmetic that combines them saturates rather than
    //! wrapping: a plugin reporting an absurd tail should pad an export, not overflow it.
    //!
    //! # How the rule is enforced
    //!
    //! `auris-engine`'s test suite installs a counting global allocator and asserts that a run of
    //! `render_block` calls allocates exactly zero times, including under the worst event load the
    //! scheduler can produce — every pitch held at once, a seek on every block so the note chase
    //! runs every time, and a full audition queue on top. The rule is a test failure, not a
    //! comment.
}

pub mod plugins {
    //! Writing an instrument or an effect.
    //!
    //! A sound source is a trait implementation and nothing else. Instruments and effects describe
    //! their own parameters, register themselves by id, and the engine and the UI pick them up
    //! without knowing anything further about them.
    //!
    //! # The two traits
    //!
    //! [`Instrument`](auris_core::plugin::Instrument) turns note events into audio;
    //! [`Effect`](auris_core::plugin::Effect) transforms audio in place. Both require
    //! [`Parameterized`](auris_core::plugin::Parameterized), which is what makes a plugin
    //! inspectable: it declares [`ParamDescriptor`](auris_core::param::ParamDescriptor)s and reads
    //! and writes them by [`ParamId`](auris_core::param::ParamId).
    //!
    //! **The parameters you declare become the editor.** The UI is generated from the descriptors
    //! — the right widget, range, unit and scaling — rather than hand-written per plugin, so a new
    //! parameter is one line rather than one line and a control.
    //!
    //! # A worked example
    //!
    //! ```
    //! use auris_core::prelude::*;
    //!
    //! /// An effect that multiplies by a gain, to show the shape of one.
    //! struct Trim {
    //!     params: Vec<ParamDescriptor>,
    //!     gain: f32,
    //! }
    //!
    //! impl Trim {
    //!     fn new() -> Self {
    //!         Self {
    //!             params: vec![ParamDescriptor::new(0u32, "gain", "Gain", 0.0, 2.0, 1.0)],
    //!             gain: 1.0,
    //!         }
    //!     }
    //! }
    //!
    //! impl Parameterized for Trim {
    //!     fn parameters(&self) -> &[ParamDescriptor] {
    //!         &self.params
    //!     }
    //!
    //!     fn param(&self, id: ParamId) -> f32 {
    //!         if id.index() == 0 { self.gain } else { 0.0 }
    //!     }
    //!
    //!     fn set_param(&mut self, id: ParamId, value: f32) {
    //!         if id.index() == 0 {
    //!             // Clamping through the descriptor is what keeps a stale project file, or a
    //!             // host sending nonsense, from reaching the audio path.
    //!             self.gain = self.params[0].clamp(value);
    //!         }
    //!     }
    //! }
    //!
    //! impl Effect for Trim {
    //!     fn descriptor(&self) -> PluginDescriptor {
    //!         PluginDescriptor::effect(
    //!             "example.fx.trim",
    //!             "Trim",
    //!             "Multiplies by a linear gain",
    //!             PluginCategory::Utility,
    //!         )
    //!     }
    //!
    //!     // Nothing to size: this effect has no state. A real one allocates here and nowhere
    //!     // else.
    //!     fn prepare(&mut self, _ctx: &PrepareContext) {}
    //!
    //!     fn reset(&mut self) {}
    //!
    //!     fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
    //!         buffer.apply_gain(self.gain);
    //!     }
    //! }
    //!
    //! let mut registry = PluginRegistry::new();
    //! registry.register_effect(|| Box::new(Trim::new()));
    //! assert!(registry.has_effect("example.fx.trim"));
    //! ```
    //!
    //! # Registering a set of them
    //!
    //! A [`PluginPack`](auris_core::registry::PluginPack) installs a whole family at once —
    //! [`auris_dsp::DspPack`] and [`auris_synth::SynthPack`] are the two that ship — so an
    //! application writes `registry.install::<DspPack>()` rather than one line per plugin.
    //! [`crate::default_registry`] is what the frontends actually call.
    //!
    //! # State
    //!
    //! [`save_state`](auris_core::plugin::Parameterized::save_state) and
    //! [`load_state`](auris_core::plugin::Parameterized::load_state) round-trip a plugin through
    //! the project file as a map of *stable string keys* to values. Keys rather than indices,
    //! because inserting a parameter in the middle would otherwise silently re-aim every saved
    //! value after it. A key the loading plugin does not recognise is ignored rather than
    //! rejected, so a project saved by a later version still opens.
    //!
    //! # Reporting latency and tails
    //!
    //! Override [`latency_frames`](auris_core::plugin::Effect::latency_frames) if your `process`
    //! hands audio back later than it went in, and
    //! [`tail_frames`](auris_core::plugin::Effect::tail_frames) if it keeps sounding after its
    //! input stops. Both are acted on — see [`super::realtime`] — so reporting them wrongly is
    //! audible rather than cosmetic. A latency that depends on a parameter is allowed; the session
    //! notices the change and rebuilds the graph around it.
    //!
    //! # Where a new plugin goes
    //!
    //! Effects belong in [`auris_dsp`] and instruments in [`auris_synth`]. DSP code in this
    //! workspace lives behind unit tests that assert on *numbers* — levels, frequencies, lengths —
    //! rather than on "it runs".
}

pub mod composition {
    //! The song specification, and how a piece is written from one.
    //!
    //! [`auris_compose`] turns a text document into notes on a timeline. The whole crate is one
    //! function — [`compose`](auris_compose::compose) — and everything it does is a pure function
    //! of the specification and its seed, so the same document always writes the same piece.
    //!
    //! # The document
    //!
    //! Line-oriented `field: value` with `[block name]` headers, chosen for the two callers it
    //! has: an agent writing it through a tool interface, which needs a grammar it cannot get
    //! subtly wrong, and a person typing it into a terminal, who needs to change one line without
    //! understanding the rest.
    //!
    //! ```text
    //! title:  Neon Drive
    //! key:    C minor
    //! tempo:  128
    //! mood:   driving
    //! chords: @marusa
    //! form:   intro verse chorus verse chorus outro
    //!
    //! [section chorus]
    //! bars: 8
    //! intensity: 0.95
    //! ```
    //!
    //! It is deliberately a *specification* rather than a notation — it says what to write, not
    //! what was written — because that is the layer at which "make the chorus busier" is one word
    //! rather than four hundred edited notes. Parse it with
    //! [`SongSpec::parse`](auris_compose::SongSpec::parse), which reports every complaint it has at
    //! once rather than stopping at the first.
    //!
    //! ```
    //! use auris_compose::{SongSpec, compose};
    //!
    //! let spec = SongSpec::parse(
    //!     "title:  Neon Drive\n\
    //!      key:    C minor\n\
    //!      tempo:  128\n\
    //!      mood:   driving\n\
    //!      chords: @marusa\n\
    //!      form:   intro verse chorus verse chorus outro\n\
    //!      \n\
    //!      [section chorus]\n\
    //!      bars: 8\n\
    //!      intensity: 0.95\n",
    //! )
    //! .expect("the specification above parses");
    //!
    //! let piece = compose(&spec);
    //! assert_eq!(piece.tempo, 128.0);
    //! assert!(piece.note_count() > 0);
    //!
    //! // Everything is a pure function of the document and its seed, so this holds however many
    //! // times it is asked.
    //! assert_eq!(compose(&spec), piece);
    //! ```
    //!
    //! # Two stages
    //!
    //! **The frame is planned first and then frozen.** Harmony, form and a melodic skeleton are
    //! decided before any part exists. That is what makes the parts agree without knowing about
    //! each other: the bass and the melody both read the same chord at the same tick, so they
    //! cannot drift.
    //!
    //! The skeleton — one structural pitch per chord — is chosen by a dynamic program over the
    //! whole phrase rather than note by note. Picking each note from its predecessor gives a line
    //! that wanders, because nothing is looking ahead to where the phrase has to end; solving the
    //! phrase at once is what makes it arrive somewhere.
    //!
    //! **Then every part is written as a pure function of that frame and its own name.** No part
    //! can depend on another's notes. What makes them sound like a band anyway is that they all
    //! read the same harmony, and the rhythm section all reads the same groove — the bass follows
    //! the kick *pattern*, not the kick *part*.
    //!
    //! # Figures, and why a section repeats
    //!
    //! Each part invents one short figure per section and restates it, because a passage built of
    //! something recognisable is a passage and one built of continuous novelty is not. The figure
    //! is written in scale steps from whatever pitch the skeleton puts under it rather than in
    //! absolute notes, so restating it over the next chord keeps its shape and still belongs to
    //! the harmony.
    //!
    //! A section played twice is the same section both times. `variation` buys the departures
    //! back: at 0 a repeat is note for note, at 1 every playing is written afresh.
    //!
    //! # Named random streams
    //!
    //! A composer drawing from one sequential generator is impossible to edit: adding a roll
    //! anywhere shifts every later draw, so changing the drum density silently rewrites the
    //! melody. Here every decision names its own stream — `["part", "lead", "surface", "chorus",
    //! 2]` — and a stream's numbers depend only on the seed and that name. Adding a part, or a
    //! pass, or a roll inside one pass, leaves every other stream untouched.
    //!
    //! Two consequences worth knowing. A roll is drawn *whether or not the caller can use it*, so
    //! pinning one field does not shift the numbers after it. And streams are named with a slice
    //! of components rather than a formatted string, so a name cannot be built by accidental
    //! concatenation: `["a", "bc"]` and `["ab", "c"]` are different streams.
    //!
    //! # Getting the result into a document
    //!
    //! [`Session::compose`](crate::Session::compose) installs a
    //! [`Composition`](auris_compose::Composition) as the open project — one undo step for the
    //! whole piece, and one graph rebuild rather than one per note. A part naming an instrument
    //! the registry does not have falls back to the first registered one and is reported, because
    //! a missing plugin should cost a timbre rather than a whole piece.
}

pub mod platforms {
    //! Where macOS and Windows differ, and the rules that keep both alive.
    //!
    //! Both run the desktop application and CI builds the whole workspace on both. Development
    //! happens on macOS, so the Windows-only paths are the ones that rot. Four rules keep them
    //! honest.
    //!
    //! # Never name a platform key
    //!
    //! gpui's `Modifiers::platform` is ⌘ on macOS and the *Windows key* on Windows, which the
    //! shell claims first. Read `Modifiers::secondary()`, and write `secondary-` in a keystroke,
    //! never `cmd-`. A default written as `cmd-s` binds the Windows key from a Mac and the command
    //! silently stops existing.
    //!
    //! # The keystroke a user sees is not the keystroke that is stored
    //!
    //! `secondary-s` is what is stored. One function turns it into what the keyboard actually
    //! reports, for comparing; another turns it into ⌘S or Ctrl+S, for reading. Three forms, and
    //! mixing them up produces a binding that works until someone looks at it.
    //!
    //! # Decide with `cfg!`, not `#[cfg]`
    //!
    //! Wherever it is a *choice* rather than an API that exists on only one platform, both arms
    //! then compile and their tests run everywhere. That is the only reason the Windows menu bar
    //! can be checked from a Mac.
    //!
    //! # Windows sets no locale variables
    //!
    //! [`Language::from_system_locale`](auris_i18n::Language::from_system_locale) is what makes a
    //! Japanese Windows install come up in Japanese; reading `LANG` alone would leave it in
    //! English.
    //!
    //! # What differs at runtime
    //!
    //! | | macOS | Windows |
    //! | --- | --- | --- |
    //! | Menu bar | drawn by the system | drawn inside the window |
    //! | Command modifier | ⌘ | Ctrl |
    //! | Audio | CoreAudio | WASAPI |
    //! | Window | Metal | DirectX |
    //! | `auris-gpu` | Metal | Vulkan |
    //!
    //! The menu is one table rendered two ways, so a command added to it reaches both platforms
    //! without being written twice and drifting.
    //!
    //! wgpu's Direct3D 12 backend is switched off because it does not compile at these versions:
    //! `gpu-allocator` resolves `windows` to 0.61 while `wgpu-hal` uses 0.62. [`auris_gpu`] is
    //! optional analysis that steps aside when no backend is present, so a machine with neither
    //! still works — everything simply runs on the CPU.
}
