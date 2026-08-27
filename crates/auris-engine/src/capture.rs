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
//! A take that was counted in begins during the count, so its file opens with however much of the
//! count the input stream caught. The playhead is not moving then and cannot say how much that
//! was — so the first block stamps the *pair*: where the playhead is, and what was left of the
//! count at that instant. [`Capture::count_in_at_start`] is the second half, and trimming it is
//! what puts a counted-in clip on the downbeat rather than a bar in front of it.
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
//!
//! # Why the reader is a separate object
//!
//! A `cpal::Stream` is only `Send` on some of the hosts cpal supports — WASAPI yes, CoreAudio no
//! — so a [`Capture`] cannot be assumed to move to another thread, and the thread that writes the
//! file has to be another thread: a UI that stalls for a second while a dialog opens would
//! otherwise cost the take a second of audio.
//!
//! So the two halves are split. [`Capture`] owns the stream and stays where it was opened, and
//! [`CaptureReader`] owns the receiving end of the pool and goes wherever the writing happens.
//! Dropping the [`Capture`] closes the device, which drops the sender, which is how the reader
//! finds out the take is over — see [`CaptureReader::is_finished`].

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
use crate::monitor::MonitorRing;

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

/// How many tracks can be monitored at once.
///
/// One ring each, all of them made when the pool is, because the input callback may not allocate
/// and the set of listeners changes while it is running — a slot switched on is an atomic flag,
/// where a ring made on demand would be an allocation under a realtime thread. Eight is a band.
/// Each costs a ring, so the whole set is a megabyte held for as long as a device is open.
pub const MONITOR_SLOTS: usize = 8;

/// How many input channels get a meter of their own.
///
/// A cap on a fixed-size array rather than a limit on the interface: a device with more channels
/// than this still records through every one of them, and the ones past it simply have no meter.
/// Thirty-two is two more than any interface anybody is likely to have on a desk, and the array
/// costs four bytes a channel.
pub const MAX_METERED_CHANNELS: usize = 32;

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
    /// Whether a take is running, which is what decides if the pool is fed at all.
    ///
    /// An open device is not a take. Monitoring holds one open with this `false`, and then the
    /// callback feeds the ring and nothing else — no file, no counters moving, no meter that says
    /// a recording is under way when none is.
    recording: AtomicBool,
    /// Samples the callback threw away because the pool was empty.
    dropped: AtomicU64,
    /// Engine playhead when the first block arrived; [`NOT_STARTED`] until then.
    started_at: AtomicU64,
    /// Frames of count-in still to come when that same first block arrived.
    ///
    /// Stamped in the same breath as `started_at` and meaningless apart from it. The playhead
    /// does not move during a count-in, so a take that began in the middle of one is a file whose
    /// first samples belong in front of the position it was stamped with — this says how many.
    count_in_at_start: AtomicU64,
    /// Loudest sample since the meter was last read, as an `f32` bit pattern.
    peak: AtomicU32,
    /// The same, one per input channel, for a meter beside each armed track.
    ///
    /// Beside the device-wide figure rather than instead of it, and the two are read by different
    /// people: the transport bar asks what is arriving at the interface, and a track asks what is
    /// arriving on the channels it is armed to. Both reset on read, so they cannot be one cell —
    /// whichever asked first would leave the other reading silence.
    channel_peaks: [AtomicU32; MAX_METERED_CHANNELS],
    /// Frames handed to the pool, which is the length of the take so far.
    frames: AtomicU64,
}

/// A running input stream.
///
/// Dropping it closes the device — there is no pause, on purpose. A stream that is open is a
/// microphone that is live, and holding one open when nothing wants it is both a battery cost
/// and, on every operating system that shows an indicator, a light saying the application is
/// listening when it is not. So the *device* lasts as long as somebody is recording or
/// monitoring, and a take is a phase within that: see [`Capture::begin_take`].
pub struct Capture {
    /// `None` in the silent capture a test uses.
    stream: Option<cpal::Stream>,
    /// Taken by whatever is going to write the samples down, and handed back when it has.
    ///
    /// Out on loan for the length of a take and nowhere else. There is one pool and it may have
    /// exactly one consumer, so a second take can only begin once the first has given this back —
    /// see [`Capture::restore_reader`].
    reader: Option<CaptureReader>,
    shared: Arc<CaptureShared>,
    /// The way back to the speakers, handed to the render graph. Unlike the reader this is shared
    /// rather than taken: a graph is rebuilt on every structural edit and needs it again each time.
    monitors: Vec<Arc<MonitorRing>>,
    name: String,
    sample_rate: f64,
    channel_count: usize,
}

/// The receiving end of a capture's pool.
///
/// Sent to the thread that writes the file. It carries no handle to the device, which is the
/// point: see the module note on why the stream cannot go with it.
pub struct CaptureReader {
    full: Receiver<Vec<f32>>,
    empty: Sender<Vec<f32>>,
    shared: Arc<CaptureShared>,
    sample_rate: f64,
    channel_count: usize,
    finished: bool,
}

impl CaptureReader {
    /// Rate the device is running at.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Channels the device delivers, which is how wide the file has to be.
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    /// Hands every block that has arrived to `consume`, and returns the pool's buffers.
    ///
    /// Returns how many *samples* were passed on — frames times channels — so a caller can tell
    /// an empty poll from a busy one and decide whether to wait. Never blocks: what has not
    /// arrived yet is left for the next call.
    pub fn drain(&mut self, mut consume: impl FnMut(&[f32])) -> usize {
        let mut samples = 0;
        loop {
            match self.full.try_recv() {
                Ok(buffer) => {
                    consume(&buffer);
                    samples += buffer.len();
                    // Straight back to the pool. The channel cannot be full — every buffer in
                    // flight came out of it — and if it somehow were, dropping this one costs a
                    // later block rather than blocking the reader here.
                    let _ = self.empty.try_send(buffer);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                // crossbeam reports this only once the queue is drained *and* every sender has
                // gone, so it means exactly "the stream was dropped and you have it all".
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        samples
    }

    /// `true` once the device has closed and every sample it sent has been handed over.
    ///
    /// Only ever true after a [`Self::drain`] that reached the end, because that is when it is
    /// found out. This is the device *dying* — unplugged, or the whole capture dropped. A take
    /// that was merely stopped ends through [`Self::is_recording`] instead, with the device still
    /// open because somebody is still monitoring through it.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// `false` once the take has been stopped, whether or not the device is still open.
    ///
    /// Not a signal to stop reading at once: the callback may have been part way through a block
    /// when this went false, and that block belongs in the file. A writer waits for a quiet pass
    /// or two before it closes.
    pub fn is_recording(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }

    /// Frames lost because nothing drained the pool in time. See [`Capture::dropped_frames`].
    pub fn dropped_frames(&self) -> u64 {
        dropped_frames(&self.shared, self.channel_count)
    }
}

/// Samples the callback threw away, as frames.
fn dropped_frames(shared: &CaptureShared, channels: usize) -> u64 {
    shared.dropped.load(Ordering::Relaxed) / channels.max(1) as u64
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

    /// How much count-in the take's first frames hold, in frames of the input device.
    ///
    /// Zero unless Record was pressed with a count-in in front of it, and zero again once the
    /// count has been played: what a take opened during a count contains, before anything the
    /// player did, is the rest of that count. Trimming it is what puts the clip on the downbeat.
    pub fn count_in_at_start(&self) -> u64 {
        self.shared.count_in_at_start.load(Ordering::Relaxed)
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
        dropped_frames(&self.shared, self.channel_count)
    }

    /// The loudest sample since this was last called, and resets the meter.
    ///
    /// Reset on read, like a peak-hold on a console: a meter that only ever climbed would read
    /// the loudest moment of the whole session for the rest of it.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::Relaxed))
    }

    /// The loudest sample on each input channel since this was last called, and resets them.
    ///
    /// `out` is resized to the device's channel count, so it is the reader's own buffer and this
    /// allocates only when the device changes. Channels past [`MAX_METERED_CHANNELS`] read zero.
    ///
    /// Reset on read for the same reason [`Self::take_peak`] is, which is why there is exactly one
    /// caller: a second would find whatever the first left, which is nothing.
    pub fn take_channel_peaks(&self, out: &mut Vec<f32>) {
        out.clear();
        for channel in 0..self.channel_count {
            let level = match self.shared.channel_peaks.get(channel) {
                Some(cell) => f32::from_bits(cell.swap(0, Ordering::Relaxed)),
                None => 0.0,
            };
            out.push(level);
        }
    }

    /// Opens a take: from here the callback feeds the pool, and the counters describe this take.
    ///
    /// Everything a take is measured by is cleared, because they all mean "so far in this take"
    /// and a second take through the same device would otherwise begin at the first one's length.
    pub fn begin_take(&self) {
        self.shared.frames.store(0, Ordering::Relaxed);
        self.shared.dropped.store(0, Ordering::Relaxed);
        self.shared.peak.store(0, Ordering::Relaxed);
        for cell in &self.shared.channel_peaks {
            cell.store(0, Ordering::Relaxed);
        }
        self.shared.started_at.store(NOT_STARTED, Ordering::Relaxed);
        self.shared.count_in_at_start.store(0, Ordering::Relaxed);
        self.shared.recording.store(true, Ordering::Release);
    }

    /// Closes the take, leaving the device open for whatever else wants it.
    pub fn end_take(&self) {
        self.shared.recording.store(false, Ordering::Release);
    }

    /// `true` between [`Self::begin_take`] and [`Self::end_take`].
    pub fn is_taking(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }

    /// The reading half, for the thread that is going to write the samples down.
    ///
    /// `None` while a take already has it: there is one pool and one consumer of it, and two
    /// readers would split a take between them. The writer gives it back through
    /// [`Self::restore_reader`] when it is done, which is what lets a second take use a device the
    /// first one left open.
    pub fn take_reader(&mut self) -> Option<CaptureReader> {
        self.reader.take()
    }

    /// Puts the reader back, so the next take can have it.
    pub fn restore_reader(&mut self, reader: CaptureReader) {
        self.reader = Some(reader);
    }

    /// Whether the reader is here rather than out on loan.
    ///
    /// `false` with no take running means the thread that borrowed it died holding it, which
    /// nothing can recover from except opening the device again.
    pub fn has_reader(&self) -> bool {
        self.reader.is_some()
    }

    /// One of the rings the render graph reads to play this input back through the mix.
    ///
    /// Handed out as often as it is asked for, unlike [`Self::take_reader`]: a ring is written by
    /// the callback and read by whichever graph is current, and a graph is replaced on every
    /// structural edit. Every slot is off until somebody calls
    /// [`set_enabled`](crate::monitor::MonitorRing::set_enabled) — an open device is not a device
    /// anybody is listening to — and which channels a slot carries is
    /// [`set_source`](crate::monitor::MonitorRing::set_source).
    ///
    /// `None` past [`MONITOR_SLOTS`], which is a caller asking to monitor more tracks at once
    /// than a room holds players.
    pub fn monitor(&self, slot: usize) -> Option<Arc<MonitorRing>> {
        self.monitors.get(slot).map(Arc::clone)
    }

    /// The worst any listening slot has had it: gaps heard, counted.
    ///
    /// The worst rather than the sum, because one is what a person is being told about — that the
    /// monitor is breaking up — and four monitors each breaking up once is that same fact said
    /// four times.
    pub fn monitor_rebuffers(&self) -> u64 {
        self.monitors
            .iter()
            .filter(|ring| ring.is_enabled())
            .map(|ring| ring.rebuffers())
            .max()
            .unwrap_or(0)
    }

    /// Stops every slot listening, for a device that is being closed or re-pointed.
    pub fn silence_monitors(&self) {
        for ring in &self.monitors {
            ring.set_enabled(false);
        }
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
    /// The count-in cell, read at the same moment as the playhead and for the same reason.
    count_in: Arc<AtomicU64>,
    channels: usize,
    /// The other readers of these samples: the render graph, for every track being monitored.
    ///
    /// Rings rather than second consumers of the pool, because the pool has exactly one by
    /// construction. See [`crate::monitor`], and note that they are written *before* the pool: a
    /// take that is losing blocks to a stalled disk should still be audible to the person playing
    /// it. A slot nobody is listening through costs the branch inside
    /// [`MonitorRing::write`](crate::monitor::MonitorRing::write) and nothing else.
    monitors: Vec<Arc<MonitorRing>>,
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
        for ring in &self.monitors {
            ring.write(data, self.channels);
        }
        // Before the take check, because an input meter is about the *device*: somebody setting a
        // level has not started a take yet, and that is exactly when they need to see one.
        self.note_peak(data);
        // An open device is not a take. Somebody monitoring holds the stream open with no take
        // running, and everything below here — the stamp, the counters, the file — belongs to a
        // take and to nothing else.
        if !self.shared.recording.load(Ordering::Acquire) {
            return;
        }
        // The first block is what fixes the take to the timeline. `compare_exchange` rather than
        // a store, so every later block leaves it alone.
        //
        // The count-in goes down with it, and *before* it, so that a block which loses the race
        // to stamp the position has not already overwritten the count belonging to the one that
        // won. The two are one reading of one moment: a position on the timeline, and how much
        // of the count was still to be played when the first sample of the take was taken.
        if self.shared.started_at.load(Ordering::Relaxed) == NOT_STARTED {
            self.shared
                .count_in_at_start
                .store(self.count_in.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        let _ = self.shared.started_at.compare_exchange(
            NOT_STARTED,
            self.playhead.load(Ordering::Relaxed),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );

        let channels = self.channels.max(1);
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
            // Whole frames, never a fraction of one. The block a reader is handed is split by
            // channel — one file per armed track — and a buffer that ended half way through a
            // frame would put the next block's first channel where the last one's second was.
            // Rounding down costs at most `channels - 1` samples out of four thousand.
            let room = (buffer.capacity() - buffer.capacity() % channels).max(1);
            let take = data.len().min(room);
            // `clear` then `extend` over an exact-size iterator: the capacity is already there,
            // so this is a conversion and a copy with no allocation in it.
            buffer.clear();
            buffer.extend(data[..take].iter().map(|sample| f32::from_sample(*sample)));
            let frames = (take / channels) as u64;
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
    /// Both meters at once: what reached the interface, and what reached each of its channels.
    ///
    /// The per-channel figures are gathered on the stack and written out once at the end, so a
    /// block costs one atomic store per channel rather than one per sample. Nothing here
    /// allocates: the array is the fixed [`MAX_METERED_CHANNELS`] and a device with more channels
    /// than that leaves the rest of them unmetered rather than growing it.
    fn note_peak<T>(&self, block: &[T])
    where
        T: Copy,
        f32: FromSample<T>,
    {
        let channels = self.channels.max(1);
        let mut peak = f32::from_bits(self.shared.peak.load(Ordering::Relaxed));
        let mut channel_peaks = [0.0f32; MAX_METERED_CHANNELS];
        for frame in block.chunks(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                let level = f32::from_sample(*sample).abs();
                // `>` rather than `max`, so a NaN from a misbehaving driver loses instead of
                // poisoning the meter for the rest of the take.
                if level > peak {
                    peak = level;
                }
                if channel < MAX_METERED_CHANNELS && level > channel_peaks[channel] {
                    channel_peaks[channel] = level;
                }
            }
        }
        self.shared.peak.store(peak.to_bits(), Ordering::Relaxed);
        for (cell, level) in self
            .shared
            .channel_peaks
            .iter()
            .zip(channel_peaks)
            .take(channels)
        {
            if level > f32::from_bits(cell.load(Ordering::Relaxed)) {
                cell.store(level.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

/// The two halves of a fresh pool, and the state they share.
fn pool(input_rate: f64) -> (Capture, CaptureSink, Arc<CaptureShared>) {
    let (full_tx, full_rx) = crossbeam_channel::bounded(POOL_BUFFERS);
    let (empty_tx, empty_rx) = crossbeam_channel::bounded(POOL_BUFFERS);
    for _ in 0..POOL_BUFFERS {
        let _ = empty_tx.try_send(Vec::with_capacity(POOL_SAMPLES));
    }
    let shared = Arc::new(CaptureShared {
        running: AtomicBool::new(false),
        recording: AtomicBool::new(false),
        dropped: AtomicU64::new(0),
        started_at: AtomicU64::new(NOT_STARTED),
        count_in_at_start: AtomicU64::new(0),
        peak: AtomicU32::new(0),
        channel_peaks: [const { AtomicU32::new(0) }; MAX_METERED_CHANNELS],
        frames: AtomicU64::new(0),
    });
    // Every slot up front: the callback writes whichever are enabled and may not make one.
    let monitors: Vec<Arc<MonitorRing>> = (0..MONITOR_SLOTS)
        .map(|_| Arc::new(MonitorRing::new(input_rate)))
        .collect();
    let capture = Capture {
        stream: None,
        reader: Some(CaptureReader {
            full: full_rx,
            empty: empty_tx,
            shared: Arc::clone(&shared),
            sample_rate: 0.0,
            channel_count: 0,
            finished: false,
        }),
        shared: Arc::clone(&shared),
        monitors: monitors.clone(),
        name: String::new(),
        sample_rate: 0.0,
        channel_count: 0,
    };
    let sink = CaptureSink {
        full: full_tx,
        empty: empty_rx,
        shared: Arc::clone(&shared),
        playhead: Arc::new(AtomicU64::new(0)),
        count_in: Arc::new(AtomicU64::new(0)),
        channels: 1,
        monitors,
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

    let (mut capture, mut sink, shared) = pool(sample_rate);
    sink.playhead = engine.playhead_cell();
    sink.count_in = engine.count_in_cell();
    sink.channels = channels;
    shared.running.store(true, Ordering::Relaxed);

    let on_error = {
        let shared = Arc::clone(&shared);
        // Same discrimination as the output stream: a rerouted default device keeps recording,
        // and declaring it dead would paint "device lost" over a take that is landing fine.
        move |error: cpal::Error| {
            if crate::device::stream_survives(error.kind()) {
                log::warn!("audio input notice: {error}; the recording stream keeps running");
                return;
            }
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
    if let Some(reader) = capture.reader.as_mut() {
        reader.sample_rate = sample_rate;
        reader.channel_count = channels;
    }
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
    fn wired(channels: usize) -> (Capture, CaptureReader, CaptureSink, Arc<AtomicU64>) {
        let (mut capture, mut sink, _) = pool(48_000.0);
        let playhead = Arc::new(AtomicU64::new(0));
        sink.playhead = Arc::clone(&playhead);
        sink.channels = channels;
        capture.channel_count = channels;
        capture.sample_rate = 48_000.0;
        let mut reader = capture.take_reader().expect("the first reader");
        reader.channel_count = channels;
        reader.sample_rate = 48_000.0;
        // Every test below is about a take. The device being open is a separate thing, and the
        // test for *that* is `an_open_device_with_no_take_running_records_nothing`.
        capture.begin_take();
        (capture, reader, sink, playhead)
    }

    /// Everything `drain` hands over, in one buffer.
    fn drained(reader: &mut CaptureReader) -> Vec<f32> {
        let mut out = Vec::new();
        reader.drain(|block| out.extend_from_slice(block));
        out
    }

    #[test]
    fn samples_come_out_the_other_side_in_the_order_they_went_in() {
        let (capture, mut reader, mut sink, _) = wired(2);
        sink.push(&[0.1f32, 0.2, 0.3, 0.4]);
        sink.push(&[0.5f32, 0.6]);
        assert_eq!(drained(&mut reader), vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(capture.frames(), 3, "six samples of stereo is three frames");
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn every_input_channel_is_metered_on_its_own() {
        // What a meter beside an armed track reads. The device-wide figure cannot answer it: a
        // room where one microphone is loud and another is silent reads as loud on both.
        let (capture, _reader, mut sink, _playhead) = wired(2);
        // Interleaved: the left channel at half, the right at an eighth.
        sink.push(&[0.5f32, 0.125, -0.5, 0.125]);

        let mut peaks = Vec::new();
        capture.take_channel_peaks(&mut peaks);
        assert_eq!(peaks.len(), 2);
        assert!((peaks[0] - 0.5).abs() < 1e-6, "{peaks:?}");
        assert!((peaks[1] - 0.125).abs() < 1e-6, "{peaks:?}");
        // The device-wide meter is the loudest of them and is its own reading — taking the
        // channels must not have emptied it.
        assert!((capture.take_peak() - 0.5).abs() < 1e-6);

        // Reset on read, like every other peak-hold here.
        capture.take_channel_peaks(&mut peaks);
        assert_eq!(peaks, vec![0.0, 0.0]);
    }

    #[test]
    fn a_take_counted_in_remembers_how_much_of_the_count_it_caught() {
        // The playhead does not move during a count-in, so where the take begins says nothing
        // about how much of the count is sitting at the front of its file. The pair does: the
        // position, and the count that was still to be played when the first sample arrived.
        let (capture, _reader, mut sink, playhead) = wired(1);
        let count_in = Arc::new(AtomicU64::new(24_000));
        sink.count_in = Arc::clone(&count_in);
        playhead.store(96_000, Ordering::Relaxed);

        sink.push(&[0.5f32; 64]);
        assert_eq!(capture.started_at(), Some(96_000));
        assert_eq!(capture.count_in_at_start(), 24_000);

        // The count runs down and the take goes on: the stamp is of one moment, not of the last
        // block to arrive.
        count_in.store(0, Ordering::Relaxed);
        sink.push(&[0.5f32; 64]);
        assert_eq!(capture.count_in_at_start(), 24_000);

        // And the next take, started after the count, carries none of it.
        capture.end_take();
        capture.begin_take();
        sink.push(&[0.5f32; 64]);
        assert_eq!(capture.count_in_at_start(), 0);
    }

    #[test]
    fn a_take_begins_where_the_playhead_was_when_the_first_block_arrived() {
        // Not where it was when the button went down: that is a UI-thread reading, one callback
        // out of date, and a take that starts eleven milliseconds early is a take that has to be
        // nudged by hand on every single recording.
        let (capture, _reader, mut sink, playhead) = wired(1);
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
        let (capture, mut reader, mut sink, _) = wired(1);
        let block = vec![1.0f32; POOL_SAMPLES + POOL_SAMPLES / 2];
        sink.push(&block);
        assert_eq!(drained(&mut reader).len(), block.len());
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn a_split_block_is_cut_between_frames_and_never_through_one() {
        // Three channels, because a pooled buffer holds 4096 samples and three does not divide
        // it. Whoever drains this splits each block by channel to write one file per armed
        // track, and a block that began half way through a frame would file every channel one
        // place along for the rest of the take.
        let channels = 3;
        let (_capture, mut reader, mut sink, _) = wired(channels);
        // Whole frames going in, because that is what a device delivers.
        let frames = POOL_SAMPLES * 2 / channels;
        let block: Vec<f32> = (0..frames * channels).map(|n| n as f32).collect();
        sink.push(&block);

        let mut lengths = Vec::new();
        let mut all = Vec::new();
        reader.drain(|part| {
            lengths.push(part.len());
            all.extend_from_slice(part);
        });
        for length in &lengths {
            assert_eq!(length % channels, 0, "a block of {length} split a frame");
        }
        // And nothing was lost or reordered on the way through.
        assert_eq!(all, block);
    }

    #[test]
    fn a_reader_that_never_runs_costs_samples_rather_than_the_device() {
        // The contract this whole module is built around: the callback must come back. Filling
        // the pool without draining it has to end in counted losses, not in a block.
        let (capture, mut reader, mut sink, _) = wired(1);
        for _ in 0..POOL_BUFFERS + 4 {
            sink.push(&[0.5f32; POOL_SAMPLES]);
        }
        assert_eq!(
            capture.dropped_frames(),
            4 * POOL_SAMPLES as u64,
            "everything past the pool should have been counted as lost"
        );
        // And what did fit is still intact, so the take up to the gap is usable.
        assert_eq!(drained(&mut reader).len(), POOL_BUFFERS * POOL_SAMPLES);
    }

    #[test]
    fn the_pool_goes_round_rather_than_running_out() {
        // Draining returns buffers, so a take longer than the pool is the ordinary case and not
        // the failure above.
        let (capture, mut reader, mut sink, _) = wired(1);
        for _ in 0..POOL_BUFFERS * 4 {
            sink.push(&[0.25f32; POOL_SAMPLES]);
            assert_eq!(drained(&mut reader).len(), POOL_SAMPLES);
        }
        assert_eq!(capture.dropped_frames(), 0);
    }

    #[test]
    fn an_integer_device_arrives_as_the_floats_everything_else_speaks() {
        let (_capture, mut reader, mut sink, _) = wired(1);
        sink.push(&[i16::MAX, 0, i16::MIN]);
        let out = drained(&mut reader);
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
        let (capture, _reader, mut sink, _) = wired(1);
        sink.push(&[0.2f32, -0.7, 0.3]);
        assert_eq!(capture.take_peak(), 0.7, "the peak is the absolute value");
        assert_eq!(capture.take_peak(), 0.0, "and reading it starts a new one");

        // A driver handing over a NaN must not leave the meter stuck at nothing readable.
        sink.push(&[f32::NAN, 0.4]);
        assert_eq!(capture.take_peak(), 0.4);
    }

    #[test]
    fn the_reader_finds_out_the_take_is_over_by_the_device_closing() {
        // There is no stop message. Dropping the `Capture` closes the stream, which drops the
        // sink, which is what turns the pool's far end into a disconnect — and a writer thread
        // that waited for anything else would either hang or truncate the last blocks.
        let (capture, mut reader, mut sink, _) = wired(1);
        sink.push(&[0.5f32; 64]);
        assert_eq!(drained(&mut reader).len(), 64);
        assert!(!reader.is_finished(), "the stream is still open");

        drop(sink);
        drop(capture);
        // The tail is still handed over before the end is reported, so nothing is truncated.
        assert!(!reader.is_finished(), "not until a drain has looked");
        assert_eq!(drained(&mut reader).len(), 0);
        assert!(reader.is_finished());
    }

    #[test]
    fn a_reader_is_out_on_loan_rather_than_handed_out_for_good() {
        // Two at once would split a take between two files, each with half the blocks. But a
        // device outlives the take that opened it — somebody monitoring keeps it open between
        // takes — so the second take has to be able to get one.
        let (mut capture, _, _, _) = pool_capture();
        let reader = capture.take_reader().expect("the first take");
        assert!(capture.take_reader().is_none(), "two consumers of one pool");
        capture.restore_reader(reader);
        assert!(capture.take_reader().is_some(), "the second take gets one");
    }

    #[test]
    fn an_open_device_with_no_take_running_records_nothing() {
        // Monitoring holds the stream open with no take under way. Everything a take is measured
        // by has to stay still, or a meter says a recording is running when none is — and the
        // samples must not reach the pool, where the next take would find them at its start.
        let (capture, mut reader, mut sink, playhead) = wired(1);
        capture.end_take();
        playhead.store(96_000, Ordering::Relaxed);
        sink.push(&[0.5f32; 128]);

        assert_eq!(drained(&mut reader).len(), 0, "it fed the pool");
        assert_eq!(capture.frames(), 0);
        assert_eq!(capture.started_at(), None);
        assert!(!capture.is_taking());
        // The meter is the one thing that does move: an input meter is about the device, and
        // somebody setting a level has not started a take yet.
        assert_eq!(capture.take_peak(), 0.5);
    }

    #[test]
    fn a_second_take_through_the_same_device_starts_from_nothing() {
        // The counters all mean "so far in this take". A device left open by a monitor would
        // otherwise hand the second take the first one's length, and its start position.
        let (capture, mut reader, mut sink, playhead) = wired(1);
        playhead.store(48_000, Ordering::Relaxed);
        sink.push(&[0.9f32; 256]);
        assert_eq!(drained(&mut reader).len(), 256);
        assert_eq!(capture.frames(), 256);
        assert_eq!(capture.started_at(), Some(48_000));
        capture.end_take();

        capture.begin_take();
        assert_eq!(capture.frames(), 0);
        assert_eq!(capture.started_at(), None, "it kept the first take's start");
        assert_eq!(capture.dropped_frames(), 0);
        playhead.store(96_000, Ordering::Relaxed);
        sink.push(&[0.2f32; 64]);
        assert_eq!(capture.started_at(), Some(96_000));
        assert_eq!(capture.frames(), 64);
    }

    /// A capture with its reader still attached, for the test above.
    fn pool_capture() -> (Capture, CaptureSink, Arc<CaptureShared>, ()) {
        let (capture, sink, shared) = pool(48_000.0);
        (capture, sink, shared, ())
    }

    #[test]
    fn an_empty_callback_neither_stamps_the_take_nor_counts_a_frame() {
        // A device is allowed to call back with nothing, and doing so before the transport has
        // moved would otherwise pin the take to whatever the playhead read at the time.
        let (capture, _reader, mut sink, _) = wired(2);
        sink.push::<f32>(&[]);
        assert_eq!(capture.started_at(), None);
        assert_eq!(capture.frames(), 0);
    }
}
