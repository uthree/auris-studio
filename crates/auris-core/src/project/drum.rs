//! Musical drum assignments, independent of acoustic measurements and instrument naming.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::plugin::PluginState;

/// A musical job a measured sound may be suitable for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrumRole {
    /// Low rhythmic foundation.
    Kick,
    /// Noisy backbeat with an audible body.
    Snare,
    /// Short high-frequency timekeeper.
    ClosedHat,
    /// High-frequency timekeeper with a longer decay.
    OpenHat,
    /// Long bright accent.
    Crash,
    /// Pitched percussive fill voice.
    Tom,
}

impl DrumRole {
    /// Every role, in a stable presentation order.
    pub const ALL: [Self; 6] = [
        Self::Kick,
        Self::Snare,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Crash,
        Self::Tom,
    ];
}

/// Explicit note assignments for future drum generation.
///
/// Missing roles stay missing: neither a conventional MIDI key nor a label supplies a fallback.
/// Assigning a sound here describes its use in a song, not its acoustic classification.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrumMap {
    /// Assigned MIDI key for each available musical role.
    pub voices: BTreeMap<DrumRole, u8>,
    /// Ordered editor lanes, independent of the roles used by automatic generation.
    ///
    /// Older projects only carry [`Self::voices`]. They acquire equivalent lanes when loaded,
    /// while an explicitly empty map remains empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<DrumLane>,
}

/// One named row in the drum editor.
///
/// A lane always addresses one physical MIDI key. Its optional roles only tell automatic
/// generation which musical job may use that key; a hand-authored lane needs no role at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrumLane {
    /// Stable identity used while the lane is renamed, moved or reassigned.
    pub id: u64,
    /// The MIDI key this row plays.
    pub note: u8,
    /// User-facing name. Empty lets a frontend derive a role or MIDI name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Optional automatic-generation roles supplied by this lane.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub roles: BTreeSet<DrumRole>,
}

impl DrumMap {
    /// Reserved instrument-state key holding musical assignments.
    pub const STATE_KEY: &'static str = "auris_drum_map";

    /// Reads a saved assignment without inferring absent roles.
    ///
    /// A present but invalid map becomes an empty assignment. Returning `None` there would
    /// confuse corrupt assignments with legacy documents and silently restore conventional keys.
    pub fn load(state: &PluginState) -> Option<Self> {
        let stored = state.extra.get(Self::STATE_KEY)?;
        let map: Self = serde_json::from_value(stored.clone()).unwrap_or_default();
        Some(map.normalized())
    }

    /// Saves an assignment beside the instrument's own opaque state and parameters.
    pub fn store(&self, state: &mut PluginState) {
        if !state.extra.is_object() {
            state.extra = serde_json::json!({});
        }
        state.extra[Self::STATE_KEY] = serde_json::json!(self.clone().normalized());
    }

    /// Builds an ordered editor map from generation-role assignments.
    pub fn from_voices(voices: impl IntoIterator<Item = (DrumRole, u8)>) -> Self {
        Self {
            voices: voices.into_iter().collect(),
            lanes: Vec::new(),
        }
        .normalized()
    }

    /// A complete General MIDI percussion map, ready for manual programming.
    pub fn general_midi() -> Self {
        const NAMES: &[(u8, &str)] = &[
            (35, "Acoustic Bass Drum"),
            (36, "Bass Drum 1"),
            (37, "Side Stick"),
            (38, "Acoustic Snare"),
            (39, "Hand Clap"),
            (40, "Electric Snare"),
            (41, "Low Floor Tom"),
            (42, "Closed Hi-Hat"),
            (43, "High Floor Tom"),
            (44, "Pedal Hi-Hat"),
            (45, "Low Tom"),
            (46, "Open Hi-Hat"),
            (47, "Low-Mid Tom"),
            (48, "Hi-Mid Tom"),
            (49, "Crash Cymbal 1"),
            (50, "High Tom"),
            (51, "Ride Cymbal 1"),
            (52, "Chinese Cymbal"),
            (53, "Ride Bell"),
            (54, "Tambourine"),
            (55, "Splash Cymbal"),
            (56, "Cowbell"),
            (57, "Crash Cymbal 2"),
            (58, "Vibraslap"),
            (59, "Ride Cymbal 2"),
            (60, "Hi Bongo"),
            (61, "Low Bongo"),
            (62, "Mute Hi Conga"),
            (63, "Open Hi Conga"),
            (64, "Low Conga"),
            (65, "High Timbale"),
            (66, "Low Timbale"),
            (67, "High Agogo"),
            (68, "Low Agogo"),
            (69, "Cabasa"),
            (70, "Maracas"),
            (71, "Short Whistle"),
            (72, "Long Whistle"),
            (73, "Short Guiro"),
            (74, "Long Guiro"),
            (75, "Claves"),
            (76, "Hi Wood Block"),
            (77, "Low Wood Block"),
            (78, "Mute Cuica"),
            (79, "Open Cuica"),
            (80, "Mute Triangle"),
            (81, "Open Triangle"),
        ];
        let mut map = Self {
            voices: [
                (DrumRole::Kick, 36),
                (DrumRole::Snare, 38),
                (DrumRole::ClosedHat, 42),
                (DrumRole::OpenHat, 46),
                (DrumRole::Crash, 49),
                (DrumRole::Tom, 47),
            ]
            .into_iter()
            .collect(),
            lanes: NAMES
                .iter()
                .enumerate()
                .map(|(index, (note, name))| DrumLane {
                    id: index as u64 + 1,
                    note: *note,
                    name: (*name).to_string(),
                    roles: BTreeSet::new(),
                })
                .collect(),
        };
        map.sync_lane_roles_from_voices();
        map
    }

    /// Adds a manual lane, returning its stable identity.
    ///
    /// A MIDI key may appear only once; adding an existing key returns its existing lane.
    pub fn add_lane(&mut self, note: u8, name: impl Into<String>) -> u64 {
        if let Some(lane) = self.lanes.iter().find(|lane| lane.note == note) {
            return lane.id;
        }
        let id = self
            .lanes
            .iter()
            .map(|lane| lane.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.lanes.push(DrumLane {
            id,
            note,
            name: name.into().trim().to_string(),
            roles: BTreeSet::new(),
        });
        id
    }

    /// Removes a lane and every generation role that pointed at it.
    pub fn remove_lane(&mut self, id: u64) -> bool {
        let before = self.lanes.len();
        self.lanes.retain(|lane| lane.id != id);
        if self.lanes.len() == before {
            return false;
        }
        self.sync_voices_from_lanes();
        true
    }

    /// Moves a lane by one or more positions, clamped to the map's ends.
    pub fn move_lane(&mut self, id: u64, offset: i32) -> bool {
        let Some(from) = self.lanes.iter().position(|lane| lane.id == id) else {
            return false;
        };
        let to = (from as i32 + offset).clamp(0, self.lanes.len() as i32 - 1) as usize;
        if from == to {
            return false;
        }
        let lane = self.lanes.remove(from);
        self.lanes.insert(to, lane);
        true
    }

    /// Rebuilds generation assignments from the roles carried by lanes.
    pub fn sync_voices_from_lanes(&mut self) {
        self.voices.clear();
        for lane in &self.lanes {
            for role in &lane.roles {
                self.voices.insert(*role, lane.note);
            }
        }
    }

    fn sync_lane_roles_from_voices(&mut self) {
        for lane in &mut self.lanes {
            lane.roles.clear();
        }
        for (role, note) in &self.voices {
            if let Some(lane) = self.lanes.iter_mut().find(|lane| lane.note == *note) {
                lane.roles.insert(*role);
            }
        }
    }

    fn normalized(mut self) -> Self {
        if self.voices.values().any(|note| *note > 127)
            || self.lanes.iter().any(|lane| lane.note > 127)
        {
            return Self::default();
        }
        if self.lanes.is_empty() {
            for role in DrumRole::ALL {
                let Some(note) = self.voices.get(&role).copied() else {
                    continue;
                };
                if let Some(lane) = self.lanes.iter_mut().find(|lane| lane.note == note) {
                    lane.roles.insert(role);
                } else {
                    self.lanes.push(DrumLane {
                        id: self.lanes.len() as u64 + 1,
                        note,
                        name: String::new(),
                        roles: [role].into_iter().collect(),
                    });
                }
            }
            return self;
        }

        let mut normalized: Vec<DrumLane> = Vec::with_capacity(self.lanes.len());
        let mut used_ids = BTreeSet::new();
        let mut next_id = self
            .lanes
            .iter()
            .map(|lane| lane.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for mut lane in self.lanes {
            lane.name = lane.name.trim().to_string();
            if lane.id == 0 || !used_ids.insert(lane.id) {
                lane.id = next_id;
                used_ids.insert(next_id);
                next_id = next_id.saturating_add(1);
            }
            if let Some(existing) = normalized.iter_mut().find(|item| item.note == lane.note) {
                existing.roles.extend(lane.roles);
                if existing.name.is_empty() {
                    existing.name = lane.name;
                }
            } else {
                normalized.push(lane);
            }
        }
        self.lanes = normalized;
        self.sync_voices_from_lanes();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_invalid_assignments_have_different_meanings() {
        let mut state = PluginState::empty();
        assert_eq!(DrumMap::load(&state), None);
        state.set_hosted_bytes(&[1, 2, 3]);
        let map = DrumMap {
            voices: [(DrumRole::Kick, 73)].into_iter().collect(),
            lanes: Vec::new(),
        };
        map.store(&mut state);
        assert_eq!(DrumMap::load(&state), Some(map.normalized()));
        assert_eq!(state.hosted_bytes(), Some(vec![1, 2, 3]));
        for invalid in [
            serde_json::json!({"voices": {"kick": 128}}),
            serde_json::json!("broken"),
            serde_json::Value::Null,
        ] {
            state.extra[DrumMap::STATE_KEY] = invalid;
            assert_eq!(DrumMap::load(&state), Some(DrumMap::default()));
        }
    }

    #[test]
    fn legacy_roles_become_ordered_lanes_without_inventing_missing_roles() {
        let map = DrumMap::from_voices([
            (DrumRole::Kick, 73),
            (DrumRole::Snare, 18),
            (DrumRole::Tom, 18),
        ]);
        assert_eq!(
            map.lanes.iter().map(|lane| lane.note).collect::<Vec<_>>(),
            [73, 18]
        );
        assert_eq!(
            map.lanes[1].roles,
            [DrumRole::Snare, DrumRole::Tom].into_iter().collect()
        );
    }

    #[test]
    fn manual_lanes_keep_their_order_names_and_optional_roles() {
        let mut map = DrumMap::default();
        let clap = map.add_lane(39, "  Clap  ");
        let kick = map.add_lane(36, "Kick close");
        assert_eq!(map.add_lane(39, "duplicate"), clap);
        map.lanes[0].roles.insert(DrumRole::Snare);
        map.sync_voices_from_lanes();
        assert_eq!(map.voices[&DrumRole::Snare], 39);
        assert!(map.move_lane(kick, -1));
        assert_eq!(
            map.lanes.iter().map(|lane| lane.note).collect::<Vec<_>>(),
            [36, 39]
        );
        assert!(map.remove_lane(clap));
        assert!(map.voices.is_empty());
    }

    #[test]
    fn general_midi_is_a_complete_named_manual_template() {
        let map = DrumMap::general_midi();
        assert_eq!(map.lanes.first().unwrap().note, 35);
        assert_eq!(map.lanes.last().unwrap().note, 81);
        assert_eq!(map.lanes.len(), 47);
        assert_eq!(map.voices[&DrumRole::ClosedHat], 42);
        assert_eq!(map.lanes[4].name, "Hand Clap");
    }
}
