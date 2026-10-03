//! Training-derived excitation, loss and causal radiation priors.
//! See docs/physical-pack.md for the fixed reference cohort and held-out measurements.

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
            hardness: 0.407,
            position: 0.492,
            decay: 1.405,
            damping: 0.422,
            normalization: 1.326079,
            gains: [
                -4.5, -2.7, 0.0, 0.0, -1.8, 0.0, 4.5, 4.5, 1.8, 2.7, 1.8, 0.0,
            ],
        },
        Pad::Snare => Profile {
            hardness: 0.65,
            position: 0.6,
            decay: 1.0,
            damping: 0.35,
            normalization: 0.948105,
            gains: [
                0.0, 0.0, 0.0, 0.0, 2.25, 1.26, -0.0, -0.0, -0.0, -0.0, -0.0, -0.36,
            ],
        },
        Pad::ClosedHat => Profile {
            hardness: 0.677,
            position: 0.627,
            decay: 1.405,
            damping: 0.134,
            normalization: 1.328915,
            gains: [0.0, 1.8, 1.8, 6.3, 6.3, 6.3, 6.3, 4.5, 2.7, 2.7, -6.3, -6.3],
        },
        Pad::OpenHat => Profile {
            hardness: 0.407,
            position: 0.546,
            decay: 1.405,
            damping: 0.518,
            normalization: 0.799053,
            gains: [0.0, 0.0, 1.8, 1.8, 6.3, 6.3, 6.3, 6.3, 1.8, -2.7, -6.3, 4.5],
        },
        Pad::Crash => Profile {
            hardness: 0.839,
            position: 0.411,
            decay: 1.405,
            damping: 0.134,
            normalization: 1.014032,
            gains: [
                0.0, 1.8, 1.8, 1.8, 1.8, -2.7, -2.7, -2.7, -4.5, -6.3, -6.3, 6.3,
            ],
        },
        Pad::Ride => Profile {
            hardness: 0.407,
            position: 0.465,
            decay: 1.405,
            damping: 0.566,
            normalization: 2.508769,
            gains: [
                0.0, 0.0, -1.8, -2.7, -6.3, -6.3, -6.3, -6.3, -6.3, -6.3, -4.5, 6.3,
            ],
        },
        Pad::Tom => Profile {
            hardness: 0.407,
            position: 0.465,
            decay: 1.405,
            damping: 0.518,
            normalization: 0.455919,
            gains: [
                5.1975, 7.4475, -0.135, -7.4475, -2.6775, -0.4275, 3.5775, 5.4225, 4.5225, 2.025,
                0.54, 0.0,
            ],
        },
    }
}
