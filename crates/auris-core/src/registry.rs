//! Runtime lookup table of available instruments and effects.
//!
//! The registry is the seam that keeps the engine and UI unaware of which plugins exist. A
//! project file stores plugin ids as strings; loading a project asks the registry to build the
//! matching objects. Adding a sound source is therefore a two-step job:
//!
//! ```ignore
//! // 1. Implement the trait.
//! impl Instrument for MyPolySynth { /* ... */ }
//!
//! // 2. Register a factory during start-up.
//! registry.register_instrument(|| Box::new(MyPolySynth::default()));
//! ```
//!
//! Groups of plugins can be bundled behind [`PluginPack`] so a whole crate installs itself
//! with one call.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::{CoreError, Result};
use crate::plugin::{Effect, Instrument, PluginCategory, PluginDescriptor, PluginKind};

/// Builds a fresh instrument instance.
pub type InstrumentFactory = Arc<dyn Fn() -> Box<dyn Instrument> + Send + Sync>;

/// Builds a fresh effect instance.
pub type EffectFactory = Arc<dyn Fn() -> Box<dyn Effect> + Send + Sync>;

/// A collection of plugins that installs itself into a registry.
///
/// Implemented by the `auris-synth` and `auris-dsp` crates so the application can pull in all
/// built-ins with `SynthPack::register(&mut registry)`.
pub trait PluginPack {
    /// Adds every plugin in this pack to `registry`.
    fn register(registry: &mut PluginRegistry);
}

struct InstrumentEntry {
    descriptor: PluginDescriptor,
    factory: InstrumentFactory,
}

struct EffectEntry {
    descriptor: PluginDescriptor,
    factory: EffectFactory,
}

/// Registry of everything that can be instantiated by id.
#[derive(Default)]
pub struct PluginRegistry {
    instruments: BTreeMap<String, InstrumentEntry>,
    effects: BTreeMap<String, EffectEntry>,
}

impl PluginRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an instrument factory.
    ///
    /// The descriptor is read from one throwaway instance, so a plugin declares its identity in
    /// exactly one place. Registering an id twice replaces the earlier entry, which lets a user
    /// pack deliberately override a built-in.
    pub fn register_instrument<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn Instrument> + Send + Sync + 'static,
    {
        let descriptor = factory().descriptor();
        debug_assert_eq!(
            descriptor.kind,
            PluginKind::Instrument,
            "instrument `{}` declared kind {:?}",
            descriptor.id,
            descriptor.kind
        );
        self.instruments.insert(
            descriptor.id.to_string(),
            InstrumentEntry {
                descriptor,
                factory: Arc::new(factory),
            },
        );
    }

    /// Registers an effect factory. See [`Self::register_instrument`] for the semantics.
    pub fn register_effect<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn Effect> + Send + Sync + 'static,
    {
        let descriptor = factory().descriptor();
        debug_assert_eq!(
            descriptor.kind,
            PluginKind::Effect,
            "effect `{}` declared kind {:?}",
            descriptor.id,
            descriptor.kind
        );
        self.effects.insert(
            descriptor.id.to_string(),
            EffectEntry {
                descriptor,
                factory: Arc::new(factory),
            },
        );
    }

    /// Installs a whole pack of plugins.
    pub fn install<P: PluginPack>(&mut self) -> &mut Self {
        P::register(self);
        self
    }

    /// Creates an instrument by id.
    pub fn create_instrument(&self, id: &str) -> Result<Box<dyn Instrument>> {
        self.instruments
            .get(id)
            .map(|entry| (entry.factory)())
            .ok_or_else(|| CoreError::UnknownPlugin(id.to_string()))
    }

    /// Creates an effect by id.
    pub fn create_effect(&self, id: &str) -> Result<Box<dyn Effect>> {
        self.effects
            .get(id)
            .map(|entry| (entry.factory)())
            .ok_or_else(|| CoreError::UnknownPlugin(id.to_string()))
    }

    /// Descriptors of every registered instrument, ordered by id.
    pub fn instruments(&self) -> impl Iterator<Item = &PluginDescriptor> {
        self.instruments.values().map(|entry| &entry.descriptor)
    }

    /// Descriptors of every registered effect, ordered by id.
    pub fn effects(&self) -> impl Iterator<Item = &PluginDescriptor> {
        self.effects.values().map(|entry| &entry.descriptor)
    }

    /// Descriptor for one plugin, whichever kind it is.
    pub fn descriptor(&self, id: &str) -> Option<&PluginDescriptor> {
        self.instruments
            .get(id)
            .map(|entry| &entry.descriptor)
            .or_else(|| self.effects.get(id).map(|entry| &entry.descriptor))
    }

    /// Effects in one category, ordered by id.
    pub fn effects_in_category(
        &self,
        category: PluginCategory,
    ) -> impl Iterator<Item = &PluginDescriptor> {
        self.effects().filter(move |d| d.category == category)
    }

    /// `true` when an instrument with this id is registered.
    pub fn has_instrument(&self, id: &str) -> bool {
        self.instruments.contains_key(id)
    }

    /// `true` when an effect with this id is registered.
    pub fn has_effect(&self, id: &str) -> bool {
        self.effects.contains_key(id)
    }

    /// Id of the first registered instrument, used as the default for new tracks.
    pub fn first_instrument_id(&self) -> Option<&str> {
        self.instruments.keys().next().map(String::as_str)
    }

    /// Total number of registered plugins.
    pub fn len(&self) -> usize {
        self.instruments.len() + self.effects.len()
    }

    /// `true` when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("instruments", &self.instruments.keys().collect::<Vec<_>>())
            .field("effects", &self.effects.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::AudioBuffer;
    use crate::param::{ParamDescriptor, ParamId};
    use crate::plugin::{NoteEvent, Parameterized, PrepareContext, ProcessContext};

    #[derive(Default)]
    struct Silence;

    impl Parameterized for Silence {
        fn parameters(&self) -> &[ParamDescriptor] {
            &[]
        }
        fn param(&self, _id: ParamId) -> f32 {
            0.0
        }
        fn set_param(&mut self, _id: ParamId, _value: f32) {}
    }

    impl Instrument for Silence {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor::instrument(
                "test.silence",
                "Silence",
                "Generates nothing",
                PluginCategory::Synth,
            )
        }
        fn prepare(&mut self, _ctx: &PrepareContext) {}
        fn reset(&mut self) {}
        fn process(&mut self, _events: &[NoteEvent], out: &mut AudioBuffer, _ctx: &ProcessContext) {
            out.clear();
        }
    }

    #[test]
    fn registered_instruments_can_be_created_by_id() {
        let mut registry = PluginRegistry::new();
        registry.register_instrument(|| Box::new(Silence));

        assert!(registry.has_instrument("test.silence"));
        assert_eq!(registry.first_instrument_id(), Some("test.silence"));
        assert!(registry.create_instrument("test.silence").is_ok());
        assert!(registry.create_instrument("nope").is_err());
        assert_eq!(
            registry.descriptor("test.silence").map(|d| d.name.as_ref()),
            Some("Silence")
        );
    }

    #[test]
    fn registering_the_same_id_replaces_the_entry() {
        let mut registry = PluginRegistry::new();
        registry.register_instrument(|| Box::new(Silence));
        registry.register_instrument(|| Box::new(Silence));
        assert_eq!(registry.len(), 1);
    }
}
