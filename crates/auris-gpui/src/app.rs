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
use std::collections::{BTreeMap, BTreeSet};
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

use crate::actions;
use crate::appearance::Appearance;
use crate::dock::{Dock, Panel, PanelLayout};
use crate::gestures::PointerGestures;
use crate::keymap::{InputSettings, Keymap};
use crate::settings_window::SettingsWindow;
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::ContextMenu;
use crate::ui::menu_bar::OpenMenu;
use crate::ui::piano_roll::RollTool;
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

/// A panel the keyboard can be in.
///
/// What a key does depends on where focus is: `t` puts the next tool in the roll's hand and does
/// nothing at all from the mixer. The variants are in tab order, which is the order they are
/// declared in — the arrangement second because it is the middle of the window whatever the panels
/// around it are doing. Tab cannot follow a panel across the window, since the order is fixed when
/// the handles are made and a panel's dock is not.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The sound library.
    Library,
    /// The track lanes and the ruler above them, in the middle of the window.
    Arrangement,
    /// The piano roll.
    PianoRoll,
    /// The mixer.
    Mixer,
    /// The inspector.
    Inspector,
}

impl Pane {
    /// Every pane, in tab order.
    pub const ALL: [Pane; 5] = [
        Pane::Library,
        Pane::Arrangement,
        Pane::PianoRoll,
        Pane::Mixer,
        Pane::Inspector,
    ];

    /// Where this pane sits in the tab order.
    ///
    /// Handed to gpui, which walks its tab stops in this order and skips the ones that were not
    /// painted — so a hidden library drops out of the cycle without anything here saying so.
    ///
    /// From one rather than zero: the window's own handle is registered too, at zero, and two
    /// stops sharing an index leaves the order between them down to which was painted first.
    pub fn tab_index(self) -> isize {
        Self::ALL.iter().position(|pane| *pane == self).unwrap_or(0) as isize + 1
    }
}

/// A focus handle for each pane.
///
/// One per pane rather than one for the window, which is what lets a binding be scoped: gpui
/// dispatches an action from whatever holds focus up through its ancestors, so a pane's key
/// context is only on that path while the pane holds focus.
pub struct PaneFocus {
    library: FocusHandle,
    arrangement: FocusHandle,
    piano_roll: FocusHandle,
    mixer: FocusHandle,
    inspector: FocusHandle,
}

impl PaneFocus {
    /// Makes a handle for every pane, in the order Tab walks them.
    ///
    /// The tab order goes on the *handle* and not on the element. `div().tab_index(n)` looks like
    /// the way to say this and is silently ignored for a handle the application owns: gpui copies
    /// those settings onto a handle it made itself, and skips the whole block when the element is
    /// tracking one that was handed to it. The tab stop map reads the handle, so every panel came
    /// out `tab_stop: false` and Tab walked a cycle with nothing in it.
    pub fn new(cx: &mut App) -> Self {
        let stop =
            |cx: &mut App, pane: Pane| cx.focus_handle().tab_index(pane.tab_index()).tab_stop(true);
        Self {
            library: stop(cx, Pane::Library),
            arrangement: stop(cx, Pane::Arrangement),
            piano_roll: stop(cx, Pane::PianoRoll),
            mixer: stop(cx, Pane::Mixer),
            inspector: stop(cx, Pane::Inspector),
        }
    }

    /// The handle for one pane.
    pub fn handle(&self, pane: Pane) -> &FocusHandle {
        match pane {
            Pane::Library => &self.library,
            Pane::Arrangement => &self.arrangement,
            Pane::PianoRoll => &self.piano_roll,
            Pane::Mixer => &self.mixer,
            Pane::Inspector => &self.inspector,
        }
    }
}

/// Which end of an audio clip a fade belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FadeEdge {
    /// The fade-in, growing from the clip's left edge.
    In,
    /// The fade-out, growing back from the clip's right edge.
    Out,
}

/// Which end of a clip a resize drag has hold of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClipEdge {
    /// The left edge: trims the front, leaving the end where it is.
    Start,
    /// The right edge: sets the length.
    End,
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
    /// Carrying a track header up or down the list.
    ///
    /// The list is reordered as the pointer moves rather than on the drop, so what follows the
    /// pointer is the arrangement itself instead of a line predicting where it will end up. The
    /// whole gesture is one transaction, which is what makes that affordable: a reorder is a
    /// structural edit, and rebuilding the render graph on every pointer move would mean
    /// instantiating every plugin in the project a hundred times across one drag.
    TrackReorder {
        /// Track in hand. Its *index* moves during the gesture, so the id is what is held.
        track: TrackId,
        /// Where the button went down, until the pointer has travelled far enough to mean it.
        ///
        /// Without it a click on a header to select a track would reorder the list whenever the
        /// pointer wobbled across a neighbour's midpoint. Cleared once the gesture is past
        /// [`crate::gestures::DRAG_THRESHOLD`].
        pressed_at: Option<Point<Pixels>>,
    },
    /// Dragging one of a clip's edges.
    ClipResize {
        /// Clip being resized.
        clip: ClipId,
        /// Which edge is in hand. The end moves the clip's length; the start trims its front and
        /// leaves the end where it is.
        edge: ClipEdge,
    },
    /// Shaping an audio clip's fade by its handle.
    ClipFade {
        /// Clip whose fade is being drawn.
        clip: ClipId,
        /// Which end of the clip the fade belongs to.
        edge: FadeEdge,
    },
    /// Dragging a point along an automation lane.
    AutomationPoint {
        /// The parameter whose lane is being shaped.
        target: ParamTarget,
        /// Where the point currently is. It moves as the drag does, because the next pointer
        /// move has to find the point where it now is rather than where it started.
        at: Ticks,
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
        /// Where the button went down, until the pointer has travelled far enough to mean it.
        ///
        /// The same guard `ClipMove` carries, for the same wobble: rows are floor-binned, so a
        /// click drifting one pixel across a row boundary transposed the whole selection — and
        /// auditioned the wrong pitch — before the hand had decided anything.
        pressed_at: Option<Point<Pixels>>,
    },
    /// Dragging a note's velocity up or down with the roll's velocity tool.
    NoteVelocity {
        /// Clip the notes live in.
        clip: ClipId,
        /// Pointer y when the drag began.
        start_y: Pixels,
        /// What every selected note was struck at when the drag began, as MIDI 1 to 127.
        ///
        /// The drag is measured against these rather than against wherever the notes are now, so
        /// a selection keeps the differences between its notes: a phrase written soft-loud-soft
        /// is still soft-loud-soft after being played harder. It also means a drag that runs off
        /// the top and comes back restores the shape, instead of leaving the whole chord
        /// flattened against the ceiling it was pushed into.
        origins: Vec<(usize, u8)>,
        /// The note that was grabbed, which the readout is drawn beside.
        grabbed: usize,
    },
    /// Dragging a note's right edge.
    NoteResize {
        /// Clip the note lives in.
        clip: ClipId,
        /// Note being resized.
        index: usize,
        /// Where the button went down, until the pointer has travelled far enough to mean it.
        ///
        /// Guards a grabbed *existing* note the way `ClipMove` guards a clip: a click wobble
        /// on an off-grid note's handle snapped its end onto the grid. `None` when the drag is
        /// drawing a brand-new note, whose end starts on the grid and should follow at once.
        pressed_at: Option<Point<Pixels>>,
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
    /// Dragging a point along a clip's pitch bend.
    BendPoint {
        /// Whose bend.
        clip: ClipId,
        /// The point being moved, by where it currently sits — a point dropped onto another
        /// replaces it, and the drag goes on holding whichever survived.
        at: Ticks,
    },
    /// Dragging a corner of the envelope graph.
    ///
    /// Absolute rather than measured from where the drag began, unlike [`Drag::Param`]: a corner
    /// follows the pointer because it is *at* the pointer, and the press only lands at all when it
    /// was already within a few pixels of one. The parameters are looked up again on every move,
    /// so nothing here can go stale under an undo or a change of plugin.
    EnvelopeHandle {
        /// Whose envelope.
        subject: crate::ui::plugin_window::PluginSubject,
        /// Which corner.
        handle: crate::ui::envelope::Handle,
        /// The parameter the whole drag is filed under, which for the decay corner is one of the
        /// two it moves.
        target: ParamTarget,
    },
    /// Dragging a section boundary along the structure lane.
    SectionLabel {
        /// The change being moved, by where it currently sits.
        at: Ticks,
        /// How far into the section the pointer took hold, kept so the boundary does not jump
        /// to the pointer on the first move.
        grab_offset: Ticks,
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
    /// Turning one of the song sheet's dials.
    ///
    /// Separate from every other dial drag because nothing is being edited: the sheet is not the
    /// document, so this writes no undo step and rebuilds no graph.
    SongDial {
        /// Which dial, and which part it belongs to when it belongs to one.
        target: crate::ui::compose_sheet::DialTarget,
        /// Where the bar was when the drag began, from 0 to 1.
        start_fraction: f32,
        /// Pointer x when the drag began.
        start_x: Pixels,
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
        /// Where the playhead sat when the drag began. The gesture turns the tempo of the
        /// stretch this falls in, held fixed so a drag during playback does not slide onto
        /// the next tempo change mid-gesture.
        at: Ticks,
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
    /// Dragging the divider between a dock and the arrangement.
    ResizeDock {
        /// Which dock is being resized.
        dock: Dock,
        /// Where the pointer was when the drag began: x for a side dock, y for the bottom one.
        start: Pixels,
        /// How large the dock was then — a width or a height, the same way round.
        start_size: Pixels,
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
            Drag::TrackReorder { .. } => Some(Edit::MoveTrack),
            Drag::ClipResize { .. } => Some(Edit::ResizeClip),
            Drag::ClipFade { .. } => Some(Edit::SetClipFade),
            Drag::AutomationPoint { target, .. } => Some(Edit::WriteAutomation(*target)),
            Drag::BendPoint { clip, .. } => Some(Edit::WriteBend(*clip)),
            Drag::NoteMove { .. } => Some(Edit::MoveNotes),
            Drag::NoteResize { .. } => Some(Edit::ResizeNote),
            Drag::NoteVelocity { .. } => Some(Edit::SetNoteVelocity),
            Drag::Param { target, .. } => Some(Edit::AdjustParameter(*target)),
            // The decay corner moves two parameters and this names one of them. The undo step is
            // one either way — the whole drag is a transaction — so this only decides the label.
            Drag::EnvelopeHandle { target, .. } => Some(Edit::AdjustParameter(*target)),
            // One undo step for the whole sweep, and the same label the right-click menu's
            // "Write It Again" uses — moving a dial is writing the part again with one thing
            // changed, and a stack full of "Adjusted parameter" would say nothing about which.
            Drag::PartDial { .. } => Some(Edit::GenerateClip),
            // A dial on the song sheet turns nothing in the document: the sheet is a question
            // about a song that has not been written yet, and nothing it does belongs on the
            // undo stack until Write is pressed.
            Drag::SongDial { .. } => None,
            Drag::Tempo { at, .. } => Some(Edit::ChangeTempo(*at)),
            // Selecting is not an edit; it changes what a later edit will act on.
            Drag::RubberBand { .. } => None,
            // Listening changes nothing at all.
            Drag::AuditionHarmony => None,
            Drag::HarmonyChord { .. } => Some(Edit::MoveChord),
            Drag::SectionLabel { .. } => Some(Edit::MoveSection),
            // How far in the view is zoomed is a property of the window, like a panel's width.
            Drag::TimeZoom { .. } => None,
            // Panel and window geometry is a property of the window, not the document: resizing
            // a panel or moving the plugin editor is not an edit and must never land on the undo
            // stack.
            Drag::ResizeDock { .. }
            | Drag::ResizeHeaders { .. }
            | Drag::MovePluginWindow { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pane_has_its_own_place_in_the_tab_order() {
        // Two stops on one index leaves the order between them down to which was painted first,
        // and the panels are not painted in the order the eye reads them.
        let mut seen = BTreeSet::new();
        for pane in Pane::ALL {
            assert!(
                seen.insert(pane.tab_index()),
                "{pane:?} shares an index with another panel"
            );
            assert!(
                pane.tab_index() > 0,
                "{pane:?} collides with the window's own handle at zero"
            );
        }
        // And the numbering follows the declaration, which is the order Tab should walk them in.
        let indices: Vec<isize> = Pane::ALL.iter().map(|pane| pane.tab_index()).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(indices, sorted);
    }

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
    fn resizing_a_dock_is_never_an_edit() {
        // It belongs in the arm that records nothing. A variant that fell through to a document
        // edit would open a session transaction and land on the undo stack, so pressing undo
        // after dragging a divider would lose the last real change instead.
        assert!(
            Drag::ResizeDock {
                dock: Dock::Left,
                start: px(0.0),
                start_size: px(0.0)
            }
            .edit()
            .is_none()
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
    /// The section strip between the ruler and the harmony.
    pub structure: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The chord strip between the structure lane and the clip lanes.
    pub harmony: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The arrangement's clip lanes.
    pub lanes: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The piano roll's note grid.
    pub roll: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The envelope graph in the open plugin window.
    pub envelope: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The pitch bend strip under the piano roll.
    pub bend: Rc<Cell<Option<Bounds<Pixels>>>>,
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
    /// Which tool the piano roll has in hand.
    ///
    /// Deliberately not a setting: a tool is a mode, and a mode the application remembers is a
    /// mode the user comes back to having forgotten. The roll opens holding the pointer every
    /// time, and the strip in its header says which one is in hand.
    pub(crate) tool: RollTool,
    pub(crate) drag: Option<Drag>,
    /// Where each panel is docked, which of them are showing, and how large each dock is.
    pub(crate) panels: PanelLayout,
    pub(crate) status: String,
    pub(crate) export: Option<ExportState>,
    /// The song sheet's dials while it is open, and nothing when it is not.
    ///
    /// State of the sheet rather than of the document: nothing here has been written until Write
    /// is pressed, which is what lets a whole song be set up and then thrown away.
    pub(crate) song_sheet: Option<crate::ui::compose_sheet::SongDials>,
    /// The progressions this installation has been taught, beside the ones it shipped with.
    ///
    /// Loaded once and held, because every chart picker lists it and reading a file per frame to
    /// draw a menu would be absurd. Written the moment one is kept.
    pub(crate) progressions: auris_session::progressions::ProgressionBook,
    /// Notes currently sounding because the user is holding a key, dragging one, or pressing a
    /// chord on the harmony lane.
    pub(crate) auditioning: Option<(TrackId, Vec<u8>)>,
    /// Keyboard focus target, so the action bindings reach this view.
    pub(crate) focus: FocusHandle,
    /// Focus handles for the panels, which is what scopes a binding to one of them.
    pub(crate) panes: PaneFocus,
    /// The panel that last held the keyboard, to give it back after a sheet closes.
    pub(crate) last_pane: Pane,
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
    /// Which tracks have an automation lane showing, and on which parameter.
    ///
    /// Presentation rather than document: what a lane *holds* is saved, but which one you happen
    /// to have open is a view of it, and the rule is that presentation stays in the frontend. A
    /// track with no entry has its lane closed, which is also what a freshly opened project gets.
    pub(crate) automation_lanes: BTreeMap<TrackId, ParamTarget>,
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
            tool: RollTool::default(),
            drag: None,
            panels: PanelLayout::load(),
            status,
            export: None,
            song_sheet: None,
            progressions: auris_session::progressions::ProgressionBook::load(),
            auditioning: None,
            focus: cx.focus_handle(),
            panes: PaneFocus::new(cx),
            last_pane: Pane::Arrangement,
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
            automation_lanes: BTreeMap::new(),
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

    // ---------------------------------------------------------------- panes

    /// Whether something on top of the window has first claim on the keyboard.
    ///
    /// A sheet, the palette, or either menu. Every binding goes out of reach while one is up: a
    /// text field needs the keystrokes to be text, and a menu being walked with the arrow keys
    /// must not also have `y` toggle the library away underneath it. Each of the four handles
    /// Escape itself, since the binding that used to close them is one of the ones now out of
    /// reach.
    pub(crate) fn keys_are_claimed(&self) -> bool {
        self.prompt.is_some()
            || self.palette.is_some()
            || self.menu.is_some()
            || self.menu_bar.is_some()
            // The song sheet is a form, and every letter typed into one of its fields has to
            // reach the field rather than the binding that letter would otherwise fire.
            || self.song_sheet.is_some()
    }

    /// The key context a pane's element should name, or `None` while a sheet is up.
    ///
    /// A prompt or the palette puts every binding out of reach so a text field can have the
    /// keystrokes. The root swaps its own context for one nothing is bound to; a pane has to drop
    /// its context altogether, because a pane that kept its name would keep matching its own
    /// bindings — it still holds focus, and `t` would put a tool in hand instead of typing a `t`.
    pub(crate) fn pane_context(&self, pane: Pane) -> Option<&'static str> {
        if self.keys_are_claimed() {
            return None;
        }
        Some(match pane {
            Pane::Library => actions::context::LIBRARY,
            Pane::Arrangement => actions::context::ARRANGEMENT,
            Pane::PianoRoll => actions::context::ROLL,
            Pane::Mixer => actions::context::MIXER,
            Pane::Inspector => actions::context::INSPECTOR,
        })
    }

    /// Whether `pane` holds the keyboard, for the ring drawn round it.
    ///
    /// A scoped binding is invisible machinery unless the window says where focus is. Without the
    /// ring, `t` working here and not there would read as the key being broken.
    pub(crate) fn pane_focused(&self, pane: Pane, window: &Window, cx: &App) -> bool {
        self.panes.handle(pane).contains_focused(window, cx)
    }

    /// Puts the keyboard in `pane`, which is what clicking one does.
    pub(crate) fn focus_pane(&mut self, pane: Pane, window: &mut Window) {
        self.last_pane = pane;
        window.focus(self.panes.handle(pane));
    }

    /// Moves the keyboard between a panel and an open sheet as one opens and closes.
    ///
    /// A text field registers itself as the window's input handler only while *its own* handle is
    /// focused — [`gpui::Window::handle_input`] checks that and quietly does nothing otherwise. The
    /// sheet's field is on the window's handle, so a sheet opened while a panel held the keyboard
    /// got no input handler at all: nothing typed reached it, and the platform, with no caret to
    /// ask about, put the IME's candidate window wherever it liked.
    ///
    /// Reconciled here rather than at each of the dozen places a sheet opens, most of which have
    /// no window to hand — and this way it is right again after any path that misses.
    pub(crate) fn reconcile_focus(&mut self, window: &mut Window) {
        let sheet = self.prompt.is_some() || self.palette.is_some();
        if sheet {
            if !self.focus.is_focused(window) {
                window.focus(&self.focus);
            }
        } else if self.focus.is_focused(window) {
            // Back where it came from, so the panel bindings work again the moment the sheet is
            // gone rather than after the next click.
            let pane = self.last_pane;
            window.focus(self.panes.handle(pane));
        }
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
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.edit().is_some() {
            // A gesture that changed nothing records no undo step and triggers no rebuild.
            self.session.end_transaction();
        }
        // A press on empty arrangement begins a sweep, and a sweep that never travelled was a
        // click — which on a timeline means "play from here". Decided on release rather than on
        // press so that reaching for a rubber band does not drag the playhead along with it, and
        // only for the lanes: the same gesture in the piano roll is over pitches, not over time.
        if let Drag::RubberBand {
            surface: BandSurface::Lanes,
            anchor,
            current,
            ..
        } = &drag
            && !crate::gestures::past_drag_threshold(*anchor, *current)
        {
            let x = anchor.x - self.lanes_origin().x;
            let tick = self.snap(self.timeline.x_to_tick(x)).max_zero();
            self.seek(tick);
        }
        // Written when the divider is let go rather than on every pointer move: a drag across the
        // window is a hundred frames, and a hundred writes of the same file.
        if matches!(drag, Drag::ResizeDock { .. } | Drag::ResizeHeaders { .. }) {
            self.remember_layout();
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
        self.audition_at(pitch, NOTE_VELOCITY);
    }

    /// Sounds one note as hard as it is written, which is what the velocity tool wants to hear.
    ///
    /// Struck once, when the note is taken hold of, and not again as the drag runs: what the
    /// gesture is *for* is the level it started from, and restriking every few pixels would turn
    /// setting a dynamic into a drum roll.
    pub(crate) fn audition_at(&mut self, pitch: u8, velocity: f32) {
        let Some(track) = self.selected_track else {
            return;
        };
        self.sound(track, vec![pitch], velocity);
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

    /// Shows `panel` when it is hidden, and hides it when its dock is showing it.
    pub(crate) fn toggle_panel(&mut self, panel: Panel) {
        self.panels.toggle(panel);
        self.remember_layout();
    }

    /// Shows `panel`, for the commands that open one as a side effect of something else.
    ///
    /// Nothing is written unless the arrangement actually changed: opening a clip in the roll is a
    /// double-click, and the roll is usually already showing.
    pub(crate) fn show_panel(&mut self, panel: Panel) {
        let before = self.panels.clone();
        self.panels.show(panel);
        if self.panels != before {
            self.remember_layout();
        }
    }

    /// Moves `panel` into `dock`, and shows it there.
    pub(crate) fn dock_panel(&mut self, panel: Panel, dock: Dock) {
        self.panels.move_to(panel, dock);
        self.remember_layout();
    }

    /// Applies a dock resize drag.
    pub(crate) fn resize_dock(&mut self, dock: Dock, start_size: Pixels, delta: Pixels) {
        // What the dock could grow into before the arrangement stops being usable. A side dock
        // reads the arrangement's *painted* width rather than the window's, so the clamp and the
        // hit tests are measuring the same thing.
        let available = match dock {
            Dock::Bottom => {
                self.viewport_height
                    - Metrics::TRANSPORT_HEIGHT
                    - Metrics::STATUS_HEIGHT
                    - PanelLayout::MIN_ARRANGEMENT
            }
            Dock::Left | Dock::Right => {
                start_size + self.arrangement_width - PanelLayout::MIN_ARRANGEMENT_WIDTH
            }
        };
        self.panels.set_size(
            dock,
            PanelLayout::resized(dock, start_size, delta, available),
        );
    }

    /// Applies a track-header column resize drag.
    pub(crate) fn resize_headers(&mut self, start_width: Pixels, delta: Pixels) {
        self.panels.header_width = PanelLayout::resized_headers(start_width, delta);
    }

    /// Writes the arrangement down, so the next launch opens the way this one was left.
    ///
    /// A failure is logged and nothing else: not being able to write a preference is no reason to
    /// refuse to move a panel.
    pub(crate) fn remember_layout(&self) {
        if let Err(error) = self.panels.save() {
            log::warn!("could not write {}: {error}", PanelLayout::path().display());
        }
    }

    // ---------------------------------------------------------------- geometry

    /// How far the left-hand dock pushes everything to its right.
    ///
    /// Only used by the fallbacks below, which answer for the frame before anything has been
    /// painted; every frame after that reads the painted bounds instead.
    fn left_dock_offset(&self) -> Pixels {
        match self.panels.showing(Dock::Left) {
            Some(_) => self.panels.size(Dock::Left) + Metrics::SPLITTER,
            None => px(0.0),
        }
    }

    /// Origin of the arrangement's clip lanes, taken from where they were last painted.
    pub(crate) fn lanes_origin(&self) -> Point<Pixels> {
        self.canvas.lanes.get().map_or_else(
            || {
                point(
                    self.left_dock_offset() + self.panels.header_width,
                    Metrics::TRANSPORT_HEIGHT + self.panels.lanes.header_height(),
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
                    self.left_dock_offset() + self.panels.header_width,
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
