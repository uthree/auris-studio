//! Hosting tests, run against a plugin that is compiled into the test binary.
//!
//! The plugin below is written with `clack-plugin` and reached through
//! [`PluginEntry::load_from_clack`], which walks the same C entry point a `.clap` file exposes.
//! So the host path under test is the real one — the factory, the descriptor, the extensions,
//! the C ABI — while `cargo test` needs no plugin installed and no build script.
//!
//! What the fixture is built to catch:
//!
//! * its parameter id is `4242`, not `0`, so anything that confuses a CLAP id with a slice index
//!   fails here;
//! * its gain is shared between the two threads, so a parameter the audio side was given can be
//!   read back from the main side;
//! * its state is four bytes of gain, so a round trip through the opaque stream is checkable.

use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};

use auris_core::buffer::AudioBuffer;
use auris_core::param::ParamId;
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginKind, PrepareContext, ProcessContext,
};
use clack_host::prelude::PluginEntry;

use crate::library::ClapLibrary;
use crate::plugin::ClapPlugin;

// ---------------------------------------------------------------- the fixture plugin

mod fixture {
    use super::*;
    use clack_extensions::audio_ports::{
        AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
        PluginAudioPortsImpl,
    };
    use clack_extensions::params::{
        ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
        PluginMainThreadParams, PluginParams,
    };
    use clack_extensions::state::{PluginState, PluginStateImpl};
    use clack_plugin::events::event_types::ParamValueEvent;
    use clack_plugin::prelude::*;
    use clack_plugin::stream::{InputStream, OutputStream};

    /// Deliberately neither zero nor a plausible slice index.
    pub const GAIN_ID: u32 = 4242;

    #[derive(Default)]
    pub struct Shared {
        gain: AtomicU32,
    }

    impl Shared {
        fn get(&self) -> f32 {
            f32::from_bits(self.gain.load(Ordering::Relaxed))
        }
        fn set(&self, value: f32) {
            self.gain.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    impl PluginShared<'_> for Shared {}

    pub struct MainThread<'a> {
        shared: &'a Shared,
    }

    impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

    pub struct Gain;

    impl Plugin for Gain {
        type AudioProcessor<'a> = Processor<'a>;
        type Shared<'a> = Shared;
        type MainThread<'a> = MainThread<'a>;

        fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
            builder
                .register::<PluginAudioPorts>()
                .register::<PluginParams>()
                .register::<PluginState>();
        }
    }

    impl DefaultPluginFactory for Gain {
        fn get_descriptor() -> PluginDescriptor {
            use clack_plugin::plugin::features::*;
            PluginDescriptor::new("studio.auris.test.gain", "Test Gain")
                .with_vendor("Auris Studio")
                .with_description("Multiplies by a parameter")
                .with_version("1.0.0")
                .with_features([AUDIO_EFFECT, UTILITY, STEREO])
        }

        fn new_shared(_host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
            let shared = Shared::default();
            shared.set(1.0);
            Ok(shared)
        }

        fn new_main_thread<'a>(
            _host: HostMainThreadHandle<'a>,
            shared: &'a Shared,
        ) -> Result<MainThread<'a>, PluginError> {
            Ok(MainThread { shared })
        }
    }

    pub struct Processor<'a> {
        shared: &'a Shared,
    }

    impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor<'a> {
        fn activate(
            _host: HostAudioProcessorHandle<'a>,
            _main_thread: &mut MainThread<'a>,
            shared: &'a Shared,
            _config: PluginAudioConfiguration,
        ) -> Result<Self, PluginError> {
            Ok(Self { shared })
        }

        fn process(
            &mut self,
            _process: Process,
            mut audio: Audio,
            events: Events,
        ) -> Result<ProcessStatus, PluginError> {
            for event in events.input {
                if let Some(event) = event.as_event::<ParamValueEvent>()
                    && event.param_id().map(|id| id.get()) == Some(GAIN_ID)
                {
                    self.shared.set(event.value() as f32);
                }
            }

            let gain = self.shared.get();
            for mut port in &mut audio {
                let Some(channels) = port.channels()?.into_f32() else {
                    continue;
                };
                for channel in channels {
                    match channel {
                        ChannelPair::InputOnly(_) => {}
                        ChannelPair::OutputOnly(out) => out.fill(0.0),
                        ChannelPair::InputOutput(input, out) => {
                            for (input, out) in input.iter().zip(out) {
                                *out = input * gain;
                            }
                        }
                        ChannelPair::InPlace(buf) => {
                            for sample in buf {
                                *sample *= gain;
                            }
                        }
                    }
                }
            }

            Ok(ProcessStatus::Continue)
        }
    }

    impl PluginAudioProcessorParams for Processor<'_> {
        fn flush(&mut self, _input: &InputEvents, _output: &mut OutputEvents) {}
    }

    impl PluginAudioPortsImpl for MainThread<'_> {
        fn count(&mut self, _is_input: bool) -> u32 {
            1
        }

        fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
            if index != 0 {
                return;
            }
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: match is_input {
                    true => b"In",
                    false => b"Out",
                },
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }

    impl PluginMainThreadParams for MainThread<'_> {
        fn count(&mut self) -> u32 {
            1
        }

        fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
            if param_index != 0 {
                return;
            }
            info.set(&ParamInfo {
                id: ClapId::new(GAIN_ID),
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Default::default(),
                name: b"Gain",
                module: b"",
                min_value: 0.0,
                max_value: 2.0,
                default_value: 1.0,
            });
        }

        fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
            match param_id.get() == GAIN_ID {
                true => Some(self.shared.get() as f64),
                false => None,
            }
        }

        fn value_to_text(
            &mut self,
            _param_id: ClapId,
            value: f64,
            writer: &mut ParamDisplayWriter,
        ) -> std::fmt::Result {
            use std::fmt::Write;
            write!(writer, "{value:.2}")
        }

        fn text_to_value(&mut self, _param_id: ClapId, text: &CStr) -> Option<f64> {
            text.to_str().ok()?.trim().parse().ok()
        }

        fn flush(&mut self, _input: &InputEvents, _output: &mut OutputEvents) {}
    }

    impl PluginStateImpl for MainThread<'_> {
        fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
            output.write_all(&self.shared.get().to_le_bytes())?;
            Ok(())
        }

        fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
            let mut bytes = [0u8; 4];
            input.read_exact(&mut bytes)?;
            self.shared.set(f32::from_le_bytes(bytes));
            Ok(())
        }
    }

    pub type Entry = SinglePluginEntry<Gain>;
}

// ---------------------------------------------------------------- the tests

fn library() -> ClapLibrary {
    let entry = PluginEntry::load_from_clack::<fixture::Entry>(c"/fake/path/test-gain.clap")
        .expect("the fixture entry must load");
    ClapLibrary::from_entry(entry, "/fake/path/test-gain.clap")
}

fn hosted() -> ClapPlugin {
    library()
        .instantiate("studio.auris.test.gain")
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
    assert_eq!(info.clap_id, "studio.auris.test.gain");
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
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].id, ParamId(0), "the slice position");
    assert_eq!(params[0].key, "clap.4242", "the plugin's own id");
    assert_eq!(params[0].name, "Gain");
    assert_eq!(params[0].min, 0.0);
    assert_eq!(params[0].max, 2.0);
    assert_eq!(params[0].default, 1.0);
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
