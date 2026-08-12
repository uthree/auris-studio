//! Hosting tests, run against the [`testkit`](crate::testkit) fixture.

use auris_core::buffer::AudioBuffer;
use auris_core::param::ParamId;
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginKind, PrepareContext, ProcessContext,
};

use crate::library::ClapLibrary;
use crate::plugin::ClapPlugin;
use crate::testkit::{FIXTURE_ID, INPUT_PORTS, fixture_library};

fn library() -> ClapLibrary {
    fixture_library()
}

fn hosted() -> ClapPlugin {
    library()
        .instantiate(FIXTURE_ID)
        .expect("the fixture plugin must instantiate")
}

fn context() -> PrepareContext {
    PrepareContext::new(48_000.0, 64, 2)
}

fn block(level: f32, frames: usize) -> AudioBuffer {
    let mut buffer = AudioBuffer::stereo(frames, 48_000.0);
    for channel in 0..buffer.channel_count() {
        buffer.channel_mut(channel).fill(level);
    }
    buffer
}

fn playing(frames: usize) -> ProcessContext {
    ProcessContext::realtime(48_000.0, frames, 0, 120.0, true)
}

#[test]
fn a_file_lists_what_is_inside_it() {
    let plugins = library().plugins().expect("the factory must be readable");
    assert_eq!(plugins.len(), 1);

    let info = &plugins[0];
    assert_eq!(info.clap_id, FIXTURE_ID);
    assert_eq!(info.name, "Test Gain");
    assert_eq!(info.vendor, "Auris Studio");
    assert_eq!(info.kind, PluginKind::Effect);
    assert_eq!(info.category, PluginCategory::Utility);
    assert_eq!(info.auris_id(), "clap:studio.auris.test.gain");
}

#[test]
fn a_plugin_that_is_not_in_the_file_is_an_error_not_a_panic() {
    let error = library().instantiate("com.example.nothing").unwrap_err();
    assert!(
        error.to_string().contains("com.example.nothing"),
        "the message should name what was asked for, got: {error}"
    );
}

#[test]
fn a_hosted_plugin_reports_its_parameters_by_the_plugins_own_id() {
    let plugin = hosted();
    let params = plugin.parameters();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].id, ParamId(0), "the slice position");
    assert_eq!(params[0].key, "clap.4242", "the plugin's own id");
    assert_eq!(params[0].name, "Gain");
    assert_eq!(params[0].min, 0.0);
    assert_eq!(params[0].max, 2.0);
    assert_eq!(params[0].default, 1.0);
    assert_eq!(params[1].key, "clap.4343", "and the second keeps its own");
}

#[test]
fn every_port_the_plugin_declared_is_handed_to_it() {
    // The fixture declares a sidechain it never reads, for the reason its module doc gives: a
    // host that passes only the ports it has audio for hands the plugin an array shorter than the
    // one the plugin was told it could index, and the plugin has no way to find that out in time.
    let mut plugin = hosted();
    let ports = plugin.ports(2);
    assert_eq!(ports.inputs, vec![2, 2], "a main port and a sidechain");
    assert_eq!(ports.outputs, vec![2]);
    assert_eq!(ports.main_input, Some(0));
    assert_eq!(ports.main_output, Some(0));

    let mut effect = plugin.activate(&context()).expect("must activate");
    effect.process(&mut block(0.5, 32), &playing(32));
    plugin.deactivate(effect);

    assert_eq!(
        plugin.value(ParamId(1)),
        Some(INPUT_PORTS as f32),
        "the plugin has to receive as many input ports as it asked for"
    );
}

#[test]
fn a_plugin_with_a_sidechain_still_renders_through_its_main_port() {
    // The sidechain is silent, and the audio has to come back out of the *main* output rather
    // than out of whichever port happened to be first in the list.
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    let mut buffer = block(0.5, 32);
    effect.process(&mut buffer, &playing(32));
    assert_eq!(buffer.channel(0)[0], 0.5);
    assert_eq!(buffer.channel(1)[31], 0.5);

    plugin.deactivate(effect);
}

#[test]
fn a_hosted_effect_renders_through_the_plugin() {
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    let mut buffer = block(0.5, 32);
    effect.process(&mut buffer, &playing(32));

    // The fixture multiplies by its gain, which defaults to 1.0.
    assert_eq!(buffer.channel(0)[0], 0.5);
    assert_eq!(buffer.channel(1)[31], 0.5);

    plugin.deactivate(effect);
}

#[test]
fn a_parameter_written_on_the_audio_side_reaches_the_plugin() {
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    effect.set_param(ParamId(0), 2.0);
    assert_eq!(effect.param(ParamId(0)), 2.0, "the host's own shadow copy");

    let mut buffer = block(0.25, 16);
    effect.process(&mut buffer, &playing(16));
    assert_eq!(
        buffer.channel(0)[0],
        0.5,
        "the event must have landed in the same block that rendered"
    );

    plugin.deactivate(effect);
    assert_eq!(
        plugin.value(ParamId(0)),
        Some(2.0),
        "and the main thread must see what the audio thread did"
    );
}

#[test]
fn a_parameter_is_clamped_to_the_range_the_plugin_declared() {
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    effect.set_param(ParamId(0), 99.0);
    assert_eq!(effect.param(ParamId(0)), 2.0);

    effect.set_param(ParamId(0), -5.0);
    assert_eq!(effect.param(ParamId(0)), 0.0);

    let mut buffer = block(0.5, 8);
    effect.process(&mut buffer, &playing(8));
    assert_eq!(
        buffer.channel(0)[0],
        0.0,
        "clamped to silence, not negative"
    );

    plugin.deactivate(effect);
}

#[test]
fn the_rendering_half_crosses_a_thread_and_the_other_half_does_not() {
    // This is the whole reason a hosted plugin is two objects. If it ever stops compiling, the
    // graph can no longer hold a CLAP plugin and the design has to change, not the test.
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    let rendered = std::thread::spawn(move || {
        effect.set_param(ParamId(0), 0.5);
        let mut buffer = block(1.0, 8);
        effect.process(&mut buffer, &playing(8));
        (effect, buffer.channel(0)[0])
    })
    .join()
    .expect("the audio thread must not panic");

    assert_eq!(rendered.1, 0.5);
    plugin.deactivate(rendered.0);
}

#[test]
fn an_effect_dropped_on_another_thread_still_releases_the_plugin() {
    // The real path: the engine hands a replaced graph back down its return channel and drops
    // it there, so the session never gets the effect object back.
    let mut plugin = hosted();
    let effect = plugin.activate(&context()).expect("must activate");
    assert!(plugin.is_active());

    assert!(
        !plugin.release(),
        "a plugin whose effect is still alive must refuse, not force"
    );

    std::thread::spawn(move || drop(effect))
        .join()
        .expect("dropping must not panic");

    assert!(plugin.release());
    assert!(!plugin.is_active());
}

#[test]
fn state_round_trips_through_the_opaque_stream() {
    let mut source = hosted();
    let mut effect = source.activate(&context()).expect("must activate");
    effect.set_param(ParamId(0), 1.5);
    let mut buffer = block(1.0, 4);
    effect.process(&mut buffer, &playing(4));
    source.deactivate(effect);

    let bytes = source.save_state().expect("the fixture saves its state");
    assert_eq!(bytes.len(), 4, "the fixture stores one f32");

    let mut restored = hosted();
    assert_eq!(restored.value(ParamId(0)), Some(1.0), "a fresh default");
    restored.load_state(&bytes).expect("must restore");
    assert_eq!(restored.value(ParamId(0)), Some(1.5));
}

#[test]
fn a_shorter_block_than_the_plugin_was_activated_for_still_renders() {
    // The engine's last block before a stop is whatever is left over.
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    let mut buffer = block(0.5, 3);
    effect.process(&mut buffer, &playing(3));
    assert_eq!(buffer.frame_count(), 3);
    assert_eq!(buffer.channel(0), [0.5, 0.5, 0.5]);

    plugin.deactivate(effect);
}

#[test]
fn two_instances_of_one_plugin_do_not_share_a_parameter() {
    // The session keeps a second instance alive while the first is still rendering in a graph
    // the audio thread has not let go of. If they shared anything, a handover would take the
    // outgoing plugin's parameters with it.
    let mut first = hosted();
    let mut second = hosted();

    let mut effect = first.activate(&context()).expect("must activate");
    effect.set_param(ParamId(0), 0.25);
    let mut buffer = block(1.0, 4);
    effect.process(&mut buffer, &playing(4));

    assert_eq!(first.value(ParamId(0)), Some(0.25));
    assert_eq!(second.value(ParamId(0)), Some(1.0));

    first.deactivate(effect);
}
