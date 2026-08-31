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

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;
use auris_session::{Session, SessionOptions, WindowPlacement};
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
use crate::ui::typing_panel::TypingPanel;

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

/// The key context the window names, given what is claiming the keyboard.
///
/// Free-standing so the precedence can be tested: the window itself needs a session and a live
/// gpui window to exist, and the rule that a sheet beats the typing keyboard is worth more than
/// the two lines it takes to state.
///
/// Built by parsing, which is what naming a context as a string always did.
/// [`gpui::KeyContext::new_with_defaults`] would add an `os` entry that was never on the root
/// before, and a context this close to every binding in the application is the wrong place to
/// change what matches as a side effect of adding a state.
fn window_context(claimed: bool, playing: bool) -> gpui::KeyContext {
    let names = if claimed {
        actions::context::PROMPT.to_string()
    } else if playing {
        format!("{} {}", actions::KEY_CONTEXT, actions::context::TYPING)
    } else {
        actions::KEY_CONTEXT.to_string()
    };
    gpui::KeyContext::try_from(names.as_str()).expect("the context names are identifiers")
}

/// How often the window redraws itself while nothing is happening to it.
///
/// A named constant rather than a number in the loop, because the input meter's fall is worked
/// out from it: a peak-hold has to know how long ago it last looked.
const REPAINT_INTERVAL: Duration = Duration::from_millis(33);

/// A peak meter's reading after `elapsed`, given the loudest sample heard in that time.
///
/// A rise is instant and a fall is not. `Session::input_peak` hands over the peak of one tick and
/// then forgets it, so a bar drawn straight from it would drop to nothing in any tick that caught
/// no audio block — a held note flickering at thirty hertz. Falling at the rate the engine's own
/// meters fall is what makes the input read like the meters beside it rather than like a
/// different instrument.
fn fallen_peak(held: f32, peak: f32, elapsed: Duration) -> f32 {
    let db = MeterBank::FALL_DB_PER_SECOND * elapsed.as_secs_f32();
    peak.max(held * 10f32.powf(-db / 20.0))
}

/// What the window should do about the open project's file on disk, asked on a slow tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExternalChange {
    /// The file is as this window left it, and no offer stands.
    Nothing,
    /// Another writer's version is on disk and nothing here would be lost: take it.
    Reload,
    /// Another writer's version is on disk and this window holds unsaved work: put the
    /// choice on screen and change nothing.
    Offer,
    /// The file is ours again — a manual save took it back — while an offer still stands.
    Withdraw,
}

/// The whole external-change policy, as a function a test can hold.
///
/// The window obeys the answer in [`AurisApp::watch_disk`]; `offered` is whether the status
/// bar's Reload button is already up. The one asymmetry worth stating: a *clean* window
/// follows the disk silently, because nothing of the person's is at stake — the same call
/// the agent panel's reload policy makes.
pub(crate) fn external_change_action(modified: bool, dirty: bool, offered: bool) -> ExternalChange {
    match (modified, dirty, offered) {
        (false, _, true) => ExternalChange::Withdraw,
        (false, _, false) => ExternalChange::Nothing,
        (true, false, _) => ExternalChange::Reload,
        (true, true, false) => ExternalChange::Offer,
        (true, true, true) => ExternalChange::Nothing,
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
    /// The log.
    Log,
    /// The agent conversation.
    Agent,
}

impl Pane {
    /// Every pane, in tab order.
    pub const ALL: [Pane; 7] = [
        Pane::Library,
        Pane::Arrangement,
        Pane::PianoRoll,
        Pane::Mixer,
        Pane::Inspector,
        Pane::Log,
        Pane::Agent,
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
    log: FocusHandle,
    agent: FocusHandle,
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
            log: stop(cx, Pane::Log),
            agent: stop(cx, Pane::Agent),
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
            Pane::Log => &self.log,
            Pane::Agent => &self.agent,
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
    /// Carrying an effect up or down its chain.
    ///
    /// The chain is reordered as the pointer moves, for [`Drag::TrackReorder`]'s reason and with
    /// its cost: one transaction around the whole gesture, so the graph is rebuilt once at the
    /// drop rather than on every pointer move.
    ///
    /// No `pressed_at` guard, unlike the drags measured in pixels. This one is driven by the row
    /// the pointer has entered rather than by a coordinate, so nothing happens at all until the
    /// pointer leaves the row it started on — the row boundary is the threshold.
    EffectReorder {
        /// Whose chain, with `None` for the master's.
        track: Option<TrackId>,
        /// The slot in hand. Its *position* moves during the gesture, so the id is what is held.
        slot: EffectSlotId,
    },
    /// Dragging one of a clip's edges.
    ClipResize {
        /// Clip being resized.
        clip: ClipId,
        /// Which edge is in hand. The end moves the clip's length; the start trims its front and
        /// leaves the end where it is.
        edge: ClipEdge,
    },
    /// Dragging the far end of a clip's repeats.
    ///
    /// Separate from [`Drag::ClipResize`] because it changes a different thing: the resize edge
    /// says what the clip *is*, and this one says how many times it is heard. Dragged back over
    /// the clip's own end it stops the repeats, which is the same gesture run the other way
    /// rather than a second thing to know about.
    ClipLoop {
        /// Clip whose repeats are being stretched.
        clip: ClipId,
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
    /// Carrying a panel's scrollbar along its track.
    PanelScroll {
        /// Which panel's, which is also which way the drag is measured.
        panel: crate::ui::scrollbars::ScrollPanel,
        /// The pointer's coordinate along that axis when the drag began.
        start: Pixels,
        /// The scroll offset then, which every move is measured against rather than against
        /// wherever the content is now — the same reason a fader remembers where it was grabbed.
        start_offset: f32,
    },
    /// Turning a parameter.
    Param {
        /// What is being changed.
        target: ParamTarget,
        /// Value where the drag was last anchored.
        start_value: f32,
        /// Pointer x where the drag was last anchored.
        start_x: Pixels,
        /// Whether the fine modifier was held at the anchor.
        ///
        /// Kept so a drag can notice the modifier being pressed or released half way through.
        /// The travel is measured from the anchor rather than block by block, so rescaling it
        /// after the fact would snap the value back to a fifth of where the hand had already
        /// taken it; the answer is to move the anchor to the pointer instead, which leaves the
        /// value exactly where it was and changes only what happens next.
        fine: bool,
    },
    /// Dragging a point along one of a clip's curves.
    CurvePoint {
        /// Whose curve.
        clip: ClipId,
        /// Which of the two.
        which: ClipCurve,
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
    /// Dragging one band's node around the equalizer's curve.
    ///
    /// Absolute rather than measured from where the drag began, for [`Drag::EnvelopeHandle`]'s
    /// reason: the node is *at* the pointer, and the press only lands at all when it was already
    /// within a few pixels of one.
    EqNode {
        /// Whose equalizer.
        subject: crate::ui::plugin_window::PluginSubject,
        /// Which band, counting from the lowest.
        band: usize,
        /// The parameter the whole drag is filed under, which is the band's frequency — the one
        /// number every shape's node moves.
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
    /// Turning one of a clip's performance dials.
    ///
    /// Separate from [`Drag::PartDial`] because the two write different things: a part dial
    /// rewrites a generated clip's notes, and this one edits the transform stack the notes are
    /// performed through — on any MIDI clip, played or written.
    PerformDial {
        /// Clip whose stack is being edited.
        clip: ClipId,
        /// Which dial.
        dial: crate::ui::performance::PerformDial,
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
    /// Moving the drawn typing keyboard by its title bar.
    MoveTypingPanel {
        /// Distance from the panel's own origin to the point that was grabbed.
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
    /// Dragging the bottom edge of a track's header to make its lane taller or shorter.
    ///
    /// The one resize gesture in the window that is an *edit*: a lane's height is stored in the
    /// document beside its name and its colour, so it is undoable and it makes the project dirty.
    /// A dock's width is a property of the window and is not.
    ResizeTrack {
        /// Track whose lane is being resized.
        track: TrackId,
        /// Pointer y when the drag began.
        start_y: Pixels,
        /// The lane's height then. Absolute rather than accumulated, so a pointer that ran past
        /// the floor and came back finds the lane where it left it.
        start_height: f32,
    },
}

impl Drag {
    /// The edit this gesture records, or `None` when it changes no document state.
    fn edit(&self) -> Option<Edit> {
        match self {
            Drag::Playhead => None,
            // Where a panel is looking is not something to undo.
            Drag::PanelScroll { .. } => None,
            Drag::LoopRegion { .. } => Some(Edit::SetLoopRegion),
            Drag::ClipMove { .. } => Some(Edit::MoveClip),
            Drag::TrackReorder { .. } => Some(Edit::MoveTrack),
            Drag::EffectReorder { .. } => Some(Edit::ReorderEffects),
            Drag::ClipResize { .. } => Some(Edit::ResizeClip),
            Drag::ClipLoop { .. } => Some(Edit::LoopClip),
            Drag::ClipFade { .. } => Some(Edit::SetClipFade),
            Drag::AutomationPoint { target, .. } => Some(Edit::WriteAutomation(*target)),
            Drag::CurvePoint { clip, which, .. } => Some(Edit::write_curve(*which, *clip)),
            Drag::NoteMove { .. } => Some(Edit::MoveNotes),
            Drag::NoteResize { .. } => Some(Edit::ResizeNote),
            Drag::NoteVelocity { .. } => Some(Edit::SetNoteVelocity),
            Drag::Param { target, .. } => Some(Edit::AdjustParameter(*target)),
            // The decay corner moves two parameters and this names one of them. The undo step is
            // one either way — the whole drag is a transaction — so this only decides the label.
            Drag::EnvelopeHandle { target, .. } => Some(Edit::AdjustParameter(*target)),
            // The same arrangement: a node moves a frequency and a gain, and this names the one
            // it always moves.
            Drag::EqNode { target, .. } => Some(Edit::AdjustParameter(*target)),
            // One undo step for the whole sweep, and the same label the right-click menu's
            // "Write It Again" uses — moving a dial is writing the part again with one thing
            // changed, and a stack full of "Adjusted parameter" would say nothing about which.
            Drag::PartDial { .. } => Some(Edit::GenerateClip),
            // One step for the sweep, named for the clip whose performance it shapes.
            Drag::PerformDial { clip, .. } => Some(Edit::SetClipTransforms(*clip)),
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
            | Drag::MovePluginWindow { .. }
            | Drag::MoveTypingPanel { .. } => None,
            // The exception among the resizes, and the reason is where the number is kept: a
            // lane's height is a field of the track, so it travels with the project and belongs
            // on the stack with everything else about that track.
            Drag::ResizeTrack { .. } => Some(Edit::SetTrackHeight),
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
    fn the_disk_is_followed_while_clean_and_offered_while_dirty() {
        use ExternalChange::*;
        // Nothing happened, nothing happens.
        assert_eq!(external_change_action(false, false, false), Nothing);
        assert_eq!(external_change_action(false, true, false), Nothing);
        // A clean window follows the disk silently — nothing of the person's is at stake.
        assert_eq!(external_change_action(true, false, false), Reload);
        // Unsaved work makes it a choice, put up once and then left standing.
        assert_eq!(external_change_action(true, true, false), Offer);
        assert_eq!(external_change_action(true, true, true), Nothing);
        // A manual save takes the file back, and the stale offer comes down with it.
        assert_eq!(external_change_action(false, false, true), Withdraw);
        assert_eq!(external_change_action(false, true, true), Withdraw);
        // The offer stood, the person undid their way back to clean: the disk still wins.
        assert_eq!(external_change_action(true, false, true), Reload);
    }

    #[test]
    fn the_input_meter_rises_at_once_and_falls_at_the_rate_the_others_do() {
        // A louder peak is taken whole: a meter that eased up to a transient would show it at
        // the wrong height, which for the one number somebody is setting a level by is the whole
        // failure.
        assert_eq!(fallen_peak(0.1, 0.9, REPAINT_INTERVAL), 0.9);

        // And a tick that heard nothing falls rather than dropping out. One second of silence is
        // one fall of the engine's own rate, which is 20 dB — a tenth of the amplitude.
        let mut held = 1.0;
        let ticks = (1.0 / REPAINT_INTERVAL.as_secs_f32()).round() as usize;
        for _ in 0..ticks {
            held = fallen_peak(held, 0.0, REPAINT_INTERVAL);
        }
        // Slack because the fall is a product of `ticks` roundings of an f32, and because
        // `REPAINT_INTERVAL` does not divide a second evenly.
        assert!(
            (held - 0.1).abs() < 0.01,
            "a second of silence should fall 20 dB, reached {held}"
        );

        // Silence is reached rather than approached: nothing above it, and never below zero.
        assert_eq!(fallen_peak(0.0, 0.0, REPAINT_INTERVAL), 0.0);
    }

    #[test]
    fn the_window_context_says_which_bindings_are_out_of_reach() {
        let plain = window_context(false, false);
        assert!(plain.contains(actions::KEY_CONTEXT));
        assert!(!plain.contains(actions::context::TYPING));

        // Playing keeps every binding except the ones bound outside the typing context, which is
        // what leaves ⌘S and the space bar working with both hands on the letters.
        let playing = window_context(false, true);
        assert!(playing.contains(actions::KEY_CONTEXT));
        assert!(playing.contains(actions::context::TYPING));

        // A sheet beats the keyboard, and takes the window's own context with it: a rename field
        // needs `a` to be an `a` rather than either a C or the inspector.
        for playing in [false, true] {
            let claimed = window_context(true, playing);
            assert!(claimed.contains(actions::context::PROMPT));
            assert!(!claimed.contains(actions::KEY_CONTEXT));
            assert!(!claimed.contains(actions::context::TYPING));
        }
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

    fn export_state() -> ExportState {
        ExportState {
            path: std::path::PathBuf::from("Song.wav"),
            progress: Arc::new(AtomicU32::new(0.4f32.to_bits())),
            result: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn a_stopped_export_is_neither_a_success_nor_a_failure() {
        let mut export = export_state();
        assert_eq!(export.outcome(), ExportOutcome::Running);

        // The press, before the render has noticed it. Still running, and the overlay says so.
        export.cancel();
        assert_eq!(export.outcome(), ExportOutcome::Running);
        assert!(export.cancelling());

        // And the render comes back. A cancellation is reported as an `Ok` message — it did
        // what was asked — so the flag is the only thing that can tell this from a written file.
        export.result = Some(Ok("stopped".to_string()));
        assert_eq!(export.outcome(), ExportOutcome::Stopped);
        assert!(!export.cancelling(), "there is nothing left to wait for");
    }

    #[test]
    fn an_export_that_finished_or_broke_says_which() {
        let mut written = export_state();
        written.result = Some(Ok("wrote Song.wav".to_string()));
        assert_eq!(written.outcome(), ExportOutcome::Wrote);

        let mut broken = export_state();
        broken.result = Some(Err("the disk is full".to_string()));
        assert_eq!(broken.outcome(), ExportOutcome::Failed);
        // Even one that was also cancelled: whatever the flag says, a render that came back an
        // error broke, and the bar is red rather than quiet.
        broken.cancel();
        assert_eq!(broken.outcome(), ExportOutcome::Failed);
    }
}

/// Where an export has got to.
///
/// Three endings rather than two. A render that was stopped on purpose is neither a success nor
/// a failure, and the overlay used to have no way to say so: filling the bar to the end would
/// claim a file that was never written, and painting it red would send somebody looking for a
/// fault they caused themselves. It stops where it got to, in the quiet colour.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    /// Still rendering.
    Running,
    /// The file was written.
    Wrote,
    /// Stopped part way, at somebody's asking. No file.
    Stopped,
    /// It broke.
    Failed,
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
    /// Raised when the render should stop; it is read between blocks.
    ///
    /// Shared with the background thread rather than sent to it. A channel would want a receiver
    /// somewhere in the render loop, and what the loop actually asks is a question with a
    /// one-word answer.
    pub cancel: Arc<AtomicBool>,
}

impl ExportState {
    /// Completion from 0.0 to 1.0.
    pub fn fraction(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    /// Asks the render to stop at the end of its current block.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether stopping has been asked for and has not happened yet.
    ///
    /// What the button reads while the current block finishes. Without it the overlay looks
    /// exactly as it did before the press, for as long as a block takes.
    pub fn cancelling(&self) -> bool {
        self.result.is_none() && self.cancel.load(Ordering::Relaxed)
    }

    /// Where the export has got to.
    ///
    /// The cancel flag rather than the result decides between [`ExportOutcome::Wrote`] and
    /// [`ExportOutcome::Stopped`], because a stopped render comes back as an `Ok` message: it
    /// did what was asked of it, and the message says so.
    pub fn outcome(&self) -> ExportOutcome {
        match (&self.result, self.cancel.load(Ordering::Relaxed)) {
            (None, _) => ExportOutcome::Running,
            (Some(Err(_)), _) => ExportOutcome::Failed,
            (Some(Ok(_)), true) => ExportOutcome::Stopped,
            (Some(Ok(_)), false) => ExportOutcome::Wrote,
        }
    }
}

/// One background sing keeping a take abreast of its notes.
///
/// Started by the repaint timer once the document has held still, never by a person — the
/// person's render is [`ExportState`] and the overlay. This one stays out of the way: no
/// overlay, no stop button, just the header badge saying the voice is at work. Cancelled
/// between chunks whenever the text it is rendering stops being the text on the screen.
pub struct AutoSing {
    /// The track being rendered.
    pub track: TrackId,
    /// The fingerprint of the text under render, to notice an edit outdating it midway.
    pub fingerprint: u64,
    /// Raised to stop the render between chunks.
    pub cancel: Arc<AtomicBool>,
    /// The revision the staleness check last ran under, so it runs once per edit rather
    /// than once per frame.
    pub checked: u64,
}

/// One cell per curve strip the roll has drawn, shared with the closures that paint them.
type CurveBounds = Rc<RefCell<HashMap<ClipCurve, Rc<Cell<Option<Bounds<Pixels>>>>>>>;

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
    /// Where each panel's scrollbar was drawn, keyed by
    /// [`ScrollPanel::index`](crate::ui::scrollbars::ScrollPanel::index).
    ///
    /// Reached through [`CanvasBounds::scrollbar`] rather than directly, so that the one place
    /// the array is indexed is the one place a panel is turned into a slot.
    scrollbars: [Rc<Cell<Option<Bounds<Pixels>>>>; crate::ui::scrollbars::ScrollPanel::COUNT],
    /// The envelope graph in the open plugin window.
    pub envelope: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The equalizer's graph in the open plugin window, above the strip of frequency numbers.
    pub analyser: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The curve strips under the piano roll, one cell per strip that has been drawn.
    ///
    /// A map rather than a field each, because which strips exist is now the user's: the bend, and
    /// a lane for any controller they have opened. Cells are kept once made — there are at most a
    /// hundred and twenty-nine of them, they are a machine word each, and a lane closed and
    /// reopened wants the rectangle it had.
    curves: CurveBounds,
}

impl CanvasBounds {
    /// Where a panel's scrollbar was drawn.
    pub(crate) fn scrollbar(
        &self,
        panel: crate::ui::scrollbars::ScrollPanel,
    ) -> Rc<Cell<Option<Bounds<Pixels>>>> {
        Rc::clone(&self.scrollbars[panel.index()])
    }

    /// Where one of the roll's curve strips was painted.
    ///
    /// Asking about a strip that has never been drawn hands back an empty cell rather than
    /// nothing: the caller is either about to paint into it or about to find it empty, and both of
    /// those are the same code as for a strip that is merely off screen.
    pub fn curve(&self, which: ClipCurve) -> Rc<Cell<Option<Bounds<Pixels>>>> {
        Rc::clone(self.curves.borrow_mut().entry(which).or_default())
    }
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
    /// Monitor dropouts already reported, so the frame loop says something once per new gap
    /// rather than thirty times a second for as long as the count stands.
    pub(crate) monitor_gaps: u64,
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
    /// Each singer track's take freshness, cached under the document revision it was read at.
    ///
    /// The repaint timer redraws thirty times a second and the freshness question renders a
    /// track's frames to answer; cached against [`Session::revision`](auris_session::session::Session::revision),
    /// it is asked once per edit instead. See [`AurisApp::singer_take_badge`].
    pub(crate) sung_badges: std::collections::HashMap<TrackId, auris_session::SingerTakeState>,
    /// The revision [`Self::sung_badges`] was computed under.
    pub(crate) sung_badges_revision: u64,
    /// The background re-render keeping voiced singer tracks abreast of their notes.
    ///
    /// Editing the score is the ask; this is the performer noticing. The repaint timer
    /// watches the revision, waits for [`crate::ui::commands`]'s debounce, and renders
    /// with no overlay — the header badge is the on-screen sign the CPU is spent.
    pub(crate) auto_sing: Option<AutoSing>,
    /// The revision the auto-render debounce last saw, and when it saw it arrive.
    pub(crate) auto_sing_seen: (u64, std::time::Instant),
    /// The revision whose auto-render was refused, so a standing refusal — an unsaved
    /// project, an empty track — waits for the next edit instead of retrying every frame.
    pub(crate) auto_sing_refused: Option<u64>,
    /// The refusal last said out loud, so the same excuse is not repeated per edit.
    pub(crate) auto_sing_excuse: Option<String>,
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
    /// How far the mixer's strips have been scrolled sideways.
    ///
    /// gpui keeps this for a scrolling container by itself; it is held here as well so that the
    /// scrollbar under the strips can read where the wheel left them and write where a drag puts
    /// them. Without the handle the two would be separate scroll positions that happen to look
    /// alike until somebody used both.
    pub(crate) mixer_scroll: gpui::ScrollHandle,
    /// The same, for the browser's list of instruments and effects.
    pub(crate) library_scroll: gpui::ScrollHandle,
    /// The same, for the inspector's column of settings.
    pub(crate) inspector_scroll: gpui::ScrollHandle,
    /// The same, for the log's lines.
    pub(crate) log_scroll: gpui::ScrollHandle,
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
    /// The `.clap` files found on this machine, scanned once and kept.
    ///
    /// `None` until the plugins section is first drawn: walking three directory trees is not a
    /// thing to do on every frame, and not a thing to do at all for somebody who never opens it.
    pub(crate) clap_files: Option<Vec<std::path::PathBuf>>,
    /// What each opened `.clap` file turned out to hold.
    ///
    /// Filled the first time a file's branch is opened, which is also the first time its binary
    /// is loaded. Kept afterwards so that shutting and reopening the branch is free — the file
    /// stays open in the session either way.
    pub(crate) clap_contents:
        std::collections::HashMap<std::path::PathBuf, Vec<auris_session::ClapPluginInfo>>,
    /// The title the operating system was last told, so it is only told again on a change.
    pub(crate) titled: String,
    /// Whether the export destination dialog is open.
    ///
    /// [`Self::export`] is not set until a path comes back, so this is what stops a second
    /// Export while the picker is still up.
    pub(crate) choosing_export: bool,
    /// What the library is being filtered by.
    ///
    /// Always present rather than opened: a browser with twenty rows of plugins and a hundred
    /// and twenty-eight sounds in one font is a list nobody scrolls twice, and a search that has
    /// to be summoned first is one people forget is there.
    pub(crate) library_search: crate::ui::text_field::TextField,
    /// Whether that field is taking the keyboard.
    pub(crate) library_search_focused: bool,
    /// The agent panel: its transcript, its fields, and the child process behind them.
    pub(crate) agent_chat: crate::ui::agent_chat::AgentChat,
    /// Whether [`Self::status`] is reporting a failure, so it can be shown as one.
    pub(crate) status_failed: bool,
    /// The open project's path, when it has changed on disk under a window holding unsaved
    /// work — the standing offer behind the status bar's Reload button.
    pub(crate) external_change: Option<PathBuf>,
    /// When the file on disk was last compared against the session, for throttling the
    /// check to well below the repaint rate. `None` before the first look.
    pub(crate) last_disk_watch: Option<std::time::Instant>,
    /// What the input meter is currently reading, as a linear peak.
    ///
    /// Held here rather than asked for while drawing, because `Session::input_peak` forgets what
    /// it hands back: it is a peak-hold, and a second reader would take half the peaks off the
    /// first. It is read exactly once per repaint tick — see [`Self::sample_input_level`] — and
    /// the meter draws this.
    pub(crate) input_level: f32,
    /// Whether the input has touched full scale since the indicator was last cleared.
    pub(crate) input_clipped: bool,
    /// The same reading, one per input channel, for the meter beside an armed track.
    ///
    /// A second buffer rather than a second look at the first: the device-wide peak and the
    /// per-channel peaks are separate accumulators that each reset on read, so both are taken
    /// once per tick and held here for whoever draws them.
    pub(crate) input_levels: Vec<f32>,
    /// Which of those channels have touched full scale since the indicators were last put out.
    pub(crate) input_clips: Vec<bool>,
    /// This tick's per-channel peaks, kept only so that taking them allocates once.
    pub(crate) input_peaks: Vec<f32>,
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
    /// Where the drawn keyboard has been dragged to. See [`crate::ui::typing_panel`].
    pub(crate) typing_panel: TypingPanel,
    /// The key the pointer is holding down on the drawn keyboard, if any.
    pub(crate) clicked_key: Option<&'static str>,

    _repaint: Task<()>,
}

impl Focusable for AurisApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How the window opens its session, out of what the settings remember.
///
/// `cfg!` rather than `#[cfg]` so both arms compile everywhere, and a free function so the test
/// window opens the same session the real one does. What a test cannot have is the hardware:
/// `cargo test` runs several windows at once in one process, and each of them claiming the
/// machine's output device — or reading two hundred megabytes of shipped SoundFont, or writing an
/// autosave over somebody's project — is three ways for a suite to be about the machine it ran on
/// rather than about the interface.
fn session_options(settings: &Settings) -> SessionOptions {
    let live = !cfg!(test);
    SessionOptions {
        audio_preferences: settings.audio.clone(),
        autosave: settings.autosave && live,
        audio: live,
        gpu: live,
        shipped_fonts: live,
        ..SessionOptions::default()
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

        let mut session =
            Session::new(session_options(&settings)).expect("a session opens even without audio");
        // The same empty document File → New gives, rather than a separate idea of what a fresh
        // start looks like. Launching used to leave a two-bar arpeggio and a bass line lying
        // around, which was useful while there was nothing else to hear and is now just
        // somebody else's music to delete before starting.
        session.new_project();

        // The dictionary the settings name, loaded once for the session's lifetime. A folder
        // that fails to load is logged and left in the settings — deleting the setting over a
        // network share that was asleep would make a transient failure permanent.
        if let Some(folder) = &settings.japanese_dictionary
            && let Err(error) = session.set_japanese_dictionary(Some(folder))
        {
            log::warn!("the Japanese dictionary did not load: {error}");
        }

        let status = audio_line(&session.audio_status(), language);
        log::info!("{status}");

        // Repaint on a timer rather than per audio block: the playhead and meters live in
        // atomics written by the audio thread, and 30 fps is plenty to read them at.
        let repaint = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REPAINT_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        this.session.poll();
                        // Here rather than while drawing, and here rather than in `poll`: the
                        // input peak is destroyed by being read, so it has to be read exactly
                        // once, on a tick of a known length, by the one thing that shows it.
                        this.sample_input_level();
                        // Separate from `poll` on purpose: that is housekeeping and this writes
                        // to somebody's disk. A success says nothing — the title bar's unsaved
                        // mark going out is the whole of the feedback, and a status line
                        // announcing a save every half minute is one that never holds anything
                        // else. A failure is worth the interruption every time.
                        if let Some(Err(error)) = this.session.autosave() {
                            let line = this.failure(Key::CmdSave, &error);
                            this.set_failed_status(line);
                        }
                        // Beside the autosave on purpose: the two are halves of one story —
                        // this notices another writer at the file, and the autosave policy
                        // refuses to write over that writer while it stands unresolved.
                        this.watch_disk(cx);
                        // Also here rather than in a command, because a monitor breaking up
                        // happens *between* commands: without this the only evidence is a noise
                        // the person playing is left to interpret.
                        this.report_monitor_gaps();
                        // The punch-out is a position the playhead crosses rather than a thing
                        // anybody does, so this is the only place that could notice it.
                        this.finish_punch();
                        // The agent's wire is another thread writing and this one reading, so
                        // it is drained where everything with that shape is.
                        this.drain_agent(cx);
                        // Edits to a voiced singer track re-render its take without being
                        // asked; the debounce and the one-at-a-time rule live in the poll.
                        this.poll_auto_sing(cx);
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
                .note_clips()
                .and_then(|clips| clips.first())
                .map(|clip| clip.id)
        });

        Self {
            session,
            theme: Appearance::load().theme(),
            timeline: TimelineView::default(),
            pitch: PitchView::default(),
            selected_track,
            monitor_gaps: 0,
            selected_clip,
            selected_clips: selected_clip.into_iter().collect(),
            selected_notes: BTreeSet::new(),
            tool: RollTool::default(),
            drag: None,
            panels: PanelLayout::load(),
            status,
            export: None,
            sung_badges: std::collections::HashMap::new(),
            sung_badges_revision: 0,
            auto_sing: None,
            auto_sing_seen: (0, std::time::Instant::now()),
            auto_sing_refused: None,
            auto_sing_excuse: None,
            song_sheet: None,
            progressions: auris_session::progressions::ProgressionBook::load(),
            auditioning: None,
            focus: cx.focus_handle(),
            panes: PaneFocus::new(cx),
            last_pane: Pane::Arrangement,
            viewport_height: px(900.0),
            arrangement_width: px(900.0),
            canvas: CanvasBounds::default(),
            mixer_scroll: gpui::ScrollHandle::new(),
            library_scroll: gpui::ScrollHandle::new(),
            inspector_scroll: gpui::ScrollHandle::new(),
            log_scroll: gpui::ScrollHandle::new(),
            menu: None,
            menu_bar: None,
            prompt: None,
            palette: None,
            plugin_window: None,
            library: crate::ui::library::LibraryTree::default(),
            clap_files: None,
            clap_contents: std::collections::HashMap::new(),
            titled: String::new(),
            choosing_export: false,
            status_failed: false,
            external_change: None,
            last_disk_watch: None,
            input_level: 0.0,
            input_clipped: false,
            input_levels: Vec::new(),
            input_clips: Vec::new(),
            input_peaks: Vec::new(),
            library_search: crate::ui::text_field::TextField::new(String::new()),
            library_search_focused: false,
            agent_chat: crate::ui::agent_chat::AgentChat::default(),
            automation_lanes: BTreeMap::new(),
            lane_scroll: px(0.0),
            settings,
            language,
            pointer: input.pointer,
            keymap,
            settings_window: None,
            typing_panel: TypingPanel::default(),
            clicked_key: None,
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
        self.taking_text_input()
            || self.menu.is_some()
            || self.menu_bar.is_some()
            // The song sheet is a form, and every letter typed into one of its fields has to
            // reach the field rather than the binding that letter would otherwise fire.
            || self.song_sheet.is_some()
    }

    /// Whether text is being typed into something in this window.
    ///
    /// Narrower than [`Self::keys_are_claimed`], and the two must not be written out separately.
    /// An open menu claims the keyboard and is not typed into; a field is both, and the second
    /// half is a *different* mechanism — the platform types through
    /// [`gpui::Window::handle_input`], which only works while the handle it was registered
    /// against is the focused one. Something added to the claim list and forgotten here is a
    /// field that goes grey, swallows every binding, and receives nothing: that is exactly what
    /// the library's search box did on the day it was written.
    ///
    /// All three of these are on the *window's* handle, which is why one question serves them.
    /// See [`Self::reconcile_focus`].
    pub(crate) fn taking_text_input(&self) -> bool {
        self.prompt.is_some()
            || self.palette.is_some()
            || self.library_search_focused
            || self.agent_chat.typing()
    }

    /// The key context the window itself should name.
    ///
    /// Three states, and each of them takes bindings away rather than adding any:
    ///
    /// * While something is being typed into, the application's own bindings must not fire — `i`
    ///   has to type an `i`, not toggle the inspector. Swapping the root's context for one nothing
    ///   is bound to disables the window's bindings in a single move; the panes drop their
    ///   contexts at the same time, in [`Self::pane_context`], which is what stops a pane-scoped
    ///   binding firing from the pane that still holds focus behind the sheet.
    /// * While the computer keyboard is being played, the window keeps its context and gains
    ///   [`actions::context::TYPING`], which every binding on a key the keyboard plays was bound
    ///   *outside* of. Those stop matching and every other binding — `space`, `escape`, ⌘S —
    ///   carries on. See [`actions::reachable_from`] for why it has to be arranged this way round.
    /// * Otherwise, the window's own context and nothing else.
    ///
    /// A sheet wins over the keyboard: the notes stop reaching the instrument at the same moment
    /// the letters start reaching a text field, which is the only order that lets somebody rename
    /// a track without playing a chord into it.
    pub(crate) fn window_context(&self) -> gpui::KeyContext {
        window_context(self.keys_are_claimed(), self.playing_the_keyboard())
    }

    /// Whether the typing keyboard is switched on *and* has an instrument to play.
    ///
    /// The two are asked together everywhere, because a keyboard with nothing to sound must not
    /// hold the alphabet: the last instrument track can be deleted while the mode is on, and
    /// letters that neither played a note nor ran their command would read as a seized-up
    /// application. This way they go back to being commands until there is something to play, and
    /// the mode is still on when there is.
    pub(crate) fn playing_the_keyboard(&self) -> bool {
        self.session.musical_typing() && self.session.audition_track(self.selected_track).is_some()
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
            Pane::Log => actions::context::LOG,
            Pane::Agent => actions::context::AGENT,
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
        // [`Self::taking_text_input`] rather than a list written out again here. The library's
        // search box is in a panel and covers nothing, but as far as *this* question goes it is
        // a sheet: something is being typed into, and the handle it is typed through has to be
        // the focused one or nothing reaches it at all.
        if self.taking_text_input() {
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

    /// Whether a gesture is in progress.
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Begins a gesture. Every edit it makes becomes one undo step and one graph rebuild.
    pub(crate) fn begin_drag(&mut self, drag: Drag) {
        if let Some(edit) = drag.edit() {
            self.session.begin_transaction(edit);
        }
        self.drag = Some(drag);
    }

    /// Lets go of a gesture whose first edit refused.
    ///
    /// For the create-and-drag gestures, which open their transaction *before* the write so
    /// that placing a point and shaping it undo as one step: when the write then refuses,
    /// this closes the transaction it opened — recording nothing, since nothing changed —
    /// rather than leaving it to swallow whatever edit comes next.
    pub(crate) fn abandon_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.edit().is_some() {
            self.session.end_transaction();
        }
    }

    /// Ends any gesture in progress.
    pub(crate) fn end_drag(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        // A clip dropped over its neighbour is a join, and the join is shaped before the gesture
        // closes so that it is part of the same undo step: the fade exists because the clip
        // landed there, and undoing the move without it would leave a fade over nothing.
        //
        // Only where neither meeting edge already carries one — see
        // `Session::crossfade_landings`, which is where that rule lives and is tested.
        if let Drag::ClipMove { origins, .. } = &drag {
            let moved: Vec<ClipId> = origins.iter().map(|(clip, _)| *clip).collect();
            let joins = self.session.crossfade_landings(&moved);
            if joins > 0 {
                self.set_status(messages::crossfaded_landings(self.language, joins));
            }
        }
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

    /// The keystroke shown beside a command, written the way this platform writes it.
    ///
    /// Empty for a command with no key and for an id this build does not have — the same answer,
    /// because both mean "nothing to print here" and neither is worth a row of its own. `id` is
    /// one of [`crate::actions::BINDABLE`]'s.
    ///
    /// Asked at the moment it is shown rather than baked into a table, which is the whole reason
    /// a tooltip can carry one: every key here is the user's to move, so the only telling that
    /// stays true is the one that reads the keymap as it draws.
    pub(crate) fn keystroke_for(&self, id: &str) -> String {
        crate::actions::bindable(id)
            .map(|command| crate::actions::menu_keystroke(&self.keymap.display(command)))
            .unwrap_or_default()
    }

    /// Takes this tick's input peak and folds it into the reading the meter draws.
    ///
    /// Called from the repaint loop and nowhere else. See [`Self::input_level`].
    pub(crate) fn sample_input_level(&mut self) {
        let peak = self.session.input_peak();
        // The input's clip latch is kept here rather than in the engine's meter bank, because
        // the input is not in the graph the bank measures: it is the device, upstream of
        // everything. The peak this reads is a held maximum rather than a falling one, so a
        // sample that touched full scale between two ticks is still in it.
        self.input_clipped |= peak >= 1.0;
        self.input_level = fallen_peak(self.input_level, peak, REPAINT_INTERVAL);

        // And the same for each channel, which is what a meter beside an armed track draws. The
        // buffer is the session's to fill and ours to keep, so it is resized by the device rather
        // than by us — and the readings beside it have to follow, or a device swapped for a wider
        // one would leave every track reading the last one's channels.
        self.session.take_input_peaks(&mut self.input_peaks);
        self.input_levels.resize(self.input_peaks.len(), 0.0);
        self.input_clips.resize(self.input_peaks.len(), false);
        for (channel, peak) in self.input_peaks.iter().enumerate() {
            self.input_clips[channel] |= *peak >= 1.0;
            self.input_levels[channel] =
                fallen_peak(self.input_levels[channel], *peak, REPAINT_INTERVAL);
        }
    }

    /// What the meter beside `track` reads, while it is armed and a device is open.
    ///
    /// `None` for a track that is not armed and whenever nothing is listening, which is what
    /// decides whether the meter is drawn at all: a bar reading silence beside every audio track
    /// in the project would say the interface was dead.
    pub(crate) fn input_level_for(&self, track: TrackId) -> Option<(f32, bool)> {
        if !self.session.input_is_open() {
            return None;
        }
        let input = self.session.track_arm(track)?;
        let clipped = self
            .input_clips
            .iter()
            .skip(input.first)
            .take(input.count)
            .any(|clipped| *clipped);
        Some((input_level_of(&self.input_levels, input), clipped))
    }

    /// Puts out every clip indicator, on the meters and on the input.
    ///
    /// Only ever by asking, which is what makes a latch a latch: an indicator that cleared
    /// itself on the next quiet block would be a reading, and the whole point is to still be lit
    /// when somebody looks up from the keyboard.
    pub(crate) fn clear_clipping(&mut self) {
        self.session.meters().clear_clipped();
        self.input_clipped = false;
        self.input_clips.fill(false);
    }

    /// Whether anything anywhere is showing a clip.
    pub(crate) fn anything_clipped(&self) -> bool {
        self.input_clipped || self.session.meters().anything_clipped()
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

    /// Notices another writer at the open project — the MCP door, a sync service, anything
    /// with the file — and obeys [`external_change_action`]: reload where nothing would be
    /// lost, offer a button where something would, withdraw the offer once the file is ours
    /// again (a manual save takes it back).
    pub(crate) fn watch_disk(&mut self, cx: &mut gpui::Context<Self>) {
        const DISK_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
        if self
            .last_disk_watch
            .is_some_and(|at| at.elapsed() < DISK_WATCH_INTERVAL)
        {
            return;
        }
        self.last_disk_watch = Some(std::time::Instant::now());
        match external_change_action(
            self.session.externally_modified(),
            self.session.is_dirty(),
            self.external_change.is_some(),
        ) {
            ExternalChange::Nothing => {}
            ExternalChange::Withdraw => {
                self.external_change = None;
                self.set_status(String::new());
                cx.notify();
            }
            ExternalChange::Reload => {
                self.external_change = None;
                if let Some(path) = self.session.path().map(std::path::Path::to_path_buf) {
                    self.open_project_at(path, cx);
                }
            }
            ExternalChange::Offer => {
                if let Some(path) = self.session.path().map(std::path::Path::to_path_buf) {
                    self.external_change = Some(path);
                    let line = self.t(Key::ExternalChangeConflict).to_string();
                    self.set_failed_status(line);
                    cx.notify();
                }
            }
        }
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

    /// Turns autosaving on or off and remembers the choice.
    ///
    /// Best-effort on the file, like every other preference: a settings file that cannot be
    /// written must not undo a change the user can already see working.
    pub(crate) fn apply_autosave(&mut self, enabled: bool) {
        self.session.set_autosave(enabled);
        self.settings.autosave = enabled;
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }
    }

    /// Points the session's Japanese text frontend at a dictionary folder, and remembers it.
    ///
    /// The error comes back in the user's language for the settings window to show beside the
    /// control — a wrong path should fail at the screen that names it, not under a lyric typed
    /// an hour later — and nothing changes when it does.
    pub(crate) fn apply_japanese_dictionary(
        &mut self,
        folder: Option<std::path::PathBuf>,
    ) -> Result<(), String> {
        self.session
            .set_japanese_dictionary(folder.as_deref())
            .map_err(|error| crate::i18n::error_text(&error, self.language()))?;
        self.settings.japanese_dictionary = folder;
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }
        Ok(())
    }

    /// Takes note of where the window is, without writing anything.
    ///
    /// Every frame, because there is no event for "the window has settled": a drag of the title
    /// bar is a hundred small moves and gpui reports the bounds rather than the gesture. Reading
    /// them is free; the file is written once, when the window is put away.
    pub(crate) fn remember_window(&mut self, window: &gpui::Window) {
        let bounds = match window.window_bounds() {
            gpui::WindowBounds::Windowed(bounds) => bounds,
            // The restore size in both cases, which is what a maximised window should come back
            // to when it is unmaximised. Its *current* rectangle is the whole screen, and saving
            // that would leave a window that fills the display and cannot be made smaller by
            // unmaximising it.
            gpui::WindowBounds::Maximized(bounds) | gpui::WindowBounds::Fullscreen(bounds) => {
                bounds
            }
        };
        self.settings.window = Some(WindowPlacement {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized: window.is_maximized(),
        });
    }

    /// Writes the settings file, so where the window is now is where it opens next time.
    ///
    /// Called on the way out rather than on every move: a settings file rewritten a hundred times
    /// while a window is dragged is a hundred writes to answer a question nobody asked yet.
    /// Best-effort, like every other preference.
    pub(crate) fn save_window_placement(&self) {
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }
    }

    /// Installs and saves how a bounce is written.
    ///
    /// Nothing in the running session reads it — an export takes a copy when it starts — so
    /// unlike the audio preferences this cannot fail and has nothing to restart.
    pub(crate) fn apply_export(&mut self, export: ExportPreferences) {
        self.settings.export = export;
        if let Err(error) = self.settings.save() {
            log::warn!("could not save settings: {error}");
        }
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
        let devices = crate::settings_window::AudioDevices {
            output: self.session.output_devices(),
            input: self.session.input_devices(),
        };
        let audio = self.session.audio_preferences().clone();
        let live = self.session.audio_status();
        let keymap = self.keymap.clone();
        let language = self.settings.language;
        let pointer = self.pointer;
        let autosave = self.session.autosave_enabled();
        let dictionary = self.settings.japanese_dictionary.clone();
        let export = self.settings.export;

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
                        app, theme, devices, audio, live, keymap, language, pointer, autosave,
                        dictionary, export, cx,
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

    /// Fixed rows stacked above and below the central row: the transport, the status bar, and —
    /// everywhere the application draws its own menu bar — that row too.
    ///
    /// One answer for both `resize_dock` and `drawn_dock_sizes`, because they have to subtract
    /// the same chrome the layout actually stacks: the menu bar was once left out of both, and
    /// on Windows a bottom dock dragged to its limit overflowed the window by exactly the 26
    /// pixels the two forgot — a mistake macOS, where development happens, never showed.
    pub(crate) fn chrome_height() -> Pixels {
        let menu = match Self::wants_menu_bar() {
            true => crate::ui::menu_bar::HEIGHT,
            false => gpui::px(0.0),
        };
        Metrics::TRANSPORT_HEIGHT + Metrics::STATUS_HEIGHT + menu
    }

    /// Applies a dock resize drag.
    pub(crate) fn resize_dock(&mut self, dock: Dock, start_size: Pixels, delta: Pixels) {
        // What the dock could grow into before the arrangement stops being usable. A side dock
        // reads the arrangement's *painted* width rather than the window's, so the clamp and the
        // hit tests are measuring the same thing.
        let available = match dock {
            Dock::Bottom => {
                self.viewport_height - Self::chrome_height() - PanelLayout::MIN_ARRANGEMENT
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

    /// Applies a lane-height drag.
    ///
    /// Nothing is clamped on the way in. `Session::set_track_height` holds the floor and the
    /// ceiling and is tested against both, and a second copy of the same two numbers up here
    /// would be a second place for them to drift — the header draws whatever the document ended
    /// up with, so the two cannot disagree.
    pub(crate) fn resize_track(&mut self, track: TrackId, start_height: f32, delta: Pixels) {
        let _ = self
            .session
            .set_track_height(track, start_height + f32::from(delta));
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

/// One gesture, one undo step — checked by making the gesture rather than by calling what it
/// calls.
///
/// Every drag opens a transaction on the way down and closes it on the way up, and a drag is a
/// hundred pointer moves that each edit the document. The number of steps a gesture leaves behind
/// is invisible on screen: the only way to find out it had become a hundred was to press ⌘Z a
/// hundred times.
#[cfg(test)]
mod window_tests {
    use gpui::TestAppContext;

    use super::AurisApp;
    use auris_session::prelude::{ClipId, TICKS_PER_QUARTER, Ticks};

    use crate::actions;
    use crate::harness::{CLIP_LENGTH, drag, lane_point, with_a_clip};

    /// Halfway along the fixture's clip.
    const HALF_CLIP: Ticks = Ticks(CLIP_LENGTH.0 / 2);

    /// Four beats, the distance these drags travel.
    const FOUR_BEATS: Ticks = Ticks(4 * TICKS_PER_QUARTER);

    /// Where the clip starts now.
    fn start(app: &gpui::Entity<AurisApp>, cx: &gpui::TestAppContext, clip: ClipId) -> Ticks {
        app.read_with(cx, |this, _| {
            this.session
                .midi_clip(clip)
                .expect("the clip is still there")
                .start
        })
    }

    #[gpui::test]
    fn one_drag_takes_one_undo_to_put_back(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let from = lane_point(&app, cx, track, HALF_CLIP);
        let to = lane_point(&app, cx, track, HALF_CLIP + FOUR_BEATS);

        drag(cx, from, to);
        assert_eq!(start(&app, cx, clip), FOUR_BEATS, "the drag landed");

        cx.dispatch_action(actions::Undo);

        assert_eq!(
            start(&app, cx, clip),
            Ticks::ZERO,
            "one Undo, not one per pointer move"
        );
    }

    /// And the way back out again, since a step that cannot be redone is half a step.
    #[gpui::test]
    fn redo_puts_the_drag_back_where_it_landed(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let from = lane_point(&app, cx, track, HALF_CLIP);
        let to = lane_point(&app, cx, track, HALF_CLIP + FOUR_BEATS);

        drag(cx, from, to);
        cx.dispatch_action(actions::Undo);
        cx.dispatch_action(actions::Redo);

        assert_eq!(start(&app, cx, clip), FOUR_BEATS);
    }

    /// A gesture that changed nothing must leave nothing behind, or every stray click on the
    /// arrangement costs a step of the history that anybody using it has to walk back through.
    #[gpui::test]
    fn a_gesture_that_moved_nothing_records_no_step(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        // Something to undo *to*, so a spurious step would show up as this not coming back.
        app.update(cx, |this, _| {
            this.session
                .move_clip(clip, FOUR_BEATS)
                .expect("the clip may be moved");
        });
        crate::harness::paint(&app, cx);

        let at = lane_point(&app, cx, track, FOUR_BEATS + HALF_CLIP);
        drag(cx, at, at);

        cx.dispatch_action(actions::Undo);
        assert_eq!(
            start(&app, cx, clip),
            Ticks::ZERO,
            "the one Undo reached the move, so the press left no step of its own"
        );
    }
}
