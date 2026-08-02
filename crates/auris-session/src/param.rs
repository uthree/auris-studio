//! Addressing a parameter wherever it lives.

use auris_core::param::ParamId;
use auris_core::{EffectSlotId, TrackId};

/// A parameter a frontend can read or write.
///
/// Routing every parameter edit through one enum means the "update the document, then tell the
/// audio thread" step exists in exactly one place. Without it each control reimplements half of
/// it, and eventually one of them forgets the second half and the document silently disagrees
/// with what is heard.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ParamTarget {
    /// A track's fader, in decibels.
    TrackGain(TrackId),
    /// A track's stereo position, -1.0 to 1.0.
    TrackPan(TrackId),
    /// The master fader, in decibels.
    MasterGain,
    /// The master bus stereo position.
    MasterPan,
    /// A parameter of a track's instrument.
    Instrument {
        /// Track whose instrument is addressed.
        track: TrackId,
        /// Index of the parameter within that instrument.
        param: ParamId,
    },
    /// A parameter of an effect, on a track or on the master bus.
    Effect {
        /// Track the effect sits on; `None` means the master bus.
        track: Option<TrackId>,
        /// Which slot in the chain.
        slot: EffectSlotId,
        /// Index of the parameter within that effect.
        param: ParamId,
    },
}

impl ParamTarget {
    /// The track this target belongs to, if any.
    ///
    /// `None` covers both the master bus and a target that is not track-scoped.
    pub fn track(self) -> Option<TrackId> {
        match self {
            ParamTarget::TrackGain(id) | ParamTarget::TrackPan(id) => Some(id),
            ParamTarget::Instrument { track, .. } => Some(track),
            ParamTarget::Effect { track, .. } => track,
            ParamTarget::MasterGain | ParamTarget::MasterPan => None,
        }
    }

    /// `true` for the mixer's own controls rather than a plugin's.
    ///
    /// These have no [`ParamDescriptor`](auris_core::param::ParamDescriptor) of their own, so
    /// the session synthesises one — see [`crate::Session::descriptor_for`].
    pub fn is_builtin(self) -> bool {
        matches!(
            self,
            ParamTarget::TrackGain(_)
                | ParamTarget::TrackPan(_)
                | ParamTarget::MasterGain
                | ParamTarget::MasterPan
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_master_bus_belongs_to_no_track() {
        assert_eq!(ParamTarget::MasterGain.track(), None);
        assert_eq!(
            ParamTarget::Effect {
                track: None,
                slot: EffectSlotId(3),
                param: ParamId(0)
            }
            .track(),
            None
        );
    }

    #[test]
    fn track_scoped_targets_report_their_track() {
        let id = TrackId(7);
        assert_eq!(ParamTarget::TrackGain(id).track(), Some(id));
        assert_eq!(ParamTarget::TrackPan(id).track(), Some(id));
        assert_eq!(
            ParamTarget::Instrument {
                track: id,
                param: ParamId(2)
            }
            .track(),
            Some(id)
        );
    }

    #[test]
    fn only_mixer_controls_are_builtin() {
        assert!(ParamTarget::MasterPan.is_builtin());
        assert!(ParamTarget::TrackGain(TrackId(1)).is_builtin());
        assert!(
            !ParamTarget::Instrument {
                track: TrackId(1),
                param: ParamId(0)
            }
            .is_builtin()
        );
    }
}
