//! The editing session: one document, one engine, one command per user action.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::harmony::Harmony;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{PluginKind, PluginState};
use auris_core::theory::chart::{Chart, catalog};
use auris_core::theory::key::Key as MusicalKey;
use auris_core::theory::numeral::Numeral;
use auris_core::time::{Seconds, Ticks};
use auris_core::{
    AssetPath, AudioBuffer, AudioSourceBank, ClipId, ClipRecipe, EffectSlotId, MidiClip, Note,
    PluginRegistry, PresetRef, Project, SoundFontId, SoundFontRef, SourceId, TrackId,
};
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineHandle, MeterBank, OutputDeviceInfo,
    RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks, compute_peaks};
use auris_io::{
    AUDIO_DIR, IoError, SoundFontPreset, byte_size, copy_into, document_in_folder, find_named,
    font_name, import_audio_file, load_project, load_soundfont, preset_count, presets,
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
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            audio: true,
            gpu: true,
            audio_preferences: AudioPreferences::default(),
            sample_rate: 48_000.0,
        }
    }
}

impl SessionOptions {
    /// No audio device and no GPU — for tests, batch rendering and headless tools.
    pub fn headless() -> Self {
        Self {
            audio: false,
            gpu: false,
            ..Self::default()
        }
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

/// What composing produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeReport {
    /// How many tracks were created.
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
            registry,
            engine,
            device: Some(device),
            gpu,
            audio,
            headless: !options.audio,
            history: History::default(),
            transaction: None,
            needs_rebuild: false,
            path: None,
            dirty: false,
            scope: Arc::new(auris_engine::Scope::new()),
            analyzer: auris_dsp::SpectrumAnalyzer::new(auris_engine::SCOPE_WINDOW),
            param_cache: HashMap::new(),
            waveforms: HashMap::new(),
        };
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
        self.dirty = false;
    }

    /// Records an undo step for the edit about to be made.
    ///
    /// Inside a transaction this does nothing: the snapshot was taken when the transaction
    /// opened, and one gesture is one step.
    fn record(&mut self, edit: Edit) {
        if self.transaction.is_none() {
            self.history.push(edit, &self.project);
        }
        self.dirty = true;
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
        self.project = project;
        self.transaction = None;
        self.needs_rebuild = false;
        self.rebuild_graph();
        // The loop lives in the audio thread's transport and only `SetLoop` moves it, so a
        // document swap that does not republish leaves playback wrapping the old range.
        self.publish_loop();
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
    pub fn set_loop_enabled(&mut self, enabled: bool) {
        self.record(Edit::ToggleLoop);
        self.project.loop_enabled = enabled;
        if enabled && self.project.loop_region.is_none() {
            let bars = self.project.time_signature.ticks_per_bar() * 2;
            self.project.loop_region = Some((Ticks::ZERO, bars));
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

    /// Sets the project tempo.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.record(Edit::ChangeTempo);
        self.project.set_bpm(bpm);
        // Notes are scheduled in frames, so the graph has to be re-flattened, and the loop's
        // frame positions move with it.
        self.invalidate_graph();
        self.publish_loop();
    }

    /// Sets the editing grid.
    pub fn set_grid(&mut self, grid: Ticks) {
        self.project.grid = Ticks(grid.raw().max(1));
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

    /// Sets the key from `at` onwards.
    ///
    /// `at` snaps to the editing grid, so a key change lands where a person aimed rather than
    /// where the pointer happened to be. Tick zero is the song's own key and is always there, so
    /// setting it there changes what the whole song is read in rather than adding a change to it.
    pub fn set_key(&mut self, at: Ticks, key: MusicalKey) {
        self.record(Edit::SetKey);
        let at = self.snap(at);
        self.project.harmony.keys.set_point(at, key);
    }

    /// Removes the key change at `at`, letting the key before it run through.
    ///
    /// The key at tick zero is not a change and cannot be removed: a song is always in some key.
    pub fn remove_key(&mut self, at: Ticks) {
        let at = self.snap(at);
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::SetKey);
        self.project.harmony.keys.remove_point(at);
    }

    /// Sets the chord sounding from `at` onwards, until the next change.
    pub fn set_chord(&mut self, at: Ticks, chord: Numeral) {
        self.record(Edit::SetChord);
        let at = self.snap(at);
        self.project.harmony.chords.set_point(at, Some(chord));
    }

    /// Removes the chord change at `at`, letting the chord before it run through.
    pub fn remove_chord(&mut self, at: Ticks) {
        self.record(Edit::SetChord);
        let at = self.snap(at);
        self.project.harmony.chords.remove_point(at);
    }

    /// Empties the chords in `from..to`, leaving the key timeline alone.
    ///
    /// What sounded at `to` still sounds there: clearing the middle of a song does not silence
    /// the end of it.
    pub fn clear_harmony(&mut self, from: Ticks, to: Ticks) {
        self.record(Edit::ClearHarmony);
        let (from, to) = (self.snap(from), self.snap(to));
        self.project.harmony.clear(from, to);
    }

    /// Writes `chart` across `bars` bars from `from`, returning how many chords it wrote.
    ///
    /// The chart repeats or is truncated to fit. `from` snaps to the editing grid, but the chords
    /// *inside* it do not: a chart divides each bar musically, and three chords in a bar of 4/4
    /// are three lots of 1280 ticks, which is not a grid position and must not be rounded to one.
    /// A stamp is a division of a bar; a drag is an edit on the grid.
    pub fn stamp_progression(&mut self, chart: &Chart, from: Ticks, bars: usize) -> usize {
        self.record(Edit::StampProgression);
        let signature = self.project.time_signature;
        let from = self.snap(from);
        self.project.harmony.stamp(chart, from, bars, signature)
    }

    /// Writes the catalogue progression called `name`, such as `axis` or `丸サ`.
    ///
    /// `bars` of zero means the chart's own length, which is what "put this progression here"
    /// usually means. A name nothing answers to is an error rather than a quiet no-op — there is
    /// no nearest right answer to a misspelling, and stamping nothing while reporting success is
    /// the one outcome nobody could debug.
    pub fn stamp_named_progression(
        &mut self,
        name: &str,
        from: Ticks,
        bars: usize,
    ) -> Result<usize, SessionError> {
        let chart =
            catalog(name).ok_or_else(|| SessionError::UnknownProgression(name.to_string()))?;
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

    /// Removes a track.
    pub fn remove_track(&mut self, id: TrackId) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::DeleteTrack);
        self.project.remove_track(id);
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

    /// Replaces a track's instrument, discarding the previous plugin's parameter values.
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
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            inner.instrument_id = instrument_id.to_string();
            // The saved values belong to the old plugin; applying them to a different one would
            // write another plugin's numbers into unrelated controls.
            inner.instrument_state = PluginState::empty();
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
    /// A track already on the sampler keeps its level, reverb and chorus: those are how the
    /// player is set up, not which sound it is playing, and losing them every time somebody
    /// auditioned a neighbouring preset would be its own small tragedy.
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
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            if inner.instrument_id != SAMPLER_ID {
                inner.instrument_id = SAMPLER_ID.to_string();
                inner.instrument_state = PluginState::empty();
            }
            store_preset(&mut inner.instrument_state, preset);
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
            track.height = height.clamp(24.0, 400.0);
        }
        Ok(())
    }

    fn require_track(&self, id: TrackId) -> Result<usize, SessionError> {
        self.project
            .track_index(id)
            .ok_or(SessionError::UnknownTrack(id.0))
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
        project.time_signature = composition.meter;

        let mut report = ComposeReport {
            tracks: 0,
            clips: 0,
            notes: 0,
            length: composition.length,
            substituted: Vec::new(),
        };

        for track in &composition.tracks {
            let instrument = if self.registry.has_instrument(&track.instrument) {
                track.instrument.clone()
            } else {
                report.substituted.push(track.instrument.clone());
                fallback.clone()
            };
            let track_id = project.add_instrument_track(&track.name, instrument);
            if let Some(entry) = project.track_mut(track_id) {
                entry.mixer.gain_db = track.gain_db;
                entry.mixer.pan = track.pan;
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

        // Loop over the whole piece, so pressing play and leaving it running plays the song.
        project.loop_region = Some((Ticks::ZERO, composition.length));
        project.loop_enabled = false;

        self.record(Edit::Compose);
        self.replace_project(project);
        self.dirty = true;
        Ok(report)
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
                actual: "an audio track",
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
    fn phrase(&self, start: Ticks, length: Ticks, recipe: &ClipRecipe) -> Vec<Note> {
        auris_compose::write_phrase(
            &self.project.harmony,
            start,
            length,
            self.project.time_signature,
            recipe,
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
        let Some(earliest) = origins.iter().map(|(_, start)| *start).min() else {
            return;
        };
        let delta = delta.max(-earliest);
        self.record(Edit::MoveClip);
        for (clip, start) in origins {
            let start = (*start + delta).max_zero();
            if let Some(midi) = self.project.midi_clip_mut(*clip) {
                midi.start = start;
            } else if let Some(audio) = self.project.audio_clip_mut(*clip) {
                audio.start = start;
            }
        }
        self.invalidate_graph();
    }

    /// Moves a clip of either kind to a new start position.
    pub fn move_clip(&mut self, clip: ClipId, start: Ticks) -> Result<(), SessionError> {
        self.record(Edit::MoveClip);
        let start = start.max_zero();
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.start = start;
        } else if let Some(audio) = self.project.audio_clip_mut(clip) {
            audio.start = start;
        } else {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags a clip's end to `end`.
    pub fn resize_clip(&mut self, clip: ClipId, end: Ticks) -> Result<(), SessionError> {
        self.record(Edit::ResizeClip);
        let grid = self.project.grid;
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.length = (end - midi.start).max(grid);
            self.invalidate_graph();
            return Ok(());
        }
        // An audio clip's length lives in source frames, so the dragged tick has to go back
        // through the tempo map rather than being stored as ticks.
        let sample_rate = self.project.sample_rate;
        let tempo = self.project.tempo_map.clone();
        let Some(audio) = self.project.audio_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let start_seconds = tempo.ticks_to_seconds(audio.start).0;
        let end_seconds = tempo.ticks_to_seconds(end).0;
        audio.length_frames = (((end_seconds - start_seconds).max(0.0)) * sample_rate) as u64;
        audio.length_frames = audio.length_frames.max(1);
        self.invalidate_graph();
        Ok(())
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
        self.record(Edit::AddNote);
        let grid = self.project.grid;
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        target.notes.push(Note {
            pitch: note.pitch.min(127),
            velocity: note.velocity.clamp(0.0, 1.0),
            start: note.start.max_zero(),
            length: Ticks(note.length.raw().max(1)),
        });
        target.fit_length_to_notes(grid);
        let index = target.notes.len() - 1;
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
        self.record(Edit::MoveNotes);
        let grid = self.project.grid;
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        for (index, start, pitch) in origins {
            if let Some(note) = target.notes.get_mut(*index) {
                note.start = (*start + delta_ticks).max_zero();
                note.pitch = (*pitch as i32 + delta_pitch).clamp(0, 127) as u8;
            }
        }
        target.fit_length_to_notes(grid);
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
        self.record(Edit::ResizeNote);
        let grid = Ticks(self.project.grid.raw().max(1));
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        if let Some(note) = target.notes.get_mut(index) {
            note.length = (end - note.start).max(grid);
        }
        target.fit_length_to_notes(grid);
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
    pub fn set_param(&mut self, target: ParamTarget, value: f32) {
        self.dirty = true;
        match target {
            ParamTarget::TrackGain(id) => {
                let Ok(index) = self.require_track(id) else {
                    return;
                };
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
                self.project.tracks[index].mixer.pan = value;
                self.send(EngineCommand::SetTrackPan { index, pan: value });
            }
            ParamTarget::MasterGain => {
                self.project.master.gain_db = value;
                self.send(EngineCommand::SetMasterGain(value));
            }
            ParamTarget::MasterPan => {
                self.project.master.pan = value;
                self.send(EngineCommand::SetMasterPan(value));
            }
            ParamTarget::Instrument { track, param } => {
                let Ok(index) = self.require_track(track) else {
                    return;
                };
                let Some(key) = self.param_key(target, param) else {
                    return;
                };
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
                let Some(strip) = self.strip_mut(track) else {
                    return;
                };
                let Some(slot_index) = strip.effects.iter().position(|s| s.id == slot) else {
                    return;
                };
                strip.effects[slot_index].state.params.insert(key, value);
                self.send(EngineCommand::SetEffectParam {
                    track: track_index,
                    slot: slot_index,
                    param,
                    value,
                });
            }
        }
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
        self.replace_project(project);

        let missing = self.reload_assets();
        self.rebuild_graph();
        Ok(missing)
    }

    /// Writes the document at exactly `path`, without moving or collecting anything.
    ///
    /// The project folder becomes the directory holding `path`, so a caller choosing a fresh
    /// location wants [`Self::save_as`] instead — this one would leave the audio behind.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        save_project(path, &self.project)?;
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
    /// SoundFonts are left where they are. A font is a library shared by every project that uses
    /// it, and a copy per project would cost gigabytes to save a path; [`Self::collect_assets`]
    /// is how someone archiving a project asks for those too.
    pub fn save_as(&mut self, chosen: &Path) -> Result<PathBuf, SessionError> {
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

        // From here the document belongs to the new folder even if the write below fails: the
        // files land there, and their references are read against wherever `self.path` says the
        // document is. Leaving it pointing at the old folder is what would be inconsistent.
        self.path = Some(document.clone());
        for (id, from) in audio {
            let Some(from) = from else { continue };
            if let Err(error) = self.collect_source(id, &from) {
                log::warn!("could not collect {}: {error}", from.display());
            }
        }

        save_project(&document, &self.project)?;
        self.dirty = false;
        Ok(document)
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
    pub fn collect_assets(&mut self) -> Result<usize, SessionError> {
        let folder = self
            .project_folder()
            .map(Path::to_path_buf)
            .ok_or(SessionError::NoPath)?;

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
        for (id, from) in sources {
            let Some(from) = from else { continue };
            self.collect_source(id, &from)?;
            collected += 1;
        }
        for (id, from) in fonts {
            let Some(from) = from else { continue };
            let name = copy_into(&from, &folder.join(AUDIO_DIR))?;
            if let Some(font) = self.project.soundfonts.get_mut(&id) {
                font.path = AssetPath::inside(Path::new(AUDIO_DIR).join(name));
            }
            collected += 1;
        }

        if collected > 0 {
            self.dirty = true;
        }
        Ok(collected)
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
        self.record(Edit::ImportSoundFont);
        let id = match self.project.soundfont_at(self.project_folder(), path) {
            Some(existing) => existing,
            None => self
                .project
                .add_soundfont(name, AssetPath::external(path), byte_size(path)),
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

        let sources: Vec<(SourceId, AssetPath)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.clone()))
            .collect();
        let fonts: Vec<(SoundFontId, AssetPath, u64)> = self
            .project
            .soundfonts
            .values()
            .map(|font| (font.id, font.path.clone(), font.byte_size))
            .collect();

        let mut search = self.search_path();
        let mut missing = Vec::new();

        for (id, stored) in sources {
            let Some(found) = locate(&stored, folder.as_deref(), &search, 0) else {
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
    fn search_path(&self) -> Vec<PathBuf> {
        let Some(folder) = self.project_folder() else {
            return Vec::new();
        };
        vec![folder.join(AUDIO_DIR), folder.to_path_buf()]
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
        session.set_param(
            ParamTarget::Instrument {
                track,
                param: ParamId(0),
            },
            1.0,
        );

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
            let mut buffer = AudioBuffer::new(2, 480, 48_000.0);
            for channel in 0..2 {
                for (frame, sample) in buffer.channel_mut(channel).iter_mut().enumerate() {
                    *sample = (frame as f32 * 0.01).sin() * 0.5;
                }
            }
            let path = self.join(name);
            auris_io::write_wav(&path, &buffer, &auris_io::WavExportSettings::default())
                .expect("a WAV file writes");
            path
        }
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
            .expect("saves");

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
                .expect("saves"),
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
    fn a_chord_lands_on_the_grid_rather_than_where_the_pointer_was() {
        let mut session = self::tests::session();
        // The default grid is a sixteenth: 240 ticks.
        session.set_chord(Ticks(3840 + 130), numeral("V"));
        assert_eq!(
            session.harmony().chords.points()[0].tick,
            Ticks(3840 + 240),
            "snapped to the nearest sixteenth"
        );
        assert_eq!(
            session.harmony().numeral_at(Ticks(3840 + 240)),
            Some(numeral("V"))
        );
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

        let document = session.save_as(&scratch.join("Song.auris")).unwrap();
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

        let document = session.save_as(&scratch.join("Song.auris")).unwrap();
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
            "title: Composed\nform: verse\nchords: @axis\n[section verse]\nbars: 4",
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();

        assert!(report.tracks > 0);
        assert!(report.notes > 0);
        assert_eq!(session.project().name, "Composed");
        assert_eq!(session.project().tracks.len(), report.tracks);

        // One step takes the whole piece back, not one note.
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.project().tracks[0].name, "Old");
    }

    #[test]
    fn a_composed_piece_renders_to_audible_audio() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            "form: verse\nchords: @marusa\n[section verse]\nbars: 4",
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
            "form: verse\n[section verse]\nbars: 2\n[part lead]\ninstrument: nope.not.here",
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
}
