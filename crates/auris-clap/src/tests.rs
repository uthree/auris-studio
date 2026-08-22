//! Hosting tests, run against the [`testkit`](crate::testkit) fixture.

use auris_core::buffer::AudioBuffer;
use auris_core::param::ParamId;
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginKind, PrepareContext,
    ProcessContext,
};

use raw_window_handle::{RawWindowHandle, Win32WindowHandle};

use crate::library::ClapLibrary;
use crate::notes::NoteLanguage;
use crate::plugin::ClapPlugin;
use crate::testkit::{
    FIXTURE_ID, INPUT_PORTS, TONE_ID, fixture_library, gui_step, instrument_library,
};

fn library() -> ClapLibrary {
    fixture_library()
}

fn hosted() -> ClapPlugin {
    library()
        .instantiate(FIXTURE_ID)
        .expect("the fixture plugin must instantiate")
}

fn tone_hosted() -> ClapPlugin {
    instrument_library()
        .instantiate(TONE_ID)
        .expect("the instrument fixture must instantiate")
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
    assert_eq!(params.len(), 5);
    assert_eq!(params[0].id, ParamId(0), "the slice position");
    assert_eq!(params[0].key, "clap.4242", "the plugin's own id");
    assert_eq!(params[0].name, "Gain");
    assert_eq!(params[0].min, 0.0);
    assert_eq!(params[0].max, 2.0);
    assert_eq!(params[0].default, 1.0);
    assert_eq!(params[1].key, "clap.4343", "and the second keeps its own");
}

#[test]
fn a_timer_the_plugin_registered_is_actually_run() {
    // End to end, because every part of this is somebody else calling somebody else: the fixture
    // asks the host for a timer while it is being created, the host writes it down, and the tick
    // the frontend drives has to find its way back into the plugin. A window that never repaints
    // is what a break anywhere along here looks like, and it looks the same as a plugin that just
    // does not draw.
    let mut plugin = hosted();
    let ticks = |plugin: &mut ClapPlugin| plugin.value(ParamId(2)).expect("the tick count");

    plugin.tick_timers();
    assert_eq!(ticks(&mut plugin), 0.0, "not due until a period has passed");

    std::thread::sleep(std::time::Duration::from_millis(
        crate::timers::TIMER_FLOOR_MS as u64 + 5,
    ));
    plugin.tick_timers();
    assert_eq!(ticks(&mut plugin), 1.0);

    plugin.tick_timers();
    assert_eq!(ticks(&mut plugin), 1.0, "and not again until the next one");
}

/// Which of the fixture's [`gui_step`] calls have been made.
fn window_calls(plugin: &mut ClapPlugin) -> u32 {
    plugin.value(ParamId(3)).expect("the window call report") as u32
}

#[test]
fn opening_a_window_makes_the_four_calls_in_the_order_clap_asks_for() {
    // A window nobody titled, or one shown before it was told what to float above, is the kind of
    // thing that looks right on one machine and wrong on the next. The fixture opens nothing and
    // records everything, so the protocol can be checked where there is no window server.
    let mut plugin = hosted();
    assert!(plugin.has_gui());
    assert!(!plugin.gui_is_open());

    // A handle to nothing: the fixture never dereferences it, and what is under test is that the
    // host offered one at all.
    let parent = RawWindowHandle::Win32(Win32WindowHandle::new(
        std::num::NonZeroIsize::new(1).unwrap(),
    ));
    plugin.open_gui(Some(parent)).expect("the fixture opens");
    assert!(plugin.gui_is_open());

    let calls = window_calls(&mut plugin);
    assert_eq!(calls & gui_step::CREATED, gui_step::CREATED);
    assert_eq!(calls & gui_step::TITLED, gui_step::TITLED);
    assert_eq!(calls & gui_step::SHOWN, gui_step::SHOWN);
    assert_eq!(
        calls & gui_step::ASKED_TO_EMBED,
        0,
        "every window this host asks for is a floating one"
    );
    // Only where the handle is the platform's own: a Win32 handle on a Mac is not a window the
    // plugin could be floated above, and clack refuses to make one.
    if cfg!(target_os = "windows") {
        assert_eq!(calls & gui_step::TRANSIENT, gui_step::TRANSIENT);
    }

    plugin.close_gui();
    assert!(!plugin.gui_is_open());
    let calls = window_calls(&mut plugin);
    assert_eq!(calls & gui_step::HIDDEN, gui_step::HIDDEN);
    assert_eq!(calls & gui_step::DESTROYED, gui_step::DESTROYED);
}

#[test]
fn a_plugin_that_will_only_embed_is_lent_a_window_to_draw_in() {
    // The case the whole `window` module exists for, and the one the first attempt at this got
    // wrong: Auris asked every plugin for a floating window, so Surge XT — which offers only
    // embedding, like everything built on JUCE — reported no window at all.
    let mut plugin = instrument_library()
        .instantiate(TONE_ID)
        .expect("the instrument fixture");
    assert!(
        plugin.has_gui(),
        "a plugin that will only embed still has a window, given one to embed in"
    );

    match plugin.open_gui(None) {
        Ok(()) => {
            let calls = plugin.value(ParamId(3)).expect("the window call report") as u32;
            assert_eq!(calls & gui_step::CREATED, gui_step::CREATED);
            assert_eq!(
                calls & gui_step::PARENTED,
                gui_step::PARENTED,
                "a window it was never given is a window it cannot draw in"
            );
            assert_eq!(calls & gui_step::SHOWN, gui_step::SHOWN);

            plugin.close_gui();
            let calls = plugin.value(ParamId(3)).expect("the window call report") as u32;
            assert_eq!(calls & gui_step::DESTROYED, gui_step::DESTROYED);
        }
        // A machine with no window server has nothing to lend, and says so rather than pretending.
        // Worth allowing rather than skipping: on any machine that *has* one this is the real test.
        Err(error) => assert!(
            error.to_string().contains("would not lend"),
            "the only acceptable failure is having no window to give: {error}"
        ),
    }
}

#[test]
fn a_window_is_opened_and_closed_once_however_often_it_is_asked_for() {
    // `create` and `destroy` are a pair CLAP leaves the host to balance, and calling either twice
    // is undefined. So the caller is allowed to be sloppy and this is not.
    let mut plugin = hosted();
    plugin.open_gui(None).expect("open");
    plugin
        .open_gui(None)
        .expect("opening again is not an error");
    assert!(plugin.gui_is_open());

    plugin.close_gui();
    assert!(!plugin.gui_is_open());
    // The fixture would record the same bits a second time, so what is checked is that the second
    // close cannot reach the plugin at all — which is what the state flag is for.
    plugin.close_gui();
}

#[test]
fn a_window_the_plugin_closed_is_reported_and_not_hidden_on_the_way_out() {
    // A plugin that destroyed its own window is owed a `destroy` to acknowledge it and nothing
    // else. Hiding it first is a call through a pointer the plugin has already freed, and that is
    // the crash this flag exists to avoid.
    let mut plugin = hosted();
    plugin.open_gui(None).expect("open");

    plugin.pretend_the_window_closed(true);
    let requests = plugin.take_requests();
    assert!(requests.gui_closed);
    assert!(
        !plugin.take_requests().gui_closed,
        "a request is answered once"
    );

    plugin.close_gui();
    let calls = window_calls(&mut plugin);
    assert_eq!(calls & gui_step::DESTROYED, gui_step::DESTROYED);
    assert_eq!(
        calls & gui_step::HIDDEN,
        0,
        "hiding a window the plugin has already destroyed is a use-after-free"
    );
}

#[test]
fn dropping_a_plugin_closes_the_window_it_left_open() {
    // The plugin's window is an OS object registered against this process, and the code that would
    // tidy it up lives inside the library about to be unmapped. Deleting a track whose editor is
    // open takes exactly this path.
    let mut plugin = hosted();
    plugin.open_gui(None).expect("open");
    drop(plugin);
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
    // The audio has to come back out of the *main* output rather than out of whichever port
    // happened to be first in the list, and the key must not have been mistaken for the audio.
    let mut plugin = hosted();
    let mut effect = plugin.activate(&context()).expect("must activate");

    let mut buffer = block(0.5, 32);
    effect.process_with_sidechain(&mut buffer, &block(0.25, 32), &playing(32));
    assert_eq!(buffer.channel(0)[0], 0.5);
    assert_eq!(buffer.channel(1)[31], 0.5);

    plugin.deactivate(effect);
}

#[test]
fn a_key_reaches_the_port_the_plugin_declared_for_one() {
    // The fixture reports the loudest thing it saw on its second input port, which is the only
    // way to tell a host that routes a key from one that declares the port and fills it with
    // silence. Those two are indistinguishable from the output, because nothing about the
    // fixture's main path changes.
    let mut plugin = hosted();
    assert_eq!(plugin.ports(2).sidechain_input(), Some(1));
    let mut effect = plugin.activate(&context()).expect("must activate");
    assert!(
        effect.wants_sidechain(),
        "the plugin declared a port for one"
    );

    effect.process_with_sidechain(&mut block(0.5, 32), &block(0.25, 32), &playing(32));
    assert_eq!(plugin.value(ParamId(4)), Some(0.25));

    // And the very next block with no key hands the port silence rather than leaving the last
    // one in it, which is what a slot that has stopped being keyed has to hear.
    effect.process(&mut block(0.5, 32), &playing(32));
    assert_eq!(plugin.value(ParamId(4)), Some(0.0));
    plugin.deactivate(effect);
}

#[test]
fn a_plugin_with_no_spare_port_is_not_offered_a_key() {
    // The tone fixture declares no audio input at all, so there is nothing for a key to go in
    // and a frontend should not be offering a source to key from.
    let mut plugin = tone_hosted();
    assert_eq!(plugin.ports(2).sidechain_input(), None);
    let instrument = plugin.activate_instrument(&context()).expect("activates");
    plugin.deactivate_instrument(instrument);
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

/// The instrument fixture, instantiated from its own library.
fn tone() -> ClapPlugin {
    instrument_library()
        .instantiate(TONE_ID)
        .expect("the instrument fixture must instantiate")
}

/// An empty stereo block, which is what an instrument is handed.
fn empty(frames: usize) -> AudioBuffer {
    AudioBuffer::stereo(frames, 48_000.0)
}

#[test]
fn a_hosted_instrument_is_recognised_as_one() {
    let plugins = instrument_library()
        .plugins()
        .expect("the factory must be readable");
    assert_eq!(plugins[0].kind, PluginKind::Instrument);
    assert_eq!(plugins[0].name, "Test Tone");
}

#[test]
fn a_note_reaches_a_hosted_instrument_on_the_frame_it_was_scheduled_for() {
    let mut plugin = tone();
    let mut instrument = plugin
        .activate_instrument(&context())
        .expect("must activate");

    let mut buffer = empty(32);
    instrument.process(
        &[
            NoteEvent::NoteOn {
                frame: 8,
                pitch: 60,
                velocity: 0.5,
            },
            NoteEvent::NoteOff {
                frame: 24,
                pitch: 60,
            },
        ],
        &mut buffer,
        &playing(32),
    );

    // Silence, then the note, then silence again — with the edges on the frames asked for, not
    // rounded to the block. A host that timestamped every event zero would fail here.
    assert_eq!(buffer.channel(0)[7], 0.0);
    assert_eq!(buffer.channel(0)[8], 0.5);
    assert_eq!(buffer.channel(1)[23], 0.5);
    assert_eq!(buffer.channel(0)[24], 0.0);

    plugin.deactivate_instrument(instrument);
}

#[test]
fn a_note_held_across_a_block_boundary_keeps_sounding() {
    let mut plugin = tone();
    let mut instrument = plugin
        .activate_instrument(&context())
        .expect("must activate");

    let mut first = empty(16);
    instrument.process(
        &[NoteEvent::NoteOn {
            frame: 0,
            pitch: 64,
            velocity: 1.0,
        }],
        &mut first,
        &playing(16),
    );
    assert_eq!(first.channel(0)[15], 1.0);

    // Nothing is sent this time. A host that reset the plugin between blocks, or that failed to
    // let the plugin keep its own state, would give silence here.
    let mut second = empty(16);
    instrument.process(&[], &mut second, &playing(16));
    assert_eq!(second.channel(0)[0], 1.0);
    assert_eq!(second.channel(1)[15], 1.0);

    // And everything stops when the whole part is released.
    let mut third = empty(16);
    instrument.process(
        &[NoteEvent::AllSoundOff { frame: 0 }],
        &mut third,
        &playing(16),
    );
    assert_eq!(third.channel(0)[0], 0.0);

    plugin.deactivate_instrument(instrument);
}

#[test]
fn the_bend_and_the_wheel_both_arrive_by_their_own_route() {
    // The two events with no single dialect between them: the bend goes as a CLAP tuning in
    // semitones, the wheel as MIDI, and the fixture records what it was actually sent.
    let mut plugin = tone();
    assert_eq!(
        plugin.note_language(),
        Some(NoteLanguage {
            clap_notes: true,
            midi: true
        })
    );
    let mut instrument = plugin
        .activate_instrument(&context())
        .expect("must activate");

    instrument.process(
        &[
            NoteEvent::PitchBend {
                frame: 0,
                semitones: -3.5,
            },
            NoteEvent::Controller {
                frame: 1,
                number: 1,
                value: 1.0,
            },
        ],
        &mut empty(8),
        &playing(8),
    );
    plugin.deactivate_instrument(instrument);

    assert_eq!(
        plugin.value(ParamId(1)),
        Some(-3.5),
        "semitones, not a fourteen-bit number scaled by a range nobody agreed on"
    );
    assert_eq!(plugin.value(ParamId(2)), Some(1.0));
}

#[test]
fn an_instrument_that_will_not_run_leaves_silence_rather_than_what_was_there() {
    // The trait's contract: an instrument overwrites its buffer. A hosted one that produces
    // nothing must still clear whatever the buffer was reused from, or the previous track's
    // audio plays out of this one.
    let mut plugin = tone();
    let mut instrument = plugin
        .activate_instrument(&context())
        .expect("must activate");

    let mut buffer = block(0.75, 16);
    instrument.process(&[], &mut buffer, &playing(16));
    assert_eq!(buffer.channel(0)[0], 0.0);
    assert_eq!(buffer.channel(1)[15], 0.0);

    plugin.deactivate_instrument(instrument);
}

#[test]
fn an_instrument_with_no_audio_input_is_still_given_the_ports_it_has() {
    let mut plugin = tone();
    let ports = plugin.ports(2);
    assert_eq!(ports.inputs, Vec::<usize>::new(), "a synth takes no audio");
    assert_eq!(ports.main_input, None);
    assert_eq!(ports.outputs, vec![2]);
    assert_eq!(ports.main_output, Some(0));
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
