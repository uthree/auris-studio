//! The editing session: one document, one engine, one command per user action.
//!
//! [`Session`] owns the document, the plugin registry, the audio engine and the undo history, and
//! exposes one method per user-level command. Every frontend drives the application through this
//! module and through nothing else.
//!
//! # Where things are
//!
//! Here: [`Session`] itself, everything it hands back — [`SessionOptions`], [`AudioStatus`],
//! [`SaveReport`], [`MidiReport`], [`ComposeReport`] — and the spine the rest of the module leans
//! on. The spine is four groups and nothing more: opening a session, reading the document,
//! recording an undo step, and rebuilding the render graph. Nearly every command in the files
//! beside this one ends by calling into two of them, which is what keeps the document, the
//! history and the audio thread in step without each command remembering to.
//!
//! The rest is a file per thing a user could ask for. `transport` runs the clock and moves the
//! tempo and the meter under it; `harmony` is the key, the chords, the sections, and hearing any
//! of them; `tracks` is what plays and where its audio goes; `clips` is what sits on a track and
//! `generated` is the half of those that write themselves; `notes` is what is inside one;
//! `mixer` is the strip, its parameters and the lanes that drive them; `files` is everything that
//! reaches the disk and `assets` is how a saved document finds the files it only names;
//! `compose` replaces the whole document with a written piece.
//!
//! Every one of them is `impl Session`, so no path a caller writes changes. And because they are
//! *children* of this module rather than neighbours of it, they read [`Session`]'s private fields
//! as they always did — the split opened up nothing except a handful of helpers that two files
//! now share.

mod assets;
mod clips;
mod compose;
mod files;
mod generated;
mod harmony;
mod mixer;
mod notes;
mod tracks;
mod transport;

#[cfg(test)]
mod fixtures;

pub use compose::{composed_gain_db, kit_trim_db};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use auris_core::param::ParamDescriptor;
use auris_core::time::{Seconds, Ticks};
use auris_core::{AudioSourceBank, PluginRegistry, Project, SourceId, TrackId};
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineHandle, MeterBank, OutputDeviceInfo,
    RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks};
use auris_io::SoundFont;
use auris_sampler::{SharedSoundFonts, SoundFontBank};

use crate::error::SessionError;
use crate::history::{Edit, History};
use crate::registry::default_registry;
use crate::render::{RenderJob, bank_at_rate};
use crate::settings::AudioPreferences;

/// How to start a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionOptions {
    /// Open an audio output device. `false` gives a silent engine that still accepts commands,
    /// which is what a CLI or a test wants.
    pub audio: bool,
    /// Try to use the GPU for waveform and loudness analysis.
    pub gpu: bool,
    /// Which device to open and how, loaded from the user's settings.
    pub audio_preferences: AudioPreferences,
    /// Sample rate for a new project.
    pub sample_rate: f64,
    /// Read the SoundFonts the application ships with, so their sounds are in the library.
    ///
    /// `false` in a test, and for one reason: whether the library is installed is a fact about
    /// the machine, and a document that holds a font on a developer's laptop and none on a CI
    /// runner is a document two test runs would disagree about. It also saves reading two hundred
    /// megabytes per session in a suite that opens hundreds of them.
    pub shipped_fonts: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            audio: true,
            gpu: true,
            audio_preferences: AudioPreferences::default(),
            sample_rate: 48_000.0,
            shipped_fonts: true,
        }
    }
}

impl SessionOptions {
    /// No audio device, no GPU and no shipped library — for tests and for anything that wants a
    /// session which behaves identically on every machine.
    ///
    /// A headless tool that is making *music* rather than checking a document — `auris compose`
    /// is the one — wants the library back, and asks for it with [`Self::with_shipped_fonts`].
    pub fn headless() -> Self {
        Self {
            audio: false,
            gpu: false,
            shipped_fonts: false,
            ..Self::default()
        }
    }

    /// Whether to read the SoundFonts the application ships with.
    pub fn with_shipped_fonts(mut self, shipped_fonts: bool) -> Self {
        self.shipped_fonts = shipped_fonts;
        self
    }

    /// Sets the sample rate a new project is created at.
    pub fn with_sample_rate(mut self, sample_rate: f64) -> Self {
        self.sample_rate = sample_rate;
        self
    }
}

/// What the audio backend ended up doing.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioStatus {
    /// Device name, or a placeholder when running silently.
    pub device: String,
    /// `true` when a real output stream is open.
    pub running: bool,
    /// Rate the engine renders at, which is the device's rate and not necessarily the project's.
    pub sample_rate: f64,
    /// Output channel count.
    pub channels: usize,
    /// Name of the GPU adapter in use, when there is one.
    pub gpu: Option<String>,
}

/// An open editing session.
///
/// Mutators keep the document, the undo history and the audio thread in step. Structural
/// changes rebuild the render graph; cheap ones send a command instead. Wrap a burst of related
/// edits — a pointer drag, a scripted batch — in [`Session::begin_transaction`] /
/// [`Session::end_transaction`] so they become one undo step and one rebuild.
pub struct Session {
    project: Project,
    /// Decoded audio at the project's own rate, which is what the document and the waveform
    /// drawing are expressed in.
    bank: AudioSourceBank,
    /// The same audio at the rate the render graph is built for.
    ///
    /// The engine renders at whatever rate the output device runs at, and a device is free to
    /// disagree with the project — 44.1 kHz hardware under a 48 kHz project is ordinary. The
    /// graph reads a clip's samples one for one against the timeline, so it needs the sources at
    /// its own rate or every audio clip plays at the wrong speed. Converting once, when the rate
    /// changes, is what keeps that off the rebuild path: resampling a long file is expensive and
    /// a rebuild happens on every structural edit.
    ///
    /// When the two rates agree — the usual case — this shares the same buffers and costs
    /// nothing but the map holding them.
    render_bank: AudioSourceBank,
    /// Rate [`Self::render_bank`] currently holds.
    render_bank_rate: f64,
    /// The SoundFonts behind [`Project::soundfonts`], for the same reason [`Self::bank`] exists:
    /// the document keeps paths, the samples live beside it.
    ///
    /// Shared with the registry, whose sampler factory captured it — which is the only way sample
    /// data reaches an instrument the registry builds.
    fonts: SharedSoundFonts,
    /// The fonts that came with the application, kept by the path they were read from.
    ///
    /// [`Self::fonts`] is emptied whenever the document is replaced, because it is keyed by ids
    /// that belong to a document. These are not: the same file is the same samples whichever
    /// project is open, and re-reading two hundred megabytes on every **File → New** is a stall
    /// nobody would understand.
    shipped: HashMap<PathBuf, Arc<SoundFont>>,
    /// Whether this session reads the shipped library at all — see
    /// [`SessionOptions::shipped_fonts`].
    shipped_library: bool,
    registry: Arc<PluginRegistry>,
    engine: EngineHandle,
    device: Option<AudioDevice>,
    gpu: Option<Arc<GpuContext>>,
    /// What the audio backend was asked for, so a settings panel can show it back.
    audio: AudioPreferences,
    /// Whether this session must never claim a real device, however it is reconfigured.
    headless: bool,

    history: History,
    transaction: Option<Transaction>,
    needs_rebuild: bool,
    /// The last edit recorded outside a transaction, and when, for [`Session::record_repeating`].
    last_record: Option<(Edit, Instant)>,

    path: Option<PathBuf>,
    dirty: bool,

    /// Where a spectrum display reads its samples.
    ///
    /// Owned by the session and handed to each graph rather than created with one, because a
    /// rebuild happens on every structural edit and an open display must not be left reading a
    /// scope that nothing writes to any more.
    scope: Arc<auris_engine::Scope>,
    /// Turns the window the engine publishes into a spectrum.
    ///
    /// Here rather than in a frontend because a frontend may not name `auris-dsp` — and because
    /// two of them wanting a spectrum should not each grow their own FFT.
    analyzer: auris_dsp::SpectrumAnalyzer,

    param_cache: HashMap<String, Arc<Vec<ParamDescriptor>>>,
    waveforms: HashMap<SourceId, Arc<WaveformPeaks>>,
}

/// What a Save As produced.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveReport {
    /// The document that was written.
    pub document: PathBuf,
    /// Audio the project refers to that could not be copied into the folder.
    ///
    /// The project saved, and it opens on this machine. It is the *folder* that is incomplete:
    /// carried to another machine, the clips these belong to would be silent. Reported rather
    /// than logged because a save that says nothing is a save the user believes was complete.
    pub uncollected: Vec<PathBuf>,
}

/// What reading a Standard MIDI File produced.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MidiReport {
    /// How many tracks the file turned into.
    pub tracks: usize,
    /// How many notes, across all of them.
    pub notes: usize,
    /// Position just past the last note.
    pub length: Ticks,
}

/// What composing produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeReport {
    /// How many tracks of *music* were created.
    ///
    /// The buses the mix routes through are not counted. They are plumbing rather than parts, and
    /// a report saying eight over a piece with six things playing in it would be answering a
    /// question nobody asked.
    pub tracks: usize,
    /// How many clips.
    pub clips: usize,
    /// How many notes.
    pub notes: usize,
    /// How long the piece is.
    pub length: Ticks,
    /// Instruments a part asked for that this build does not have.
    pub substituted: Vec<String>,
}

struct Transaction {
    edit: Edit,
    before: Project,
    /// Whether the document had unsaved changes when the gesture opened.
    ///
    /// Kept so [`Session::revert_transaction`] can put that back too: a drag on a freshly saved
    /// document that is then abandoned has changed nothing, and the window should not claim it
    /// has.
    dirty_before: bool,
}

impl Session {
    /// Opens a session.
    ///
    /// Never fails for want of audio hardware: the engine falls back to running silently, so a
    /// machine with no output device still edits and exports.
    pub fn new(options: SessionOptions) -> Result<Self, SessionError> {
        let fonts = SoundFontBank::shared();
        let registry = default_registry(Arc::clone(&fonts));
        let project = Project::new("Untitled", options.sample_rate);
        let audio = options.audio_preferences.clone();

        let settings = AudioSettings {
            device: audio.device.clone(),
            sample_rate: audio.sample_rate.or_else(|| {
                // A headless engine has no device to ask, so it needs to be told a rate.
                (!options.audio).then_some(options.sample_rate.round().max(1.0) as u32)
            }),
            block_frames: Some(audio.block_frames.max(16)),
            ..AudioSettings::default()
        };
        let (device, engine) = if options.audio {
            start_audio(&settings)?
        } else {
            // `start_audio` opens the default device, which a headless tool must not do — it
            // would claim the hardware and spin an audio thread for nothing.
            auris_engine::start_silent(&settings)
        };

        let gpu = if options.gpu {
            GpuContext::new().map(Arc::new)
        } else {
            None
        };

        let render_bank_rate = engine.sample_rate();
        let mut session = Self {
            project,
            bank: AudioSourceBank::new(),
            render_bank: AudioSourceBank::new(),
            render_bank_rate,
            fonts,
            shipped: HashMap::new(),
            shipped_library: options.shipped_fonts,
            registry,
            engine,
            device: Some(device),
            gpu,
            audio,
            headless: !options.audio,
            history: History::default(),
            transaction: None,
            needs_rebuild: false,
            last_record: None,
            path: None,
            dirty: false,
            scope: Arc::new(auris_engine::Scope::new()),
            analyzer: auris_dsp::SpectrumAnalyzer::new(auris_engine::SCOPE_WINDOW),
            param_cache: HashMap::new(),
            waveforms: HashMap::new(),
        };
        session.install_shipped_fonts();
        session.rebuild_graph();
        Ok(session)
    }

    /// What the audio backend ended up doing, for a status line.
    pub fn audio_status(&self) -> AudioStatus {
        AudioStatus {
            device: self
                .device
                .as_ref()
                .map_or_else(|| "none".to_string(), |d| d.name().to_string()),
            running: self.engine.is_running(),
            sample_rate: self.engine.sample_rate(),
            channels: self.engine.channel_count(),
            gpu: self
                .gpu
                .as_ref()
                .map(|context| format!("{} ({})", context.adapter_name(), context.backend())),
        }
    }

    /// Every output device the host can see, and what each can do.
    ///
    /// Queried on demand rather than cached: devices come and go while the application runs,
    /// and this is only called when a settings panel opens.
    pub fn output_devices(&self) -> Vec<OutputDeviceInfo> {
        auris_engine::output_devices()
    }

    /// The audio preferences this session was opened with.
    pub fn audio_preferences(&self) -> &AudioPreferences {
        &self.audio
    }

    /// Reopens the audio output with new preferences.
    ///
    /// The old device is dropped first: two streams on the same hardware is at best a glitch
    /// and at worst a refusal to open. Transport position is deliberately *not* carried over —
    /// the new device starts from where the playhead was, but stopped, because a device swap
    /// mid-playback produces a discontinuity nobody wants to hear.
    pub fn set_audio_preferences(
        &mut self,
        preferences: AudioPreferences,
    ) -> Result<(), SessionError> {
        // Capture the position in seconds, not frames: the new device may run at a different
        // rate, and frames counted at the old one would land somewhere else entirely.
        let playhead = self.engine.playhead_seconds();
        let settings = AudioSettings {
            device: preferences.device.clone(),
            sample_rate: preferences.sample_rate,
            block_frames: Some(preferences.block_frames.max(16)),
            ..AudioSettings::default()
        };

        self.device = None;
        let (device, engine) = if self.headless {
            auris_engine::start_silent(&settings)
        } else {
            start_audio(&settings).map_err(|error| SessionError::AudioRestart(error.to_string()))?
        };

        self.device = Some(device);
        self.engine = engine;
        self.audio = preferences;

        // The new engine starts with no graph, no loop and a playhead at zero.
        self.rebuild_graph();
        self.publish_loop();
        self.seek(self.project.tempo_map.seconds_to_ticks(Seconds(playhead)));
        Ok(())
    }

    /// Housekeeping a frontend should call periodically — once per rendered frame is plenty.
    ///
    /// Frees graphs the audio thread has retired, drains the command queue when running silently
    /// where nothing else would, and rebuilds the graph when a parameter has moved a plugin's
    /// latency out from under the delay compensation.
    pub fn poll(&mut self) {
        self.engine.collect_garbage();
        if let Some(device) = &self.device
            && !device.is_running()
        {
            device.discard_pending();
        }
        // Writing a parameter is the one edit that reaches a plugin without rebuilding, so it is
        // also the one that can leave the tracks compensated for the wrong number of frames.
        // Rebuilding re-measures every chain and re-sizes the delay lines to match — and going
        // through `invalidate_graph` is what keeps a drag on such a control to one rebuild at the
        // end of the gesture instead of one per pointer move.
        if self.engine.latency_is_stale() {
            self.invalidate_graph();
        }
    }

    /// The document.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Everything the registry knows how to build.
    pub fn registry(&self) -> &Arc<PluginRegistry> {
        &self.registry
    }

    /// Decoded audio for the project's sources.
    pub fn bank(&self) -> &AudioSourceBank {
        &self.bank
    }

    /// The audio engine's UI-side handle.
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    /// Fills `bands` with the spectrum of whatever strip is being watched, in dBFS.
    ///
    /// Bands rather than bins, spaced by octave between `low_hz` and `high_hz`, because that is
    /// what a display can show and what a musician reads. Silence when nothing is being watched
    /// or the window was written through mid-copy — the next call is one repaint away and will
    /// find a settled one, which is cheaper than making the audio thread wait.
    pub fn spectrum(&mut self, low_hz: f64, high_hz: f64, bands: &mut [f32]) {
        bands.fill(auris_dsp::SILENCE_DB);
        let mut samples = vec![0.0f32; self.analyzer.size()];
        if !self.scope.read(&mut samples) {
            return;
        }
        let rate = self.scope.sample_rate();
        self.analyzer.reset();
        self.analyzer.push(&samples);
        let mut bins = vec![0.0f32; self.analyzer.bin_count()];
        self.analyzer.magnitudes(&mut bins);
        auris_dsp::bands_from_bins(&bins, rate, low_hz, high_hz, bands);
    }

    /// Level a band with nothing in it reports, so a display knows where its floor is.
    pub fn spectrum_silence() -> f32 {
        auris_dsp::SILENCE_DB
    }

    /// Which strip the scope should follow, given what a frontend currently has open.
    ///
    /// Here rather than in the frontend because it is the mapping from *a plugin's place in the
    /// document* to *a position in the render graph*, and positions in the graph are this layer's
    /// business: a frontend that worked it out itself would be reading the track order to do it.
    pub fn watch_strip(&self, track: Option<TrackId>) {
        let source = match track {
            None => auris_engine::ScopeSource::Master,
            Some(id) => match self.project.track_index(id) {
                Some(index) => auris_engine::ScopeSource::Track(index),
                None => auris_engine::ScopeSource::Off,
            },
        };
        self.scope.watch(source);
    }

    /// Stops the analysis, for when nothing is looking at it.
    pub fn stop_watching(&self) {
        self.scope.watch(auris_engine::ScopeSource::Off);
    }

    /// Lock-free level meters.
    pub fn meters(&self) -> &MeterBank {
        self.engine.meters()
    }

    /// Where the document was last saved or loaded from.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// `true` when there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Waveform peaks for an imported source, once it has been analysed.
    pub fn waveform(&self, source: SourceId) -> Option<&Arc<WaveformPeaks>> {
        self.waveforms.get(&source)
    }

    /// A `Send` snapshot that can be rendered on another thread.
    pub fn render_job(&self) -> RenderJob {
        RenderJob::new(
            self.project.clone(),
            self.bank.clone(),
            Arc::clone(&self.registry),
        )
    }

    /// Starts a gesture. Mutations until [`Self::end_transaction`] become one undo step.
    ///
    /// Nesting is not supported; a second call replaces the first, which is what a frontend
    /// wants when a gesture is interrupted.
    pub fn begin_transaction(&mut self, edit: Edit) {
        self.transaction = Some(Transaction {
            edit,
            before: self.project.clone(),
            dirty_before: self.dirty,
        });
    }

    /// Ends a gesture, returning whether it changed anything.
    ///
    /// A transaction that made no difference records no undo step and triggers no rebuild — so
    /// a click that only selected something cannot push real history off the end of the stack.
    pub fn end_transaction(&mut self) -> bool {
        let Some(transaction) = self.transaction.take() else {
            return false;
        };
        if transaction.before == self.project {
            self.needs_rebuild = false;
            return false;
        }
        self.history.push(transaction.edit, &transaction.before);
        self.dirty = true;
        if std::mem::take(&mut self.needs_rebuild) {
            self.rebuild_graph();
        }
        true
    }

    /// Abandons a gesture without recording it. The document keeps whatever it currently holds.
    pub fn cancel_transaction(&mut self) {
        self.transaction = None;
        if std::mem::take(&mut self.needs_rebuild) {
            self.rebuild_graph();
        }
    }

    /// Abandons a gesture and puts the document back the way it was when it opened.
    ///
    /// What Escape means during a drag: the clip goes back where it was picked up from, and
    /// nothing lands on the undo stack. [`Self::cancel_transaction`] keeps the half-finished
    /// result instead, for a host that has already told the user the edit took.
    ///
    /// Returns whether anything was put back.
    pub fn revert_transaction(&mut self) -> bool {
        let Some(transaction) = self.transaction.take() else {
            return false;
        };
        if transaction.before == self.project {
            // Nothing moved, so there is nothing to restore — but a mutation that cancelled
            // itself out may still have marked the graph stale.
            if std::mem::take(&mut self.needs_rebuild) {
                self.rebuild_graph();
            }
            return false;
        }
        self.replace_project(transaction.before);
        self.dirty = transaction.dirty_before;
        true
    }

    /// Steps back one edit, returning what it reversed.
    pub fn undo(&mut self) -> Option<Edit> {
        let edit = self.history.undo_edit()?;
        let project = self.history.undo(&self.project)?;
        self.replace_project(project);
        self.dirty = true;
        Some(edit)
    }

    /// Steps forward one edit, returning what it reapplied.
    pub fn redo(&mut self) -> Option<Edit> {
        let edit = self.history.redo_edit()?;
        let project = self.history.redo(&self.project)?;
        self.replace_project(project);
        self.dirty = true;
        Some(edit)
    }

    /// `true` when there is something to undo.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// `true` when there is something to redo.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Drops the undo history and marks the document as unmodified.
    ///
    /// For scaffolding a host writes itself — a demo project, a template — which should not be
    /// undoable and should not make a freshly opened document look edited.
    pub fn forget_history(&mut self) {
        self.history.clear();
        self.transaction = None;
        self.last_record = None;
        self.dirty = false;
    }

    /// Records an undo step for the edit about to be made.
    ///
    /// Inside a transaction this does nothing: the snapshot was taken when the transaction
    /// opened, and one gesture is one step.
    ///
    /// Validate *before* calling this. Pushing a step clears the redo stack and marks the
    /// document dirty, so a command that records first and refuses after has already cost the
    /// user both — a phantom step that undoes nothing, and a redo branch that is simply gone.
    fn record(&mut self, edit: Edit) {
        if self.transaction.is_none() {
            self.history.push(edit, &self.project);
        }
        // Any ordinary edit breaks a run of repeats: a tempo nudge, a note, another tempo nudge
        // must be three steps, or undoing the second nudge would take the note with it.
        self.last_record = None;
        self.dirty = true;
    }

    /// How long a repeated edit keeps folding into the step before it.
    ///
    /// Long enough to cover the gap between two notches of a wheel a user is still turning,
    /// short enough that coming back to the same control a moment later is a step of its own.
    const COALESCE: Duration = Duration::from_millis(600);

    /// Records an edit that arrives as a stream of small steps — a wheel notch, a held arrow key
    /// — folding it into the previous step when it is the same edit made a moment ago.
    ///
    /// A gesture with a beginning and an end wants [`Self::begin_transaction`] instead. This is
    /// for the ones that have neither: without it a flick of the wheel over the tempo readout
    /// pushes one undo step per event and shoves the real history off the end of the stack.
    fn record_repeating(&mut self, edit: Edit) {
        self.record_repeating_at(edit, Instant::now());
    }

    /// [`Self::record_repeating`] against a clock a test can hold still.
    fn record_repeating_at(&mut self, edit: Edit, now: Instant) {
        let folds = self
            .last_record
            .is_some_and(|(last, at)| last == edit && now.duration_since(at) < Self::COALESCE);
        if folds {
            self.dirty = true;
        } else {
            self.record(edit);
        }
        self.last_record = Some((edit, now));
    }

    /// Marks the render graph as stale, rebuilding immediately outside a transaction.
    fn invalidate_graph(&mut self) {
        if self.transaction.is_some() {
            self.needs_rebuild = true;
        } else {
            self.rebuild_graph();
        }
    }

    /// Rebuilds the render graph and hands it to the audio thread.
    pub fn rebuild_graph(&mut self) {
        let rate = self.engine.sample_rate();
        // Only ever true just after the output device changed, which is also the only time
        // resampling every source is worth what it costs.
        if self.render_bank_rate != rate {
            self.render_bank = bank_at_rate(&self.bank, rate);
            self.render_bank_rate = rate;
        }
        let mut graph = RenderGraph::build_at(
            &self.project,
            &self.render_bank,
            &self.registry,
            self.engine.max_block(),
            rate,
        );
        graph.set_scope(Arc::clone(&self.scope));
        if let Err(error) = self.engine.set_graph(graph) {
            log::warn!("could not update the render graph: {error}");
        }
    }

    fn send(&self, command: EngineCommand) {
        if let Err(error) = self.engine.send(command) {
            log::debug!("engine command dropped: {error}");
        }
    }

    /// Replaces the whole document, keeping the engine in step.
    fn replace_project(&mut self, project: Project) {
        self.adopt_project(project);
        self.rebuild_graph();
        // The loop lives in the audio thread's transport and only `SetLoop` moves it, so a
        // document swap that does not republish leaves playback wrapping the old range.
        self.publish_loop();
    }

    /// Replaces the document *without* telling the engine, for a caller that has more to do first.
    ///
    /// [`Self::open`] is the one: its document names files that have not been read yet, and a
    /// graph built over those would be a graph of silent tracks — logged, one warning per track,
    /// about assets that are about to arrive — and then thrown away and built again. Every other
    /// caller wants [`Self::replace_project`].
    fn adopt_project(&mut self, project: Project) {
        self.project = project;
        self.transaction = None;
        self.needs_rebuild = false;
        self.last_record = None;
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("project", &self.project.name)
            .field("tracks", &self.project.tracks.len())
            .field("dirty", &self.dirty)
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamTarget;
    use crate::session::fixtures::session;
    use auris_core::param::ParamId;
    use auris_core::{ClipId, EffectSlotId, Note};

    /// A moment `ms` after whichever `Instant` it is added to.
    fn tick(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_refused_edit_leaves_no_step_and_no_dirt_behind_anywhere() {
        // Every mutator that once recorded before validating, exercised with ids nothing owns.
        // What this pins: a record clears the redo stack and marks the document dirty, so a
        // command that refuses must refuse *before* it records — the same invariant the
        // preset and instrument commands already keep.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();
        let ghost = ClipId(9_999);
        let slot = EffectSlotId(9_999);

        assert!(session.move_clip(ghost, Ticks::ZERO).is_err());
        assert!(session.resize_clip(ghost, Ticks(960)).is_err());
        assert!(
            session
                .add_note(ghost, Note::new(60, Ticks::ZERO, Ticks(960)))
                .is_err()
        );
        assert!(
            session
                .move_notes(ghost, &[(0, Ticks::ZERO, 60)], Ticks::ZERO, 0)
                .is_err()
        );
        assert!(session.resize_note(ghost, 0, Ticks(960)).is_err());
        session.move_clips(&[(ghost, Ticks::ZERO)], Ticks(960));
        session.remove_effect(slot);
        session.set_effect_enabled(Some(track), slot, false);
        session.move_effect(Some(track), slot, 1);
        session.set_param(ParamTarget::TrackGain(TrackId(9_999)), -3.0);

        assert!(!session.can_undo(), "a refused edit left a step behind");
        assert!(
            !session.is_dirty(),
            "a refused edit marked the document dirty"
        );

        // And the redo branch survives a stale gesture, which is where the cost actually
        // landed: the piano roll can hold a clip id that undo just removed.
        session
            .add_midi_clip(track, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session.undo();
        assert!(session.can_redo());
        assert!(session.move_clip(ghost, Ticks::ZERO).is_err());
        assert!(
            session.can_redo(),
            "a refused edit destroyed the redo stack"
        );
    }

    #[test]
    fn a_headless_session_opens_without_audio() {
        let session = session();
        let status = session.audio_status();
        assert!(!status.running);
        assert!(!session.is_playing());
        assert!(session.project().tracks.is_empty());
    }

    #[test]
    fn a_transaction_that_changes_nothing_records_no_undo_step() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        let steps_before = session.can_undo();

        session.begin_transaction(Edit::MoveClip);
        let changed = session.end_transaction();

        assert!(!changed);
        assert_eq!(session.can_undo(), steps_before);
        // The no-op transaction must not have discarded anything either.
        assert!(session.undo().is_some());
    }

    #[test]
    fn a_transaction_collapses_many_edits_into_one_undo_step() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();

        session.begin_transaction(Edit::MoveClip);
        for beat in 1..=8 {
            session
                .move_clip(clip, Ticks::from_beats(beat as f64))
                .unwrap();
        }
        assert!(session.end_transaction());

        assert_eq!(
            session.midi_clip(clip).unwrap().start,
            Ticks::from_beats(8.0)
        );
        session.undo().unwrap();
        // One step takes the clip all the way back, not one beat back.
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
    }

    #[test]
    fn undo_and_redo_walk_the_document() {
        let mut session = session();
        session.add_default_instrument_track("One").unwrap();
        session.add_default_instrument_track("Two").unwrap();
        assert_eq!(session.project().tracks.len(), 2);

        assert_eq!(session.undo(), Some(Edit::AddInstrumentTrack));
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.redo(), Some(Edit::AddInstrumentTrack));
        assert_eq!(session.project().tracks.len(), 2);
    }

    #[test]
    fn an_abandoned_gesture_puts_the_document_back_and_leaves_no_trace() {
        // Escape during a drag. The clip goes back where it was picked up from, the undo stack
        // is untouched, and a document that was saved a moment ago is still saved.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();
        let steps = session.history.can_undo();

        session.begin_transaction(Edit::MoveClip);
        session.move_clips(&[(clip, Ticks::ZERO)], Ticks::QUARTER);
        assert_ne!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);

        assert!(session.revert_transaction());
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
        assert_eq!(session.history.can_undo(), steps);
        assert!(!session.is_dirty(), "an abandoned gesture edited nothing");
    }

    #[test]
    fn a_gesture_that_moved_nothing_is_not_worth_reverting() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.begin_transaction(Edit::MoveClip);
        assert!(!session.revert_transaction());
        assert!(
            session.transaction.is_none(),
            "the gesture is over either way"
        );
    }

    #[test]
    fn a_parameter_changed_without_a_drag_can_still_be_undone() {
        // A menu choice, a toggle or the wheel reaches `set_param` with no gesture around it.
        // Unrecorded, Undo took back whatever the user did *before* touching the knob instead.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let descriptor = session.descriptor_for(target).unwrap();
        let before = session.param_value(target, &descriptor);
        let after = descriptor.clamp(descriptor.max);
        assert_ne!(before, after, "the test needs a parameter it can move");

        session.set_param(target, after);
        assert_eq!(session.undo(), Some(Edit::AdjustParameter(target)));
        assert_eq!(session.param_value(target, &descriptor), before);
    }

    #[test]
    fn notches_on_two_different_controls_are_two_undo_steps() {
        // Coalescing compares edits, and `AdjustParameter` used to carry no target — so a
        // cutoff notch and a fader notch within the window folded into one step, and undo
        // silently took back the first alongside the second.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let cutoff = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let fader = ParamTarget::TrackGain(track);
        session.forget_history();

        let start = Instant::now();
        session.record_repeating_at(Edit::AdjustParameter(cutoff), start);
        session.record_repeating_at(Edit::AdjustParameter(fader), start + tick(50));
        assert_eq!(session.undo(), Some(Edit::AdjustParameter(fader)));
        assert_eq!(
            session.undo(),
            Some(Edit::AdjustParameter(cutoff)),
            "two controls inside the window folded into one step"
        );
    }

    #[test]
    fn a_stream_of_notches_is_one_undo_step_and_a_later_one_is_its_own() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();

        let start = Instant::now();
        for notch in 1..=8i32 {
            session.record_repeating_at(
                Edit::ChangeTempo(Ticks::ZERO),
                start + tick(notch as u64 * 50),
            );
            session.project.set_bpm(120.0 + f64::from(notch));
        }
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 120.0);
        assert!(!session.can_undo(), "eight notches were one step");

        // Coming back to the control after the window has closed is a step of its own.
        session.redo();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start + tick(5_000));
        session.project.set_bpm(140.0);
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 128.0);
    }

    #[test]
    fn an_edit_in_between_keeps_two_repeats_apart() {
        // Nudge, write a note, nudge again. Folding the second nudge into the first would make
        // one Undo take the note with it.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();

        let start = Instant::now();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start);
        session.project.set_bpm(130.0);
        session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start + tick(20));
        session.project.set_bpm(140.0);

        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 130.0);
        assert_eq!(session.undo(), Some(Edit::AddClip));
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 120.0);
    }

    #[test]
    fn a_render_job_is_independent_of_later_edits() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        let job = session.render_job();
        session.remove_track(track).unwrap();

        // The job kept its own copy, so the render still contains the note.
        let rendered = job
            .render(&auris_engine::OfflineOptions::whole_project(), &mut |_| {})
            .unwrap();
        assert!(rendered.peak() > 0.01);
        assert!(session.project().tracks.is_empty());
    }
}
