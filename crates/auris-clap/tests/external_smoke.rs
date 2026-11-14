use std::path::PathBuf;

use auris_clap::ClapLibrary;
use auris_core::{
    AudioBuffer, Effect, Instrument, NoteEvent, PluginKind, PrepareContext, ProcessContext,
};

#[test]
#[ignore = "requires AURIS_CLAP_EFFECT_SMOKE_PLUGIN to point to an external CLAP effect"]
fn an_external_effect_restores_state_and_processes_audio() {
    let path = PathBuf::from(
        std::env::var_os("AURIS_CLAP_EFFECT_SMOKE_PLUGIN")
            .expect("AURIS_CLAP_EFFECT_SMOKE_PLUGIN must name a CLAP effect"),
    );
    // SAFETY: the person running this ignored test explicitly selected the library.
    let library = unsafe { ClapLibrary::load(path) }.expect("the library should load");
    let info = library
        .plugins()
        .unwrap()
        .into_iter()
        .find(|info| info.kind == PluginKind::Effect)
        .expect("the library should contain an effect");
    eprintln!("testing {} {} ({})", info.vendor, info.name, info.clap_id);
    let mut plugin = library.instantiate(&info.clap_id).unwrap();
    let state = plugin.save_state().expect("effect state should save");
    plugin
        .load_state(&state)
        .expect("effect state should restore");
    let prepare = PrepareContext::new(48_000.0, 512, 2);
    let mut effect = plugin.activate(&prepare).expect("effect should activate");
    let mut out = AudioBuffer::stereo(512, 48_000.0);
    let mut peak = 0.0_f32;
    for block in 0..64 {
        for channel in out.iter_channels_mut() {
            for (frame, sample) in channel.iter_mut().enumerate() {
                let index = block * 512 + frame;
                *sample = (std::f32::consts::TAU * 440.0 * index as f32 / 48_000.0).sin() * 0.1;
            }
        }
        let context = ProcessContext::realtime(48_000.0, 512, (block * 512) as u64, 120.0, true);
        effect.process(&mut out, &context);
        assert!(
            out.iter_channels()
                .flatten()
                .all(|sample| sample.is_finite())
        );
        peak = peak.max(out.peak());
    }
    assert!(!effect.processing_failed(), "the effect renderer failed");
    assert!(
        peak > 1.0e-5,
        "the effect returned no audio for a continuous input"
    );
    eprintln!("effect output peak: {peak:.6}");
    plugin.deactivate(effect);
}

#[test]
#[ignore = "requires AURIS_CLAP_SMOKE_PLUGIN to point to an external CLAP instrument"]
fn stopping_an_external_instrument_releases_held_notes() {
    let path = PathBuf::from(
        std::env::var_os("AURIS_CLAP_SMOKE_PLUGIN")
            .expect("AURIS_CLAP_SMOKE_PLUGIN must name a CLAP instrument"),
    );
    // SAFETY: the person running this ignored test explicitly selected the library.
    let library = unsafe { ClapLibrary::load(path) }.expect("the library should load");
    let info = library
        .plugins()
        .unwrap()
        .into_iter()
        .find(|info| info.kind == PluginKind::Instrument)
        .expect("the library should contain an instrument");
    let mut plugin = library.instantiate(&info.clap_id).unwrap();
    let prepare = PrepareContext::new(48_000.0, 512, 2);
    let mut instrument = plugin.activate_instrument(&prepare).unwrap();
    let mut out = AudioBuffer::stereo(512, 48_000.0);
    let playing = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
    for _ in 0..16 {
        instrument.process(&[], &mut out, &playing);
    }
    instrument.process(&[NoteEvent::note_on(0, 60, 0.8)], &mut out, &playing);
    let mut peak = out.peak();
    for _ in 0..64 {
        instrument.process(&[], &mut out, &playing);
        peak = peak.max(out.peak());
    }
    assert!(peak > 1.0e-5, "the instrument must sound before stopping");

    instrument.reset();
    instrument.reset(); // Stop followed by returning the playhead to zero.
    let stopped = ProcessContext {
        is_playing: false,
        ..playing
    };
    for _ in 0..1_000 {
        instrument.process(&[], &mut out, &stopped);
    }
    assert!(!instrument.processing_failed());
    assert!(
        out.peak() < 1.0e-5,
        "{} kept sounding after stop: {}",
        info.name,
        out.peak()
    );

    // Seeking while playing must let the newly chased note sound after the reset.
    instrument.reset();
    instrument.process(&[NoteEvent::note_on(0, 64, 0.8)], &mut out, &playing);
    let mut peak = out.peak();
    for _ in 0..64 {
        instrument.process(&[], &mut out, &playing);
        peak = peak.max(out.peak());
    }
    assert!(peak > 1.0e-5, "the new note must survive the pending reset");
    plugin.deactivate_instrument(instrument);
}
