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

use crate::appearance::Appearance;
use crate::gestures::PointerGestures;
use crate::keymap::{InputSettings, Keymap};
use crate::settings_window::SettingsWindow;
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::ContextMenu;
use crate::ui::menu_bar::OpenMenu;
use crate::ui::prompt::Prompt;
use crate::ui::timeline::{PitchView, TimelineView};

/// What a press or a sweep at one position should do to whatever is already sounding.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Audition {
    /// Nothing is written here; release what is sounding.
    Silence,
    /// The same chord is already sounding; leave it alone.
    Hold,
    /// A different chord; strike it.
    Strike,
}

/// Which of the three a set of pitches calls for, given what is sounding now.
///
/// Extracted from the handler because [`Audition::Hold`] is the case that is easy to lose and
/// impossible to miss once it is gone: a sweep along the lane asks this on every pointer move, and
/// without it a chord four bars wide is struck again every few pixels.
pub fn audition_for(sounding: Option<&[u8]>, pitches: &[u8]) -> Audition {
    if pitches.is_empty() {
        Audition::Silence
    } else if sounding == Some(pitches) {
        Audition::Hold
    } else {
        Audition::Strike
    }
}

/// How hard an auditioned note is struck.
const NOTE_VELOCITY: f32 = 0.8;

/// How hard each note of an auditioned chord is struck.
///
/// Softer than one note, because four or five voices sum. A piano would not need this — striking
/// a chord is striking each key — but four synth voices at the same velocity as one are four
/// times the signal, and the point is to identify the chord rather than to be startled by it.
const CHORD_VELOCITY: f32 = 0.55;

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
    /// Moving one or more clips along the timeline, and between tracks.
    ClipMove {
        /// Clip under the pointer, whose snapped position drives the others.
        clip: ClipId,
        /// Distance from that clip's start to the point that was grabbed, so it does not jump
        /// under the pointer.
        grab_offset: Ticks,
        /// Starting position of every selected clip, so the whole selection moves together.
        origins: Vec<(ClipId, Ticks)>,
        /// Lane each selected clip started on, so the selection keeps its shape as it crosses
        /// tracks: every clip moves by the same number of lanes rather than collapsing onto the
        /// one under the pointer.
        origin_lanes: Vec<(ClipId, usize)>,
        /// Lane the grabbed clip started on, which the pointer's lane is measured against.
        grab_lane: usize,
        /// Where the button went down, until the pointer has travelled far enough to mean it.
        ///
        /// A clip's start is snapped to the grid as it moves, so a clip that is *not* on the grid
        /// — after a split at the playhead, say — jumped onto it the instant a click wobbled by a
        /// pixel. Cleared once the gesture is past [`crate::gestures::DRAG_THRESHOLD`], so coming
        /// back towards the starting point still moves the clip rather than freezing it.
        pressed_at: Option<Point<Pixels>>,
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
    /// Sweeping along the harmony lane with the button held, hearing each chord in turn.
    ///
    /// Carries nothing: what sounds is whatever is written under the pointer, which the document
    /// already knows. It is a drag rather than a click so that a progression can be *heard as a
    /// progression* — pressing four chords one at a time tells you what each one is, and dragging
    /// across them tells you whether they go anywhere.
    AuditionHarmony,
    /// Moving a chord along the harmony lane by its leading edge.
    HarmonyChord {
        /// Where the chord being moved sits *now*, which the drag updates as it goes: the
        /// document is edited on every pointer move, so the position it started from stops being
        /// the position it is at.
        at: Ticks,
        /// Distance from the chord to the point that was grabbed, so it does not jump under the
        /// pointer.
        grab_offset: Ticks,
    },
    /// Turning one of a generated clip's dials.
    ///
    /// Separate from [`Drag::Param`] because a recipe is not a plugin parameter: there is no
    /// descriptor to normalise through, and moving it rewrites the clip's notes rather than
    /// setting a number the audio thread reads.
    PartDial {
        /// Clip being rewritten.
        clip: ClipId,
        /// Which dial.
        dial: crate::ui::part::Dial,
        /// Where the bar was when the drag began, from 0 to 1.
        start_fraction: f32,
        /// Pointer x when the drag began.
        start_x: Pixels,
    },
    /// Dragging the time-zoom slider.
    TimeZoom {
        /// Slider position when the drag began, from 0 to 1.
        start_fraction: f32,
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
    /// Moving the floating plugin editor by its title bar.
    MovePluginWindow {
        /// Distance from the window's own origin to the point that was grabbed, so it does not
        /// jump under the pointer.
        grab_offset: Point<Pixels>,
    },
    /// Dragging the divider between the library and the arrangement.
    ResizeLibrary {
        /// Pointer x when the drag began.
        start_x: Pixels,
        /// Panel width when the drag began.
        start_width: Pixels,
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
            // One undo step for the whole sweep, and the same label the right-click menu's
            // "Write It Again" uses — moving a dial is writing the part again with one thing
            // changed, and a stack full of "Adjusted parameter" would say nothing about which.
            Drag::PartDial { .. } => Some(Edit::GenerateClip),
            Drag::Tempo { .. } => Some(Edit::ChangeTempo),
            // Selecting is not an edit; it changes what a later edit will act on.
            Drag::RubberBand { .. } => None,
            // Listening changes nothing at all.
            Drag::AuditionHarmony => None,
            Drag::HarmonyChord { .. } => Some(Edit::MoveChord),
            // How far in the view is zoomed is a property of the window, like a panel's width.
            Drag::TimeZoom { .. } => None,
            // Panel and window geometry is a property of the window, not the document: resizing
            // a panel or moving the plugin editor is not an edit and must never land on the undo
            // stack.
            Drag::ResizeLibrary { .. }
            | Drag::ResizeInspector { .. }
            | Drag::ResizeEditor { .. }
            | Drag::ResizeHeaders { .. }
            | Drag::MovePluginWindow { .. } => None,
        }
    }
}

/// How wide or tall the resizable panels are, and whether they are showing.
///
/// Kept together so the layout, the hit tests and the splitters all read one source rather
/// than each deriving the geometry from constants and drifting apart.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// Whether the left-hand library is visible.
    pub library_visible: bool,
    /// Width of the library.
    pub library_width: Pixels,
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
            // Open on first run, unlike Logic, which defaults its Library closed. There, a
            // channel strip's instrument slot is a second way to load one; here the library is
            // the only one, so starting closed would leave a new project with no visible way to
            // choose an instrument at all.
            library_visible: true,
            library_width: Metrics::LIBRARY_WIDTH,
            inspector_visible: true,
            inspector_width: Metrics::INSPECTOR_WIDTH,
            editor_visible: true,
            editor_height: Metrics::EDITOR_HEIGHT,
            header_width: Metrics::TRACK_HEADER_WIDTH,
        }
    }
}

impl PanelLayout {
    /// Narrowest the library may be dragged.
    pub const MIN_LIBRARY: Pixels = px(180.0);
    /// Widest the library may be dragged.
    pub const MAX_LIBRARY: Pixels = px(420.0);
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
    /// Width the arrangement must keep, for the same reason horizontally.
    pub const MIN_ARRANGEMENT_WIDTH: Pixels = px(240.0);

    /// Library width after dragging its divider by `delta`.
    ///
    /// The divider sits on the panel's *right* edge, so dragging right widens it and the delta
    /// is added — the opposite of [`Self::resized_inspector`], whose divider is on its left. The
    /// sign is the whole difference between the two, and getting it wrong makes a panel run away
    /// from the pointer.
    ///
    /// `available` is what is left of the window once the arrangement's own minimum width is
    /// accounted for, in the shape [`Self::resized_editor`] already uses vertically. It is
    /// clamped up to the minimum, so a window already narrower than the arrangement needs still
    /// yields a usable library rather than a negative one.
    pub fn resized_library(start_width: Pixels, delta: Pixels, available: Pixels) -> Pixels {
        let ceiling = Self::MAX_LIBRARY.min(available).max(Self::MIN_LIBRARY);
        (start_width + delta).max(Self::MIN_LIBRARY).min(ceiling)
    }

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
    fn sweeping_a_progression_strikes_each_chord_once() {
        // One gesture across four bars of chords, as a sequence of pointer moves. What must come
        // out is one strike per chord — not one per pixel, which is what a check for "is this
        // already sounding" is the only thing preventing.
        let c = [48u8, 60, 64, 67];
        let g = [43u8, 55, 59, 62];

        assert_eq!(audition_for(None, &c), Audition::Strike, "the first press");
        assert_eq!(audition_for(Some(&c), &c), Audition::Hold, "still on C");
        assert_eq!(audition_for(Some(&c), &g), Audition::Strike, "onto G");

        // Off the end of the written chords, and back on again.
        assert_eq!(audition_for(Some(&g), &[]), Audition::Silence);
        assert_eq!(audition_for(None, &[]), Audition::Silence, "already quiet");
        assert_eq!(audition_for(None, &g), Audition::Strike);
    }

    #[test]
    fn pressing_the_same_chord_again_sounds_it_again() {
        // Two separate presses on one chord, which is a different thing from a sweep that never
        // left it: the button coming up releases the notes, so the second press finds silence.
        let c = [48u8, 60, 64, 67];
        assert_eq!(audition_for(Some(&c), &c), Audition::Hold);
        assert_eq!(
            audition_for(None, &c),
            Audition::Strike,
            "after the release"
        );
    }

    #[test]
    fn the_library_follows_the_pointer_and_stops_at_its_limits() {
        // Its divider is on the panel's right edge, so the delta is *added* — the opposite sign
        // to the inspector's, whose test sits directly below this one so the difference is
        // written down rather than remembered.
        let roomy = px(9_000.0);
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(40.0), roomy),
            px(280.0)
        );
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(-40.0), roomy),
            px(200.0)
        );
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(9_000.0), roomy),
            PanelLayout::MAX_LIBRARY
        );
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(-9_000.0), roomy),
            PanelLayout::MIN_LIBRARY
        );
    }

    #[test]
    fn the_library_cannot_squeeze_the_arrangement_off_the_window() {
        // A drag with plenty of travel left still stops where the arrangement's minimum begins.
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(9_000.0), px(300.0)),
            px(300.0)
        );
        // And a window already narrower than the arrangement needs gives a usable library rather
        // than a negative one, the way the editor does when the window is too short for both.
        assert_eq!(
            PanelLayout::resized_library(px(240.0), px(9_000.0), px(-500.0)),
            PanelLayout::MIN_LIBRARY
        );
    }

    #[test]
    fn resizing_the_library_is_never_an_edit() {
        // It belongs in the arm that records nothing. A variant that fell through to a document
        // edit would open a session transaction and land on the undo stack, so pressing undo
        // after dragging a divider would lose the last real change instead.
        assert!(
            Drag::ResizeLibrary {
                start_x: px(0.0),
                start_width: px(0.0)
            }
            .edit()
            .is_none()
        );
    }

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
    /// The chord strip between the ruler and the lanes.
    pub harmony: Rc<Cell<Option<Bounds<Pixels>>>>,
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
    /// Which panels are showing and how large they are.
    pub(crate) panels: PanelLayout,
    pub(crate) status: String,
    pub(crate) export: Option<ExportState>,
    /// Notes currently sounding because the user is holding a key, dragging one, or pressing a
    /// chord on the harmony lane.
    pub(crate) auditioning: Option<(TrackId, Vec<u8>)>,
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
    /// Which menu-bar menu is open, on the platforms that draw their own bar.
    pub(crate) menu_bar: Option<OpenMenu>,
    /// The open rename sheet, if any.
    pub(crate) prompt: Option<Prompt>,
    /// The open command palette, if any.
    pub(crate) palette: Option<crate::ui::palette::Palette>,
    /// The open plugin editor, if any.
    pub(crate) plugin_window: Option<crate::ui::plugin_window::PluginWindow>,
    /// Which branches of the library are open.
    pub(crate) library: crate::ui::library::LibraryTree,
    /// The title the operating system was last told, so it is only told again on a change.
    pub(crate) titled: String,
    /// Whether the export destination dialog is open.
    ///
    /// [`Self::export`] is not set until a path comes back, so this is what stops a second
    /// Export while the picker is still up.
    pub(crate) choosing_export: bool,
    /// Whether [`Self::status`] is reporting a failure, so it can be shown as one.
    pub(crate) status_failed: bool,
    /// How far the arrangement's lanes are scrolled down, in pixels.
    ///
    /// The headers and the clip canvas both read it, so the two columns cannot slide apart —
    /// which is the whole reason it lives here rather than in either of them.
    pub(crate) lane_scroll: Pixels,

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
    /// Builds the application, starting audio and opening an empty document.
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
        // The same empty document File → New gives, rather than a separate idea of what a fresh
        // start looks like. Launching used to leave a two-bar arpeggio and a bass line lying
        // around, which was useful while there was nothing else to hear and is now just
        // somebody else's music to delete before starting.
        session.new_project();

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

        Self {
            session,
            theme: Appearance::load().theme(),
            timeline: TimelineView::default(),
            pitch: PitchView::default(),
            selected_track,
            selected_clip,
            selected_clips: selected_clip.into_iter().collect(),
            selected_notes: BTreeSet::new(),
            drag: None,
            editor: EditorTab::PianoRoll,
            panels: PanelLayout::default(),
            status,
            export: None,
            auditioning: None,
            focus: cx.focus_handle(),
            viewport_height: px(900.0),
            arrangement_width: px(900.0),
            canvas: CanvasBounds::default(),
            menu: None,
            menu_bar: None,
            prompt: None,
            palette: None,
            plugin_window: None,
            library: crate::ui::library::LibraryTree::default(),
            titled: String::new(),
            choosing_export: false,
            status_failed: false,
            lane_scroll: px(0.0),
            settings,
            language,
            pointer: input.pointer,
            keymap,
            settings_window: None,
            _repaint: repaint,
        }
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

    /// Abandons any gesture in progress, putting the document back where it started.
    ///
    /// The counterpart to [`Self::end_drag`], for the paths that mean "never mind" rather than
    /// "that will do". Every path that clears [`Self::drag`] must go through one of the two: a
    /// transaction left open swallows every later edit, because [`auris_session::Session`]
    /// neither records nor rebuilds while one is running.
    pub(crate) fn abort_drag(&mut self) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if drag.edit().is_some() {
            self.session.revert_transaction();
        }
        true
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

    /// `tick` on the grid, unless the gesture asked for it not to be.
    ///
    /// Holding the platform's command modifier suspends snapping for as long as it is held,
    /// which is the gesture every DAW uses for "put it exactly here". Without it — and without
    /// an off position on the grid button, which there also was not — nothing a user placed
    /// could sit off the beat.
    pub(crate) fn snap_unless_held(&self, tick: Ticks, modifiers: gpui::Modifiers) -> Ticks {
        if modifiers.secondary() {
            tick
        } else {
            self.snap(tick)
        }
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

    /// Sounds one note on the selected track, replacing anything already sounding.
    pub(crate) fn audition(&mut self, pitch: u8) {
        let Some(track) = self.selected_track else {
            return;
        };
        self.sound(track, vec![pitch], NOTE_VELOCITY);
    }

    /// Sounds the chord in force at `tick`, and says so when nothing can play it.
    ///
    /// The instrument is borrowed rather than required to be selected: harmony belongs to the
    /// timeline, and the point of writing it first is that the parts do not exist yet.
    ///
    /// Safe to call on every pointer move, which is what lets one gesture sweep a progression. A
    /// chord already sounding is left alone rather than struck again — retriggering it every few
    /// pixels would turn a sweep into a machine gun — and moving off the end of the chords goes
    /// quiet, because that is what is written there.
    pub(crate) fn audition_chord(&mut self, tick: Ticks) {
        let pitches = self.session.harmony_voicing(tick);
        let sounding = self
            .auditioning
            .as_ref()
            .map(|(_, pitches)| pitches.as_slice());
        match audition_for(sounding, &pitches) {
            Audition::Silence => self.stop_audition(),
            Audition::Hold => {}
            Audition::Strike => match self.session.audition_track(self.selected_track) {
                Some(track) => self.sound(track, pitches, CHORD_VELOCITY),
                None => self.set_status(self.t(Key::NoInstrumentToHearItOn)),
            },
        }
    }

    /// Sounds a set of notes, replacing anything already sounding.
    fn sound(&mut self, track: TrackId, pitches: Vec<u8>, velocity: f32) {
        self.stop_audition();
        self.session.notes_on(track, &pitches, velocity);
        self.auditioning = Some((track, pitches));
    }

    /// Releases whatever is being auditioned.
    pub(crate) fn stop_audition(&mut self) {
        if let Some((track, pitches)) = self.auditioning.take() {
            self.session.notes_off(track, &pitches);
        }
    }

    /// Whether `pitch` is the only thing sounding, which is what a note drag needs to know.
    pub(crate) fn is_auditioning(&self, pitch: u8) -> bool {
        self.auditioning
            .as_ref()
            .is_some_and(|(_, pitches)| pitches.as_slice() == [pitch])
    }

    // ---------------------------------------------------------------- chrome

    /// Sets the status line.
    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_failed = false;
    }

    /// Reports a failure on the status line, in the colour of one.
    ///
    /// Separate from [`Self::set_status`] because the status bar had no error colour at all:
    /// a command that could not be carried out was reported in the same pale grey as the sample
    /// rate beside it.
    pub(crate) fn set_failed_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_failed = true;
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
        // The system menu bar belongs to the platform, not to a view, so it is rebuilt rather
        // than re-rendered — nothing would redraw it otherwise. The in-window bar is a view and
        // needs none of this; it reads `self.language` on the next frame.
        cx.set_menus(crate::menu::menus(self.language));
        let name = self.language.endonym();
        self.set_status(messages::language_changed(self.language, name));
    }

    /// Repaints the window in another colour scheme, and remembers the choice.
    ///
    /// Everything visual reads [`Self::theme`] on the next frame, so there is nothing to
    /// invalidate. The floating plugin editor and the settings window each hold their own copy;
    /// the first is re-read every frame, and the second updates itself where it makes the change.
    pub(crate) fn apply_scheme(&mut self, id: &str) {
        self.theme = Theme::named(id);
        let appearance = Appearance {
            scheme: self.theme.scheme.to_string(),
        };
        // Best-effort, like the input settings: a preferences file that cannot be written must
        // not undo a change the user can already see.
        if let Err(error) = appearance.save() {
            log::warn!("could not save the colour scheme: {error}");
        }
        let name = crate::theme::scheme_or_default(id).name;
        self.set_status(messages::scheme_changed(self.language(), name));
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

    /// Shows or hides the left-hand library.
    pub(crate) fn toggle_library(&mut self) {
        self.panels.library_visible = !self.panels.library_visible;
    }

    /// Shows or hides the right-hand inspector.
    pub(crate) fn toggle_inspector(&mut self) {
        self.panels.inspector_visible = !self.panels.inspector_visible;
    }

    /// Applies a library resize drag.
    pub(crate) fn resize_library(&mut self, start_width: Pixels, delta: Pixels) {
        // What the library could grow into before the arrangement stops being usable. Read from
        // the arrangement's painted width rather than the window's, so the clamp and the hit
        // tests are measuring the same thing.
        let available = start_width + self.arrangement_width - PanelLayout::MIN_ARRANGEMENT_WIDTH;
        self.panels.library_width = PanelLayout::resized_library(start_width, delta, available);
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

    /// How far the library pushes everything to its right.
    ///
    /// Only used by the fallbacks below, which answer for the frame before anything has been
    /// painted; every frame after that reads the painted bounds instead.
    fn library_offset(&self) -> Pixels {
        if self.panels.library_visible {
            self.panels.library_width + Metrics::SPLITTER
        } else {
            px(0.0)
        }
    }

    /// Origin of the arrangement's clip lanes, taken from where they were last painted.
    pub(crate) fn lanes_origin(&self) -> Point<Pixels> {
        self.canvas.lanes.get().map_or_else(
            || {
                point(
                    self.library_offset() + self.panels.header_width,
                    Metrics::TRANSPORT_HEIGHT + Metrics::RULER_HEIGHT,
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
                    self.library_offset() + self.panels.header_width,
                    Metrics::TRANSPORT_HEIGHT,
                )
            },
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
