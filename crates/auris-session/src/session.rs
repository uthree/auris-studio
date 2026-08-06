//! The editing session: one document, one engine, one command per user action.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use auris_core::automation::{Automation, AutomationCurve};
use auris_core::harmony::Harmony;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{PluginKind, PluginState};
use auris_core::project::{ClipCurve, CurvePoint};
use auris_core::theory::chart::{Chart, catalog};
use auris_core::theory::chord::Chord;
use auris_core::theory::key::Key as MusicalKey;
use auris_core::theory::numeral::Numeral;
use auris_core::theory::pitch::MIDDLE_C;
use auris_core::time::{Seconds, SignatureMap, Ticks, TimeSignature};
use auris_core::{
    AssetPath, AudioBuffer, AudioSourceBank, AuxSend, ClipId, ClipRecipe, Color, EffectSlotId,
    MidiClip, Note, Output, PluginRegistry, PresetRef, Project, SendId, SoundFontId, SoundFontRef,
    SourceId, Track, TrackId,
};
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineHandle, MeterBank, OutputDeviceInfo,
    RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks, compute_peaks};
use auris_io::{
    AUDIO_DIR, IoError, SoundFont, SoundFontPreset, byte_size, copy_into, document_in_folder,
    find_named, font_name, import_audio_file, load_project, load_soundfont, preset_count, presets,
    save_project,
};
use auris_sampler::{SAMPLER_ID, SharedSoundFonts, SoundFontBank, store_preset, stored_preset};

use crate::error::SessionError;
use crate::history::{Edit, History};
use crate::param::ParamTarget;
use crate::registry::default_registry;
use crate::render::{RenderJob, bank_at_rate, source_at_rate};
use crate::settings::AudioPreferences;

/// How many samples one waveform bucket covers.
///
/// At 256 a five-minute stereo file's peak data stays under a megabyte while still resolving
/// individual drum hits at normal zoom levels.
const WAVEFORM_BUCKET: u32 = 256;

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

/// The instrument a track that played on the drum channel is given.
const DRUM_INSTRUMENT: &str = "auris.synth.noisedrum";

/// MIDI's drum channel, 0-based. Channel 10, counting the way a musician does.
const DRUM_CHANNEL: u8 = 9;

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
    // ---------------------------------------------------------------- lifecycle

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

    // ---------------------------------------------------------------- accessors

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

    // ---------------------------------------------------------------- history

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

    // ---------------------------------------------------------------- engine

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

    // ---------------------------------------------------------------- transport

    /// Starts playback.
    pub fn play(&mut self) {
        self.send(EngineCommand::Play);
    }

    /// Stops playback, leaving the playhead where it is.
    pub fn stop(&mut self) {
        self.send(EngineCommand::Stop);
    }

    /// Starts or stops playback.
    pub fn toggle_play(&mut self) {
        if self.is_playing() {
            self.stop();
        } else {
            self.play();
        }
    }

    /// `true` when the transport is rolling, read from the audio thread itself.
    pub fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    /// Moves the playhead, clamping to the timeline start.
    pub fn seek(&mut self, tick: Ticks) {
        let frames = self
            .project
            .tempo_map
            .ticks_to_samples(tick.max_zero(), self.engine.sample_rate())
            .raw();
        self.send(EngineCommand::Seek { frames });
    }

    /// Where the playhead is, in ticks.
    pub fn playhead(&self) -> Ticks {
        self.project
            .tempo_map
            .seconds_to_ticks(Seconds(self.engine.playhead_seconds()))
    }

    /// Silences every voice.
    pub fn panic(&mut self) {
        self.send(EngineCommand::Panic);
    }

    /// Turns looping on or off, seeding a two-bar region when there is none.
    ///
    /// Deliberately not recorded. Cycling is how a user listens, not something they write: a
    /// loop-and-listen pass would otherwise fill the undo stack with toggles and push the edits
    /// the pass was checking off the end of it. Dragging the region *is* recorded — that is
    /// aimed at a place in the song rather than at the transport.
    pub fn set_loop_enabled(&mut self, enabled: bool) {
        self.dirty = true;
        self.project.loop_enabled = enabled;
        if enabled && self.project.loop_region.is_none() {
            // Bars one and two, asked for as bars rather than as twice a bar length: with a meter
            // change in the second bar those are not the same span, and the one a person means by
            // "the first two bars" is this one.
            self.project.loop_region = Some((Ticks::ZERO, self.project.signatures.bar_start(3)));
        }
        self.publish_loop();
    }

    /// Sets the loop region. A region that is not positive disables the loop.
    pub fn set_loop_region(&mut self, start: Ticks, end: Ticks) {
        self.record(Edit::SetLoopRegion);
        let (start, end) = if end < start {
            (end, start)
        } else {
            (start, end)
        };
        self.project.loop_region = Some((start.max_zero(), end.max_zero()));
        self.publish_loop();
    }

    /// Sends the loop region to the audio thread.
    ///
    /// The region is stored in ticks and the transport holds frames, so this has to run again
    /// after anything that moves the mapping between them — a document swap or a tempo change.
    fn publish_loop(&self) {
        let (start, end) = self
            .project
            .loop_region
            .unwrap_or((Ticks::ZERO, Ticks::ZERO));
        let rate = self.engine.sample_rate();
        self.send(EngineCommand::SetLoop {
            enabled: self.project.loop_enabled && end > start,
            start: self.project.tempo_map.ticks_to_samples(start, rate).raw(),
            end: self.project.tempo_map.ticks_to_samples(end, rate).raw(),
        });
    }

    /// Sets the project tempo at the start of the timeline.
    ///
    /// The whole-song knob: it turns the stretch that begins at tick zero and leaves any tempo
    /// changes written further along where they are. A transport readout parked mid-song wants
    /// [`Self::set_tempo_at`] with the playhead instead.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.set_tempo_at(Ticks::ZERO, bpm);
    }

    /// Replaces the tempo of the stretch `at` falls in.
    ///
    /// A wheel over the readout arrives as a stream of small deltas, and re-flattening the
    /// graph is the expensive half of this. A value that has not moved is not a change — and
    /// it is the *clamped* value that decides, or holding the wheel past 999 would keep
    /// recording steps that change nothing. The map is a handful of points; probing a clone
    /// is cheaper than the rebuild it saves.
    ///
    /// The recorded edit carries the position of the change being turned, so nudging the tempo
    /// of one stretch and then another stays two undo steps however quickly the hand moves.
    pub fn set_tempo_at(&mut self, at: Ticks, bpm: f64) {
        let at = self.project.tempo_map.change_at(at.max_zero());
        let mut probe = self.project.tempo_map.clone();
        probe.set_point(at, bpm);
        if probe == self.project.tempo_map {
            return;
        }
        self.record_repeating(Edit::ChangeTempo(at));
        self.project.tempo_map = probe;
        // Notes are scheduled in frames, so the graph has to be re-flattened, and the loop's
        // frame positions move with it.
        self.invalidate_graph();
        self.publish_loop();
    }

    /// Sets the tempo from `at` onwards, writing a change on the beat `at` rounds to.
    ///
    /// `at` snaps the way the harmony does — see [`Self::snap_harmony`] — because a tempo
    /// change is aimed at a place in the song, not at the sixteenth the pointer happened to
    /// cross. Writing at tick zero turns the song's opening tempo rather than adding to it,
    /// exactly as [`Self::set_key`] treats the anchor.
    pub fn set_tempo_point(&mut self, at: Ticks, bpm: f64) {
        let at = self.snap_harmony(at);
        let mut probe = self.project.tempo_map.clone();
        probe.set_point(at, bpm);
        if probe == self.project.tempo_map {
            return;
        }
        self.record(Edit::SetTempoPoint);
        self.project.tempo_map = probe;
        self.invalidate_graph();
        self.publish_loop();
    }

    /// Removes the tempo change in force at `at`, letting the tempo before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]. The
    /// anchor at tick zero is not a change and cannot be removed: a song always has a tempo.
    pub fn remove_tempo_point(&mut self, at: Ticks) {
        let at = self.project.tempo_map.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::RemoveTempoPoint);
        self.project.tempo_map.remove_point(at);
        self.invalidate_graph();
        self.publish_loop();
    }

    // ------------------------------------------------------------- time signature
    //
    // The tempo trio again, and deliberately shaped the same way, but none of these touch the
    // engine. A meter is notation: the notes are written in ticks, the tempo map turns ticks
    // into samples, and neither asks how many beats are in a bar. Editing this moves the bar
    // lines and not one sample.
    //
    // Where the tempo commands take any position, these land on bar lines — see
    // [`SignatureMap`](auris_core::time::SignatureMap) for why a change that did not would leave
    // the bars after it uncountable.

    /// The signature in force at `at`.
    pub fn signature_at(&self, at: Ticks) -> TimeSignature {
        self.project.signatures.signature_at(at)
    }

    /// Replaces the signature of the stretch `at` falls in.
    ///
    /// The counterpart of [`Self::set_tempo_at`], and coalescing for the same reason: the wheel
    /// over the transport readout arrives as a stream of steps, and a meter that has not moved is
    /// not a change. The recorded edit carries the position of the change being turned, so
    /// nudging one stretch and then another stays two undo steps.
    pub fn set_signature_at(&mut self, at: Ticks, signature: TimeSignature) {
        let at = self.project.signatures.change_at(at.max_zero());
        let mut probe = self.project.signatures.clone();
        probe.set_point(at, signature);
        if probe == self.project.signatures {
            return;
        }
        self.record_repeating(Edit::ChangeSignature(at));
        self.project.signatures = probe;
    }

    /// Sets the signature from `at` onwards, writing a change on the bar `at` rounds to.
    ///
    /// The ruler's counterpart to [`Self::set_signature_at`]. Writing at tick zero turns the
    /// song's opening meter rather than adding a change to it, exactly as [`Self::set_key`] and
    /// [`Self::set_tempo_point`] treat the anchor.
    pub fn set_signature_point(&mut self, at: Ticks, signature: TimeSignature) {
        let mut probe = self.project.signatures.clone();
        probe.set_point(at.max_zero(), signature);
        if probe == self.project.signatures {
            return;
        }
        self.record(Edit::SetSignaturePoint);
        self.project.signatures = probe;
    }

    /// Removes the signature change in force at `at`, letting the meter before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]. The anchor
    /// at tick zero is not a change and cannot be removed: a song is always in some meter.
    pub fn remove_signature_point(&mut self, at: Ticks) {
        let at = self.project.signatures.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::RemoveSignaturePoint);
        self.project.signatures.remove_point(at);
    }

    /// Sets the editing grid.
    pub fn set_grid(&mut self, grid: Ticks) {
        let grid = Ticks(grid.raw().max(1));
        if self.project.grid == grid {
            return;
        }
        self.project.grid = grid;
        // Not recorded — cycling the grid is a view-adjacent tweak nobody wants on the undo
        // stack — but it is a stored document field and has to reach the file: unmarked, a
        // grid-only change closed without the unsaved prompt and was quietly lost.
        self.dirty = true;
    }

    // ---------------------------------------------------------------- harmony
    //
    // None of these touch the engine. Harmony is a part of the document the render graph never
    // reads: the notes are already written, and changing the chord underneath them does not
    // change a sample. Composing *from* the harmony does rebuild the graph, but that is a
    // different command.

    /// The key and the chords the song is written in, over the timeline.
    pub fn harmony(&self) -> &Harmony {
        &self.project.harmony
    }

    /// The grid a chord or a key change lands on at `at`: the beat, or the editing grid where that
    /// is coarser.
    ///
    /// Harmony is written coarser than notes are. A sixteenth-note editing grid is the right
    /// resolution for placing a hi-hat and the wrong one for placing a chord — nobody aiming at
    /// bar five means bar five and a sixteenth, and at a normal zoom the two are three pixels
    /// apart. The editing grid still wins when it is the coarser of the two, because somebody who
    /// set the grid to a bar asked for whole bars and should get them.
    ///
    /// Which beat, and so which grid, depends on where: an eighth is the beat in 7/8 and half of
    /// one in 3/4.
    pub fn harmony_grid_at(&self, at: Ticks) -> Ticks {
        self.project
            .signatures
            .signature_at(at)
            .ticks_per_beat()
            .max(self.project.grid)
    }

    /// Rounds a position onto [`Self::harmony_grid_at`]. What every harmony command writes through.
    ///
    /// Public because a frontend has to agree with it: a menu that offers "remove the chord here"
    /// only where one exists has to round the pointer the same way the command that writes them
    /// does, or the two disagree by a sixteenth and the item is never offered.
    ///
    /// Counted from the start of the stretch the meter is in force over rather than from tick
    /// zero. A bar line after a meter change need not be a multiple of the new beat — a 7/8 bar
    /// is 3360 ticks and a quarter note is 960 — so a grid counted from the origin would sit a
    /// fraction off every bar line for the rest of the song.
    pub fn snap_harmony(&self, at: Ticks) -> Ticks {
        let at = at.max_zero();
        let origin = self.project.signatures.change_at(at);
        origin + (at - origin).snap_nearest(self.harmony_grid_at(at))
    }

    /// Sets the key from `at` onwards.
    ///
    /// `at` snaps to the harmony grid, so a key change lands where a person aimed rather than
    /// where the pointer happened to be. Tick zero is the song's own key and is always there, so
    /// setting it there changes what the whole song is read in rather than adding a change to it.
    pub fn set_key(&mut self, at: Ticks, key: MusicalKey) {
        self.record(Edit::SetKey);
        let at = self.snap_harmony(at);
        self.project.harmony.keys.set_point(at, key);
    }

    /// Removes the key change in force at `at`, letting the key before it run through.
    ///
    /// *In force at*, not *starting at*: a key change is a boundary, and the stretch it governs
    /// runs to the next one. Removing the change that put the song in E flat means pointing
    /// anywhere in the E flat, which is the whole of what is on screen — rather than at the one
    /// grid position the change happens to sit on.
    ///
    /// The key at tick zero is not a change and cannot be removed: a song is always in some key.
    pub fn remove_key(&mut self, at: Ticks) {
        let at = self.project.harmony.keys.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::SetKey);
        self.project.harmony.keys.remove_point(at);
    }

    /// Sets the chord sounding from `at` onwards, until the next change.
    pub fn set_chord(&mut self, at: Ticks, chord: Numeral) {
        self.record(Edit::SetChord);
        let at = self.snap_harmony(at);
        self.project.harmony.chords.set_point(at, Some(chord));
    }

    /// Removes the chord change in force at `at`, letting the chord before it run through.
    ///
    /// Found through [`ChordMap::change_at`](auris_core::harmony::ChordMap::change_at) rather
    /// than by rounding `at`, for the reason given on [`Self::remove_key`] and one more: a
    /// progression stamped three chords to a bar sits on thirds of a bar, which is not a position
    /// any editing grid can round to, so a rounded removal would silently miss every one of them.
    pub fn remove_chord(&mut self, at: Ticks) {
        let Some(at) = self.project.harmony.chords.change_at(at.max_zero()) else {
            return;
        };
        self.record(Edit::SetChord);
        self.project.harmony.chords.remove_point(at);
    }

    /// Moves the chord change in force at `from` to `to`, and says whether it moved one.
    ///
    /// `to` snaps to the harmony grid; `from` is resolved the way [`Self::remove_chord`] resolves
    /// its argument, so a drag can start anywhere inside the chord rather than on the one pixel
    /// it begins at. Dropping a chord onto another replaces that one.
    pub fn move_chord(&mut self, from: Ticks, to: Ticks) -> bool {
        let Some(from) = self.project.harmony.chords.change_at(from.max_zero()) else {
            return false;
        };
        let to = self.snap_harmony(to);
        if from == to {
            return false;
        }
        self.record(Edit::MoveChord);
        self.project.harmony.chords.move_point(from, to)
    }

    /// Empties the chords in `from..to`, leaving the key timeline alone.
    ///
    /// What sounded at `to` still sounds there: clearing the middle of a song does not silence
    /// the end of it.
    pub fn clear_harmony(&mut self, from: Ticks, to: Ticks) {
        self.record(Edit::ClearHarmony);
        let (from, to) = (self.snap_harmony(from), self.snap_harmony(to));
        self.project.harmony.clear(from, to);
    }

    /// Writes `chart` across `bars` bars from `from`, returning how many chords it wrote.
    ///
    /// The chart repeats or is truncated to fit. `from` snaps to the harmony grid, but the chords
    /// *inside* it do not: a chart divides each bar musically, and three chords in a bar of 4/4
    /// are three lots of 1280 ticks, which is not a grid position and must not be rounded to one.
    /// A stamp is a division of a bar; a drag is an edit on the grid.
    pub fn stamp_progression(&mut self, chart: &Chart, from: Ticks, bars: usize) -> usize {
        self.record(Edit::StampProgression);
        let from = self.snap_harmony(from);
        // The meter the chart begins in: a progression was written in bars of one meter, and a
        // change part way through the stamped range does not re-bar the chart behind it.
        let signature = self.project.signatures.signature_at(from);
        self.project.harmony.stamp(chart, from, bars, signature)
    }

    // ---------------------------------------------------------------- structure
    //
    // Like the harmony, none of these touch the engine: the notes already written do not move
    // when the stretch around them is renamed. What a label changes is what the composer will
    // write *next* — a clip generated inside a section draws its material from the label, so
    // two stretches called サビ get recognisably the same figures.

    /// Names the section of the song beginning at the bar `at` falls in.
    ///
    /// Sections snap to bar lines rather than to the editing grid: 「サビはこの小節から」 is
    /// the thing being said, and a section starting mid-bar is not a thing a person means by
    /// pointing. `None` — or a name of nothing but whitespace — leaves the stretch from there
    /// deliberately unnamed, which is how a song's structure ends.
    pub fn set_section(&mut self, at: Ticks, label: Option<String>) {
        self.record(Edit::SetSection);
        let at = self.snap_section(at);
        self.project.sections.set_point(at, label);
    }

    /// Removes the section change in force at `at`, letting the one before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]: a
    /// section is a stretch, and pointing anywhere inside it is pointing at it.
    pub fn remove_section(&mut self, at: Ticks) {
        let Some(at) = self.project.sections.change_at(at.max_zero()) else {
            return;
        };
        self.record(Edit::SetSection);
        self.project.sections.remove_point(at);
    }

    /// Moves the section change in force at `from` to the start of the bar `to` falls in.
    pub fn move_section(&mut self, from: Ticks, to: Ticks) -> bool {
        let Some(from) = self.project.sections.change_at(from.max_zero()) else {
            return false;
        };
        let to = self.snap_section(to);
        if from == to {
            return false;
        }
        self.record(Edit::MoveSection);
        self.project.sections.move_point(from, to)
    }

    /// The start of the bar `at` falls in, which is the only place a section may begin.
    fn snap_section(&self, at: Ticks) -> Ticks {
        self.project.signatures.bar_floor(at)
    }

    /// Writes the catalogue progression called `name`, such as `axis` or `丸サ`.
    ///
    /// `bars` of zero means the chart's own length, which is what "put this progression here"
    /// usually means. A name nothing answers to is an error rather than a quiet no-op — there is
    /// no nearest right answer to a misspelling, and stamping nothing while reporting success is
    /// the one outcome nobody could debug.
    ///
    /// The chart is read against the key in force where it lands, so a major-mode progression
    /// dropped into a minor stretch names its chords from the relative key rather than having
    /// its degrees read literally: the same loop, centred where the music is.
    pub fn stamp_named_progression(
        &mut self,
        name: &str,
        from: Ticks,
        bars: usize,
    ) -> Result<usize, SessionError> {
        let chart =
            catalog(name).ok_or_else(|| SessionError::UnknownProgression(name.to_string()))?;
        let chart = chart.spelled_in(self.project.harmony.key_at(from.max_zero()));
        let bars = if bars == 0 { chart.bar_count() } else { bars };
        Ok(self.stamp_progression(&chart, from, bars))
    }

    /// Rounds a position onto the editing grid, and never before the start of the song.
    fn snap(&self, at: Ticks) -> Ticks {
        at.max_zero().snap_nearest(self.project.grid)
    }

    // ---------------------------------------------------------------- tracks

    /// Appends an instrument track.
    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: &str,
    ) -> Result<TrackId, SessionError> {
        if !self.registry.has_instrument(instrument_id) {
            return Err(SessionError::UnknownPlugin(instrument_id.to_string()));
        }
        self.record(Edit::AddInstrumentTrack);
        let id = self.project.add_instrument_track(name, instrument_id);
        self.invalidate_graph();
        Ok(id)
    }

    /// Appends an instrument track playing whatever the registry nominates as its default.
    pub fn add_default_instrument_track(
        &mut self,
        name: impl Into<String>,
    ) -> Result<TrackId, SessionError> {
        let instrument = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        self.add_instrument_track(name, &instrument)
    }

    /// Appends an audio track.
    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddAudioTrack);
        let id = self.project.add_audio_track(name);
        self.invalidate_graph();
        id
    }

    /// Appends a bus: a mixing point that nothing is routed to yet.
    pub fn add_bus_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddBusTrack);
        let id = self.project.add_bus_track(name);
        self.invalidate_graph();
        id
    }

    /// Points a track's output at a bus, or back at the master.
    ///
    /// Refused when the destination is not a bus, or when the route would make a signal loop back
    /// on itself. Both are checked before anything is recorded: a command that pushes an undo step
    /// and then fails has cost the user a rung that reverses nothing and a redo branch that is
    /// simply gone.
    pub fn set_track_output(&mut self, id: TrackId, output: Output) -> Result<(), SessionError> {
        self.require_track(id)?;
        if let Some(bus) = output.bus() {
            self.require_bus(bus)?;
            if self.project.routing_would_cycle(id, bus) {
                return Err(SessionError::RoutingLoop {
                    from: id.0,
                    to: bus.0,
                });
            }
        }
        if self.project.track(id).is_some_and(|t| t.output == output) {
            return Ok(());
        }
        self.record(Edit::SetTrackOutput);
        if let Some(track) = self.project.track_mut(id) {
            track.output = output;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Adds a post-fader send at unity from `id` to `bus`, returning its new id.
    ///
    /// Unity because a send is added in order to be heard: starting it at silence would make the
    /// first thing every user does be to undo the default.
    pub fn add_send(&mut self, id: TrackId, bus: TrackId) -> Result<SendId, SessionError> {
        self.require_track(id)?;
        self.require_bus(bus)?;
        if self.project.routing_would_cycle(id, bus) {
            return Err(SessionError::RoutingLoop {
                from: id.0,
                to: bus.0,
            });
        }
        self.record(Edit::AddSend);
        let send = self.project.next_send_id();
        if let Some(track) = self.project.track_mut(id) {
            track.sends.push(AuxSend::new(send, bus));
        }
        self.invalidate_graph();
        Ok(send)
    }

    /// Removes a send from a track.
    pub fn remove_send(&mut self, id: TrackId, send: SendId) -> Result<(), SessionError> {
        self.require_send(id, send)?;
        self.record(Edit::RemoveSend);
        self.project.remove_send(id, send);
        self.invalidate_graph();
        Ok(())
    }

    /// Turns a send's level, in decibels.
    ///
    /// A knob rather than a structural edit, so the change travels as a command and the graph is
    /// left where it is. The undo step is [`Edit::AdjustParameter`] over
    /// [`ParamTarget::Send`], the same one [`Self::set_param`] records — so a drag on one send
    /// folds into a single step whichever of the two paths the frontend reached it by, and
    /// turning a *different* send starts a new one.
    pub fn set_send_level(
        &mut self,
        id: TrackId,
        send: SendId,
        level_db: f32,
    ) -> Result<(), SessionError> {
        let (index, position) = self.require_send(id, send)?;
        if !level_db.is_finite() {
            return Err(SessionError::NotFinite(level_db as f64));
        }
        self.record_repeating(Edit::AdjustParameter(ParamTarget::Send { track: id, send }));
        self.project.tracks[index].sends[position].level_db = level_db;
        self.send(EngineCommand::SetSendLevel {
            track: index,
            send: position,
            level_db,
        });
        Ok(())
    }

    /// Moves a send's tap before or after the track's fader.
    ///
    /// Where the copy is taken from is part of the graph's shape rather than a value in it, so
    /// unlike the level this rebuilds.
    pub fn set_send_pre_fader(
        &mut self,
        id: TrackId,
        send: SendId,
        pre_fader: bool,
    ) -> Result<(), SessionError> {
        let (index, position) = self.require_send(id, send)?;
        if self.project.tracks[index].sends[position].pre_fader == pre_fader {
            return Ok(());
        }
        self.record(Edit::SetSendPreFader);
        self.project.tracks[index].sends[position].pre_fader = pre_fader;
        self.invalidate_graph();
        Ok(())
    }

    /// Every bus in the project, for a routing picker.
    pub fn buses(&self) -> impl Iterator<Item = &Track> {
        self.project
            .tracks
            .iter()
            .filter(|track| track.kind.is_bus())
    }

    /// `true` when `id` could be routed into `bus` — as an output or through a send — without the
    /// signal looping back on itself.
    ///
    /// The rule a picker should grey a row out by, worked out here rather than in each frontend:
    /// which destinations are legal is a fact about the document, and a UI that decided it for
    /// itself would eventually disagree with the command that has to enforce it.
    pub fn can_route(&self, id: TrackId, bus: TrackId) -> bool {
        self.project
            .track(bus)
            .is_some_and(|track| track.kind.is_bus())
            && bus != id
            && !self.project.routing_would_cycle(id, bus)
    }

    /// The buses `id` could be routed into without making a loop.
    pub fn available_buses(&self, id: TrackId) -> Vec<TrackId> {
        self.buses()
            .map(|bus| bus.id)
            .filter(|bus| self.can_route(id, *bus))
            .collect()
    }

    /// Removes a track.
    pub fn remove_track(&mut self, id: TrackId) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::DeleteTrack);
        self.project.remove_track(id);
        self.invalidate_graph();
        Ok(())
    }

    /// Moves a track to a new position in the list, clamping into range.
    ///
    /// Structural, so the graph is rebuilt: the engine addresses tracks by *position*, and every
    /// index in flight would otherwise point one track away from what it named. Nothing the
    /// document holds is addressed that way — automation lanes, a routing output and a send all
    /// name a track by id — so the mix survives the move unchanged.
    pub fn move_track(&mut self, id: TrackId, to_index: usize) -> Result<(), SessionError> {
        let from = self.require_track(id)?;
        let to = to_index.min(self.project.tracks.len().saturating_sub(1));
        if from == to {
            return Ok(());
        }
        self.record(Edit::MoveTrack);
        self.project.move_track(id, to);
        self.invalidate_graph();
        Ok(())
    }

    /// Renames a track.
    pub fn rename_track(
        &mut self,
        id: TrackId,
        name: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::RenameTrack);
        if let Some(track) = self.project.track_mut(id) {
            track.name = name.into();
        }
        Ok(())
    }

    /// Tints a track, and the clips on it.
    ///
    /// A new track picks a palette entry by its position, which is a sensible start and a poor
    /// finish: the order tracks were made in has nothing to do with which of them are drums. This
    /// is what makes the colour a choice. Nothing is heard, so the graph is left alone.
    pub fn set_track_color(&mut self, id: TrackId, color: Color) -> Result<(), SessionError> {
        self.require_track(id)?;
        if self
            .project
            .track(id)
            .is_some_and(|track| track.color == color)
        {
            return Ok(());
        }
        self.record(Edit::SetTrackColor);
        if let Some(track) = self.project.track_mut(id) {
            track.color = color;
        }
        Ok(())
    }

    /// Silences or unsilences a track.
    pub fn set_track_mute(&mut self, id: TrackId, mute: bool) -> Result<(), SessionError> {
        let index = self.require_track(id)?;
        self.record(Edit::MuteTrack);
        self.project.tracks[index].mixer.mute = mute;
        self.send(EngineCommand::SetTrackMute { index, mute });
        Ok(())
    }

    /// Solos or unsolos a track.
    ///
    /// Solo decides which *other* tracks are audible, so unlike mute it cannot be expressed as
    /// one per-track command and the graph is rebuilt instead.
    pub fn set_track_solo(&mut self, id: TrackId, solo: bool) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::SoloTrack);
        if let Some(track) = self.project.track_mut(id) {
            track.mixer.solo = solo;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Replaces a track's instrument, discarding the previous plugin's parameter values and the
    /// automation that drove them.
    pub fn set_track_instrument(
        &mut self,
        id: TrackId,
        instrument_id: &str,
    ) -> Result<(), SessionError> {
        if self.registry.descriptor(instrument_id).map(|d| d.kind) != Some(PluginKind::Instrument) {
            return Err(SessionError::UnknownPlugin(instrument_id.to_string()));
        }
        self.require_track(id)?;
        // An audio track has no instrument to change. This used to fall through the `if let`
        // below, do nothing, and report success — after recording an undo step for the edit it
        // had not made, so the history grew a rung that reversed nothing. The guard belongs here
        // rather than in whichever frontend happens to remember: the invariant is the document's.
        if !self
            .project
            .track(id)
            .is_some_and(|track| track.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: id.0,
                actual: "an audio track",
                expected: "an instrument track",
            });
        }
        self.record(Edit::ChangeInstrument);
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            swapped = inner.instrument_id != instrument_id;
            inner.instrument_id = instrument_id.to_string();
            // The saved values belong to the old plugin; applying them to a different one would
            // write another plugin's numbers into unrelated controls.
            inner.instrument_state = PluginState::empty();
        }
        if swapped {
            // And so do the curves that were writing those values every block, for the same
            // reason. After the `record`, so that undo brings the lanes back with the plugin.
            self.project.remove_instrument_automation(id);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Points a track at one of an imported SoundFont's sounds.
    ///
    /// Choosing a sound implies choosing the instrument that makes it, so a track playing
    /// anything else is switched to the sampler as part of the same edit — which is what makes
    /// picking a preset out of a library one gesture rather than two.
    ///
    /// A track already on the sampler keeps its level, reverb and chorus — and the lanes that
    /// drive them: those are how the player is set up, not which sound it is playing, and losing
    /// them every time somebody auditioned a neighbouring preset would be its own small tragedy.
    /// A track arriving from another instrument loses both, because they described that plugin.
    pub fn set_track_preset(&mut self, id: TrackId, preset: PresetRef) -> Result<(), SessionError> {
        self.require_track(id)?;
        if !self
            .project
            .track(id)
            .is_some_and(|track| track.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: id.0,
                actual: "an audio track",
                expected: "an instrument track",
            });
        }
        if !self.project.soundfonts.contains_key(&preset.font) {
            return Err(SessionError::UnknownSoundFont(preset.font.0));
        }
        self.record(Edit::ChoosePreset);
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            if inner.instrument_id != SAMPLER_ID {
                inner.instrument_id = SAMPLER_ID.to_string();
                inner.instrument_state = PluginState::empty();
                swapped = true;
            }
            store_preset(&mut inner.instrument_state, preset);
        }
        if swapped {
            // The track was playing something else a moment ago, so its lanes were addressed to
            // that plugin's parameters. After the `record`, so that undo brings them back.
            self.project.remove_instrument_automation(id);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Which SoundFont sound a track plays, or `None` when it plays something else entirely.
    pub fn track_preset(&self, id: TrackId) -> Option<PresetRef> {
        let inner = self.project.track(id)?.kind.as_instrument()?;
        if inner.instrument_id != SAMPLER_ID {
            return None;
        }
        stored_preset(&inner.instrument_state)
    }

    /// Copies a track, its clips and its whole effect chain, below the original.
    pub fn duplicate_track(&mut self, id: TrackId) -> Result<TrackId, SessionError> {
        self.require_track(id)?;
        self.record(Edit::DuplicateTrack);
        let copy = self
            .project
            .duplicate_track(id)
            .ok_or(SessionError::UnknownTrack(id.0))?;
        self.invalidate_graph();
        Ok(copy)
    }

    /// Sets a track's lane height, for frontends that draw one.
    pub fn set_track_height(&mut self, id: TrackId, height: f32) -> Result<(), SessionError> {
        self.require_track(id)?;
        if let Some(track) = self.project.track_mut(id) {
            // `clamp` would pass a NaN straight into a stored field; the midpoint is as good
            // an answer as any to a height that is not a number.
            track.height = if height.is_finite() {
                height.clamp(24.0, 400.0)
            } else {
                24.0
            };
        }
        Ok(())
    }

    fn require_track(&self, id: TrackId) -> Result<usize, SessionError> {
        self.project
            .track_index(id)
            .ok_or(SessionError::UnknownTrack(id.0))
    }

    /// A track that exists *and* is a mixing point, which is the only thing audio can be sent to.
    fn require_bus(&self, id: TrackId) -> Result<usize, SessionError> {
        let index = self.require_track(id)?;
        match self.project.tracks[index].kind.is_bus() {
            true => Ok(index),
            false => Err(SessionError::NotABus(id.0)),
        }
    }

    /// The track's index and the send's position in its list, both of which the engine addresses
    /// things by.
    fn require_send(&self, id: TrackId, send: SendId) -> Result<(usize, usize), SessionError> {
        let index = self.require_track(id)?;
        let position = self.project.tracks[index]
            .sends
            .iter()
            .position(|existing| existing.id == send)
            .ok_or(SessionError::UnknownSend {
                track: id.0,
                send: send.0,
            })?;
        Ok((index, position))
    }

    /// A unit-range value fit to store: clamped, with non-finite input becoming the floor.
    ///
    /// `clamp` passes NaN through, and this layer owns the promise that what goes into the
    /// document can come back out of it — `serde_json` writes a non-finite float as `null`,
    /// which no `f32` field will ever deserialise again.
    fn finite_unit(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    // ---------------------------------------------------------------- clips

    /// Replaces the document with a composed piece.
    ///
    /// One edit, not several hundred: building the project directly and swapping it in means the
    /// whole piece is a single undo step, and the render graph is rebuilt once rather than once
    /// per note.
    ///
    /// A part naming an instrument the registry does not have falls back to the first registered
    /// one and is reported, because a missing plugin should cost a timbre, not a whole piece.
    pub fn compose(
        &mut self,
        composition: &auris_compose::Composition,
    ) -> Result<ComposeReport, SessionError> {
        let fallback = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();

        let mut project = Project::new(&composition.title, self.project.sample_rate);
        project.set_bpm(composition.tempo);
        // One meter for the whole piece: a specification says `meter: 6/8` once, and the composer
        // has no vocabulary for changing it part way through. Nothing stops the meter being
        // edited afterwards — the document holds a map either way.
        project.signatures = SignatureMap::constant(composition.meter);
        // The harmony and the structure the composer wrote the parts against, rather than an
        // empty lane over a song that plainly has chords. It is not decoration: a clip generated
        // afterwards reads the label and the chords under it, so a part added by hand to a
        // composed song comes out belonging to the same song.
        project.harmony = composition.harmony.clone();
        project.sections = composition.sections.clone();
        // What it was asked for, kept with what it produced. A song sheet reopened after a save
        // and a reload refills itself from this, and Another Take goes on working on a piece
        // nobody has the original `.asong` for.
        project.song_spec = Some(composition.spec.clone());

        let mut report = ComposeReport {
            tracks: 0,
            clips: 0,
            notes: 0,
            length: composition.length,
            substituted: Vec::new(),
        };

        // The buses first, so that the tracks routed into them have somewhere to land. They are
        // *added* first and then moved below the parts, because the arrangement reads better as
        // the music followed by the places it is mixed at — and a bus above its feeders would be
        // the first thing a person clicked on.
        let mut buses: Vec<TrackId> = Vec::new();
        for bus in &composition.buses {
            let id = project.add_bus_track(&bus.name);
            if let Some(entry) = project.track_mut(id) {
                entry.color = bus.color;
                entry.mixer.gain_db = bus.gain_db;
            }
            for effect in &bus.effects {
                if !self.registry.has_effect(&effect.id) {
                    // The same trade the instruments get: a missing plugin costs a colour, not a
                    // piece. Without the reverb the room bus is a plain sum, which is quiet and
                    // harmless rather than wrong.
                    report.substituted.push(effect.id.clone());
                    continue;
                }
                let Some(slot) = project.add_effect(Some(id), &effect.id) else {
                    continue;
                };
                if let Some(entry) = project.track_mut(id)
                    && let Some(added) = entry.mixer.effects.iter_mut().find(|s| s.id == slot)
                {
                    added.state = effect.state.clone();
                }
            }
            buses.push(id);
        }

        // The General MIDI font, once, and only if a part actually asked for a sound out of it.
        // A piece written entirely on the built-in voices should not carry a two-hundred-megabyte
        // reference it never plays.
        let general_midi = composition
            .tracks
            .iter()
            .any(|track| track.sound.is_some())
            .then(|| self.adopt_general_midi(&mut project))
            .flatten();
        if general_midi.is_none() && composition.tracks.iter().any(|t| t.sound.is_some()) {
            // Named the way a missing plugin is, because it is the same thing happening: the
            // piece plays, on the instruments the parts also name, and the report is where
            // somebody finds out why it sounds like an oscillator.
            report.substituted.push("General MIDI".to_string());
        }

        for track in &composition.tracks {
            let sound = general_midi.and(track.sound);
            let instrument = match &sound {
                // Choosing a sound is choosing the instrument that makes it, exactly as it is in
                // `set_track_preset`.
                Some(_) => SAMPLER_ID.to_string(),
                None if self.registry.has_instrument(&track.instrument) => track.instrument.clone(),
                None => {
                    report.substituted.push(track.instrument.clone());
                    fallback.clone()
                }
            };
            let track_id = project.add_instrument_track(&track.name, instrument);
            if let Some((sound, font)) = sound.zip(general_midi)
                && let Some(inner) = project
                    .track_mut(track_id)
                    .and_then(|entry| entry.kind.as_instrument_mut())
            {
                store_preset(
                    &mut inner.instrument_state,
                    PresetRef {
                        font,
                        bank: i32::from(sound.bank),
                        patch: i32::from(sound.patch),
                    },
                );
            }
            if let Some(entry) = project.track_mut(track_id) {
                // The composer's colour, not the palette's. `add_instrument_track` takes the next
                // palette entry by position, so which colour a part got depended on how many
                // parts were declared before it.
                entry.color = track.color;
                entry.mixer.gain_db = track.gain_db;
                entry.mixer.pan = track.pan;
                // A draft names a bus by its position in the composition, because the composer has
                // no ids to name it by; this is where the two meet.
                entry.output = track
                    .output
                    .and_then(|index| buses.get(index))
                    .map_or(Output::Master, |bus| Output::Bus(*bus));
            }
            for send in &track.sends {
                let Some(bus) = buses.get(send.bus).copied() else {
                    continue;
                };
                let id = project.next_send_id();
                if let Some(entry) = project.track_mut(track_id) {
                    entry.sends.push(AuxSend {
                        id,
                        target: bus,
                        level_db: send.level_db,
                        pre_fader: false,
                    });
                }
            }
            report.tracks += 1;

            for clip in &track.clips {
                let Some(clip_id) = project.add_midi_clip(
                    track_id,
                    &clip.name,
                    clip.start,
                    Ticks(clip.length.raw().max(1)),
                ) else {
                    continue;
                };
                if let Some(target) = project.midi_clip_mut(clip_id) {
                    target.notes = clip.notes.clone();
                    report.notes += clip.notes.len();
                }
                report.clips += 1;
            }
        }

        // The buses to the bottom, where a mixing point belongs. Moved rather than created there,
        // because a send cannot name a bus that does not exist yet.
        for bus in &buses {
            project.move_track(*bus, project.tracks.len());
        }

        // Loop over the whole piece, so pressing play and leaving it running plays the song.
        project.loop_region = Some((Ticks::ZERO, composition.length));
        project.loop_enabled = false;

        self.record(Edit::Compose);
        self.replace_project(project);
        self.install_shipped_fonts();
        self.dirty = true;
        Ok(report)
    }

    // --------------------------------------------------- the curves on a clip

    /// Writes a point on one of a clip's curves, replacing whatever was at that instant.
    ///
    /// Kept in time order here rather than wherever a curve is read, because everything that
    /// reads one — the renderer, the MIDI writer, the roll — assumes it: a point out of order
    /// would draw a line backwards and schedule a jump.
    ///
    /// `which` is the only thing that tells the bend from the modulation anywhere in this crate.
    /// They are the same shape and obey the same rules, and two copies of these four commands
    /// would be two chances for the wheel to behave differently from the bend for no reason
    /// anybody could see.
    pub fn set_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        at: Ticks,
        value: f32,
    ) -> bool {
        let at = at.max_zero();
        let (low, high) = which.range();
        let value = value.clamp(low, high);
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return false;
        };
        let at = at.min(target.length);
        if target
            .curve(which)
            .iter()
            .any(|point| point.at == at && point.value == value)
        {
            return false;
        }
        self.record(Edit::write_curve(which, clip));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            let points = target.curve_mut(which);
            points.retain(|point| point.at != at);
            points.push(CurvePoint { at, value });
            points.sort_by_key(|point| point.at);
        }
        self.invalidate_graph();
        true
    }

    /// Moves a point along a curve, taking a new value with it.
    ///
    /// Returns where it landed, which is not always where it was asked to go: dropped onto another
    /// point it replaces that one, since one instant cannot hold two values. A drag wants
    /// [`Self::begin_transaction`] around the whole gesture, the way every other drag does.
    pub fn move_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Option<Ticks> {
        let (low, high) = which.range();
        let value = value.clamp(low, high);
        let length = self.project.midi_clip(clip)?.1.length;
        let to = to.max_zero().min(length);
        let held = self
            .project
            .midi_clip(clip)?
            .1
            .curve(which)
            .iter()
            .find(|point| point.at == from)
            .copied()?;
        if held.at == to && held.value == value {
            return Some(to);
        }
        self.record(Edit::write_curve(which, clip));
        let target = self.project.midi_clip_mut(clip)?;
        let points = target.curve_mut(which);
        points.retain(|point| point.at != from && point.at != to);
        points.push(CurvePoint { at: to, value });
        points.sort_by_key(|point| point.at);
        self.invalidate_graph();
        Some(to)
    }

    /// Takes one point off a curve.
    pub fn remove_curve_point(&mut self, clip: ClipId, which: ClipCurve, at: Ticks) -> bool {
        if !self
            .project
            .midi_clip(clip)
            .is_some_and(|target| target.1.curve(which).iter().any(|point| point.at == at))
        {
            return false;
        }
        self.record(Edit::erase_curve(which));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.curve_mut(which).retain(|point| point.at != at);
        }
        self.invalidate_graph();
        true
    }

    /// Straightens a clip out, removing one of its curves entirely.
    pub fn clear_curve(&mut self, clip: ClipId, which: ClipCurve) -> bool {
        if !self
            .project
            .midi_clip(clip)
            .is_some_and(|target| !target.1.curve(which).is_empty())
        {
            return false;
        }
        self.record(Edit::erase_curve(which));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.curve_mut(which).clear();
        }
        self.invalidate_graph();
        true
    }

    /// Adds an empty MIDI clip to an instrument track.
    pub fn add_midi_clip(
        &mut self,
        track: TrackId,
        name: impl Into<String>,
        start: Ticks,
        length: Ticks,
    ) -> Result<ClipId, SessionError> {
        let index = self.require_track(track)?;
        if self.project.tracks[index].kind.as_instrument().is_none() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                // The track's own word for itself, rather than "an audio track" — which was true
                // of the only other kind there used to be, and is a lie about a bus.
                actual: self.project.tracks[index].kind.label(),
                expected: "an instrument track",
            });
        }
        self.record(Edit::AddClip);
        let id = self
            .project
            .add_midi_clip(track, name, start.max_zero(), Ticks(length.raw().max(1)))
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.invalidate_graph();
        Ok(id)
    }

    // ------------------------------------------------------- clips that write themselves

    /// Writes a clip on `track` from the harmony underneath it.
    ///
    /// The clip keeps its recipe, so it can be written again after the chords change or with a
    /// different feel. Its notes are stored like anybody else's: the engine, the exporter and the
    /// piano roll never learn that a composer was involved.
    ///
    /// A range with no chords under it produces an empty clip rather than an error. That is the
    /// honest answer — there was nothing to play — and it leaves something on the timeline to
    /// aim the next progression at.
    pub fn generate_clip(
        &mut self,
        track: TrackId,
        start: Ticks,
        length: Ticks,
        recipe: ClipRecipe,
    ) -> Result<ClipId, SessionError> {
        let index = self.require_track(track)?;
        if self.project.tracks[index].kind.as_instrument().is_none() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: "an audio track",
                expected: "an instrument track",
            });
        }
        let start = self.snap(start);
        let length = Ticks(length.raw().max(1));
        let notes = self.phrase(start, length, &recipe);

        self.record(Edit::GenerateClip);
        let id = self
            .project
            .add_midi_clip(track, recipe.preset.name(), start, length)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if let Some(clip) = self.project.midi_clip_mut(id) {
            clip.notes = notes;
            clip.recipe = Some(recipe);
        }
        self.invalidate_graph();
        Ok(id)
    }

    /// Writes a generated clip's notes again from its own recipe, and returns how many there are.
    ///
    /// With the harmony unchanged this writes the same notes back, which is what makes it safe to
    /// press. What it is for is the other case: the chords underneath moved, and the part should
    /// follow them.
    pub fn regenerate_clip(&mut self, clip: ClipId) -> Result<usize, SessionError> {
        let recipe = self.recipe_of(clip)?;
        self.rewrite(clip, recipe)
    }

    /// Writes another take of a generated clip, and returns how many notes it has.
    ///
    /// The next seed rather than a random one, so pressing it twice from the same starting point
    /// lands in the same place and a take somebody liked can be got back to.
    pub fn reroll_clip(&mut self, clip: ClipId) -> Result<usize, SessionError> {
        let recipe = self.recipe_of(clip)?;
        let next = recipe.seed.wrapping_add(1);
        self.rewrite(clip, recipe.with_seed(next))
    }

    /// Replaces a generated clip's recipe and writes its notes again.
    pub fn set_clip_recipe(
        &mut self,
        clip: ClipId,
        recipe: ClipRecipe,
    ) -> Result<usize, SessionError> {
        self.recipe_of(clip)?;
        self.rewrite(clip, recipe)
    }

    /// Drops a clip's recipe, leaving its notes exactly where they are.
    ///
    /// What "keep this one" means. The notes stop being derived from anything, so nothing can
    /// rewrite them afterwards — which is the point.
    pub fn freeze_clip(&mut self, clip: ClipId) -> Result<(), SessionError> {
        self.recipe_of(clip)?;
        self.record(Edit::FreezeClip);
        if let Some(clip) = self.project.midi_clip_mut(clip) {
            clip.recipe = None;
        }
        Ok(())
    }

    /// Drops every recipe on a track, and returns how many clips stopped being generated.
    pub fn freeze_track(&mut self, track: TrackId) -> Result<usize, SessionError> {
        let index = self.require_track(track)?;
        let Some(instrument) = self.project.tracks[index].kind.as_instrument() else {
            return Ok(0);
        };
        let generated = instrument
            .clips
            .iter()
            .filter(|clip| clip.is_generated())
            .count();
        if generated == 0 {
            return Ok(0);
        }
        self.record(Edit::FreezeClip);
        if let Some(instrument) = self.project.tracks[index].kind.as_instrument_mut() {
            for clip in &mut instrument.clips {
                clip.recipe = None;
            }
        }
        Ok(generated)
    }

    /// The recipe a clip was written from.
    pub fn clip_recipe(&self, clip: ClipId) -> Option<&ClipRecipe> {
        self.project.midi_clip(clip)?.1.recipe.as_ref()
    }

    /// The recipe of a clip that has one, or the reason it has not.
    fn recipe_of(&self, clip: ClipId) -> Result<ClipRecipe, SessionError> {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        midi.recipe
            .clone()
            .ok_or(SessionError::NotGenerated(clip.0))
    }

    /// Writes `recipe` onto `clip` and replaces its notes with what that recipe says.
    fn rewrite(&mut self, clip: ClipId, recipe: ClipRecipe) -> Result<usize, SessionError> {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let (start, length) = (midi.start, midi.length);
        let notes = self.phrase(start, length, &recipe);
        let written = notes.len();

        self.record(Edit::GenerateClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.notes = notes;
            midi.recipe = Some(recipe);
        }
        self.invalidate_graph();
        Ok(written)
    }

    /// The notes a recipe writes over a stretch of this document's harmony.
    ///
    /// The section under the clip's start travels along as the composer's hint: two clips
    /// written into stretches with the same label draw the same figures, which is what makes
    /// the second サビ recognisably the first.
    fn phrase(&self, start: Ticks, length: Ticks, recipe: &ClipRecipe) -> Vec<Note> {
        auris_compose::write_phrase(
            &self.project.harmony,
            start,
            length,
            // The meter the clip begins in. `write_phrase` builds every figure on one grid, so a
            // clip is written in one meter however many the timeline holds.
            self.project.signatures.signature_at(start),
            recipe,
            self.project.sections.section_at(start),
        )
    }

    /// Removes a clip of either kind.
    pub fn remove_clip(&mut self, clip: ClipId) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() && !self.audio_clip_exists(clip) {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::DeleteClip);
        self.project.remove_clip(clip);
        self.invalidate_graph();
        Ok(())
    }

    /// Which track a clip sits on.
    pub fn track_of_clip(&self, clip: ClipId) -> Option<TrackId> {
        self.project.track_of_clip(clip)
    }

    /// `true` when `clip` could be moved onto `track`.
    ///
    /// Asked before a move rather than discovered during one, so a pointer drag can refuse a
    /// lane it cannot land on instead of dropping half a selection onto it.
    pub fn clip_fits_track(&self, clip: ClipId, track: TrackId) -> bool {
        let Some(source) = self.project.track_of_clip(clip) else {
            return false;
        };
        let kind_of = |id: TrackId| {
            self.project
                .track(id)
                .map(|track| track.kind.is_instrument())
        };
        match (kind_of(source), kind_of(track)) {
            (Some(from), Some(to)) => from == to,
            _ => false,
        }
    }

    /// Moves clips onto another track, keeping their positions.
    ///
    /// Every clip or none: a selection dragged across lanes is one gesture, and landing half of
    /// it on the new track and leaving the rest behind is not what dropping it meant. The whole
    /// move is refused when any clip does not belong on its destination.
    pub fn move_clips_to_track(&mut self, clips: &[(ClipId, TrackId)]) -> Result<(), SessionError> {
        if clips.is_empty() {
            return Ok(());
        }
        for (clip, track) in clips {
            self.require_clip(*clip)?;
            self.require_track(*track)?;
            if !self.clip_fits_track(*clip, *track) {
                return Err(SessionError::UnknownTrack(track.0));
            }
        }
        // Nothing to record when every clip is already where it is being sent, which is what a
        // pointer drag asks for on most of its moves.
        if clips
            .iter()
            .all(|(clip, track)| self.project.track_of_clip(*clip) == Some(*track))
        {
            return Ok(());
        }
        self.record(Edit::MoveClip);
        for (clip, track) in clips {
            self.project.move_clip_to_track(*clip, *track);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Copies a clip onto its own track, immediately after the original.
    pub fn duplicate_clip(&mut self, clip: ClipId) -> Result<ClipId, SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::DuplicateClip);
        let copy = self
            .project
            .duplicate_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        self.invalidate_graph();
        Ok(copy)
    }

    /// Divides a clip in two at a timeline position, returning the right-hand piece.
    pub fn split_clip(&mut self, clip: ClipId, at: Ticks) -> Result<ClipId, SessionError> {
        self.require_clip(clip)?;
        // The split is attempted before any history is recorded. A position outside the clip
        // leaves the document untouched, and an undo step that undoes nothing visible is worse
        // than no step at all.
        let before = self.project.clone();
        let Some(right) = self.project.split_clip(clip, at) else {
            return Err(SessionError::CannotSplit(clip.0));
        };
        if self.transaction.is_none() {
            self.history.push(Edit::SplitClip, &before);
        }
        self.dirty = true;
        self.invalidate_graph();
        Ok(right)
    }

    /// Renames a clip of either kind.
    pub fn rename_clip(
        &mut self,
        clip: ClipId,
        name: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::RenameClip);
        let name = name.into();
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.name = name;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.name = name;
        }
        Ok(())
    }

    /// Silences or unsilences a single clip.
    pub fn set_clip_muted(&mut self, clip: ClipId, muted: bool) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::MuteClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.muted = muted;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.muted = muted;
        }
        self.invalidate_graph();
        Ok(())
    }

    fn require_clip(&self, clip: ClipId) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_some() || self.audio_clip_exists(clip) {
            Ok(())
        } else {
            Err(SessionError::UnknownClip(clip.0))
        }
    }

    /// Removes several clips as one edit.
    ///
    /// Ids that do not exist are ignored, so a stale selection cannot fail the whole delete.
    pub fn remove_clips(&mut self, clips: &[ClipId]) -> Result<(), SessionError> {
        let present: Vec<ClipId> = clips
            .iter()
            .copied()
            .filter(|clip| self.require_clip(*clip).is_ok())
            .collect();
        if present.is_empty() {
            return Ok(());
        }
        self.record(Edit::DeleteClip);
        for clip in present {
            self.project.remove_clip(clip);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Moves several clips by one delta, from positions captured before the gesture began.
    ///
    /// The delta is clamped so that the earliest clip lands on zero rather than each clip being
    /// clamped separately — that would pile the leading clips on top of each other and quietly
    /// destroy the spacing the user is dragging.
    pub fn move_clips(&mut self, origins: &[(ClipId, Ticks)], delta: Ticks) {
        // Only clips that still exist: a selection can outlive an undo, and a gesture over
        // nothing must not record a step over nothing.
        let present: Vec<(ClipId, Ticks)> = origins
            .iter()
            .copied()
            .filter(|(clip, _)| self.require_clip(*clip).is_ok())
            .collect();
        let Some(earliest) = present.iter().map(|(_, start)| *start).min() else {
            return;
        };
        let delta = delta.max(-earliest);
        self.record(Edit::MoveClip);
        for (clip, start) in present {
            let start = (start + delta).max_zero();
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.start = start;
            } else if let Some(audio) = self.project.audio_clip_mut(clip) {
                audio.start = start;
            }
        }
        self.invalidate_graph();
    }

    /// Moves a clip of either kind to a new start position.
    pub fn move_clip(&mut self, clip: ClipId, start: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        self.record(Edit::MoveClip);
        let start = start.max_zero();
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.start = start;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.start = start;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags a clip's end to `end`.
    ///
    /// The three cases answer differently, because a length means a different thing to each.
    ///
    /// A **generated** clip is its recipe rather than its notes: the notes were written to fill a
    /// length, so a new length gets them written again. Dragged out it fills the bars it gained
    /// instead of trailing silence; dragged in it stops where it stops instead of keeping notes
    /// hanging past its own end. Nothing is lost by it, because the recipe still says what the
    /// clip is and dragging back out writes the material back. Dragged shorter than a bar it has
    /// no bars to write and comes out empty, which is the honest reading of "this part, this
    /// long".
    ///
    /// A clip somebody **played** keeps every note exactly where it is. Its notes are derived
    /// from nothing, so there is nothing to derive them from again, and inventing or discarding
    /// one would be editing work the resize was not aimed at.
    ///
    /// An **audio** clip is a trim, and a trim stops where the material does.
    pub fn resize_clip(&mut self, clip: ClipId, end: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        let grid = self.project.grid;

        if let Some((start, recipe)) = self
            .project
            .midi_clip(clip)
            .map(|(_, midi)| (midi.start, midi.recipe.clone()))
        {
            let length = (end - start).max(grid);
            // Written before anything is recorded, so the length and the notes land in the one
            // undo step the drag opened rather than in two.
            let notes = recipe
                .as_ref()
                .map(|recipe| self.phrase(start, length, recipe));
            self.record(Edit::ResizeClip);
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.length = length;
                // The length is now the user's, so nothing grows it back. A clip dragged shorter
                // to hide a tail used to reappear at full length on the next note edit.
                midi.length_is_explicit = true;
                if let Some(notes) = notes {
                    midi.notes = notes;
                }
            }
            self.invalidate_graph();
            return Ok(());
        }

        // An audio clip's length lives in source frames, so the dragged tick has to go back
        // through the tempo map rather than being stored as ticks.
        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        let Some(audio) = self.project.audio_clip(clip) else {
            return Ok(());
        };
        // What the source has left past the clip's own offset into it. Unbounded, the edge
        // dragged into a stretch of silence that the clip drew and saved with its waveform
        // stopping part way — and that the renderer clamped on the way to the speakers anyway,
        // so the picture and the sound disagreed.
        let available = self
            .project
            .audio_sources
            .get(&audio.source)
            .map_or(u64::MAX, |source| {
                source.frame_count.saturating_sub(audio.offset_frames)
            });
        let start_seconds = tempo.ticks_to_seconds(audio.start).0;
        let end_seconds = tempo.ticks_to_seconds(end).0;
        let asked = ((end_seconds - start_seconds).max(0.0) * sample_rate) as u64;
        let length = asked.clamp(1, available.max(1));
        if length == audio.length_frames {
            // A drag that has run out of material still moves the pointer, and every frame of it
            // arrives here saying the same thing. Not an edit.
            return Ok(());
        }
        self.record(Edit::ResizeClip);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.length_frames = length;
            // The fades keep fitting inside the clip as it shrinks, under the same rule
            // `set_clip_fades` writes them by: the fade-in keeps its place and the fade-out
            // takes what is left.
            audio.fade_in_frames = audio.fade_in_frames.min(audio.length_frames);
            audio.fade_out_frames = audio
                .fade_out_frames
                .min(audio.length_frames - audio.fade_in_frames);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags a clip's *start* to `start`, keeping its end where it is.
    ///
    /// The other half of [`Self::resize_clip`], and it answers the three cases the same way. A
    /// generated clip is written again over the stretch it now covers. An audio clip walks its
    /// offset into the source along with its start, which is what makes this a trim rather than
    /// a move: the material under the clip stays where it sounds, and dragging the edge back out
    /// uncovers what was hidden rather than repeating what is left. A played clip's notes are
    /// rebased onto the new start, keeping the sounding half of anything the trim runs through —
    /// the rule a split already follows.
    ///
    /// Both ends are bounded by what there is. An audio clip's front stops at the first frame of
    /// its source, and neither kind may be dragged past its own end. A played or generated clip
    /// that is already shorter than the editing grid has nothing left to give and refuses to be
    /// shortened from the front at all — it can still be dragged the other way, which lengthens
    /// it.
    pub fn trim_clip_start(&mut self, clip: ClipId, start: Ticks) -> Result<(), SessionError> {
        self.require_clip(clip)?;
        let grid = self.project.grid;

        if let Some((was, length, recipe)) = self
            .project
            .midi_clip(clip)
            .map(|(_, midi)| (midi.start, midi.length, midi.recipe.clone()))
        {
            // Never past its own end: the clip keeps at least a grid division, which is the same
            // floor the other edge stops at. A clip that is *already* shorter than a division —
            // a piece of a split, anything drawn at a finer grid than the one now set — has no
            // room under that floor, and `was + length - grid` falls behind `was`. Clamped to
            // `was` it simply refuses to be shortened from the front, instead of being dragged
            // leftwards into a lengthening nobody asked for and, in the first bar, a start
            // before zero. Dragging the other way still lengthens it: it is only the shortening
            // that has nowhere to go.
            let now = start.max_zero().min((was + length - grid).max(was));
            let by = now - was;
            if by == Ticks::ZERO {
                return Ok(());
            }
            let length = length - by;
            let notes = match &recipe {
                Some(recipe) => self.phrase(now, length, recipe),
                None => self
                    .project
                    .midi_clip(clip)
                    .map(|(_, midi)| auris_core::notes_trimmed_from_front(&midi.notes, by))
                    .unwrap_or_default(),
            };
            self.record(Edit::ResizeClip);
            if let Some(midi) = self.project.midi_clip_mut(clip) {
                midi.start = now;
                midi.length = length;
                midi.length_is_explicit = true;
                midi.notes = notes;
            }
            self.invalidate_graph();
            return Ok(());
        }

        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        let Some(audio) = self.project.audio_clip(clip) else {
            return Ok(());
        };
        let (was, offset, length) = (audio.start, audio.offset_frames, audio.length_frames);
        let was_seconds = tempo.ticks_to_seconds(was).0;
        let asked = ((tempo.ticks_to_seconds(start.max_zero()).0 - was_seconds) * sample_rate)
            .round() as i64;
        // How far back the edge can go: to the source's first frame, or to the start of the
        // timeline, whichever it meets first. The second bound matters for a clip that was
        // trimmed and then moved left — its window still has material behind it, but there is
        // nowhere on the timeline to put it, and clamping the *tick* alone would leave the start
        // at bar one while the window kept walking and the far end slid right.
        let head_room = (was_seconds * sample_rate).round() as i64;
        let back = (offset as i64).min(head_room.max(0));
        // Forward, to one frame short of the clip's own end. The *clamped* delta is what moves the
        // start, so an edge that has run out of material stops instead of sliding on with the
        // pointer and leaving the sound behind.
        let by = asked.clamp(-back, length as i64 - 1);
        if by == 0 {
            return Ok(());
        }
        let now = tempo.seconds_to_ticks(Seconds(was_seconds + by as f64 / sample_rate.max(1.0)));
        self.record(Edit::ResizeClip);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.start = now.max_zero();
            audio.offset_frames = (offset as i64 + by) as u64;
            audio.length_frames = (length as i64 - by) as u64;
            audio.fade_in_frames = audio.fade_in_frames.min(audio.length_frames);
            audio.fade_out_frames = audio
                .fade_out_frames
                .min(audio.length_frames - audio.fade_in_frames);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Sets an audio clip's own gain, in decibels.
    ///
    /// Clip gain is the clip's, not the track's: it travels with the clip when it moves, and it
    /// is applied before the track's effect chain, which is what makes it the tool for evening
    /// out a loud take against its neighbours. Clamped to −60…+24 dB; a non-finite value is
    /// refused outright.
    pub fn set_clip_gain(&mut self, clip: ClipId, gain_db: f32) -> Result<(), SessionError> {
        if !gain_db.is_finite() {
            return Err(SessionError::NotFinite(f64::from(gain_db)));
        }
        let gain_db = gain_db.clamp(-60.0, 24.0);
        if self.require_audio_clip(clip)?.gain_db == gain_db {
            return Ok(());
        }
        self.record(Edit::SetClipGain);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.gain_db = gain_db;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Sets an audio clip's fades, in frames of its source.
    ///
    /// The fade-in is clamped to the clip and the fade-out to what the fade-in leaves, so the
    /// two can meet but never cross — crossed fades would multiply into a dip no hand drew.
    /// [`auris_core::AudioClip::fade_gain_at`] is the shape, shared by playback and by
    /// whatever a frontend draws.
    pub fn set_clip_fades(
        &mut self,
        clip: ClipId,
        fade_in: u64,
        fade_out: u64,
    ) -> Result<(), SessionError> {
        let current = self.require_audio_clip(clip)?;
        let length = current.length_frames;
        let fade_in = fade_in.min(length);
        let fade_out = fade_out.min(length - fade_in);
        if current.fade_in_frames == fade_in && current.fade_out_frames == fade_out {
            return Ok(());
        }
        self.record(Edit::SetClipFade);
        if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.fade_in_frames = fade_in;
            audio.fade_out_frames = fade_out;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// The audio clip called `clip`, or the error saying what was addressed instead.
    fn require_audio_clip(&self, clip: ClipId) -> Result<&auris_core::AudioClip, SessionError> {
        let found = self
            .project
            .tracks
            .iter()
            .find_map(|track| track.kind.as_audio()?.clips.iter().find(|c| c.id == clip));
        match found {
            Some(audio) => Ok(audio),
            None if self.project.midi_clip(clip).is_some() => Err(SessionError::NotAudio(clip.0)),
            None => Err(SessionError::UnknownClip(clip.0)),
        }
    }

    fn audio_clip_exists(&self, clip: ClipId) -> bool {
        self.project.tracks.iter().any(|track| {
            track
                .kind
                .as_audio()
                .is_some_and(|inner| inner.clips.iter().any(|c| c.id == clip))
        })
    }

    /// Length of an audio clip on the musical timeline.
    pub fn audio_clip_length_ticks(&self, clip: &auris_core::AudioClip) -> Ticks {
        self.project.audio_clip_length_ticks(clip)
    }

    // ---------------------------------------------------------------- notes

    /// Adds a note to a MIDI clip, returning its index.
    pub fn add_note(&mut self, clip: ClipId, note: Note) -> Result<usize, SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::AddNote);
        let grid = self.project.grid;
        let mut index = 0;
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.notes.push(Note {
                pitch: note.pitch.min(127),
                velocity: Self::finite_unit(note.velocity),
                start: note.start.max_zero(),
                length: Ticks(note.length.raw().max(1)),
            });
            target.fit_length_to_notes(grid);
            index = target.notes.len() - 1;
        }
        self.invalidate_graph();
        Ok(index)
    }

    /// Removes notes by index. Indices that do not exist are ignored.
    pub fn remove_notes(&mut self, clip: ClipId, indices: &[usize]) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        if indices.is_empty() {
            return Ok(());
        }
        self.record(Edit::DeleteNotes);
        let mut doomed: Vec<usize> = indices.to_vec();
        doomed.sort_unstable();
        doomed.dedup();
        if let Some(target) = self.project.midi_clip_mut(clip) {
            // Remove from the back so the earlier indices stay valid.
            for index in doomed.into_iter().rev() {
                if index < target.notes.len() {
                    target.notes.remove(index);
                }
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Copies notes, offset by the length of the selection, and returns the copies' indices.
    ///
    /// Offsetting by the whole selection rather than by one note's length is what makes
    /// repeated duplication chain a figure end to end instead of piling copies on top of it.
    pub fn duplicate_notes(
        &mut self,
        clip: ClipId,
        indices: &[usize],
    ) -> Result<Vec<usize>, SessionError> {
        let Some(target) = self.project.midi_clip(clip).map(|(_, clip)| clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let chosen: Vec<Note> = indices
            .iter()
            .filter_map(|index| target.notes.get(*index).copied())
            .collect();
        if chosen.is_empty() {
            return Ok(Vec::new());
        }
        let first = chosen
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or_default();
        let last = chosen.iter().map(Note::end).max().unwrap_or_default();
        let offset = last - first;

        self.record(Edit::DuplicateNotes);
        let grid = self.project.grid;
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let base = target.notes.len();
        for note in chosen {
            target.notes.push(Note {
                start: note.start + offset,
                ..note
            });
        }
        target.fit_length_to_notes(grid);
        let copies = (base..target.notes.len()).collect();
        self.invalidate_graph();
        Ok(copies)
    }

    /// Sets how hard the named notes are struck, from 0 to 1.
    ///
    /// The piano roll has painted a velocity heat map since it was written, and there was no
    /// command that could change what it was showing: the one thing the colour said about a note
    /// was the one thing about it nobody could edit.
    pub fn set_note_velocity(
        &mut self,
        clip: ClipId,
        indices: &[usize],
        velocity: f32,
    ) -> Result<(), SessionError> {
        let changes: Vec<(usize, f32)> = indices.iter().map(|index| (*index, velocity)).collect();
        self.set_note_velocities(clip, &changes)
    }

    /// Sets how hard individual notes are struck, each to a value of its own from 0 to 1.
    ///
    /// The per-note form is what a *gesture* needs. Dragging the dynamics of a chord has to keep
    /// the differences between its notes — a phrase written soft-loud-soft is still soft-loud-soft
    /// once it is played harder — so every note in the selection lands somewhere different, and
    /// one value for all of them cannot say that.
    ///
    /// An index that names no note is skipped rather than refused: a selection is held by
    /// position, and half a chord going through is better than a whole gesture failing because
    /// one note was deleted underneath it.
    pub fn set_note_velocities(
        &mut self,
        clip: ClipId,
        changes: &[(usize, f32)],
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        // Nothing to record for a set that changes nothing — a menu applied to a chord already at
        // that level should not push an undo step, and neither should a drag that has not yet
        // travelled far enough to move a note off the value it started on.
        let unchanged = self
            .project
            .midi_clip(clip)
            .map(|(_, target)| {
                changes.iter().all(|(index, velocity)| {
                    target
                        .notes
                        .get(*index)
                        .is_none_or(|note| note.velocity == velocity.clamp(0.0, 1.0))
                })
            })
            .unwrap_or(true);
        if unchanged {
            return Ok(());
        }

        self.record(Edit::SetNoteVelocity);
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        for (index, velocity) in changes {
            if let Some(note) = target.notes.get_mut(*index) {
                note.velocity = Self::finite_unit(*velocity);
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Shifts notes in pitch, keeping the intervals between them.
    ///
    /// The whole selection moves by the same amount or not at all: clamping each note to the
    /// MIDI range separately would silently flatten a chord into a cluster.
    pub fn transpose_notes(
        &mut self,
        clip: ClipId,
        indices: &[usize],
        semitones: i32,
    ) -> Result<(), SessionError> {
        let Some(target) = self.project.midi_clip(clip).map(|(_, clip)| clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let pitches: Vec<u8> = indices
            .iter()
            .filter_map(|index| target.notes.get(*index).map(|note| note.pitch))
            .collect();
        let (Some(lowest), Some(highest)) =
            (pitches.iter().min().copied(), pitches.iter().max().copied())
        else {
            return Ok(());
        };
        let shift = semitones.max(-(lowest as i32)).min(127 - highest as i32);
        if shift == 0 {
            return Ok(());
        }

        self.record(Edit::TransposeNotes);
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        for index in indices {
            if let Some(note) = target.notes.get_mut(*index) {
                note.pitch = (note.pitch as i32 + shift).clamp(0, 127) as u8;
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Moves notes by a tick and pitch delta, from positions captured before the gesture began.
    pub fn move_notes(
        &mut self,
        clip: ClipId,
        origins: &[(usize, Ticks, u8)],
        delta_ticks: Ticks,
        delta_pitch: i32,
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::MoveNotes);
        let grid = self.project.grid;
        if let Some(target) = self.project.midi_clip_mut(clip) {
            for (index, start, pitch) in origins {
                if let Some(note) = target.notes.get_mut(*index) {
                    note.start = (*start + delta_ticks).max_zero();
                    note.pitch = (*pitch as i32 + delta_pitch).clamp(0, 127) as u8;
                }
            }
            target.fit_length_to_notes(grid);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags one note's end to `end`, clip-relative.
    pub fn resize_note(
        &mut self,
        clip: ClipId,
        index: usize,
        end: Ticks,
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::ResizeNote);
        let grid = Ticks(self.project.grid.raw().max(1));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            if let Some(note) = target.notes.get_mut(index) {
                note.length = (end - note.start).max(grid);
            }
            target.fit_length_to_notes(grid);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// A MIDI clip anywhere in the project.
    pub fn midi_clip(&self, clip: ClipId) -> Option<&MidiClip> {
        self.project.midi_clip(clip).map(|(_, clip)| clip)
    }

    // ---------------------------------------------------------------- effects

    /// Adds an effect to a track's chain, or to the master bus when `track` is `None`.
    pub fn add_effect(
        &mut self,
        track: Option<TrackId>,
        effect_id: &str,
    ) -> Result<EffectSlotId, SessionError> {
        if !self.registry.has_effect(effect_id) {
            return Err(SessionError::UnknownPlugin(effect_id.to_string()));
        }
        if let Some(id) = track {
            self.require_track(id)?;
        }
        self.record(Edit::AddEffect);
        let slot = self
            .project
            .add_effect(track, effect_id)
            .ok_or_else(|| SessionError::UnknownTrack(track.map_or(0, |t| t.0)))?;
        self.invalidate_graph();
        Ok(slot)
    }

    /// Removes an effect from wherever it is.
    pub fn remove_effect(&mut self, slot: EffectSlotId) {
        // Look before recording: a slot that is already gone — a double-click, a stale menu —
        // must not cost a redo stack and a snapshot of nothing.
        let exists = std::iter::once(&self.project.master)
            .chain(self.project.tracks.iter().map(|track| &track.mixer))
            .any(|strip| strip.effects.iter().any(|s| s.id == slot));
        if !exists {
            return;
        }
        self.record(Edit::RemoveEffect);
        self.project.remove_effect(slot);
        self.invalidate_graph();
    }

    /// Whether an effect slot is enabled, or `None` when the slot does not exist.
    pub fn effect_enabled(&self, track: Option<TrackId>, slot: EffectSlotId) -> Option<bool> {
        self.strip(track)?
            .effects
            .iter()
            .find(|s| s.id == slot)
            .map(|s| s.enabled)
    }

    /// Bypasses or re-enables an effect.
    pub fn set_effect_enabled(
        &mut self,
        track: Option<TrackId>,
        slot: EffectSlotId,
        enabled: bool,
    ) {
        if self.effect_enabled(track, slot).is_none() {
            return;
        }
        self.record(Edit::BypassEffect);
        if let Some(strip) = self.strip_mut(track)
            && let Some(effect) = strip.effects.iter_mut().find(|s| s.id == slot)
        {
            effect.enabled = enabled;
        }
        self.invalidate_graph();
    }

    /// Moves an effect along its chain by `delta` positions.
    pub fn move_effect(&mut self, track: Option<TrackId>, slot: EffectSlotId, delta: isize) {
        let found = self
            .strip(track)
            .and_then(|strip| strip.effects.iter().position(|s| s.id == slot));
        if found.is_none() {
            return;
        }
        self.record(Edit::ReorderEffects);
        if let Some(strip) = self.strip_mut(track)
            && let Some(index) = strip.effects.iter().position(|s| s.id == slot)
        {
            let target = (index as isize + delta).clamp(0, strip.effects.len() as isize - 1);
            let effect = strip.effects.remove(index);
            strip.effects.insert(target as usize, effect);
        }
        self.invalidate_graph();
    }

    fn strip_mut(&mut self, track: Option<TrackId>) -> Option<&mut auris_core::MixerStrip> {
        match track {
            Some(id) => self.project.track_mut(id).map(|t| &mut t.mixer),
            None => Some(&mut self.project.master),
        }
    }

    // ---------------------------------------------------------------- parameters

    /// Parameter descriptors for a plugin, built once by instantiating it.
    pub fn param_descriptors(&mut self, plugin_id: &str) -> Arc<Vec<ParamDescriptor>> {
        if let Some(cached) = self.param_cache.get(plugin_id) {
            return Arc::clone(cached);
        }
        let descriptors = self
            .registry
            .create_instrument(plugin_id)
            .map(|plugin| plugin.parameters().to_vec())
            .or_else(|_| {
                self.registry
                    .create_effect(plugin_id)
                    .map(|plugin| plugin.parameters().to_vec())
            })
            .unwrap_or_default();
        let descriptors = Arc::new(descriptors);
        self.param_cache
            .insert(plugin_id.to_string(), Arc::clone(&descriptors));
        descriptors
    }

    /// The descriptor describing a target, including the mixer's own controls.
    ///
    /// Gain and pan are not plugin parameters, but giving them descriptors lets a frontend
    /// render and edit them with exactly the same code as everything else.
    pub fn descriptor_for(&mut self, target: ParamTarget) -> Option<ParamDescriptor> {
        if let Some(builtin) = Self::mixer_descriptor(target) {
            return Some(builtin);
        }
        let plugin_id = self.plugin_id_for(target)?;
        let index = match target {
            ParamTarget::Instrument { param, .. } | ParamTarget::Effect { param, .. } => {
                param.index()
            }
            _ => return None,
        };
        self.param_descriptors(&plugin_id).get(index).cloned()
    }

    /// The synthesised descriptor for a mixer control, or `None` for a plugin parameter.
    ///
    /// Separate from [`Self::descriptor_for`] because these need no parameter cache, so a
    /// caller holding the session immutably — a render pass building a fader — can still get one.
    pub fn mixer_descriptor(target: ParamTarget) -> Option<ParamDescriptor> {
        match target {
            ParamTarget::TrackPan(_) | ParamTarget::MasterPan => Some(
                ParamDescriptor::new(0u32, "pan", "Pan", -1.0, 1.0, 0.0).with_unit(ParamUnit::Pan),
            ),
            ParamTarget::TrackGain(_) | ParamTarget::MasterGain => Some(ParamDescriptor::decibels(
                0u32, "gain", "Volume", -60.0, 12.0, 0.0,
            )),
            // A send has no headroom above unity: it is how much of a track goes somewhere, and
            // more of it than there is would be a gain stage wearing a send's name.
            ParamTarget::Send { .. } => Some(ParamDescriptor::decibels(
                0u32, "send", "Send", -60.0, 0.0, 0.0,
            )),
            _ => None,
        }
    }

    /// Current value of a parameter, falling back to its default.
    pub fn param_value(&self, target: ParamTarget, descriptor: &ParamDescriptor) -> f32 {
        let from_state = |state: &PluginState| {
            state
                .params
                .get(descriptor.key.as_ref())
                .copied()
                .unwrap_or(descriptor.default)
        };
        match target {
            ParamTarget::TrackGain(id) => self.project.track(id).map_or(0.0, |t| t.mixer.gain_db),
            ParamTarget::TrackPan(id) => self.project.track(id).map_or(0.0, |t| t.mixer.pan),
            ParamTarget::MasterGain => self.project.master.gain_db,
            ParamTarget::MasterPan => self.project.master.pan,
            ParamTarget::Send { track, send } => self
                .project
                .track(track)
                .and_then(|track| track.sends.iter().find(|existing| existing.id == send))
                .map_or(descriptor.default, |send| send.level_db),
            ParamTarget::Instrument { track, .. } => self
                .project
                .track(track)
                .and_then(|t| t.kind.as_instrument())
                .map_or(descriptor.default, |inner| {
                    from_state(&inner.instrument_state)
                }),
            ParamTarget::Effect { track, slot, .. } => self
                .strip(track)
                .and_then(|strip| strip.effects.iter().find(|s| s.id == slot))
                .map_or(descriptor.default, |s| from_state(&s.state)),
        }
    }

    /// Writes a parameter to the document and forwards it to the audio thread.
    ///
    /// Recorded like any other edit. Dragging a knob opens a transaction first, so a sweep is
    /// still one step; every other way of reaching a parameter — a menu choice, a toggle, the
    /// wheel — has no gesture around it and was going unrecorded, which made Undo take back the
    /// edit *before* the parameter change instead.
    pub fn set_param(&mut self, target: ParamTarget, value: f32) {
        // A value that is not a number is not stored: NaN slips every clamp downstream, and
        // `serde_json` writes a non-finite float as `null` — a saved project that can never be
        // opened again. The shipped frontends already sanitise their inputs; this layer is the
        // one that owns the promise, for whichever caller comes next.
        if !value.is_finite() {
            return;
        }
        // Each arm looks its target up before recording, so a stale id costs nothing — and the
        // record carries the target, because coalescing compares edits: two wheel notches on
        // *different* controls within the window must not fold into one step.
        match target {
            ParamTarget::TrackGain(id) => {
                let Ok(index) = self.require_track(id) else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.tracks[index].mixer.gain_db = value;
                self.send(EngineCommand::SetTrackGain {
                    index,
                    gain_db: value,
                });
            }
            ParamTarget::TrackPan(id) => {
                let Ok(index) = self.require_track(id) else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.tracks[index].mixer.pan = value;
                self.send(EngineCommand::SetTrackPan { index, pan: value });
            }
            ParamTarget::MasterGain => {
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.master.gain_db = value;
                self.send(EngineCommand::SetMasterGain(value));
            }
            ParamTarget::MasterPan => {
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.master.pan = value;
                self.send(EngineCommand::SetMasterPan(value));
            }
            // Through the typed command rather than repeating its body: a send that has gone is
            // an error there and a silent no-op here, and only one of the two knows how to write
            // the value.
            ParamTarget::Send { track, send } => {
                let _ = self.set_send_level(track, send, value);
            }
            ParamTarget::Instrument { track, param } => {
                let Ok(index) = self.require_track(track) else {
                    return;
                };
                let Some(key) = self.param_key(target, param) else {
                    return;
                };
                if self.project.tracks[index].kind.as_instrument().is_none() {
                    return;
                }
                self.record_repeating(Edit::AdjustParameter(target));
                if let Some(inner) = self.project.tracks[index].kind.as_instrument_mut() {
                    inner.instrument_state.params.insert(key, value);
                }
                self.send(EngineCommand::SetInstrumentParam {
                    track: index,
                    param,
                    value,
                });
            }
            ParamTarget::Effect { track, slot, param } => {
                let Some(key) = self.param_key(target, param) else {
                    return;
                };
                let track_index = match track {
                    Some(id) => match self.require_track(id) {
                        Ok(index) => Some(index),
                        Err(_) => return,
                    },
                    None => None,
                };
                let Some(slot_index) = self
                    .strip(track)
                    .and_then(|strip| strip.effects.iter().position(|s| s.id == slot))
                else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                if let Some(strip) = self.strip_mut(track) {
                    strip.effects[slot_index].state.params.insert(key, value);
                }
                self.send(EngineCommand::SetEffectParam {
                    track: track_index,
                    slot: slot_index,
                    param,
                    value,
                });
            }
        }
    }

    // ------------------------------------------------------------------ automation
    //
    // A parameter's value over the timeline, beside the value it is set to. The two do not
    // compete: a target with no lane keeps its static value and only an existing lane takes over,
    // which is what lets a mix be automated one control at a time.
    //
    // Unlike the tempo and the meter, none of these snap. A tempo change is aimed at a place in
    // the song; an automation point is aimed at a moment in the sound, and a filter sweep that
    // could only begin on a sixteenth is a filter sweep with a stutter in it. The frontend is
    // where a grid is offered, through the modifier every other drag already answers to.

    /// Every automated parameter in the document.
    pub fn automation(&self) -> &Automation {
        &self.project.automation
    }

    /// Whether `target` is driven by a lane rather than by its stored value.
    ///
    /// A frontend asks before letting a fader be dragged: moving one that automation is about to
    /// overwrite looks like a control that does not work.
    pub fn is_automated(&self, target: ParamTarget) -> bool {
        self.project.automation.lane(target).is_some()
    }

    /// The value driving `target` at `at`, or `None` when it is not automated.
    pub fn automated_value(&self, target: ParamTarget, at: Ticks) -> Option<f32> {
        self.project.automation.value_at(target, at.max_zero())
    }

    /// Writes a point on `target`'s lane, starting the lane if it had none.
    ///
    /// The value is clamped by the parameter's own descriptor, which is also what snaps a
    /// discrete one onto a step: a lane is written in the parameter's units, so a point outside
    /// its range is a point the plugin would refuse anyway.
    ///
    /// Returns whether anything changed, which is `false` for a target this document does not
    /// have and for a point identical to the one already there.
    pub fn set_automation_point(&mut self, target: ParamTarget, at: Ticks, value: f32) -> bool {
        let Some(descriptor) = self.automatable(target) else {
            return false;
        };
        let value = descriptor.clamp(value);
        let curve = curve_for(&descriptor);
        let at = at.max_zero();
        let mut probe = self.project.automation.clone();
        if !probe.set_point(target, curve, at, value) || probe == self.project.automation {
            return false;
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Moves a point along its lane, taking a new value with it.
    ///
    /// Returns where it landed, which is not always where it was asked to go: dropped onto
    /// another point it replaces that one, since one instant cannot hold two values. A drag wants
    /// [`Self::begin_transaction`] around the whole gesture, the way every other drag does.
    pub fn move_automation_point(
        &mut self,
        target: ParamTarget,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Option<Ticks> {
        let descriptor = self.automatable(target)?;
        let value = descriptor.clamp(value);
        let mut probe = self.project.automation.clone();
        let landed = probe.move_point(target, from, to.max_zero(), value)?;
        if probe == self.project.automation {
            return Some(landed);
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        Some(landed)
    }

    /// Removes one point, and the lane with it when that was the last one.
    ///
    /// A lane holding nothing is not an empty lane, it is no lane: the parameter goes back to
    /// the value stored on its strip or in its plugin state.
    pub fn remove_automation_point(&mut self, target: ParamTarget, at: Ticks) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.remove_point(target, at) {
            return false;
        }
        self.record(Edit::EraseAutomation);
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Removes a whole lane, giving the parameter its stored value back.
    pub fn clear_automation(&mut self, target: ParamTarget) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.remove_lane(target) {
            return false;
        }
        self.record(Edit::ClearAutomation);
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Changes how an existing lane gets between its points.
    pub fn set_automation_curve(&mut self, target: ParamTarget, curve: AutomationCurve) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.set_curve(target, curve) || probe == self.project.automation {
            return false;
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// The descriptor for a target this document can actually automate.
    ///
    /// `None` for one it does not have. [`Self::descriptor_for`] answers for a track id nobody
    /// ever created, because a fader's descriptor is synthesised rather than looked up — so the
    /// existence check has to be made here, or a lane could be written into thin air and then
    /// dropped again by the graph builder without anyone being told.
    fn automatable(&mut self, target: ParamTarget) -> Option<ParamDescriptor> {
        let present = match target {
            ParamTarget::MasterGain | ParamTarget::MasterPan => true,
            ParamTarget::TrackGain(id) | ParamTarget::TrackPan(id) => {
                self.project.track(id).is_some()
            }
            ParamTarget::Send { track, send } => self
                .project
                .track(track)
                .is_some_and(|track| track.sends.iter().any(|existing| existing.id == send)),
            ParamTarget::Instrument { track, .. } => self.project.track(track).is_some(),
            ParamTarget::Effect { track, slot, .. } => self
                .strip(track)
                .is_some_and(|strip| strip.effects.iter().any(|effect| effect.id == slot)),
        };
        present.then(|| self.descriptor_for(target)).flatten()
    }

    fn strip(&self, track: Option<TrackId>) -> Option<&auris_core::MixerStrip> {
        match track {
            Some(id) => self.project.track(id).map(|t| &t.mixer),
            None => Some(&self.project.master),
        }
    }

    fn plugin_id_for(&self, target: ParamTarget) -> Option<String> {
        match target {
            ParamTarget::Instrument { track, .. } => Some(
                self.project
                    .track(track)?
                    .kind
                    .as_instrument()?
                    .instrument_id
                    .clone(),
            ),
            ParamTarget::Effect { track, slot, .. } => Some(
                self.strip(track)?
                    .effects
                    .iter()
                    .find(|s| s.id == slot)?
                    .effect_id
                    .clone(),
            ),
            _ => None,
        }
    }

    fn param_key(&mut self, target: ParamTarget, param: ParamId) -> Option<String> {
        let plugin_id = self.plugin_id_for(target)?;
        self.param_descriptors(&plugin_id)
            .get(param.index())
            .map(|descriptor| descriptor.key.to_string())
    }

    // ---------------------------------------------------------------- audition

    /// Sounds a note on a track's instrument, outside the timeline.
    pub fn note_on(&mut self, track: TrackId, pitch: u8, velocity: f32) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::NoteOn {
                track: index,
                pitch: pitch.min(127),
                velocity: velocity.clamp(0.0, 1.0),
            });
        }
    }

    /// Releases an auditioned note.
    pub fn note_off(&mut self, track: TrackId, pitch: u8) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::NoteOff {
                track: index,
                pitch: pitch.min(127),
            });
        }
    }

    /// Sounds several notes at once, which is what it takes to hear a chord.
    pub fn notes_on(&mut self, track: TrackId, pitches: &[u8], velocity: f32) {
        for pitch in pitches {
            self.note_on(track, *pitch, velocity);
        }
    }

    /// Releases them.
    pub fn notes_off(&mut self, track: TrackId, pitches: &[u8]) {
        for pitch in pitches {
            self.note_off(track, *pitch);
        }
    }

    /// The pitches to sound to hear the chord in force at `tick`.
    ///
    /// Empty when nothing is written there, which is the honest answer and not an error: the
    /// stretches between chords are part of a progression too.
    pub fn harmony_voicing(&self, tick: Ticks) -> Vec<u8> {
        self.project
            .harmony
            .chord_at(tick)
            .map(Self::voice_for_audition)
            .unwrap_or_default()
    }

    /// A chord laid out to be listened to rather than played by a part.
    ///
    /// The body sits around middle C, where a chord is easiest to identify, and the bass an
    /// octave and a half below it — far enough down to be heard as a bass rather than as the
    /// chord's own lowest note, which is what makes a slash chord audibly a slash chord.
    ///
    /// This is not what any part would play. A part has a register to keep, neighbours to stay out
    /// of the way of, and a previous chord to lead from; an audition has one job, which is to let
    /// somebody recognise the chord they just wrote down.
    pub fn voice_for_audition(chord: Chord) -> Vec<u8> {
        let mut pitches: Vec<i32> = chord.voiced_near(MIDDLE_C);
        pitches.push(chord.bass_class().midi(2));
        pitches.retain(|pitch| (0..=127).contains(pitch));
        pitches.sort_unstable();
        pitches.dedup();
        pitches.into_iter().map(|pitch| pitch as u8).collect()
    }

    /// A track that can sound an audition: `preferred` when it can, the first that can otherwise.
    ///
    /// Harmony belongs to the timeline rather than to any one track, so hearing it has to borrow
    /// somebody's instrument. Falling back matters more than it looks: writing the chords before
    /// the parts is the whole point of the lane, and at that moment the selected track may well be
    /// the audio track somebody imported a reference mix onto.
    pub fn audition_track(&self, preferred: Option<TrackId>) -> Option<TrackId> {
        let plays_notes = |id: TrackId| {
            self.project
                .track(id)
                .is_some_and(|track| track.kind.as_instrument().is_some())
        };
        preferred.filter(|id| plays_notes(*id)).or_else(|| {
            self.project
                .tracks
                .iter()
                .find(|track| track.kind.as_instrument().is_some())
                .map(|track| track.id)
        })
    }

    // ---------------------------------------------------------------- files

    /// Replaces the document with an empty project holding one instrument track.
    pub fn new_project(&mut self) {
        let mut project = Project::new("Untitled", self.project.sample_rate);
        if let Some(instrument) = self.registry.default_instrument_id() {
            project.add_instrument_track("Track 1", instrument);
        }
        self.history.clear();
        self.path = None;
        self.dirty = false;
        // History is cleared, so nothing can bring the old document's audio back; keeping the
        // decoded buffers would hold them for the rest of the process.
        self.clear_sources();
        self.replace_project(project);
        self.install_shipped_fonts();
    }

    /// Reads a Standard MIDI File as a new document.
    ///
    /// A new document rather than tracks added to this one, because a MIDI file carries its own
    /// tempo and meter: dropping its notes into a piece running at a different speed would give
    /// you the right notes at the wrong lengths, and there would be no way to tell from looking.
    /// The caller deals with unsaved work first, exactly as it does for an opened project — this
    /// clears the history and the path, so the imported piece has to be saved somewhere new
    /// rather than over the `.auris` that happened to be open.
    ///
    /// A track that played on **channel 10** gets the noise-drum instrument where the registry has
    /// one. It is the only thing a bare MIDI file says about what a track is *for*, and a General
    /// MIDI drum part played on a lead synth is not something anyone would keep.
    pub fn import_midi(&mut self, path: &Path) -> Result<MidiReport, SessionError> {
        let imported = auris_io::read_midi_file(path)?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        let fallback = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        let drums = match self.registry.has_instrument(DRUM_INSTRUMENT) {
            true => DRUM_INSTRUMENT.to_string(),
            false => fallback.clone(),
        };

        let mut project = Project::new(name, self.project.sample_rate);
        project.tempo_map = imported.tempo_map.clone();
        project.signatures = imported.signatures.clone();
        let mut report = MidiReport {
            tracks: 0,
            notes: 0,
            length: imported.end(),
        };

        for track in &imported.tracks {
            let instrument = match track.channel {
                DRUM_CHANNEL => drums.clone(),
                _ => fallback.clone(),
            };
            let track_id = project.add_instrument_track(&track.name, instrument);
            // One clip per track, spanning the material rather than the song: a part that does not
            // start until bar forty gets a clip at bar forty, not forty bars of empty clip with
            // its notes at the far end.
            let (Some(first), Some(last)) = (
                track.notes.iter().map(|note| note.start).min(),
                track.notes.iter().map(|note| note.end()).max(),
            ) else {
                continue;
            };
            let Some(clip_id) = project.add_midi_clip(
                track_id,
                &track.name,
                first,
                Ticks((last - first).raw().max(1)),
            ) else {
                continue;
            };
            if let Some(clip) = project.midi_clip_mut(clip_id) {
                clip.notes = track
                    .notes
                    .iter()
                    .map(|note| Note {
                        start: note.start - first,
                        ..*note
                    })
                    .collect();
                // Rebased the same way the notes are, and cut to the clip: a curve written before
                // the first note or after the last has nothing here to shape.
                let rebase = |points: &[CurvePoint]| -> Vec<CurvePoint> {
                    points
                        .iter()
                        .filter(|point| point.at >= first && point.at <= last)
                        .map(|point| CurvePoint {
                            at: point.at - first,
                            ..*point
                        })
                        .collect()
                };
                clip.bend = rebase(&track.bend);
                clip.modulation = rebase(&track.modulation);
                // The file said where the notes are; nothing should grow the clip past them on the
                // next edit and quietly change what it holds.
                clip.length_is_explicit = true;
                report.notes += clip.notes.len();
            }
            report.tracks += 1;
        }

        self.history.clear();
        self.path = None;
        self.clear_sources();
        self.replace_project(project);
        self.install_shipped_fonts();
        // Dirty from the first frame: nothing on disk holds this document, and the `.mid` it came
        // from cannot hold it either.
        self.dirty = true;
        Ok(report)
    }

    /// Writes the open document's instrument tracks as a Standard MIDI File.
    ///
    /// Returns how many notes were written. What a `.mid` has nowhere to put — audio tracks, the
    /// mixer, which instrument each track plays, the automation — is left behind; see
    /// [`auris_io::midi`] for the whole list.
    pub fn export_midi(&self, path: &Path) -> Result<usize, SessionError> {
        Ok(auris_io::write_midi_file(path, &self.project)?)
    }

    /// The folder holding the current document, which relative asset paths resolve against.
    ///
    /// `None` for a project that has never been saved — and one of those has collected nothing,
    /// so every asset it names is still external and resolves without help.
    pub fn project_folder(&self) -> Option<&Path> {
        self.path.as_deref().and_then(auris_io::project_folder)
    }

    /// Opens a project file.
    ///
    /// Returns the references that could not be found — audio files and SoundFonts alike. The
    /// project still opens; whatever named them is silent until the files come back, which is
    /// far friendlier than refusing to open a session because one sample moved.
    ///
    /// A file that has moved but can still be found is written back into the document under its
    /// new reference, which leaves the project dirty. That is the point: the search happens once,
    /// and saving makes the repair permanent.
    pub fn open(&mut self, path: &Path) -> Result<Vec<PathBuf>, SessionError> {
        let project = load_project(path)?;
        self.history.clear();
        self.clear_sources();
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.adopt_project(project);

        let missing = self.reload_assets();
        // After the search, not before it. A project saved on another machine names the shipped
        // font at *that* machine's path; the search finds this machine's copy and writes the new
        // path into the document, and only then does the id it already has match the file about
        // to be installed. The other way round, the same font would arrive twice under two ids —
        // and be held in memory twice, which for this font is four hundred megabytes.
        self.install_shipped_fonts();
        self.rebuild_graph();
        // The document was adopted without telling the engine, so the loop it holds is still the
        // one the *previous* project had.
        self.publish_loop();
        Ok(missing)
    }

    /// Writes the document at exactly `path`, without moving or collecting anything.
    ///
    /// The project folder becomes the directory holding `path`, so a caller choosing a fresh
    /// location wants [`Self::save_as`] instead — this one would leave the audio behind.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        save_project(path, &mut self.project)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    /// Saves to a new location, creating the project folder and collecting the audio into it.
    ///
    /// `chosen` is whatever a save dialog returned; the document lands at the path this returns,
    /// which is `chosen` placed in a folder of its own. The audio the project owns is copied in
    /// alongside it, so the folder can afterwards be moved, renamed, copied to another machine or
    /// zipped up and still open.
    ///
    /// SoundFonts outside the folder are left where they are. A font is a library shared by every
    /// project that uses it, and a copy per project would cost gigabytes to save a path;
    /// [`Self::collect_assets`] is how someone archiving a project asks for those too. A font
    /// already *inside* the folder is a file this project owns like any other, and travels.
    pub fn save_as(&mut self, chosen: &Path) -> Result<SaveReport, SessionError> {
        let document = document_in_folder(chosen);
        // The system save dialog offered to replace whatever is at `chosen`. It is not what gets
        // written: a project goes into a folder named after the file, so choosing `Songs/Ballad`
        // when `Songs/Ballad/Ballad.auris` already exists looked to the dialog like a name
        // nothing was using, and destroyed last week's song without a word. Saving back over
        // *this* project is not a replacement and is allowed to proceed.
        if document.exists() && self.path.as_deref() != Some(document.as_path()) {
            return Err(SessionError::WouldReplace(document));
        }
        self.save_as_replacing(chosen)
    }

    /// [`Self::save_as`] with the replacement already agreed to.
    ///
    /// For a host that has shown the user which project is about to be overwritten and been told
    /// to go ahead. Nothing else differs.
    pub fn save_as_replacing(&mut self, chosen: &Path) -> Result<SaveReport, SessionError> {
        let document = document_in_folder(chosen);
        let folder = auris_io::project_folder(&document)
            .ok_or(SessionError::NoPath)?
            .to_path_buf();
        std::fs::create_dir_all(&folder).map_err(|source| IoError::Filesystem {
            path: folder.clone(),
            source,
        })?;

        // Resolve before the document moves: an `Inside` reference read against the new folder
        // would point at a file that has not been copied there yet.
        let audio: Vec<(SourceId, Option<PathBuf>)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.resolve(self.project_folder())))
            .collect();
        // A font is left where it lies, and an external one stays external — Save As is not the
        // archiving opt-in. But a font that is already `Inside` lives in the *old* folder, and
        // carrying its reference across unchanged would leave the copy naming a file that is not
        // there: the save would report success, playback here would go on sounding from the
        // samples already in memory, and every track on that font would open silent elsewhere.
        let fonts: Vec<(SoundFontId, Option<PathBuf>)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| font.path.is_inside())
            .map(|font| (font.id, font.path.resolve(self.project_folder())))
            .collect();

        // From here the document belongs to the new folder even if the write below fails: the
        // files land there, and their references are read against wherever `self.path` says the
        // document is. Leaving it pointing at the old folder is what would be inconsistent.
        self.path = Some(document.clone());
        let mut uncollected = Vec::new();
        for (id, from) in audio {
            let Some(from) = from else { continue };
            if let Err(error) = self.collect_source(id, &from) {
                log::warn!("could not collect {}: {error}", from.display());
                uncollected.push(from);
            }
        }
        for (id, from) in fonts {
            let Some(from) = from else { continue };
            if let Err(error) = self.collect_font(id, &from) {
                log::warn!("could not collect {}: {error}", from.display());
                uncollected.push(from);
            }
        }

        save_project(&document, &mut self.project)?;
        self.dirty = false;
        Ok(SaveReport {
            document,
            uncollected,
        })
    }

    /// Saves to the path the project was last saved to or opened from.
    pub fn save_in_place(&mut self) -> Result<(), SessionError> {
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        self.save(&path)
    }

    /// Copies every file the project refers to into its folder, however large.
    ///
    /// The command for archiving a project or sending it to someone else: afterwards the folder
    /// holds everything, and nothing outside it is needed to open the project. Explicit rather
    /// than automatic because a SoundFont library runs to hundreds of megabytes per font, and
    /// paying that on every save to shorten a path nobody reads would be a poor trade.
    ///
    /// Returns how many files were copied in. Anything already inside is left alone, so running
    /// this twice costs a directory listing.
    ///
    /// A file that cannot be copied is skipped and the first failure reported *after* every
    /// other file has had its attempt — missing assets are reported, never fatal, and what was
    /// copied stays copied and marked unsaved. A retry adopts what already landed.
    pub fn collect_assets(&mut self) -> Result<usize, SessionError> {
        // Each copy below finds the folder for itself. This is here so that a project which has
        // never been saved is told so, rather than being handed a cheerful `Ok(0)` for having
        // collected nothing into nowhere.
        if self.project_folder().is_none() {
            return Err(SessionError::NoPath);
        }

        let sources: Vec<(SourceId, Option<PathBuf>)> = self
            .project
            .audio_sources
            .values()
            .filter(|source| !source.path.is_inside())
            .map(|source| (source.id, source.path.resolve(None)))
            .collect();
        let fonts: Vec<(SoundFontId, Option<PathBuf>)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| !font.path.is_inside())
            .map(|font| (font.id, font.path.resolve(None)))
            .collect();

        let mut collected = 0;
        let mut failed: Option<SessionError> = None;
        for (id, from) in sources {
            let Some(from) = from else { continue };
            match self.collect_source(id, &from) {
                Ok(()) => collected += 1,
                // Aborting here used to leave the rest uncopied and — worse — the documents
                // already rewritten to `Inside` unmarked, so a clean-looking session disagreed
                // with its own file.
                Err(error) => failed = failed.or(Some(error)),
            }
        }
        for (id, from) in fonts {
            let Some(from) = from else { continue };
            match self.collect_font(id, &from) {
                Ok(()) => collected += 1,
                Err(error) => failed = failed.or(Some(error)),
            }
        }

        if collected > 0 {
            self.dirty = true;
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(collected),
        }
    }

    /// Imports an audio file, adds a track for it and places a clip at `start`.
    ///
    /// The file is copied into the project folder, so the song owns its own audio from the
    /// moment it is imported. A project that has not been saved yet has no folder to copy into
    /// and refers to the file where it lies; saving picks it up.
    pub fn import_audio(&mut self, path: &Path, start: Ticks) -> Result<ClipId, SessionError> {
        let buffer = import_audio_file(path, self.project.sample_rate)?;
        self.record(Edit::ImportAudio);
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio".to_string());
        let source = self.project.add_audio_source(
            name.clone(),
            AssetPath::external(path),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        // Written down now rather than after the copy below, because the copy is the branch that
        // may not happen: a project with no folder yet keeps pointing at the file where it lies,
        // and that reference is the one most likely to need finding again later.
        self.record_source_size(source, path);
        // A failure to copy is not a failure to import: the audio decoded, and referring to it
        // where it lies is exactly what an unsaved project does anyway.
        let has_folder = self.project_folder().is_some();
        if has_folder && let Err(error) = self.collect_source(source, path) {
            log::warn!("could not collect {}: {error}", path.display());
        }
        let track = self.project.add_audio_track(name);
        let clip = self
            .project
            .add_audio_clip(track, source, start.max_zero())
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();
        Ok(clip)
    }

    /// Imports a SoundFont, making its sounds available to every track in the project.
    ///
    /// The file is referred to where it lies rather than copied in — see [`Self::save_as`] for
    /// why — so what the document records is enough to recognise it again: the path, and the
    /// size that tells the font which moved from a different one wearing its name.
    ///
    /// Importing the same file twice returns the id it already has, so a second attempt costs
    /// nothing and, more to the point, does not put a second copy of a very large object in
    /// memory. Nothing is heard until a track is pointed at one of its presets with
    /// [`Self::set_track_preset`].
    pub fn import_soundfont(&mut self, path: &Path) -> Result<SoundFontId, SessionError> {
        let font = load_soundfont(path)?;
        let name = font_name(&font, path);
        let id = match self.project.soundfont_at(self.project_folder(), path) {
            Some(existing) => existing,
            None => {
                // Only a font the document does not know is an edit. Re-importing a known one
                // reloads its samples — which changes what is heard, not what is saved — and a
                // step for it would clear the redo stack over a document that did not move.
                self.record(Edit::ImportSoundFont);
                self.project
                    .add_soundfont(name, AssetPath::external(path), byte_size(path))
            }
        };
        self.fonts.insert(id, font);
        // Fonts the document names but could not find may well be siblings of the one that was
        // just located by hand. Fixing one is then enough to fix the rest.
        if let Some(directory) = path.parent() {
            self.recover_fonts_from(directory);
        }
        // A track already naming this font — one whose file was missing when the project
        // opened, and which the user has just gone and found — starts sounding again.
        self.invalidate_graph();
        Ok(id)
    }

    /// Puts the SoundFonts the application ships with into the document, so their sounds are in
    /// the library from the moment a project opens.
    ///
    /// Called wherever a document is created or opened rather than once at start-up, because a
    /// document is what holds the reference and every new one needs its own.
    ///
    /// Not an edit. The built-in instruments are not in the history either, and for the same
    /// reason: they are what this installation *has*, not something the user did. So no undo step
    /// is recorded, the dirty flag is left exactly as it was, and a new document that has only
    /// ever been looked at still counts as unmodified.
    ///
    /// A font already in the document under the same path keeps its id, which is what makes this
    /// safe on a project that was saved with one.
    ///
    /// Nothing to install is the ordinary answer on a build nobody has run
    /// `tools/fetch-soundfonts.sh` for, and the application runs on its own instruments.
    fn install_shipped_fonts(&mut self) {
        if !self.shipped_library {
            return;
        }
        let dirty = self.dirty;
        let mut installed_any = false;
        for (font, path) in crate::library::installed_fonts() {
            match self.adopt_font(&path) {
                Some(_) => installed_any = true,
                None => log::warn!("could not read the shipped {}", font.name),
            }
        }
        self.dirty = dirty;
        if installed_any {
            self.invalidate_graph();
        }
    }

    /// Reads a font from the shipped library into the document without recording an edit.
    ///
    /// The samples are cached by path in [`Self::shipped`], so the second call — after a
    /// **File → New**, which empties the id-keyed bank — costs a map lookup rather than two
    /// hundred megabytes of file.
    fn adopt_font(&mut self, path: &Path) -> Option<SoundFontId> {
        let font = self.shipped_font_data(path)?;
        let id = match self.project.soundfont_at(self.project_folder(), path) {
            Some(existing) => existing,
            None => {
                let name = font_name(&font, path);
                self.project
                    .add_soundfont(name, AssetPath::external(path), byte_size(path))
            }
        };
        self.fonts.insert(id, font);
        Some(id)
    }

    /// A shipped font's samples, read from the file the first time and cached after that.
    fn shipped_font_data(&mut self, path: &Path) -> Option<Arc<SoundFont>> {
        if let Some(font) = self.shipped.get(path) {
            return Some(Arc::clone(font));
        }
        let font = load_soundfont(path)
            .inspect_err(|error| log::warn!("{}: {error}", path.display()))
            .ok()?;
        self.shipped.insert(path.to_path_buf(), Arc::clone(&font));
        Some(font)
    }

    /// Puts the shipped General MIDI font into a project being built, and returns its new id.
    ///
    /// Takes the project rather than working on [`Self::project`] because the only caller is
    /// [`Self::compose`], which assembles a whole document before it swaps one in — a font added
    /// to the open project would belong to the piece being replaced.
    ///
    /// `None` when nothing is installed, which is what makes a part asking for a violin come out
    /// as the oscillator it also names rather than as silence.
    fn adopt_general_midi(&mut self, project: &mut Project) -> Option<SoundFontId> {
        if !self.shipped_library {
            return None;
        }
        let font = crate::library::shipped(crate::library::GENERAL_MIDI)?;
        let path = crate::library::installed(font)?;
        let data = self.shipped_font_data(&path)?;
        let name = font_name(&data, &path);
        let id = project.add_soundfont(name, AssetPath::external(&path), byte_size(&path));
        // Into the bank now rather than when the document is swapped in. The swap rebuilds the
        // graph, and a graph built while the samples were still missing would log a warning per
        // track about a font that is right here, then be thrown away and built again.
        self.fonts.insert(id, data);
        Some(id)
    }

    /// Every SoundFont the project knows about, whether or not its file is still there.
    pub fn soundfonts(&self) -> impl Iterator<Item = &SoundFontRef> {
        self.project.soundfonts.values()
    }

    /// `true` when a font's samples are actually in memory, so a track naming it will sound.
    pub fn soundfont_is_loaded(&self, id: SoundFontId) -> bool {
        self.fonts.contains(id)
    }

    /// How many sounds an imported font offers, without building the list.
    pub fn soundfont_preset_count(&self, id: SoundFontId) -> usize {
        self.fonts
            .get(id)
            .map(|font| preset_count(&font))
            .unwrap_or(0)
    }

    /// Every sound one imported font offers, in bank and patch order.
    ///
    /// Empty for a font whose file could not be read, which is the same thing a font with no
    /// presets would give — and a library showing nothing under a name is already the message.
    pub fn soundfont_presets(&self, id: SoundFontId) -> Vec<SoundFontPreset> {
        self.fonts
            .get(id)
            .map(|font| presets(&font))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------- asset plumbing

    /// Reads every file the document names, reporting the references nothing could be found for.
    ///
    /// Two passes, because the second needs what the first learned. Anything whose stored
    /// reference is still true is read straight away; only then is there a set of directories
    /// that assets are demonstrably living in, which is where the ones that moved are looked for.
    fn reload_assets(&mut self) -> Vec<PathBuf> {
        let rate = self.project.sample_rate;
        let folder = self.project_folder().map(Path::to_path_buf);

        let sources: Vec<(SourceId, AssetPath, u64)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.clone(), source.byte_size))
            .collect();
        let fonts: Vec<(SoundFontId, AssetPath, u64)> = self
            .project
            .soundfonts
            .values()
            .map(|font| (font.id, font.path.clone(), font.byte_size))
            .collect();

        let mut search = self.search_path();
        let mut missing = Vec::new();

        for (id, stored, size) in sources {
            let Some(found) = locate(&stored, folder.as_deref(), &search, size) else {
                log::warn!("no audio file for {stored}");
                missing.push(stored.as_stored().to_path_buf());
                continue;
            };
            match import_audio_file(&found, rate) {
                Ok(buffer) => {
                    self.relocate_source(id, &stored, &found);
                    remember_directory(&mut search, &found);
                    self.install_source(id, Arc::new(buffer));
                }
                Err(error) => {
                    log::warn!("could not reload {}: {error}", found.display());
                    missing.push(stored.as_stored().to_path_buf());
                }
            }
        }

        for (id, stored, size) in fonts {
            let Some(found) = locate(&stored, folder.as_deref(), &search, size) else {
                log::warn!("no SoundFont file for {stored}");
                missing.push(stored.as_stored().to_path_buf());
                continue;
            };
            match load_soundfont(&found) {
                Ok(font) => {
                    self.relocate_font(id, &stored, &found);
                    remember_directory(&mut search, &found);
                    self.fonts.insert(id, font);
                }
                Err(error) => {
                    log::warn!("could not reload {}: {error}", found.display());
                    missing.push(stored.as_stored().to_path_buf());
                }
            }
        }

        missing
    }

    /// Directories to look in for a file whose stored path has stopped being true.
    ///
    /// The project folder and its audio directory, which is where a file that travelled with the
    /// project will be. Callers add the directories that assets actually turn up in as they go,
    /// so a document naming twenty fonts in one folder finds all twenty once it has found one.
    ///
    /// Then the shipped library, because a project that names the SoundFont this application came
    /// with names it at the path it had on the machine it was saved on. Every installation has
    /// that file, somewhere of its own — so the one reference most likely to be broken by sending
    /// a project to somebody else is also the one that always has an answer.
    fn search_path(&self) -> Vec<PathBuf> {
        let mut roots = match self.project_folder() {
            Some(folder) => vec![folder.join(AUDIO_DIR), folder.to_path_buf()],
            None => Vec::new(),
        };
        roots.extend(crate::library::library_roots());
        roots
    }

    /// Looks again for the fonts the document could not find, now that `directory` is known to
    /// hold at least one of them.
    fn recover_fonts_from(&mut self, directory: &Path) {
        let lost: Vec<(SoundFontId, AssetPath, u64)> = self
            .project
            .soundfonts
            .values()
            .filter(|font| !self.fonts.contains(font.id))
            .map(|font| (font.id, font.path.clone(), font.byte_size))
            .collect();

        let search = [directory.to_path_buf()];
        for (id, stored, size) in lost {
            let Some(name) = stored.file_name() else {
                continue;
            };
            let Some(found) = find_named(name, &search, size) else {
                continue;
            };
            match load_soundfont(&found) {
                Ok(font) => {
                    log::info!("found {} again at {}", stored, found.display());
                    self.relocate_font(id, &stored, &found);
                    self.fonts.insert(id, font);
                }
                Err(error) => log::warn!("could not read {}: {error}", found.display()),
            }
        }
    }

    /// Copies one audio file into the project folder and points the document at the copy.
    fn collect_source(&mut self, id: SourceId, from: &Path) -> Result<(), SessionError> {
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;
        let name = copy_into(from, &folder.join(AUDIO_DIR))?;
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.path = AssetPath::inside(Path::new(AUDIO_DIR).join(name));
        }
        // `copy_into` either copied these bytes or found a file already holding them, so the size
        // of the source is the size of the copy the document now names.
        self.record_source_size(id, from);
        Ok(())
    }

    /// Copies one SoundFont into the project folder and points the document at the copy.
    ///
    /// Whether a font *should* be copied is policy and belongs to the callers — a font is a
    /// library shared by every project, so only [`Self::collect_assets`] brings an external one
    /// in, while [`Self::save_as`] carries across the ones this project already owns. This is the
    /// mechanism both of them use, so there is one account of what "the project owns it" means on
    /// disk.
    fn collect_font(&mut self, id: SoundFontId, from: &Path) -> Result<(), SessionError> {
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;
        let name = copy_into(from, &folder.join(AUDIO_DIR))?;
        if let Some(font) = self.project.soundfonts.get_mut(&id) {
            font.path = AssetPath::inside(Path::new(AUDIO_DIR).join(name));
        }
        Ok(())
    }

    /// Records that an audio file turned out to be somewhere other than where it was stored.
    fn relocate_source(&mut self, id: SourceId, stored: &AssetPath, found: &Path) {
        let Some(reference) = self.moved_reference(stored, found) else {
            return;
        };
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.path = reference;
        }
        self.record_source_size(id, found);
        self.dirty = true;
    }

    /// Records that a SoundFont turned out to be somewhere other than where it was stored.
    fn relocate_font(&mut self, id: SoundFontId, stored: &AssetPath, found: &Path) {
        let Some(reference) = self.moved_reference(stored, found) else {
            return;
        };
        if let Some(font) = self.project.soundfonts.get_mut(&id) {
            font.path = reference;
            font.byte_size = byte_size(found);
        }
        self.dirty = true;
    }

    /// Writes down how large the file an audio source names is.
    ///
    /// The fingerprint `Session::reload_assets` confirms a candidate with, so it is refreshed
    /// everywhere the reference is rewritten — a font does the same thing inline in
    /// `Session::relocate_font`, and a source needs it in three places rather than one. A file
    /// that cannot be measured records 0, which means "no fingerprint" and leaves the name to be
    /// taken on trust: the same answer a document written before the field existed gives.
    fn record_source_size(&mut self, id: SourceId, file: &Path) {
        if let Some(source) = self.project.audio_sources.get_mut(&id) {
            source.byte_size = byte_size(file);
        }
    }

    /// How the document should refer to a file now found at `found`, or `None` when that is
    /// already what it says and nothing needs writing back.
    fn moved_reference(&self, stored: &AssetPath, found: &Path) -> Option<AssetPath> {
        let reference = match self
            .project_folder()
            .and_then(|folder| found.strip_prefix(folder).ok())
        {
            Some(relative) => AssetPath::inside(relative),
            None => AssetPath::external(found),
        };
        (&reference != stored).then_some(reference)
    }

    /// Stores decoded audio, the peaks used to draw it, and the copy the graph will render.
    fn install_source(&mut self, id: SourceId, buffer: Arc<AudioBuffer>) {
        let peaks = compute_peaks(self.gpu.as_deref(), &buffer, WAVEFORM_BUCKET);
        self.waveforms.insert(id, Arc::new(peaks));
        if let Some(at_rate) = source_at_rate(id, &buffer, self.render_bank_rate) {
            self.render_bank.insert(id, at_rate);
        }
        self.bank.insert(id, buffer);
    }

    /// Drops everything decoded: both audio banks, the waveform cache and the fonts.
    fn clear_sources(&mut self) {
        self.bank = AudioSourceBank::new();
        self.render_bank = AudioSourceBank::new();
        self.waveforms.clear();
        self.fonts.clear();
    }
}

/// Where an asset's file actually is.
///
/// The stored reference when it is still true, and otherwise the first place a search turns up a
/// file of the right name — confirmed by `expected_size` where the document recorded one, so a
/// different file wearing the same name is not quietly adopted.
fn locate(
    stored: &AssetPath,
    folder: Option<&Path>,
    search: &[PathBuf],
    expected_size: u64,
) -> Option<PathBuf> {
    if let Some(direct) = stored.resolve(folder)
        && direct.is_file()
    {
        return Some(direct);
    }
    find_named(stored.file_name()?, search, expected_size)
}

/// How a new lane over a parameter should get between its points.
///
/// A parameter with discrete positions holds; everything else runs in a straight line.
/// Interpolating a chooser would sweep through every option between two settings and sound all of
/// them on the way — a filter opening is a gesture, a waveform changing is not.
///
/// Only consulted when a lane is created. Changing an existing one is
/// [`Session::set_automation_curve`], so writing a point cannot restyle a curve somebody shaped.
fn curve_for(descriptor: &ParamDescriptor) -> AutomationCurve {
    match descriptor.steps {
        Some(_) => AutomationCurve::Hold,
        None => AutomationCurve::Linear,
    }
}

/// Adds the directory holding `found` to the places later searches will look.
fn remember_directory(search: &mut Vec<PathBuf>, found: &Path) {
    let Some(directory) = found.parent() else {
        return;
    };
    if !search.iter().any(|known| known == directory) {
        search.push(directory.to_path_buf());
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

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session always opens")
    }

    /// A moment `ms` after whichever `Instant` it is added to.
    fn tick(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// How many steps deep the undo stack is, counted by walking it.
    fn undo_depth(session: &mut Session) -> usize {
        let mut depth = 0;
        while session.undo().is_some() {
            depth += 1;
        }
        while session.redo().is_some() {}
        depth
    }

    /// Registers a font in the document without a file behind it.
    ///
    /// Enough to exercise every command that decides what a track *plays*; what it *sounds* like
    /// needs a real SoundFont, which is somebody's 200 MB file rather than a test fixture.
    fn named_font(session: &mut Session, name: &str) -> SoundFontId {
        session.project.add_soundfont(
            name,
            AssetPath::external(format!("/fonts/{name}.sf2")),
            1024,
        )
    }

    /// A session holding one instrument track and one bus, with nothing routed yet.
    fn routed_session() -> (Session, TrackId, TrackId) {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let bus = session.add_bus_track("Reverb");
        (session, track, bus)
    }

    #[test]
    fn a_track_moves_up_and_down_the_list_in_one_step() {
        let mut session = session();
        let first = session.add_default_instrument_track("A").expect("track");
        let second = session.add_default_instrument_track("B").expect("track");
        let third = session.add_default_instrument_track("C").expect("track");
        let order = |session: &Session| -> Vec<TrackId> {
            session
                .project
                .tracks
                .iter()
                .map(|track| track.id)
                .collect()
        };

        session.move_track(first, 2).expect("to the end");
        assert_eq!(order(&session), vec![second, third, first]);
        session.undo().expect("a step");
        assert_eq!(order(&session), vec![first, second, third]);

        // Past the end is the end rather than an error: a hand that overshoots means the bottom.
        session.move_track(first, 99).expect("clamped");
        assert_eq!(order(&session), vec![second, third, first]);
        // And a move to where it already is changes nothing and records nothing.
        let before = undo_depth(&mut session);
        session.move_track(first, 2).expect("already there");
        assert_eq!(undo_depth(&mut session), before);
    }

    #[test]
    fn reordering_tracks_leaves_the_routing_alone() {
        // Everything in the document names a track by id, so a bus may end up *above* the tracks
        // feeding it — which is only a fact about the list, not about the mix. The renderer walks
        // the routing order rather than the list, so what is heard does not change.
        let (mut session, kick, bus) = routed_session();
        session.set_track_output(kick, Output::Bus(bus)).unwrap();
        session.set_param(ParamTarget::TrackGain(bus), -6.0);

        session.move_track(bus, 0).expect("the bus goes first");
        assert_eq!(session.project.tracks[0].id, bus);
        assert_eq!(
            session.project.track(kick).unwrap().output,
            Output::Bus(bus)
        );
        assert_eq!(session.project.track(bus).unwrap().mixer.gain_db, -6.0);
        // A bus above its feeders is still rendered after them.
        let order = session.project.routing_order();
        let at = |id: TrackId| {
            let index = session.project.track_index(id).unwrap();
            order.iter().position(|slot| *slot == index).unwrap()
        };
        assert!(at(kick) < at(bus));
    }

    #[test]
    fn a_track_routes_into_a_bus_and_back_out_to_the_master() {
        let (mut session, track, bus) = routed_session();
        session
            .set_track_output(track, Output::Bus(bus))
            .expect("a bus is a legal destination");
        assert_eq!(
            session.project.track(track).unwrap().output,
            Output::Bus(bus)
        );

        // And it is one undo step, which puts the track back on the master.
        session.undo().expect("a step");
        assert_eq!(session.project.track(track).unwrap().output, Output::Master);
    }

    #[test]
    fn only_a_bus_can_be_routed_into() {
        let (mut session, track, _) = routed_session();
        let other = session.add_audio_track("Sample");
        let error = session
            .set_track_output(track, Output::Bus(other))
            .expect_err("an audio track is not a mixing point");
        assert!(matches!(error, SessionError::NotABus(id) if id == other.0));
        assert!(matches!(
            session.add_send(track, other),
            Err(SessionError::NotABus(_))
        ));
    }

    #[test]
    fn a_refused_route_costs_neither_a_step_nor_the_redo_branch() {
        // Validation before `record`, stated as a test: a command that pushes a step and then
        // fails leaves a rung that reverses nothing and throws away whatever could be redone.
        let (mut session, track, bus) = routed_session();
        session.set_track_output(track, Output::Bus(bus)).unwrap();
        session.undo().expect("back to the master");
        // Two steps behind and one ahead: exactly the state a refused command must not disturb.

        let _ = session.set_track_output(track, Output::Bus(TrackId(9_999)));
        let _ = session.add_send(track, TrackId(9_999));

        assert!(session.redo().is_some(), "the redo branch was thrown away");
        assert_eq!(
            session.project.track(track).unwrap().output,
            Output::Bus(bus)
        );
        // And nothing was pushed: one undo is back at the master, with no phantom rung between.
        session.undo().expect("a step");
        assert_eq!(session.project.track(track).unwrap().output, Output::Master);
    }

    #[test]
    fn routing_that_would_loop_is_refused() {
        let (mut session, _, first) = routed_session();
        let second = session.add_bus_track("Delay");
        session
            .set_track_output(first, Output::Bus(second))
            .expect("one bus into another is fine");

        let error = session
            .set_track_output(second, Output::Bus(first))
            .expect_err("that closes the circle");
        assert!(matches!(
            error,
            SessionError::RoutingLoop { from, to } if from == second.0 && to == first.0
        ));
        // A send round the same circle is refused for the same reason, and so is a bus into
        // itself — a loop has no order it can be rendered in either way.
        assert!(matches!(
            session.add_send(second, first),
            Err(SessionError::RoutingLoop { .. })
        ));
        assert!(matches!(
            session.set_track_output(first, Output::Bus(first)),
            Err(SessionError::RoutingLoop { .. })
        ));
    }

    #[test]
    fn what_may_be_routed_where_is_one_rule_the_picker_and_the_command_share() {
        // A frontend greys a row out by this and the command refuses by the same facts, so the
        // list can never offer something the session would then turn down.
        let (mut session, track, first) = routed_session();
        let second = session.add_bus_track("Delay");
        let audio = session.add_audio_track("Sample");
        session
            .set_track_output(first, Output::Bus(second))
            .unwrap();

        assert!(session.can_route(track, first));
        assert!(session.can_route(first, second));
        // Round the circle, into itself, into something that is not a bus, and into a track that
        // was never made.
        assert!(!session.can_route(second, first));
        assert!(!session.can_route(first, first));
        assert!(!session.can_route(track, audio));
        assert!(!session.can_route(track, TrackId(9_999)));

        // And every one of those refusals is the error the command gives back.
        for (from, to) in [(second, first), (first, first)] {
            assert!(matches!(
                session.set_track_output(from, Output::Bus(to)),
                Err(SessionError::RoutingLoop { .. })
            ));
        }
        assert!(matches!(
            session.set_track_output(track, Output::Bus(audio)),
            Err(SessionError::NotABus(_))
        ));
    }

    #[test]
    fn the_buses_offered_are_the_ones_that_would_not_loop() {
        // The list a picker shows is a fact about the document, so it is worked out here rather
        // than in each frontend — one that offered an illegal destination would be offering an
        // error message.
        let (mut session, track, first) = routed_session();
        let second = session.add_bus_track("Delay");
        session
            .set_track_output(first, Output::Bus(second))
            .unwrap();

        assert_eq!(session.available_buses(track), vec![first, second]);
        // `first` already feeds `second`, so `second` cannot feed it back — and no bus can feed
        // itself.
        assert_eq!(session.available_buses(second), Vec::new());
        assert_eq!(session.available_buses(first), vec![second]);
    }

    #[test]
    fn a_send_starts_at_unity_after_the_fader() {
        // A send is added in order to be heard. Starting it at silence would make the first thing
        // every user does be to undo the default.
        let (mut session, track, bus) = routed_session();
        let send = session.add_send(track, bus).expect("a send");
        let added = &session.project.track(track).unwrap().sends[0];
        assert_eq!(added.id, send);
        assert_eq!(added.target, bus);
        assert_eq!(added.level_db, 0.0);
        assert!(!added.pre_fader);
    }

    #[test]
    fn turning_one_send_repeatedly_is_one_step_and_turning_another_is_a_new_one() {
        let (mut session, track, bus) = routed_session();
        let first = session.add_send(track, bus).unwrap();
        let second = session.add_send(track, bus).unwrap();
        let before = undo_depth(&mut session);
        while session.redo().is_some() {}

        for level in [-1.0, -2.0, -3.0] {
            session.set_send_level(track, first, level).unwrap();
        }
        session.set_send_level(track, second, -9.0).unwrap();
        assert_eq!(
            undo_depth(&mut session),
            before + 2,
            "a drag on one send folds; moving to another must not"
        );
    }

    #[test]
    fn a_send_to_a_deleted_bus_leaves_with_it() {
        let (mut session, track, bus) = routed_session();
        session.set_track_output(track, Output::Bus(bus)).unwrap();
        session.add_send(track, bus).unwrap();

        session.remove_track(bus).expect("the bus goes");
        let track = session.project.track(track).unwrap();
        assert_eq!(track.output, Output::Master);
        assert!(track.sends.is_empty());
    }

    #[test]
    fn a_send_that_is_not_there_is_named_rather_than_ignored() {
        let (mut session, track, _) = routed_session();
        let error = session
            .set_send_level(track, SendId(1_234), -6.0)
            .expect_err("no such send");
        assert!(matches!(
            error,
            SessionError::UnknownSend { track: t, send } if t == track.0 && send == 1_234
        ));
    }

    #[test]
    fn a_send_level_that_is_not_a_number_is_refused() {
        // The same promise every stored float carries: `serde_json` writes a non-finite as
        // `null`, and no `f32` field will ever read that back.
        let (mut session, track, bus) = routed_session();
        let send = session.add_send(track, bus).unwrap();
        assert!(matches!(
            session.set_send_level(track, send, f32::NAN),
            Err(SessionError::NotFinite(_))
        ));
        assert_eq!(
            session.project.track(track).unwrap().sends[0].level_db,
            0.0,
            "the refused value must not have landed"
        );
    }

    #[test]
    fn a_new_track_never_starts_on_the_sampler() {
        // The sampler sorts first by plugin id, so before there was a nominated default it won
        // the "first registered instrument" race — and a new track came up silent.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let instrument = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .map(|inner| inner.instrument_id.clone())
            .expect("an instrument track");
        assert_ne!(instrument, SAMPLER_ID);
        assert_eq!(instrument, crate::registry::DEFAULT_INSTRUMENT);
    }

    #[test]
    fn choosing_a_sound_also_chooses_the_instrument_that_makes_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        let preset = PresetRef {
            font,
            bank: 0,
            patch: 40,
        };

        session.set_track_preset(track, preset).expect("chosen");

        let inner = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .expect("an instrument track");
        assert_eq!(inner.instrument_id, SAMPLER_ID);
        assert_eq!(session.track_preset(track), Some(preset));
    }

    #[test]
    fn auditioning_a_second_preset_keeps_how_the_player_is_set_up() {
        // Level, reverb and chorus describe the player, not the sound it is playing. Clearing
        // them every time somebody tried the next preset along would be its own small tragedy.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 40,
                },
            )
            .expect("chosen");
        if let Some(inner) = session
            .project
            .track_mut(track)
            .and_then(|t| t.kind.as_instrument_mut())
        {
            inner.instrument_state.params.insert("level".into(), -6.0);
        }
        let knob = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        assert!(session.set_automation_point(knob, Ticks::ZERO, 0.0));

        let second = PresetRef {
            font,
            bank: 0,
            patch: 41,
        };
        session.set_track_preset(track, second).expect("chosen");

        let inner = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .expect("an instrument track");
        assert_eq!(session.track_preset(track), Some(second));
        assert_eq!(inner.instrument_state.params.get("level"), Some(&-6.0));
        // The plugin did not change, so its curves still address exactly what they were drawn
        // for. Dropping them here would lose a sweep to the act of trying the next patch along.
        assert!(
            session.automation().lane(knob).is_some(),
            "an audition is not a change of plugin"
        );
    }

    #[test]
    fn a_preset_from_a_font_the_project_does_not_have_is_refused() {
        // The id would end up in the document and resolve to nothing for the rest of its life.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        session.forget_history();

        let refused = session.set_track_preset(
            track,
            PresetRef {
                font: SoundFontId(999),
                bank: 0,
                patch: 0,
            },
        );
        assert!(matches!(refused, Err(SessionError::UnknownSoundFont(999))));
        assert!(!session.can_undo(), "a refused edit left a step behind");
        assert_eq!(session.track_preset(track), None);
    }

    #[test]
    fn an_audio_track_has_no_sound_to_choose() {
        let mut session = session();
        let audio = session.add_audio_track("Sample");
        let font = named_font(&mut session, "Orchestra");
        session.forget_history();

        let refused = session.set_track_preset(
            audio,
            PresetRef {
                font,
                bank: 0,
                patch: 0,
            },
        );
        assert!(matches!(refused, Err(SessionError::WrongTrackKind { .. })));
        assert!(!session.can_undo(), "a refused edit left a step behind");
    }

    #[test]
    fn a_track_on_another_instrument_reports_no_preset() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        assert_eq!(session.track_preset(track), None);
    }

    #[test]
    fn importing_a_soundfont_that_is_not_there_changes_nothing() {
        // The picker hands over whatever the user chose, and a file can be gone by the time it
        // is opened. Failing has to leave the document exactly as it was.
        let mut session = session();
        session.forget_history();

        let refused = session.import_soundfont(Path::new("no-such-soundfont.sf2"));
        assert!(refused.is_err());
        assert_eq!(session.soundfonts().count(), 0);
        assert!(!session.can_undo(), "a failed import left a step behind");
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_font_the_project_names_but_has_not_loaded_is_reported_as_such() {
        // What a library panel needs in order to say "this file has moved" rather than showing
        // an empty list of sounds and leaving the user to guess.
        let mut session = session();
        let font = named_font(&mut session, "Orchestra");
        assert_eq!(session.soundfonts().count(), 1);
        assert!(!session.soundfont_is_loaded(font));
        assert!(session.soundfont_presets(font).is_empty());
    }

    #[test]
    fn an_audio_track_has_no_instrument_to_change() {
        // This used to return `Ok` having changed nothing and recorded an undo step anyway, so
        // the history grew a rung that reversed nothing at all.
        let mut session = session();
        let audio = session.add_audio_track("Sample");
        let instrument = session
            .registry()
            .default_instrument_id()
            .expect("the default registry has instruments")
            .to_string();
        session.forget_history();

        let refused = session.set_track_instrument(audio, &instrument);
        assert!(matches!(refused, Err(SessionError::WrongTrackKind { .. })));
        assert!(!session.can_undo(), "a refused edit left a step behind");
    }

    #[test]
    fn a_section_is_written_on_a_bar_line_and_found_from_anywhere_inside_it() {
        let bar = |n: i64| Ticks(3_840 * n);
        let mut session = session();
        session.set_section(Ticks(5), Some("イントロ".into()));
        session.set_section(bar(4) + Ticks(999), Some("サビ".into()));

        let points = session.project().sections.points();
        assert_eq!(points[0].tick, Ticks::ZERO, "snapped to its bar");
        assert_eq!(points[1].tick, bar(4));
        assert_eq!(
            session.project().sections.section_at(bar(6)),
            Some(("サビ", 1))
        );

        // Renaming is writing at the same bar; removing acts through the whole stretch.
        session.set_section(bar(4), Some("落ちサビ".into()));
        assert_eq!(
            session.project().sections.label_at(bar(5)),
            Some("落ちサビ")
        );
        session.remove_section(bar(7));
        assert_eq!(
            session.project().sections.label_at(bar(5)),
            Some("イントロ"),
            "the section before it runs through"
        );

        assert!(session.move_section(bar(2), bar(8) + Ticks(1)));
        assert_eq!(session.project().sections.points()[0].tick, bar(8));

        assert!(session.is_dirty());
        while session.undo().is_some() {}
        assert!(session.project().sections.is_empty());
    }

    #[test]
    fn a_generated_clip_reads_the_section_it_sits_in() {
        // The hint the structure exists for: labelling a stretch changes what the composer
        // writes there next. Same clip, same recipe, same harmony — a label appears under it,
        // and regenerating draws different material keyed by that label.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .expect("the catalogue knows axis");
        let recipe = ClipRecipe::new(ClipPreset::Lead, 7);
        let clip = session
            .generate_clip(track, Ticks::ZERO, Ticks::from_beats(16.0), recipe)
            .expect("generated");
        let unlabelled = session.midi_clip(clip).expect("clip").notes.clone();

        session.set_section(Ticks::ZERO, Some("サビ".into()));
        session.regenerate_clip(clip).expect("regenerated");
        let labelled = session.midi_clip(clip).expect("clip").notes.clone();
        assert_ne!(
            unlabelled, labelled,
            "the label should key the clip's material"
        );

        // And the same label writes the same take again: the hint is deterministic.
        session.regenerate_clip(clip).expect("regenerated again");
        assert_eq!(session.midi_clip(clip).expect("clip").notes, labelled);
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
    fn a_selection_dragged_across_lanes_moves_together_or_not_at_all() {
        // Dropping half a selection on a new track and leaving the rest behind is not what the
        // gesture meant, so one clip that cannot land refuses the whole move.
        let mut session = session();
        let source = session.add_default_instrument_track("Lead").expect("track");
        let destination = session
            .add_default_instrument_track("Second")
            .expect("track");
        let audio = session.add_audio_track("Sample");
        let first = session
            .add_midi_clip(source, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("clip");
        let second = session
            .add_midi_clip(source, "B", Ticks::from_beats(4.0), Ticks::from_beats(4.0))
            .expect("clip");

        // One of the two is sent somewhere it cannot go, so neither moves.
        let refused = session.move_clips_to_track(&[(first, destination), (second, audio)]);
        assert!(refused.is_err());
        assert_eq!(session.track_of_clip(first), Some(source));
        assert_eq!(session.track_of_clip(second), Some(source));

        // Both to a track that accepts them, and both arrive.
        session
            .move_clips_to_track(&[(first, destination), (second, destination)])
            .expect("both clips belong on an instrument track");
        assert_eq!(session.track_of_clip(first), Some(destination));
        assert_eq!(session.track_of_clip(second), Some(destination));
        assert!(session.can_undo(), "the move left no undo step");
    }

    #[test]
    fn moving_clips_nowhere_records_no_undo_step() {
        // A pointer drag calls this on every move, and most of them are within one track.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let clip = session
            .add_midi_clip(track, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("clip");
        session.forget_history();

        session
            .move_clips_to_track(&[(clip, track)])
            .expect("a clip always fits the track it is on");
        assert!(
            !session.can_undo(),
            "a move that moved nothing pushed a step onto the history"
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
    fn a_tempo_that_has_not_moved_is_not_an_edit() {
        let mut session = session();
        session.forget_history();
        session.set_bpm(session.project().bpm());
        assert!(!session.can_undo());
        // And neither is one the clamp refuses, however long the wheel is held past the end.
        session.set_bpm(10_000.0);
        let steps = undo_depth(&mut session);
        assert_eq!(steps, 1, "the first push through the ceiling did move it");
        session.set_bpm(20_000.0);
        assert_eq!(undo_depth(&mut session), steps, "and the next one did not");
        // A written change that changes nothing is not an edit either.
        session.set_tempo_point(Ticks::ZERO, session.project().bpm());
        assert_eq!(undo_depth(&mut session), steps);
    }

    #[test]
    fn a_tempo_change_lands_on_the_beat_and_stays_undoable() {
        let mut session = session();
        session.forget_history();

        // A pointer aims a little past the second beat; the change lands on the beat itself.
        session.set_tempo_point(Ticks(970), 90.0);
        let points = session.project().tempo_map.points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].tick, Ticks(960));
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(959)), 120.0);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(960)), 90.0);
        // The initial tempo is untouched: the change is a change, not the project knob.
        assert_eq!(session.project().bpm(), 120.0);

        assert_eq!(session.undo(), Some(Edit::SetTempoPoint));
        assert_eq!(session.project().tempo_map.points().len(), 1);
    }

    #[test]
    fn editing_the_tempo_edits_the_stretch_it_is_aimed_at() {
        let mut session = session();
        session.set_tempo_point(Ticks(3_840), 90.0);
        session.forget_history();

        // Aimed mid-stretch, the edit turns the change governing that stretch rather than
        // writing a new one.
        session.set_tempo_at(Ticks(5_000), 96.0);
        assert_eq!(session.project().tempo_map.points().len(), 2);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 96.0);
        assert_eq!(
            session.project().bpm(),
            120.0,
            "the opening stretch kept its own"
        );

        // Turning the opening stretch straight afterwards is its own undo step: the recorded
        // edits carry different positions, so they can never coalesce however fast they come.
        session.set_tempo_at(Ticks::ZERO, 110.0);
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks(3_840))));
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 90.0);
    }

    #[test]
    fn removing_a_tempo_change_is_aimed_from_anywhere_inside_it() {
        let mut session = session();
        session.set_tempo_point(Ticks(3_840), 90.0);
        session.forget_history();

        // The anchor is not a change: pointing inside the opening stretch removes nothing.
        session.remove_tempo_point(Ticks(100));
        assert_eq!(session.project().tempo_map.points().len(), 2);
        assert!(
            !session.can_undo(),
            "refusing to remove the anchor is not an edit"
        );

        // Pointing far past the change still removes the change in force there.
        session.remove_tempo_point(Ticks(50_000));
        assert_eq!(session.project().tempo_map.points().len(), 1);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 120.0);
        assert_eq!(session.undo(), Some(Edit::RemoveTempoPoint));
        assert_eq!(session.project().tempo_map.points().len(), 2);
    }

    #[test]
    fn a_signature_change_lands_on_a_bar_and_comes_back_off_it() {
        let mut session = session();
        session.forget_history();
        let three_four = TimeSignature::new(3, 4);

        // A pointer lands mid-bar; the change lands on the bar line it was aimed at.
        session.set_signature_point(Ticks(BAR.raw() * 2 + 400), three_four);
        let points = session.project().signatures.points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].tick, BAR * 2);
        assert_eq!(session.signature_at(BAR * 2), three_four);
        assert_eq!(
            session.signature_at(BAR * 2 - Ticks(1)),
            TimeSignature::default(),
            "the bars before it are what they were"
        );
        // And the bar numbering follows: bar 3 is where the 3/4 starts, bar 4 three beats later.
        assert_eq!(session.project().signatures.bar_of(BAR * 2), 3);
        assert_eq!(
            session.project().signatures.bar_start(4),
            BAR * 2 + three_four.ticks_per_bar()
        );

        assert_eq!(session.undo(), Some(Edit::SetSignaturePoint));
        assert!(session.project().signatures.is_constant());
    }

    #[test]
    fn editing_the_signature_edits_the_stretch_it_is_aimed_at() {
        let mut session = session();
        session.set_signature_point(BAR * 4, TimeSignature::new(3, 4));
        session.forget_history();

        // Aimed mid-stretch, the edit turns the change governing that stretch rather than
        // writing a new one.
        session.set_signature_at(BAR * 6, TimeSignature::new(7, 8));
        assert_eq!(session.project().signatures.points().len(), 2);
        assert_eq!(session.signature_at(BAR * 4), TimeSignature::new(7, 8));
        assert_eq!(
            session.project().signatures.initial(),
            TimeSignature::default(),
            "the opening stretch kept its own"
        );

        // Turning the opening stretch straight afterwards is its own undo step: the recorded
        // edits carry different positions, so they can never coalesce however fast they come.
        session.set_signature_at(Ticks::ZERO, TimeSignature::new(5, 4));
        assert_eq!(session.undo(), Some(Edit::ChangeSignature(Ticks::ZERO)));
        assert_eq!(session.undo(), Some(Edit::ChangeSignature(BAR * 4)));
        assert_eq!(session.signature_at(BAR * 4), TimeSignature::new(3, 4));
    }

    #[test]
    fn removing_a_signature_change_is_aimed_from_anywhere_inside_it() {
        let mut session = session();
        session.set_signature_point(BAR * 4, TimeSignature::new(3, 4));
        session.forget_history();

        // The anchor is not a change: pointing inside the opening stretch removes nothing.
        session.remove_signature_point(Ticks(100));
        assert_eq!(session.project().signatures.points().len(), 2);
        assert!(
            !session.can_undo(),
            "refusing to remove the anchor is not an edit"
        );

        // Pointing far past the change still removes the change in force there.
        session.remove_signature_point(Ticks(500_000));
        assert!(session.project().signatures.is_constant());
        assert_eq!(session.undo(), Some(Edit::RemoveSignaturePoint));
        assert_eq!(session.project().signatures.points().len(), 2);
    }

    #[test]
    fn a_meter_change_moves_the_bar_lines_and_not_one_note() {
        // The whole reason this is not on the audio path. A note is a tick position; the tempo
        // map turns ticks into samples; neither asks how many beats are in a bar.
        let mut session = session();
        let track = session
            .add_default_instrument_track("Lead")
            .expect("the registry has an instrument");
        let clip = session
            .add_midi_clip(track, "Part", Ticks::ZERO, BAR * 4)
            .expect("an instrument track takes a midi clip");
        session
            .add_note(clip, Note::new(60, BAR * 2, BAR))
            .expect("the note fits the clip");
        let before = session.project().midi_clip(clip).unwrap().1.notes.clone();
        let seconds = session.project().duration_seconds();

        session.set_signature_point(BAR, TimeSignature::new(7, 8));

        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes,
            before,
            "a note moved when the meter changed"
        );
        assert_eq!(
            session.project().duration_seconds(),
            seconds,
            "the song got longer or shorter when the meter changed"
        );
    }

    #[test]
    fn harmony_snaps_to_the_beat_of_the_meter_it_is_written_in() {
        let mut session = session();
        // Seven eight: a bar is 3360 ticks, and the beat is an eighth rather than a quarter.
        session.set_signature_point(BAR, TimeSignature::new(7, 8));
        let seven_eight_bar = TimeSignature::new(7, 8).ticks_per_bar();

        // Counted from the change, not from tick zero. The second bar of 7/8 starts 3360 ticks
        // past a 3840-tick bar, which is not a multiple of anything the grid would offer — a
        // snap measured from the origin would sit a fraction off it.
        let second = BAR + seven_eight_bar;
        assert_eq!(session.snap_harmony(second + Ticks(20)), second);
        assert_eq!(
            session.harmony_grid_at(second),
            Ticks(auris_core::TICKS_PER_QUARTER / 2),
            "an eighth is the beat in seven eight"
        );
    }

    /// An audio clip of `frames` frames on its own track, with no samples behind it.
    ///
    /// Enough to exercise every command that shapes the clip; what it *sounds* like needs
    /// decoded audio, which is an importer's business rather than a fixture's.
    fn audio_clip(session: &mut Session, frames: u64) -> ClipId {
        let rate = session.project().sample_rate;
        let track = session.project.add_audio_track("Take");
        let source = session.project.add_audio_source(
            "take",
            AssetPath::external("/audio/take.wav"),
            frames,
            rate,
            2,
        );
        session
            .project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("the track was just added")
    }

    /// The audio clip's stored shape, read back for assertions.
    fn audio_shape(session: &Session, clip: ClipId) -> (f32, u64, u64) {
        let audio = session
            .project()
            .tracks
            .iter()
            .find_map(|track| track.kind.as_audio()?.clips.iter().find(|c| c.id == clip))
            .expect("the clip exists");
        (audio.gain_db, audio.fade_in_frames, audio.fade_out_frames)
    }

    /// How many source frames an audio clip plays.
    fn audio_frames(session: &Session, clip: ClipId) -> u64 {
        session
            .project()
            .audio_clip(clip)
            .expect("the clip exists")
            .length_frames
    }

    #[test]
    fn clip_gain_belongs_to_audio_and_comes_back_on_undo() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        let track = session.add_default_instrument_track("Lead").unwrap();
        let midi = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();

        session.set_clip_gain(clip, -6.0).unwrap();
        assert_eq!(audio_shape(&session, clip).0, -6.0);
        // Way past the range is the nearest gain that exists, not an error.
        session.set_clip_gain(clip, 100.0).unwrap();
        assert_eq!(audio_shape(&session, clip).0, 24.0);
        // NaN has no nearest anything and is refused outright.
        assert!(matches!(
            session.set_clip_gain(clip, f32::NAN),
            Err(SessionError::NotFinite(_))
        ));
        // A note clip's loudness is its velocities; addressing it here says so.
        assert!(matches!(
            session.set_clip_gain(midi, 0.0),
            Err(SessionError::NotAudio(_))
        ));
        assert!(matches!(
            session.set_clip_gain(ClipId(9_999), 0.0),
            Err(SessionError::UnknownClip(_))
        ));

        // A value that has not moved is not an edit.
        session.set_clip_gain(clip, 24.0).unwrap();
        assert_eq!(session.undo(), Some(Edit::SetClipGain));
        assert_eq!(session.undo(), Some(Edit::SetClipGain));
        assert_eq!(audio_shape(&session, clip).0, 0.0);
        assert!(!session.can_undo());
    }

    #[test]
    fn fades_fit_the_clip_and_never_cross() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        session.set_clip_fades(clip, 10_000, 6_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 10_000, 6_000));
        // A fade asked for past the end takes the whole clip and leaves the other nothing.
        session.set_clip_fades(clip, 96_000, 6_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 48_000, 0));
        // Two that would cross meet instead: the fade-out takes what the fade-in leaves.
        session.set_clip_fades(clip, 30_000, 30_000).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 30_000, 18_000));
        // Writing what is already there is not an edit.
        session.set_clip_fades(clip, 30_000, 18_000).unwrap();
        assert_eq!(undo_depth(&mut session), 3);
    }

    #[test]
    fn shrinking_a_clip_keeps_its_fades_inside_it() {
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.set_clip_fades(clip, 30_000, 18_000).unwrap();
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats; dragging the end to beat one
        // halves the clip to 24 000 frames, which the fades must fit inside.
        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        assert_eq!(audio_shape(&session, clip), (0.0, 24_000, 0));
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(audio_shape(&session, clip), (0.0, 30_000, 18_000));
    }

    #[test]
    fn an_audio_clip_cannot_be_dragged_past_the_end_of_its_material() {
        // The right edge is a trim, and there is nothing past the last frame to trim to. Left
        // unbounded the clip drew — and saved — a block of silence with the waveform stopping
        // part way, which the renderer then clamped anyway: the picture and the sound disagreed.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats. Dragging the end to bar three asks
        // for four beats and gets the two that exist.
        session.resize_clip(clip, Ticks::QUARTER * 4).unwrap();
        assert_eq!(audio_shape(&session, clip).1, 0, "fades were not touched");
        assert_eq!(audio_frames(&session, clip), 48_000);
        assert!(
            !session.can_undo(),
            "a drag that could not lengthen the clip is not an edit"
        );

        // Shortening still works, and lengthening afterwards comes back to the whole source.
        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        assert_eq!(audio_frames(&session, clip), 24_000);
        session.resize_clip(clip, Ticks::QUARTER * 8).unwrap();
        assert_eq!(audio_frames(&session, clip), 48_000);
    }

    #[test]
    fn dragging_a_generated_clip_longer_writes_the_part_again_to_fill_it() {
        // A generated clip is its recipe, not its notes: the notes were written to fill a
        // length, so a new length wants them written again. Dragged out it used to gain a tail
        // of silence, and dragged in it kept notes hanging past its own end.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_chord(Ticks::ZERO, numeral("I"));
        let recipe = ClipRecipe::new(ClipPreset::Chords, 7);
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 2, recipe)
            .unwrap();
        let two_bars = session.midi_clip(clip).unwrap().notes.len();
        assert!(two_bars > 0, "the fixture wrote nothing to begin with");
        session.forget_history();

        session.resize_clip(clip, BAR * 4).unwrap();
        let four_bars = session.midi_clip(clip).unwrap().notes.len();
        assert!(
            four_bars > two_bars,
            "four bars of the same part wrote {four_bars} notes against {two_bars}"
        );
        assert!(
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .any(|note| note.start >= BAR * 2),
            "the new bars are empty"
        );

        // One drag, one undo step — and it puts back both the length and the notes.
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(session.midi_clip(clip).unwrap().length, BAR * 2);
        assert_eq!(session.midi_clip(clip).unwrap().notes.len(), two_bars);
    }

    #[test]
    fn dragging_a_played_clip_leaves_its_notes_exactly_where_they_are() {
        // The other half of the rule: a clip with no recipe is notes somebody put there, and
        // resizing it must not invent or discard any of them.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let before = session.midi_clip(clip).unwrap().notes.clone();

        session.resize_clip(clip, BAR * 3).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes, before);
        assert_eq!(session.midi_clip(clip).unwrap().length, BAR * 3);
    }

    #[test]
    fn trimming_an_audio_clip_from_the_front_moves_its_window_into_the_source() {
        // The difference between a trim and a move: the material under the clip has to stay
        // where it sounds. Walking `start` without walking `offset_frames` would slide the whole
        // take along the timeline and call it a trim.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.forget_history();

        // 48 000 frames at 120 BPM and 48 kHz is two beats. Trimming to beat two hides the first
        // 24 000 frames and leaves the end where it was.
        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.start, Ticks::QUARTER);
        assert_eq!(audio.offset_frames, 24_000);
        assert_eq!(audio.length_frames, 24_000);

        // Dragging back out uncovers what was hidden rather than repeating what is left.
        session.trim_clip_start(clip, Ticks::ZERO).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.offset_frames, 0);
        assert_eq!(audio.length_frames, 48_000);

        // And it stops at the source's first frame: there is nothing before it to uncover.
        session.trim_clip_start(clip, -Ticks::QUARTER * 4).unwrap();
        let audio = session.project().audio_clip(clip).unwrap();
        assert_eq!(audio.offset_frames, 0);
        assert_eq!(audio.length_frames, 48_000);
        assert_eq!(audio.start, Ticks::ZERO);
    }

    #[test]
    fn a_trimmed_clip_moved_to_the_start_cannot_uncover_what_will_not_fit() {
        // Its window still has material behind it, and there is nowhere on the timeline to put
        // it. Clamping the tick alone would leave the start pinned at bar one while the window
        // kept walking backwards — and the far end would slide right, off a drag aimed at the
        // left edge.
        let mut session = session();
        let clip = audio_clip(&mut session, 48_000);
        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        session.move_clip(clip, Ticks::ZERO).unwrap();
        session.forget_history();

        let before = session.project().audio_clip(clip).unwrap().clone();
        session.trim_clip_start(clip, -Ticks::QUARTER * 4).unwrap();
        let after = session.project().audio_clip(clip).unwrap();
        assert_eq!(after.start, Ticks::ZERO);
        assert_eq!(after.offset_frames, before.offset_frames);
        assert_eq!(after.length_frames, before.length_frames);
        assert!(
            !session.can_undo(),
            "an edge with nowhere to go is not an edit"
        );
    }

    #[test]
    fn trimming_a_generated_clip_from_the_front_writes_it_again_over_what_is_left() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_chord(Ticks::ZERO, numeral("I"));
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Chords, 7),
            )
            .unwrap();
        session.forget_history();

        session.trim_clip_start(clip, BAR * 2).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR * 2);
        assert_eq!(midi.length, BAR * 2);
        assert!(!midi.notes.is_empty(), "the two bars left are empty");
        assert!(
            midi.notes.iter().all(|note| note.end() <= BAR * 2),
            "a note hangs past the clip it was written into"
        );
        assert_eq!(session.undo(), Some(Edit::ResizeClip));
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
    }

    #[test]
    fn trimming_a_played_clip_from_the_front_rebases_the_notes_it_keeps() {
        // A played clip's notes are nobody's to reinvent, so they move with the edge. The rule is
        // the one a split already follows: a note the cut runs through keeps its sounding half.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR * 2)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER * 3))
            .unwrap();
        session
            .add_note(clip, Note::new(67, BAR, Ticks::QUARTER))
            .unwrap();

        session.trim_clip_start(clip, Ticks::QUARTER * 2).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, Ticks::QUARTER * 2);
        assert_eq!(midi.length, BAR * 2 - Ticks::QUARTER * 2);
        // The first note is gone, the second keeps the half the cut left it, the third moved.
        let kept: Vec<(u8, i64, i64)> = midi
            .notes
            .iter()
            .map(|note| (note.pitch, note.start.raw(), note.length.raw()))
            .collect();
        assert_eq!(
            kept,
            vec![
                (64, 0, Ticks::QUARTER.raw() * 2),
                (67, Ticks::QUARTER.raw() * 2, Ticks::QUARTER.raw()),
            ]
        );
    }

    #[test]
    fn neither_edge_may_be_dragged_past_the_other() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session.add_midi_clip(track, "Riff", BAR, BAR * 2).unwrap();

        // The front stops a grid division short of the end rather than turning the clip inside
        // out, which is the same floor the other edge keeps.
        session.trim_clip_start(clip, BAR * 9).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.length, session.project().grid);
        assert_eq!(midi.start + midi.length, BAR * 3, "the end moved");

        session.resize_clip(clip, Ticks::ZERO).unwrap();
        assert_eq!(
            session.midi_clip(clip).unwrap().length,
            session.project().grid
        );
    }

    #[test]
    fn a_clip_already_shorter_than_the_grid_refuses_to_be_trimmed_from_the_front() {
        // A clip shorter than a grid division is ordinary — a piece of a split, a clip drawn
        // before the grid was made coarser — and the floor the front stops at then sits behind
        // the clip's own start. Taken as a ceiling it dragged the start *backwards* on the first
        // mouse-move of a gesture with no threshold, lengthening a clip nobody asked to lengthen
        // and, in the first bar, pushing its start below zero.
        let mut session = session();
        session.set_grid(BAR);
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();

        session.trim_clip_start(clip, Ticks::QUARTER).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, Ticks::ZERO, "the start moved, and backwards");
        assert_eq!(
            midi.length,
            Ticks::QUARTER,
            "the clip grew of its own accord"
        );
        assert!(
            !session.can_undo(),
            "an edge with nowhere to go is not an edit"
        );

        // Dragging the other way is still a lengthening, because uncovering earlier material is
        // never the thing that runs out of room.
        let clip = session
            .add_midi_clip(track, "Short", BAR * 2, Ticks::QUARTER)
            .unwrap();
        session.trim_clip_start(clip, BAR).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR);
        assert_eq!(midi.length, BAR + Ticks::QUARTER, "the end moved");

        // And a clip longer than the grid trims exactly as it did: to where it was asked, and no
        // further than a division short of its own end.
        let clip = session
            .add_midi_clip(track, "Long", Ticks::ZERO, BAR * 4)
            .unwrap();
        session.trim_clip_start(clip, BAR).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR);
        assert_eq!(midi.length, BAR * 3);

        session.trim_clip_start(clip, BAR * 9).unwrap();
        let midi = session.midi_clip(clip).unwrap();
        assert_eq!(midi.start, BAR * 3);
        assert_eq!(midi.length, BAR);
    }

    #[test]
    fn cycling_is_listening_and_does_not_land_on_the_undo_stack() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();

        for _ in 0..4 {
            session.set_loop_enabled(true);
            session.set_loop_enabled(false);
        }
        assert_eq!(
            session.undo(),
            Some(Edit::AddClip),
            "the edits are still there"
        );
    }

    #[test]
    fn a_clip_dragged_shorter_stays_shorter() {
        // The trimmed tail is still in the note list, and `fit_length_to_notes` grew the clip
        // back to cover it on the next edit — material the user had just cut reappeared and
        // started sounding again, with nothing on screen to explain it.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(clip, Note::new(67, Ticks::QUARTER * 2, Ticks::QUARTER))
            .unwrap();

        session.resize_clip(clip, Ticks::QUARTER).unwrap();
        let trimmed = session.midi_clip(clip).unwrap().length;
        assert!(
            trimmed < Ticks::QUARTER * 2,
            "the second note is past the end now"
        );

        session
            .add_note(clip, Note::new(64, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        assert_eq!(
            session.midi_clip(clip).unwrap().length,
            trimmed,
            "the next note edit must not grow it back",
        );
    }

    #[test]
    fn a_clip_that_has_never_been_resized_still_grows_to_hold_what_is_written_in_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::QUARTER * 4, Ticks::QUARTER))
            .unwrap();
        assert!(session.midi_clip(clip).unwrap().length > Ticks::QUARTER * 4);
    }

    #[test]
    fn how_hard_a_note_is_struck_can_be_changed() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let first = session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let second = session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();

        session
            .set_note_velocity(clip, &[first, second], 0.25)
            .unwrap();
        let velocities: Vec<f32> = session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .map(|note| note.velocity)
            .collect();
        assert_eq!(velocities, vec![0.25, 0.25]);

        assert_eq!(session.undo(), Some(Edit::SetNoteVelocity));
        assert!(session.midi_clip(clip).unwrap().notes[0].velocity > 0.25);

        // Out of range is clamped rather than refused, and a set that changes nothing is not an
        // edit — applying a marking to a chord already at it should not push an undo step.
        session.redo();
        session.set_note_velocity(clip, &[first], 4.0).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].velocity, 1.0);
        let depth = undo_depth(&mut session);
        session.set_note_velocity(clip, &[first], 1.0).unwrap();
        assert_eq!(undo_depth(&mut session), depth);
    }

    #[test]
    fn a_chord_can_be_played_harder_without_losing_its_shape() {
        // What a velocity drag over a selection needs: each note goes somewhere of its own, so
        // the phrasing written into the part survives being made louder.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let quiet = session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let loud = session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();

        session
            .set_note_velocities(clip, &[(quiet, 0.4), (loud, 0.6)])
            .unwrap();
        let velocities = |session: &Session| -> Vec<f32> {
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .map(|note| note.velocity)
                .collect()
        };
        assert_eq!(velocities(&session), vec![0.4, 0.6]);

        // One undo step for the pair, not one each: the whole gesture is a single edit.
        let depth = undo_depth(&mut session);
        session
            .set_note_velocities(clip, &[(quiet, 0.5), (loud, 0.7)])
            .unwrap();
        assert_eq!(velocities(&session), vec![0.5, 0.7]);
        assert_eq!(undo_depth(&mut session), depth + 1);
        assert_eq!(session.undo(), Some(Edit::SetNoteVelocity));
        assert_eq!(velocities(&session), vec![0.4, 0.6]);

        // A note that has gone is skipped, and the rest of the gesture still lands. A selection
        // is held by position, so one missing index must not throw the others away.
        session.remove_notes(clip, &[loud]).unwrap();
        session
            .set_note_velocities(clip, &[(quiet, 0.9), (loud, 0.9)])
            .unwrap();
        assert_eq!(velocities(&session), vec![0.9]);

        assert!(matches!(
            session.set_note_velocities(ClipId(9999), &[(0, 0.5)]),
            Err(SessionError::UnknownClip(_))
        ));
    }

    #[test]
    fn saving_over_another_project_is_refused_until_it_is_agreed_to() {
        // The system save dialog offers to replace the *name* that was typed. A project is
        // written one folder deeper than that, so the dialog never saw the document that would
        // actually be destroyed and asked nothing.
        let scratch = Scratch::new("would-replace");
        let mut first = session();
        first.add_default_instrument_track("Old").unwrap();
        let existing = first
            .save_as(&scratch.join("Ballad.auris"))
            .expect("saves")
            .document;
        assert!(existing.exists());

        let mut second = session();
        second.add_default_instrument_track("New").unwrap();
        let refused = second.save_as(&scratch.join("Ballad.auris")).unwrap_err();
        assert!(
            matches!(&refused, SessionError::WouldReplace(path) if *path == existing),
            "the error names the document that would go, not the name that was typed",
        );
        assert!(
            second.is_dirty(),
            "nothing was written, so nothing is saved"
        );

        // And with the replacement agreed to it goes ahead.
        second
            .save_as_replacing(&scratch.join("Ballad.auris"))
            .expect("replaces");
        let reopened = {
            let mut session = session();
            session.open(&existing).expect("opens");
            session.project().tracks[0].name.clone()
        };
        assert_eq!(reopened, "New");
    }

    #[test]
    fn saving_back_over_this_project_is_not_a_replacement() {
        let scratch = Scratch::new("save-over-itself");
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        let document = session
            .save_as(&scratch.join("Ballad.auris"))
            .expect("saves")
            .document;

        session.add_default_instrument_track("Bass").unwrap();
        let again = session.save_as(&document).expect("saves over itself");
        assert_eq!(again.document, document);
    }

    #[test]
    fn unknown_plugin_ids_are_refused_rather_than_stored() {
        let mut session = session();
        let error = session
            .add_instrument_track("Ghost", "nobody.synth.missing")
            .unwrap_err();
        assert!(matches!(error, SessionError::UnknownPlugin(_)));
        assert!(session.project().tracks.is_empty());

        let track = session.add_default_instrument_track("Real").unwrap();
        assert!(matches!(
            session.add_effect(Some(track), "nobody.fx.missing"),
            Err(SessionError::UnknownPlugin(_))
        ));
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .mixer
                .effects
                .is_empty()
        );
    }

    #[test]
    fn a_midi_clip_cannot_be_added_to_an_audio_track() {
        let mut session = session();
        let track = session.add_audio_track("Audio");
        let error = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap_err();
        assert!(matches!(error, SessionError::WrongTrackKind { .. }));
    }

    #[test]
    fn parameters_round_trip_through_the_document() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let descriptor = session.descriptor_for(target).unwrap();

        let value = descriptor.clamp(descriptor.max);
        session.set_param(target, value);
        assert_eq!(session.param_value(target, &descriptor), value);

        // The value must land in the saved state under the descriptor's stable key.
        let state = &session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        assert_eq!(state.params.get(descriptor.key.as_ref()), Some(&value));
    }

    #[test]
    fn mixer_controls_get_synthesised_descriptors() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();

        let gain = session
            .descriptor_for(ParamTarget::TrackGain(track))
            .unwrap();
        assert_eq!(gain.format(0.0), "+0.0 dB");
        session.set_param(ParamTarget::TrackGain(track), -6.0);
        assert_eq!(session.project().track(track).unwrap().mixer.gain_db, -6.0);

        let pan = session.descriptor_for(ParamTarget::MasterPan).unwrap();
        assert_eq!(pan.format(0.0), "C");
        session.set_param(ParamTarget::MasterPan, 1.0);
        assert_eq!(session.project().master.pan, 1.0);
    }

    #[test]
    fn changing_the_instrument_discards_the_old_plugin_state() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let first = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        session.set_param(first, 1.0);
        assert!(session.set_automation_point(first, Ticks::ZERO, 1.0));

        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let state = &session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        assert!(
            state.params.is_empty(),
            "another plugin's values must not survive the swap"
        );
        // A lane names the track and the parameter's index, never the plugin, so one left behind
        // would go on sweeping whatever the new instrument keeps at that index.
        assert!(
            session.automation().lane(first).is_none(),
            "another plugin's curve must not survive the swap either"
        );

        // The removal sits after the `record`, so the whole edit comes back together.
        assert_eq!(session.undo(), Some(Edit::ChangeInstrument));
        assert!(
            session.automation().lane(first).is_some(),
            "undo put the instrument back without its automation"
        );
    }

    #[test]
    fn choosing_a_sound_drops_the_lanes_that_drove_the_old_instrument() {
        // Picking a preset switches a track off its synth, which is a change of plugin like any
        // other — and the lanes belonged to the synth.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        let first = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        assert!(session.set_automation_point(first, Ticks::ZERO, 1.0));

        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 40,
                },
            )
            .expect("chosen");
        assert!(
            session.automation().lane(first).is_none(),
            "the synth's curve stayed behind to drive the sampler"
        );

        assert_eq!(session.undo(), Some(Edit::ChoosePreset));
        assert!(
            session.automation().lane(first).is_some(),
            "undo put the instrument back without its automation"
        );
    }

    #[test]
    fn notes_are_clamped_into_range() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();

        let index = session
            .add_note(
                clip,
                Note {
                    pitch: 200,
                    velocity: 5.0,
                    start: Ticks(-500),
                    length: Ticks(0),
                },
            )
            .unwrap();
        let note = session.midi_clip(clip).unwrap().notes[index];
        assert_eq!(note.pitch, 127);
        assert_eq!(note.velocity, 1.0);
        assert_eq!(note.start, Ticks::ZERO);
        assert!(note.length.raw() >= 1);
    }

    #[test]
    fn removing_notes_takes_the_ones_asked_for() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        for pitch in [60, 62, 64, 65] {
            session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }

        // Out-of-range and duplicate indices must not disturb the rest.
        session.remove_notes(clip, &[0, 2, 2, 99]).unwrap();
        let pitches: Vec<u8> = session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(pitches, vec![62, 65]);
    }

    #[test]
    fn a_project_round_trips_through_a_file() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.set_bpm(96.0);

        let path = std::env::temp_dir().join("auris-session-round-trip.auris");
        session.save(&path).unwrap();
        assert!(!session.is_dirty());
        let saved = session.project().clone();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&path).unwrap();
        assert!(missing.is_empty());
        assert_eq!(reopened.project(), &saved);
        assert!(!reopened.can_undo(), "opening must not be undoable");

        std::fs::remove_file(&path).ok();
    }

    // ------------------------------------------------------------- project folders

    /// A directory under the system temp area that deletes itself when the test ends.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "auris-session-{}-{unique}-{name}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a temp directory can be made");
            Self { path }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        /// Writes a short tone so `import_audio` has a real file to decode.
        fn tone(&self, name: &str) -> PathBuf {
            write_tone(&self.join(name), 480)
        }
    }

    /// Writes a decodable tone of `frames` frames wherever it is asked to.
    ///
    /// The length is a parameter because the tests about a file that moved turn on two files of
    /// the same name being different files, and the length is what makes them different sizes.
    fn write_tone(path: &Path, frames: usize) -> PathBuf {
        let mut buffer = AudioBuffer::new(2, frames, 48_000.0);
        for channel in 0..2 {
            for (frame, sample) in buffer.channel_mut(channel).iter_mut().enumerate() {
                *sample = (frame as f32 * 0.01).sin() * 0.5;
            }
        }
        auris_io::write_wav(path, &buffer, &auris_io::WavExportSettings::default())
            .expect("a WAV file writes");
        path.to_path_buf()
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn saving_under_a_new_name_gathers_the_song_into_one_folder() {
        let scratch = Scratch::new("gather");
        let loose = scratch.tone("kick.wav");

        let mut session = session();
        session.import_audio(&loose, Ticks::ZERO).expect("imports");
        let document = session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves")
            .document;

        assert_eq!(document, scratch.join("MySong").join("MySong.auris"));
        assert!(
            scratch
                .join("MySong")
                .join("Audio")
                .join("kick.wav")
                .is_file(),
            "the audio has to travel with the document"
        );
        let source = session.project().audio_sources.values().next().unwrap();
        assert_eq!(
            source.path,
            AssetPath::inside(Path::new("Audio").join("kick.wav")),
            "and the document has to refer to its own copy"
        );
    }

    #[test]
    fn a_project_folder_that_has_been_moved_still_opens() {
        // The whole reason for relative references. Nothing here touches the document: the folder
        // is renamed underneath it, which is what a person dragging it somewhere else does.
        let scratch = Scratch::new("moved");
        let loose = scratch.tone("kick.wav");

        let mut session = session();
        session.import_audio(&loose, Ticks::ZERO).expect("imports");
        assert_eq!(
            session
                .save_as(&scratch.join("Before.auris"))
                .expect("saves")
                .document,
            scratch.join("Before").join("Before.auris")
        );
        drop(session);
        std::fs::remove_file(&loose).expect("the file it was imported from goes away too");

        let moved = scratch.join("After");
        std::fs::rename(scratch.join("Before"), &moved).expect("the folder moves");

        let mut reopened = self::tests::session();
        let missing = reopened
            .open(&moved.join("Before.auris"))
            .expect("the moved project opens");
        assert!(missing.is_empty(), "nothing should be missing: {missing:?}");
        assert_eq!(reopened.project().audio_sources.len(), 1);
    }

    #[test]
    fn audio_imported_into_a_saved_project_is_copied_in_at_once() {
        let scratch = Scratch::new("import-after-save");
        let mut session = session();
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");

        let loose = scratch.tone("snare.wav");
        session.import_audio(&loose, Ticks::ZERO).expect("imports");

        assert!(
            scratch
                .join("MySong")
                .join("Audio")
                .join("snare.wav")
                .is_file()
        );
        let source = session.project().audio_sources.values().next().unwrap();
        assert!(source.path.is_inside());
    }

    #[test]
    fn a_soundfont_is_referred_to_where_it_lies() {
        // The policy that pays for itself: a font is a library, and twenty projects using one
        // must not mean twenty copies of it.
        let scratch = Scratch::new("font-external");
        let font = scratch.join("GM.sf2");
        std::fs::write(&font, b"not a real font").unwrap();

        let mut session = session();
        // The file is not a SoundFont, so the import fails — but the document must not have
        // gained a reference to it either way.
        assert!(session.import_soundfont(&font).is_err());

        session
            .project
            .add_soundfont("GM", AssetPath::external(&font), auris_io::byte_size(&font));
        let stored = session.project().soundfonts.values().next().unwrap();
        assert!(!stored.path.is_inside());
        assert_eq!(stored.byte_size, 15);
    }

    #[test]
    fn collecting_brings_the_fonts_in_too() {
        let scratch = Scratch::new("collect");
        let font = scratch.join("GM.sf2");
        std::fs::write(&font, b"stand-in for a very large font").unwrap();

        let mut session = session();
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");
        session
            .project
            .add_soundfont("GM", AssetPath::external(&font), auris_io::byte_size(&font));

        assert_eq!(session.collect_assets().expect("collects"), 1);
        assert!(
            scratch
                .join("MySong")
                .join("Audio")
                .join("GM.sf2")
                .is_file()
        );
        assert!(
            session
                .project()
                .soundfonts
                .values()
                .next()
                .unwrap()
                .path
                .is_inside()
        );
        assert_eq!(
            session.collect_assets().expect("collects again"),
            0,
            "nothing is left outside, so a second run copies nothing"
        );
    }

    #[test]
    fn saving_under_a_new_name_takes_a_font_the_project_already_owns_with_it() {
        // A collected font lives in the *old* folder. Carrying its reference across unchanged
        // reported success, went on sounding here from the samples already in memory, and opened
        // silent on the machine the copy was made for.
        let scratch = Scratch::new("font-travels");
        let library = scratch.join("GM.sf2");
        std::fs::write(&library, b"stand-in for a very large font").unwrap();

        let mut session = session();
        session
            .save_as(&scratch.join("First.auris"))
            .expect("saves");
        session.project.add_soundfont(
            "GM",
            AssetPath::external(&library),
            auris_io::byte_size(&library),
        );
        assert_eq!(session.collect_assets().expect("collects"), 1);
        // The library it came from goes away, so nothing below can be reading the original.
        std::fs::remove_file(&library).unwrap();

        let report = session
            .save_as(&scratch.join("Second.auris"))
            .expect("saves again");
        assert!(
            report.uncollected.is_empty(),
            "nothing should have been left behind: {:?}",
            report.uncollected
        );

        let second = scratch.join("Second");
        assert!(
            second.join(AUDIO_DIR).join("GM.sf2").is_file(),
            "a font the project owns has to travel with it"
        );
        assert!(
            scratch
                .join("First")
                .join(AUDIO_DIR)
                .join("GM.sf2")
                .is_file(),
            "and Save As copies rather than moves, so the project saved from still has its own"
        );
        let stored = session.project().soundfonts.values().next().unwrap();
        assert_eq!(
            stored.path.resolve(session.project_folder()),
            Some(second.join(AUDIO_DIR).join("GM.sf2")),
            "the stored reference has to resolve to the copy in the new folder"
        );
    }

    #[test]
    fn saving_under_a_new_name_leaves_a_font_in_its_library_alone() {
        // The policy Save As must not quietly change: a font is shared by every project that uses
        // it, and `collect_assets` is the opt-in that pays for a copy.
        let scratch = Scratch::new("font-stays");
        let library = scratch.join("GM.sf2");
        std::fs::write(&library, b"stand-in for a very large font").unwrap();

        let mut session = session();
        session.project.add_soundfont(
            "GM",
            AssetPath::external(&library),
            auris_io::byte_size(&library),
        );
        session
            .save_as(&scratch.join("MySong.auris"))
            .expect("saves");

        assert!(
            !scratch
                .join("MySong")
                .join(AUDIO_DIR)
                .join("GM.sf2")
                .exists(),
            "hundreds of megabytes per save is what this policy exists to avoid"
        );
        assert_eq!(
            session.project().soundfonts.values().next().unwrap().path,
            AssetPath::external(&library)
        );
    }

    #[test]
    fn collecting_needs_somewhere_to_collect_into() {
        let mut session = session();
        assert!(matches!(
            session.collect_assets(),
            Err(SessionError::NoPath)
        ));
    }

    #[test]
    fn an_audio_file_that_moved_next_to_the_project_is_found_again() {
        // A version 1 document names its audio absolutely. Copying the project folder to another
        // machine breaks that path, and the file sitting in `Audio/` is the obvious candidate.
        let scratch = Scratch::new("relocate");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();
        // Put the file where a collected project would have it, and break the stored path.
        std::fs::rename(
            scratch.join("kick.wav"),
            folder.join(AUDIO_DIR).join("kick.wav"),
        )
        .unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert!(missing.is_empty(), "the file is right there: {missing:?}");
        assert_eq!(
            reopened.project().audio_sources[&source].path,
            AssetPath::inside(Path::new(AUDIO_DIR).join("kick.wav")),
            "and finding it must be written down, so it is found once rather than every time"
        );
        assert!(
            reopened.is_dirty(),
            "the repair is an unsaved change like any other"
        );
    }

    #[test]
    fn a_sample_of_the_wrong_size_wearing_the_name_is_not_adopted() {
        // A plain `save` collects nothing, so the document points outside its own folder — and
        // that file has gone. Something else called `kick.wav` is sitting in `Audio/`. Playing
        // that instead of reporting the sample missing is a wrong answer nobody is told about,
        // and Collect Assets afterwards writes it into the document for good.
        let scratch = Scratch::new("decoy");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();

        std::fs::remove_file(scratch.join("kick.wav")).unwrap();
        write_tone(&folder.join(AUDIO_DIR).join("kick.wav"), 4_800);

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert_eq!(missing.len(), 1, "a different file is not the file");
        assert!(
            !reopened.project().audio_sources[&source].path.is_inside(),
            "and the reference must not be rewritten to point at the impostor"
        );
    }

    #[test]
    fn a_sample_of_the_right_size_wearing_the_name_is_still_found() {
        // The other half of the same rule: the size confirms a candidate, it must not veto one.
        // The copy in `Audio/` is a different file on disk holding the same bytes, which is what
        // a project someone copied folder-first looks like.
        let scratch = Scratch::new("twin");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(folder.join(AUDIO_DIR)).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        let source = session.project().audio_sources.values().next().unwrap().id;
        session.save(&folder.join("MySong.auris")).unwrap();

        std::fs::copy(
            scratch.join("kick.wav"),
            folder.join(AUDIO_DIR).join("kick.wav"),
        )
        .unwrap();
        std::fs::remove_file(scratch.join("kick.wav")).unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert!(missing.is_empty(), "the file is right there: {missing:?}");
        assert_eq!(
            reopened.project().audio_sources[&source].path,
            AssetPath::inside(Path::new(AUDIO_DIR).join("kick.wav")),
            "and finding it is written down, so it is found once rather than every time"
        );
    }

    #[test]
    fn a_file_that_is_really_gone_is_reported_rather_than_guessed_at() {
        let scratch = Scratch::new("gone");
        let folder = scratch.join("MySong");
        std::fs::create_dir_all(&folder).unwrap();

        let mut session = session();
        session
            .import_audio(&scratch.tone("kick.wav"), Ticks::ZERO)
            .unwrap();
        session.save(&folder.join("MySong.auris")).unwrap();
        std::fs::remove_file(scratch.join("kick.wav")).unwrap();

        let mut reopened = self::tests::session();
        let missing = reopened.open(&folder.join("MySong.auris")).unwrap();
        assert_eq!(missing.len(), 1, "the project opens, and says what is gone");
        assert_eq!(reopened.project().tracks.len(), 1);
    }

    #[test]
    fn new_project_clears_the_history_and_the_path() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.new_project();
        assert!(!session.can_undo());
        assert!(session.path().is_none());
        assert_eq!(session.project().tracks.len(), 1);
    }

    /// A session with one instrument track holding a one-bar clip of two notes.
    fn session_with_clip() -> (Session, TrackId, ClipId) {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();
        (session, track, clip)
    }

    #[test]
    fn duplicating_a_track_is_one_undo_step() {
        let (mut session, track, _) = session_with_clip();
        let copy = session.duplicate_track(track).unwrap();
        assert_eq!(session.project().tracks.len(), 2);
        assert_ne!(copy, track);

        session.undo().unwrap();
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn a_duplicated_clip_can_be_edited_without_touching_the_original() {
        let (mut session, _, clip) = session_with_clip();
        let copy = session.duplicate_clip(clip).unwrap();

        session
            .move_clip(copy, Ticks::from_beats(16.0))
            .expect("the copy is addressable in its own right");

        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
        assert_eq!(
            session.midi_clip(copy).unwrap().start,
            Ticks::from_beats(16.0)
        );
    }

    #[test]
    fn splitting_outside_a_clip_records_no_undo_step() {
        let (mut session, _, clip) = session_with_clip();
        session.forget_history();

        let error = session.split_clip(clip, Ticks::from_beats(99.0));
        assert!(matches!(error, Err(SessionError::CannotSplit(_))));
        assert!(
            !session.can_undo(),
            "a split that did nothing must not leave an undo step behind"
        );

        let right = session.split_clip(clip, Ticks::from_beats(1.0)).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().length, Ticks::QUARTER);
        assert_eq!(session.midi_clip(right).unwrap().start, Ticks::QUARTER);
        assert!(session.can_undo());
    }

    #[test]
    fn moving_several_clips_keeps_the_spacing_between_them() {
        let (mut session, track, first) = session_with_clip();
        let second = session
            .add_midi_clip(track, "B", Ticks::from_beats(8.0), Ticks::from_beats(4.0))
            .unwrap();
        let origins = [(first, Ticks::ZERO), (second, Ticks::from_beats(8.0))];

        // Far enough left that the first clip would go negative on its own.
        session.move_clips(&origins, Ticks::from_beats(-4.0));

        assert_eq!(session.midi_clip(first).unwrap().start, Ticks::ZERO);
        assert_eq!(
            session.midi_clip(second).unwrap().start,
            Ticks::from_beats(8.0),
            "the whole selection stops when the earliest clip reaches zero"
        );
    }

    #[test]
    fn deleting_several_clips_is_one_undo_step() {
        let (mut session, track, first) = session_with_clip();
        let second = session
            .add_midi_clip(track, "B", Ticks::from_beats(8.0), Ticks::from_beats(4.0))
            .unwrap();
        session.forget_history();

        session
            .remove_clips(&[first, second, ClipId(9999)])
            .unwrap();
        assert!(session.midi_clip(first).is_none());
        assert!(session.midi_clip(second).is_none());

        session.undo().unwrap();
        assert!(session.midi_clip(first).is_some());
        assert!(session.midi_clip(second).is_some());
    }

    #[test]
    fn duplicated_notes_chain_after_the_selection() {
        let (mut session, _, clip) = session_with_clip();
        let copies = session.duplicate_notes(clip, &[0, 1]).unwrap();
        assert_eq!(copies, vec![2, 3]);

        let notes = &session.midi_clip(clip).unwrap().notes;
        // The selection spans two quarters, so the copies start one half-note along.
        assert_eq!(notes[2].start, Ticks::from_beats(2.0));
        assert_eq!(notes[2].pitch, 60);
        assert_eq!(notes[3].start, Ticks::from_beats(3.0));
        assert_eq!(notes[3].pitch, 64);
    }

    #[test]
    fn transposing_keeps_the_intervals_when_it_would_run_off_the_end() {
        let (mut session, _, clip) = session_with_clip();
        session
            .add_note(clip, Note::new(120, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        // +12 would push 120 past 127, so the whole selection moves by 7 instead.
        session.transpose_notes(clip, &[0, 1, 2], 12).unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes[0].pitch, 67);
        assert_eq!(notes[1].pitch, 71);
        assert_eq!(notes[2].pitch, 127);
        assert_eq!(
            notes[1].pitch - notes[0].pitch,
            4,
            "a clamped transposition must not flatten the interval"
        );
    }

    #[test]
    fn a_muted_clip_is_silent_but_still_present() {
        let (mut session, _, clip) = session_with_clip();
        session.set_clip_muted(clip, true).unwrap();
        assert!(session.midi_clip(clip).unwrap().muted);

        let rendered = session
            .render_job()
            .render(&auris_engine::OfflineOptions::whole_project(), &mut |_| {})
            .unwrap();
        assert!(rendered.peak() < 1e-6, "a muted clip must not sound");

        session.undo().unwrap();
        assert!(!session.midi_clip(clip).unwrap().muted);
    }

    // ------------------------------------------------------------------ harmony

    /// One bar of 4/4, which is what every harmony test below counts in.
    const BAR: Ticks = Ticks(3840);

    fn numeral(text: &str) -> Numeral {
        Numeral::parse(text).expect("a numeral the test wrote itself")
    }

    #[test]
    fn a_new_project_is_in_c_major_with_nothing_written_in_it() {
        let session = session();
        assert_eq!(session.harmony().key_at(Ticks::ZERO).to_text(), "C major");
        assert!(session.harmony().is_empty());
        assert_eq!(session.harmony().chord_at(Ticks::ZERO), None);
    }

    #[test]
    fn the_key_survives_undo_and_redo() {
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("F# minor").unwrap());
        assert_eq!(session.harmony().key_at(BAR).to_text(), "F# minor");

        assert_eq!(session.undo(), Some(Edit::SetKey));
        assert_eq!(session.harmony().key_at(BAR).to_text(), "C major");
        assert_eq!(session.redo(), Some(Edit::SetKey));
        assert_eq!(session.harmony().key_at(BAR).to_text(), "F# minor");
    }

    #[test]
    fn the_key_at_the_start_of_the_song_cannot_be_removed() {
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("D major").unwrap());
        session.forget_history();

        session.remove_key(Ticks::ZERO);
        assert_eq!(session.harmony().key_at(Ticks::ZERO).to_text(), "D major");
        assert!(
            !session.can_undo(),
            "a command that cannot do anything should not push an undo step"
        );
    }

    #[test]
    fn a_chord_lands_on_the_beat_rather_than_where_the_pointer_was() {
        let mut session = self::tests::session();
        // The editing grid is a sixteenth — 240 ticks — and harmony is written coarser than
        // that: a third of a beat past the bar line means the bar line.
        assert_eq!(session.project().grid, Ticks(240));
        assert_eq!(
            session.harmony_grid_at(Ticks::ZERO),
            Ticks(960),
            "one beat of 4/4"
        );

        session.set_chord(BAR + Ticks(300), numeral("V"));
        assert_eq!(session.harmony().chords.points()[0].tick, BAR);
        assert_eq!(session.harmony().numeral_at(BAR), Some(numeral("V")));

        // Two thirds of the way along is the next beat, not the next sixteenth.
        session.set_chord(BAR + Ticks(700), numeral("IV"));
        assert_eq!(
            session.harmony().chords.points()[1].tick,
            BAR + Ticks(960),
            "rounded up to beat two"
        );
    }

    #[test]
    fn a_grid_coarser_than_a_beat_is_what_a_chord_lands_on() {
        // Somebody who set the editing grid to a bar asked for whole bars, and harmony must not
        // quietly offer them something finer than they chose.
        let mut session = self::tests::session();
        session.set_grid(BAR);
        assert_eq!(session.harmony_grid_at(Ticks::ZERO), BAR);
        session.set_chord(BAR + Ticks(960), numeral("V"));
        assert_eq!(session.harmony().chords.points()[0].tick, BAR);
    }

    #[test]
    fn a_chord_is_removed_and_moved_by_pointing_anywhere_inside_it() {
        // A chord occupies everything up to the next change, and a stamp divides a bar musically
        // — three chords in a bar of 4/4 sit on thirds of it. Neither is reachable by rounding a
        // pointer position onto a grid, so both commands resolve through the change in force.
        let mut session = self::tests::session();
        session.set_chord(BAR, numeral("I"));
        session.set_chord(BAR * 4, numeral("V"));

        assert!(
            session.move_chord(BAR * 2 + Ticks(17), BAR * 3),
            "mid-chord"
        );
        assert_eq!(session.harmony().numeral_at(BAR * 3), Some(numeral("I")));
        assert_eq!(
            session.harmony().numeral_at(BAR * 2),
            None,
            "the chord left where it was, rather than being copied"
        );
        assert_eq!(session.undo(), Some(Edit::MoveChord));
        assert_eq!(session.harmony().numeral_at(BAR * 2), Some(numeral("I")));

        session.remove_chord(BAR * 2 + Ticks(17));
        assert!(session.harmony().numeral_at(BAR).is_none());
        assert_eq!(
            session.harmony().numeral_at(BAR * 4),
            Some(numeral("V")),
            "the one after it is untouched"
        );
    }

    #[test]
    fn nothing_to_move_or_remove_is_not_an_undo_step() {
        let mut session = self::tests::session();
        session.set_chord(BAR * 4, numeral("V"));
        session.forget_history();

        // Before the first chord there is nothing in force to act on.
        assert!(!session.move_chord(Ticks::ZERO, BAR));
        session.remove_chord(Ticks::ZERO);
        // And a move that rounds back onto where the chord already sits changes nothing.
        assert!(!session.move_chord(BAR * 4, BAR * 4 + Ticks(30)));
        assert!(!session.can_undo(), "none of those changed the document");
    }

    #[test]
    fn a_stamped_progression_is_one_undo_step_and_divides_its_bars_musically() {
        let mut session = self::tests::session();
        let written = session
            .stamp_named_progression("axis", Ticks::ZERO, 8)
            .unwrap();
        assert_eq!(written, 8, "four bars of the axis, laid down twice");
        assert_eq!(
            session.harmony().chord_at(Ticks::ZERO).unwrap().to_string(),
            "C"
        );
        assert_eq!(
            session.harmony().chord_at(BAR * 2).unwrap().to_string(),
            "Am"
        );

        // A three-chord bar is three lots of 1280, which is not a grid position — the stamp must
        // not have been snapped to one.
        session.forget_history();
        let chart = Chart::parse("| I V vi |").unwrap();
        session.stamp_progression(&chart, Ticks::ZERO, 1);
        let ticks: Vec<i64> = session
            .harmony()
            .chords
            .points()
            .iter()
            .take(3)
            .map(|point| point.tick.raw())
            .collect();
        assert_eq!(ticks, [0, 1280, 2560]);

        assert_eq!(session.undo(), Some(Edit::StampProgression));
        assert_eq!(
            session.harmony().chord_at(BAR * 2).unwrap().to_string(),
            "Am"
        );
    }

    #[test]
    fn a_progression_can_be_asked_for_by_the_name_a_japanese_musician_uses() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("丸サ", Ticks::ZERO, 0)
            .expect("the catalogue knows it under that name too");
        assert_eq!(
            session.harmony().chord_at(Ticks::ZERO).unwrap().to_string(),
            "Fmaj7",
            "bars of zero means the chart's own length"
        );
        assert_eq!(session.harmony().chords.points().len(), 4);
    }

    #[test]
    fn an_unknown_progression_is_an_error_and_writes_nothing() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        session.forget_history();

        let before = session.project().harmony.clone();
        let error = session
            .stamp_named_progression("marusaa", Ticks::ZERO, 4)
            .unwrap_err();
        assert!(matches!(error, SessionError::UnknownProgression(name) if name == "marusaa"));
        assert_eq!(session.project().harmony, before, "nothing was written");
        assert!(!session.can_undo(), "and nothing was recorded either");
    }

    #[test]
    fn clearing_a_stretch_does_not_silence_what_comes_after_it() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 16)
            .unwrap();

        session.clear_harmony(BAR * 8, BAR * 12);
        assert!(
            session.harmony().chord_at(BAR * 7).is_some(),
            "before the gap"
        );
        assert!(session.harmony().chord_at(BAR * 9).is_none(), "inside it");
        assert_eq!(
            session.harmony().chord_at(BAR * 12).unwrap().to_string(),
            "C",
            "and the song picks up again on the far side"
        );
    }

    #[test]
    fn a_modulation_reharmonises_a_progression_without_rewriting_it() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 8)
            .unwrap();
        let before: Vec<Numeral> = session
            .harmony()
            .chords
            .points()
            .iter()
            .filter_map(|point| point.chord)
            .collect();

        session.set_key(BAR * 4, MusicalKey::parse("Eb major").unwrap());
        assert_eq!(
            session.harmony().chord_at(BAR * 3).unwrap().to_string(),
            "F"
        );
        assert_eq!(
            session.harmony().chord_at(BAR * 4).unwrap().to_string(),
            "Eb"
        );

        let after: Vec<Numeral> = session
            .harmony()
            .chords
            .points()
            .iter()
            .filter_map(|point| point.chord)
            .collect();
        assert_eq!(before, after, "not one chord was touched");
    }

    #[test]
    fn the_harmony_is_saved_and_comes_back() {
        let scratch = Scratch::new("harmony-round-trip");
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("Bb minor").unwrap());
        session
            .stamp_named_progression("axis-minor", Ticks::ZERO, 4)
            .unwrap();
        let written = session.project().harmony.clone();

        let document = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        let mut reopened = self::tests::session();
        reopened.open(&document).unwrap();
        assert_eq!(reopened.project().harmony, written);
        assert_eq!(reopened.harmony().key_at(Ticks::ZERO).to_text(), "Bb minor");
    }

    #[test]
    fn editing_the_harmony_leaves_the_notes_and_the_engine_alone() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR)
            .expect("an empty clip");
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, BAR))
            .unwrap();
        let notes_before = session.project().clone();
        session.forget_history();

        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        assert_eq!(
            session.project().tracks,
            notes_before.tracks,
            "harmony is not a note and must not move one"
        );
    }

    // ------------------------------------------------- hearing the harmony

    #[test]
    fn the_chord_under_a_position_can_be_heard_and_the_silence_between_them_cannot() {
        let (session, _) = with_a_progression();

        // The axis progression in C major: I is C, and it sounds as one.
        let opening = session.harmony_voicing(Ticks::ZERO);
        assert!(!opening.is_empty(), "the first chord is silent");
        let chord = session.project().harmony.chord_at(Ticks::ZERO).unwrap();
        for pitch in &opening {
            assert!(
                chord.contains_midi(i32::from(*pitch)),
                "{pitch} is not in {chord}"
            );
        }

        // Nothing written is nothing sounded, rather than an error or a guess.
        let empty = self::tests::session();
        assert!(empty.harmony_voicing(Ticks::ZERO).is_empty());
    }

    #[test]
    fn an_audition_puts_the_bass_below_the_chord_and_the_chord_around_middle_c() {
        // A slash chord is the case that decides the layout: if the bass were voiced with the
        // rest, `C/E` and `C` would sound identical and the slash would be a silent decoration.
        let plain = Session::voice_for_audition(Chord::parse("C").unwrap());
        let slash = Session::voice_for_audition(Chord::parse("C/E").unwrap());
        assert_ne!(plain, slash);
        assert!(slash[0] < slash[1], "the bass is the lowest note");
        assert_eq!(
            i32::from(slash[0]) % 12,
            4,
            "the bass is the one that was named"
        );

        for chord in ["C", "F#m", "Bbmaj7", "G7", "D9"] {
            let pitches = Session::voice_for_audition(Chord::parse(chord).unwrap());
            let body = &pitches[1..];
            assert!(pitches[0] < 48, "{chord}: the bass is not a bass");
            assert!(
                body.iter().all(|pitch| (48..=96).contains(pitch)),
                "{chord} left the register a chord is recognised in: {pitches:?}"
            );
            assert!(
                pitches.windows(2).all(|pair| pair[0] < pair[1]),
                "{chord} sounded a pitch twice: {pitches:?}"
            );
        }
    }

    #[test]
    fn an_audition_borrows_an_instrument_when_the_selection_cannot_play_one() {
        // The case this exists for: chords are written before parts are, so the selected track at
        // that moment may be an audio track — or there may be no selection at all.
        let mut session = self::tests::session();
        let audio = session.add_audio_track("Reference");
        assert_eq!(
            session.audition_track(Some(audio)),
            None,
            "nothing can play"
        );

        let instrument = session.add_default_instrument_track("Piano").unwrap();
        assert_eq!(session.audition_track(None), Some(instrument));
        assert_eq!(session.audition_track(Some(audio)), Some(instrument));
        assert_eq!(
            session.audition_track(Some(instrument)),
            Some(instrument),
            "a track that can play keeps the audition"
        );
    }

    // ------------------------------------------------- clips that write themselves

    use auris_core::ClipPreset;

    /// A session with four bars of the axis progression and a track to put a part on.
    fn with_a_progression() -> (Session, TrackId) {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Bass").unwrap();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        session.forget_history();
        (session, track)
    }

    #[test]
    fn a_generated_clip_is_an_ordinary_clip_that_remembers_how_it_was_written() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Bass, 1),
            )
            .unwrap();

        let (owner, midi) = session.project().midi_clip(clip).expect("a real clip");
        assert_eq!(owner, track);
        assert!(!midi.notes.is_empty(), "a clip with no notes in it");
        assert_eq!(midi.start, Ticks::ZERO);
        assert_eq!(midi.length, BAR * 4);
        assert!(midi.is_generated());
        assert_eq!(
            session.clip_recipe(clip).map(|recipe| recipe.preset),
            Some(ClipPreset::Bass)
        );
    }

    #[test]
    fn regenerating_writes_the_same_notes_until_the_chords_move() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 5),
            )
            .unwrap();
        let first = session.project().midi_clip(clip).unwrap().1.notes.clone();

        // Nothing changed, so nothing should: this is what makes the button safe to press.
        session.regenerate_clip(clip).unwrap();
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, first);

        // Now move the harmony underneath it. The part should follow.
        session
            .stamp_named_progression("marusa", Ticks::ZERO, 4)
            .unwrap();
        session.regenerate_clip(clip).unwrap();
        assert_ne!(
            session.project().midi_clip(clip).unwrap().1.notes,
            first,
            "the chords changed and the part did not"
        );
    }

    #[test]
    fn another_take_changes_the_notes_for_every_preset_from_the_seed_the_app_starts_at() {
        // The desktop application gives the first clip in a project seed 1, so the first press of
        // "another take" is always 1 to 2. If that one pair happened to write the same notes the
        // button would look broken however well every other seed behaved.
        for preset in ClipPreset::ALL {
            let (mut session, track) = with_a_progression();
            let clip = session
                .generate_clip(track, Ticks::ZERO, BAR * 4, ClipRecipe::new(preset, 1))
                .unwrap();
            let first = session.project().midi_clip(clip).unwrap().1.notes.clone();
            assert!(!first.is_empty(), "{} wrote nothing", preset.name());

            session.reroll_clip(clip).unwrap();
            let second = session.project().midi_clip(clip).unwrap().1.notes.clone();
            assert_ne!(
                first,
                second,
                "{} wrote the same notes for seed 1 and seed 2",
                preset.name()
            );
        }
    }

    #[test]
    fn another_take_is_a_different_phrase_of_the_same_part() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 5),
            )
            .unwrap();
        let first = session.project().midi_clip(clip).unwrap().1.notes.clone();

        session.reroll_clip(clip).unwrap();
        let second = session.project().midi_clip(clip).unwrap().1.notes.clone();
        assert_ne!(first, second);
        assert!(!second.is_empty());
        assert_eq!(
            session.clip_recipe(clip).unwrap().seed,
            6,
            "the next seed, not a random one, so a take can be got back to"
        );

        // And one undo step takes the take back, not one note.
        assert_eq!(session.undo(), Some(Edit::GenerateClip));
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, first);
    }

    #[test]
    fn freezing_keeps_the_notes_and_forgets_how_they_got_there() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Chords, 2),
            )
            .unwrap();
        let kept = session.project().midi_clip(clip).unwrap().1.notes.clone();

        session.freeze_clip(clip).unwrap();
        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes,
            kept,
            "freezing must not touch a note"
        );
        assert!(!session.project().midi_clip(clip).unwrap().1.is_generated());

        // And now nothing can rewrite it, which is the whole point of having frozen it.
        let error = session.regenerate_clip(clip).unwrap_err();
        assert!(matches!(error, SessionError::NotGenerated(id) if id == clip.0));
    }

    #[test]
    fn a_clip_somebody_played_is_never_rewritten_by_accident() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .add_midi_clip(track, "Played", Ticks::ZERO, BAR)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, BAR))
            .unwrap();
        session.forget_history();

        for outcome in [
            session.regenerate_clip(clip),
            session.reroll_clip(clip),
            session.freeze_clip(clip).map(|()| 0),
        ] {
            assert!(matches!(
                outcome,
                Err(SessionError::NotGenerated(id)) if id == clip.0
            ));
        }
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes.len(), 1);
        assert!(!session.can_undo(), "a refusal must not cost an undo step");
    }

    #[test]
    fn a_generated_clip_survives_a_save_and_writes_itself_again_after() {
        let scratch = Scratch::new("clip-recipe");
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Bass, 3),
            )
            .unwrap();
        let written = session.project().midi_clip(clip).unwrap().1.notes.clone();

        let document = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        let mut reopened = self::tests::session();
        reopened.open(&document).unwrap();

        let (_, midi) = reopened
            .project()
            .midi_clip(clip)
            .expect("the clip came back");
        assert_eq!(midi.notes, written, "the notes are stored, not recomputed");
        assert_eq!(reopened.clip_recipe(clip).unwrap().seed, 3);
        assert_eq!(reopened.regenerate_clip(clip).unwrap(), written.len());
    }

    #[test]
    fn a_range_with_no_chords_under_it_makes_an_empty_clip_rather_than_an_error() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 1),
            )
            .expect("nothing to play is not a failure");
        assert!(
            session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
        assert!(
            session.project().midi_clip(clip).unwrap().1.is_generated(),
            "so that writing a progression and pressing regenerate fills it in"
        );
    }

    #[test]
    fn generating_needs_a_track_that_can_hold_notes() {
        let (mut session, _) = with_a_progression();
        let audio = session.add_audio_track("Vocals");
        let error = session
            .generate_clip(
                audio,
                Ticks::ZERO,
                BAR,
                ClipRecipe::new(ClipPreset::Lead, 1),
            )
            .unwrap_err();
        assert!(matches!(error, SessionError::WrongTrackKind { .. }));
    }

    #[test]
    fn composing_replaces_the_document_in_one_undo_step() {
        let mut session = session();
        session.add_default_instrument_track("Old").unwrap();
        session.forget_history();

        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Composed"
                form = "verse"
                chords = "@axis"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();

        assert!(report.tracks > 0);
        assert!(report.notes > 0);
        assert_eq!(session.project().name, "Composed");
        // Every part reported, plus the buses the mix routes through — which the report does not
        // count, because they are plumbing rather than things that play.
        let buses = session
            .project()
            .tracks
            .iter()
            .filter(|track| track.kind.is_bus())
            .count();
        assert_eq!(session.project().tracks.len(), report.tracks + buses);

        // One step takes the whole piece back, not one note.
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.project().tracks[0].name, "Old");
    }

    #[test]
    fn a_piece_asking_for_sounds_this_build_has_none_of_still_plays() {
        // `session()` is headless, which means no shipped library — deliberately, so that this
        // test says the same thing on a machine with the SoundFont installed and one without.
        // What it pins is the fallback: a part naming a violin comes out on the oscillator it
        // *also* names, and the report says why rather than leaving a piece that sounds wrong for
        // no visible reason.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = ["verse"]

                [[part]]
                name    = "lead"
                role    = "melody"
                program = "Violin"
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();

        assert!(
            report.substituted.iter().any(|name| name == "General MIDI"),
            "the report should say the font was missing: {:?}",
            report.substituted
        );
        assert!(
            session.project().soundfonts.is_empty(),
            "and the document should not name a font that is not there"
        );
        let lead = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "lead")
            .expect("the part became a track");
        assert_eq!(
            lead.kind.as_instrument().map(|inner| &inner.instrument_id),
            Some(&auris_compose::Role::Melody.default_instrument().to_string()),
            "the part keeps the plugin it named"
        );
        assert_eq!(session.track_preset(lead.id), None);
    }

    #[test]
    fn a_composed_document_remembers_what_it_was_asked_for() {
        // Without this, a piece composed, saved and reopened comes back to a dialog full of
        // defaults, and Another Take on it writes a different song rather than another take of
        // that one. The text is the format's own, so it reads back as the specification it was.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Remembered"
                key = "C minor"
                form = "verse chorus"
                chords = "@marusa"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let remembered = session
            .project()
            .song_spec
            .clone()
            .expect("a composed document carries its specification");
        assert_eq!(
            auris_compose::SongSpec::parse(&remembered),
            Ok(spec),
            "\n{remembered}"
        );

        // A document nobody composed carries nothing, rather than a specification that would
        // describe a song it is not.
        assert!(Project::new("By Hand", 48_000.0).song_spec.is_none());
    }

    #[test]
    fn a_composed_document_carries_its_harmony_and_its_structure() {
        // Both were computed by the composer and then dropped on the floor here: a composed song
        // opened with an empty harmony lane and an empty structure lane over a piece that plainly
        // had chords and sections. What made it worse than cosmetic is that `generate_clip` reads
        // both — so a part added to a composed song by hand had nothing to agree with.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
            title  = "Whole"
            key    = "C minor"
            form   = "intro verse chorus"
            chords = "@marusa"

            [section.intro]
            bars = 4

            [section.verse]
            bars = 8

            [section.chorus]
            bars = 8
            "#,
        )
        .unwrap();
        let piece = auris_compose::compose(&spec);
        session.compose(&piece).unwrap();

        let project = session.project();
        assert!(!project.harmony.chords.is_empty(), "no chords were carried");
        assert_eq!(project.harmony.keys.initial(), piece.harmony.keys.initial());
        assert_eq!(
            project.harmony.chords.points().len(),
            piece.harmony.chords.points().len()
        );

        // The labels the clips were named after are the labels on the timeline.
        assert_eq!(
            project
                .sections
                .section_at(Ticks::ZERO)
                .map(|(name, _)| name),
            Some("intro")
        );
        let bar = project.signatures.signature_at(Ticks::ZERO).ticks_per_bar();
        assert_eq!(
            project.sections.section_at(bar * 4).map(|(name, _)| name),
            Some("verse")
        );

        // And a clip generated afterwards finds them: the same question the piano roll asks.
        let track = project.tracks[0].id;
        let clip = session
            .generate_clip(
                track,
                bar * 4,
                bar * 4,
                ClipRecipe::new(ClipPreset::Lead, 7),
            )
            .expect("a clip over the composed song");
        let notes = session.project().midi_clip(clip).unwrap().1.notes.len();
        assert!(
            notes > 0,
            "a clip written over a composed song came out empty"
        );
    }

    #[test]
    fn a_composed_document_arrives_already_routed() {
        // The composer names a bus by its position, because it has no ids to name one by. This is
        // where the two meet, and getting it wrong would point every send at the wrong strip.
        let mut session = session();
        let spec = auris_compose::SongSpec::default();
        let piece = auris_compose::compose(&spec);
        session.compose(&piece).unwrap();

        let project = session.project();
        let bus = |name: &str| {
            project
                .tracks
                .iter()
                .find(|track| track.kind.is_bus() && track.name == name)
                .unwrap_or_else(|| panic!("no {name} bus"))
        };
        let drums = bus("Drums").id;
        let room = bus("Room").id;

        // The kit under one fader, and the room fed rather than routed through.
        let track = |name: &str| project.tracks.iter().find(|t| t.name == name).unwrap();
        assert_eq!(track("kick").output, Output::Bus(drums));
        assert_eq!(track("lead").output, Output::Master);
        assert_eq!(track("lead").sends[0].target, room);
        assert!(track("bass").sends.is_empty());

        // The reverb is on the bus and is all reflection, which is the one setting a send/return
        // reverb cannot be left at its default for.
        let reverb = &bus("Room").mixer.effects[0];
        assert_eq!(reverb.effect_id, "auris.fx.reverb");
        assert_eq!(reverb.state.params.get("mix"), Some(&1.0));

        // The buses sit below the music, and nothing about the routing loops.
        assert!(project.tracks[project.tracks.len() - 1].kind.is_bus());
        for track in &project.tracks {
            for target in track.feeds() {
                assert!(!project.routing_would_cycle(track.id, target));
            }
        }
    }

    #[test]
    fn a_composed_piece_renders_to_audible_audio() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = "verse"
                chords = "@marusa"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let rendered = session
            .render_job()
            .render(&auris_engine::OfflineOptions::whole_project(), &mut |_| {})
            .unwrap();
        assert!(
            rendered.peak() > 0.01,
            "a composed piece rendered silence, peak {}",
            rendered.peak()
        );
    }

    #[test]
    fn an_unknown_instrument_costs_a_timbre_rather_than_the_piece() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = "verse"
                [section.verse]
                bars = 2
                [[part]]
                name = "lead"
                instrument = "nope.not.here"
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();
        assert_eq!(report.substituted, ["nope.not.here"]);
        assert_eq!(report.tracks, 1, "the track was still created");
        assert!(report.notes > 0);
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

    #[test]
    fn a_track_keeps_the_colour_it_is_given_and_it_is_one_undo_step() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let was = session.project().track(track).unwrap().color;
        let wanted = Color::PALETTE
            .iter()
            .copied()
            .find(|color| *color != was)
            .expect("the palette has more than one entry");
        session.forget_history();

        session.set_track_color(track, wanted).unwrap();
        assert_eq!(session.project().track(track).unwrap().color, wanted);
        // The same colour again is not an edit; a palette full of them would otherwise fill the
        // undo stack with steps that undo nothing visible.
        session.set_track_color(track, wanted).unwrap();
        assert_eq!(undo_depth(&mut session), 1);

        session.set_track_color(track, was).unwrap();
        assert_eq!(session.undo(), Some(Edit::SetTrackColor));
        assert_eq!(session.project().track(track).unwrap().color, wanted);
    }

    #[test]
    fn freezing_a_track_stops_every_generated_clip_on_it_and_says_how_many() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        for bar in 0..3 {
            session
                .generate_clip(
                    track,
                    Ticks::from_beats(bar as f64 * 4.0),
                    Ticks::from_beats(4.0),
                    ClipRecipe::new(ClipPreset::Lead, bar),
                )
                .unwrap();
        }
        // One clip written by hand, which has no recipe to drop.
        session
            .add_midi_clip(
                track,
                "By hand",
                Ticks::from_beats(12.0),
                Ticks::from_beats(4.0),
            )
            .unwrap();

        assert_eq!(session.freeze_track(track).unwrap(), 3);
        let generated = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips
            .iter()
            .filter(|clip| clip.is_generated())
            .count();
        assert_eq!(generated, 0);
        // Nothing left to freeze, so nothing happens and nothing is recorded.
        session.forget_history();
        assert_eq!(session.freeze_track(track).unwrap(), 0);
        assert!(!session.can_undo());
    }

    // ------------------------------------------------------------------ automation

    #[test]
    fn a_lane_takes_over_a_parameter_and_giving_it_up_hands_it_back() {
        // The whole contract in one test: no lane means no answer and the stored value stands,
        // a lane answers, and removing the last point is removing the lane.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        assert_eq!(session.automated_value(fader, Ticks::ZERO), None);
        assert!(!session.is_automated(fader));

        assert!(session.set_automation_point(fader, Ticks::ZERO, -6.0));
        assert!(session.is_automated(fader));
        assert_eq!(
            session.automated_value(fader, Ticks::from_beats(9.0)),
            Some(-6.0)
        );

        assert!(session.remove_automation_point(fader, Ticks::ZERO));
        assert!(!session.is_automated(fader));
        assert_eq!(session.automated_value(fader, Ticks::ZERO), None);
    }

    #[test]
    fn a_lane_reads_between_its_points() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -12.0);
        session.set_automation_point(fader, Ticks::from_beats(8.0), 0.0);
        assert_eq!(
            session.automated_value(fader, Ticks::from_beats(4.0)),
            Some(-6.0)
        );
    }

    #[test]
    fn a_written_value_is_clamped_by_the_parameter_it_drives() {
        // A lane is written in the parameter's own units, so a point outside its range is a point
        // the plugin would refuse anyway — better stored as what will actually be heard.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, 500.0);
        let written = session.automated_value(fader, Ticks::ZERO).expect("a lane");
        assert!(
            written <= 12.0,
            "the fader tops out at +12 dB, wrote {written}"
        );
    }

    #[test]
    fn a_discrete_parameter_gets_a_lane_that_holds() {
        // Interpolating a chooser would sweep through every option between two settings and sound
        // all of them. Which curve a lane gets is decided where the descriptor is legible.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let waveform = session
            .param_descriptors(
                &session.project().tracks[0]
                    .kind
                    .as_instrument()
                    .unwrap()
                    .instrument_id
                    .clone(),
            )
            .iter()
            .position(|descriptor| descriptor.steps.is_some())
            .map(|index| ParamTarget::Instrument {
                track,
                param: ParamId(index as u32),
            });
        let Some(chooser) = waveform else {
            panic!("the default instrument has no discrete parameter to test with");
        };
        session.set_automation_point(chooser, Ticks::ZERO, 0.0);
        session.set_automation_point(chooser, Ticks::from_beats(8.0), 2.0);
        assert_eq!(
            session.automation().lane(chooser).map(|lane| lane.curve),
            Some(AutomationCurve::Hold)
        );
        assert_eq!(
            session.automated_value(chooser, Ticks::from_beats(4.0)),
            Some(0.0),
            "a chooser holds rather than passing through what is between"
        );
    }

    #[test]
    fn a_lane_cannot_be_written_into_thin_air() {
        // A fader's descriptor is synthesised rather than looked up, so it answers for a track id
        // nobody ever created; without an existence check a lane would be written and then
        // silently dropped by the graph builder.
        let mut session = session();
        assert!(!session.set_automation_point(
            ParamTarget::TrackGain(TrackId(9_999)),
            Ticks::ZERO,
            -6.0
        ));
        assert!(session.automation().is_empty());
    }

    #[test]
    fn every_automation_command_is_one_undo_step_and_only_when_it_changed_something() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.forget_history();

        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        assert_eq!(undo_depth(&mut session), 1);
        // The same point again is not an edit.
        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        assert_eq!(undo_depth(&mut session), 1);
        // Nor is removing a point that was never there.
        assert!(!session.remove_automation_point(fader, Ticks::from_beats(4.0)));
        assert_eq!(undo_depth(&mut session), 1);

        session.set_automation_point(fader, Ticks::from_beats(4.0), 0.0);
        assert_eq!(session.undo(), Some(Edit::WriteAutomation(fader)));
        assert_eq!(
            session.automation().lane(fader).map(|l| l.points().len()),
            Some(1)
        );
    }

    #[test]
    fn a_drag_across_a_lane_is_one_undo_step() {
        // The mechanism every other drag uses: the transaction is opened by the gesture, so the
        // fifty points a pointer writes on the way collapse into the one it landed on.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        session.forget_history();

        session.begin_transaction(Edit::WriteAutomation(fader));
        let mut at = Ticks::ZERO;
        for step in 1..=20 {
            at = session
                .move_automation_point(fader, at, Ticks(step * 48), -6.0 + step as f32 * 0.1)
                .expect("the point is there to move");
        }
        session.end_transaction();
        assert_eq!(undo_depth(&mut session), 1);
    }

    #[test]
    fn deleting_a_track_takes_its_lanes_with_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_automation_point(ParamTarget::TrackGain(track), Ticks::ZERO, -6.0);
        session.set_automation_point(ParamTarget::MasterGain, Ticks::ZERO, -3.0);
        session.remove_track(track).unwrap();
        assert_eq!(session.automation().len(), 1);
        assert!(session.automation().lane(ParamTarget::MasterGain).is_some());
    }

    #[test]
    fn a_lane_survives_a_save_and_an_open() {
        let scratch = Scratch::new("automation-round-trip");
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -12.0);
        session.set_automation_point(fader, Ticks::from_beats(8.0), 0.0);
        let report = session.save_as(&scratch.join("Automated.auris")).unwrap();

        let mut reopened = self::tests::session();
        reopened.open(&report.document).unwrap();
        let fader = ParamTarget::TrackGain(reopened.project().tracks[0].id);
        assert_eq!(
            reopened.automated_value(fader, Ticks::from_beats(4.0)),
            Some(-6.0),
            "the curve has to survive the round trip, not just the points"
        );
    }
}
