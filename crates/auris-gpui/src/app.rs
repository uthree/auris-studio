//! The application view: view state, and the gpui plumbing around a [`Session`].
//!
//! Everything that edits the document lives in `auris-session`. What is left here is what a
//! desktop UI genuinely owns: what is selected, where the timeline is scrolled, which panel is
//! showing, and what the pointer is currently dragging.
//!
//! Auris Studio is a single gpui view rather than a tree of entities. Every panel needs the
//! session and the selection, and gpui entities do not share mutable state, so the panels are
//! `impl AurisApp` blocks in the [`crate::ui`] modules — one owner of the truth, each panel
//! still in its own file.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Focusable, Pixels, Point, Task, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions, point, px, size,
};

use crate::gestures::PointerGestures;
use crate::keymap::{InputSettings, Keymap};
use crate::settings_window::SettingsWindow;
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::ContextMenu;
use crate::ui::prompt::Prompt;
use crate::ui::timeline::{PitchView, TimelineView};

/// A surface a selection rectangle can be swept across.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BandSurface {
    /// The piano roll's note grid.
    Roll,
    /// The arrangement's clip lanes.
    Lanes,
}

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
    /// Moving one or more clips along the timeline.
    ClipMove {
        /// Clip under the pointer, whose snapped position drives the others.
        clip: ClipId,
        /// Distance from that clip's start to the point that was grabbed, so it does not jump
        /// under the pointer.
        grab_offset: Ticks,
        /// Starting position of every selected clip, so the whole selection moves together.
        origins: Vec<(ClipId, Ticks)>,
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
    /// Sweeping a rectangle to select what it covers.
    RubberBand {
        /// Which surface is being swept.
        surface: BandSurface,
        /// Where the sweep began, in window coordinates.
        anchor: Point<Pixels>,
        /// Where the pointer is now, in window coordinates.
        current: Point<Pixels>,
        /// What was selected before the sweep started, kept so a shift-drag can add to it.
        base_notes: BTreeSet<usize>,
        /// Clips selected before the sweep started.
        base_clips: BTreeSet<ClipId>,
    },
    /// Dragging the divider between the arrangement and the inspector.
    ResizeInspector {
        /// Pointer x when the drag began.
        start_x: Pixels,
        /// Panel width when the drag began.
        start_width: Pixels,
    },
    /// Dragging the divider between the arrangement and the bottom editor.
    ResizeEditor {
        /// Pointer y when the drag began.
        start_y: Pixels,
        /// Panel height when the drag began.
        start_height: Pixels,
    },
    /// Dragging the divider between the track headers and the timeline.
    ResizeHeaders {
        /// Pointer x when the drag began.
        start_x: Pixels,
        /// Column width when the drag began.
        start_width: Pixels,
    },
}

impl Drag {
    /// The edit this gesture records, or `None` when it changes no document state.
    fn edit(&self) -> Option<Edit> {
        match self {
            Drag::Playhead => None,
            Drag::LoopRegion { .. } => Some(Edit::SetLoopRegion),
            Drag::ClipMove { .. } => Some(Edit::MoveClip),
            Drag::ClipResize { .. } => Some(Edit::ResizeClip),
            Drag::NoteMove { .. } => Some(Edit::MoveNotes),
            Drag::NoteResize { .. } => Some(Edit::ResizeNote),
            Drag::Param { .. } => Some(Edit::AdjustParameter),
            Drag::Tempo { .. } => Some(Edit::ChangeTempo),
            // Selecting is not an edit; it changes what a later edit will act on.
            Drag::RubberBand { .. } => None,
            // Panel geometry is a property of the window, not the document: resizing a panel
            // is not an edit and must never land on the undo stack.
            Drag::ResizeInspector { .. }
            | Drag::ResizeEditor { .. }
            | Drag::ResizeHeaders { .. } => None,
        }
    }
}

/// How wide or tall the resizable panels are, and whether they are showing.
///
/// Kept together so the layout, the hit tests and the splitters all read one source rather
/// than each deriving the geometry from constants and drifting apart.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// Whether the right-hand inspector is visible.
    pub inspector_visible: bool,
    /// Width of the inspector.
    pub inspector_width: Pixels,
    /// Whether the bottom editor is visible.
    pub editor_visible: bool,
    /// Height of the bottom editor.
    pub editor_height: Pixels,
    /// Width of the track header column.
    pub header_width: Pixels,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            inspector_visible: true,
            inspector_width: Metrics::INSPECTOR_WIDTH,
            editor_visible: true,
            editor_height: Metrics::EDITOR_HEIGHT,
            header_width: Metrics::TRACK_HEADER_WIDTH,
        }
    }
}

impl PanelLayout {
    /// Narrowest the inspector may be dragged.
    pub const MIN_INSPECTOR: Pixels = px(200.0);
    /// Widest the inspector may be dragged.
    pub const MAX_INSPECTOR: Pixels = px(520.0);
    /// Shortest the bottom editor may be dragged.
    pub const MIN_EDITOR: Pixels = px(120.0);
    /// Narrowest the track header column may be dragged.
    pub const MIN_HEADERS: Pixels = px(140.0);
    /// Widest the track header column may be dragged.
    pub const MAX_HEADERS: Pixels = px(360.0);

    /// Height the arrangement must keep, so a dragged editor cannot swallow it.
    pub const MIN_ARRANGEMENT: Pixels = px(140.0);

    /// Inspector width after dragging its divider by `delta`.
    ///
    /// The divider sits on the panel's *left* edge, so dragging left widens it — hence the
    /// subtraction. Getting that sign wrong makes the panel run away from the pointer.
    pub fn resized_inspector(start_width: Pixels, delta: Pixels) -> Pixels {
        (start_width - delta)
            .max(Self::MIN_INSPECTOR)
            .min(Self::MAX_INSPECTOR)
    }

    /// Bottom-editor height after dragging its divider by `delta`.
    ///
    /// `available` is what is left of the window once the transport, the status bar and the
    /// arrangement's own minimum are accounted for. It is clamped up to the minimum height as
    /// well, so a window too short for both panels still yields a usable editor rather than a
    /// negative one.
    pub fn resized_editor(start_height: Pixels, delta: Pixels, available: Pixels) -> Pixels {
        (start_height - delta)
            .max(Self::MIN_EDITOR)
            .min(available.max(Self::MIN_EDITOR))
    }

    /// Track-header column width after dragging its divider by `delta`.
    pub fn resized_headers(start_width: Pixels, delta: Pixels) -> Pixels {
        (start_width + delta)
            .max(Self::MIN_HEADERS)
            .min(Self::MAX_HEADERS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inspector_follows_the_pointer_and_stops_at_its_limits() {
        let start = px(300.0);
        // Dragging the divider left widens the panel it sits in front of.
        assert_eq!(PanelLayout::resized_inspector(start, px(-40.0)), px(340.0));
        assert_eq!(PanelLayout::resized_inspector(start, px(40.0)), px(260.0));
        assert_eq!(
            PanelLayout::resized_inspector(start, px(9_000.0)),
            PanelLayout::MIN_INSPECTOR
        );
        assert_eq!(
            PanelLayout::resized_inspector(start, px(-9_000.0)),
            PanelLayout::MAX_INSPECTOR
        );
    }

    #[test]
    fn the_editor_never_swallows_the_arrangement() {
        let available = px(400.0);
        assert_eq!(
            PanelLayout::resized_editor(px(280.0), px(-60.0), available),
            px(340.0)
        );
        assert_eq!(
            PanelLayout::resized_editor(px(280.0), px(-9_000.0), available),
            available
        );
        assert_eq!(
            PanelLayout::resized_editor(px(280.0), px(9_000.0), available),
            PanelLayout::MIN_EDITOR
        );
    }

    #[test]
    fn a_window_too_short_for_both_panels_still_gives_a_usable_editor() {
        // `available` goes negative once the window cannot hold the arrangement's minimum.
        let squeezed = PanelLayout::resized_editor(px(280.0), px(0.0), px(-50.0));
        assert_eq!(squeezed, PanelLayout::MIN_EDITOR);
    }

    #[test]
    fn the_header_column_stops_at_its_limits() {
        let start = px(200.0);
        assert_eq!(PanelLayout::resized_headers(start, px(30.0)), px(230.0));
        assert_eq!(
            PanelLayout::resized_headers(start, px(-9_000.0)),
            PanelLayout::MIN_HEADERS
        );
        assert_eq!(
            PanelLayout::resized_headers(start, px(9_000.0)),
            PanelLayout::MAX_HEADERS
        );
    }
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

/// Waveform peaks keyed by audio source, shared by every lane in a frame.
pub type WaveformMap = std::collections::HashMap<SourceId, Arc<WaveformPeaks>>;

/// The whole application.
pub struct AurisApp {
    /// The document, the engine and every command that touches them.
    pub(crate) session: Session,

    pub(crate) theme: Theme,
    pub(crate) timeline: TimelineView,
    pub(crate) pitch: PitchView,
    pub(crate) selected_track: Option<TrackId>,
    /// The clip the editors point at. Always a member of [`Self::selected_clips`].
    pub(crate) selected_clip: Option<ClipId>,
    /// Every selected clip. Edits act on all of them; the piano roll edits the primary one.
    pub(crate) selected_clips: BTreeSet<ClipId>,
    pub(crate) selected_notes: BTreeSet<usize>,
    pub(crate) drag: Option<Drag>,
    pub(crate) editor: EditorTab,
    pub(crate) inspector: InspectorTab,
    /// Which panels are showing and how large they are.
    pub(crate) panels: PanelLayout,
    pub(crate) status: String,
    pub(crate) export: Option<ExportState>,
    /// Note currently sounding because the user is holding a key or dragging one.
    pub(crate) auditioning: Option<(TrackId, u8)>,
    /// Keyboard focus target, so the action bindings reach this view.
    pub(crate) focus: FocusHandle,
    /// Window height as of the last frame, used only as a fallback before the first paint.
    pub(crate) viewport_height: Pixels,
    /// Width of the arrangement body as of the last frame.
    pub(crate) arrangement_width: Pixels,
    /// Rectangles the canvases were painted into last frame.
    pub(crate) canvas: CanvasBounds,
    /// The open right-click menu, if any.
    pub(crate) menu: Option<ContextMenu>,
    /// The open rename sheet, if any.
    pub(crate) prompt: Option<Prompt>,

    /// Preferences that outlive the session.
    pub(crate) settings: Settings,
    /// Language every string in the window is looked up in.
    pub(crate) language: Language,
    /// The user's key bindings.
    pub(crate) keymap: Keymap,
    /// What a click creates and what deletes.
    pub(crate) pointer: PointerGestures,
    /// The settings window, while it is open.
    pub(crate) settings_window: Option<WindowHandle<SettingsWindow>>,

    _repaint: Task<()>,
}

impl Focusable for AurisApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AurisApp {
    /// Builds the application, starting audio and creating a small demo project.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let language = settings.language();
        let input = InputSettings::load();
        let keymap = input.keys.clone();
        keymap.apply(cx);

        let mut session = Session::new(SessionOptions {
            audio_preferences: settings.audio.clone(),
            ..SessionOptions::default()
        })
        .expect("a session opens even without audio");
        seed_demo_project(&mut session);

        let status = audio_line(&session.audio_status(), language);
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
                        this.session.poll();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let selected_track = session.project().tracks.first().map(|track| track.id);
        let selected_clip = session.project().tracks.first().and_then(|track| {
            track
                .kind
                .as_instrument()
                .and_then(|inner| inner.clips.first())
                .map(|clip| clip.id)
        });

        let mut app = Self {
            session,
            theme: Theme::dark(),
            timeline: TimelineView::default(),
            pitch: PitchView::default(),
            selected_track,
            selected_clip,
            selected_clips: selected_clip.into_iter().collect(),
            selected_notes: BTreeSet::new(),
            drag: None,
            editor: EditorTab::PianoRoll,
            inspector: InspectorTab::Track,
            panels: PanelLayout::default(),
            status,
            export: None,
            auditioning: None,
            focus: cx.focus_handle(),
            viewport_height: px(900.0),
            arrangement_width: px(900.0),
            canvas: CanvasBounds::default(),
            menu: None,
            prompt: None,
            settings,
            language,
            pointer: input.pointer,
            keymap,
            settings_window: None,
            _repaint: repaint,
        };
        // The default pitch view sits two octaves above the demo material, so without this the
        // roll opens on an empty grid and the clip looks like it has no notes.
        app.center_roll_on_selection();
        app
    }

    /// The document.
    pub(crate) fn project(&self) -> &Project {
        self.session.project()
    }

    /// Everything the registry knows how to build.
    pub(crate) fn registry(&self) -> &Arc<PluginRegistry> {
        self.session.registry()
    }

    /// Waveform peaks for every loaded source, as the arrangement's paint closure needs them.
    pub(crate) fn waveform_map(&self) -> WaveformMap {
        self.project()
            .audio_sources
            .keys()
            .filter_map(|id| {
                self.session
                    .waveform(*id)
                    .map(|peaks| (*id, Arc::clone(peaks)))
            })
            .collect()
    }

    // ---------------------------------------------------------------- gestures

    /// Begins a gesture. Every edit it makes becomes one undo step and one graph rebuild.
    pub(crate) fn begin_drag(&mut self, drag: Drag) {
        if let Some(edit) = drag.edit() {
            self.session.begin_transaction(edit);
        }
        self.drag = Some(drag);
    }

    /// Ends any gesture in progress.
    pub(crate) fn end_drag(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take()
            && drag.edit().is_some()
        {
            // A gesture that changed nothing records no undo step and triggers no rebuild.
            self.session.end_transaction();
        }
    }

    // ---------------------------------------------------------------- transport

    /// `true` when the transport is rolling.
    pub(crate) fn is_playing(&self) -> bool {
        self.session.is_playing()
    }

    /// Starts or stops playback.
    pub(crate) fn toggle_play(&mut self) {
        self.session.toggle_play();
    }

    /// Playhead position in ticks.
    pub(crate) fn playhead_ticks(&self) -> Ticks {
        self.session.playhead()
    }

    /// Moves the playhead.
    pub(crate) fn seek(&mut self, tick: Ticks) {
        self.session.seek(tick);
    }

    // ---------------------------------------------------------------- selection

    /// Selects one clip, or nothing, replacing whatever was selected.
    ///
    /// Going through here rather than assigning the field is what keeps the primary clip and
    /// the selection from disagreeing — an editor pointed at a clip that is not selected shows
    /// notes that Delete would not touch.
    pub(crate) fn select_clip(&mut self, clip: Option<ClipId>) {
        self.selected_clip = clip;
        self.selected_clips = clip.into_iter().collect();
    }

    /// Selects a set of clips, pointing the editors at `primary`.
    ///
    /// `primary` joins the selection if it is not already in it, and is dropped when it is not
    /// one of them and the set is not empty.
    pub(crate) fn select_clips(&mut self, clips: BTreeSet<ClipId>, primary: Option<ClipId>) {
        self.selected_clip = match primary {
            Some(id) if clips.contains(&id) => Some(id),
            _ => clips.iter().next().copied(),
        };
        self.selected_clips = clips;
    }

    /// Whether a clip of either kind is still in the document.
    pub(crate) fn clip_exists(&self, clip: ClipId) -> bool {
        self.session.midi_clip(clip).is_some()
            || self.project().tracks.iter().any(|track| {
                track
                    .kind
                    .as_audio()
                    .is_some_and(|inner| inner.clips.iter().any(|c| c.id == clip))
            })
    }

    /// The selected MIDI clip, if there is one.
    pub(crate) fn selected_midi_clip(&self) -> Option<&MidiClip> {
        self.selected_clip.and_then(|id| self.session.midi_clip(id))
    }

    /// Snaps a tick to the project grid.
    pub(crate) fn snap(&self, tick: Ticks) -> Ticks {
        tick.snap_nearest(self.project().grid)
    }

    // ---------------------------------------------------------------- metering

    /// Linear peak level of a track.
    pub(crate) fn track_level(&self, index: usize) -> f32 {
        self.session.meters().track_peak(index)
    }

    /// Linear peak level of the master bus.
    pub(crate) fn master_level(&self) -> f32 {
        self.session.meters().master_peak()
    }

    // ---------------------------------------------------------------- audition

    /// Sounds one note on the selected track, replacing any note already sounding.
    pub(crate) fn audition(&mut self, pitch: u8) {
        let Some(track) = self.selected_track else {
            return;
        };
        self.stop_audition();
        self.session.note_on(track, pitch, 0.8);
        self.auditioning = Some((track, pitch));
    }

    /// Releases the auditioned note, if any.
    pub(crate) fn stop_audition(&mut self) {
        if let Some((track, pitch)) = self.auditioning.take() {
            self.session.note_off(track, pitch);
        }
    }

    // ---------------------------------------------------------------- chrome

    /// Sets the status line.
    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Window title, marking unsaved changes the way every editor does.
    pub(crate) fn window_title(&self) -> String {
        let name = self
            .session
            .path()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| self.project().name.clone());
        if self.session.is_dirty() {
            format!("{name} • Auris Studio")
        } else {
            format!("{name} — Auris Studio")
        }
    }

    // ---------------------------------------------------------------- settings

    /// Switches the audio backend and remembers the choice.
    ///
    /// Returns the status line to show. Saving is best-effort: failing to write a preferences
    /// file must not undo a device change that already worked.
    pub(crate) fn apply_audio_preferences(
        &mut self,
        audio: AudioPreferences,
    ) -> Result<String, String> {
        self.session
            .set_audio_preferences(audio.clone())
            .map_err(|error| error.to_string())?;
        self.settings.audio = audio;
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }

        let line = audio_line(&self.session.audio_status(), self.language);
        self.set_status(line.clone());
        Ok(line)
    }

    /// Switches the interface language and remembers the choice.
    ///
    /// `preference` of `None` means "follow the system", which is what a fresh install does.
    /// Saving is best-effort: a preferences file that cannot be written must not undo a change
    /// the user can already see.
    pub(crate) fn apply_language(&mut self, preference: Option<Language>, cx: &mut App) {
        self.settings.language = preference;
        self.language = self.settings.language();
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }
        // The menu bar belongs to the platform, not to a view, so it is rebuilt rather than
        // re-rendered — nothing would redraw it otherwise.
        cx.set_menus(crate::menus(self.language));
        let name = self.language.endonym();
        self.set_status(messages::language_changed(self.language, name));
    }

    /// Installs an edited keymap and remembers it.
    pub(crate) fn apply_keymap(&mut self, keymap: Keymap, cx: &mut App) {
        keymap.apply(cx);
        self.keymap = keymap;
        self.save_input();
    }

    /// Installs edited pointer gestures and remembers them.
    pub(crate) fn apply_pointer_gestures(&mut self, pointer: PointerGestures) {
        self.pointer = pointer;
        self.save_input();
    }

    /// Writes the input settings file.
    ///
    /// Best-effort: a preferences file that cannot be written must not undo a change the user
    /// can already see working.
    fn save_input(&self) {
        let settings = InputSettings {
            keys: self.keymap.clone(),
            pointer: self.pointer,
        };
        if let Err(error) = settings.save() {
            log::warn!("could not save input settings: {error}");
        }
    }

    /// Opens the settings window, or brings the open one forward.
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }

        // Gather everything the window needs *before* opening it: the constructor runs inside
        // this update, and reading `self` back through the entity handle would panic.
        let app = cx.entity().downgrade();
        let theme = self.theme.clone();
        let devices = self.session.output_devices();
        let audio = self.session.audio_preferences().clone();
        let live = self.session.audio_status();
        let keymap = self.keymap.clone();
        let language = self.settings.language;
        let pointer = self.pointer;

        let bounds = Bounds::centered(None, size(px(560.), px(620.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(self.t(Key::Settings).into()),
                    ..Default::default()
                }),
                focus: true,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    SettingsWindow::new(
                        app, theme, devices, audio, live, keymap, language, pointer, cx,
                    )
                })
            },
        );
        match opened {
            Ok(handle) => self.settings_window = Some(handle),
            Err(error) => {
                let text =
                    messages::failed(self.language, self.t(Key::Settings), &error.to_string());
                self.set_status(text);
            }
        }
    }

    // ---------------------------------------------------------------- panels

    /// Shows or hides the right-hand inspector.
    pub(crate) fn toggle_inspector(&mut self) {
        self.panels.inspector_visible = !self.panels.inspector_visible;
    }

    /// Shows or hides the bottom editor.
    pub(crate) fn toggle_editor(&mut self) {
        self.panels.editor_visible = !self.panels.editor_visible;
    }

    /// Applies an inspector resize drag.
    pub(crate) fn resize_inspector(&mut self, start_width: Pixels, delta: Pixels) {
        self.panels.inspector_width = PanelLayout::resized_inspector(start_width, delta);
    }

    /// Applies a bottom-editor resize drag.
    pub(crate) fn resize_editor(&mut self, start_height: Pixels, delta: Pixels) {
        let available = self.viewport_height
            - Metrics::TRANSPORT_HEIGHT
            - Metrics::STATUS_HEIGHT
            - PanelLayout::MIN_ARRANGEMENT;
        self.panels.editor_height = PanelLayout::resized_editor(start_height, delta, available);
    }

    /// Applies a track-header column resize drag.
    pub(crate) fn resize_headers(&mut self, start_width: Pixels, delta: Pixels) {
        self.panels.header_width = PanelLayout::resized_headers(start_width, delta);
    }

    // ---------------------------------------------------------------- geometry

    /// Origin of the arrangement's clip lanes, taken from where they were last painted.
    pub(crate) fn lanes_origin(&self) -> Point<Pixels> {
        self.canvas.lanes.get().map_or_else(
            || {
                point(
                    self.panels.header_width,
                    Metrics::TRANSPORT_HEIGHT + Metrics::RULER_HEIGHT,
                )
            },
            |bounds| bounds.origin,
        )
    }

    /// Origin of the bar ruler, taken from where it was last painted.
    pub(crate) fn timeline_origin(&self) -> Point<Pixels> {
        self.canvas.ruler.get().map_or_else(
            || point(self.panels.header_width, Metrics::TRANSPORT_HEIGHT),
            |bounds| bounds.origin,
        )
    }
}

/// The status line describing what the audio backend ended up doing.
fn audio_line(status: &auris_session::AudioStatus, language: Language) -> String {
    let engine = if status.running {
        messages::audio_status(
            language,
            &status.device,
            status.sample_rate,
            status.channels,
        )
    } else {
        Key::NoAudioOutput.get(language).to_string()
    };
    let gpu = match &status.gpu {
        Some(adapter) => messages::gpu_in_use(language, adapter),
        None => messages::gpu_unavailable(language),
    };
    format!("{engine} · {gpu}")
}

/// Fills a fresh session with something audible, so the app is not an empty grid on launch.
fn seed_demo_project(session: &mut Session) {
    let bar = Ticks::from_beats(4.0);

    let Ok(lead) = session.add_default_instrument_track("Lead") else {
        return;
    };
    let Ok(bass) = session.add_default_instrument_track("Bass") else {
        return;
    };
    session.add_audio_track("Audio");

    // A two-bar arpeggio over a root-fifth bass, in C minor.
    if let Ok(clip) = session.add_midi_clip(lead, "Arp", Ticks::ZERO, bar * 2) {
        let step = Ticks(Ticks::QUARTER.raw() / 2);
        for (index, pitch) in [60u8, 63, 67, 70, 67, 63]
            .iter()
            .cycle()
            .take(16)
            .enumerate()
        {
            let _ = session.add_note(clip, Note::new(*pitch, step * index as i64, step));
        }
    }
    if let Ok(clip) = session.add_midi_clip(bass, "Root", Ticks::ZERO, bar * 2) {
        for bar_index in 0..2 {
            let start = bar * bar_index;
            let _ = session.add_note(clip, Note::new(36, start, Ticks::QUARTER * 2));
            let _ = session.add_note(
                clip,
                Note::new(43, start + Ticks::QUARTER * 2, Ticks::QUARTER * 2),
            );
        }
    }

    session.set_param(ParamTarget::TrackGain(bass), -4.0);
    session.set_loop_region(Ticks::ZERO, bar * 2);

    // The demo is scaffolding, not an edit the user made: nothing here should be undoable, and
    // the document should open clean rather than already marked as modified.
    session.forget_history();
}
