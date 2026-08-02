//! Assembling the plugin registry.

use std::sync::Arc;

use auris_core::PluginRegistry;
use auris_dsp::DspPack;
use auris_synth::SynthPack;

/// A registry holding every plugin that ships with Auris Studio.
///
/// This is the one place the built-in packs are named. Everything downstream — the engine, the
/// session, every frontend — works through [`PluginRegistry`] and never mentions a concrete
/// instrument or effect, which is what keeps adding one a matter of registering a factory.
pub fn default_registry() -> Arc<PluginRegistry> {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>();
    registry.install::<DspPack>();
    Arc::new(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_packs_are_installed() {
        let registry = default_registry();
        assert!(registry.has_instrument("auris.synth.chiptune"));
        assert!(registry.has_instrument("auris.synth.fm2"));
        assert!(registry.has_instrument("auris.synth.noisedrum"));
        for id in [
            "auris.fx.gain",
            "auris.fx.eq",
            "auris.fx.compressor",
            "auris.fx.delay",
            "auris.fx.reverb",
            "auris.fx.distortion",
            "auris.fx.limiter",
        ] {
            assert!(registry.has_effect(id), "{id} is not registered");
        }
    }

    #[test]
    fn the_default_instrument_is_the_chiptune_synth() {
        // New tracks take the first registered instrument, so which one that is matters.
        assert_eq!(
            default_registry().first_instrument_id(),
            Some("auris.synth.chiptune")
        );
    }
}
