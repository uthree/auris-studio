//! Opening the audio hardware and driving the renderer from its callback.
//!
//! The callback thread owns exactly one [`RenderGraph`] and one [`Transport`]. It talks to the
//! UI through two bounded queues and shared atomics: commands come down one queue, retired graphs
//! go back up the other so that plugin instances and sample buffers are freed on the UI thread,
//! while playhead, count-in, play state, latency status and meters are published atomically.
//! Inside the callback there is no allocation, no lock, no logging and no I/O.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use auris_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, I24, Sample, SampleFormat, StreamConfig, SupportedBufferSize, U24,
};
use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::command::EngineCommand;
use crate::error::EngineError;
use crate::graph::{RETIRED_GRAPH_SLOTS, RenderGraph};
use crate::handle::{EngineHandle, Retired};
use crate::meter::MeterBank;
use crate::renderer::render_block;
use crate::transport::Transport;

/// Block size used when the backend will not say what it prefers.
const DEFAULT_MAX_BLOCK: usize = 2_048;

/// Smallest command queue that still absorbs one UI frame's worth of fader moves.
const MIN_COMMAND_CAPACITY: usize = 16;

/// Conventional rates worth offering when a backend reports one continuous range.
const STANDARD_SAMPLE_RATES: [u32; 8] = [
    8_000, 22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
];

pub(crate) fn add_sample_rate_range(rates: &mut Vec<u32>, min: u32, max: u32) {
    for rate in [min, max].into_iter().chain(
        STANDARD_SAMPLE_RATES
            .into_iter()
            .filter(|rate| (min..=max).contains(rate)),
    ) {
        if !rates.contains(&rate) {
            rates.push(rate);
        }
    }
}

/// What to ask the audio backend for.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSettings {
    /// Backend name, such as `ASIO` or `WASAPI`. `None` uses the platform default.
    pub host: Option<String>,
    /// Name of the output device to open. `None` uses the system default.
    ///
    /// Matched by name because that is the only stable handle cpal offers across runs — a
    /// device index changes when anything is plugged in or removed.
    pub device: Option<String>,
    /// Preferred sample rate in Hz. Ignored when the device cannot do it.
    pub sample_rate: Option<u32>,
    /// Preferred callback size in frames. Ignored when the device cannot do it.
    pub block_frames: Option<u32>,
    /// How many commands may be queued before [`EngineHandle::send`] starts refusing.
    pub command_capacity: usize,
    /// How many track meters to allocate. Tracks past this many are simply not metered.
    pub meter_tracks: usize,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            host: None,
            device: None,
            sample_rate: None,
            block_frames: None,
            command_capacity: 64,
            meter_tracks: 128,
        }
    }
}

/// An output device the host could open, and what it can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    /// Name, which is also how [`AudioSettings::device`] refers to it.
    pub name: String,
    /// Whether this is the system's default output.
    pub is_default: bool,
    /// Sample rates the device advertises, ascending and deduplicated.
    pub sample_rates: Vec<u32>,
    /// Highest channel count the device advertises.
    pub max_channels: u16,
}

/// Every output device the default host can see.
///
/// Enumerating devices talks to the OS audio server and can block for tens of milliseconds, so
/// call it when a settings panel opens rather than on every frame. A device that errors while
/// being queried is skipped rather than failing the whole list — one broken aggregate device
/// should not hide the working ones.
pub fn output_devices() -> Vec<AudioDeviceInfo> {
    output_devices_for_host(None)
}

/// Names of the audio backends available on this platform.
pub fn audio_hosts() -> Vec<String> {
    cpal::available_hosts()
        .into_iter()
        .map(|id| id.name().to_owned())
        .collect()
}

/// Resolves a backend without silently selecting a different one.
pub(crate) fn audio_host(name: Option<&str>) -> Result<cpal::Host, EngineError> {
    let Some(name) = name else {
        return Ok(cpal::default_host());
    };
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| EngineError::HostUnavailable(name.to_owned()))?;
    Ok(cpal::host_from_id(id)?)
}

/// Every output device visible to the selected backend.
pub fn output_devices_for_host(name: Option<&str>) -> Vec<AudioDeviceInfo> {
    let Ok(host) = audio_host(name) else {
        return Vec::new();
    };
    // `Display` is what `open_output` already records as the device name and what the status
    // bar shows, so matching on it keeps every place that names a device in agreement.
    let default_name = host
        .default_output_device()
        .map(|device| device.to_string());

    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };

    devices
        .map(|device| {
            let name = device.to_string();
            let mut sample_rates = Vec::new();
            let mut max_channels = 0;
            if let Ok(configs) = device.supported_output_configs() {
                for config in configs {
                    max_channels = max_channels.max(config.channels());
                    add_sample_rate_range(
                        &mut sample_rates,
                        config.min_sample_rate(),
                        config.max_sample_rate(),
                    );
                }
            }
            sample_rates.sort_unstable();
            AudioDeviceInfo {
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
                sample_rates,
                max_channels,
            }
        })
        .collect()
}

/// A running (or silently idle) output stream.
pub struct AudioDevice {
    stream: Option<cpal::Stream>,
    // ASIO input must clone this device: re-enumeration creates separate stream state.
    pub(crate) hardware: Option<cpal::Device>,
    host: Option<String>,
    outputs: Vec<AudioDeviceInfo>,
    /// A second reading end of the command queue, so that a queue nothing is consuming — no
    /// device at all, or a stream that died — can still be drained from the UI thread.
    idle_commands: Option<Receiver<EngineCommand>>,
    /// Shared with [`EngineHandle`] and with the stream's error callback, which clears it when
    /// the device disappears out from under the stream.
    running: Arc<AtomicBool>,
    name: String,
    sample_rate: f64,
    channel_count: usize,
    max_block: usize,
    sample_format: Option<SampleFormat>,
}

impl AudioDevice {
    /// Backend actually opened, or `None` in silent mode.
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Hardware buffer size reported by the stream, never the renderer's chunk size.
    pub fn buffer_frames(&self) -> Option<u32> {
        if !self.is_running() {
            return None;
        }
        self.stream
            .as_ref()?
            .buffer_size()
            .ok()
            .filter(|frames| *frames > 0)
    }

    /// ASIO devices captured before opening the driver, which then prevents enumeration.
    pub fn cached_outputs(&self) -> &[AudioDeviceInfo] {
        &self.outputs
    }

    /// `true` when a real output stream is open and still alive.
    ///
    /// A stream whose device was unplugged still *exists* — cpal only reports the error — so
    /// existence alone would answer `true` for a callback that will never run again.
    pub fn is_running(&self) -> bool {
        self.stream.is_some() && self.running.load(Ordering::Relaxed)
    }

    /// Name of the device, or a placeholder in silent mode.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rate the stream runs at.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Channel count of the stream.
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    /// Largest block the engine renders at once.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Sample format the device consumes, or `None` in silent mode.
    pub fn sample_format(&self) -> Option<SampleFormat> {
        self.sample_format
    }

    /// Starts the stream.
    pub fn play(&self) -> Result<(), EngineError> {
        match &self.stream {
            Some(stream) => stream.play().map_err(EngineError::from),
            None => Ok(()),
        }
    }

    /// Pauses the stream.
    pub fn pause(&self) -> Result<(), EngineError> {
        match &self.stream {
            Some(stream) => stream.pause().map_err(EngineError::from),
            None => Ok(()),
        }
    }

    /// Throws away commands that piled up with nothing consuming them.
    ///
    /// Without an audio thread — none was ever opened, or the device disappeared and the stream
    /// died — nothing drains the queue, so after `command_capacity` sends every later command
    /// would be refused. A frontend's per-frame housekeeping should call this occasionally.
    /// Returns how many commands were discarded, and refuses to touch the queue while a live
    /// audio thread is the consumer. When the backend has reported a fatal stream error, the
    /// stream is dropped first so its callback has fully stopped before this thread drains the
    /// callback's receiving end.
    pub fn discard_pending(&mut self) -> usize {
        if self.is_running() {
            return 0;
        }
        let commands = &self.idle_commands;
        after_stream_stopped(&mut self.stream, || {
            commands
                .as_ref()
                .map_or(0, |commands| commands.try_iter().count())
        })
    }
}

/// Runs UI-thread cleanup only after the possibly-live audio callback has been joined by Drop.
fn after_stream_stopped<T, R>(stream: &mut Option<T>, cleanup: impl FnOnce() -> R) -> R {
    drop(stream.take());
    cleanup()
}

impl std::fmt::Debug for AudioDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDevice")
            .field("name", &self.name)
            .field("running", &self.is_running())
            .field("sample_rate", &self.sample_rate)
            .field("channel_count", &self.channel_count)
            .field("max_block", &self.max_block)
            .finish()
    }
}

/// Opens the default output device and starts rendering.
///
/// Falls back to a silent engine when there is no usable output: the returned handle still
/// accepts commands and reports `is_running() == false`, so the application opens and stays
/// editable on a machine with no audio hardware.
pub fn start_audio(settings: &AudioSettings) -> Result<(AudioDevice, EngineHandle), EngineError> {
    match open_output(settings) {
        Ok(setup) => match start_with_device(settings, setup) {
            Ok(started) => Ok(started),
            Err(error) => {
                log::warn!("could not start the audio stream ({error}); running silently");
                Ok(start_silent(settings))
            }
        },
        Err(error) => {
            log::warn!("no audio output available ({error}); running silently");
            Ok(start_silent(settings))
        }
    }
}

fn start_with_device(
    settings: &AudioSettings,
    mut setup: DeviceSetup,
) -> Result<(AudioDevice, EngineHandle), EngineError> {
    let capacity = settings.command_capacity.max(MIN_COMMAND_CAPACITY);
    let (command_tx, command_rx) = crossbeam_channel::bounded(capacity);
    let (graph_tx, graph_rx) = crossbeam_channel::bounded(capacity);
    let playhead = Arc::new(AtomicU64::new(0));
    let count_in = Arc::new(AtomicU64::new(0));
    let meters = Arc::new(MeterBank::new(settings.meter_tracks));
    let running = Arc::new(AtomicBool::new(false));
    let playing = Arc::new(AtomicBool::new(false));
    let latency_stale = Arc::new(AtomicBool::new(false));

    let sample_rate = f64::from(setup.config.sample_rate);
    let channel_count = usize::from(setup.config.channels).max(1);
    let max_block = setup.max_block;
    // cpal takes the callback by value and drops it when the build fails, so the retry below
    // needs a second engine. Building one is cheap and happens off the audio thread; the queues
    // and shared state are the same objects either way.
    let new_engine = || {
        AudioEngine::new(
            command_rx.clone(),
            graph_tx.clone(),
            Arc::clone(&playhead),
            Arc::clone(&count_in),
            Arc::clone(&meters),
            Arc::clone(&playing),
            Arc::clone(&latency_stale),
            sample_rate,
            channel_count,
            max_block,
        )
    };

    let stream = match build_stream(
        &setup.device,
        setup.config,
        setup.sample_format,
        new_engine(),
        Arc::clone(&running),
    ) {
        Ok(stream) => stream,
        // Several backends (WASAPI in particular) refuse an explicit buffer size outright.
        // Losing all audio over a *preference* would be much worse than ignoring it, so retry
        // with whatever the device wants; `fill` chunks the callback to `max_block` regardless.
        Err(error) if matches!(setup.config.buffer_size, BufferSize::Fixed(_)) => {
            log::warn!(
                "the device refused a fixed buffer size ({error}); using its own buffer size"
            );
            setup.config.buffer_size = BufferSize::Default;
            build_stream(
                &setup.device,
                setup.config,
                setup.sample_format,
                new_engine(),
                Arc::clone(&running),
            )?
        }
        Err(error) => return Err(error),
    };
    stream.play()?;
    running.store(true, Ordering::Relaxed);

    let handle = EngineHandle {
        commands: command_tx,
        returned_graphs: graph_rx,
        playhead,
        count_in,
        meters,
        running,
        playing,
        latency_stale,
        sample_rate,
        channel_count,
        max_block: setup.max_block,
    };
    let device = AudioDevice {
        stream: Some(stream),
        hardware: Some(setup.device),
        host: Some(setup.host),
        outputs: setup.outputs,
        // The clone matters: while the stream is alive its engine is the queue's consumer and
        // `discard_pending` refuses to compete with it, but the moment the error callback
        // declares the stream dead, this is what lets the queue be drained at all.
        idle_commands: Some(command_rx),
        running: Arc::clone(&handle.running),
        name: setup.name,
        sample_rate,
        channel_count,
        max_block: setup.max_block,
        sample_format: Some(setup.sample_format),
    };
    Ok((device, handle))
}

/// Builds a handle backed by no hardware at all.
/// Opens an engine with no output device.
///
/// [`start_audio`] falls back to this when the hardware is unusable, but a headless host —
/// a command line tool, a test, an automation server — should call it directly rather than
/// opening the default device only to ignore it. The returned handle accepts every command and
/// reports `is_running() == false`; nothing drains its queue, so such a host should call
/// [`AudioDevice::discard_pending`] periodically.
pub fn start_silent(settings: &AudioSettings) -> (AudioDevice, EngineHandle) {
    let capacity = settings.command_capacity.max(MIN_COMMAND_CAPACITY);
    let (command_tx, command_rx) = crossbeam_channel::bounded(capacity);
    let (_graph_tx, graph_rx) = crossbeam_channel::bounded(capacity);
    let sample_rate = f64::from(settings.sample_rate.unwrap_or(48_000));
    let max_block = settings.block_frames.unwrap_or(512) as usize;

    let handle = EngineHandle {
        commands: command_tx,
        returned_graphs: graph_rx,
        playhead: Arc::new(AtomicU64::new(0)),
        count_in: Arc::new(AtomicU64::new(0)),
        meters: Arc::new(MeterBank::new(settings.meter_tracks)),
        running: Arc::new(AtomicBool::new(false)),
        // A silent engine never renders, so nothing ever sets these; the transport reads as
        // stopped, which is exactly what the UI should show with no output device, and no graph
        // is ever installed for its compensation to go stale.
        playing: Arc::new(AtomicBool::new(false)),
        latency_stale: Arc::new(AtomicBool::new(false)),
        sample_rate,
        channel_count: 2,
        max_block,
    };
    let device = AudioDevice {
        stream: None,
        hardware: None,
        host: None,
        outputs: Vec::new(),
        idle_commands: Some(command_rx),
        running: Arc::clone(&handle.running),
        name: "silent".to_string(),
        sample_rate,
        channel_count: 2,
        max_block,
        sample_format: None,
    };
    (device, handle)
}

/// Everything resolved about the chosen output before a stream exists.
struct DeviceSetup {
    host: String,
    outputs: Vec<AudioDeviceInfo>,
    device: cpal::Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    max_block: usize,
    name: String,
}

fn open_output(settings: &AudioSettings) -> Result<DeviceSetup, EngineError> {
    let host = audio_host(settings.host.as_deref())?;
    let outputs = if host.id().name() == "ASIO" {
        output_devices_for_host(settings.host.as_deref())
    } else {
        Vec::new()
    };
    // A named device that has since been unplugged falls back to the default rather than
    // refusing to start: losing the interface should not also lose the session.
    let device = settings
        .device
        .as_deref()
        .and_then(|wanted| {
            let found = host
                .output_devices()
                .ok()?
                .find(|device| device.to_string() == wanted);
            if found.is_none() {
                log::warn!("output device `{wanted}` is not available; using the default");
            }
            found
        })
        .or_else(|| host.default_output_device())
        .ok_or(EngineError::NoOutputDevice)?;
    let supported = device.default_output_config()?;
    let sample_format = supported.sample_format();
    let mut config = supported.config();

    // Honour the device's own channel count and rate; only override the rate when the caller
    // asked for one the device actually advertises.
    if let Some(rate) = settings.sample_rate
        && rate != config.sample_rate
        && supports_rate(&device, config.channels, sample_format, rate)
    {
        config.sample_rate = rate;
    }

    let (min_buffer, max_buffer) = match supported.buffer_size() {
        SupportedBufferSize::Range { min, max } => (*min, *max),
        SupportedBufferSize::Unknown => (0, 0),
    };
    let max_block = match settings.block_frames {
        Some(frames) => {
            let frames = preferred_block_size(frames, min_buffer, max_buffer);
            config.buffer_size = BufferSize::Fixed(frames);
            frames as usize
        }
        // The callback may still deliver more than this; the renderer splits what it gets.
        None if max_buffer > 0 => (max_buffer as usize).min(DEFAULT_MAX_BLOCK),
        None => DEFAULT_MAX_BLOCK,
    };

    Ok(DeviceSetup {
        host: host.id().name().to_owned(),
        outputs,
        name: device.to_string(),
        device,
        config,
        sample_format,
        max_block: max_block.max(1),
    })
}

fn preferred_block_size(frames: u32, min_buffer: u32, max_buffer: u32) -> u32 {
    if max_buffer > 0 {
        // `clamp` panics when its bounds cross, which a backend advertising a degenerate range
        // would otherwise make happen inside what is supposed to be a fallible open.
        let lowest = min_buffer.max(1).min(max_buffer);
        frames.clamp(lowest, max_buffer)
    } else {
        frames.max(1).min(DEFAULT_MAX_BLOCK as u32)
    }
}

fn supports_rate(
    device: &cpal::Device,
    channels: cpal::ChannelCount,
    format: SampleFormat,
    rate: cpal::SampleRate,
) -> bool {
    match device.supported_output_configs() {
        Ok(configs) => configs.into_iter().any(|range| {
            range.channels() == channels
                && range.sample_format() == format
                && range.min_sample_rate() <= rate
                && rate <= range.max_sample_rate()
        }),
        Err(_) => false,
    }
}

/// Whether the stream is still running after reporting this error.
///
/// cpal's error callback carries more than deaths. `DeviceChanged` is the default device being
/// rerouted under a still-running stream — its own documentation says no rebuild is required —
/// and `RealtimeDenied` is a refused scheduling promotion on a stream that keeps playing.
/// Treating those as fatal would clear `running` while the audio thread is still consuming the
/// command queue, and [`AudioDevice::discard_pending`] would then start draining commands out
/// from under it: two consumers on one queue, every later command landing on either at random.
pub(crate) fn stream_survives(kind: cpal::ErrorKind) -> bool {
    matches!(
        kind,
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

fn build_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    engine: AudioEngine,
    running: Arc<AtomicBool>,
) -> Result<cpal::Stream, EngineError> {
    // cpal reports stream errors from a thread of its own, not from the audio callback, so a
    // log is allowed here — but a log alone tells nobody. Unplugging the device stops the
    // callback for good while the stream object lives on, and everything reading `is_running`
    // would go on seeing a live engine: the playhead frozen, the queue filling, every later
    // command silently refused. Clearing the flag is what turns "the device is gone" into a
    // state the rest of the application can see — but only for an error that really is a
    // death; see `stream_survives`.
    let on_error = move |error: cpal::Error| {
        if stream_survives(error.kind()) {
            log::warn!("audio stream notice: {error}; the stream keeps running");
            return;
        }
        running.store(false, Ordering::Relaxed);
        log::error!("audio stream error: {error}; the output stream is dead");
    };
    let mut engine = engine;
    let stream = match format {
        SampleFormat::I8 => device.build_output_stream(
            config,
            move |data: &mut [i8], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::I24 => device.build_output_stream(
            config,
            move |data: &mut [I24], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::I32 => device.build_output_stream(
            config,
            move |data: &mut [i32], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::I64 => device.build_output_stream(
            config,
            move |data: &mut [i64], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::U8 => device.build_output_stream(
            config,
            move |data: &mut [u8], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::U24 => device.build_output_stream(
            config,
            move |data: &mut [U24], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::U32 => device.build_output_stream(
            config,
            move |data: &mut [u32], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::U64 => device.build_output_stream(
            config,
            move |data: &mut [u64], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        SampleFormat::F64 => device.build_output_stream(
            config,
            move |data: &mut [f64], _: &cpal::OutputCallbackInfo| engine.fill(data),
            on_error,
            None,
        )?,
        other => return Err(EngineError::UnsupportedSampleFormat(other.to_string())),
    };
    Ok(stream)
}

/// The audio-thread half of the engine.
pub(crate) struct AudioEngine {
    graph: Option<Box<RenderGraph>>,
    transport: Transport,
    commands: Receiver<EngineCommand>,
    returned_graphs: Sender<Retired>,
    /// Retired data waiting for room in the return queue. Fixed capacity: pushing never
    /// allocates.
    ///
    /// The `Box` inside [`Retired::Graph`] is what travels down the return channel, and
    /// unwrapping it here would mean copying a large struct out of the heap and boxing it
    /// again to send it — an allocation on the audio thread, which is exactly what this whole
    /// mechanism exists to avoid.
    retired: Vec<Retired>,
    /// One ownership-carrying command deferred while the return path is full.
    deferred_retiring: Option<EngineCommand>,
    playhead: Arc<AtomicU64>,
    /// Frames of count-in left, published beside the playhead and read by the same stranger.
    ///
    /// A take started with a count-in in front of it begins writing at once, so its file opens
    /// with however much of the count the input stream happened to catch. The two streams run on
    /// their own clocks and neither can say where the other began — but the pair of numbers a
    /// capture stamps together, at the first block that reaches it, can. This is the other half
    /// of that pair.
    count_in: Arc<AtomicU64>,
    /// Whether the last callback had a count-in running, so the end of one is published once.
    ///
    /// The cell above is written from both threads and this is what keeps them out of each
    /// other's way: the audio thread only writes it while it is counting, and for the one
    /// callback that finishes a count.
    counting_in: bool,
    meters: Arc<MeterBank>,
    /// Mirrors `transport.playing` for the UI, published once per callback.
    playing: Arc<AtomicBool>,
    /// Set when the graph's delay compensation no longer matches what its chains need.
    ///
    /// Published only when a command could have changed it, rather than every callback: the
    /// answer costs a virtual call per effect, and nothing but writing an effect parameter can
    /// move it — every other way of changing a chain arrives as a whole new graph.
    latency_stale: Arc<AtomicBool>,
    scratch: AudioBuffer,
    sample_rate: f64,
    channels: usize,
    max_block: usize,
}

impl AudioEngine {
    // Ten arguments, all of them shared state the callback needs; grouping them into a struct
    // would only move the same list one level down.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        commands: Receiver<EngineCommand>,
        returned_graphs: Sender<Retired>,
        playhead: Arc<AtomicU64>,
        count_in: Arc<AtomicU64>,
        meters: Arc<MeterBank>,
        playing: Arc<AtomicBool>,
        latency_stale: Arc<AtomicBool>,
        sample_rate: f64,
        channels: usize,
        max_block: usize,
    ) -> Self {
        let max_block = max_block.max(1);
        let mut scratch = AudioBuffer::new(channels.max(1), max_block, sample_rate);
        scratch.reserve_frames(max_block);
        Self {
            graph: None,
            transport: Transport::new(),
            commands,
            returned_graphs,
            retired: Vec::with_capacity(RETIRED_GRAPH_SLOTS),
            deferred_retiring: None,
            playhead,
            count_in,
            counting_in: false,
            meters,
            playing,
            latency_stale,
            scratch,
            sample_rate,
            channels: channels.max(1),
            max_block,
        }
    }

    /// Republishes whether the graph's compensation still matches its chains.
    fn publish_latency(&self) {
        let stale = self
            .graph
            .as_ref()
            .is_some_and(|graph| graph.latency_is_stale());
        self.latency_stale.store(stale, Ordering::Relaxed);
    }

    /// Fills one interleaved device buffer. This is the realtime path.
    pub(crate) fn fill<T>(&mut self, data: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
        self.poll_commands();

        let frames = data.len() / self.channels;
        let mut written = 0;
        while written < frames {
            let count = (frames - written).min(self.max_block);
            self.scratch.set_frame_count(count);
            match &mut self.graph {
                Some(graph) => {
                    render_block(graph, &mut self.transport, &mut self.scratch, false);
                }
                None => self.scratch.clear(),
            }
            let start = written * self.channels;
            let end = start + count * self.channels;
            interleave(&self.scratch, &mut data[start..end], self.channels);
            self.publish_meters(count);
            written += count;
        }
        // Automation writes effect parameters while rendering, and those parameters may change
        // a plugin's declared latency. Re-check once per callback after every such write.
        self.publish_latency();
        // A trailing partial frame the device asked for is silence rather than stale samples.
        for sample in &mut data[frames * self.channels..] {
            *sample = T::from_sample(0.0f32);
        }

        self.playhead
            .store(self.transport.position_frames, Ordering::Relaxed);
        // Only while there is a count to report, and once more when it ends. Publishing a zero
        // every callback would look harmless and is the one thing this cell must not do: the UI
        // thread writes the count it is *about* to ask for before sending the command, because an
        // input stream can hand over its first block before the audio thread has picked the
        // command up — and a callback in that gap would overwrite the figure with a zero, which
        // reads as "the count is over" and leaves a whole count-in at the head of the take.
        match self.transport.frames_to_count_in_end() {
            Some(left) => {
                self.count_in.store(left, Ordering::Relaxed);
                self.counting_in = true;
            }
            None if self.counting_in => {
                self.count_in.store(0, Ordering::Relaxed);
                self.counting_in = false;
            }
            None => {}
        }
        self.playing
            .store(self.transport.playing, Ordering::Relaxed);
    }

    fn publish_meters(&self, frames: usize) {
        let Some(graph) = &self.graph else {
            return;
        };
        let metered = graph.track_count().min(self.meters.track_capacity());
        for index in 0..metered {
            self.meters
                .report_track(index, graph.track_peak(index), frames, self.sample_rate);
        }
        for channel in 0..2 {
            self.meters.report_master(
                channel,
                graph.master_channel_peak(channel),
                frames,
                self.sample_rate,
            );
        }
    }

    fn poll_commands(&mut self) {
        self.flush_retired();
        if self.retired.len() < RETIRED_GRAPH_SLOTS
            && let Some(command) = self.deferred_retiring.take()
        {
            self.apply(command);
        }
        while self.deferred_retiring.is_none() {
            let Ok(command) = self.commands.try_recv() else {
                break;
            };
            if self.retired.len() == RETIRED_GRAPH_SLOTS && command_may_retire(&command) {
                self.deferred_retiring = Some(command);
                break;
            }
            self.apply(command);
        }
    }

    /// Hands stashed retirements back to the UI thread as soon as the queue has room.
    fn flush_retired(&mut self) {
        while let Some(load) = self.retired.pop() {
            match self.returned_graphs.try_send(load) {
                Ok(()) => {}
                Err(TrySendError::Full(load)) => {
                    self.retired.push(load);
                    break;
                }
                // The UI is gone, so this is shutdown and there is nobody left to free it.
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
    }

    fn retire(&mut self, load: Retired) {
        match self.returned_graphs.try_send(load) {
            Ok(()) => {}
            Err(TrySendError::Full(load)) => self.retired.push(load),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn apply(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::SetGraph(graph) => {
                // Meters past the new track count would otherwise sit at the level a deleted
                // track last reached, with nothing left to report them down.
                self.meters.clear_tracks_from(graph.track_count());
                if let Some(previous) = self.graph.replace(graph) {
                    self.retire(Retired::Graph(previous));
                }
                // A freshly built graph is compensated for its own chains, so this clears any
                // staleness the graph it replaced had reported.
                self.publish_latency();
            }
            EngineCommand::Play => self.transport.playing = true,
            EngineCommand::CountIn(count) => self.transport.set_count_in(count),
            EngineCommand::Stop => {
                self.transport.playing = false;
                // A count nobody is going to play to is over. Left running, it would hold the
                // next press of Play at the position this one was stopped at, counting out the
                // rest of a bar somebody has already walked away from.
                self.transport.count_in = None;
                if let Some(graph) = &mut self.graph {
                    graph.reset_voices();
                }
            }
            EngineCommand::Seek { frames } => {
                self.transport.seek(frames);
                // Counted in to a position the transport is no longer at.
                self.transport.count_in = None;
                // Jumping the playhead must not leave notes hanging.
                if let Some(graph) = &mut self.graph {
                    graph.reset_voices();
                }
            }
            EngineCommand::SetLoop {
                enabled,
                start,
                end,
            } => self.transport.set_loop(enabled, start, end),
            EngineCommand::SetTrackGain { index, gain_db } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_track_gain_db(index, gain_db);
                }
            }
            EngineCommand::SetTrackPan { index, pan } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_track_pan(index, pan);
                }
            }
            EngineCommand::SetTrackMute { index, mute } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_track_mute(index, mute);
                }
            }
            EngineCommand::SetSoloResolution(audible) => {
                if let Some(graph) = &mut self.graph {
                    graph.set_live_audible(&audible);
                }
                self.retire(Retired::SoloResolution(audible));
            }
            EngineCommand::SetSendLevel {
                track,
                send,
                level_db,
            } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_send_level_db(track, send, level_db);
                }
            }
            EngineCommand::SetMasterGain(gain_db) => {
                if let Some(graph) = &mut self.graph {
                    graph.set_master_gain_db(gain_db);
                }
            }
            EngineCommand::SetMasterPan(pan) => {
                if let Some(graph) = &mut self.graph {
                    graph.set_master_pan(pan);
                }
            }
            EngineCommand::SetEffectParam {
                track,
                slot,
                param,
                value,
            } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_effect_param(track, slot, param, value);
                }
                // A look-ahead length is a parameter like any other, so this is the one command
                // that can leave the delay lines compensating for the wrong number of frames.
                self.publish_latency();
            }
            EngineCommand::SetInstrumentParam {
                track,
                param,
                value,
            } => {
                if let Some(graph) = &mut self.graph {
                    graph.set_instrument_param(track, param, value);
                }
            }
            EngineCommand::NoteOn {
                track,
                pitch,
                velocity,
            } => {
                if let Some(graph) = &mut self.graph {
                    graph.note_on(track, pitch, velocity);
                }
            }
            EngineCommand::NoteOff { track, pitch } => {
                if let Some(graph) = &mut self.graph {
                    graph.note_off(track, pitch);
                }
            }
            EngineCommand::PitchBend { track, semitones } => {
                if let Some(graph) = &mut self.graph {
                    graph.pitch_bend(track, semitones);
                }
            }
            EngineCommand::Controller {
                track,
                number,
                value,
            } => {
                if let Some(graph) = &mut self.graph {
                    graph.controller(track, number, value);
                }
            }
            EngineCommand::PlayOneShot { track, buffer } => match &mut self.graph {
                Some(graph) => {
                    if let Some(previous) = graph.play_one_shot(track, buffer) {
                        self.retire(Retired::Buffer(previous));
                    }
                }
                // No graph, nowhere to play it — and nowhere to free it either.
                None => self.retire(Retired::Buffer(buffer)),
            },
            EngineCommand::StopOneShot { track } => {
                if let Some(graph) = &mut self.graph {
                    graph.stop_one_shot(track);
                }
            }
            EngineCommand::SetMetronome(enabled) => {
                if let Some(graph) = &mut self.graph {
                    graph.set_metronome(enabled);
                }
            }
            EngineCommand::Panic => {
                if let Some(graph) = &mut self.graph {
                    graph.panic();
                }
                self.meters.reset();
            }
        }
    }
}

fn command_may_retire(command: &EngineCommand) -> bool {
    matches!(
        command,
        EngineCommand::SetGraph(_)
            | EngineCommand::SetSoloResolution(_)
            | EngineCommand::PlayOneShot { .. }
    )
}

/// Writes the planar mix bus into an interleaved device buffer.
fn interleave<T>(source: &AudioBuffer, data: &mut [T], channels: usize)
where
    T: Sample + FromSample<f32>,
{
    for (frame, slot) in data.chunks_mut(channels).enumerate() {
        for (channel, sample) in slot.iter_mut().enumerate() {
            *sample = T::from_sample(source.sample(channel, frame));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_device_ranges_include_standard_sample_rates() {
        let mut rates = Vec::new();
        add_sample_rate_range(&mut rates, 8_000, 192_000);
        rates.sort_unstable();
        for expected in [44_100, 48_000, 88_200, 96_000] {
            assert!(rates.contains(&expected), "missing {expected} Hz");
        }
    }
    use crate::testkit::{self, TONE_AMPLITUDE};
    use crate::transport::CountIn;
    use auris_core::ParamId;
    use auris_core::automation::AutomationCurve;
    use auris_core::param::ParamTarget;
    use auris_core::project::{AudioSourceBank, Note, Project};
    use auris_core::time::Ticks;

    const SAMPLE_RATE: f64 = 48_000.0;

    #[test]
    fn failed_stream_is_dropped_before_its_command_queue_is_drained() {
        struct DropSignal(Arc<AtomicBool>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let mut stream = Some(DropSignal(Arc::clone(&stopped)));
        let drained = after_stream_stopped(&mut stream, || {
            assert!(stopped.load(Ordering::Relaxed));
            2
        });

        assert!(stream.is_none());
        assert_eq!(drained, 2);
    }

    #[test]
    fn a_rerouted_default_device_is_not_a_dead_stream() {
        // cpal documents both of these as errors the stream survives; treating them as deaths
        // hands the command queue to `discard_pending` while the callback is still consuming it.
        assert!(stream_survives(cpal::ErrorKind::DeviceChanged));
        assert!(stream_survives(cpal::ErrorKind::RealtimeDenied));
    }

    #[test]
    fn a_lost_device_still_kills_the_stream() {
        assert!(!stream_survives(cpal::ErrorKind::DeviceNotAvailable));
        assert!(!stream_survives(cpal::ErrorKind::StreamInvalidated));
        assert!(!stream_survives(cpal::ErrorKind::HostUnavailable));
    }

    fn held_note_project() -> Project {
        let mut project = Project::new("Device", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(8.0),
        ));
        project
    }

    fn graph() -> Box<RenderGraph> {
        Box::new(RenderGraph::build(
            &held_note_project(),
            &AudioSourceBank::new(),
            &testkit::registry(),
            256,
        ))
    }

    #[allow(clippy::type_complexity)]
    fn engine() -> (
        AudioEngine,
        Sender<EngineCommand>,
        Receiver<Retired>,
        Arc<MeterBank>,
        Arc<AtomicU64>,
    ) {
        let (command_tx, command_rx) = crossbeam_channel::bounded(16);
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(16);
        let meters = Arc::new(MeterBank::new(8));
        let playhead = Arc::new(AtomicU64::new(0));
        let engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::clone(&playhead),
            Arc::new(AtomicU64::new(0)),
            Arc::clone(&meters),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            2,
            256,
        );
        (engine, command_tx, graph_rx, meters, playhead)
    }

    /// The same, with the count-in cell handed back as well.
    ///
    /// The return queue comes back too, and only because it must outlive the engine: a receiver
    /// dropped early turns every retired graph into a stash entry, which is another test's
    /// subject and noise in this one.
    #[allow(clippy::type_complexity)]
    fn counting_engine() -> (
        AudioEngine,
        Sender<EngineCommand>,
        Arc<AtomicU64>,
        Receiver<Retired>,
    ) {
        let (command_tx, command_rx) = crossbeam_channel::bounded(16);
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(16);
        let count_in = Arc::new(AtomicU64::new(0));
        let engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::clone(&count_in),
            Arc::new(MeterBank::new(8)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            2,
            256,
        );
        (engine, command_tx, count_in, graph_rx)
    }

    #[test]
    fn a_count_in_that_has_been_asked_for_is_not_published_away_before_it_arrives() {
        // The bug this exists for: a take opens the moment Record is pressed, and its first block
        // of audio can reach the capture before the audio thread has picked the command up. The
        // count is therefore written down by the thread that asked for it — and a callback in
        // that gap must not put a zero back over it, which reads as "the count is over" and
        // leaves a whole count-in at the head of the take.
        let (mut engine, commands, count_in, _graphs) = counting_engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        let mut data = [0.0f32; 512];

        count_in.store(1_000, Ordering::Relaxed);
        engine.fill(&mut data);
        assert_eq!(
            count_in.load(Ordering::Relaxed),
            1_000,
            "the callback published a zero over a count that had been asked for"
        );

        // Once the command lands, the audio thread owns the figure and counts it down. 512
        // samples of interleaved stereo is 256 frames.
        commands
            .send(EngineCommand::CountIn(CountIn::new(1, 1_000, 1)))
            .unwrap();
        commands.send(EngineCommand::Play).unwrap();
        engine.fill(&mut data);
        assert_eq!(count_in.load(Ordering::Relaxed), 744);

        // And the end of the count is published, once, so the take after this one is not trimmed
        // by a count nobody played.
        for _ in 0..4 {
            engine.fill(&mut data);
        }
        assert_eq!(count_in.load(Ordering::Relaxed), 0);
        count_in.store(2_000, Ordering::Relaxed);
        engine.fill(&mut data);
        assert_eq!(
            count_in.load(Ordering::Relaxed),
            2_000,
            "the cell was not left alone"
        );
    }

    #[test]
    fn a_cancelled_count_in_is_published_as_over() {
        let (mut engine, commands, count_in, _graphs) = counting_engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        let mut data = [0.0f32; 512];
        commands
            .send(EngineCommand::CountIn(CountIn::new(8, 24_000, 4)))
            .unwrap();
        commands.send(EngineCommand::Play).unwrap();
        engine.fill(&mut data);
        assert!(count_in.load(Ordering::Relaxed) > 0);

        commands.send(EngineCommand::Stop).unwrap();
        engine.fill(&mut data);
        assert_eq!(count_in.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_engine_without_a_graph_writes_silence() {
        let (mut engine, _commands, _graphs, _meters, playhead) = engine();
        let mut data = [1.0f32; 512];
        engine.fill(&mut data);
        assert!(data.iter().all(|sample| *sample == 0.0));
        assert_eq!(playhead.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn installing_a_graph_and_playing_produces_audio() {
        let (mut engine, commands, _graphs, _meters, playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();

        let mut data = [0.0f32; 1_024];
        engine.fill(&mut data);
        // 1024 samples of interleaved stereo is 512 frames.
        assert_eq!(playhead.load(Ordering::Relaxed), 512);
        for sample in data {
            assert!((sample - TONE_AMPLITUDE).abs() < 1e-5, "got {sample}");
        }
    }

    #[test]
    fn a_replaced_one_shot_travels_back_as_retired_data() {
        // The Arc must never lose its last reference on the audio thread, so the swap hands
        // the previous buffer up the same channel retired graphs use.
        let (mut engine, commands, graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        let voice = || {
            let mut buffer = AudioBuffer::new(1, 64, SAMPLE_RATE);
            buffer.channel_mut(0)[..64].fill(0.5);
            Arc::new(buffer)
        };
        commands
            .send(EngineCommand::PlayOneShot {
                track: 0,
                buffer: voice(),
            })
            .unwrap();
        let mut data = [0.0f32; 64];
        engine.fill(&mut data);
        assert_eq!(
            graphs.try_iter().count(),
            0,
            "the first play replaces nothing"
        );

        commands
            .send(EngineCommand::PlayOneShot {
                track: 0,
                buffer: voice(),
            })
            .unwrap();
        engine.fill(&mut data);
        let retired: Vec<Retired> = graphs.try_iter().collect();
        assert_eq!(retired.len(), 1);
        assert!(
            matches!(retired[0], Retired::Buffer(_)),
            "the replaced buffer travels back to the UI thread"
        );
    }

    #[test]
    fn replacing_a_graph_hands_the_old_one_back() {
        let (mut engine, commands, graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        let mut data = [0.0f32; 64];
        engine.fill(&mut data);
        assert_eq!(graphs.try_iter().count(), 0);

        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        engine.fill(&mut data);
        assert_eq!(
            graphs.try_iter().count(),
            1,
            "the retired graph must travel back to the UI thread"
        );
    }

    #[test]
    fn stop_and_seek_release_sounding_notes() {
        let (mut engine, commands, _graphs, _meters, playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        let mut data = [0.0f32; 256];
        engine.fill(&mut data);

        commands.send(EngineCommand::Stop).unwrap();
        engine.fill(&mut data);
        assert!(data.iter().all(|sample| sample.abs() < 1e-6));
        assert_eq!(playhead.load(Ordering::Relaxed), 128);

        commands
            .send(EngineCommand::Seek { frames: 96_000 })
            .unwrap();
        engine.fill(&mut data);
        assert_eq!(playhead.load(Ordering::Relaxed), 96_000);
    }

    #[test]
    fn meters_follow_the_master_bus() {
        let (mut engine, commands, _graphs, meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        let mut data = [0.0f32; 512];
        engine.fill(&mut data);
        assert!((meters.master_peak() - TONE_AMPLITUDE).abs() < 1e-5);
        assert!((meters.track_peak(0) - TONE_AMPLITUDE).abs() < 1e-5);

        commands.send(EngineCommand::Panic).unwrap();
        engine.fill(&mut data);
        assert!(meters.master_peak() < 1e-5);
    }

    #[test]
    fn a_master_fader_command_reaches_the_graph() {
        let (mut engine, commands, _graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        commands
            .send(EngineCommand::SetMasterGain(-6.020_6))
            .unwrap();
        let mut data = [0.0f32; 512];
        engine.fill(&mut data);
        engine.fill(&mut data);
        for sample in data {
            assert!((sample - TONE_AMPLITUDE * 0.5).abs() < 1e-4, "got {sample}");
        }
    }

    #[test]
    fn integer_output_formats_are_converted() {
        let (mut engine, commands, _graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();

        let mut signed = [0i16; 128];
        engine.fill(&mut signed);
        // 0.5 of full scale in i16 is around 16 384.
        assert!((signed[0] as i32 - 16_384).abs() < 64, "got {}", signed[0]);

        let mut unsigned = [0u16; 128];
        engine.fill(&mut unsigned);
        // The same level in u16, whose origin is 32 768.
        assert!(
            (unsigned[0] as i32 - 49_152).abs() < 64,
            "got {}",
            unsigned[0]
        );
    }

    fn assert_output_sample<T>(engine: &mut AudioEngine)
    where
        T: Sample + FromSample<f32> + Copy,
        f32: FromSample<T>,
    {
        let mut data = vec![T::from_sample(0.0); 128];
        engine.fill(&mut data);
        let sample = f32::from_sample(data[0]);
        assert!((sample - TONE_AMPLITUDE).abs() < 0.02, "got {sample}");
    }

    #[test]
    fn every_pcm_output_format_is_converted() {
        let (mut engine, commands, _graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();

        assert_output_sample::<i8>(&mut engine);
        assert_output_sample::<i16>(&mut engine);
        assert_output_sample::<I24>(&mut engine);
        assert_output_sample::<i32>(&mut engine);
        assert_output_sample::<i64>(&mut engine);
        assert_output_sample::<u8>(&mut engine);
        assert_output_sample::<u16>(&mut engine);
        assert_output_sample::<U24>(&mut engine);
        assert_output_sample::<u32>(&mut engine);
        assert_output_sample::<u64>(&mut engine);
        assert_output_sample::<f32>(&mut engine);
        assert_output_sample::<f64>(&mut engine);
    }

    #[test]
    fn an_unknown_device_buffer_range_bounds_the_preferred_block() {
        assert_eq!(preferred_block_size(0, 0, 0), 1);
        assert_eq!(
            preferred_block_size(u32::MAX, 0, 0),
            DEFAULT_MAX_BLOCK as u32
        );
        assert_eq!(preferred_block_size(512, 0, 0), 512);
    }

    #[test]
    fn a_live_solo_resolution_fades_instead_of_stepping() {
        let (mut engine, commands, retired, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();

        let mut before = [0.0f32; 256];
        engine.fill(&mut before);
        assert!((before[0] - TONE_AMPLITUDE).abs() < 1e-5);

        commands
            .send(EngineCommand::SetSoloResolution(
                vec![false].into_boxed_slice(),
            ))
            .unwrap();
        let mut after = [0.0f32; 512];
        engine.fill(&mut after);

        assert!(
            (after[0] - before[254]).abs() < 1e-5,
            "solo resolution stepped from {} to {}",
            before[254],
            after[0]
        );
        assert_eq!(after[510], 0.0, "the fade never reached silence");
        assert!(matches!(retired.try_recv(), Ok(Retired::SoloResolution(_))));
    }

    /// Builds an engine for a device with `channels` channels.
    #[allow(clippy::type_complexity)]
    fn engine_with_channels(
        channels: usize,
    ) -> (AudioEngine, Sender<EngineCommand>, Receiver<Retired>) {
        let (command_tx, command_rx) = crossbeam_channel::bounded(64);
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(64);
        let engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(MeterBank::new(8)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            channels,
            256,
        );
        (engine, command_tx, graph_rx)
    }

    #[test]
    fn a_mono_device_gets_the_downmix() {
        let (mut engine, commands, _graphs) = engine_with_channels(1);
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        // 300 frames through a 256-frame engine also exercises the chunk split.
        let mut data = [0.0f32; 300];
        engine.fill(&mut data);
        for sample in data {
            assert!((sample - TONE_AMPLITUDE).abs() < 1e-5, "got {sample}");
        }
    }

    #[test]
    fn a_seven_channel_device_is_fed_without_touching_the_extra_channels() {
        let channels = 7;
        let (mut engine, commands, _graphs) = engine_with_channels(channels);
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        // Deliberately not a whole number of frames.
        let mut data = [1.0f32; 7 * 300 + 3];
        engine.fill(&mut data);
        for (index, sample) in data.iter().enumerate() {
            if index >= 7 * 300 {
                assert_eq!(*sample, 0.0, "the trailing partial frame must be silence");
            } else if index % channels < 2 {
                assert!((sample - TONE_AMPLITUDE).abs() < 1e-5, "index {index}");
            } else {
                assert_eq!(*sample, 0.0, "channel {} must be silent", index % channels);
            }
        }
    }

    #[test]
    fn a_parameter_that_moves_a_plugins_latency_reports_the_graph_as_stale() {
        // Every other way of changing a chain arrives as a whole new graph, so this is the only
        // path that can leave the delay lines compensating for a length no longer being used.
        // The audio thread cannot re-size them itself — that would allocate — so it raises a flag
        // and the session rebuilds.
        let (command_tx, command_rx) = crossbeam_channel::bounded(16);
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(16);
        let latency_stale = Arc::new(AtomicBool::new(false));
        let mut engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(MeterBank::new(8)),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&latency_stale),
            SAMPLE_RATE,
            2,
            256,
        );

        let mut project = held_note_project();
        let track = project.tracks[0].id;
        project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
        project.add_instrument_track("Plain", testkit::TONE_ID);
        let built = Box::new(RenderGraph::build(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            256,
        ));

        let mut data = [0.0f32; 512];
        command_tx.send(EngineCommand::SetGraph(built)).unwrap();
        engine.fill(&mut data);
        assert!(
            !latency_stale.load(Ordering::Relaxed),
            "a freshly built graph compensates for its own chains"
        );

        command_tx
            .send(EngineCommand::SetEffectParam {
                track: Some(0),
                slot: 0,
                param: ParamId(0),
                value: 0.0,
            })
            .unwrap();
        engine.fill(&mut data);
        assert!(
            latency_stale.load(Ordering::Relaxed),
            "shortening the look-ahead left the other track compensated for the old length"
        );

        // Installing a rebuilt graph clears it again.
        let rebuilt = Box::new(RenderGraph::build(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            256,
        ));
        command_tx.send(EngineCommand::SetGraph(rebuilt)).unwrap();
        engine.fill(&mut data);
        assert!(!latency_stale.load(Ordering::Relaxed));
        drop(graph_rx);
    }

    #[test]
    fn automation_that_moves_a_plugins_latency_reports_the_graph_as_stale() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(16);
        let (graph_tx, _graph_rx) = crossbeam_channel::bounded(16);
        let latency_stale = Arc::new(AtomicBool::new(false));
        let mut engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(MeterBank::new(8)),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&latency_stale),
            SAMPLE_RATE,
            2,
            256,
        );

        let mut project = held_note_project();
        let track = project.tracks[0].id;
        let slot = project
            .add_effect(Some(track), testkit::LOOKAHEAD_ID)
            .expect("the track exists");
        project.automation.set_point(
            ParamTarget::Effect {
                track: Some(track),
                slot,
                param: ParamId(0),
            },
            None,
            AutomationCurve::Linear,
            Ticks::ZERO,
            0.0,
        );
        let graph = Box::new(RenderGraph::build(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            256,
        ));

        command_tx.send(EngineCommand::SetGraph(graph)).unwrap();
        command_tx.send(EngineCommand::Play).unwrap();
        engine.fill(&mut [0.0f32; 512]);

        assert!(
            latency_stale.load(Ordering::Relaxed),
            "the automated write changed the plugin after compensation was built"
        );
    }

    #[test]
    fn retired_graphs_survive_a_full_return_queue() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(64);
        // A return queue far too small for the traffic, so the stash is exercised.
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(1);
        let mut engine = AudioEngine::new(
            command_rx,
            graph_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(MeterBank::new(8)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            2,
            128,
        );
        let mut data = [0.0f32; 256];
        let sent = 40;
        let mut returned = 0;
        for _ in 0..sent {
            let _ = command_tx.try_send(EngineCommand::SetGraph(graph()));
            engine.fill(&mut data);
            returned += graph_rx.try_iter().count();
        }
        for _ in 0..sent {
            engine.fill(&mut data);
            returned += graph_rx.try_iter().count();
        }
        // Every graph but the one still installed must have reached the UI thread to be dropped.
        assert_eq!(
            returned,
            sent - 1,
            "a retired graph was dropped or stranded"
        );
    }

    #[test]
    fn a_full_retirement_stash_does_not_block_transport_commands() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(16);
        let (return_tx, return_rx) = crossbeam_channel::bounded(1);
        return_tx
            .send(Retired::SoloResolution(Vec::new().into_boxed_slice()))
            .unwrap();
        let mut engine = AudioEngine::new(
            command_rx,
            return_tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(MeterBank::new(0)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            2,
            128,
        );
        for _ in 0..RETIRED_GRAPH_SLOTS {
            engine
                .retired
                .push(Retired::SoloResolution(Vec::new().into_boxed_slice()));
        }
        command_tx.send(EngineCommand::Play).unwrap();

        engine.poll_commands();

        assert!(engine.transport.playing);
        assert_eq!(engine.retired.len(), RETIRED_GRAPH_SLOTS);
        drop(return_rx);
    }

    #[test]
    fn installing_a_smaller_graph_clears_the_meters_of_the_tracks_that_went_away() {
        let (mut engine, commands, _graphs, _meters, _playhead) = engine();
        let mut two_tracks = held_note_project();
        two_tracks.add_instrument_track("Second", testkit::TONE_ID);
        commands
            .send(EngineCommand::SetGraph(Box::new(RenderGraph::build(
                &two_tracks,
                &AudioSourceBank::new(),
                &testkit::registry(),
                256,
            ))))
            .unwrap();
        commands.send(EngineCommand::Play).unwrap();
        let mut data = [0.0f32; 512];
        engine.fill(&mut data);
        assert!(engine.meters.track_peak(0) > 0.0);

        // The second track is deleted; its meter must not keep the level for ever.
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        engine.fill(&mut data);
        assert_eq!(engine.meters.track_peak(1), 0.0);
        assert!(engine.meters.track_peak(0) > 0.0);
    }

    #[test]
    fn the_audio_callback_never_allocates() {
        let (mut engine, commands, _graphs, _meters, _playhead) = engine();
        commands.send(EngineCommand::SetGraph(graph())).unwrap();
        commands.send(EngineCommand::Play).unwrap();
        let mut data = [0.0f32; 1_024];
        // Warm up outside the watched region.
        engine.fill(&mut data);

        let allocations = testkit::count_allocations(|| {
            for _ in 0..100 {
                engine.fill(&mut data);
            }
        });
        assert_eq!(allocations, 0, "the cpal callback allocated");
    }

    #[test]
    fn a_silent_engine_still_accepts_commands() {
        let (mut device, handle) = start_silent(&AudioSettings::default());
        assert!(!handle.is_running());
        assert!(!device.is_running());
        assert_eq!(device.name(), "silent");
        assert_eq!(device.host(), None);
        assert_eq!(device.buffer_frames(), None);
        assert_eq!(handle.sample_rate(), 48_000.0);
        handle.send(EngineCommand::Play).expect("queued");
        handle.set_graph(*graph()).expect("queued");
        assert_eq!(handle.collect_garbage(), 0);
        assert_eq!(handle.playhead_frames(), 0);
        assert_eq!(device.discard_pending(), 2);
    }

    #[test]
    fn an_unavailable_host_does_not_select_the_default_backend() {
        assert!(matches!(
            audio_host(Some("nonexistent-auris-test-host")),
            Err(EngineError::HostUnavailable(_))
        ));
    }

    #[test]
    fn a_full_command_queue_is_reported_rather_than_blocking() {
        let (_device, handle) = start_silent(&AudioSettings {
            command_capacity: 1,
            ..AudioSettings::default()
        });
        // The floor is MIN_COMMAND_CAPACITY, so fill past it.
        for _ in 0..MIN_COMMAND_CAPACITY {
            handle.send(EngineCommand::Play).expect("queued");
        }
        assert!(matches!(
            handle.send(EngineCommand::Play),
            Err(EngineError::CommandQueueFull)
        ));
    }
}
