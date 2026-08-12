//! A CLAP plugin compiled into the binary, for tests here and in the crates above.
//!
//! Hosting cannot be tested against nothing, and it should not be tested against whatever the
//! machine happens to have installed: a suite that passes on a laptop with Surge XT and fails on
//! a CI runner without it is a suite nobody trusts. So the fixture is written with `clack-plugin`
//! and reached through [`PluginEntry::load_from_clack`], which walks the same C entry point a
//! `.clap` file exposes — the host path under test is the real one, and `cargo test` needs
//! nothing installed and no build script.
//!
//! The fixture is built to catch the three things a hand-written stub would let through:
//!
//! * its parameter id is `4242`, not `0`, so anything that confuses a CLAP id with a slice index
//!   fails here;
//! * its gain is shared between the two threads, so a parameter the audio side was given can be
//!   read back from the main side;
//! * its state is four bytes of gain, so a round trip through the opaque stream is checkable.
//!
//! Behind the `testkit` feature, which nothing but a test build should ever turn on.

use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};

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

use crate::library::ClapLibrary;

/// The fixture's CLAP id.
pub const FIXTURE_ID: &str = "studio.auris.test.gain";

/// The fixture's one parameter id. Deliberately neither zero nor a plausible slice index.
pub const GAIN_ID: u32 = 4242;

/// A library holding nothing but the fixture.
///
/// # Panics
///
/// Panics if the fixture entry cannot be loaded, which would mean the crate itself is broken.
pub fn fixture_library() -> ClapLibrary {
    let entry = clack_host::prelude::PluginEntry::load_from_clack::<Entry>(
        c"/auris/testkit/test-gain.clap",
    )
    .expect("the fixture entry must load");
    ClapLibrary::from_entry(entry, "/auris/testkit/test-gain.clap")
}

/// State shared between the fixture's two threads.
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

/// The fixture's main-thread half.
pub struct MainThread<'a> {
    shared: &'a Shared,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

/// A plugin that multiplies by one automatable parameter.
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
        PluginDescriptor::new(FIXTURE_ID, "Test Gain")
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

/// The fixture's audio-thread half.
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

/// The fixture's entry point, as a `.clap` file would expose it.
pub type Entry = SinglePluginEntry<Gain>;
