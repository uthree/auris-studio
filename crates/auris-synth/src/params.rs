//! Parameter storage shared by the built-in instruments.

use auris_core::{ParamDescriptor, ParamId};

/// A plugin's parameter descriptors plus its current values.
///
/// The descriptor list doubles as the value layout: value `n` belongs to descriptor `n`, which
/// is what the [`Parameterized`](auris_core::Parameterized) contract requires of the ids.
/// Everything is clamped through the descriptor on the way in, so a value read back out is
/// always inside the declared range and always finite.
#[derive(Clone, Debug)]
pub struct ParamBank {
    descriptors: Vec<ParamDescriptor>,
    values: Vec<f32>,
}

impl ParamBank {
    /// Builds a bank holding each descriptor's default value.
    pub fn new(descriptors: Vec<ParamDescriptor>) -> Self {
        let values = descriptors.iter().map(|d| d.clamp(d.default)).collect();
        Self {
            descriptors,
            values,
        }
    }

    /// The descriptors, in id order.
    pub fn descriptors(&self) -> &[ParamDescriptor] {
        &self.descriptors
    }

    /// Current value of a parameter; out-of-range ids read as `0.0`.
    pub fn get(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    /// Current value of the parameter at `index`.
    pub fn at(&self, index: u32) -> f32 {
        self.get(ParamId(index))
    }

    /// Clamps and stores a value, returning `false` for an unknown id.
    pub fn set(&mut self, id: ParamId, value: f32) -> bool {
        let Some(descriptor) = self.descriptors.get(id.index()) else {
            return false;
        };
        let clamped = descriptor.clamp(value);
        if let Some(slot) = self.values.get_mut(id.index()) {
            *slot = clamped;
        }
        true
    }

    /// Number of parameters.
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// `true` when the plugin has no parameters.
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

/// Returns `value` when it is finite, otherwise `fallback`.
///
/// Note events come from MIDI hardware and from other plugins, so their payloads are not
/// trusted: one NaN velocity would otherwise turn a voice's output into NaN for as long as it
/// sounds, and that spreads through the whole mix bus.
pub(crate) fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::ParamDescriptor;

    fn bank() -> ParamBank {
        ParamBank::new(vec![
            ParamDescriptor::new(0u32, "gain", "Gain", -60.0, 6.0, -6.0),
            ParamDescriptor::new(1u32, "steps", "Steps", 1.0, 4.0, 1.0).with_steps(4),
        ])
    }

    #[test]
    fn values_start_at_their_defaults() {
        let bank = bank();
        assert_eq!(bank.at(0), -6.0);
        assert_eq!(bank.at(1), 1.0);
        assert_eq!(bank.len(), 2);
        assert!(!bank.is_empty());
    }

    #[test]
    fn writes_are_clamped_and_snapped() {
        let mut bank = bank();
        assert!(bank.set(ParamId(0), 100.0));
        assert_eq!(bank.at(0), 6.0);
        assert!(bank.set(ParamId(1), 2.4));
        assert_eq!(bank.at(1), 2.0);
        assert!(bank.set(ParamId(0), f32::NAN));
        assert_eq!(bank.at(0), -6.0, "NaN falls back to the default");
    }

    #[test]
    fn unknown_ids_are_rejected() {
        let mut bank = bank();
        assert!(!bank.set(ParamId(9), 1.0));
        assert_eq!(bank.get(ParamId(9)), 0.0);
    }

    #[test]
    fn finite_or_filters_bad_input() {
        assert_eq!(finite_or(0.5, 1.0), 0.5);
        assert_eq!(finite_or(f32::NAN, 1.0), 1.0);
        assert_eq!(finite_or(f32::INFINITY, 1.0), 1.0);
    }
}
