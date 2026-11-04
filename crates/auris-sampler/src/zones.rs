//! The audio sampler's map: which recording a note plays.
//!
//! A sampler holding one recording pitches it across the whole keyboard, and two octaves from
//! its root a piano note sounds like a music box and a voice like a cartoon. Real sampled
//! instruments are therefore *multisamples*: a recording every few keys, each played only near
//! the pitch it was recorded at, and often several recordings of the same key struck softly and
//! hard, because a loud note is not a quiet note turned up. A [`SampleZone`] is one recording in
//! that map, and [`pick_zone`] is the rule that turns a key and a velocity into one of them.
//!
//! # The rule
//!
//! Choosing by the nearest root is what a sampler's "auto map" does, and it is the default here:
//! a zone with no key range of its own covers whatever keys are closer to its root than to any
//! other zone's. That alone is a playable multisample with nothing to set. On top of it sit the
//! two refinements every sampler format since the Akai has had, because the nearest root cannot
//! express them:
//!
//! * **Velocity layers.** Each zone answers a range of velocities. A note goes to the layer whose
//!   range holds its velocity — or, falling in a gap between layers, to the nearest one — and the
//!   nearest-root rule runs only inside that layer. A soft and a hard recording of the same key
//!   are therefore two zones with one root and two ranges.
//! * **Explicit key ranges.** A zone may name the keys it covers, for a split that is not at the
//!   midpoint — a bass under the left hand and a lead over the right, or a set of drum hits each
//!   on its own key. Where an explicit range covers the key it wins over every automatic zone in
//!   the layer; a key that only explicit zones are mapped around, and none of them covers, is
//!   silent, which is what naming the keys means.
//!
//! Recordings that tie — the same root, in the same layer — are **round-robin alternates**, the
//! way a sampled drum or a plucked string avoids the machine-gun of one recording played twice:
//! successive notes take them in turn.

use auris_core::SourceId;
use auris_core::plugin::PluginState;
use serde::{Deserialize, Serialize};

/// Where the zones live inside [`PluginState::extra`].
const ZONES_KEY: &str = "zones";

/// Where a single sample was stored before the sampler held a map, read so that such a project
/// still plays.
const LEGACY_SAMPLE_KEY: &str = "sample";

/// The root note a single legacy sample took its pitch from.
const LEGACY_ROOT_KEY: &str = "root";

/// The most zones one sampler holds.
///
/// A fully chromatic instrument with four velocity layers is 512, and nothing anyone builds by
/// hand comes near; the bound is what keeps a model asking for a sample per frame from building
/// an instrument a graph rebuild has to decode for minutes.
pub const MAX_ZONES: usize = 512;

/// The keys from `low` to `high`, both included.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRange {
    /// The lowest key, as a MIDI note.
    pub low: u8,
    /// The highest key, as a MIDI note.
    pub high: u8,
}

impl KeyRange {
    /// A range, put the right way round whichever order the ends arrive in.
    pub fn new(a: u8, b: u8) -> Self {
        Self {
            low: a.min(b).min(127),
            high: a.max(b).min(127),
        }
    }

    /// Whether `key` is inside.
    pub fn contains(&self, key: u8) -> bool {
        (self.low..=self.high).contains(&key)
    }
}

/// The velocities from `low` to `high`, both included, normalised 0..1 as every note is.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VelocityRange {
    /// The softest velocity answered.
    pub low: f32,
    /// The hardest velocity answered.
    pub high: f32,
}

impl VelocityRange {
    /// Every velocity: what a zone with no layer answers.
    pub const FULL: Self = Self {
        low: 0.0,
        high: 1.0,
    };

    /// A range, clamped into 0..1 and put the right way round.
    pub fn new(a: f32, b: f32) -> Self {
        let a = if a.is_finite() {
            a.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let b = if b.is_finite() {
            b.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            low: a.min(b),
            high: a.max(b),
        }
    }

    /// How far `velocity` is outside the range; zero inside it.
    pub fn distance(&self, velocity: f32) -> f32 {
        if velocity < self.low {
            self.low - velocity
        } else if velocity > self.high {
            velocity - self.high
        } else {
            0.0
        }
    }

    /// `true` for the range that answers everything.
    pub fn is_full(&self) -> bool {
        self.low <= 0.0 && self.high >= 1.0
    }
}

impl Default for VelocityRange {
    fn default() -> Self {
        Self::FULL
    }
}

/// One recording in the sampler's map.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleZone {
    /// The project audio source it plays.
    pub source: SourceId,
    /// The key the recording sounds at.
    pub root: u8,
    /// The keys it covers, or `None` for the automatic map: whatever keys are nearer its root
    /// than any other zone's in its layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<KeyRange>,
    /// The velocities it answers.
    #[serde(default, skip_serializing_if = "VelocityRange::is_full")]
    pub velocity: VelocityRange,
    /// Fine tuning of this recording alone, in semitones, on top of the sampler's own.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tune: f32,
    /// Level of this recording alone, in decibels, for balancing layers against each other.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

impl SampleZone {
    /// A zone playing `source` from `root` over the automatic map, at every velocity.
    pub fn new(source: SourceId, root: u8) -> Self {
        Self {
            source,
            root: root.min(127),
            keys: None,
            velocity: VelocityRange::FULL,
            tune: 0.0,
            gain_db: 0.0,
        }
    }

    /// Puts every field back inside its range, for a zone read from somewhere untrusted.
    pub fn sanitised(mut self) -> Self {
        self.root = self.root.min(127);
        self.keys = self.keys.map(|keys| KeyRange::new(keys.low, keys.high));
        self.velocity = VelocityRange::new(self.velocity.low, self.velocity.high);
        self.tune = if self.tune.is_finite() {
            self.tune.clamp(-24.0, 24.0)
        } else {
            0.0
        };
        self.gain_db = if self.gain_db.is_finite() {
            self.gain_db.clamp(-60.0, 12.0)
        } else {
            0.0
        };
        self
    }
}

/// Which zone a note plays, or `None` when none answers it.
///
/// `turn` chooses among round-robin alternates: the zone taken is the `turn`-th of the ties,
/// counting round. **Called on the audio thread**, so it walks the slice a few times rather than
/// collecting anything.
pub fn pick_zone(zones: &[SampleZone], key: u8, velocity: f32, turn: usize) -> Option<usize> {
    if zones.is_empty() {
        return None;
    }
    let velocity = if velocity.is_finite() { velocity } else { 1.0 };
    // The layer: every zone whose range holds the velocity, or the nearest when none does.
    let nearest_layer = zones
        .iter()
        .map(|zone| zone.velocity.distance(velocity))
        .fold(f32::INFINITY, f32::min);
    let in_layer = |zone: &SampleZone| zone.velocity.distance(velocity) <= nearest_layer;
    let explicit = zones
        .iter()
        .any(|zone| in_layer(zone) && zone.keys.is_some_and(|keys| keys.contains(key)));
    let candidate = |zone: &SampleZone| {
        in_layer(zone)
            && match zone.keys {
                Some(keys) => explicit && keys.contains(key),
                None => !explicit,
            }
    };
    // Nearest root; between two equally near the lower wins, so the choice never depends on the
    // order the zones were added in.
    let best_root = zones
        .iter()
        .filter(|zone| candidate(zone))
        .map(|zone| zone.root)
        .min_by_key(|root| (root.abs_diff(key), *root))?;
    let ties = zones
        .iter()
        .filter(|zone| candidate(zone) && zone.root == best_root)
        .count();
    let wanted = turn % ties.max(1);
    zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| candidate(zone) && zone.root == best_root)
        .nth(wanted)
        .map(|(index, _)| index)
}

/// The keys zone `index` actually plays at a velocity inside its own layer, lowest and highest.
///
/// What the map *means*, for a reader: an automatic zone's range is decided by its neighbours, so
/// this is the only way to say it. Ties count as covered by every alternate.
pub fn keys_played_by(zones: &[SampleZone], index: usize) -> Option<KeyRange> {
    let zone = zones.get(index)?;
    let velocity = 0.5 * (zone.velocity.low + zone.velocity.high);
    let mut played = (0..=127u8).filter(|&key| {
        pick_zone(zones, key, velocity, 0).is_some_and(|picked| {
            picked == index || {
                let other = &zones[picked];
                other.root == zone.root
                    && other.keys == zone.keys
                    && other.velocity == zone.velocity
            }
        })
    });
    let low = played.next()?;
    let high = played.next_back().unwrap_or(low);
    Some(KeyRange { low, high })
}

/// Writes the zones into the state a project stores for a track's instrument.
pub fn store_zones(state: &mut PluginState, zones: &[SampleZone]) {
    let encoded = serde_json::to_value(zones).unwrap_or(serde_json::Value::Null);
    if !state.extra.is_object() {
        state.extra = serde_json::Value::Object(serde_json::Map::new());
    }
    if let Some(map) = state.extra.as_object_mut() {
        map.remove(LEGACY_SAMPLE_KEY);
        map.insert(ZONES_KEY.to_string(), encoded);
    }
}

/// Reads the zones back; empty when the state names none.
///
/// A state from before the map, naming one sample and a root-note parameter, reads as the one
/// zone it meant.
pub fn stored_zones(state: &PluginState) -> Vec<SampleZone> {
    if let Some(zones) = state.extra.get(ZONES_KEY) {
        return serde_json::from_value::<Vec<SampleZone>>(zones.clone())
            .map(|zones| {
                zones
                    .into_iter()
                    .take(MAX_ZONES)
                    .map(SampleZone::sanitised)
                    .collect()
            })
            .unwrap_or_default();
    }
    let Some(source) = state.extra.get(LEGACY_SAMPLE_KEY).and_then(|v| v.as_u64()) else {
        return Vec::new();
    };
    let root = state
        .params
        .get(LEGACY_ROOT_KEY)
        .copied()
        .filter(|root| root.is_finite())
        .map_or(60, |root| root.round().clamp(0.0, 127.0) as u8);
    vec![SampleZone::new(SourceId(source), root)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(source: u64, root: u8) -> SampleZone {
        SampleZone::new(SourceId(source), root)
    }

    fn layered(source: u64, root: u8, low: f32, high: f32) -> SampleZone {
        SampleZone {
            velocity: VelocityRange::new(low, high),
            ..zone(source, root)
        }
    }

    #[test]
    fn each_key_plays_the_nearest_root() {
        let zones = [zone(1, 48), zone(2, 60), zone(3, 72)];
        assert_eq!(pick_zone(&zones, 30, 1.0, 0), Some(0));
        assert_eq!(pick_zone(&zones, 53, 1.0, 0), Some(0));
        assert_eq!(pick_zone(&zones, 55, 1.0, 0), Some(1));
        assert_eq!(pick_zone(&zones, 60, 1.0, 0), Some(1));
        assert_eq!(pick_zone(&zones, 67, 1.0, 0), Some(2));
        assert_eq!(pick_zone(&zones, 120, 1.0, 0), Some(2));
    }

    #[test]
    fn a_key_halfway_between_two_roots_takes_the_lower_whatever_the_order() {
        let forward = [zone(1, 60), zone(2, 64)];
        let backward = [zone(2, 64), zone(1, 60)];
        assert_eq!(forward[pick_zone(&forward, 62, 1.0, 0).unwrap()].root, 60);
        assert_eq!(backward[pick_zone(&backward, 62, 1.0, 0).unwrap()].root, 60);
    }

    #[test]
    fn velocity_layers_choose_before_the_root_does() {
        let zones = [
            layered(1, 60, 0.0, 0.5),
            layered(2, 60, 0.5, 1.0),
            layered(3, 72, 0.5, 1.0),
        ];
        assert_eq!(pick_zone(&zones, 60, 0.3, 0), Some(0));
        assert_eq!(pick_zone(&zones, 60, 0.9, 0), Some(1));
        // The soft layer has only C4, so a soft C5 plays it rather than the loud C5.
        assert_eq!(pick_zone(&zones, 72, 0.3, 0), Some(0));
        assert_eq!(pick_zone(&zones, 72, 0.9, 0), Some(2));
    }

    #[test]
    fn a_velocity_in_a_gap_goes_to_the_nearest_layer() {
        let zones = [layered(1, 60, 0.0, 0.3), layered(2, 60, 0.8, 1.0)];
        assert_eq!(pick_zone(&zones, 60, 0.4, 0), Some(0));
        assert_eq!(pick_zone(&zones, 60, 0.7, 0), Some(1));
    }

    #[test]
    fn an_explicit_range_wins_and_leaves_its_outside_to_the_rest() {
        let mut bass = zone(1, 40);
        bass.keys = Some(KeyRange::new(0, 59));
        let mut lead = zone(2, 72);
        lead.keys = Some(KeyRange::new(60, 127));
        let zones = [bass, lead];
        // The split is where it was named, not at the midpoint between 40 and 72.
        assert_eq!(pick_zone(&zones, 59, 1.0, 0), Some(0));
        assert_eq!(pick_zone(&zones, 60, 1.0, 0), Some(1));
    }

    #[test]
    fn keys_only_explicit_zones_surround_are_silent() {
        let mut kick = zone(1, 36);
        kick.keys = Some(KeyRange::new(36, 36));
        let mut snare = zone(2, 38);
        snare.keys = Some(KeyRange::new(38, 38));
        let zones = [kick, snare];
        assert_eq!(pick_zone(&zones, 36, 1.0, 0), Some(0));
        assert_eq!(pick_zone(&zones, 37, 1.0, 0), None);
        assert_eq!(pick_zone(&zones, 38, 1.0, 0), Some(1));
    }

    #[test]
    fn identical_zones_take_turns() {
        let zones = [zone(1, 60), zone(2, 60), zone(3, 72)];
        assert_eq!(pick_zone(&zones, 61, 1.0, 0), Some(0));
        assert_eq!(pick_zone(&zones, 61, 1.0, 1), Some(1));
        assert_eq!(pick_zone(&zones, 61, 1.0, 2), Some(0));
        // A zone with no alternate is the same zone every turn.
        assert_eq!(pick_zone(&zones, 72, 1.0, 1), Some(2));
    }

    #[test]
    fn an_empty_map_plays_nothing() {
        assert_eq!(pick_zone(&[], 60, 1.0, 0), None);
    }

    #[test]
    fn the_keys_a_zone_plays_follow_its_neighbours() {
        let zones = [zone(1, 48), zone(2, 60), zone(3, 72)];
        assert_eq!(keys_played_by(&zones, 0), Some(KeyRange::new(0, 54)));
        assert_eq!(keys_played_by(&zones, 1), Some(KeyRange::new(55, 66)));
        assert_eq!(keys_played_by(&zones, 2), Some(KeyRange::new(67, 127)));
    }

    #[test]
    fn zones_survive_the_state_and_a_legacy_sample_reads_as_one() {
        let mut state = PluginState::empty();
        let zones = vec![
            zone(1, 48),
            SampleZone {
                keys: Some(KeyRange::new(70, 90)),
                velocity: VelocityRange::new(0.5, 1.0),
                tune: 0.25,
                gain_db: -3.0,
                ..zone(2, 72)
            },
        ];
        store_zones(&mut state, &zones);
        assert_eq!(stored_zones(&state), zones);

        let mut legacy = PluginState::empty();
        legacy.extra = serde_json::json!({ "sample": 9 });
        legacy.params.insert("root".into(), 57.0);
        assert_eq!(stored_zones(&legacy), vec![zone(9, 57)]);
        store_zones(&mut legacy, &[zone(9, 57)]);
        assert!(legacy.extra.get("sample").is_none());
    }

    #[test]
    fn a_stored_zone_out_of_range_is_brought_back_in() {
        let mut state = PluginState::empty();
        state.extra = serde_json::json!({ "zones": [
            { "source": 1, "root": 60, "keys": { "low": 90, "high": 20 },
              "velocity": { "low": 2.0, "high": -1.0 }, "tune": 99.0 }
        ]});
        let zone = stored_zones(&state)[0];
        assert_eq!(zone.keys, Some(KeyRange::new(20, 90)));
        assert_eq!(zone.velocity, VelocityRange::FULL);
        assert_eq!(zone.tune, 24.0);
    }
}
