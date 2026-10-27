//! Canonical descriptors for the controls owned by a mixer strip.

use super::{ParamDescriptor, ParamTarget, ParamUnit};

/// A built-in mixer control with no plugin parameter index.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MixerControl {
    /// A strip or master gain in decibels.
    Gain,
    /// A strip or master stereo position.
    Pan,
    /// A send level in decibels.
    Send,
}

impl MixerControl {
    /// Returns the canonical descriptor used by the session, graph inspector and native model.
    pub fn descriptor(self) -> ParamDescriptor {
        match self {
            Self::Gain => ParamDescriptor::decibels(0u32, "gain", "Volume", -60.0, 12.0, 0.0),
            Self::Pan => {
                ParamDescriptor::new(0u32, "pan", "Pan", -1.0, 1.0, 0.0).with_unit(ParamUnit::Pan)
            }
            Self::Send => ParamDescriptor::decibels(0u32, "send", "Send", -60.0, 0.0, 0.0),
        }
    }
}

/// Resolves a conventional parameter target to its canonical mixer descriptor.
///
/// Plugin targets return `None`; their descriptors come from the owning plugin instance.
pub fn mixer_descriptor(target: ParamTarget) -> Option<ParamDescriptor> {
    let control = match target {
        ParamTarget::TrackPan(_) | ParamTarget::MasterPan => MixerControl::Pan,
        ParamTarget::TrackGain(_) | ParamTarget::MasterGain => MixerControl::Gain,
        ParamTarget::Send { .. } => MixerControl::Send,
        ParamTarget::Instrument { .. } | ParamTarget::Effect { .. } => return None,
    };
    Some(control.descriptor())
}
