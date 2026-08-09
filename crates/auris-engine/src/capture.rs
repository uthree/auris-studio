//! Audio coming the other way.
//!
//! [`device`](crate::device) opens the stream that plays the mix; this one opens the stream that
//! records into it. They are two *separate* cpal streams, which is not an implementation detail
//! anybody can ignore: they run on their own callbacks and, when the microphone and the speakers
//! are different pieces of hardware, on their own crystals. What follows is what that costs and
//! what is done about it.
//!
//! # Where a take lands on the timeline
//!
//! The first block to arrive reads the engine's playhead and writes it down, so a take begins at
//! the frame it was actually started at rather than at whatever the UI thread thought the
//! position was when the button went down. Everything after that is counted from there at the
//! input device's own rate.
//!
//! Two clocks means the count and the playhead drift apart — a few frames a minute between two
//! ordinary devices, which is under a millisecond and inaudible, and rather more between a cheap
//! USB interface and a laptop's own output. Nothing here corrects for it. Correcting would mean
//! resampling the take against a rate nobody has measured, and a take that is a few frames long
//! by the end of an hour is a problem a musician can see and nudge; one that has been silently
//! stretched is not.
//!
//! # How the samples get off the callback
//!
//! A pool of buffers going round two bounded channels — full ones out, empty ones back — which is
//! the same exchange the audio thread already uses to hand retired graphs back to be dropped. The
//! callback takes an empty buffer, copies into it and sends it on; it never allocates, never
//! locks and never waits. If the reader falls behind far enough to empty the pool, the callback
//! throws the block away and counts it, because the one thing it must not do is block: a
//! recording with a gap in it is a bad take, and a recording that stalled the audio device is a
//! stalled machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
};
use crossbeam_channel::{Receiver, Sender};

use crate::device::AudioDeviceInfo;
use crate::error::EngineError;
use crate::handle::EngineHandle;

/// Buffers in the pool.
///
/// Together with [`POOL_SAMPLES`] this is the slack between the callback and whatever is writing
/// the file: 32 buffers of 4096 samples is 128k samples, which at 48 kHz in stereo is 1.3
/// seconds. A disk that stalls for longer than that during a take has bigger news to deliver.
const POOL_BUFFERS: usize = 32;

/// Samples one pooled buffer holds.
///
/// Comfortably more than any callback asks for, so a block is normally one buffer; a larger one
/// is split across several rather than growing a buffer, which would be an allocation on the
/// callback thread.
const POOL_SAMPLES: usize = 4_096;

/// What [`CaptureShared::started_at`] holds until the first block arrives.
const NOT_STARTED: u64 = u64::MAX;

/// Every input device the default host can see.
///
/// The mirror of [`output_devices`](crate::device::output_devices), with the same caveats: it
/// talks to the OS audio server, so call it when a settings panel opens rather than every frame,
/// and a device that errors while being queried is skipped rather than losing the whole list.
pub fn input_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|device| device.to_string());

    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };

    devices
        .map(|device| {
            let name = device.to_string();
            let mut sample_rates = Vec::new();
            let mut max_channels = 0;
            if let Ok(configs) = device.supported_input_configs() {
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

/// What to ask the input device for.
///
/// Separate from [`AudioSettings`](crate::device::AudioSettings) because it is a separate device:
/// a musician recording through an interface and listening on the laptop's own output is the
/// ordinary case, not the exotic one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureSettings {
    /// Name of the input device to open. `None` uses the system default.
    pub device: Option<String>,
    /// Preferred sample rate in Hz. Ignored when the device cannot do it.
    ///
    /// Worth asking for the engine's rate: a take recorded at the rate the project renders at
    /// needs no resampling, and resampling a recording is the one conversion that cannot be
    /// undone later.
    pub sample_rate: Option<u32>,
    /// Preferred callback size in frames. Ignored when the device cannot do it.
    pub block_frames: Option<u32>,
}

/// State the callback writes and the reader reads.
#[derive(Debug)]
struct CaptureShared {
    running: AtomicBool,
    /// Samples the callback threw away because the pool was empty.
    dropped: AtomicU64,
    /// Engine playhead when the first block arrived; [`NOT_STARTED`] until then.
    started_at: AtomicU64,
    /// Loudest sample since the meter was last read, as an `f32` bit pattern.
    peak: AtomicU32,
    /// Frames handed to the pool, which is the length of the take so far.
    frames: AtomicU64,
}

/// A running input stream.
///
/// Dropping it closes the device — there is no pause, on purpose. A stream that is open is a
/// microphone that is live, and holding one open between takes is both a battery cost and, on
/// every operating system that shows an indicator, a light saying the application is listening
/// when it is not.
pub struct Capture {
    /// `None` in the silent capture a test uses.
    stream: Option<cpal::Stream>,
    full: Receiver<Vec<f32>>,
    empty: Sender<Vec<f32>>,
    shared: Arc<CaptureShared>,
    name: String,
    sample_rate: f64,
    channel_count: usize,
}

impl Capture {
    /// Name of the device being recorded from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rate the device is running at, which is not necessarily the rate that was asked for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Channels the device delivers. One is a mono microphone; two is an interface.
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    /// `true` while the stream is alive. Goes false if the device disappears mid-take.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    /// The playhead frame the first captured frame lines up with, once one has arrived.
    ///
    /// `None` means nothing has been recorded yet — the stream opened but no callback has run,
    /// which is the first few milliseconds of every take.
    pub fn started_at(&self) -> Option<u64> {
        match self.shared.started_at.load(Ordering::Relaxed) {
            NOT_STARTED => None,
            frame => Some(frame),
        }
    }

    /// How many frames have been handed over, which is how long the take is so far.
    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// Frames lost because nothing drained the pool in time.
    ///
    /// Anything but zero is a hole in the recording, and the take should be reported as damaged
    /// rather than quietly kept: the samples are gone and everything after them has moved earlier
    /// by that much.
    pub fn dropped_frames(&self) -> u64 {
        let channels = self.channel_count.max(1) as u64;
        self.shared.dropped.load(Ordering::Relaxed) / channels
    }

    /// The loudest sample since this was last called, and resets the meter.
    ///
    /// Reset on read, like a peak-hold on a console: a meter that only ever climbed would read
    /// the loudest moment of the whole session for the rest of it.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::Relaxed))
    }

    /// Hands every block that has arrived to `consume`, and returns the pool's buffers.
    ///
    /// Returns how many *samples* were passed on — frames times channels — so a caller that is
    /// writing them down can tell an empty poll from a busy one. Never blocks: what is not ready
    /// yet is simply left for the next call.
    pub fn drain(&self, mut consume: impl FnMut(&[f32])) -> usize {
        let mut samples = 0;
        while let Ok(buffer) = self.full.try_recv() {
            consume(&buffer);
            samples += buffer.len();
            // Straight back to the pool. The channel cannot be full — every buffer in flight came
            // out of it — and if it somehow were, dropping this one costs a later block rather
            // than blocking the reader here.
            let _ = self.empty.try_send(buffer);
        }
        samples
    }
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("name", &self.name)
            .field("running", &self.is_running())
            .field("sample_rate", &self.sample_rate)
            .field("channel_count", &self.channel_count)
            .field("frames", &self.frames())
            .field("dropped_frames", &self.dropped_frames())
            .finish()
    }
}

/// The callback's half: takes blocks from the device and passes them on.
struct CaptureSink {
    full: Sender<Vec<f32>>,
    empty: Receiver<Vec<f32>>,
    shared: Arc<CaptureShared>,
    playhead: Arc<AtomicU64>,
    channels: usize,
}

impl CaptureSink {
    /// Accepts one callback's worth of interleaved samples, in whatever format the device speaks.
    fn push<T>(&mut self, mut data: &[T])
    where
        T: Copy,
        f32: FromSample<T>,
    {
        if data.is_empty() {
            return;
        }
        // The first block is what fixes the take to the timeline. `compare_exchange` rather than
        // a store, so every later block leaves it alone.
        let _ = self.shared.started_at.compare_exchange(
            NOT_STARTED,
            self.playhead.load(Ordering::Relaxed),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );

        while !data.is_empty() {
            let Ok(mut buffer) = self.empty.try_recv() else {
                // The pool is empty: the reader has not run in over a second. Dropping is the
                // only option that does not stall the device, and it is counted so that the take
                // can be reported as damaged rather than passing for a good one.
                self.shared
                    .dropped
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                return;
            };
            let take = data.len().min(buffer.capacity());
            // `clear` then `extend` over an exact-size iterator: the capacity is already there,
            // so this is a conversion and a copy with no allocation in it.
            buffer.clear();
            buffer.extend(data[..take].iter().map(|sample| f32::from_sample(*sample)));
            self.note_peak(&buffer);
            let frames = (take / self.channels.max(1)) as u64;
            match self.full.try_send(buffer) {
                Ok(()) => self.shared.frames.fetch_add(frames, Ordering::Relaxed),
                Err(_) => self
                    .shared
                    .dropped
                    .fetch_add(take as u64, Ordering::Relaxed),
            };
            data = &data[take..];
        }
    }

    /// Raises the meter to the loudest sample in `block`, if it is louder than what is there.
    fn note_peak(&self, block: &[f32]) {
        let mut peak = f32::from_bits(self.shared.peak.load(Ordering::Relaxed));
        for sample in block {
            let level = sample.abs();
            // `>` rather than `max`, so a NaN from a misbehaving driver loses instead of
            // poisoning the meter for the rest of the take.
            if level > peak {
                peak = level;
            }
        }
        self.shared.peak.store(peak.to_bits(), Ordering::Relaxed);
    }
}

/// The two halves of a fresh pool, and the state they share.
fn pool() -> (Capture, CaptureSink, Arc<CaptureShared>) {
    let (full_tx, full_rx) = crossbeam_channel::bounded(POOL_BUFFERS);
    let (empty_tx, empty_rx) = crossbeam_channel::bounded(POOL_BUFFERS);
    for _ in 0..POOL_BUFFERS {
        let _ = empty_tx.try_send(Vec::with_capacity(POOL_SAMPLES));
    }
    let shared = Arc::new(CaptureShared {
        running: AtomicBool::new(false),
        dropped: AtomicU64::new(0),
        started_at: AtomicU64::new(NOT_STARTED),
        peak: AtomicU32::new(0),
        frames: AtomicU64::new(0),
    });
    let capture = Capture {
        stream: None,
        full: full_rx,
        empty: empty_tx,
        shared: Arc::clone(&shared),
        name: String::new(),
        sample_rate: 0.0,
        channel_count: 0,
    };
    let sink = CaptureSink {
        full: full_tx,
        empty: empty_rx,
        shared: Arc::clone(&shared),
        playhead: Arc::new(AtomicU64::new(0)),
        channels: 1,
    };
    (capture, sink, shared)
}

/// Opens an input device and starts recording into the pool.
///
/// `engine` is only read from: the capture takes its playhead so that the first block can stamp
/// where the take begins. A capture outlives nothing — dropping it closes the device.
pub fn start_capture(
    settings: &CaptureSettings,
    engine: &EngineHandle,
) -> Result<Capture, EngineError> {
    let setup = open_input(settings)?;
    let channels = setup.config.channels as usize;
    let sample_rate = f64::from(setup.config.sample_rate);

    let (mut capture, mut sink, shared) = pool();
    sink.playhead = engine.playhead_cell();
    sink.channels = channels;
    shared.running.store(true, Ordering::Relaxed);

    let on_error = {
        let shared = Arc::clone(&shared);
        move |error: cpal::Error| {
            shared.running.store(false, Ordering::Relaxed);
            log::error!("audio input error: {error}; the recording stream is dead");
        }
    };

    let stream = match setup.sample_format {
        SampleFormat::F32 => input_stream::<f32>(&setup.device, setup.config, sink, on_error),
        SampleFormat::I16 => input_stream::<i16>(&setup.device, setup.config, sink, on_error),
        SampleFormat::U16 => input_stream::<u16>(&setup.device, setup.config, sink, on_error),
        other => return Err(EngineError::UnsupportedSampleFormat(other.to_string())),
    }?;
    stream.play()?;

    capture.stream = Some(stream);
    capture.name = setup.name;
    capture.sample_rate = sample_rate;
    capture.channel_count = channels;
    Ok(capture)
}

fn input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut sink: CaptureSink,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, EngineError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| sink.push(data),
            on_error,
            None,
        )
        .map_err(EngineError::from)
}

/// Everything resolved about the chosen input before a stream exists.
struct InputSetup {
    device: cpal::Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    name: String,
}

fn open_input(settings: &CaptureSettings) -> Result<InputSetup, EngineError> {
    let host = cpal::default_host();
    // A named device that has since been unplugged falls back to the default, exactly as the
    // output does: losing the interface should not also lose the take about to be played.
    let device = settings
        .device
        .as_deref()
        .and_then(|wanted| {
            let found = host
                .input_devices()
                .ok()?
                .find(|device| device.to_string() == wanted);
            if found.is_none() {
                log::warn!("input device `{wanted}` is not available; using the default");
            }
            found
        })
        .or_else(|| host.default_input_device())
        .ok_or(EngineError::NoInputDevice)?;
    let supported = device.default_input_config()?;
    let sample_format = supported.sample_format();
    let mut config = supported.config();

    if let Some(rate) = settings.sample_rate
        && rate != config.sample_rate
        && supports_input_rate(&device, config.channels, sample_format, rate)
    {
        config.sample_rate = rate;
    }

    if let Some(frames) = settings.block_frames {
        let (min_buffer, max_buffer) = match supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => (*min, *max),
            SupportedBufferSize::Unknown => (0, 0),
        };
        let frames = if max_buffer > 0 {
            // `clamp` panics when its bounds cross, which a backend advertising a degenerate
            // range would otherwise make happen inside what is supposed to be a fallible open.
            let lowest = min_buffer.max(1).min(max_buffer);
            frames.clamp(lowest, max_buffer)
        } else {
            frames.max(1)
        };
        config.buffer_size = BufferSize::Fixed(frames);
    }

    Ok(InputSetup {
        name: device.to_string(),
        device,
        config,
        sample_format,
    })
}

fn supports_input_rate(
    device: &cpal::Device,
    channels: cpal::ChannelCount,
    format: SampleFormat,
    rate: cpal::SampleRate,
) -> bool {
    match device.supported_input_configs() {
        Ok(configs) => configs.into_iter().any(|range| {
            range.channels() == channels
                && range.sample_format() == format
                && range.min_sample_rate() <= rate
                && rate <= range.max_sample_rate()
        }),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture and its callback half, wired together with no device behind them.
    ///
    /// Everything below the stream is what these tests are for: no machine running CI has a
    /// microphone, and the parts that would go wrong — the pool running dry, the stamp, the
    /// format conversion — are all on this side of it anyway.
    fn wired(channels: usize) -> (Capture, CaptureSink, Arc<AtomicU64>) {
        let (mut capture, mut sink, _) = pool();
        let playhead = Arc::new(AtomicU64::new(0));
        sink.playhead = Arc::clone(&playhead);
        sink.channels = channels;
        capture.channel_count = channels;
        capture.sample_rate = 48_000.0;
        (capture, sink, playhead)
    }

    /// Everything `drain` hands over, in one buffer.
    fn drained(capture: &Capture) -> Vec<f32> {
        let mut out = Vec::new();
        capture.drain(|block| out.extend_from_slice(block));
        out
    }

    #[test]
    fn samples_come_out_the_other_side_in_the_order_they_went_in() {
        let (capture, mut sink, _) = wired(2);
        sink.push(&[0.1f32, 0.2, 0.3, 0.4]);
        sink.push(&[0.5f32, 0.6]);
        assert_eq!(drained(&capture), vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(capture.frames(), 3, "six samples of stereo is three frames");
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn a_take_begins_where_the_playhead_was_when_the_first_block_arrived() {
        // Not where it was when the button went down: that is a UI-thread reading, one callback
        // out of date, and a take that starts eleven milliseconds early is a take that has to be
        // nudged by hand on every single recording.
        let (capture, mut sink, playhead) = wired(1);
        assert_eq!(capture.started_at(), None, "nothing has arrived yet");

        playhead.store(96_000, Ordering::Relaxed);
        sink.push(&[0.0f32; 8]);
        assert_eq!(capture.started_at(), Some(96_000));

        // And the playhead moving on does not move the take that is already running.
        playhead.store(200_000, Ordering::Relaxed);
        sink.push(&[0.0f32; 8]);
        assert_eq!(capture.started_at(), Some(96_000));
    }

    #[test]
    fn a_block_larger_than_a_pooled_buffer_is_split_rather_than_grown() {
        // Growing it would be an allocation on the audio callback. Splitting is the whole reason
        // `push` has a loop in it.
        let (capture, mut sink, _) = wired(1);
        let block = vec![1.0f32; POOL_SAMPLES + POOL_SAMPLES / 2];
        sink.push(&block);
        assert_eq!(drained(&capture).len(), block.len());
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn a_reader_that_never_runs_costs_samples_rather_than_the_device() {
        // The contract this whole module is built around: the callback must come back. Filling
        // the pool without draining it has to end in counted losses, not in a block.
        let (capture, mut sink, _) = wired(1);
        for _ in 0..POOL_BUFFERS + 4 {
            sink.push(&[0.5f32; POOL_SAMPLES]);
        }
        assert_eq!(
            capture.dropped_frames(),
            4 * POOL_SAMPLES as u64,
            "everything past the pool should have been counted as lost"
        );
        // And what did fit is still intact, so the take up to the gap is usable.
        assert_eq!(drained(&capture).len(), POOL_BUFFERS * POOL_SAMPLES);
    }

    #[test]
    fn the_pool_goes_round_rather_than_running_out() {
        // Draining returns buffers, so a take longer than the pool is the ordinary case and not
        // the failure above.
        let (capture, mut sink, _) = wired(1);
        for _ in 0..POOL_BUFFERS * 4 {
            sink.push(&[0.25f32; POOL_SAMPLES]);
            assert_eq!(drained(&capture).len(), POOL_SAMPLES);
        }
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn an_integer_device_arrives_as_the_floats_everything_else_speaks() {
        let (capture, mut sink, _) = wired(1);
        sink.push(&[i16::MAX, 0, i16::MIN]);
        let out = drained(&capture);
        assert!(
            (out[0] - 1.0).abs() < 1.0e-4,
            "full scale came out as {}",
            out[0]
        );
        assert_eq!(out[1], 0.0);
        assert!(
            (out[2] + 1.0).abs() < 1.0e-4,
            "full scale came out as {}",
            out[2]
        );
    }

    #[test]
    fn the_meter_holds_the_loudest_sample_and_clears_when_it_is_read() {
        let (capture, mut sink, _) = wired(1);
        sink.push(&[0.2f32, -0.7, 0.3]);
        assert_eq!(capture.take_peak(), 0.7, "the peak is the absolute value");
        assert_eq!(capture.take_peak(), 0.0, "and reading it starts a new one");

        // A driver handing over a NaN must not leave the meter stuck at nothing readable.
        sink.push(&[f32::NAN, 0.4]);
        assert_eq!(capture.take_peak(), 0.4);
    }

    #[test]
    fn an_empty_callback_neither_stamps_the_take_nor_counts_a_frame() {
        // A device is allowed to call back with nothing, and doing so before the transport has
        // moved would otherwise pin the take to whatever the playhead read at the time.
        let (capture, mut sink, _) = wired(2);
        sink.push::<f32>(&[]);
        assert_eq!(capture.started_at(), None);
        assert_eq!(capture.frames(), 0);
    }
}
