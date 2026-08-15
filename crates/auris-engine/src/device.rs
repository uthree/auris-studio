//! Opening the audio hardware and driving the renderer from its callback.
//!
//! The callback thread owns exactly one [`RenderGraph`] and one [`Transport`]. It talks to the
//! UI through two bounded queues and nothing else: commands come down one, retired graphs go
//! back up the other so that plugin instances and sample buffers are freed on the UI thread.
//! Inside the callback there is no allocation, no lock, no logging and no I/O.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use auris_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, StreamConfig, SupportedBufferSize};
use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::command::EngineCommand;
use crate::error::EngineError;
use crate::graph::{RETIRED_GRAPH_SLOTS, RenderGraph};
use crate::handle::EngineHandle;
use crate::meter::MeterBank;
use crate::renderer::render_block;
use crate::transport::Transport;

/// Block size used when the backend will not say what it prefers.
const DEFAULT_MAX_BLOCK: usize = 2_048;

/// Smallest command queue that still absorbs one UI frame's worth of fader moves.
const MIN_COMMAND_CAPACITY: usize = 16;

/// What to ask the audio backend for.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSettings {
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
    let host = cpal::default_host();
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
                    for rate in [config.min_sample_rate(), config.max_sample_rate()] {
                        if !sample_rates.contains(&rate) {
                            sample_rates.push(rate);
                        }
                    }
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
    /// audio thread is the consumer.
    pub fn discard_pending(&self) -> usize {
        if self.is_running() {
            return 0;
        }
        self.idle_commands
            .as_ref()
            .map_or(0, |commands| commands.try_iter().count())
    }
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
    device: cpal::Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    max_block: usize,
    name: String,
}

fn open_output(settings: &AudioSettings) -> Result<DeviceSetup, EngineError> {
    let host = cpal::default_host();
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
            let frames = if max_buffer > 0 {
                // `clamp` panics when its bounds cross, which a backend advertising a degenerate
                // range would otherwise make happen inside what is supposed to be a fallible open.
                let lowest = min_buffer.max(1).min(max_buffer);
                frames.clamp(lowest, max_buffer)
            } else {
                frames.max(1)
            };
            config.buffer_size = BufferSize::Fixed(frames);
            frames as usize
        }
        // The callback may still deliver more than this; the renderer splits what it gets.
        None if max_buffer > 0 => (max_buffer as usize).min(DEFAULT_MAX_BLOCK),
        None => DEFAULT_MAX_BLOCK,
    };

    Ok(DeviceSetup {
        name: device.to_string(),
        device,
        config,
        sample_format,
        max_block: max_block.max(1),
    })
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
    // state the rest of the application can see.
    let on_error = move |error: cpal::Error| {
        running.store(false, Ordering::Relaxed);
        log::error!("audio stream error: {error}; the output stream is dead");
    };
    let mut engine = engine;
    let stream = match format {
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
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| engine.fill(data),
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
    returned_graphs: Sender<Box<RenderGraph>>,
    /// Graphs waiting for room in the return queue. Fixed capacity: pushing never allocates.
    ///
    /// The `Box` is what travels down the return channel, and unwrapping it here would mean
    /// copying a large struct out of the heap and boxing it again to send it — an allocation on
    /// the audio thread, which is exactly what this whole mechanism exists to avoid.
    #[allow(clippy::vec_box)]
    retired: Vec<Box<RenderGraph>>,
    playhead: Arc<AtomicU64>,
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
    // Nine arguments, all of them shared state the callback needs; grouping them into a struct
    // would only move the same list one level down.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        commands: Receiver<EngineCommand>,
        returned_graphs: Sender<Box<RenderGraph>>,
        playhead: Arc<AtomicU64>,
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
            playhead,
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
        // A trailing partial frame the device asked for is silence rather than stale samples.
        for sample in &mut data[frames * self.channels..] {
            *sample = T::from_sample(0.0f32);
        }

        self.playhead
            .store(self.transport.position_frames, Ordering::Relaxed);
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
        while self.retired.len() < RETIRED_GRAPH_SLOTS {
            let Ok(command) = self.commands.try_recv() else {
                break;
            };
            self.apply(command);
        }
    }

    /// Hands stashed graphs back to the UI thread as soon as the queue has room.
    fn flush_retired(&mut self) {
        while let Some(graph) = self.retired.pop() {
            match self.returned_graphs.try_send(graph) {
                Ok(()) => {}
                Err(TrySendError::Full(graph)) => {
                    self.retired.push(graph);
                    break;
                }
                // The UI is gone, so this is shutdown and there is nobody left to free it.
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
    }

    fn retire(&mut self, graph: Box<RenderGraph>) {
        match self.returned_graphs.try_send(graph) {
            Ok(()) => {}
            Err(TrySendError::Full(graph)) => self.retired.push(graph),
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
                    self.retire(previous);
                }
                // A freshly built graph is compensated for its own chains, so this clears any
                // staleness the graph it replaced had reported.
                self.publish_latency();
            }
            EngineCommand::Play => self.transport.playing = true,
            EngineCommand::Stop => {
                self.transport.playing = false;
                if let Some(graph) = &mut self.graph {
                    graph.reset_voices();
                }
            }
            EngineCommand::Seek { frames } => {
                self.transport.seek(frames);
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
            EngineCommand::Modulation { track, amount } => {
                if let Some(graph) = &mut self.graph {
                    graph.modulation(track, amount);
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
    use crate::testkit::{self, TONE_AMPLITUDE};
    use auris_core::ParamId;
    use auris_core::project::{AudioSourceBank, Note, Project};
    use auris_core::time::Ticks;

    const SAMPLE_RATE: f64 = 48_000.0;

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
        Receiver<Box<RenderGraph>>,
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
            Arc::clone(&meters),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            SAMPLE_RATE,
            2,
            256,
        );
        (engine, command_tx, graph_rx, meters, playhead)
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

    /// Builds an engine for a device with `channels` channels.
    #[allow(clippy::type_complexity)]
    fn engine_with_channels(
        channels: usize,
    ) -> (
        AudioEngine,
        Sender<EngineCommand>,
        Receiver<Box<RenderGraph>>,
    ) {
        let (command_tx, command_rx) = crossbeam_channel::bounded(64);
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(64);
        let engine = AudioEngine::new(
            command_rx,
            graph_tx,
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
    fn retired_graphs_survive_a_full_return_queue() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(64);
        // A return queue far too small for the traffic, so the stash is exercised.
        let (graph_tx, graph_rx) = crossbeam_channel::bounded(1);
        let mut engine = AudioEngine::new(
            command_rx,
            graph_tx,
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
        let (device, handle) = start_silent(&AudioSettings::default());
        assert!(!handle.is_running());
        assert!(!device.is_running());
        assert_eq!(device.name(), "silent");
        assert_eq!(handle.sample_rate(), 48_000.0);
        handle.send(EngineCommand::Play).expect("queued");
        handle.set_graph(*graph()).expect("queued");
        assert_eq!(handle.collect_garbage(), 0);
        assert_eq!(handle.playhead_frames(), 0);
        assert_eq!(device.discard_pending(), 2);
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
