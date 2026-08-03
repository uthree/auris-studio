//! Assembling the plugin registry.

use std::sync::Arc;

use auris_core::PluginRegistry;
use auris_dsp::DspPack;
use auris_sampler::{SharedSoundFonts, SoundFontBank, register_sampler};
use auris_synth::SynthPack;

/// What a new track plays until somebody chooses otherwise.
pub const DEFAULT_INSTRUMENT: &str = "auris.synth.chiptune";

/// A registry holding every plugin that ships with Auris Studio.
///
/// This is the one place the built-in packs are named. Everything downstream — the engine, the
/// session, every frontend — works through [`PluginRegistry`] and never mentions a concrete
/// instrument or effect, which is what keeps adding one a matter of registering a factory.
///
/// `fonts` is the bank the sampler will read SoundFonts from. It is a parameter rather than
/// something built here because the session has to fill it — a font is loaded by a command, long
/// after the registry exists — and because every sampler must read the *same* one.
pub fn default_registry(fonts: SharedSoundFonts) -> Arc<PluginRegistry> {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>();
    registry.install::<DspPack>();
    register_sampler(&mut registry, fonts);
    // A new track has to make a sound. The sampler sorts first by id and would otherwise win by
    // accident, and a sampler with no font imported yet is silence.
    registry.set_default_instrument(DEFAULT_INSTRUMENT);
    Arc::new(registry)
}

/// A registry with a font bank nobody else can fill.
///
/// For callers that only want to ask what exists — a plugin listing, a set of parameter
/// descriptors — where a sampler that can never find a font is exactly as informative as one
/// that could.
pub fn plugin_catalogue() -> Arc<PluginRegistry> {
    default_registry(SoundFontBank::shared())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_packs_are_installed() {
        let registry = plugin_catalogue();
        assert!(registry.has_instrument("auris.synth.chiptune"));
        assert!(registry.has_instrument("auris.synth.fm2"));
        assert!(registry.has_instrument("auris.synth.noisedrum"));
        assert!(registry.has_instrument(auris_sampler::SAMPLER_ID));
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
        // New tracks take this, so which one it is matters — and it must not be the sampler,
        // which sorts first by id and makes no sound at all until a font has been imported.
        let registry = plugin_catalogue();
        assert_eq!(registry.default_instrument_id(), Some(DEFAULT_INSTRUMENT));
        assert_ne!(
            registry.default_instrument_id(),
            Some(auris_sampler::SAMPLER_ID)
        );
    }
}
