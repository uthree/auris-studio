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
}

impl Panel {
    /// Every panel, in the order a dock stacks their icons.
    pub const ALL: [Panel; 4] = [
        Panel::Library,
        Panel::PianoRoll,
        Panel::Mixer,
        Panel::Inspector,
    ];

    /// Where the keyboard is while this panel holds it.
    pub fn pane(self) -> Pane {
        match self {
            Panel::Library => Pane::Library,
            Panel::PianoRoll => Pane::PianoRoll,
            Panel::Mixer => Pane::Mixer,
            Panel::Inspector => Pane::Inspector,
        }
    }

    /// What the panel is called.
    pub fn label(self) -> Key {
        match self {
            Panel::Library => Key::Library,
            Panel::PianoRoll => Key::PianoRoll,
            Panel::Mixer => Key::Mixer,
            Panel::Inspector => Key::Inspector,
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

/// Where every panel sits, whether it is showing, and how large each dock is drawn.
///
/// Kept together so the layout, the hit tests and the dividers all read one source rather than
/// each deriving the geometry from constants and drifting apart.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// Which dock each panel is in, in [`Panel::ALL`] order.
    docks: [Dock; 4],
    /// Whether each panel is showing, in [`Panel::ALL`] order.
    ///
    /// At most one is true per dock; [`Self::show`] is what keeps that so.
    open: [bool; 4],
    /// How large each dock is drawn, in [`Dock::ALL`] order: a width for the sides, a height for
    /// the bottom.
    sizes: [Pixels; 3],
    /// Width of the track header column.
    pub header_width: Pixels,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            // Library on the left, inspector on the right, the two editors sharing the strip along
            // the bottom: Logic's arrangement, and the one this window was laid out at.
            docks: [Dock::Left, Dock::Bottom, Dock::Bottom, Dock::Right],
            // The mixer starts closed because the piano roll has the dock they share. The library
            // starts open, unlike Logic, which defaults its Library closed: there, a channel
            // strip's instrument slot is a second way to load one; here the library is the only
            // one, so starting closed would leave a new project with no visible way to choose an
            // instrument at all.
            open: [true, true, false, true],
            sizes: [
                Metrics::LEFT_DOCK_WIDTH,
                Metrics::BOTTOM_DOCK_HEIGHT,
                Metrics::RIGHT_DOCK_WIDTH,
            ],
            header_width: Metrics::TRACK_HEADER_WIDTH,
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
        // The mixer is still down there, waiting to be asked for.
        assert_eq!(layout.panels_in(Dock::Bottom).count(), 2);
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
    fn an_arrangement_round_trips_through_the_file() {
        let mut layout = PanelLayout::default();
        layout.move_to(Panel::Mixer, Dock::Left);
        layout.set_size(Dock::Right, px(360.0));
        layout.header_width = px(220.0);

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
