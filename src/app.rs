//! The application view: document state, engine wiring and the panel layout.
//!
//! Auris Studio is a single gpui view rather than a tree of entities. Every panel needs the
//! project, the selection and the engine handle, and gpui entities do not share mutable state,
//! so splitting them would mean either duplicating that state or threading handles through
//! every callback. Instead the panels are `impl AurisApp` blocks in the [`crate::ui`] modules,
//! which keeps one owner of the truth while still keeping each panel's code in its own file.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::PluginState;
use auris_core::time::{Seconds, Ticks};
use auris_core::{
    AudioSourceBank, ClipId, EffectSlotId, MidiClip, Note, PluginRegistry, Project, SourceId,
    TrackId,
};
use auris_dsp::DspPack;
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineHandle, RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks};
use auris_synth::SynthPack;
use gpui::{App, Bounds, Context, FocusHandle, Focusable, Pixels, Point, Task, Window, point, px};

use crate::history::History;
use crate::theme::Theme;
use crate::ui::timeline::{PitchView, TimelineView};

/// Which editor occupies the bottom panel.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EditorTab {
    /// The piano roll for the selected MIDI clip.
    PianoRoll,
    /// The mixer, showing every track's strip side by side.
    Mixer,
}

/// Which page the right-hand inspector shows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InspectorTab {
    /// The selected track's instrument and effect chain.
    Track,
    /// The plugin browser.
    Browser,
}

/// Something the user is currently dragging.
#[derive(Clone, Debug)]
pub enum Drag {
    /// Scrubbing the playhead along the ruler.
    Playhead,
    /// Sweeping out a loop region; `anchor` is where the drag began.
    LoopRegion {
        /// Tick the drag started at.
        anchor: Ticks,
    },
    /// Moving a clip along the timeline.
    ClipMove {
        /// Clip being moved.
        clip: ClipId,
        /// Distance from the clip's start to the point that was grabbed, so the clip does not
        /// jump under the pointer.
        grab_offset: Ticks,
    },
    /// Dragging a clip's right edge.
    ClipResize {
        /// Clip being resized.
        clip: ClipId,
    },
    /// Moving one or more notes in the piano roll.
    NoteMove {
        /// Clip the notes live in.
        clip: ClipId,
        /// Tick under the pointer when the drag began.
        origin_tick: Ticks,
        /// Pitch under the pointer when the drag began.
        origin_pitch: u8,
        /// Starting position of every selected note, so the whole selection moves together.
        origins: Vec<(usize, Ticks, u8)>,
    },
    /// Dragging a note's right edge.
    NoteResize {
        /// Clip the note lives in.
        clip: ClipId,
        /// Note being resized.
        index: usize,
    },
    /// Turning a parameter.
    Param {
        /// What is being changed.
        target: ParamTarget,
        /// Value when the drag began.
        start_value: f32,
        /// Pointer x when the drag began.
        start_x: Pixels,
    },
    /// Dragging the tempo readout.
    Tempo {
        /// Tempo when the drag began.
        start_bpm: f64,
        /// Pointer x when the drag began.
        start_x: Pixels,
    },
}

/// A parameter the UI can change, wherever it lives.
///
/// Routing every edit through one enum means the "update the project, then tell the engine"
/// step exists in exactly one place, so no control can forget half of it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamTarget {
    /// A track's fader.
    TrackGain(TrackId),
    /// A track's pan.
    TrackPan(TrackId),
    /// The master fader.
    MasterGain,
    /// The master pan.
    MasterPan,
    /// A parameter of a track's instrument.
    Instrument {
        /// Track whose instrument is being edited.
        track: TrackId,
        /// Parameter index.
        param: ParamId,
    },
    /// A parameter of an effect, on a track or on the master bus.
    Effect {
        /// Track the effect sits on; `None` means the master bus.
        track: Option<TrackId>,
        /// Which slot in the chain.
        slot: EffectSlotId,
        /// Parameter index.
        param: ParamId,
    },
}

/// Progress of a running export.
///
/// The render runs on a background thread, so progress travels through an atomic the UI polls
/// on its repaint tick rather than through a channel that would need draining.
#[derive(Clone, Debug)]
pub struct ExportState {
    /// Where the file is being written.
    pub path: PathBuf,
    /// Completion from 0.0 to 1.0, stored as `f32` bits.
    pub progress: Arc<AtomicU32>,
    /// Set once the render finishes, successfully or not.
    pub result: Option<Result<String, String>>,
}

impl ExportState {
    /// Completion from 0.0 to 1.0.
    pub fn fraction(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }
}

/// Where each canvas was actually drawn last frame.
///
/// Hit tests used to re-derive these rectangles from the window size and the `Metrics`
/// constants, which is only correct while the flex tree lays out exactly as assumed — a short
/// window, or a one-pixel border nobody accounted for, silently moved every click. Recording
/// what was painted removes the whole class: the pointer is compared against the same geometry
/// the user saw.
///
/// `Rc<Cell<_>>` because the paint closures are `'static` and run on the UI thread, so they
/// cannot borrow the view.
#[derive(Clone, Default)]
pub struct CanvasBounds {
    /// The bar ruler above the arrangement.
    pub ruler: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The arrangement's clip lanes.
    pub lanes: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The piano roll's note grid.
    pub roll: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// The whole application.
pub struct AurisApp {
    pub(crate) project: Project,
    pub(crate) bank: AudioSourceBank,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) dirty: bool,
    pub(crate) history: History,

    pub(crate) registry: Arc<PluginRegistry>,
    pub(crate) engine: EngineHandle,
    pub(crate) gpu: Option<Arc<GpuContext>>,
    device: Option<AudioDevice>,

    /// Parameter descriptors by plugin id, built once per plugin by instantiating it.
    pub(crate) param_cache: HashMap<String, Arc<Vec<ParamDescriptor>>>,
    /// Drawn waveform peaks by audio source.
    pub(crate) waveforms: HashMap<SourceId, Arc<WaveformPeaks>>,

    pub(crate) theme: Theme,
    pub(crate) timeline: TimelineView,
    pub(crate) pitch: PitchView,
    pub(crate) selected_track: Option<TrackId>,
    pub(crate) selected_clip: Option<ClipId>,
    pub(crate) selected_notes: BTreeSet<usize>,
    pub(crate) drag: Option<Drag>,
    /// Undo label for a drag that has not changed anything yet.
    ///
    /// Snapshotting at mouse-down turned every selection click into an undo step, which quietly
    /// pushed real history off the end of the stack. The label waits here until a handler
    /// actually mutates the document.
    pub(crate) pending_edit: Option<&'static str>,
    /// Whether the gesture in progress has mutated the document.
    pub(crate) drag_changed: bool,
    pub(crate) editor: EditorTab,
    pub(crate) inspector: InspectorTab,
    pub(crate) status: String,
    pub(crate) export: Option<ExportState>,
    /// Note currently sounding because the user is holding a key or dragging one.
    pub(crate) auditioning: Option<(TrackId, u8)>,
    /// Keyboard focus target, so the action bindings reach this view.
    pub(crate) focus: FocusHandle,
    /// Window size as of the last frame. Pointer events arrive in window coordinates and the
    /// bottom editor is positioned from the window's bottom edge, so hit tests need this.
    pub(crate) viewport_height: Pixels,
    /// Width of the arrangement body as of the last frame.
    pub(crate) arrangement_width: Pixels,
    /// Rectangles the canvases were painted into last frame.
    pub(crate) canvas: CanvasBounds,

    _repaint: Task<()>,
}

impl Focusable for AurisApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AurisApp {
    /// Frames the audio engine renders per block. 512 frames is ~11 ms at 48 kHz: low enough to
    /// feel responsive when auditioning notes, long enough to keep per-block overhead small.
    pub const BLOCK_FRAMES: usize = 512;

    /// Builds the application, starting audio and creating a small demo project.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut registry = PluginRegistry::new();
        registry.install::<SynthPack>();
        registry.install::<DspPack>();
        let registry = Arc::new(registry);

        let mut project = Project::new("Untitled", 48_000.0);
        seed_demo_project(&mut project, &registry);

        // `start_audio` falls back to a silent engine rather than failing, so the window opens
        // and stays editable on a machine with no output device.
        let settings = AudioSettings {
            block_frames: Some(Self::BLOCK_FRAMES as u32),
            ..AudioSettings::default()
        };
        let (device, engine) = start_audio(&settings).expect("start_audio falls back to silence");
        let status = if engine.is_running() {
            format!(
                "Audio: {} · {:.0} Hz · {} ch",
                device.name(),
                engine.sample_rate(),
                engine.channel_count()
            )
        } else {
            "No audio output — editing and export still work".to_string()
        };
        let device = Some(device);

        let gpu = GpuContext::new().map(Arc::new);
        let status = match &gpu {
            Some(context) => format!(
                "{status} · GPU: {} ({})",
                context.adapter_name(),
                context.backend()
            ),
            None => format!("{status} · GPU: unavailable, using CPU"),
        };
        log::info!("{status}");

        // Repaint on a timer rather than per audio block: the playhead and meters live in
        // atomics written by the audio thread, and 30 fps is plenty to read them at.
        let repaint = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.engine.collect_garbage();
                        // With no output device nothing consumes the command queue, so every
                        // graph the UI builds would sit in it holding plugins and samples.
                        if let Some(device) = &this.device
                            && !device.is_running()
                        {
                            device.discard_pending();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let selected_track = project.tracks.first().map(|track| track.id);
        let selected_clip = project.tracks.first().and_then(|track| {
            track
                .kind
                .as_instrument()
                .and_then(|inner| inner.clips.first())
                .map(|clip| clip.id)
        });

        let mut app = Self {
            project,
            bank: AudioSourceBank::new(),
            project_path: None,
            dirty: false,
            history: History::default(),
            registry,
            engine,
            gpu,
            device,
            param_cache: HashMap::new(),
            waveforms: HashMap::new(),
            theme: Theme::dark(),
            timeline: TimelineView::default(),
            pitch: PitchView::default(),
            selected_track,
            selected_clip,
            selected_notes: BTreeSet::new(),
            drag: None,
            pending_edit: None,
            drag_changed: false,
            editor: EditorTab::PianoRoll,
            inspector: InspectorTab::Track,
            status,
            export: None,
            auditioning: None,
            focus: cx.focus_handle(),
            viewport_height: px(900.0),
            arrangement_width: px(900.0),
            canvas: CanvasBounds::default(),
            _repaint: repaint,
        };
        app.rebuild_graph();
        app
    }

    /// Rebuilds the render graph and hands it to the audio thread.
    ///
    /// Called after any structural change — a new track, a moved clip, an added effect. Cheap
    /// parameter tweaks go through [`Self::set_param_value`] instead, which sends a small
    /// command rather than a whole graph.
    pub(crate) fn rebuild_graph(&mut self) {
        let graph = RenderGraph::build_at(
            &self.project,
            &self.bank,
            &self.registry,
            self.engine.max_block(),
            self.engine.sample_rate(),
        );
        if let Err(error) = self.engine.set_graph(graph) {
            log::warn!("could not update the render graph: {error}");
        }
    }

    /// Records an undo step, then marks the document dirty.
    pub(crate) fn edit(&mut self, label: &str) {
        self.history.push(label, &self.project);
        self.dirty = true;
        self.drag_changed = true;
    }

    /// Begins a gesture, remembering the undo label without spending it yet.
    pub(crate) fn begin_drag(&mut self, label: &'static str, drag: Drag) {
        self.drag = Some(drag);
        self.pending_edit = Some(label);
        self.drag_changed = false;
    }

    /// Begins a gesture that records no undo step of its own, such as a playhead scrub.
    pub(crate) fn begin_drag_without_edit(&mut self, drag: Drag) {
        self.drag = Some(drag);
        self.pending_edit = None;
        self.drag_changed = false;
    }

    /// Spends the pending undo label. Call this immediately before the first mutation a drag
    /// makes, so a gesture that only selected something leaves no history entry behind.
    pub(crate) fn commit_pending_edit(&mut self) {
        if let Some(label) = self.pending_edit.take() {
            self.edit(label);
        }
    }

    /// Replaces the whole document, for undo, redo and loading.
    pub(crate) fn set_project(&mut self, project: Project) {
        self.project = project;
        self.selected_track = self
            .selected_track
            .filter(|id| self.project.track(*id).is_some())
            .or_else(|| self.project.tracks.first().map(|track| track.id));
        self.selected_clip = self
            .selected_clip
            .filter(|id| self.project.midi_clip(*id).is_some());
        self.selected_notes.clear();
        self.drag = None;
        self.pending_edit = None;
        self.rebuild_graph();
        // The loop lives in the audio thread's transport and only SetLoop moves it, so a
        // document swap that does not republish leaves playback wrapping the old range.
        self.push_loop_to_engine();
    }

    /// Parameter descriptors for a plugin, instantiating it once to read them.
    pub(crate) fn param_descriptors(&mut self, plugin_id: &str) -> Arc<Vec<ParamDescriptor>> {
        if let Some(cached) = self.param_cache.get(plugin_id) {
            return cached.clone();
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
            .insert(plugin_id.to_string(), descriptors.clone());
        descriptors
    }

    /// Current value of a parameter, falling back to the descriptor's default.
    pub(crate) fn param_value(&self, target: ParamTarget, descriptor: &ParamDescriptor) -> f32 {
        let state_value = |state: &PluginState| {
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
                    state_value(&inner.instrument_state)
                }),
            ParamTarget::Effect { track, slot, .. } => {
                let strip = match track {
                    Some(id) => self.project.track(id).map(|t| &t.mixer),
                    None => Some(&self.project.master),
                };
                strip
                    .and_then(|strip| strip.effects.iter().find(|s| s.id == slot))
                    .map_or(descriptor.default, |s| state_value(&s.state))
            }
        }
    }

    /// Writes a parameter to the document and forwards it to the audio thread.
    pub(crate) fn set_param_value(&mut self, target: ParamTarget, value: f32) {
        self.dirty = true;
        match target {
            ParamTarget::TrackGain(id) => {
                let Some(index) = self.project.track_index(id) else {
                    return;
                };
                self.project.tracks[index].mixer.gain_db = value;
                self.send(EngineCommand::SetTrackGain {
                    index,
                    gain_db: value,
                });
            }
            ParamTarget::TrackPan(id) => {
                let Some(index) = self.project.track_index(id) else {
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
                let Some(index) = self.project.track_index(track) else {
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
                    Some(id) => match self.project.track_index(id) {
                        Some(index) => Some(index),
                        None => return,
                    },
                    None => None,
                };
                let strip = match track_index {
                    Some(index) => &mut self.project.tracks[index].mixer,
                    None => &mut self.project.master,
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

    /// Stable storage key for a plugin parameter, used when writing it into the project.
    fn param_key(&mut self, target: ParamTarget, param: ParamId) -> Option<String> {
        let plugin_id = match target {
            ParamTarget::Instrument { track, .. } => self
                .project
                .track(track)?
                .kind
                .as_instrument()?
                .instrument_id
                .clone(),
            ParamTarget::Effect { track, slot, .. } => {
                let strip = match track {
                    Some(id) => &self.project.track(id)?.mixer,
                    None => &self.project.master,
                };
                strip
                    .effects
                    .iter()
                    .find(|s| s.id == slot)?
                    .effect_id
                    .clone()
            }
            _ => return None,
        };
        let descriptors = self.param_descriptors(&plugin_id);
        descriptors
            .get(param.index())
            .map(|descriptor| descriptor.key.to_string())
    }

    /// Sends a command, logging rather than failing when the engine is not running.
    pub(crate) fn send(&self, command: EngineCommand) {
        if let Err(error) = self.engine.send(command) {
            log::debug!("engine command dropped: {error}");
        }
    }

    /// Playhead position in ticks, read from the audio thread's atomic.
    pub(crate) fn playhead_ticks(&self) -> Ticks {
        self.project
            .tempo_map
            .seconds_to_ticks(Seconds(self.engine.playhead_seconds()))
    }

    /// Moves the playhead, clamping to the timeline start.
    pub(crate) fn seek(&mut self, tick: Ticks) {
        let tick = tick.max_zero();
        let frames = self
            .project
            .tempo_map
            .ticks_to_samples(tick, self.engine.sample_rate())
            .raw();
        self.send(EngineCommand::Seek { frames });
    }

    /// `true` when the transport is running.
    pub(crate) fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    /// Starts or stops playback.
    pub(crate) fn toggle_play(&mut self) {
        if self.is_playing() {
            self.send(EngineCommand::Stop);
        } else {
            self.send(EngineCommand::Play);
        }
    }

    /// The selected MIDI clip, if there is one.
    pub(crate) fn selected_midi_clip(&self) -> Option<(TrackId, &MidiClip)> {
        self.selected_clip.and_then(|id| self.project.midi_clip(id))
    }

    /// Snaps a tick to the project grid.
    pub(crate) fn snap(&self, tick: Ticks) -> Ticks {
        tick.snap_nearest(self.project.grid)
    }

    /// Linear peak level of a track, for its meter.
    pub(crate) fn track_level(&self, index: usize) -> f32 {
        self.engine.meters().track_peak(index)
    }

    /// Linear peak level of the master bus.
    pub(crate) fn master_level(&self) -> f32 {
        self.engine.meters().master_peak()
    }

    /// Auditions a note through a track's instrument.
    pub(crate) fn audition_note_on(&self, track: TrackId, pitch: u8, velocity: f32) {
        if let Some(index) = self.project.track_index(track) {
            self.send(EngineCommand::NoteOn {
                track: index,
                pitch,
                velocity,
            });
        }
    }

    /// Releases an auditioned note.
    pub(crate) fn audition_note_off(&self, track: TrackId, pitch: u8) {
        if let Some(index) = self.project.track_index(track) {
            self.send(EngineCommand::NoteOff {
                track: index,
                pitch,
            });
        }
    }

    /// Sets the status line.
    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Window title, marking unsaved changes the way every editor does.
    pub(crate) fn window_title(&self) -> String {
        let name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| self.project.name.clone());
        if self.dirty {
            format!("{name} • Auris Studio")
        } else {
            format!("{name} — Auris Studio")
        }
    }

    /// Ends any drag in progress.
    pub(crate) fn end_drag(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let drag = self.drag.take();
        self.pending_edit = None;
        let changed = std::mem::take(&mut self.drag_changed);

        // Only clip, note and tempo drags move the scheduled events; a fader or a playhead
        // scrub does not. Rebuilding regardless re-instantiated every plugin, which discarded
        // reverb tails and re-attacked held notes on the release of an unrelated gesture.
        let structural = matches!(
            drag,
            Some(
                Drag::ClipMove { .. }
                    | Drag::ClipResize { .. }
                    | Drag::NoteMove { .. }
                    | Drag::NoteResize { .. }
                    | Drag::Tempo { .. }
            )
        );
        if changed && structural {
            self.rebuild_graph();
        }
    }

    /// Origin of the arrangement's clip lanes, taken from where they were last painted.
    pub(crate) fn lanes_origin(&self) -> Point<Pixels> {
        self.canvas.lanes.get().map_or_else(
            || {
                point(
                    crate::theme::Metrics::TRACK_HEADER_WIDTH,
                    crate::theme::Metrics::TRANSPORT_HEIGHT + crate::theme::Metrics::RULER_HEIGHT,
                )
            },
            |bounds| bounds.origin,
        )
    }

    /// Origin of the bar ruler, taken from where it was last painted.
    pub(crate) fn timeline_origin(&self) -> Point<Pixels> {
        self.canvas.ruler.get().map_or_else(
            || {
                point(
                    crate::theme::Metrics::TRACK_HEADER_WIDTH,
                    crate::theme::Metrics::TRANSPORT_HEIGHT,
                )
            },
            |bounds| bounds.origin,
        )
    }
}

/// Fills a fresh project with something audible, so the app is not an empty grid on launch.
fn seed_demo_project(project: &mut Project, registry: &PluginRegistry) {
    let instrument = registry
        .first_instrument_id()
        .unwrap_or("auris.synth.chiptune")
        .to_string();

    let lead = project.add_instrument_track("Lead", &instrument);
    let bass = project.add_instrument_track("Bass", &instrument);
    project.add_audio_track("Audio");

    // A two-bar arpeggio over a root-fifth bass, in C minor.
    let bar = Ticks::from_beats(4.0);
    if let Some(clip_id) = project.add_midi_clip(lead, "Arp", Ticks::ZERO, bar * 2)
        && let Some(clip) = project.midi_clip_mut(clip_id)
    {
        let pattern = [60, 63, 67, 70, 67, 63];
        let step = Ticks(Ticks::QUARTER.0 / 2);
        for (index, pitch) in pattern.iter().cycle().take(16).enumerate() {
            clip.notes
                .push(Note::new(*pitch, step * index as i64, step));
        }
    }
    if let Some(clip_id) = project.add_midi_clip(bass, "Root", Ticks::ZERO, bar * 2)
        && let Some(clip) = project.midi_clip_mut(clip_id)
    {
        for bar_index in 0..2 {
            let start = bar * bar_index;
            clip.notes.push(Note::new(36, start, Ticks::QUARTER * 2));
            clip.notes.push(Note::new(
                43,
                start + Ticks::QUARTER * 2,
                Ticks::QUARTER * 2,
            ));
        }
    }

    if let Some(track) = project.track_mut(bass) {
        track.mixer.gain_db = -4.0;
    }
    project.loop_region = Some((Ticks::ZERO, bar * 2));
}
