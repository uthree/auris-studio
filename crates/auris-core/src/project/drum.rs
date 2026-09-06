//! Musical drum assignments, independent of acoustic measurements and instrument naming.

use std::collections::BTreeMap;

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
        Some(if map.voices.values().all(|note| *note <= 127) {
            map
        } else {
            Self::default()
        })
    }

    /// Saves an assignment beside the instrument's own opaque state and parameters.
    pub fn store(&self, state: &mut PluginState) {
        if !state.extra.is_object() {
            state.extra = serde_json::json!({});
        }
        state.extra[Self::STATE_KEY] = serde_json::json!(self);
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
        };
        map.store(&mut state);
        assert_eq!(DrumMap::load(&state), Some(map));
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
}
