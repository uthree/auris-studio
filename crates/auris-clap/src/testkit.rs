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
//! * its state is four bytes of gain, so a round trip through the opaque stream is checkable;
//! * it declares a **sidechain** input it does not use, and reports through [`PORTS_ID`] how many
//!   input ports it was actually handed. A host that passes only the ports it cares about is not
//!   making a layout mistake the plugin can notice — it is handing the plugin an array shorter
//!   than the one the plugin was told to index. Surge XT Effects has exactly this shape, and
//!   reading the missing port took the whole application down.
//! * it registers a **timer** with the host and counts the ticks it gets through [`TICKS_ID`],
//!   which is the only way to tell a host that runs a plugin's clock from one that quietly does
//!   not. The visible symptom of the latter is a window that never repaints, which looks exactly
//!   like a plugin that draws nothing.
//! * it answers the **GUI** extension without opening anything, and reports through [`GUI_ID`]
//!   which calls the host made. A test runner has no window server, and what is worth testing
//!   about a host's windowing is the order it does things in rather than anybody's drawing.
//!
//! Behind the `testkit` feature, which nothing but a test build should ever turn on.

use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGui, PluginGuiImpl, Window,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::timer::{HostTimer, PluginTimer, PluginTimerImpl, TimerId};
use clack_plugin::events::event_types::{
    MidiEvent, NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent,
    ParamValueEvent,
};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};

use crate::library::ClapLibrary;

/// The fixture's CLAP id.
pub const FIXTURE_ID: &str = "studio.auris.test.gain";

/// The fixture's gain parameter id. Deliberately neither zero nor a plausible slice index.
pub const GAIN_ID: u32 = 4242;

/// The id of the read-only parameter reporting how many input ports the last block arrived with.
pub const PORTS_ID: u32 = 4343;

/// The id of the read-only parameter counting the timer ticks the host has delivered.
pub const TICKS_ID: u32 = 4344;

/// The id of the read-only parameter reporting what the host has done to the fixture's window.
///
/// Its value is the [`gui_step`] flags, added together, in the order they happened to arrive.
pub const GUI_ID: u32 = 4345;

/// What a host can have done to the fixture's window, one bit each.
///
/// The fixture opens no real window — a test runner has no window server to open one on, and what
/// is under test is the *protocol*, not anybody's drawing. So each call is recorded instead, and a
/// host that forgets one, or makes them in the wrong order, is caught by the bits it left unset.
pub mod gui_step {
    /// `create` was called, and asked for a floating window.
    pub const CREATED: u32 = 1;
    /// A parent window was offered for the window to stay above.
    pub const TRANSIENT: u32 = 2;
    /// A title was suggested.
    pub const TITLED: u32 = 4;
    /// `show` was called.
    pub const SHOWN: u32 = 8;
    /// `hide` was called.
    pub const HIDDEN: u32 = 16;
    /// `destroy` was called.
    pub const DESTROYED: u32 = 32;
    /// `create` was called asking to be embedded in a window of the host's, which the *gain*
    /// fixture says it cannot do. A host that asks anyway is a host that ignored the answer.
    pub const ASKED_TO_EMBED: u32 = 64;
    /// `set_parent` was called with a window to draw inside — the whole of what an embedded
    /// plugin needs and the whole of what Auris could not give one until it had a window to lend.
    pub const PARENTED: u32 = 128;
}

/// How many input ports the fixture declares: a main one and a sidechain.
pub const INPUT_PORTS: u32 = 2;

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
    /// Input ports the last `process` call arrived with. `u32::MAX` until there has been one, so
    /// "nobody has rendered yet" cannot be mistaken for "the host passed none".
    ports_seen: AtomicU32,
    /// How many times the host has fired the timer the fixture registered.
    ticks: AtomicU32,
    /// Which of the [`gui_step`] calls the host has made.
    gui: AtomicU32,
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
    /// The timer the fixture registered, if the host offered any.
    ///
    /// Kept so the fixture can tell its *own* timer from somebody else's, which is the whole
    /// point of a host handing back an id.
    timer: Option<TimerId>,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

impl PluginTimerImpl for MainThread<'_> {
    fn on_timer(&mut self, timer_id: TimerId) {
        if Some(timer_id) == self.timer {
            self.shared.ticks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl MainThread<'_> {
    /// Records one of the [`gui_step`] calls.
    fn gui_did(&self, step: u32) {
        self.shared.gui.fetch_or(step, Ordering::Relaxed);
    }
}

/// A window the fixture never actually opens, recording what a host does to it.
impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&mut self, configuration: GuiConfiguration) -> bool {
        // Floating only, which is the shape Auris asks for and also the shape a host on Wayland
        // has no choice about. A fixture that said yes to everything could not catch a host
        // asking to embed.
        configuration.is_floating
            && Some(configuration.api_type) == GuiApiType::default_for_current_platform()
    }

    fn get_preferred_api(&mut self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: GuiApiType::default_for_current_platform()?,
            is_floating: true,
        })
    }

    fn create(&mut self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        match configuration.is_floating {
            true => {
                self.gui_did(gui_step::CREATED);
                Ok(())
            }
            false => {
                self.gui_did(gui_step::ASKED_TO_EMBED);
                Err(PluginError::Message("this fixture cannot be embedded"))
            }
        }
    }

    fn destroy(&mut self) {
        self.gui_did(gui_step::DESTROYED);
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: 400,
            height: 300,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_parent(&mut self, _window: Window) -> Result<(), PluginError> {
        Err(PluginError::Message("this fixture cannot be embedded"))
    }

    fn set_transient(&mut self, _window: Window) -> Result<(), PluginError> {
        self.gui_did(gui_step::TRANSIENT);
        Ok(())
    }

    fn suggest_title(&mut self, _title: &str) {
        self.gui_did(gui_step::TITLED);
    }

    fn show(&mut self) -> Result<(), PluginError> {
        self.gui_did(gui_step::SHOWN);
        Ok(())
    }

    fn hide(&mut self) -> Result<(), PluginError> {
        self.gui_did(gui_step::HIDDEN);
        Ok(())
    }
}

/// A plugin that multiplies by one automatable parameter.
pub struct Gain;

impl Plugin for Gain {
    type AudioProcessor<'a> = Processor<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginGui>()
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginTimer>();
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
        shared.ports_seen.store(u32::MAX, Ordering::Relaxed);
        Ok(shared)
    }

    fn new_main_thread<'a>(
        mut host: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<MainThread<'a>, PluginError> {
        // Asking for the fastest tick the host will give, which is what a plugin repainting a
        // window asks for. A host with no timers at all is not an error — the fixture simply
        // never ticks, which is what its tick count then says.
        let timer = host
            .shared()
            .get_extension::<HostTimer>()
            .and_then(|timer| timer.register_timer(&mut host, 0).ok());
        Ok(MainThread { shared, timer })
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
        // What the host actually handed over, which is the whole point of the sidechain below.
        self.shared
            .ports_seen
            .store(audio.input_port_count() as u32, Ordering::Relaxed);

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
    fn count(&mut self, is_input: bool) -> u32 {
        match is_input {
            true => INPUT_PORTS,
            false => 1,
        }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        let inputs = match is_input {
            true => INPUT_PORTS,
            false => 1,
        };
        if index >= inputs {
            return;
        }
        writer.set(&AudioPortInfo {
            id: ClapId::new(index),
            name: match (is_input, index) {
                (true, 0) => b"In",
                // Never read. It exists so that a host which quietly drops it is caught here
                // rather than by a plugin indexing off the end of what it was given.
                (true, _) => b"Sidechain",
                (false, _) => b"Out",
            },
            channel_count: 2,
            flags: match index {
                0 => AudioPortFlags::IS_MAIN,
                _ => AudioPortFlags::empty(),
            },
            port_type: Some(AudioPortType::STEREO),
            in_place_pair: None,
        });
    }
}

impl PluginMainThreadParams for MainThread<'_> {
    fn count(&mut self) -> u32 {
        4
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let common = ParamInfo {
            id: ClapId::new(GAIN_ID),
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Default::default(),
            name: b"Gain",
            module: b"",
            min_value: 0.0,
            max_value: 2.0,
            default_value: 1.0,
        };
        match param_index {
            0 => info.set(&common),
            1 => info.set(&ParamInfo {
                id: ClapId::new(PORTS_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Input Ports Seen",
                min_value: 0.0,
                max_value: 16.0,
                default_value: 0.0,
                ..common
            }),
            2 => info.set(&ParamInfo {
                id: ClapId::new(TICKS_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Timer Ticks",
                min_value: 0.0,
                max_value: f64::from(u16::MAX),
                default_value: 0.0,
                ..common
            }),
            3 => info.set(&ParamInfo {
                id: ClapId::new(GUI_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Window Calls",
                min_value: 0.0,
                max_value: f64::from(u16::MAX),
                default_value: 0.0,
                ..common
            }),
            _ => {}
        }
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        match param_id.get() {
            GAIN_ID => Some(self.shared.get() as f64),
            PORTS_ID => match self.shared.ports_seen.load(Ordering::Relaxed) {
                u32::MAX => Some(0.0),
                seen => Some(seen as f64),
            },
            TICKS_ID => Some(self.shared.ticks.load(Ordering::Relaxed) as f64),
            GUI_ID => Some(self.shared.gui.load(Ordering::Relaxed) as f64),
            _ => None,
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

/// The instrument fixture's CLAP id.
pub const TONE_ID: &str = "studio.auris.test.tone";

/// [`Tone`]'s level parameter id.
pub const LEVEL_ID: u32 = 7001;
/// The read-only parameter reporting the last bend the fixture was sent, in semitones.
pub const BEND_ID: u32 = 7002;
/// The read-only parameter reporting the last modulation amount the fixture was sent.
pub const WHEEL_ID: u32 = 7003;
/// The read-only parameter reporting what the host has done to [`Tone`]'s window.
///
/// The other half of what [`GUI_ID`] covers. The gain fixture will only float and this one will
/// only embed, which between them are the two arrangements CLAP has and the two a host must
/// implement — and the second is the one nearly every real plugin uses.
pub const TONE_GUI_ID: u32 = 7004;

/// A library holding nothing but the instrument fixture.
///
/// A second entry rather than a second plugin inside the first, because that is also how it looks
/// on disk: an effect and an instrument are usually two files, and loading two libraries is the
/// path the session actually takes.
///
/// # Panics
///
/// Panics if the fixture entry cannot be loaded, which would mean the crate itself is broken.
pub fn instrument_library() -> ClapLibrary {
    let entry = clack_host::prelude::PluginEntry::load_from_clack::<InstrumentEntry>(
        c"/auris/testkit/test-tone.clap",
    )
    .expect("the instrument fixture entry must load");
    ClapLibrary::from_entry(entry, "/auris/testkit/test-tone.clap")
}

/// State shared between the instrument fixture's two threads.
///
/// The last bend and wheel are here so a *host* test can prove the translation in
/// [`notes`](crate::notes) arrived, rather than only that it was constructed.
#[derive(Default)]
pub struct ToneShared {
    level: AtomicU32,
    bend: AtomicU32,
    wheel: AtomicU32,
    /// Which of the [`gui_step`] calls the host has made.
    gui: AtomicU32,
}

impl PluginShared<'_> for ToneShared {}

/// The instrument fixture's main-thread half.
pub struct ToneMainThread<'a> {
    shared: &'a ToneShared,
}

impl<'a> PluginMainThread<'a, ToneShared> for ToneMainThread<'a> {}

/// An instrument that holds a steady level for as long as a note is held.
///
/// Deliberately not an oscillator: a constant is something a test can assert an exact number
/// about, and what is under test is the note plumbing rather than anybody's synthesis.
pub struct Tone;

impl Plugin for Tone {
    type AudioProcessor<'a> = ToneProcessor<'a>;
    type Shared<'a> = ToneShared;
    type MainThread<'a> = ToneMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&ToneShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginGui>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

/// A window this fixture will only draw *inside*, which is the shape Surge XT has and every other
/// plugin built on JUCE.
///
/// Nothing is drawn and no window is made — the host's window is, and that is the half under test.
impl PluginGuiImpl for ToneMainThread<'_> {
    fn is_api_supported(&mut self, configuration: GuiConfiguration) -> bool {
        !configuration.is_floating
            && Some(configuration.api_type) == GuiApiType::default_for_current_platform()
    }

    fn get_preferred_api(&mut self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: GuiApiType::default_for_current_platform()?,
            is_floating: false,
        })
    }

    fn create(&mut self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        match configuration.is_floating {
            false => {
                self.gui_did(gui_step::CREATED);
                Ok(())
            }
            true => Err(PluginError::Message("this fixture cannot float")),
        }
    }

    fn destroy(&mut self) {
        self.gui_did(gui_step::DESTROYED);
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: 640,
            height: 480,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_parent(&mut self, _window: Window) -> Result<(), PluginError> {
        self.gui_did(gui_step::PARENTED);
        Ok(())
    }

    fn set_transient(&mut self, _window: Window) -> Result<(), PluginError> {
        Err(PluginError::Message("this fixture cannot float"))
    }

    fn show(&mut self) -> Result<(), PluginError> {
        self.gui_did(gui_step::SHOWN);
        Ok(())
    }

    fn hide(&mut self) -> Result<(), PluginError> {
        self.gui_did(gui_step::HIDDEN);
        Ok(())
    }
}

impl ToneMainThread<'_> {
    /// Records one of the [`gui_step`] calls.
    fn gui_did(&self, step: u32) {
        self.shared.gui.fetch_or(step, Ordering::Relaxed);
    }
}

impl DefaultPluginFactory for Tone {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new(TONE_ID, "Test Tone")
            .with_vendor("Auris Studio")
            .with_description("Holds a level while a note is held")
            .with_version("1.0.0")
            .with_features([INSTRUMENT, SYNTHESIZER, STEREO])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<ToneShared, PluginError> {
        let shared = ToneShared::default();
        shared.level.store(1.0f32.to_bits(), Ordering::Relaxed);
        Ok(shared)
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a ToneShared,
    ) -> Result<ToneMainThread<'a>, PluginError> {
        Ok(ToneMainThread { shared })
    }
}

/// The instrument fixture's audio-thread half.
pub struct ToneProcessor<'a> {
    shared: &'a ToneShared,
    /// Amplitude carried between blocks — a note held across a block boundary keeps sounding.
    amplitude: f32,
    /// One amplitude per frame, built from the block's events before anything is written, so an
    /// event lands on its own sample rather than at the start of the block.
    envelope: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, ToneShared, ToneMainThread<'a>> for ToneProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut ToneMainThread<'a>,
        shared: &'a ToneShared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            shared,
            amplitude: 0.0,
            envelope: vec![0.0; config.max_frames_count as usize],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let frames = audio.frames_count() as usize;
        let mut cursor = 0usize;

        for event in events.input {
            let at = (event.header().time() as usize).min(frames);
            self.envelope[cursor..at].fill(self.amplitude);
            cursor = at;

            if let Some(on) = event.as_event::<NoteOnEvent>() {
                self.amplitude = on.velocity() as f32;
            } else if event.as_event::<NoteOffEvent>().is_some()
                || event.as_event::<NoteChokeEvent>().is_some()
            {
                self.amplitude = 0.0;
            } else if let Some(expression) = event.as_event::<NoteExpressionEvent>()
                && expression.expression_type() == Some(NoteExpressionType::Tuning)
            {
                self.shared
                    .bend
                    .store((expression.value() as f32).to_bits(), Ordering::Relaxed);
            } else if let Some(midi) = event.as_event::<MidiEvent>() {
                let data = midi.data();
                if data[0] == 0xB0 && data[1] == 1 {
                    let amount = data[2] as f32 / 127.0;
                    self.shared.wheel.store(amount.to_bits(), Ordering::Relaxed);
                }
            } else if let Some(param) = event.as_event::<ParamValueEvent>()
                && param.param_id().map(|id| id.get()) == Some(LEVEL_ID)
            {
                self.shared
                    .level
                    .store((param.value() as f32).to_bits(), Ordering::Relaxed);
            }
        }
        self.envelope[cursor..frames].fill(self.amplitude);

        let level = f32::from_bits(self.shared.level.load(Ordering::Relaxed));
        for mut port in &mut audio {
            let Some(channels) = port.channels()?.into_f32() else {
                continue;
            };
            for channel in channels {
                let out = match channel {
                    ChannelPair::OutputOnly(out) => out,
                    ChannelPair::InputOutput(_, out) => out,
                    ChannelPair::InPlace(buf) => buf,
                    ChannelPair::InputOnly(_) => continue,
                };
                for (out, amplitude) in out.iter_mut().zip(&self.envelope[..frames]) {
                    *out = amplitude * level;
                }
            }
        }

        Ok(ProcessStatus::Continue)
    }
}

impl PluginAudioProcessorParams for ToneProcessor<'_> {
    fn flush(&mut self, _input: &InputEvents, _output: &mut OutputEvents) {}
}

impl PluginAudioPortsImpl for ToneMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        // No audio input at all, which is the shape that would catch a host insisting on one.
        match is_input {
            true => 0,
            false => 1,
        }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if is_input || index != 0 {
            return;
        }
        writer.set(&AudioPortInfo {
            id: ClapId::new(0),
            name: b"Out",
            channel_count: 2,
            flags: AudioPortFlags::IS_MAIN,
            port_type: Some(AudioPortType::STEREO),
            in_place_pair: None,
        });
    }
}

impl PluginNotePortsImpl for ToneMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        match is_input {
            true => 1,
            false => 0,
        }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if !is_input || index != 0 {
            return;
        }
        writer.set(&NotePortInfo {
            id: ClapId::new(0),
            name: b"Notes",
            // Both, like most real instruments, which is what lets the host send notes as CLAP
            // events and still have somewhere to put a modulation wheel.
            supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
            preferred_dialect: Some(NoteDialect::Clap),
        });
    }
}

impl PluginMainThreadParams for ToneMainThread<'_> {
    fn count(&mut self) -> u32 {
        4
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let common = ParamInfo {
            id: ClapId::new(LEVEL_ID),
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Default::default(),
            name: b"Level",
            module: b"",
            min_value: 0.0,
            max_value: 1.0,
            default_value: 1.0,
        };
        match param_index {
            0 => info.set(&common),
            1 => info.set(&ParamInfo {
                id: ClapId::new(BEND_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Last Bend",
                min_value: -48.0,
                max_value: 48.0,
                default_value: 0.0,
                ..common
            }),
            2 => info.set(&ParamInfo {
                id: ClapId::new(WHEEL_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Last Wheel",
                default_value: 0.0,
                ..common
            }),
            3 => info.set(&ParamInfo {
                id: ClapId::new(TONE_GUI_ID),
                flags: ParamInfoFlags::IS_READONLY,
                name: b"Window Calls",
                min_value: 0.0,
                max_value: f64::from(u16::MAX),
                default_value: 0.0,
                ..common
            }),
            _ => {}
        }
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        let read = |cell: &AtomicU32| f32::from_bits(cell.load(Ordering::Relaxed)) as f64;
        match param_id.get() {
            LEVEL_ID => Some(read(&self.shared.level)),
            BEND_ID => Some(read(&self.shared.bend)),
            WHEEL_ID => Some(read(&self.shared.wheel)),
            TONE_GUI_ID => Some(self.shared.gui.load(Ordering::Relaxed) as f64),
            _ => None,
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

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            if let Some(event) = event.as_event::<ParamValueEvent>()
                && event.param_id().map(|id| id.get()) == Some(LEVEL_ID)
            {
                self.shared
                    .level
                    .store((event.value() as f32).to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl PluginStateImpl for ToneMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.level.load(Ordering::Relaxed).to_le_bytes())?;
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = [0u8; 4];
        input.read_exact(&mut bytes)?;
        self.shared
            .level
            .store(u32::from_le_bytes(bytes), Ordering::Relaxed);
        Ok(())
    }
}

/// The instrument fixture's entry point, as a second `.clap` file would expose it.
pub type InstrumentEntry = SinglePluginEntry<Tone>;
