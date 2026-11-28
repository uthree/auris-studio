//! Training-derived excitation, loss and causal radiation priors.
//! The fixed reference cohort and held-out measurements are in
//! `tools/eval/references/physical-pack-{captures,notes,validation}.json`.

use super::Pad;

#[derive(Clone, Copy, Debug)]
pub(super) struct Profile {
    pub(super) hardness: f32,
    pub(super) position: f32,
    pub(super) decay: f32,
    pub(super) damping: f32,
    pub(super) normalization: f32,
    pub(super) gains: [f32; 12],
}

pub(super) fn profile(pad: Pad) -> Profile {
    match pad {
        Pad::Kick => Profile {
            hardness: 0.461,
            position: 0.411,
            decay: 0.775,
            damping: 0.326,
            normalization: 0.804996,
            gains: [
                2.7, -1.8, 1.8, -4.5, -1.8, 0.0, 0.0, -4.5, 0.0, -1.8, -1.8, 0.0,
            ],
        },
        Pad::Snare => Profile {
            hardness: 0.407,
            position: 0.492,
            decay: 0.82,
            damping: 0.374,
            normalization: 0.372844,
            gains: [1.8, 6.3, 1.8, -6.3, 2.7, 4.5, 6.3, 0.0, 0.0, 1.8, 0.0, -2.7],
        },
        Pad::ClosedHat => Profile {
            hardness: 0.893,
            position: 0.843,
            decay: 0.91,
            damping: 0.326,
            normalization: 0.296489,
            gains: [
                -2.7, 0.0, 0.0, -1.8, 0.0, 0.0, -1.8, 0.0, -1.8, 0.0, -1.8, 0.0,
            ],
        },
        Pad::OpenHat => Profile {
            hardness: 0.407,
            position: 0.519,
            decay: 1.315,
            damping: 0.35,
            normalization: 0.268367,
            gains: [
                -4.5, -4.5, -4.5, 1.8, 1.8, -4.5, 0.0, 1.8, 0.0, 1.8, 0.0, 6.3,
            ],
        },
        Pad::Crash => Profile {
            hardness: 0.893,
            position: 0.627,
            decay: 1.315,
            damping: 0.182,
            normalization: 1.027223,
            gains: [
                -6.3, -6.3, -6.3, 6.3, -2.7, -6.3, -4.5, 0.0, -2.7, 0.0, 0.0, -2.7,
            ],
        },
        Pad::Ride => Profile {
            hardness: 0.407,
            position: 0.6,
            decay: 1.315,
            damping: 0.23,
            normalization: 0.150196,
            gains: [
                -6.3, -4.5, -4.5, 0.0, 0.0, 0.0, -2.7, -2.7, 1.8, -2.7, -2.7, 4.5,
            ],
        },
        Pad::Tom => Profile {
            hardness: 0.407,
            position: 0.357,
            decay: 0.685,
            damping: 0.518,
            normalization: 0.477492,
            gains: [
                2.7, 6.3, -1.8, -4.5, -2.7, 0.0, 0.0, -4.5, 0.0, 0.0, -1.8, 0.0,
            ],
        },
    }
}
