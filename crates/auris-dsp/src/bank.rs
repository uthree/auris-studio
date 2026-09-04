//! Parameter storage shared by every effect in this crate.
//!
//! `auris_core::plugin::Parameterized` requires each plugin to own its descriptor list and its
//! current values. That bookkeeping is identical everywhere, so it lives here once.

use auris_core::param::{ParamDescriptor, ParamId};

/// A plugin's descriptor list plus the current value of each parameter.
pub(crate) struct ParamBank {
    descriptors: Vec<ParamDescriptor>,
    values: Vec<f32>,
}

impl ParamBank {
    /// Builds a bank from descriptors, seeding every value with its default.
    pub(crate) fn new(descriptors: Vec<ParamDescriptor>) -> Self {
        debug_assert!(
            descriptors
                .iter()
                .enumerate()
                .all(|(index, d)| d.id.index() == index),
            "descriptor ids must match their position in the slice"
        );
        let values = descriptors.iter().map(|d| d.default).collect();
        Self {
            descriptors,
            values,
        }
    }

    /// The descriptors, in id order.
    pub(crate) fn descriptors(&self) -> &[ParamDescriptor] {
        &self.descriptors
    }

    /// Mutable descriptors, for ranges that become known when the host supplies its format.
    pub(crate) fn descriptors_mut(&mut self) -> &mut [ParamDescriptor] {
        &mut self.descriptors
    }

    /// Current value of `id`, or `0.0` when the id is out of range.
    pub(crate) fn get(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    /// Current value of the parameter at `index`.
    pub(crate) fn at(&self, index: u32) -> f32 {
        self.get(ParamId(index))
    }

    /// A toggle or choice parameter read as a boolean.
    pub(crate) fn flag(&self, index: u32) -> bool {
        self.at(index) >= 0.5
    }

    /// Clamps `value` with the matching descriptor and stores it.
    ///
    /// Returns `false` for an unknown id so callers can skip any cached-coefficient update.
    pub(crate) fn set(&mut self, id: ParamId, value: f32) -> bool {
        match self.descriptors.get(id.index()) {
            Some(descriptor) => {
                self.values[id.index()] = descriptor.clamp(value);
                true
            }
            None => false,
        }
    }
}
