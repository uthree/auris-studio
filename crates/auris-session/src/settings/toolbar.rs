//! Persistent ordering and visibility of the desktop's top toolbar.

use serde::{Deserialize, Serialize};

/// An independently configurable toolbar item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarItem {
    /// Return, stop and play controls.
    Playback,
    /// Record control.
    Record,
    /// Punch recording control.
    Punch,
    /// Loop control.
    Loop,
    /// Metronome control.
    Metronome,
    /// Editing grid.
    Grid,
    /// Arrangement zoom.
    Zoom,
    /// Musical and clock position.
    Position,
    /// Tempo at the playhead.
    Tempo,
    /// Time signature at the playhead.
    Signature,
    /// Chord at the playhead.
    Chord,
    /// Recording clock and count-in.
    Take,
    /// Audio input level.
    Input,
    /// Master output level and clipping reset.
    Master,
    /// Compact live audio spectrum.
    Visualizer,
}

impl ToolbarItem {
    /// Every item in its default order, including optional items.
    pub const ALL: [Self; 15] = [
        Self::Playback,
        Self::Record,
        Self::Punch,
        Self::Loop,
        Self::Metronome,
        Self::Grid,
        Self::Zoom,
        Self::Position,
        Self::Tempo,
        Self::Signature,
        Self::Chord,
        Self::Take,
        Self::Input,
        Self::Master,
        Self::Visualizer,
    ];

    /// Layout lane: playback, editing, readouts, or meters.
    pub fn lane(self) -> usize {
        match self {
            Self::Playback | Self::Record | Self::Punch | Self::Loop | Self::Metronome => 0,
            Self::Grid | Self::Zoom => 1,
            Self::Position | Self::Tempo | Self::Signature | Self::Chord => 2,
            Self::Take | Self::Input | Self::Master | Self::Visualizer => 3,
        }
    }
}

/// A toolbar item's position and visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolbarEntry {
    /// Which control occupies this position.
    pub item: ToolbarItem,
    /// Whether it is drawn.
    pub visible: bool,
}

/// Installation preferences for the desktop toolbar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarPreferences {
    /// Ordered entries; order applies within each layout lane.
    pub entries: Vec<ToolbarEntry>,
}

impl Default for ToolbarPreferences {
    fn default() -> Self {
        Self {
            entries: ToolbarItem::ALL
                .into_iter()
                .map(|item| ToolbarEntry {
                    item,
                    visible: item != ToolbarItem::Visualizer,
                })
                .collect(),
        }
    }
}

impl ToolbarPreferences {
    /// Removes duplicates and restores entries omitted from an older settings file.
    pub fn normalize(&mut self) {
        let mut seen = Vec::new();
        self.entries.retain(|entry| {
            if seen.contains(&entry.item) {
                return false;
            }
            seen.push(entry.item);
            true
        });
        self.entries.extend(
            Self::default()
                .entries
                .into_iter()
                .filter(|entry| !seen.contains(&entry.item)),
        );
    }

    /// Visible controls in a layout lane, in the chosen order.
    pub fn visible_in(&self, lane: usize) -> impl Iterator<Item = ToolbarItem> + '_ {
        self.entries
            .iter()
            .filter(move |entry| entry.visible && entry.item.lane() == lane)
            .map(|entry| entry.item)
    }

    /// Whether a control is visible.
    pub fn contains(&self, item: ToolbarItem) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.item == item && entry.visible)
    }

    /// Moves an item one position within its lane. Returns false at either end.
    pub fn move_item(&mut self, item: ToolbarItem, forward: bool) -> bool {
        let positions: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.item.lane() == item.lane())
            .map(|(index, _)| index)
            .collect();
        let Some(at) = positions
            .iter()
            .position(|&index| self.entries[index].item == item)
        else {
            return false;
        };
        let next = if forward {
            at.checked_add(1)
        } else {
            at.checked_sub(1)
        };
        let Some(next) = next.and_then(|next| positions.get(next)) else {
            return false;
        };
        self.entries.swap(positions[at], *next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_custom_choices_and_fills_missing_controls() {
        let mut prefs = ToolbarPreferences {
            entries: vec![
                ToolbarEntry {
                    item: ToolbarItem::Zoom,
                    visible: false,
                },
                ToolbarEntry {
                    item: ToolbarItem::Zoom,
                    visible: true,
                },
            ],
        };
        prefs.normalize();
        assert_eq!(prefs.entries.len(), ToolbarItem::ALL.len());
        assert!(!prefs.contains(ToolbarItem::Zoom));
        assert_eq!(
            prefs.visible_in(1).collect::<Vec<_>>(),
            vec![ToolbarItem::Grid]
        );
    }

    #[test]
    fn moving_an_item_stays_in_its_lane_and_keeps_visibility() {
        let mut prefs = ToolbarPreferences::default();
        assert!(prefs.move_item(ToolbarItem::Zoom, false));
        assert!(!prefs.move_item(ToolbarItem::Zoom, false));
        assert_eq!(
            prefs.visible_in(1).collect::<Vec<_>>(),
            vec![ToolbarItem::Zoom, ToolbarItem::Grid]
        );
    }
}
