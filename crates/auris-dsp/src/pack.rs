//! Registration of every effect in this crate.

use auris_core::registry::{PluginPack, PluginRegistry};

use crate::compressor::Compressor;
use crate::delay::Delay;
use crate::distortion::Distortion;
use crate::eq::Equalizer;
use crate::gain::GainPan;
use crate::limiter::Limiter;
use crate::reverb::Reverb;

/// Installs the built-in effects into a [`PluginRegistry`].
///
/// ```
/// use auris_core::registry::PluginRegistry;
/// use auris_dsp::DspPack;
///
/// let mut registry = PluginRegistry::new();
/// registry.install::<DspPack>();
/// assert!(registry.has_effect("auris.fx.reverb"));
/// ```
pub struct DspPack;

impl PluginPack for DspPack {
    fn register(registry: &mut PluginRegistry) {
        registry.register_effect(|| Box::new(GainPan::new()));
        registry.register_effect(|| Box::new(Equalizer::new()));
        registry.register_effect(|| Box::new(Compressor::new()));
        registry.register_effect(|| Box::new(Delay::new()));
        registry.register_effect(|| Box::new(Reverb::new()));
        registry.register_effect(|| Box::new(Distortion::new()));
        registry.register_effect(|| Box::new(Limiter::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::AudioBuffer;
    use auris_core::plugin::{Effect, PluginKind, PrepareContext, ProcessContext};

    const SR: f64 = 48_000.0;
    const FRAMES: usize = 2_048;

    const EXPECTED_IDS: [&str; 7] = [
        "auris.fx.compressor",
        "auris.fx.delay",
        "auris.fx.distortion",
        "auris.fx.eq",
        "auris.fx.gain",
        "auris.fx.limiter",
        "auris.fx.reverb",
    ];

    fn registry() -> PluginRegistry {
        let mut registry = PluginRegistry::new();
        registry.install::<DspPack>();
        registry
    }

    fn every_effect() -> Vec<(String, Box<dyn Effect>)> {
        let registry = registry();
        EXPECTED_IDS
            .iter()
            .map(|id| ((*id).to_string(), registry.create_effect(id).unwrap()))
            .collect()
    }

    fn context() -> ProcessContext {
        ProcessContext::realtime(SR, FRAMES, 0, 120.0, true)
    }

    #[test]
    fn the_pack_registers_all_seven_effects() {
        let registry = registry();
        assert_eq!(registry.len(), 7);
        let ids: Vec<&str> = registry.effects().map(|d| d.id.as_ref()).collect();
        assert_eq!(ids, EXPECTED_IDS);
        for descriptor in registry.effects() {
            assert_eq!(descriptor.kind, PluginKind::Effect);
            assert!(!descriptor.name.is_empty());
        }
    }

    #[test]
    fn every_effect_turns_silence_into_silence() {
        for (id, mut effect) in every_effect() {
            effect.prepare(&PrepareContext::new(SR, FRAMES, 2));
            let mut buffer = AudioBuffer::stereo(FRAMES, SR);
            for _ in 0..4 {
                effect.process(&mut buffer, &context());
            }
            assert_eq!(buffer.peak(), 0.0, "{id} produced output from silence");
        }
    }

    #[test]
    fn every_effect_keeps_a_signal_finite_and_bounded() {
        let step = std::f32::consts::TAU * 440.0 / SR as f32;
        for (id, mut effect) in every_effect() {
            effect.prepare(&PrepareContext::new(SR, FRAMES, 2));
            for block in 0..32 {
                let mut buffer = AudioBuffer::stereo(FRAMES, SR);
                for channel in buffer.channels_mut() {
                    for (n, sample) in channel.iter_mut().enumerate() {
                        let phase = step * (block * FRAMES + n) as f32;
                        *sample = phase.sin() * 0.7;
                    }
                }
                effect.process(&mut buffer, &context());
                assert!(
                    buffer.channels().iter().flatten().all(|s| s.is_finite()),
                    "{id} produced a non-finite sample in block {block}"
                );
                assert!(
                    buffer.peak() < 32.0,
                    "{id} peaked at {} in block {block}",
                    buffer.peak()
                );
            }
        }
    }

    #[test]
    fn every_effect_round_trips_its_state() {
        for (id, mut effect) in every_effect() {
            // The value each parameter should end up holding: 70 % of the way up its range,
            // put through the descriptor's own clamp so toggles and choices land on a step.
            let wanted: Vec<(String, f32)> = effect
                .parameters()
                .iter()
                .map(|d| (d.key.to_string(), d.clamp(d.min + (d.max - d.min) * 0.7)))
                .collect();
            for (key, value) in &wanted {
                assert!(effect.set_param_by_key(key, *value), "{id}/{key} unknown");
            }
            // Comparing the two instances alone would pass even if `set_param` did nothing at
            // all, since both would then be sitting on their defaults.
            for (key, value) in &wanted {
                assert_eq!(
                    effect.param_by_key(key),
                    Some(*value),
                    "{id}/{key} did not take the value it was given"
                );
            }

            let saved = effect.save_state();
            assert_eq!(saved.params.len(), wanted.len(), "{id}");

            let mut restored = registry().create_effect(&id).unwrap();
            restored.load_state(&saved);
            for (key, value) in &wanted {
                assert_eq!(
                    restored.param_by_key(key),
                    Some(*value),
                    "{id}/{key} did not round trip"
                );
            }
        }
    }

    #[test]
    fn every_effect_is_independent_of_the_block_size() {
        // The host is free to change the block size between callbacks, and the offline renderer
        // uses a different one from the device. Any state an effect carries per block rather
        // than per sample — a ramp restarted at the block boundary, a detector reset, a filter
        // flushed — shows up here as a mismatch against the single-block reference.
        const TOTAL: usize = 4_096;

        fn material(frames: usize) -> AudioBuffer {
            let mut buffer = AudioBuffer::stereo(frames, SR);
            for (channel, samples) in buffer.channels_mut().iter_mut().enumerate() {
                for (n, sample) in samples.iter_mut().enumerate() {
                    let t = n as f32 / SR as f32;
                    let detune = 220.0 + 50.0 * channel as f32;
                    *sample = (std::f32::consts::TAU * detune * t).sin() * 0.6
                        + (std::f32::consts::TAU * 3_000.0 * t).sin() * 0.2;
                }
            }
            buffer
        }

        for (id, mut whole) in every_effect() {
            whole.prepare(&PrepareContext::new(SR, TOTAL, 2));
            let mut reference = material(TOTAL);
            whole.process(
                &mut reference,
                &ProcessContext::realtime(SR, TOTAL, 0, 120.0, true),
            );

            for block_frames in [1usize, 7, 64, 513] {
                let mut split = registry().create_effect(&id).unwrap();
                split.prepare(&PrepareContext::new(SR, TOTAL, 2));

                let source = material(TOTAL);
                let mut collected = AudioBuffer::stereo(TOTAL, SR);
                let mut scratch = AudioBuffer::stereo(0, SR);
                let mut start = 0;
                while start < TOTAL {
                    let frames = block_frames.min(TOTAL - start);
                    scratch.set_frame_count(frames);
                    for channel in 0..2 {
                        scratch
                            .channel_mut(channel)
                            .copy_from_slice(&source.channel(channel)[start..start + frames]);
                    }
                    split.process(
                        &mut scratch,
                        &ProcessContext::realtime(SR, frames, start as u64, 120.0, true),
                    );
                    for channel in 0..2 {
                        collected.channel_mut(channel)[start..start + frames]
                            .copy_from_slice(scratch.channel(channel));
                    }
                    start += frames;
                }

                for channel in 0..2 {
                    for frame in 0..TOTAL {
                        let expected = reference.channel(channel)[frame];
                        let actual = collected.channel(channel)[frame];
                        assert!(
                            (expected - actual).abs() < 1.0e-4,
                            "{id} in blocks of {block_frames}: channel {channel} frame {frame} \
                             was {actual}, one block gave {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_effect_tolerates_a_channel_count_prepare_did_not_expect() {
        // The engine is supposed to prepare an effect with the channel count it will hand to
        // `process`, but a mismatch must degrade rather than panic or read out of bounds.
        // Effects with per-channel state process `min(prepared, actual)` channels and leave the
        // rest untouched; nothing may index past either end.
        for (prepared, actual) in [(1usize, 4usize), (6, 2), (2, 1), (1, 2), (6, 1)] {
            for (id, mut effect) in every_effect() {
                effect.prepare(&PrepareContext::new(SR, FRAMES, prepared));
                let mut buffer = AudioBuffer::new(actual, FRAMES, SR);
                for channel in buffer.channels_mut() {
                    for (n, sample) in channel.iter_mut().enumerate() {
                        *sample = ((n % 64) as f32 / 32.0) - 1.0;
                    }
                }
                effect.process(&mut buffer, &context());
                assert!(
                    buffer.channels().iter().flatten().all(|s| s.is_finite()),
                    "{id} prepared for {prepared} channels broke on {actual}"
                );
            }
        }
    }

    #[test]
    fn every_effect_survives_being_processed_before_prepare() {
        // `Effect::prepare` is documented as being called before rendering, but a UI that audits
        // a freshly created plugin must not be able to crash the process by skipping it.
        for (id, mut effect) in every_effect() {
            let mut buffer = AudioBuffer::stereo(64, SR);
            for channel in buffer.channels_mut() {
                for (n, sample) in channel.iter_mut().enumerate() {
                    *sample = (n as f32 * 0.05).sin() * 0.5;
                }
            }
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(SR, 64, 0, 120.0, true),
            );
            assert!(
                buffer.channels().iter().flatten().all(|s| s.is_finite()),
                "{id} misbehaved without a prepare"
            );

            let mut empty = AudioBuffer::stereo(0, SR);
            effect.process(&mut empty, &ProcessContext::realtime(SR, 0, 0, 120.0, true));
        }
    }

    #[test]
    fn every_effect_survives_mono_and_surround_buffers() {
        for channel_count in [1usize, 2, 6] {
            for (id, mut effect) in every_effect() {
                effect.prepare(&PrepareContext::new(SR, FRAMES, channel_count));
                let mut buffer = AudioBuffer::new(channel_count, FRAMES, SR);
                for channel in buffer.channels_mut() {
                    for (n, sample) in channel.iter_mut().enumerate() {
                        *sample = ((n % 64) as f32 / 32.0) - 1.0;
                    }
                }
                effect.process(&mut buffer, &context());
                assert!(
                    buffer.channels().iter().flatten().all(|s| s.is_finite()),
                    "{id} broke on {channel_count} channels"
                );
            }
        }
    }

    #[test]
    fn only_the_time_based_effects_report_a_tail() {
        let registry = registry();
        for id in ["auris.fx.gain", "auris.fx.eq", "auris.fx.distortion"] {
            let mut effect = registry.create_effect(id).unwrap();
            effect.prepare(&PrepareContext::new(SR, FRAMES, 2));
            assert_eq!(effect.tail_frames(), 0, "{id}");
            assert_eq!(effect.latency_frames(), 0, "{id}");
        }
        for id in ["auris.fx.delay", "auris.fx.reverb"] {
            let mut effect = registry.create_effect(id).unwrap();
            effect.prepare(&PrepareContext::new(SR, FRAMES, 2));
            assert!(effect.tail_frames() > 4_800, "{id} reported no tail");
            assert_eq!(effect.latency_frames(), 0, "{id}");
        }
    }
}
