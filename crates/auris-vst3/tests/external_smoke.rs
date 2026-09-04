use std::path::PathBuf;

use auris_core::buffer::AudioBuffer;
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, PluginKind, PrepareContext, ProcessContext,
};
use auris_vst3::{Vst3Plugin, plugins_in};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK_FRAMES: usize = 512;

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
    let mut instrument = plugin
        .instrument()
        .expect("the instrument render adapter should start");
    let mut buffer = AudioBuffer::stereo(BLOCK_FRAMES, SAMPLE_RATE);
    let mut peak = 0.0_f32;
    for block in 0..64 {
        let note_on = [NoteEvent::NoteOn {
            frame: 0,
            pitch: 60,
            velocity: 0.8,
        }];
        let events = if block == 0 { &note_on[..] } else { &[] };
        let context = ProcessContext::realtime(
            SAMPLE_RATE,
            BLOCK_FRAMES,
            (block * BLOCK_FRAMES) as u64,
            120.0,
            true,
        );
        instrument.process(events, &mut buffer, &context);
        assert_finite(&buffer);
        peak = peak.max(
            buffer
                .iter_channels()
                .flatten()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs())),
        );
    }
    assert!(
        peak > 1.0e-6,
        "the instrument should produce audible output"
    );
}

fn smoke_effect(plugin: &Vst3Plugin) {
    let mut effect = plugin
        .effect()
        .expect("the effect render adapter should start");
    let mut buffer = AudioBuffer::stereo(BLOCK_FRAMES, SAMPLE_RATE);
    for channel in buffer.iter_channels_mut() {
        for (frame, sample) in channel.iter_mut().enumerate() {
            *sample =
                (std::f32::consts::TAU * 440.0 * frame as f32 / SAMPLE_RATE as f32).sin() * 0.1;
        }
    }
    let context = ProcessContext::realtime(SAMPLE_RATE, BLOCK_FRAMES, 0, 120.0, true);
    effect.process(&mut buffer, &context);
    assert_finite(&buffer);
    assert!(
        buffer
            .iter_channels()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6),
        "the effect should return audio for an audible input"
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
