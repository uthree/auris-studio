//! Training-derived excitation, loss and causal radiation priors.
//! See docs/physical-pack.md for the fixed reference cohort and held-out measurements.

use super::Model;

#[derive(Clone, Copy, Debug)]
pub(super) struct Profile {
    pub(super) hardness: f32,
    pub(super) position: f32,
    pub(super) decay: f32,
    pub(super) damping: f32,
    pub(super) normalization: f32,
    pub(super) gains: [f32; 12],
}

pub(super) fn profile(model: Model) -> Option<Profile> {
    Some(match model {
        Model::Bass => Profile {
            hardness: 0.319,
            position: 0.312,
            decay: 6.659,
            damping: 0.0,
            normalization: 0.435712,
            gains: [
                4.5, 4.5, 1.8, -4.5, -4.5, -4.5, -2.7, -1.8, -1.8, 0.0, 0.0, 0.0,
            ],
        },
        Model::Bell => Profile {
            hardness: 0.407,
            position: 0.45,
            decay: 9.159,
            damping: 0.0,
            normalization: 0.210631,
            gains: [
                2.7, 2.7, 2.7, 6.3, 6.3, 6.3, 4.5, -2.7, -6.3, -6.3, -4.5, -2.7,
            ],
        },
        Model::Mallet => Profile {
            hardness: 0.157,
            position: 0.45,
            decay: 4.859,
            damping: 0.0,
            normalization: 0.119921,
            gains: [
                2.7, 6.3, 6.3, 4.5, 0.0, -2.7, -4.5, -4.5, -4.5, -2.7, 0.0, 0.0,
            ],
        },
        _ => return None,
    })
}
