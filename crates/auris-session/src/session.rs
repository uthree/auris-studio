//! The editing session: one document, one engine, one command per user action.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{PluginKind, PluginState};
use auris_core::time::{Seconds, Ticks};
use auris_core::{
    AudioBuffer, AudioSourceBank, ClipId, EffectSlotId, MidiClip, Note, PluginRegistry, Project,
    SourceId, TrackId,
};
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineHandle, MeterBank, OutputDeviceInfo,
    RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks, compute_peaks};
use auris_io::{import_audio_file, load_project, save_project};

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
        let registry = default_registry();
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
        let graph = RenderGraph::build_at(
            &self.project,
            &self.render_bank,
            &self.registry,
            self.engine.max_block(),
            rate,
        );
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

    /// Appends an instrument track playing the first registered instrument.
    pub fn add_default_instrument_track(
        &mut self,
        name: impl Into<String>,
    ) -> Result<TrackId, SessionError> {
        let instrument = self
            .registry
            .first_instrument_id()
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
            .first_instrument_id()
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
        if let Some(instrument) = self.registry.first_instrument_id() {
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

    /// Opens a project file.
    ///
    /// Returns the source paths that could not be re-decoded. The project still opens; those
    /// clips are silent until the files come back, which is far friendlier than refusing to
    /// open a session because one sample moved.
    pub fn open(&mut self, path: &Path) -> Result<Vec<PathBuf>, SessionError> {
        let project = load_project(path)?;
        self.history.clear();
        self.clear_sources();
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.replace_project(project);

        let sources: Vec<(SourceId, PathBuf)> = self
            .project
            .audio_sources
            .values()
            .map(|source| (source.id, source.path.clone()))
            .collect();
        let rate = self.project.sample_rate;
        let mut missing = Vec::new();
        for (id, source_path) in sources {
            match import_audio_file(&source_path, rate) {
                Ok(buffer) => self.install_source(id, Arc::new(buffer)),
                Err(error) => {
                    log::warn!("could not reload {}: {error}", source_path.display());
                    missing.push(source_path);
                }
            }
        }
        self.rebuild_graph();
        Ok(missing)
    }

    /// Saves the project.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        save_project(path, &self.project)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    /// Saves to the path the project was last saved to or opened from.
    pub fn save_in_place(&mut self) -> Result<(), SessionError> {
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        self.save(&path)
    }

    /// Imports an audio file, adds a track for it and places a clip at `start`.
    pub fn import_audio(&mut self, path: &Path, start: Ticks) -> Result<ClipId, SessionError> {
        let buffer = import_audio_file(path, self.project.sample_rate)?;
        self.record(Edit::ImportAudio);
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio".to_string());
        let source = self.project.add_audio_source(
            name.clone(),
            path.to_path_buf(),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        let track = self.project.add_audio_track(name);
        let clip = self
            .project
            .add_audio_clip(track, source, start.max_zero())
            .ok_or(SessionError::UnknownTrack(track.0))?;
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();
        Ok(clip)
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

    /// Drops every decoded source, from both banks and from the waveform cache.
    fn clear_sources(&mut self) {
        self.bank = AudioSourceBank::new();
        self.render_bank = AudioSourceBank::new();
        self.waveforms.clear();
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
