//! How Auris Studio fits together.
//!
//! The workspace has no root crate — `cargo doc` on it produces a list of thirteen crates and no
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
//! * [`timelines`] — the four maps laid along the song, and what the meter has that they lack.
//! * [`harmony`] — the key and the chords, and why they belong to the timeline.
//! * [`documents`] — what a project is on disk, and how it refers to files it does not contain.
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
    //!   auris-core      types, music theory, plugin traits, project model — no local dependencies
    //!   auris-dsp       effects and DSP primitives
    //!   auris-synth     built-in instruments                      → auris-dsp
    //!   auris-sampler   SoundFont playback: the bank and the sampler instrument → auris-dsp
    //!   auris-clap      hosting of third-party CLAP plugins        → auris-core only
    //!   auris-engine    render graph, transport, cpal in and out, offline renderer
    //!   auris-io        audio file import/export, project save/load
    //!   auris-gpu       optional wgpu compute for offline analysis
    //!   auris-compose   score-based automatic composition
    //!   auris-vocal     lyrics to phonemes, notes to voice-model frames → auris-core only
    //!   auris-i18n      interface text in every language
    //!   auris-session   the document, the engine and every command a frontend needs
    //!   auris-toolbox   the commands presented as tools for a language model → auris-session
    //!
    //! FRONTEND
    //!   auris-gpui      the desktop application  (binary: auris-studio)
    //!   auris-cli       the command line tool    (binary: auris)
    //!   auris-mcp       the Model Context Protocol server (binary: auris-mcp)
    //!   auris-agent     the model client — Ollama or OpenAI-compatible (binary: auris-agent)
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
    //! [`auris_core::theory`] is here for that reason and no other. Keys, scales, chords and roman
    //! numerals started in the composer, where they were used; they came down when the document
    //! itself gained a key and a chord progression, because the document model may not name a crate
    //! above it. [`auris_compose`] re-exports the module, so the composer's own paths are unchanged.
    //!
    //! Both instrument crates depend on [`auris_dsp`], for its primitives and not its effects.
    //! [`auris_dsp::Adsr`] is the one that matters: the built-in voices and the sampler's
    //! per-note fade are the same generator, so an attack of five milliseconds means the same
    //! thing on both, and it will go on meaning the same thing after the next correction.
    //!
    //! **[`auris_engine`] does not depend on [`auris_dsp`] or [`auris_synth`].** It drives plugins
    //! purely through the `auris-core` traits. That is what keeps the plugin system honest: if the
    //! engine could reach for a concrete effect it would eventually do so, and the traits would
    //! quietly stop being sufficient for anyone else's. The binary is what installs those packs
    //! into the registry — see [`crate::default_registry`].
    //!
    //! **A frontend depends on [`crate::Session`], on its own toolkit, and on the presentation
    //! crate for its reader — and on nothing else in the workspace.** There are two presentation
    //! crates because there are two kinds of reader: `auris-i18n` is every word said to a
    //! *person*, in their language; `auris-toolbox` is every word said to a *model* — tool
    //! names, descriptions, argument schemas and the work behind them, in English, because
    //! every model reads it and neither protocol has a language field. The window and the CLI
    //! take the first; `auris-mcp` and `auris-agent` take the second, and taking it from one
    //! shared crate is what keeps the tool called `compose` identical at both doors. If
    //! `auris-gpui` ever needs `auris-engine`, `auris-core` or `auris-io` directly, something
    //! that belongs in the session layer has leaked into the UI. Move it down rather than
    //! adding the dependency.
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
    //! UI stops compiling there. `auris-mcp` is the same wager made a third time — the identical
    //! session behind the Model Context Protocol, over stdio, so a language model's harness can
    //! compose, render and inspect projects as tools. `auris-agent` makes it a fourth, from the
    //! other direction: Auris itself dials a model — a local Ollama server or any
    //! OpenAI-compatible API — hands it the same `auris-toolbox` tools, and runs the loop.
    //!
    //! **New work that is a *command* — anything a user could ask for — goes in `auris-session` so
    //! every frontend gets it. New work that is *presentation* stays in the frontend.**
    //!
    //! # A selection is an argument, never a field
    //!
    //! Several commands mean something different depending on what is selected, and none of them
    //! holds a selection: [`audition_track`](crate::Session::audition_track) sounds a chord on
    //! whatever instrument is to hand, and [`record_targets`](crate::Session::record_targets) says
    //! where a take would land. Both take an `Option<TrackId>` from the caller.
    //!
    //! A selection belongs to whatever is *showing* the document. The command line has none at all,
    //! two windows onto one session would have one each, and a `selected` field on the session
    //! would be a piece of view state that every frontend then had to remember to keep true. What
    //! the session does own is the deliberate half — the armed tracks are stored, because arming is
    //! something a person did rather than somewhere they happen to be looking. A selection can
    //! stand in for exactly one arm, which is why arming is what a band recording at once does:
    //! there is only one thing an eye can be on.
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
    //! # Reading a file is not a change to the document
    //!
    //! Decoding audio and reading a SoundFont are the two slowest things the application does that
    //! are not audio, and neither of them needs the document: each turns a path into bytes in
    //! memory. So each is split in two. [`decode_audio`](crate::decode_audio) and
    //! [`read_soundfont`](crate::read_soundfont) are free functions that run anywhere;
    //! [`Session::place_audio`](crate::Session::place_audio) and
    //! [`Session::install_soundfont`](crate::Session::install_soundfont) change the document, and
    //! so run on the thread that owns it. [`Session::import_audio`](crate::Session::import_audio)
    //! is still both halves in one call, for a caller with nothing else to be doing — the command
    //! line has nothing else to be doing. The gpui frontend runs the first half on a worker and
    //! keeps painting.
    //!
    //! What the split buys costs one thing back: the document can move while a file is being read.
    //! The placing half is where that is caught — audio decoded against a sample rate the project
    //! no longer has is decoded again rather than laid down to play at the wrong pitch.
    //!
    //! # The third thread, and why recording needed one
    //!
    //! Two threads is the whole story for playback and one short of it for recording. cpal has no
    //! duplex stream, so an input device is a *second* callback on a *second* clock — and the
    //! samples it produces have to reach a file, which is blocking I/O that may not happen on
    //! either audio thread.
    //!
    //! ```text
    //!    input callback            record thread              UI thread
    //!   ┌───────────────┐  full   ┌──────────────┐          ┌────────────┐
    //!   │ Capture       │ ──────► │ CaptureReader│ ──────►  │  Session   │
    //!   │ (device)      │ ◄────── │ WavRecorder  │   file   │  Take      │
    //!   └───────────────┘  empty  └──────────────┘          └────────────┘
    //!                                                    drop(Capture) ends it
    //! ```
    //!
    //! Two rules came out of building it, and both are worth knowing before adding anything else
    //! that leaves an audio callback.
    //!
    //! **The pool is the same exchange the graph handover uses, and for the same reason.** Full
    //! buffers out, empty ones back, both bounded. The callback never allocates and never waits;
    //! when the reader falls behind far enough to empty the pool it *drops* the block and counts
    //! it, because a recording with a hole in it is a bad take and a recording that stalled the
    //! device is a stalled machine.
    //!
    //! **The writer runs on a thread of its own, not on the session's.** The session's thread is a
    //! UI's, and a modal dialog that blocks it for a second would cost the take a second of audio.
    //! That in turn is why `Capture` hands out a separate reader: `cpal::Stream` is not `Send` on
    //! every host cpal supports, so the device cannot travel and the pool's far end must.
    //!
    //! **One thread however many tracks are armed.** A take on four tracks is one device's
    //! channels split four ways, not four devices, and the pool has exactly one consumer by
    //! construction — so the block is split by channel where it is drained, and each part goes to
    //! its own file. That is the whole of multitrack recording below the session, and the only
    //! thing it asked of the callback was that a pooled buffer end on a frame boundary: one that
    //! stopped half way through a frame would file every channel one place along for the rest of
    //! the take. A track armed to a channel the device does not have records silence rather than
    //! its neighbour, so a missing input says which one went missing.
    //!
    //! **A count-in is a phase of the transport, not a stretch of the timeline.** The playhead is
    //! held at the position the take will begin from while the click counts, and the arrangement
    //! is silent — `auris_engine::transport::Transport::rolling` is what every source asks, and
    //! the difference between that and `playing` is the whole feature. Nothing else would let
    //! bar one be counted in, there being no timeline in front of it. The take opens *during* the
    //! count, so its file holds however much of the count the input caught: the capture stamps
    //! that alongside the position, and the head is trimmed on the way to a clip.
    //!
    //! # Monitoring, and why the device stopped belonging to the take
    //!
    //! Ending a take used to be `drop`: closing the device dropped the sender, and a disconnected
    //! channel is a signal that cannot arrive before the last block has. It was the neatest part
    //! of the design and it did not survive monitoring, which holds the same device open with no
    //! take running — pressing stop would have taken the monitor down every time.
    //!
    //! So the device belongs to the [`Session`](crate::Session), one of it for both, and a take is
    //! a *phase* within an open device. Two consequences, both load bearing:
    //!
    //! * **The pool's reader is on loan.** There is one, a second take needs it, so the writer
    //!   thread hands it back with the frame count instead of dying with it.
    //! * **Stopping is a flag, and a flag can be read before the block already on its way.** The
    //!   writer waits for two quiet passes rather than closing on the first.
    //!
    //! The monitored signal itself takes a different road out. It cannot use the pool — that has
    //! one consumer by construction, and it is the file writer — so
    //! [`auris_engine::monitor`] is a second channel from the same callback: a ring of stereo
    //! frames the input callback fills and the *render graph* empties, joined to a track so the
    //! input arrives before the effects, the fader and the sends.
    //!
    //! **A ring per monitored track, all of them made when the device opens.** The set changes
    //! while the input callback is running and that callback may not allocate, so the slots exist
    //! before they are wanted and a switch is an atomic flag on one of them. That is also the
    //! whole of why there is a limit on how many tracks can be monitored at once.
    //!
    //! The two clocks turn up again here and get the opposite answer from the one
    //! [`auris_engine::capture`] gives. A take may not be silently corrected, because a take is
    //! being kept. A monitor may, because it is being *listened to*: when drift closes the gap or
    //! opens it too far the reader jumps to the live edge and counts it. It never jumps backwards,
    //! which is the one refinement worth remembering — replaying a third of a second of somebody's
    //! own voice is a worse noise than the gap that prompted it.
    //!
    //! # Playing when there is no MIDI keyboard on the desk
    //!
    //! The computer keyboard can be one, in Logic and GarageBand's layout key for key: `a` is C
    //! up to `;`, the octave on `z` and `x`, the velocity on `c` and `v`, the bend on `1` and `2`,
    //! the modulation wheel across `3` to `8`, and sustain on Tab. The layout is copied rather
    //! than improved on because its whole value is that a hand already knows it.
    //!
    //! All of it is in [`MusicalTyping`](crate::MusicalTyping) rather than in a frontend, because
    //! none of "the letter `d` means E" needs a window — a frontend turns a key event into
    //! [`typing_press`](crate::Session::typing_press) and
    //! [`typing_release`](crate::Session::typing_release) and has done its whole job.
    //!
    //! Three rules are worth carrying away, because each is the sort only found by getting it
    //! wrong:
    //!
    //! * **A held key remembers the track it was pressed on.** Which track a strike goes to is the
    //!   caller's choice and it can change while a key is down. A release that followed the
    //!   selection instead would leave the first track sounding for ever, and there is no gesture
    //!   a user can make to recover from that.
    //! * **A release is answered even when the keyboard is switched off,** because that is exactly
    //!   how a key comes to be down with the mode gone: somebody let go of the switch before the
    //!   note. Anything that ends a performance without a key being lifted — the window losing
    //!   focus, a panic — goes through
    //!   [`release_typed_notes`](crate::Session::release_typed_notes) for the same reason.
    //! * **The keyboard puts back what belongs to the instrument, and keeps what belongs to it.**
    //!   The octave and the velocity are read at the moment a note is struck and sent nowhere, so
    //!   they stay where the hands left them. The bend and the wheel are channel state the
    //!   instrument goes on holding — [`pitch_bend`](crate::Session::pitch_bend) and
    //!   [`controller`](crate::Session::controller) are the live commands for them — so putting
    //!   the keyboard away returns both to zero on every track it moved them on. Otherwise a
    //!   track is left bent, nothing on screen says so, and the timeline plays back wrong.
    //!
    //! The part that *is* a frontend's problem is that the letters are already bound to commands,
    //! and this cannot be solved by intercepting them: gpui resolves a keystroke to an action
    //! before any key listener runs, so a bound key never reaches one. The binding has to fail to
    //! match instead. `auris_gpui::actions::reachable_from` asks
    //! [`shadows_musical_typing`](crate::shadows_musical_typing) about each keystroke as it binds
    //! it, and puts the ones the keyboard plays outside a context the window adds while the mode
    //! is on. Asked about the keystroke rather than the command, so a rebound key is handled
    //! without anything having to know it moved.
    //!
    //! The desktop frontend also *draws* the keyboard, as a floating panel that is on screen for
    //! as long as the mode is. Everything on it is read back out of
    //! [`MusicalTyping`](crate::MusicalTyping) on each frame —
    //! [`sounding`](crate::MusicalTyping::sounding) lights the keys that are down,
    //! [`root`](crate::MusicalTyping::root) says which note each key is on, and
    //! [`wheel_step`](crate::MusicalTyping::wheel_step) which of `3` to `8` is in force. Those
    //! three exist for that panel and are the reason it needs no state: a drawn keyboard with
    //! its own copy of any of them would be a second thing to keep in step, and the day the two
    //! disagreed it would light one key while another sounded.
    //!
    //! A panel inside the window rather than a window of its own, and here the reason is this
    //! module's rather than a layout preference: a second operating-system window takes the
    //! platform's key events with it, so a picture of the keyboard in one would stop the keyboard
    //! working the moment somebody clicked on the picture.
    //!
    //! # Where the audio actually goes
    //!
    //! [`auris_engine::render_block`] is the single implementation of "produce N frames".
    //! Realtime playback calls it from the device callback; [`auris_engine::render_project`] calls
    //! it in a loop as fast as the CPU allows. Because there is only one path, an export matches
    //! what was heard, sample for sample — there is no second implementation that could drift.
    //!
    //! # How it gets there: outputs, buses and sends
    //!
    //! A track's audio does not necessarily go to the master. Its
    //! [`Output`](auris_core::Output) is either the master or a **bus**, and it may also carry any
    //! number of [`AuxSend`](auris_core::AuxSend)s — taps that feed a bus *as well as*, rather
    //! than instead of, its own output. One reverb shared by six tracks at six different levels is
    //! six sends; a single fader over a whole drum kit is six outputs.
    //!
    //! A bus is a [`TrackKind`](auris_core::TrackKind), not a thing of its own. That is what gives
    //! it a fader, a pan, a mute, an effect chain, a colour and an automation lane without any of
    //! them being written a second time, and it is why every command that addresses a strip by
    //! [`TrackId`](auris_core::TrackId) addresses a bus too. What it has instead of clips is
    //! whatever is routed into it.
    //!
    //! Two consequences are worth knowing before reading the code.
    //!
    //! **The order tracks are mixed in is not the order they are stored in.** A bus cannot go
    //! through its own strip until everything feeding it has arrived, so the renderer walks
    //! [`Project::routing_order`](auris_core::Project::routing_order) rather than the track list.
    //!
    //! **Solo travels both ways along the routing.** Soloing a drum track has to leave the drum
    //! bus open, or its audio has nowhere to go; soloing the drum *bus* has to leave the drum
    //! tracks open, or a thing with no material of its own plays silence. A track is audible
    //! exactly when it lies on a path through something soloed —
    //! [`Project::solo_resolution`](auris_core::Project::solo_resolution) is that rule.
    //!
    //! A loop has no order it can be rendered in, so the session refuses to close one and
    //! [`Project::repair_routing`](auris_core::Project::repair_routing) breaks any that arrives in
    //! a file. What routing does to plugin delay compensation is in [`super::realtime`].
    //!
    //! # A key is an edge too, and it faces the other way
    //!
    //! An effect slot may name a track to be **keyed** from: a compressor on the bass listening to
    //! the kick drum, or a hosted plugin with a sidechain port. That is
    //! [`EffectSlot::sidechain`](auris_core::project::EffectSlot::sidechain), and three things
    //! follow from where it is written down.
    //!
    //! **It is recorded on the listener, not the source.** The effect is the end that wants one; a
    //! track has no idea how many chains are keyed from it. So it is the one edge in the routing
    //! stored facing backwards, and
    //! [`Project::routing_order`](auris_core::Project::routing_order) turns it round rather than
    //! every walk doing so again. The question "would this loop?" therefore has two names —
    //! [`routing_would_cycle`](auris_core::Project::routing_would_cycle) for an output or a send,
    //! and [`sidechain_would_cycle`](auris_core::Project::sidechain_would_cycle) for a key, whose
    //! whole content is the swap.
    //!
    //! **The key is what the source puts into the mix.** The engine taps it after that track's
    //! chain, fader, pan and mute — so turning a kick down ducks less, and muting it stops the
    //! duck. A solo elsewhere silences a key for the same reason, which is consistent: under that
    //! solo the source is contributing nothing either.
    //!
    //! **Nothing pays for it unless it is used.** A tap is made per track some chain actually
    //! listens to, and only where the effect in the slot answers
    //! [`Effect::wants_sidechain`](auris_core::plugin::Effect::wants_sidechain) — so a project
    //! with no key in it carries no extra buffer and copies nothing.
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
    //! [`Effect::latency_frames`](auris_core::plugin::Effect::latency_frames), and the engine
    //! equalises it over the whole routing. The rule is one sentence: **a signal's whole journey
    //! to the output must take the same time however it goes.** A track's chain is only part of
    //! that journey — the bus it feeds has a chain too, and so does the master — so what is
    //! equalised is the *path*, not the chain. A bus carrying a look-ahead limiter therefore holds
    //! back the tracks that do *not* pass through it, and an export renders the resulting lead-in
    //! and drops it so the file still starts on the timeline.
    //!
    //! Each outgoing *edge* gets a delay of its own on top, because one track can take two paths
    //! at once: feeding the master dry while sending to that same limiting bus. With a single
    //! delay for the whole track the dry and the wet land the lookahead apart and comb-filter each
    //! other, which no fader can undo. Every copy is taken from an undelayed tap and held back by
    //! its own amount; in a graph where nothing looks ahead all of them are zero and cost nothing.
    //!
    //! [`Effect::tail_frames`](auris_core::plugin::Effect::tail_frames) says how long the effect
    //! keeps sounding after its input stops. Tails **add up** along a path rather than overlap,
    //! because a delay feeding a reverb keeps feeding it for the whole of its own decay — and for
    //! the same reason a track's tail, its bus's and the master's add end to end. The offline
    //! renderer uses the longest such path to decide how far past the last clip to keep going.
    //!
    //! Both figures come from plugins, so the arithmetic that combines them saturates rather than
    //! wrapping: a plugin reporting an absurd tail should pad an export, not overflow it.
    //!
    //! # A repeat is flattened, not remembered
    //!
    //! A clip may be looped: [`loop_end`](auris_core::MidiClip::loop_end) says how far past its own
    //! end the content keeps being laid down. The audio thread never learns that. Building the
    //! graph walks [`loop_passes`](auris_core::loop_passes) and writes each pass out as though a
    //! person had duplicated the clip by hand — a MIDI clip becomes note events at every repeat, an
    //! audio clip becomes one window onto its buffer per repeat — and the renderer plays a flat
    //! list it cannot tell from an unlooped song.
    //!
    //! That is not an optimisation, it is the only shape the rule above permits. A source that knew
    //! it repeated would have to answer "which pass is frame *n* in" inside the callback, on every
    //! block, for every track; the same question asked once at build time costs a `Vec` push per
    //! repeat and is asked on a thread that may allocate.
    //!
    //! It also settles what a loop *is*. `loop_passes` yields **one pass for a clip that does not
    //! repeat**, so there is no looped code path and unlooped code path to keep in step — the
    //! renderer, the MIDI exporter and the arrangement painter all walk passes and the ordinary
    //! case falls out of the general one. And a loop is a *length* rather than a count, which is
    //! what lets its edge be dragged continuously and its last pass be cut half way through.
    //!
    //! # A stretch is prepared, not performed
    //!
    //! An audio clip may [`follow the tempo`](auris_core::AudioClip::follows_tempo): it is
    //! stretched so that it goes on covering the same bars when the piece is played at another
    //! speed. Time stretching is expensive — a search of tens of operations per output sample —
    //! and it allocates the whole result, so it is subject to the rule above and cannot happen
    //! anywhere near the callback.
    //!
    //! So it happens twice removed from it. The session runs `auris_dsp::stretch` while it is
    //! building a graph, puts the result in the render bank beside the source it came from, and
    //! the graph *looks it up*: `AudioSourceBank::stretched`, keyed by the source and by
    //! [`stretch_key`](auris_core::project::stretch_key). The audio thread sees nothing but
    //! another buffer, exactly as it does for a source resampled to the device's rate — which is
    //! the same arrangement, for the same reason, and was here first.
    //!
    //! Three things follow from the key being a *number the clip works out*:
    //!
    //! * It is **rounded to a thousandth**. The side that fills the cache and the side that reads
    //!   it both call [`AudioClip::stretch_in`](auris_core::AudioClip::stretch_in), and two
    //!   readings of one stretch have to be the same `f64` bit for bit or the lookup misses.
    //! * A copy nobody asks for any more is **dropped on the next rebuild**. Dragging a tempo from
    //!   90 to 140 asks for a copy at every value it passes through.
    //! * A lookup that misses **plays the clip as recorded** and says so in the log. A clip out of
    //!   time is wrong in a way somebody can hear and put right; silence is not.
    //!
    //! One clip is one stretch, taken from the tempo where the clip *starts*. A tempo change
    //! underneath a long clip therefore does not bend it — the audio carries on at the speed it
    //! began at. Following a curve would mean re-stretching continuously, which is a feature to
    //! build when somebody asks for it rather than a thing to half-do now.
    //!
    //! Which leaves one question that only shows up at an edit: what happens to that "where the
    //! clip starts" when a clip is cut in two the far side of a tempo change. The half that moves
    //! would read the tempo at its new start, come back at another speed, and put a seam through
    //! the middle of one take — an edit about *where the boundary is* changing what is heard. So a
    //! clip can be anchored: [`AudioClip::tempo_anchor`](auris_core::AudioClip::tempo_anchor) is
    //! the tick whose tempo it obeys, written by a split and by a front trim, and everything reads
    //! it through [`anchored_at`](auris_core::AudioClip::anchored_at) rather than looking at
    //! `start`. A **move** clears it again, and that is the line the field draws: dividing a clip
    //! keeps the sound and changes the boundary, moving one asks for it somewhere else, and
    //! somewhere else is allowed to be a different tempo.
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
    //! # A note-off names a pitch, not a note
    //!
    //! [`NoteEvent::NoteOff`](auris_core::plugin::NoteEvent) carries a pitch and a frame, and that
    //! is all it can carry — nothing in MIDI, in CLAP or here gives a release the identity of the
    //! strike it answers. So an instrument that finds *two* notes of that pitch still sounding has
    //! to choose which of them it ends, and there is one right answer:
    //!
    //! **Release the one that started first.** Then the next off ends the next one, each note
    //! sounds for the span it was written, and a held pedal note is not cut short by a line
    //! crossing it. Releasing the newest instead plays the older note over the newer one's span
    //! and leaves the newer one a click, which is a note the piece silently loses.
    //!
    //! [`VoiceAllocator::note_off`](auris_synth::VoiceAllocator::note_off) is the built-in voices'
    //! implementation of it and is worth using rather than repeating; the sampler keeps its own
    //! because its channels are a font's and not a voice pool's, and it answers the same way.
    //!
    //! The rule is here rather than in either of them because it is a fact about the *workspace*:
    //! a project that played differently depending on whether a part landed on a font or on an
    //! oscillator would be one nobody could mix. `auris-compose` takes it further and writes no
    //! overlap at all — see `parts::untangle` — because a hosted CLAP plugin is under no
    //! obligation to have read this page.
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
    //! # A plugin that needs something a factory cannot carry
    //!
    //! A factory takes no arguments and a [`PluginState`](auris_core::plugin::PluginState) is a
    //! map of `f32`, so neither can deliver two hundred megabytes of samples to an instrument.
    //! [`auris_sampler`] resolves that with a bank: the session owns an
    //! [`auris_sampler::SoundFontBank`], [`auris_sampler::register_sampler`] captures it in the
    //! factory's closure, and the document names a sound by font id, bank and patch — three
    //! numbers small enough to save. Anything that needs to reach a large shared resource from
    //! inside a plugin can be built the same way; what must not happen is the resource travelling
    //! through the state.
    //!
    //! It is also why [`PluginRegistry::set_default_instrument`](auris_core::registry::PluginRegistry::set_default_instrument)
    //! exists. A new track takes
    //! [`default_instrument_id`](auris_core::registry::PluginRegistry::default_instrument_id),
    //! which without a nomination is simply the first id in alphabetical order — and a sampler
    //! with no font imported yet would have won that race and handed somebody silence.
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
    //! # A plugin that wants a picture
    //!
    //! Descriptors buy an editor, not a *display*. An equalizer's whole job is deciding where to
    //! put a curve, and twenty-four numbers do not say where — so its window draws the response,
    //! and each band that is switched in gets a node on it that follows the pointer.
    //!
    //! What that costs is one rule: **the picture is computed by the crate that makes the sound.**
    //! [`auris_dsp::eq_response_db`] takes the settings a display is holding and returns the same
    //! sum [`auris_dsp::Equalizer`] makes from its cached coefficients, over the same
    //! [`auris_dsp::EQ_LAYOUT`] table; the frontend reads the band's four parameters by the keys
    //! that table names and draws what comes back. A frontend that worked the curve out itself
    //! would be a second implementation of the cookbook filters, and the two would agree until the
    //! day one of them was corrected.
    //!
    //! Which plugin gets a picture is a *frontend's* decision and is spelled there — one id, in
    //! `auris_gpui::ui::analyser`. Asking every plugin author about a window they have never seen
    //! would put a frontend's concern into the plugin contract, and a `draws_a_curve` method on
    //! the [`Effect`](auris_core::plugin::Effect) trait is exactly that.
    //!
    //! The envelope graph is the same idea without the id: any plugin declaring all four of
    //! `attack`, `decay`, `sustain` and `release` is drawn one, and dragging its corners writes
    //! those four parameters. All four or none — three of them is not an envelope with a piece
    //! missing but a different shape, and the drum synth's lone decay would be drawn as a ramp
    //! with two corners invented for it. That is why [`auris_sampler`] gained the picture the day
    //! it gained the parameters, without a line changing in the frontend.
    //!
    //! # An envelope over a sound that already has one
    //!
    //! [`auris_sampler`] does not synthesise its own voices — a font's regions carry their own
    //! volume envelopes, and rustysynth neither exposes them nor lets a caller reach a voice. The
    //! one per-note gain there is, is *channel expression*, so a note that is to be faded on its
    //! own has to be given a channel of its own: fifteen of the library's sixteen, one note each.
    //!
    //! That trade is real — fifteen notes instead of 128 voices, and a drum kit's choke groups
    //! only work between notes sharing a channel — so the whole mechanism sits behind a switch:
    //! an `envelope` toggle, off by default, and while it is off the sampler plays the font
    //! exactly as it is written. Two rules came out of it, and both generalise:
    //!
    //! **A feature that costs something is switched on explicitly, not inferred.** An earlier
    //! version turned the mechanism on when any of the four sliders left its default, which is
    //! tidier and wrong: a control that arms itself when you nudge it to see what it does is a
    //! control nobody can disarm with confidence. The test for "on" is a free function
    //! (`auris_sampler`'s `shaping`), so it can be read and asserted on rather than inferred from
    //! the audio.
    //!
    //! **A cost the user cannot see is a cost the window has to say out loud.** Missing polyphony
    //! shows up as "the top note of my chord dropped out", which is the kind of symptom that gets
    //! blamed on anything but the switch that caused it, so the plugin window carries a caution
    //! strip for as long as the envelope is on. Which plugin warns, and about what, is decided in
    //! `auris_gpui::ui::plugin_window::caution` — the frontend, for the same reason it decides
    //! which plugin gets a curve.
    //!
    //! # Where a new plugin goes
    //!
    //! Effects belong in [`auris_dsp`] and instruments in [`auris_synth`], unless the instrument
    //! plays recorded material, which is [`auris_sampler`]'s half. DSP code in this workspace
    //! lives behind unit tests that assert on *numbers* — levels, frequencies, lengths — rather
    //! than on "it runs".
    //!
    //! # Somebody else's plugin
    //!
    //! A CLAP plugin is hosted rather than written, and everything above stops applying at once.
    //! It cannot be held to the realtime contract, it cannot be trusted not to crash the process,
    //! and its state is a byte stream nobody here may read. [`auris_clap`] is where all of that is
    //! kept, and three facts about it are worth carrying:
    //!
    //! **A hosted plugin is two objects, because CLAP splits the same two threads Auris does.**
    //! `ClapPlugin` is not [`Send`] and stays on the session's thread, where it answers what the
    //! parameters are and what the state is. `ClapEffect` and `ClapInstrument` are [`Send`],
    //! implement [`Effect`](auris_core::plugin::Effect) and
    //! [`Instrument`](auris_core::plugin::Instrument), and are the only halves a graph ever holds
    //! — so [`auris_engine`] needs no CLAP dependency and has none. The two reference-count one
    //! instance, so either may be dropped first, on any thread, which is exactly what the
    //! engine's return channel does to a retired graph. CLAP itself has only one kind of plugin;
    //! which half it hands out is the session's choice, made from the plugin's declared features.
    //!
    //! **It cannot go through the registry.** [`PluginRegistry`](auris_core::registry::PluginRegistry)'s
    //! factory is `Fn() -> Box<dyn Effect>`, which cannot produce the main-thread half as well.
    //! So the session builds hosted plugins itself and hands them to
    //! [`RenderGraph::build_with`](auris_engine::RenderGraph::build_with) through the
    //! [`PlacedEffects`](auris_engine::PlacedEffects) and
    //! [`PlacedInstruments`](auris_engine::PlacedInstruments) maps. Every other plugin takes the
    //! path it always did.
    //!
    //! **A slot owns up to two instances, and that is not an optimisation.** A graph is rebuilt on
    //! every structural edit, and the graph being replaced does not come back until the audio
    //! thread has swapped it out — after the replacement had to exist. So at the moment a new
    //! effect is needed the old one is still rendering, and a CLAP plugin will not activate twice.
    //! The session keeps the outgoing instance, hands its state to the incoming one, and reuses it
    //! next time round. A quiet session settles back to one instance; what is never paid twice is
    //! reading the plugin off disk. It is all in `auris_session::session::hosted`.
    //!
    //! **Notes are translated, and the translation is lossy in one direction only.** A note port
    //! declares which dialects it speaks. CLAP's own carries the key as a field and a bend as a
    //! tuning in *semitones*, so nothing has to be scaled by a pitch-bend range the host was never
    //! told; MIDI is the only one of the two with controllers. A plugin speaking both gets
    //! the better half of each, and one speaking neither gets no notes rather than events into a
    //! void. `auris_clap::notes` is the rule, with the whole of it under test.
    //!
    //! **Floating first, embedded when it has to be — and never a panel.** CLAP lets a plugin offer
    //! a window it makes and manages itself, an embedded one it draws inside a window of the host's,
    //! or both, and it is allowed to offer only one. Floating is the one to prefer: everything about
    //! such a window is the plugin's problem rather than a second window implementation living
    //! here, and the plugin is simply told through `set_transient` which window to stay above.
    //!
    //! It is also not what most plugins offer. Everything built on JUCE — most of them, Surge XT
    //! included — draws only into a window it is handed, so the host has to be able to make one.
    //! `auris_clap::window` is that window: a plain native `HWND` or `NSView` with a title bar,
    //! owned by the application's window so it stays above it, painting nothing, because the
    //! plugin's own drawing covers every pixel. `auris_clap::gui::plan_for` is the choice between
    //! the two, and `auris_clap::window::CAN_LEND` is asked *before* a button appears rather than
    //! after it fails.
    //!
    //! What is still ruled out is a panel *inside* the application, and for the reason it always
    //! was: gpui draws its whole interface onto one surface and has no child window to give away,
    //! so a panel reserving a rectangle would be reserving a rectangle of a picture. A second gpui
    //! window does not stand in for one either — on Windows gpui presents through a flip-model swap
    //! chain, and a child window over one of those is composited away rather than drawn.
    //!
    //! So the plugin ends up in a window of its own either way and the difference is only who made
    //! it. Which matters for one thing: a window the host made reports a close by *not destroying
    //! anything*, because the plugin's window is its child and pulling it out from under code that
    //! has not been told yet is a crash. The flag is raised, the window is left standing, and the
    //! owner takes the two down in order — plugin first — when it next looks.
    //!
    //! [`Session::set_plugin_window_parent`] is how the frontend names the window to stay above,
    //! once, and [`PluginWindow`] is what everything else is addressed by.
    //!
    //! **A window belongs to an instance, and wanting one open is a fact about the slot.** Which is
    //! why the wish is kept beside the pair rather than read off either half of it: on the rare
    //! rebuild that really does swap instances, the window moves to whichever one the parameter
    //! panel is now reading. A window drawn by one instance beside a panel reading another is two
    //! views of one slot that disagree.
    //!
    //! **A hosted plugin has no clock and its window will not repaint without one.** It registers
    //! a timer with the host and waits to be called back;
    //! [`Session::poll`](crate::Session::poll) is what calls it, off the same tick the interface
    //! repaints on. This is also where a window the user closed is noticed — the plugin sets a
    //! flag and says nothing else about it.
    //!
    //! [`Session::set_plugin_window_parent`]: crate::Session::set_plugin_window_parent
    //! [`PluginWindow`]: crate::PluginWindow
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
    //! TOML, with the extension `.asong` — the syntax rather than the identity, exactly as a
    //! project file is JSON inside `.auris`. Serde reads and writes it, which is the point: a
    //! format that can only be read makes a dialog that saves its settings a second
    //! implementation of the same grammar, free to disagree with the first.
    //!
    //! ```text
    //! title  = "Neon Drive"
    //! key    = "C minor"
    //! tempo  = 128
    //! mood   = "driving"
    //! chords = "@marusa"
    //! form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]
    //!
    //! [section.chorus]
    //! bars      = 8
    //! intensity = 0.95
    //!
    //! [[part]]
    //! name       = "lead"
    //! instrument = "auris.synth.chiptune"
    //! ```
    //!
    //! Sections are a table keyed by name because their order means nothing — `form` decides what
    //! plays when. Parts are an array of tables because theirs does: it is the order the tracks
    //! are created in.
    //!
    //! It is deliberately a *specification* rather than a notation — it says what to write, not
    //! what was written — because that is the layer at which "make the chorus busier" is one word
    //! rather than four hundred edited notes. Read one with
    //! [`SongSpec::parse`](auris_compose::SongSpec::parse) and write one back with
    //! [`SongSpec::to_toml`](auris_compose::SongSpec::to_toml). A syntax error stops at the first
    //! and says which line it is on; every complaint about *meaning* is reported at once.
    //!
    //! ```
    //! use auris_compose::{SongSpec, compose};
    //!
    //! let spec = SongSpec::parse(
    //!     r#"
    //!     title  = "Neon Drive"
    //!     key    = "C minor"
    //!     tempo  = 128
    //!     mood   = "driving"
    //!     chords = "@marusa"
    //!     form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]
    //!
    //!     [section.chorus]
    //!     bars      = 8
    //!     intensity = 0.95
    //!     tempo     = 132
    //!     "#,
    //! )
    //! .expect("the specification above parses");
    //!
    //! let piece = compose(&spec);
    //! assert!(piece.note_count() > 0);
    //!
    //! // The tempo arrives as the map a document holds, because a section may lift away from the
    //! // song's own tempo — and this chorus does.
    //! assert_eq!(piece.tempo_map.initial_bpm(), 128.0);
    //! assert_eq!(piece.tempo_map.points().len(), 5);
    //!
    //! // Everything is a pure function of the document and its seed, so this holds however many
    //! // times it is asked.
    //! assert_eq!(compose(&spec), piece);
    //!
    //! // And the document it writes is the document it read: the dialog and the file are two
    //! // faces of one type, so neither can drift from the other.
    //! assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
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
    //! Reading the same harmony has to mean reading the same *notes*, which is why a part steps
    //! through [`ChordScale`](auris_core::theory::chord_scale::ChordScale) rather than through
    //! the key's own scale. A chord that borrows does not add a note, it replaces the degree the
    //! note came from — and a melody drawn from the plain scale went on playing the degree it
    //! replaced, so both versions sounded at once. That is the one dissonance an ear calls a
    //! mistake rather than colour. Over a G7 in C minor the notes are `D Eb F G Ab B`, the
    //! harmonic minor, arrived at rather than named.
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
    //!
    //! # Every clip knows what it is
    //!
    //! Each clip a piece arrives with carries a [`ClipRecipe`](auris_core::ClipRecipe), derived by
    //! [`recipe_for`](auris_compose::recipe_for) from the part *as that section played it* — a
    //! chorus that patched the bass an octave up produces a clip whose recipe says so. That one
    //! field is the whole of the feature: another take, write it again, the dial panel and keep
    //! this one all read [`Session::clip_recipe`](crate::Session::clip_recipe) and nothing else, so
    //! not one line downstream had to learn what a composed song is. A composed piece used to be
    //! several hundred anonymous notes whose only granularity was composing the whole thing again.
    //!
    //! Each clip's seed is a stream of the song's seed named by the part and the stretch —
    //! [`clip_seed`](auris_compose::clip_seed) — so it is reproducible from the specification and
    //! different for every clip, which is what makes "individually" mean anything. It is held to
    //! six digits because a seed is a number a person reads off a panel and types back in.
    //!
    //! **A recipe describes a clip; it does not reproduce it.** The two writers are not one
    //! machine and this is where that shows. A whole song is planned with things one clip has no
    //! room for — how far a repeated section departs from its first playing, what leads into what,
    //! the arch of intensity across the form — so writing a composed clip again hands it to
    //! [`write_phrase`](auris_compose::write_phrase), which knows the document's harmony and not
    //! the plan. The notes move. What holds is everything a person meant by the clip: the same
    //! part, the same register, the same density, over the same chords, played the same way.
    //!
    //! The alternative was making `compose` write its clips *through* `write_phrase` so the two
    //! agreed exactly. That trades the composer's output for a button's arithmetic, and the output
    //! is the product. Regenerating from the stored [`Project::song_spec`](auris_core::Project) was
    //! the other one, and it fails the thing regeneration is *for*: it would hand back the
    //! specification's chords over a harmony lane somebody had since edited by hand.
    //!
    //! One consequence to know rather than to be surprised by: a composed clip is a generated clip
    //! in every respect, so dragging its edge rewrites it, exactly as it does for a clip written by
    //! hand. Keep This One is how a take stops being at the mercy of a gesture.
    //!
    //! # The other direction
    //!
    //! [`auris_compose::analysis`] runs the same machinery backwards: notes in, harmony out. It
    //! reads a key off a melody by correlating what the melody actually plays against the
    //! probe-tone profiles, then picks one chord per bar by how much of the bar each of the key's
    //! seven triads accounts for. Nothing in it draws a random number, so the same melody always
    //! reads the same way.
    //!
    //! [`Session::accompany`](crate::Session::accompany) is what that is for. It writes the
    //! detected key and the chart into the document's harmony lane and then generates parts over
    //! the melody's span through the ordinary [`ClipRecipe`](auris_core::ClipRecipe) path — so the
    //! parts it produces are indistinguishable from parts written by hand from a progression
    //! written by hand, which is the point. What it guessed is *visible* and every part it wrote
    //! can be rewritten, because a melody is one voice and one voice cannot settle every question
    //! it raises: a tune in A minor and a tune in C major play the same notes.
    //!
    //! This is why the whole thing is one transaction rather than a document swap. Composing
    //! replaces what is there; accompanying must not, because the melody is the part somebody
    //! wrote.
}

pub mod singing {
    //! The singer track: notes that carry words, the frames a voice model is fed, and the take
    //! it sings back.
    //!
    //! A [`SingerTrack`](auris_core::SingerTrack) is the frontend for a singing-voice
    //! synthesiser that renders offline. The notes carry lyrics, the lyrics become phonemes,
    //! the frames become a waveform through a trained
    //! [auris-singer](https://github.com/uthree/auris-singer) model, and what playback uses is
    //! the [`SingerTake`](auris_core::SingerTake) that render produced.
    //! [`Session::export_singer_frames`](crate::Session::export_singer_frames) still writes
    //! the raw frames, which is how a model is developed against real documents.
    //!
    //! # The voice, and the take
    //!
    //! A voice is one self-contained ONNX file: the phoneme table, the audio parameters and a
    //! presentational voice card all ride inside it, so
    //! [`Session::set_singer_voice`](crate::Session::set_singer_voice) pointing at the file is
    //! the whole installation. The file gets the policy a SoundFont gets — a library shared by
    //! every project, referenced where it lies — while the card's display name is written into
    //! the document the way a font's name is, so a header can say 波音リツ without opening two
    //! hundred megabytes. Loaded models are kept for the session, keyed by path, shared by
    //! every track that sings with them.
    //!
    //! [`Session::sing`](crate::Session::sing) is the whole pipeline in one synchronous call;
    //! a window that must keep painting takes the three-step form —
    //! [`sing_plan`](crate::Session::sing_plan) checks and gathers everything up front,
    //! [`VoiceModel::sing_with`](crate::VoiceModel::sing_with) runs on the caller's thread,
    //! [`land_singer_take`](crate::Session::land_singer_take) files the result. The take lands
    //! in `Audio/` exactly as a recorded take does: an ordinary audio source, reloaded with
    //! everything else when the project reopens, played by the graph from the beginning of the
    //! timeline. The engine keeps the preview instrument standing by under a take, fed nothing
    //! but auditioned notes, so clicking a note in the roll still sounds.
    //!
    //! Once a voice is chosen, the audition itself sings. The pieces:
    //! [`Session::preview_note_frames`](crate::Session::preview_note_frames) writes a tiny
    //! fixed-length score around the one grabbed note,
    //! [`Session::singer_voice_model`](crate::Session::singer_voice_model) hands the loaded
    //! model to whatever thread the frontend renders on, and the finished audio goes to the
    //! engine as a *one-shot*: [`Session::singer_preview_buffer`](crate::Session::singer_preview_buffer)
    //! resamples it to the engine's rate off the audio thread, and
    //! [`Session::play_singer_preview`](crate::Session::play_singer_preview) sends the whole
    //! buffer across the command channel — the `SetGraph` discipline in miniature. The
    //! callback only adds the buffer into the track's scratch; a replaced or spent buffer is
    //! never freed there, it travels back up the retired-data channel with the graphs. A
    //! frontend is expected to cache renders (same voice, seed, pitch and phonemes are the
    //! same audio) so a drag across pitches is instant everywhere it has already been.
    //!
    //! # A take is kept, never silently rewritten
    //!
    //! The contract from "the score does not change; the performer does": every random choice
    //! in a render is pinned by a seed the document stores, regeneration is always a command
    //! aimed at the track, and an edit after a render leaves the take *playing* — a voice
    //! someone chose must not fall back to the formant preview over one edited word. What an
    //! edit does change is the answer to
    //! [`Session::singer_take_state`](crate::Session::singer_take_state), which compares a
    //! fingerprint of everything the render read (the frames, the voice, the seed) against the
    //! one the take carries: `Behind` is a badge a frontend shows, never a fallback the
    //! playback takes.
    //!
    //! # Two representations, one stored
    //!
    //! A note on a singer track holds its [`lyric`](auris_core::Note::lyric) — what the person
    //! typed — and its [`phonemes`](auris_core::Note::phonemes) — what the model is given, as
    //! IPA tokens. The phonemes are **stored, not derived at the moment of use**, for two
    //! reasons that are really one: deriving needs a grapheme-to-phoneme step that may consult
    //! a dictionary the machine opening the document does not have, and a person corrects a
    //! reading by hand, which only a stored list can remember. The command that sets a lyric
    //! writes both fields; [`Session::set_note_phonemes`](crate::Session::set_note_phonemes)
    //! writes the phonemes alone and leaves the word as spelt.
    //!
    //! The frame-level representation is never stored. It is a pure function of the track and
    //! the tempo map — [`auris_vocal::render_frames`] — sampled at the track's
    //! [`frame_hop`](auris_core::SingerTrack::frame_hop): one phoneme id, one pitch in Hz and
    //! one energy per hop, with the bend curve moving the pitch and controller 11 scaling the
    //! energy. The timing rules (consonants take sixty milliseconds at a note's edges,
    //! syllabics stretch, one note sounds at a time) live in [`auris_vocal::frames`] beside
    //! the tests that measure them.
    //!
    //! # One vocabulary, two readers
    //!
    //! Lyrics in kana go through a built-in table ([`auris_vocal::kana`]) and need nothing
    //! installed; kanji and mixed text go through jpreprocess over a **dictionary folder loaded
    //! at run time** — a prebuilt `naist-jdic`, named in the settings and opened where it lies,
    //! exactly the policy a SoundFont gets and for the same reason: it is a library shared by
    //! every project, and bundling it would mean a build script that downloads. The two paths
    //! must emit identical tokens for the same syllable — the vocabulary is OpenJTalk's
    //! phonemic analysis written in IPA glyphs — and a test in [`auris_vocal::openjtalk`] pins
    //! them together, because one syllable spelt two ways is two symbols a model has to learn
    //! were the same sound. Text that needs the dictionary on a machine without one is its own
    //! error variant, so a frontend answers with the setting that fixes it.
    //!
    //! # Why the preview is an instrument and the model is never `process`
    //!
    //! Without a take, a singer track plays through `auris.synth.vocal` — a formant filter
    //! over a saw, one vowel, built from the same primitives as every other built-in — by way
    //! of the same render-graph arm an instrument track uses. Neural inference can never take
    //! that path: [`realtime`](super::realtime) is the contract it would break — allocation,
    //! latency, everything — so the model renders **offline**, frames in, audio out, and what
    //! reaches the audio thread is the finished take. Nor is a whole song ever one inference:
    //! the model's attention grows with the square of the frame count, so `auris-singer` cuts
    //! the timeline at silences into bounded chunks and stitches the answers — memory is paid
    //! per chunk, not per song.
    //!
    //! # Where each piece lives
    //!
    //! The same split as everything else. What a track *stores* is `auris-core`. How text
    //! becomes phonemes and notes become frames is [`auris_vocal`], which depends on the
    //! document model and nothing else local, the same shape as `auris-compose`; how frames
    //! become audio is `auris-singer`, one step further down the same pipeline, and the only
    //! crate that knows an inference runtime exists. What a person can *ask for* — add the
    //! track, set a lyric, choose a voice, sing — is a command on
    //! [`Session`](crate::Session), so every frontend gets it. The lyric sheet, the picker,
    //! the progress overlay and the behind-the-notes badge are presentation, and stay in the
    //! frontend — which reaches the model type through this crate's re-export
    //! ([`VoiceModel`](crate::VoiceModel)), never by naming `auris-singer` itself.
}

pub mod documents {
    //! What a project is on disk, and how it refers to files it does not contain.
    //!
    //! # A folder, not a file
    //!
    //! The document is pretty-printed JSON — [`auris_io::save_project`] writes it, and it holds
    //! no samples, only ids and parameter values. It lives in a folder of its own, with the audio
    //! the song owns beside it:
    //!
    //! ```text
    //! MySong/
    //!   MySong.auris
    //!   Audio/
    //!     kick.wav
    //! ```
    //!
    //! [`Session::save_as`](crate::Session::save_as) creates that folder rather than trusting
    //! anyone to, and [`auris_io::document_in_folder`] is the rule it applies: choosing
    //! `MySong.auris` gives `MySong/MySong.auris`. The invariant being defended is **one folder,
    //! one project** — two documents sharing a folder would share its `Audio/`, and saving one
    //! under a new name would leave both pointing at the same files.
    //!
    //! # Why an asset reference is not a path
    //!
    //! An absolute path is the weakest reference there is: it breaks when the folder moves, when
    //! the project is copied to another machine, when a drive is mounted under a different letter.
    //! [`AssetPath`](auris_core::AssetPath) splits the two cases that want opposite treatment.
    //!
    //! * `Inside` — relative to the project folder, so the folder is what moves. Stored with `/`
    //!   separators on every platform, because `Audio\kick.wav` is a *file called that* on a Unix
    //!   system, and a project saved on Windows would arrive on a Mac with silent tracks.
    //! * `External` — absolute, because nothing else would find it.
    //!
    //! Which one an asset gets is policy, not a property of the file. Imported audio belongs to
    //! one song and is copied in by [`Session::import_audio`](crate::Session::import_audio). A
    //! SoundFont is a library shared by every project that uses it, often hundreds of megabytes,
    //! and is referred to where it lies;
    //! [`Session::collect_assets`](crate::Session::collect_assets) is how somebody archiving a
    //! project asks for those too.
    //!
    //! # Finding a file that moved anyway
    //!
    //! [`Session::open`](crate::Session::open) resolves in two passes. Everything whose stored
    //! reference is still true is read first; only then is there a set of directories that assets
    //! are demonstrably living in, and the ones that moved are looked for there and in the project
    //! folder. A candidate is confirmed by size where the document recorded one, so a different
    //! file wearing the same name is not quietly adopted — which is what
    //! [`SoundFontRef::byte_size`](auris_core::SoundFontRef::byte_size) is for.
    //!
    //! The directories searched include the shipped library — see [`crate::library`] — because the
    //! font that comes with the application is named by its path like any other, and that path is
    //! whatever it was on the machine the project was saved on. Every installation has that file
    //! somewhere, so the reference most likely to break when a project changes hands is the one
    //! reference that always has an answer.
    //!
    //! Anything found under a new path is written back into the document, so the search happens
    //! once rather than on every open. That leaves the project dirty, which is the honest
    //! signal: there is a repair to save. Anything genuinely gone is returned to the caller and
    //! the project still opens, because refusing to open a session over one moved sample would be
    //! a poor trade.
    //!
    //! # Versions
    //!
    //! [`Project::FORMAT_VERSION`](auris_core::Project::FORMAT_VERSION) is checked before the full
    //! parse, so a file from a newer build produces "update Auris Studio" rather than a confusing
    //! complaint about an unknown field. Backwards compatibility is carried by `serde(default)` on
    //! every optional field: version 1 stored assets as bare path strings, which is exactly what
    //! `External` means, so reading those as `External` is the whole of that migration.
    //!
    //! The version moves when an older build could *misread* a newer file, not merely when the
    //! schema grows. A field an old build has never heard of is skipped and everything else opens;
    //! `AssetPath` bumped the version because it changed how an *existing* field was to be read,
    //! and an old build would have resolved a relative path as an absolute one and lost the audio.
    //! [`harmony`](auris_core::harmony) did not, because nothing already in the file changed
    //! meaning.
}

pub mod timelines {
    //! The maps laid along the song, and what the meter has that they lack.
    //!
    //! Four things about a song change as it goes on, and at any one moment every track obeys the
    //! same one of each. None belongs to a track, because a track that could disagree with its
    //! neighbour about where bar nine is would be a bug with no home to fix it in. So all four
    //! sit on the [`Project`](auris_core::Project) beside one another:
    //!
    //! | Map | Says | Anchored at tick 0 with |
    //! |---|---|---|
    //! | [`TempoMap`](auris_core::time::TempoMap) | how fast | the song's tempo |
    //! | [`SignatureMap`](auris_core::time::SignatureMap) | how the bars are counted | the song's meter |
    //! | [`KeyMap`](auris_core::harmony::KeyMap) | what key | the song's key |
    //! | [`ChordMap`](auris_core::harmony::ChordMap) | what chord | nothing — a song may have none |
    //!
    //! A fifth sits beside them and is the odd one out twice over.
    //! [`Automation`](auris_core::automation::Automation) is a *set* of maps — one lane per
    //! automated parameter — rather than one map for the project, because a fader and a cutoff
    //! have nothing to say to each other. And a lane is **not anchored at tick 0**: it holds its
    //! nearest value flat outside the stretch it was written over, because it made a claim about
    //! that stretch and none about the rest of the song. A tempo has to be defined from the first
    //! sample; a filter cutoff does not.
    //!
    //! The contract that lets a mix be automated one control at a time is that
    //! [`Automation::value_at`](auris_core::automation::Automation::value_at) returns an
    //! `Option`. No lane is not "a lane at the default" — it is no answer, and the parameter
    //! keeps the static value on its strip or in its plugin state.
    //!
    //! They are deliberately the same shape — a sorted list of points, each in force until the
    //! next, deserialised through a repr rather than a plain derive so a hand-edited file cannot
    //! produce a list the binary search will misread — so that a reader who knows one knows the
    //! rest. Each answers `…_at(tick)` for the value and `change_at(tick)` for where the change
    //! governing that stretch sits. That second one is what an editor acts *through*: "remove the
    //! tempo change here" means the one driving the clock, not one that happens to begin under the
    //! pixel the pointer landed on.
    //!
    //! # The meter is the one that moves the ruler
    //!
    //! Three of the four can change anywhere. A tempo may turn on an off-beat, a key may change
    //! mid-bar, and the notes underneath neither move nor care.
    //!
    //! A meter cannot. It decides where the bar lines *are*, so a 3/4 beginning half way through a
    //! bar of 4/4 leaves that bar with no length and the bars after it uncountable — and bar
    //! numbers are how a person says where anything is. So [`SignatureMap`] carries a fourth
    //! invariant beside the three the others keep: every change sits a whole number of the
    //! previous meter's bars past the change before it. A change written anywhere is moved onto
    //! the nearest bar line, and a change written *before* one already there pushes the later ones
    //! onto the new grid — which is a real edit to their positions, and the honest one. The
    //! alternative is storing bar numbers, and a stored bar number moves the notes underneath it
    //! every time an earlier bar changes length.
    //!
    //! Because bar numbering is an accumulation rather than a division,
    //! [`SignatureMap::spans`] hands out each stretch with the bar it opens on. That is what the
    //! ruler and the grid painter walk: within a stretch the arithmetic is the uniform arithmetic
    //! it always was, and only the phase changes.
    //!
    //! # What reaches the audio, and what does not
    //!
    //! [`Session::set_tempo_at`](crate::Session::set_tempo_at) re-flattens the render graph and
    //! republishes the loop region, because notes are scheduled in frames and the mapping from
    //! ticks to frames has just moved. Every automation command rebuilds it too, for a different
    //! reason: the lanes are resolved to graph positions when the graph is built, so a lane the
    //! graph has not seen is a lane nothing is driving.
    //!
    //! The meter and the harmony touch nothing. A meter is notation — positions are ticks, the
    //! tempo map turns ticks into samples, and neither asks how many beats are in a bar. Harmony
    //! is already written into the notes that were generated from it. Changing either moves bar
    //! lines and words on screen and not one sample, which is why
    //! [`set_signature_at`](crate::Session::set_signature_at) and its neighbours record an undo
    //! step and stop.
    //!
    //! Automation is read once per rendered segment rather than once per sample. For a fader that
    //! is not an approximation — a gain and a pan are targets the strip ramps across the block it
    //! is given, so a segment-rate write comes out as a slope — and for a plugin parameter there
    //! is nowhere finer to put one. A seek or a loop wrap *arrives* at its values rather than
    //! sliding to them, the same distinction a track draws with `continued_from` when it chases
    //! notes: a playhead that jumped is not a fader that moved.
    //!
    //! [`SignatureMap`]: auris_core::time::SignatureMap
    //! [`SignatureMap::spans`]: auris_core::time::SignatureMap::spans
}

pub mod harmony {
    //! The key and the chords, and why they belong to the timeline.
    //!
    //! # Whose property harmony is
    //!
    //! Harmony changes as a song goes on, and at any one moment every track obeys the same one.
    //! That is the same shape as tempo, and so [`Harmony`](auris_core::harmony::Harmony) sits
    //! beside [`TempoMap`](auris_core::time::TempoMap) on the
    //! [`Project`](auris_core::Project) — not on a track, which could then disagree with its
    //! neighbour, and not as a single constant, which could not hold a modulation.
    //!
    //! It is deliberately built to imitate the tempo map: a sorted list of points, each in force
    //! until the next, anchored at tick 0, and deserialised through a repr rather than a plain
    //! derive so a hand-edited file cannot produce a list the binary search will misread.
    //!
    //! It parts company with the tempo map on what a bad value costs. An impossible tempo divides
    //! by zero in the render path and refuses the file; an impossible chord costs that one chord
    //! and is dropped, because a song with a typo in bar nine should still open.
    //!
    //! # Two timelines
    //!
    //! [`KeyMap`](auris_core::harmony::KeyMap) and
    //! [`ChordMap`](auris_core::harmony::ChordMap) are separate because a key change and a chord
    //! change are separate events. Eight bars of `I V vi IV` should not restate the key eight
    //! times, and a modulation should not rewrite eight chords. The cost is two lookups, which
    //! [`Harmony::chord_at`](auris_core::harmony::Harmony::chord_at) does together.
    //!
    //! They differ in one way that matters. A key map is never empty and
    //! [`key_at`](auris_core::harmony::KeyMap::key_at) always answers; a chord map may be empty and
    //! [`numeral_at`](auris_core::harmony::ChordMap::numeral_at) may answer `None`. A song nobody
    //! has written chords into has no chords, and putting an imaginary `I` at tick 0 to make the
    //! lookup total would be a lie the composer would then go and obey.
    //!
    //! # Numerals, so that a key change means something
    //!
    //! A span stores a [`Numeral`](auris_core::theory::numeral::Numeral) — `IVmaj7` — and not a
    //! [`Chord`](auris_core::theory::chord::Chord). Transposing the key then transposes the
    //! progression, and a modulation halfway through a section reharmonises the rest of it without
    //! a single span being touched. Storing the sounding chord would make both of those into
    //! rewrites.
    //!
    //! That decision has a sharp edge in the file format.
    //! [`Numeral`](auris_core::theory::numeral::Numeral) is stored as the text a musician writes,
    //! but *not* its [`Display`](std::fmt::Display) form: `Quality::Major`'s suffix is empty and
    //! `Quality::Dominant7`'s is `7`, so `IM` would print as `I` and a forced dominant would print
    //! as the extension `7`, and both would read back as something else.
    //! [`Numeral::to_text`](auris_core::theory::numeral::Numeral::to_text) writes the unambiguous
    //! spelling. Without it a chord the user spelled out would come back from a save as a chord the
    //! composer is free to rewrite.
    //!
    //! # Ticks, built from bars
    //!
    //! Positions are [`Ticks`](auris_core::time::Ticks), like every other position in the
    //! document, because a stored bar number changes meaning the moment the time signature is
    //! edited — notes would stay where they sound while chords slid elsewhere. Bars are how a
    //! position is *built*: [`SignatureMap::position`](auris_core::time::SignatureMap::position)
    //! takes a bar and a fractional beat, so several chords in one bar and a chord on an off-beat
    //! are both sayable, and nothing hands out a bare tick.
    //!
    //! A [`Chart`](auris_core::theory::chart::Chart) — the named progressions like `@axis` — is a
    //! tool that stamps spans, never a thing that is stored. Storing the chart would leave the
    //! timeline uneditable: changing one chord of bar six would mean editing a pattern that also
    //! governs bars two and ten.
    //!
    //! A quoted chart follows its **chords** rather than its degrees. Each catalogue entry says
    //! which mode it was written in, and
    //! [`Chart::spelled_in`](auris_core::theory::chart::Chart::spelled_in) reads it against the
    //! relative key when the music is in the other one — so 丸サ進行 asked for in C minor is
    //! Abmaj7, G7, Cm7, Eb7, the loop centred where the piece is, and not its degrees read
    //! against an aeolian scale. That is the same principle as never recolouring a quotation:
    //! the whole value of naming a progression is that it comes out sounding like itself. A
    //! chart somebody wrote out by hand declares no mode and is taken at face value.
    //!
    //! # Clips that write themselves
    //!
    //! A [`MidiClip`](auris_core::MidiClip) may carry a
    //! [`ClipRecipe`](auris_core::ClipRecipe): a preset, a seed and a few dials saying how the
    //! notes in it were written from the harmony underneath. It can then be written again — after
    //! the chords move, or with a different feel, or simply as another take —
    //! and [`freeze_clip`](crate::Session::freeze_clip) drops the recipe when one of the takes
    //! turns out to be the keeper. This is Logic's Drummer, in a shape that costs one optional
    //! field.
    //!
    //! It is a field on a clip and not a third kind of track, which was the alternative and is
    //! how Logic does it. A generated clip *is* a clip: the engine plays it, the exporter writes
    //! it, the piano roll edits it and undo reverses it, with no code anywhere that knows the
    //! difference. A third [`TrackKind`](auris_core::TrackKind) would have broken fourteen
    //! exhaustive matches across six crates and had most of them answer exactly what an
    //! instrument track answers.
    //!
    //! # The score does not change; the performer does
    //!
    //! What the recipe promises — and what it deliberately does not — is one contract in three
    //! clauses, and it is the contract for every derived layer in Auris, not just this one.
    //!
    //! **Within one build, everything is deterministic.** The notes are stored, not recomputed
    //! on load, so a project plays and exports without the composer running at all; and every
    //! random choice is drawn from a seed the file carries, so the same file is the same piece
    //! on every open. This clause is absolute — it is also what makes the measurements in
    //! `docs/evaluation.md` mean anything.
    //!
    //! **Across builds, the text is immutable and the performance may drift.** What a saved file
    //! guarantees forever is what was written down: the notes somebody typed, edited or froze.
    //! Everything that *performs* that text — the instruments, the effects, and the composer when
    //! it is asked to write again — is code, and code improves. A seed therefore names a take
    //! within a build, not an archival format: regenerating under a newer composer is a redraw
    //! in the current style, not a reproduction of the old take, and the old take cannot be
    //! re-derived once replaced. Old composers are never kept around to run old recipes — that
    //! would be a museum of every writer ever shipped, bugs preserved — because the protection
    //! for a keeper is [`freeze_clip`](crate::Session::freeze_clip), which already exists and
    //! costs nothing.
    //!
    //! **Nothing rewrites a clip implicitly.** Regeneration is a command aimed at the clip: the
    //! button, or the resize that must write the phrase again because stretching a recipe means
    //! writing it again. Nothing rewrites a clip because something *nearby* changed, which is
    //! what makes it safe to edit a generated clip by hand — and why contrast between a verse
    //! and a chorus is two clips with two sets of dials rather than a notion of song form that
    //! the document would otherwise have to carry. Safe, and *seen*: every write stamps a
    //! digest of the text into the recipe ([`ClipRecipe::text_digest`](auris_core::ClipRecipe)),
    //! so a clip whose notes have drifted from it —
    //! [`clip_hand_edited`](crate::Session::clip_hand_edited) — says so on its own panel, before
    //! the button that would replace the edits rather than after. A warning, never a gate.
    //!
    //! The contract repeats at every layer. A song spec is text that the composer performs; a
    //! stored note is text that the instruments and effects perform. The left side is saved
    //! verbatim and outlives every update, and the right side is allowed to get better.
    //!
    //! # Performed, not rewritten
    //!
    //! The contract's performer has a note-domain half. A clip — played or written, recipe or
    //! none — may carry [`NoteTransform`](auris_core::NoteTransform)s: humanise, lean, swing,
    //! transpose, gate, applied in order as
    //! [`sounding_notes`](auris_core::MidiClip::sounding_notes) answers. The renderer and the
    //! MIDI writer both ask that one question, so what exports is what plays; the piano roll
    //! reads the notes themselves, so what is shown is the text. Every wander draws from a seed
    //! the transform stores, through the same named streams the composer draws from
    //! ([`auris_core::rng`]), and it draws per loop pass — a repeated bar is loose differently
    //! each time around, which is the one thing baking the wobble into the notes could never do.
    //!
    //! The composer performs through the same stack. It writes its text on the grid — swing
    //! excepted, which the groove decides and the writers keep — and installs the feel as
    //! transforms on the clips it delivers: the wander per pitched part, the lean its role
    //! table says (the hat pushes, the snare lays back), nothing at all on a kick that keeps
    //! the time. `auris_compose::perform` is that table. A recipe therefore carries no
    //! `humanize` dial any more: how loosely a clip is played is its stack's business, one
    //! panel edits it, and writing the text again — regenerate, another take, a dial on the
    //! recipe — leaves the stack alone save for the wander's seed, which follows the take so
    //! one number keeps naming both.
    //!
    //! [`set_clip_transforms`](crate::Session::set_clip_transforms) replaces the stack whole,
    //! and [`freeze_clip_transforms`](crate::Session::freeze_clip_transforms) is the recipe's
    //! trade again: the performance is written into the text and stops being derived from
    //! anything — including the pass, so a frozen loop rehearses its first pass forever. The
    //! transform arithmetic lives in `auris-core` beside the document, and carries the same
    //! duty every effect carries: changing how it sounds changes saved pieces, so its numbers
    //! are pinned by tests and re-measured when they move.
    //!
    //! # Hearing it before anything plays it
    //!
    //! Writing the chords first is the workflow the lane exists for, and until this the result was
    //! silent until a part had been generated from it — structure first, but confirmation last.
    //! [`harmony_voicing`](crate::Session::harmony_voicing) answers with the pitches to sound at a
    //! position and [`audition_track`](crate::Session::audition_track) finds somebody's instrument
    //! to sound them on, because harmony belongs to the timeline and owns none.
    //!
    //! The voicing is deliberately not a part's.
    //! [`voice_for_audition`](crate::Session::voice_for_audition) puts the body
    //! around middle C and the bass an octave and a half under it, every time, so that a slash
    //! chord is audibly a slash chord and consecutive chords are comparable. A part instead has a
    //! register to keep, neighbours to stay clear of and a previous chord to lead from — all of
    //! which make it *better music* and a *worse answer* to "what did I just write down".
    //!
    //! # Two grids, and editing through the chord rather than at it
    //!
    //! Harmony is written coarser than notes are. Everything that writes it rounds through
    //! [`snap_harmony`](crate::Session::snap_harmony), which is the beat — or the editing grid
    //! where that is coarser, since somebody who set the grid to a bar asked for whole bars. A
    //! sixteenth is the right resolution for placing a hi-hat and the wrong one for placing a
    //! chord: at a normal zoom the two are three pixels apart, and nobody aiming at bar five means
    //! bar five and a sixteenth.
    //!
    //! Editing runs the other way. A chord occupies everything from where it starts to the next
    //! change, so [`remove_chord`](crate::Session::remove_chord),
    //! [`move_chord`](crate::Session::move_chord) and
    //! [`remove_key`](crate::Session::remove_key) resolve their argument through
    //! [`change_at`](auris_core::harmony::ChordMap::change_at) — the change *in force* there —
    //! rather than by rounding it. Rounding would be wrong twice over: the chord being edited
    //! almost never starts under the pointer, and a stamped progression divides each bar
    //! musically, so three chords in a bar of 4/4 sit on thirds of it and no editing grid can name
    //! them at all.
}

pub mod platforms {
    //! Where macOS and Windows differ, and the rules that keep both alive.
    //!
    //! Both run the desktop application and CI builds the whole workspace on both. Development
    //! happens on macOS, so the Windows-only paths are the ones that rot. Four rules keep them
    //! honest, and one place deliberately breaks with both platforms' conventions at once.
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
    //!
    //! # One configuration directory, and it is not the platform's
    //!
    //! [`config_dir`](crate::config_dir) answers `~/.config/auris-studio` on macOS and on Windows
    //! as well as on Linux, rather than `~/Library/Application Support` and `%APPDATA%`.
    //! Five files live there — `settings.json` and `progressions.json` from this crate,
    //! `keymap.json`, `appearance.json` and `layout.json` from the desktop frontend — and they
    //! are small, hand-editable and worth version controlling. The people who do that keep a
    //! dotfiles repository checked out over `~/.config`, and a file in `%APPDATA%` cannot join
    //! it.
    //!
    //! It is the one rule above inverted on purpose, so it is worth being explicit about the
    //! trade: the platform convention buys migration by an OS installer and a location a support
    //! page can name, and neither is worth as much here as being able to carry a keymap between
    //! two machines by cloning a repository. `AURIS_CONFIG_DIR` names the directory outright and
    //! `XDG_CONFIG_HOME` moves its parent, for a setup that has already relocated one.
    //!
    //! [`migrate_legacy_config`](crate::migrate_legacy_config) carries an older installation's
    //! files across on the first run after the move. It copies rather than moves and never writes
    //! over a file already in the new place, so it is safe to call on every start-up — which both
    //! frontends do, before the first `Settings::load`.
}

#[cfg(test)]
mod tests {
    /// What this module actually says, as one line of prose.
    ///
    /// The doc lines alone rather than the whole source, because a test asking whether the guide
    /// names a file would otherwise be answered by the name written in the test. Joined with a
    /// space rather than kept as lines, so that rewrapping a paragraph — which is an edit to how
    /// it reads and not to what it claims — cannot fail this.
    fn account() -> String {
        include_str!("guide.rs")
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("//!"))
            .map(str::trim)
            .collect::<Vec<&str>>()
            .join(" ")
    }

    /// The account of the configuration directory has to name every file that turns up in it.
    ///
    /// A list of files is the kind of claim that goes stale in silence. Nothing at runtime reads
    /// it — [`migrate_legacy_config`](crate::migrate_legacy_config) carries whatever the directory
    /// holds rather than a list of names — so a file nobody added to the prose costs nothing at
    /// all until somebody trusts the guide, which is the one thing the guide is for.
    #[test]
    fn every_configuration_file_is_named_in_the_account_of_the_directory() {
        let account = account();

        // The two this crate writes, asked of the code rather than written out a second time.
        for path in [
            crate::Settings::path(),
            crate::progressions::ProgressionBook::path(),
        ] {
            let name = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .expect("a configuration file has a name");
            assert!(
                account.contains(name),
                "{name} is written into the configuration directory and the guide does not say so"
            );
        }

        // And the three the desktop frontend writes. Spelled out rather than asked for, because
        // `auris-gpui` sits above this crate and nothing here may name it — which is also why the
        // guide is where the whole list lives.
        for name in ["keymap.json", "appearance.json", "layout.json"] {
            assert!(
                account.contains(name),
                "{name} is written into the configuration directory and the guide does not say so"
            );
        }

        assert!(
            account.contains("Five files live there"),
            "the count and the list are one claim, so they have to move together"
        );
    }
}
