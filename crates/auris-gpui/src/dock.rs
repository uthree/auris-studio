//! Which edge each panel is docked to, whether it is showing, and where that is kept.
//!
//! Zed's arrangement, because it is the one that survives contact with a window somebody else
//! laid out: three docks — a column down each side and a strip along the bottom — and every panel
//! belongs to one of them. A dock shows **one** panel at a time, so moving the mixer next to the
//! library does not halve the height of both; the status bar carries an icon for each panel in
//! each dock, and pressing one is how a dock is asked to show something else.
//!
//! The arrangement is not a panel. It is the middle of the window, and the docks are around it.
//!
//! Its own preferences file, for the reason [`Appearance`](crate::appearance::Appearance) has one:
//! nothing at or below `auris-session` has a window to arrange, so there is no second reader to
//! be told twice.

use std::collections::BTreeMap;
use std::path::PathBuf;

use auris_i18n::Key;
use auris_session::prelude::ClipCurve;
use gpui::{Pixels, px};
use serde::{Deserialize, Serialize};

use crate::app::Pane;
use crate::theme::Metrics;
use crate::ui::icons::Icon;

/// An edge of the window a panel can be docked to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dock {
    /// The column down the left-hand side.
    Left,
    /// The strip along the bottom, between the two side docks.
    Bottom,
    /// The column down the right-hand side.
    Right,
}

impl Dock {
    /// Every dock, in the order the window lays them out.
    pub const ALL: [Dock; 3] = [Dock::Left, Dock::Bottom, Dock::Right];

    /// Whether this is a column down one side, rather than the strip along the bottom.
    ///
    /// The difference decides what a dock's size *means* — a width or a height — and which way
    /// its divider runs, so it is asked here rather than matched on in a dozen places.
    pub fn is_side(self) -> bool {
        !matches!(self, Dock::Bottom)
    }

    /// The menu row that moves a panel here.
    pub fn label(self) -> Key {
        match self {
            Dock::Left => Key::DockLeft,
            Dock::Bottom => Key::DockBottom,
            Dock::Right => Key::DockRight,
        }
    }

    /// Where this dock's size sits in [`PanelLayout`].
    fn index(self) -> usize {
        Self::ALL.iter().position(|dock| *dock == self).unwrap_or(0)
    }
}

/// A panel that can be moved from one dock to another.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    /// The sound library.
    Library,
    /// The piano roll, showing the selected MIDI clip.
    PianoRoll,
    /// The mixer, showing every track's strip side by side.
    Mixer,
    /// The inspector for the selected track.
    Inspector,
    /// What the application has logged.
    Log,
}

impl Panel {
    /// Every panel, in the order a dock stacks their icons.
    pub const ALL: [Panel; 5] = [
        Panel::Library,
        Panel::PianoRoll,
        Panel::Mixer,
        Panel::Inspector,
        Panel::Log,
    ];

    /// Where the keyboard is while this panel holds it.
    pub fn pane(self) -> Pane {
        match self {
            Panel::Library => Pane::Library,
            Panel::PianoRoll => Pane::PianoRoll,
            Panel::Mixer => Pane::Mixer,
            Panel::Inspector => Pane::Inspector,
            Panel::Log => Pane::Log,
        }
    }

    /// What the panel is called.
    pub fn label(self) -> Key {
        match self {
            Panel::Library => Key::Library,
            Panel::PianoRoll => Key::PianoRoll,
            Panel::Mixer => Key::Mixer,
            Panel::Inspector => Key::Inspector,
            Panel::Log => Key::LogPanel,
        }
    }

    /// The bindable command that shows and hides it, as an id in [`crate::actions::BINDABLE`].
    ///
    /// So the status bar's switch can say which key also works. The switch is a mark and nothing
    /// else — the panel it opens is a thing learned by clicking all five — and the key is exactly
    /// what somebody who has just learned it would rather not have to click for again.
    pub fn command(self) -> &'static str {
        match self {
            Panel::Library => "view.library",
            Panel::PianoRoll => "view.piano_roll",
            Panel::Mixer => "view.mixer",
            Panel::Inspector => "view.inspector",
            Panel::Log => "view.log",
        }
    }

    /// The mark that stands for it in the status bar.
    ///
    /// A picture of what is inside rather than of which edge it is on: a panel that can be moved
    /// cannot be named by its position, and the icon has to keep meaning the same thing after it
    /// has crossed the window.
    pub fn icon(self) -> Icon {
        match self {
            Panel::Library => Icon::Library,
            Panel::PianoRoll => Icon::Notes,
            Panel::Mixer => Icon::Faders,
            Panel::Inspector => Icon::Sliders,
            Panel::Log => Icon::Log,
        }
    }

    /// Where this panel's placement sits in [`PanelLayout`].
    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|panel| *panel == self)
            .unwrap_or(0)
    }
}

/// The strips between the ruler and the clips, and whether each is drawn.
///
/// One question asked three times: how much of the window the *song* gets, against how much its
/// structure and its harmony get. A piece with neither written is paying fifty pixels a row for
/// two empty strips; a piece being arranged around a chorus wants both of them and the tempo
/// marks as well.
///
/// A property of the window rather than of the document, like a panel's width — hiding a lane
/// hides the drawing of something, never the thing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineLanes {
    /// The strip of section names: イントロ, Aメロ, サビ.
    pub structure: bool,
    /// The key and the chords.
    pub harmony: bool,
    /// Tempo changes, marked along the ruler's lower edge.
    ///
    /// Not a lane of its own — it is drawn *on* the ruler — so turning it off changes no height.
    /// It is here because it is the same question: a strip of numbers over a song that never
    /// changes tempo says nothing, and one over a song that does is the only place it is said.
    pub tempo: bool,
}

impl Default for TimelineLanes {
    /// All three. A lane nobody has hidden is a lane that is drawn: somebody who does not know
    /// these can be turned off should still see what the document holds.
    fn default() -> Self {
        Self {
            structure: true,
            harmony: true,
            tempo: true,
        }
    }
}

impl TimelineLanes {
    /// Everything above the clip lanes on the right, and the strip that matches it on the left.
    ///
    /// The track headers line up with their lanes only because the left column reserves exactly
    /// what the right column spends above them. Adding a lane on one side and not the other slides
    /// every header out of register with the track it names — a bug that reads as a paint glitch
    /// and that no test can see, because nothing here is ever rendered in one. Both sides call
    /// this, so they cannot disagree.
    pub fn header_height(self) -> Pixels {
        let mut height = Metrics::RULER_HEIGHT;
        if self.structure {
            height += Metrics::STRUCTURE_LANE_HEIGHT;
        }
        if self.harmony {
            height += Metrics::HARMONY_LANE_HEIGHT;
        }
        height
    }
}

/// Where every panel sits, whether it is showing, and how large each dock is drawn.
///
/// Kept together so the layout, the hit tests and the dividers all read one source rather than
/// each deriving the geometry from constants and drifting apart.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// Which dock each panel is in, in [`Panel::ALL`] order.
    docks: [Dock; 5],
    /// Whether each panel is showing, in [`Panel::ALL`] order.
    ///
    /// At most one is true per dock; [`Self::show`] is what keeps that so.
    open: [bool; 5],
    /// How large each dock is drawn, in [`Dock::ALL`] order: a width for the sides, a height for
    /// the bottom.
    sizes: [Pixels; 3],
    /// Width of the track header column.
    pub header_width: Pixels,
    /// Which strips above the clip lanes are drawn.
    pub lanes: TimelineLanes,
    /// Whether the piano roll draws its pitch bend strip.
    ///
    /// Off until asked for. A bend is a thing a few parts do and most do not, and a strip that is
    /// always there takes seventy pixels off the notes of every clip that never bends.
    pub bend_lane: bool,
    /// Which controllers the piano roll draws a strip for, in number order.
    ///
    /// A set rather than a flag each, on the same terms as the bend: a lane is opened when it is
    /// wanted and takes its seventy pixels only then. Empty to begin with — even the modulation
    /// wheel, which was a flag here before a clip could hold anything else, and which is one menu
    /// item away like all the rest.
    controller_lanes: Vec<u8>,
}

impl PanelLayout {
    /// Whether one of the roll's curve strips is drawn.
    pub fn curve_lane(&self, which: ClipCurve) -> bool {
        match which {
            ClipCurve::Bend => self.bend_lane,
            ClipCurve::Controller(number) => self.controller_lanes.contains(&number),
        }
    }

    /// Shows or hides one of them.
    pub fn set_curve_lane(&mut self, which: ClipCurve, shown: bool) {
        match which {
            ClipCurve::Bend => self.bend_lane = shown,
            ClipCurve::Controller(number) => match shown {
                // In number order, because that is the order the strips are stacked in and the
                // order the menu lists them in — a lane that appeared wherever it was asked for
                // would move the others every time one was opened.
                true => {
                    if let Err(at) = self.controller_lanes.binary_search(&number) {
                        self.controller_lanes.insert(at, number);
                    }
                }
                false => self.controller_lanes.retain(|open| *open != number),
            },
        }
    }

    /// Every strip the roll draws, the bend first and the controllers in number order.
    pub fn curve_lanes(&self) -> Vec<ClipCurve> {
        let bend = self.bend_lane.then_some(ClipCurve::Bend);
        bend.into_iter()
            .chain(
                self.controller_lanes
                    .iter()
                    .map(|n| ClipCurve::Controller(*n)),
            )
            .collect()
    }
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            // Library on the left, inspector on the right, the two editors sharing the strip along
            // the bottom: Logic's arrangement, and the one this window was laid out at.
            // The log shares the bottom with the two editors: it is read in the same posture as a
            // mixer — glanced at and dismissed — and what it wants is width rather than height.
            docks: [
                Dock::Left,
                Dock::Bottom,
                Dock::Bottom,
                Dock::Right,
                Dock::Bottom,
            ],
            // The mixer starts closed because the piano roll has the dock they share. The library
            // starts open, unlike Logic, which defaults its Library closed: there, a channel
            // strip's instrument slot is a second way to load one; here the library is the only
            // one, so starting closed would leave a new project with no visible way to choose an
            // instrument at all.
            // The log starts closed and stays closed until somebody wants it: it is the panel
            // that is interesting on the day something goes wrong and noise on every other one.
            open: [true, true, false, true, false],
            sizes: [
                Metrics::LEFT_DOCK_WIDTH,
                Metrics::BOTTOM_DOCK_HEIGHT,
                Metrics::RIGHT_DOCK_WIDTH,
            ],
            header_width: Metrics::TRACK_HEADER_WIDTH,
            lanes: TimelineLanes::default(),
            bend_lane: false,
            controller_lanes: Vec::new(),
        }
    }
}

impl PanelLayout {
    /// Narrowest a side dock may be dragged.
    pub const MIN_SIDE: Pixels = px(180.0);
    /// Widest a side dock may be dragged.
    pub const MAX_SIDE: Pixels = px(520.0);
    /// Shortest the bottom dock may be dragged.
    pub const MIN_BOTTOM: Pixels = px(120.0);
    /// Narrowest the track header column may be dragged.
    pub const MIN_HEADERS: Pixels = px(140.0);
    /// Widest the track header column may be dragged.
    pub const MAX_HEADERS: Pixels = px(360.0);

    /// Height the arrangement must keep, so a dragged dock cannot swallow it.
    pub const MIN_ARRANGEMENT: Pixels = px(140.0);
    /// Width the arrangement must keep, for the same reason horizontally.
    pub const MIN_ARRANGEMENT_WIDTH: Pixels = px(240.0);

    /// Which dock `panel` is in.
    pub fn dock(&self, panel: Panel) -> Dock {
        self.docks[panel.index()]
    }

    /// Whether `panel` is showing.
    pub fn is_open(&self, panel: Panel) -> bool {
        self.open[panel.index()]
    }

    /// The panel `dock` is showing, if it is showing one.
    pub fn showing(&self, dock: Dock) -> Option<Panel> {
        Panel::ALL
            .into_iter()
            .find(|panel| self.dock(*panel) == dock && self.is_open(*panel))
    }

    /// Every panel that lives in `dock`, open or not, in [`Panel::ALL`] order.
    ///
    /// What the status bar draws an icon for: a panel with no icon anywhere is a panel with no way
    /// back, and a dock that is shut has to keep saying what is in it.
    pub fn panels_in(&self, dock: Dock) -> impl Iterator<Item = Panel> + use<'_> {
        Panel::ALL
            .into_iter()
            .filter(move |panel| self.dock(*panel) == dock)
    }

    /// How large `dock` is drawn: a width for the sides, a height for the bottom.
    pub fn size(&self, dock: Dock) -> Pixels {
        self.sizes[dock.index()]
    }

    /// Sets how large `dock` is drawn.
    pub fn set_size(&mut self, dock: Dock, size: Pixels) {
        self.sizes[dock.index()] = size;
    }

    /// Shows `panel`, in place of whatever its dock was showing.
    ///
    /// One at a time is the whole reason a dock can hold several panels without any of them
    /// becoming unusable — two stacked in a 240-pixel column would be two half-panels.
    pub fn show(&mut self, panel: Panel) {
        let dock = self.dock(panel);
        for other in Panel::ALL {
            if self.dock(other) == dock {
                self.open[other.index()] = other == panel;
            }
        }
    }

    /// Hides `panel`, closing its dock if it was the one showing.
    pub fn hide(&mut self, panel: Panel) {
        self.open[panel.index()] = false;
    }

    /// Shows `panel` when it is hidden, and hides it when it is the one its dock is showing.
    pub fn toggle(&mut self, panel: Panel) {
        if self.is_open(panel) {
            self.hide(panel);
        } else {
            self.show(panel);
        }
    }

    /// Moves `panel` into `dock`, and shows it there.
    ///
    /// Shown rather than merely moved: choosing where a panel should live and then seeing nothing
    /// happen reads as the choice not having taken.
    pub fn move_to(&mut self, panel: Panel, dock: Dock) {
        self.docks[panel.index()] = dock;
        self.show(panel);
    }

    /// A dock's size after dragging its divider by `delta`.
    ///
    /// Which way the delta counts is a property of the dock. The left dock's divider sits on its
    /// *right* edge, so dragging right widens it and the delta is added; the right dock's and the
    /// bottom dock's sit on the near side, and theirs is subtracted. The sign is the whole
    /// difference between them, and getting it wrong makes a dock run away from the pointer.
    ///
    /// `available` is what is left of the window once the arrangement's own minimum is accounted
    /// for. It is clamped up to the dock's minimum, so a window already too small for the
    /// arrangement still yields a usable panel rather than a negative one.
    pub fn resized(dock: Dock, start_size: Pixels, delta: Pixels, available: Pixels) -> Pixels {
        let wanted = match dock {
            Dock::Left => start_size + delta,
            Dock::Bottom | Dock::Right => start_size - delta,
        };
        let floor = Self::smallest(dock);
        let ceiling = match dock.is_side() {
            true => Self::MAX_SIDE.min(available),
            false => available,
        };
        wanted.max(floor).min(ceiling.max(floor))
    }

    /// Track-header column width after dragging its divider by `delta`.
    pub fn resized_headers(start_width: Pixels, delta: Pixels) -> Pixels {
        (start_width + delta)
            .max(Self::MIN_HEADERS)
            .min(Self::MAX_HEADERS)
    }

    /// Smallest `dock` may be drawn at.
    fn smallest(dock: Dock) -> Pixels {
        match dock.is_side() {
            true => Self::MIN_SIDE,
            false => Self::MIN_BOTTOM,
        }
    }

    /// `size` brought inside the range a drag could have produced for `dock`.
    ///
    /// For the file, which is hand-editable text: a side dock two thousand pixels wide is a window
    /// with no arrangement in it.
    fn clamped(dock: Dock, size: Pixels) -> Pixels {
        let ceiling = match dock.is_side() {
            true => Self::MAX_SIDE,
            false => px(f32::MAX),
        };
        size.max(Self::smallest(dock)).min(ceiling)
    }

    /// Where the file lives.
    pub fn path() -> PathBuf {
        auris_session::config_dir().join("layout.json")
    }

    /// Loads the file, falling back to the defaults.
    ///
    /// A missing file is a first run. A malformed one is logged and then also falls back: the file
    /// outlives the build that wrote it, and opening with the panels somewhere else beats not
    /// opening.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<StoredLayout>(&text) {
            Ok(stored) => Self::from(stored),
            Err(error) => {
                log::warn!("ignoring malformed {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the file, creating the configuration directory if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&StoredLayout::from(self))
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(path, text)
    }
}

/// One panel's placement, as `layout.json` writes it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct StoredPanel {
    /// Which dock it lives in.
    dock: Dock,
    /// Whether it is the one that dock is showing.
    open: bool,
}

/// Everything in `layout.json`.
///
/// Maps rather than fixed fields, so a file written by a build with one panel fewer still loads,
/// and so the numbers stay plain: `Pixels` is a length the layout works in, not a thing to ask a
/// person to type.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredLayout {
    /// Where each panel sits.
    panels: BTreeMap<Panel, StoredPanel>,
    /// How large each dock is drawn.
    sizes: BTreeMap<Dock, f32>,
    /// Width of the track header column.
    header_width: Option<f32>,
    /// Which strips above the clip lanes are drawn.
    lanes: TimelineLanes,
    /// Whether the piano roll draws its pitch bend strip.
    bend_lane: bool,
    /// Which controllers it draws a strip for.
    controller_lanes: Vec<u8>,
}

impl From<&PanelLayout> for StoredLayout {
    fn from(layout: &PanelLayout) -> Self {
        Self {
            panels: Panel::ALL
                .into_iter()
                .map(|panel| {
                    (
                        panel,
                        StoredPanel {
                            dock: layout.dock(panel),
                            open: layout.is_open(panel),
                        },
                    )
                })
                .collect(),
            sizes: Dock::ALL
                .into_iter()
                .map(|dock| (dock, f32::from(layout.size(dock))))
                .collect(),
            header_width: Some(f32::from(layout.header_width)),
            lanes: layout.lanes,
            bend_lane: layout.bend_lane,
            controller_lanes: layout.controller_lanes.clone(),
        }
    }
}

impl From<StoredLayout> for PanelLayout {
    fn from(stored: StoredLayout) -> Self {
        let mut layout = Self::default();
        for (panel, placement) in stored.panels {
            layout.docks[panel.index()] = placement.dock;
            layout.open[panel.index()] = placement.open;
        }
        for (dock, size) in stored.sizes {
            layout.set_size(dock, PanelLayout::clamped(dock, px(size)));
        }
        if let Some(width) = stored.header_width {
            layout.header_width = Self::resized_headers(px(width), px(0.0));
        }
        layout.lanes = stored.lanes;
        layout.bend_lane = stored.bend_lane;
        // Through the setter, so a file naming the same controller twice, or naming them out of
        // order, still produces the stack the roll draws rather than a duplicate strip.
        for number in stored.controller_lanes {
            layout.set_curve_lane(ClipCurve::Controller(number), true);
        }
        // A dock shows one panel at a time, which nothing in the file is obliged to respect. The
        // first one named wins, and the rest are shut: two open panels in one dock would draw over
        // each other, and only one of them could be hidden again from the status bar.
        for dock in Dock::ALL {
            let crowd: Vec<Panel> = layout
                .panels_in(dock)
                .filter(|panel| layout.is_open(*panel))
                .collect();
            for extra in crowd.into_iter().skip(1) {
                layout.hide(extra);
            }
        }
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_panel_switch_names_a_command_that_exists() {
        // The id is the only link between a switch and the key that also works it, and a typo in
        // one is invisible: `keystroke_for` answers an unknown id with an empty string, which is
        // exactly what a command with no key looks like. The tooltip would simply stop mentioning
        // the key and nothing would say why.
        for panel in Panel::ALL {
            let id = panel.command();
            let found = crate::actions::bindable(id);
            assert!(found.is_some(), "{panel:?} names no command (`{id}`)");
            // And it is the command that shows *that* panel, not merely some command: the two
            // tables are written by hand and the labels are what would catch a swap.
            assert_eq!(
                found.map(|command| command.label),
                Some(match panel {
                    Panel::Library => Key::CmdShowLibrary,
                    Panel::PianoRoll => Key::CmdShowPianoRoll,
                    Panel::Mixer => Key::CmdShowMixer,
                    Panel::Inspector => Key::CmdShowInspector,
                    Panel::Log => Key::CmdShowLog,
                }),
                "{panel:?} is wired to another panel's command"
            );
        }
    }

    #[test]
    fn the_roll_stacks_its_strips_in_one_order_however_they_were_opened() {
        // The strips are stacked in the order this returns and the menu lists them in the same
        // one. A lane that appeared wherever it was asked for would move every strip below it
        // each time one was opened, under a pointer that was drawing on one of them.
        let mut layout = PanelLayout::default();
        assert!(layout.curve_lanes().is_empty(), "a fresh roll shows none");

        layout.set_curve_lane(ClipCurve::Controller(64), true);
        layout.set_curve_lane(ClipCurve::MODULATION, true);
        layout.set_curve_lane(ClipCurve::Bend, true);
        layout.set_curve_lane(ClipCurve::Controller(11), true);
        assert_eq!(
            layout.curve_lanes(),
            vec![
                ClipCurve::Bend,
                ClipCurve::Controller(1),
                ClipCurve::Controller(11),
                ClipCurve::Controller(64),
            ]
        );

        // Opening one that is already open changes nothing: two strips on one controller would
        // draw the same curve twice and share an element id.
        layout.set_curve_lane(ClipCurve::Controller(11), true);
        assert_eq!(layout.curve_lanes().len(), 4);

        layout.set_curve_lane(ClipCurve::Controller(11), false);
        assert!(!layout.curve_lane(ClipCurve::Controller(11)));
        assert!(
            layout.curve_lane(ClipCurve::Controller(64)),
            "closing one closed another"
        );

        // And the whole stack survives the file it is remembered in.
        let stored = StoredLayout::from(&layout);
        let reopened = PanelLayout::from(stored);
        assert_eq!(reopened.curve_lanes(), layout.curve_lanes());
    }

    #[test]
    fn every_panel_and_dock_indexes_itself() {
        // The placements are arrays indexed by these, so an index that did not agree with `ALL`
        // would silently give one panel another's dock.
        for panel in Panel::ALL {
            assert_eq!(Panel::ALL[panel.index()], panel);
        }
        for dock in Dock::ALL {
            assert_eq!(Dock::ALL[dock.index()], dock);
        }
    }

    #[test]
    fn every_panel_has_a_switch_in_exactly_one_dock() {
        // The status bar draws one switch per panel per dock, and a switch is the only way back to
        // a panel that has been put away. A panel counted twice would have two switches disagreeing
        // about whether it is showing; a panel counted nowhere could never be asked for again.
        let mut layout = PanelLayout::default();
        layout.move_to(Panel::Mixer, Dock::Right);
        layout.hide(Panel::Library);

        let mut switched: Vec<Panel> = Dock::ALL
            .into_iter()
            .flat_map(|dock| layout.panels_in(dock).collect::<Vec<_>>())
            .collect();
        switched.sort();
        assert_eq!(switched, Panel::ALL.to_vec());
    }

    #[test]
    fn the_default_arrangement_is_the_one_the_window_was_laid_out_at() {
        let layout = PanelLayout::default();
        assert_eq!(layout.showing(Dock::Left), Some(Panel::Library));
        assert_eq!(layout.showing(Dock::Bottom), Some(Panel::PianoRoll));
        assert_eq!(layout.showing(Dock::Right), Some(Panel::Inspector));
        // In the bottom dock, but not the one it is showing.
        assert!(!layout.is_open(Panel::Mixer));
        assert_eq!(layout.dock(Panel::Mixer), Dock::Bottom);
    }

    #[test]
    fn a_dock_shows_one_panel_at_a_time() {
        let mut layout = PanelLayout::default();
        layout.show(Panel::Mixer);
        assert_eq!(layout.showing(Dock::Bottom), Some(Panel::Mixer));
        assert!(
            !layout.is_open(Panel::PianoRoll),
            "the roll shared the dock and must have given it up"
        );
        // And the other docks are none of its business.
        assert_eq!(layout.showing(Dock::Left), Some(Panel::Library));
    }

    #[test]
    fn toggling_the_panel_a_dock_is_showing_shuts_the_dock() {
        let mut layout = PanelLayout::default();
        layout.toggle(Panel::PianoRoll);
        assert_eq!(layout.showing(Dock::Bottom), None);
        // The mixer and the log are still down there, waiting to be asked for.
        assert_eq!(layout.panels_in(Dock::Bottom).count(), 3);
        layout.toggle(Panel::PianoRoll);
        assert_eq!(layout.showing(Dock::Bottom), Some(Panel::PianoRoll));
    }

    #[test]
    fn moving_a_panel_shows_it_where_it_lands() {
        let mut layout = PanelLayout::default();
        layout.move_to(Panel::Mixer, Dock::Right);
        assert_eq!(layout.showing(Dock::Right), Some(Panel::Mixer));
        assert!(
            !layout.is_open(Panel::Inspector),
            "the inspector was showing in the dock the mixer moved into"
        );
        // The dock it came from carries on with what it was showing.
        assert_eq!(layout.showing(Dock::Bottom), Some(Panel::PianoRoll));
    }

    #[test]
    fn a_side_dock_follows_the_pointer_and_stops_at_its_limits() {
        // The left dock's divider is on its right edge, so the delta is *added* — the opposite
        // sign to the right dock's, whose case sits directly below so the difference is written
        // down rather than remembered.
        let roomy = px(9_000.0);
        assert_eq!(
            PanelLayout::resized(Dock::Left, px(240.0), px(40.0), roomy),
            px(280.0)
        );
        assert_eq!(
            PanelLayout::resized(Dock::Right, px(240.0), px(40.0), roomy),
            px(200.0)
        );
        assert_eq!(
            PanelLayout::resized(Dock::Right, px(240.0), px(-40.0), roomy),
            px(280.0)
        );
        assert_eq!(
            PanelLayout::resized(Dock::Left, px(240.0), px(9_000.0), roomy),
            PanelLayout::MAX_SIDE
        );
        assert_eq!(
            PanelLayout::resized(Dock::Left, px(240.0), px(-9_000.0), roomy),
            PanelLayout::MIN_SIDE
        );
    }

    #[test]
    fn a_dock_cannot_squeeze_the_arrangement_off_the_window() {
        // A drag with plenty of travel left still stops where the arrangement's minimum begins.
        assert_eq!(
            PanelLayout::resized(Dock::Left, px(240.0), px(9_000.0), px(300.0)),
            px(300.0)
        );
        assert_eq!(
            PanelLayout::resized(Dock::Bottom, px(280.0), px(-9_000.0), px(400.0)),
            px(400.0)
        );
        // `available` goes negative once the window cannot hold the arrangement's minimum, and a
        // window too small for both still has to yield a usable panel rather than a negative one.
        assert_eq!(
            PanelLayout::resized(Dock::Left, px(240.0), px(9_000.0), px(-500.0)),
            PanelLayout::MIN_SIDE
        );
        assert_eq!(
            PanelLayout::resized(Dock::Bottom, px(280.0), px(0.0), px(-50.0)),
            PanelLayout::MIN_BOTTOM
        );
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

    #[test]
    fn the_two_columns_reserve_the_same_height_above_the_lanes() {
        // The track headers line up with their lanes only because the left column reserves
        // exactly what the ruler and whichever strips are showing spend on the right. Nothing
        // renders in a test, so this arithmetic is the only place the misalignment can be caught
        // before a person sees every header sitting off the track it names.
        let all = TimelineLanes::default();
        assert_eq!(
            all.header_height(),
            Metrics::RULER_HEIGHT + Metrics::STRUCTURE_LANE_HEIGHT + Metrics::HARMONY_LANE_HEIGHT
        );

        let bare = TimelineLanes {
            structure: false,
            harmony: false,
            tempo: false,
        };
        assert_eq!(
            bare.header_height(),
            Metrics::RULER_HEIGHT,
            "the ruler is not a lane and never goes"
        );

        // The tempo marks are drawn on the ruler, so hiding them buys no height at all.
        let no_tempo = TimelineLanes {
            tempo: false,
            ..all
        };
        assert_eq!(no_tempo.header_height(), all.header_height());

        // And each of the two that *are* lanes gives back exactly its own row.
        let no_structure = TimelineLanes {
            structure: false,
            ..all
        };
        assert_eq!(
            all.header_height() - no_structure.header_height(),
            Metrics::STRUCTURE_LANE_HEIGHT
        );
    }

    #[test]
    fn an_arrangement_round_trips_through_the_file() {
        let mut layout = PanelLayout::default();
        layout.move_to(Panel::Mixer, Dock::Left);
        layout.set_size(Dock::Right, px(360.0));
        layout.header_width = px(220.0);
        layout.lanes.harmony = false;

        let text = serde_json::to_string(&StoredLayout::from(&layout)).unwrap();
        let restored = PanelLayout::from(serde_json::from_str::<StoredLayout>(&text).unwrap());
        assert_eq!(restored, layout);
    }

    #[test]
    fn an_empty_or_partial_file_keeps_the_defaults_for_what_it_omits() {
        let empty = PanelLayout::from(serde_json::from_str::<StoredLayout>("{}").unwrap());
        assert_eq!(empty, PanelLayout::default());

        let partial: StoredLayout = serde_json::from_str(r#"{"sizes":{"bottom":320.0}}"#).unwrap();
        let layout = PanelLayout::from(partial);
        assert_eq!(layout.size(Dock::Bottom), px(320.0));
        assert_eq!(
            layout.size(Dock::Left),
            PanelLayout::default().size(Dock::Left)
        );
        assert_eq!(layout.showing(Dock::Left), Some(Panel::Library));
    }

    #[test]
    fn a_file_that_opens_two_panels_in_one_dock_gets_the_first() {
        // Nothing stops a hand-edited file saying it, and two panels drawn over each other leaves
        // only one of them reachable from the status bar.
        let stored: StoredLayout = serde_json::from_str(
            r#"{"panels":{
                "piano_roll":{"dock":"bottom","open":true},
                "mixer":{"dock":"bottom","open":true}
            }}"#,
        )
        .unwrap();
        let layout = PanelLayout::from(stored);
        assert_eq!(layout.showing(Dock::Bottom), Some(Panel::PianoRoll));
        assert!(!layout.is_open(Panel::Mixer));
    }

    #[test]
    fn a_file_asking_for_an_impossible_size_is_brought_back_inside_the_limits() {
        let stored: StoredLayout =
            serde_json::from_str(r#"{"sizes":{"left":9000.0,"bottom":1.0},"header_width":9000.0}"#)
                .unwrap();
        let layout = PanelLayout::from(stored);
        assert_eq!(layout.size(Dock::Left), PanelLayout::MAX_SIDE);
        assert_eq!(layout.size(Dock::Bottom), PanelLayout::MIN_BOTTOM);
        assert_eq!(layout.header_width, PanelLayout::MAX_HEADERS);
    }
}
