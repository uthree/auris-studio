use std::path::PathBuf;

use auris_core::buffer::AudioBuffer;
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, PluginKind, PrepareContext, ProcessContext,
};
use auris_vst3::{Vst3Plugin, plugins_in};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK_FRAMES: usize = 512;

#[test]
#[ignore = "requires AURIS_VST3_MONO_EFFECT_SMOKE_PLUGIN to name an installed mono-output effect"]
fn a_mono_external_effect_replaces_both_sides_of_a_stereo_track() {
    let path = PathBuf::from(
        std::env::var_os("AURIS_VST3_MONO_EFFECT_SMOKE_PLUGIN")
            .expect("a mono-output VST3 effect is required"),
    );
    let info = plugins_in(&path)
        .unwrap()
        .into_iter()
        .find(|info| info.kind == PluginKind::Effect)
        .expect("the bundle must contain an effect");
    let mut host = vst3_host::Vst3Host::builder()
        .sample_rate(SAMPLE_RATE)
        .block_size(BLOCK_FRAMES)
        .build()
        .unwrap();
    let probe = host.load_plugin_class(&path, &info.class_id).unwrap();
    let layout = probe.audio_bus_layout().unwrap();
    eprintln!("{} audio layout: {layout:?}", info.name);
    assert_eq!(layout.outputs[0].channel_count, 1, "a mono main output");
    drop(probe);

    let prepare = PrepareContext::new(SAMPLE_RATE, BLOCK_FRAMES, 2);
    let plugin = Vst3Plugin::load(&path, &info.class_id, &prepare).unwrap();
    let mut effect = plugin.effect().unwrap();
    effect.prepare(&prepare);
    let mut buffer = AudioBuffer::stereo(BLOCK_FRAMES, SAMPLE_RATE);
    for block in 0..16 {
        for channel in buffer.iter_channels_mut() {
            for (frame, sample) in channel.iter_mut().enumerate() {
                let index = block * BLOCK_FRAMES + frame;
                *sample =
                    (std::f32::consts::TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin() * 0.1;
            }
        }
        effect.process(
            &mut buffer,
            &ProcessContext::realtime(
                SAMPLE_RATE,
                BLOCK_FRAMES,
                (block * BLOCK_FRAMES) as u64,
                120.0,
                true,
            ),
        );
        assert!(!effect.processing_failed());
        assert_finite(&buffer);
        assert_eq!(
            buffer.channel(0),
            buffer.channel(1),
            "both sides must contain the mono result, including during plugin latency"
        );
        std::thread::sleep(std::time::Duration::from_millis(11));
    }
}

#[test]
#[ignore = "requires AURIS_VST3_SMOKE_PLUGIN to point to an external VST3 bundle"]
fn an_external_vst3_can_be_discovered_loaded_and_processed() {
    let path = PathBuf::from(
        std::env::var_os("AURIS_VST3_SMOKE_PLUGIN")
            .expect("AURIS_VST3_SMOKE_PLUGIN must name a VST3 bundle"),
    );
    let classes = plugins_in(&path).expect("the VST3 bundle should be discoverable");
    assert!(
        !classes.is_empty(),
        "the bundle should expose an audio class"
    );

    for info in classes {
        eprintln!(
            "smoke testing {} {} ({}) as {:?}",
            info.vendor, info.name, info.class_id, info.kind
        );
        let prepare = PrepareContext::new(SAMPLE_RATE, BLOCK_FRAMES, 2);
        let plugin = Vst3Plugin::load(&path, &info.class_id, &prepare)
            .expect("the discovered VST3 class should load");

        let state = plugin.save_state().expect("plugin state should save");
        plugin
            .load_state(&state)
            .expect("saved plugin state should restore");
        if let Some(parameter) = plugin.parameters().first() {
            let value = plugin
                .value(parameter.id)
                .expect("a listed parameter should have a value");
            plugin
                .set_param(parameter.id, value)
                .expect("a listed parameter should accept its current value");
        }

        match info.kind {
            PluginKind::Instrument => smoke_instrument(&plugin),
            PluginKind::Effect => smoke_effect(&plugin),
        }
    }
}

fn smoke_instrument(plugin: &Vst3Plugin) {
    let offline = std::env::var_os("AURIS_VST3_SMOKE_OFFLINE").is_some();
    let mut instrument = if offline {
        plugin.offline_instrument()
    } else {
        plugin.instrument()
    }
    .expect("the instrument render adapter should start");
    instrument.prepare(&PrepareContext::new(SAMPLE_RATE, BLOCK_FRAMES, 2));
    let mut buffer = AudioBuffer::stereo(BLOCK_FRAMES, SAMPLE_RATE);
    let mut peak = 0.0_f32;
    for block in 0..64 {
        // Duplicate voices at one pitch require one tracked release each.
        let note_on = [
            NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            },
            NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.5,
            },
        ];
        let events = if block == 0 { &note_on[..] } else { &[] };
        let context = ProcessContext {
            is_offline: offline,
            ..ProcessContext::realtime(
                SAMPLE_RATE,
                BLOCK_FRAMES,
                (block * BLOCK_FRAMES) as u64,
                120.0,
                true,
            )
        };
        instrument.process(events, &mut buffer, &context);
        assert_finite(&buffer);
        peak = peak.max(
            buffer
                .iter_channels()
                .flatten()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs())),
        );
        if !offline {
            std::thread::sleep(std::time::Duration::from_millis(11));
        }
    }
    assert!(
        peak > 1.0e-6,
        "the instrument should produce audible output (peak={peak}, processing_failed={})",
        instrument.processing_failed()
    );
    instrument.reset();
    let context = ProcessContext::realtime(
        SAMPLE_RATE,
        BLOCK_FRAMES,
        64 * BLOCK_FRAMES as u64,
        120.0,
        false,
    );
    instrument.process(&[], &mut buffer, &context);
    assert_finite(&buffer);
    assert!(
        !instrument.processing_failed(),
        "reset and its zero-frame release flush must fit the realtime queues"
    );

    instrument.process(
        &[NoteEvent::NoteOn {
            frame: 0,
            pitch: 64,
            velocity: 0.8,
        }],
        &mut buffer,
        &context,
    );
    instrument.process(
        &[NoteEvent::AllNotesOff { frame: 0 }],
        &mut buffer,
        &context,
    );
    assert!(
        !instrument.processing_failed(),
        "a discontinuity marker must use the same bounded release flush"
    );
}

fn smoke_effect(plugin: &Vst3Plugin) {
    let offline = std::env::var_os("AURIS_VST3_SMOKE_OFFLINE").is_some();
    let mut effect = if offline {
        plugin.offline_effect()
    } else {
        plugin.effect()
    }
    .expect("the effect render adapter should start");
    effect.prepare(&PrepareContext::new(SAMPLE_RATE, BLOCK_FRAMES, 2));
    let mut buffer = AudioBuffer::stereo(BLOCK_FRAMES, SAMPLE_RATE);
    let mut peak = 0.0_f32;
    for block in 0..128 {
        for channel in buffer.iter_channels_mut() {
            for (frame, sample) in channel.iter_mut().enumerate() {
                let index = block * BLOCK_FRAMES + frame;
                *sample =
                    (std::f32::consts::TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin() * 0.1;
            }
        }
        let context = ProcessContext {
            is_offline: offline,
            ..ProcessContext::realtime(
                SAMPLE_RATE,
                BLOCK_FRAMES,
                (block * BLOCK_FRAMES) as u64,
                120.0,
                true,
            )
        };
        effect.process(&mut buffer, &context);
        assert_finite(&buffer);
        peak = peak.max(
            buffer
                .iter_channels()
                .flatten()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs())),
        );
        if !offline {
            std::thread::sleep(std::time::Duration::from_millis(11));
        }
    }
    let context = ProcessContext::realtime(SAMPLE_RATE, BLOCK_FRAMES, 0, 120.0, true);
    assert!(
        peak > 1.0e-6,
        "the effect should return audio for an audible input (peak={peak}, processing_failed={})",
        effect.processing_failed()
    );

    effect.reset();
    buffer.clear();
    effect.process(&mut buffer, &context);
    assert_finite(&buffer);
    assert!(
        !effect.processing_failed(),
        "effect reset must restart the realtime processor without stopping the renderer"
    );
}

fn assert_finite(buffer: &AudioBuffer) {
    assert!(
        buffer
            .iter_channels()
            .flatten()
            .all(|sample| sample.is_finite()),
        "the plugin should not return NaN or infinity"
    );
}
