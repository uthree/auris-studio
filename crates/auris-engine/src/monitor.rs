//! Hearing the input through the mix.
//!
//! [`capture`](crate::capture) gets the microphone onto a file. This gets it onto the *speakers*,
//! which is a different problem with a different answer, because the file is written by a thread
//! that may take its time and the speakers are fed by a callback that may not.
//!
//! # Why a ring and not the pool
//!
//! The capture's pool has exactly one consumer by construction — two would split a take between
//! them — and that consumer is the thread writing the file. Monitoring is a *second* reader of the
//! same samples, on the output callback, so it gets a channel of its own: a fixed ring of stereo
//! frames that the input callback writes and the render graph reads.
//!
//! Neither realtime path allocates, locks or waits. The samples are `AtomicU32` bit patterns
//! rather than a `Vec<f32>` behind a lock, which costs a relaxed load per sample and buys a data
//! structure two realtime threads may share without any `unsafe` at all. UI-side configuration
//! may wait for one in-flight input callback before publishing a new source generation.
//! The two callbacks also publish short-lived activity flags. They never wait on one another:
//! the reader claims the oldest frame it may touch, while a writer that would have to invalidate
//! that claim drops its monitor block instead. This is what prevents a pre-empted output callback
//! from reading slots the input callback has wrapped around and overwritten.
//!
//! # The two clocks, again
//!
//! The input device and the output device do not share a crystal, and may not even share a sample
//! rate. Both are the same problem here — the reader consumes at a slightly different speed from
//! the writer — and both are answered the same way: the reader steps through the ring at
//! `input rate ÷ output rate` frames per output frame, interpolating between the two frames it
//! lands between.
//!
//! That fixes the *rate*. It does not fix the *drift*, and nothing can without measuring a rate
//! nobody has measured. So the reader sits `TARGET_BLOCKS` blocks behind the writer, and `seat`
//! decides what to do when it stops being there: too close and there is nothing to play, too far
//! and the monitor is no longer worth listening to. Both are answered by re-seating at the live
//! edge and counting it — but never *backwards*, because replaying a third of a second of
//! somebody's own voice at them is a worse noise than the gap that prompted it.
//!
//! A monitor may do that. This signal is being listened to and not kept, so a rebuffer costs
//! somebody a syllable rather than costing them a take. It is the opposite of the call
//! [`capture`](crate::capture) makes for the recording itself, and for the opposite reason.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

use auris_core::AudioBuffer;
use cpal::{FromSample, Sample};

/// Baseline frames the ring holds at a 1:1 device-rate ratio, per channel.
///
/// A third of a second at 48 kHz. It is not a latency figure — the reader deliberately runs
/// `TARGET_BLOCKS` blocks behind the writer, not a ring's length behind it — but the slack that
/// stops an output callback arriving late from being an overrun. Stereo `f32`, so 128 KiB.
const BASE_RING_FRAMES: usize = 16_384;

/// Largest input-monitor ring permitted per listening slot.
///
/// One million stereo atomic samples is 8 MiB; all four monitor slots together remain 32 MiB.
/// A backend advertising a corrupt rate or an impractically large rate/block ratio is rejected
/// before it can turn device discovery into an unbounded allocation.
pub(crate) const MAX_RING_FRAMES: usize = 1 << 20;

/// Channels the ring carries. The graph mixes in stereo; see
/// [`RENDER_CHANNELS`](crate::graph::RENDER_CHANNELS).
const CHANNELS: usize = 2;

/// How far behind the writer the reader sits, in output blocks.
///
/// This *is* the latency figure, and the only one worth arguing about. Two blocks is the least
/// that can work at all — the two callbacks are on unsynchronised clocks, so their phase drifts
/// through a whole block and back — and a third is the margin that stops an ordinary scheduling
/// hiccup being audible. At 512 frames and 48 kHz that is 32 ms of buffer on top of whatever the
/// two devices cost, which is why an interface's own direct monitoring is the better answer
/// whenever there is one.
const TARGET_BLOCKS: u64 = 3;

/// What [`MonitorRing::read`] holds while the reader is waiting for enough input to start.
const NOT_STARTED: u64 = u64::MAX;

/// A ring of input frames, written by the capture callback and read by the render graph.
///
/// Every method takes `&self`: there is one writer and one reader, they are different threads, and
/// neither may block. Shared as an `Arc` — the capture's callback holds one end, and the graph is
/// handed the other on every rebuild, the same way [`Scope`](crate::scope::Scope) is.
#[derive(Debug)]
pub struct MonitorRing {
    /// Interleaved stereo `f32` bit patterns, `capacity * CHANNELS` of them.
    samples: Box<[AtomicU32]>,
    /// Frames in `samples`, chosen before either realtime thread receives the ring.
    capacity: u64,
    /// Frames the input callback has written since the ring was made.
    written: AtomicU64,
    /// Frame the renderer has reached, or [`NOT_STARTED`] while it waits to be far enough behind.
    read: AtomicU64,
    /// How far into [`Self::read`] the renderer is, in `[0, 1)`, as an `f32` bit pattern.
    ///
    /// Only ever touched by the reader, so the pair not being one atomic costs nothing: there is
    /// no other thread that could observe them disagreeing.
    phase: AtomicU32,
    /// Rate the input device is running at, which is what sets the step through the ring.
    input_rate: f64,
    /// Serialises UI-side source/toggle changes. Neither realtime callback ever takes it.
    configuration: Mutex<()>,
    /// Handshake with a source/toggle change that must wait for an in-flight writer to publish.
    writer_active: AtomicBool,
    /// Whether the output callback is currently reading this ring.
    ///
    /// Paired with [`Self::reader_claim`]. A writer may continue into free slots while this is
    /// true, but it may not reset the ring generation and reuse the claimed window.
    reader_active: AtomicBool,
    /// Oldest frame an active reader may still load, or [`NOT_STARTED`] while it is choosing one.
    ///
    /// Written before any sample load. The writer treats the choosing state as a reason to drop
    /// one monitor block, never as permission to overwrite an unknown window.
    reader_claim: AtomicU64,
    /// Whether anybody is listening. One atomic load is what monitoring costs when it is off.
    enabled: AtomicBool,
    /// Reseat requests from the UI. Only the reader still writes `read` and `phase`.
    generation: AtomicU64,
    /// Writer position after which a newly selected/enabled source has valid samples.
    generation_start: AtomicU64,
    /// Last reseat generation observed by the reader.
    reader_generation: AtomicU64,
    /// `generation_start` captured by the reader for its current generation.
    reader_start: AtomicU64,
    /// Times the reader had to re-seat itself: the writer stalled, or lapped it, or drift closed
    /// the gap. Each one is a short silence somebody heard.
    rebuffers: AtomicU64,
    /// First device channel and channel count, packed into two `u32`s. See [`Self::set_source`].
    source: AtomicU64,
}

impl MonitorRing {
    /// An empty ring for an input device running at `input_rate`, with monitoring off.
    pub fn new(input_rate: f64) -> Self {
        Self::with_capacity(input_rate, BASE_RING_FRAMES)
    }

    /// A ring sized for the actual input/output clocks and largest output callback.
    ///
    /// `None` means the rate pair or block size would need more than [`MAX_RING_FRAMES`]. The
    /// caller may still record from that input, but must not offer software monitoring until the
    /// device rates are brought closer or the block size is reduced.
    pub(crate) fn for_output(input_rate: f64, output_rate: f64, max_block: usize) -> Option<Self> {
        let capacity = planned_capacity(input_rate, output_rate, max_block)?;
        Some(Self::with_capacity(input_rate, capacity))
    }

    fn with_capacity(input_rate: f64, capacity: usize) -> Self {
        let capacity = capacity.clamp(1, MAX_RING_FRAMES);
        MonitorRing {
            samples: (0..capacity * CHANNELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            capacity: capacity as u64,
            written: AtomicU64::new(0),
            read: AtomicU64::new(NOT_STARTED),
            phase: AtomicU32::new(0),
            input_rate,
            configuration: Mutex::new(()),
            writer_active: AtomicBool::new(false),
            reader_active: AtomicBool::new(false),
            reader_claim: AtomicU64::new(NOT_STARTED),
            enabled: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            generation_start: AtomicU64::new(0),
            reader_generation: AtomicU64::new(0),
            reader_start: AtomicU64::new(0),
            rebuffers: AtomicU64::new(0),
            source: AtomicU64::new(2_u64 << 32),
        }
    }

    #[cfg(test)]
    pub(crate) fn capacity_frames(&self) -> usize {
        self.capacity as usize
    }

    /// Points the monitor at the device channel a take would read from.
    ///
    /// The first channel of the pair; the one beside it comes with it. What makes monitoring a
    /// track armed to input 5 play input 5 rather than the microphone on input 1 — a monitor that
    /// listened somewhere other than where the take was going would be a level meter for the
    /// wrong signal.
    ///
    /// One ring, so one answer: monitoring is one track at a time, and pointing it somewhere is
    /// what re-pointing it costs.
    pub fn set_source(&self, first: usize) {
        self.set_source_channels(first, 2);
    }

    /// Points the monitor at exactly the device channels a take would keep.
    ///
    /// A one-channel source is copied to both monitor sides; two or more use the first stereo
    /// pair. Packing both values into one atomic prevents a capture callback observing half of a
    /// re-pointing operation.
    pub fn set_source_channels(&self, first: usize, count: usize) {
        let _configuration = self
            .configuration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = u64::from(u32::try_from(first).unwrap_or(u32::MAX));
        let count = u64::from(u32::try_from(count).unwrap_or(u32::MAX));
        let source = first | (count.max(1) << 32);
        if self.source.load(Ordering::Acquire) == source {
            return;
        }

        // Stop publication while changing the source. A writer already in flight checks both
        // this flag and the generation before publishing its block. Waiting for its active flag
        // closes the smaller check-then-publish race: the new generation's start is sampled only
        // after every old-source frame has either been published or abandoned.
        let was_enabled = self.enabled.swap(false, Ordering::AcqRel);
        self.wait_for_writer();
        self.source.store(source, Ordering::Release);
        if was_enabled {
            self.request_reseat(self.written.load(Ordering::Acquire));
            self.enabled.store(true, Ordering::Release);
        }
    }

    /// Whether the renderer should be mixing this in.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Turns monitoring on or off.
    ///
    /// Switching on re-seats the reader rather than resuming where it stopped, because what is in
    /// the ring is however many seconds old the pause was. Which is also why a caller that may be
    /// switching on something already on should ask [`Self::is_enabled`] first: re-seating a ring
    /// that never stopped is a gap somebody hears for no reason at all.
    pub fn set_enabled(&self, enabled: bool) {
        let _configuration = self
            .configuration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enabled == self.is_enabled() {
            return;
        }
        if !enabled {
            self.enabled.store(false, Ordering::Release);
            self.wait_for_writer();
            return;
        }
        self.wait_for_writer();
        self.request_reseat(self.written.load(Ordering::Acquire));
        self.enabled.store(true, Ordering::Release);
    }

    /// Waits outside the realtime threads until an old-source callback has left publication.
    fn wait_for_writer(&self) {
        while self.writer_active.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }

    /// Publishes a new reader generation after recording where valid new input begins.
    fn request_reseat(&self, written: u64) {
        // More than one producer can request this (UI source changes and an oversized input
        // callback). Positions are monotonic, so the latest/largest boundary is always safest.
        self.generation_start.fetch_max(written, Ordering::AcqRel);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Times the reader has had to re-seat itself, each of them an audible gap.
    ///
    /// Not an error and not hidden either. A handful over a long session is the two clocks drifting
    /// and is nothing to act on; a steady stream of them is a machine that cannot keep up, and the
    /// user is the only one who can do anything about that.
    pub fn rebuffers(&self) -> u64 {
        self.rebuffers.load(Ordering::Relaxed)
    }

    /// Accepts one input callback's worth of interleaved samples.
    ///
    /// Takes the device's own format, so the conversion happens once. Two channels of it, from
    /// wherever [`set_source`](Self::set_source) points; a mono device is fanned across both,
    /// which is what makes a laptop microphone monitor in the middle rather than hard left.
    pub fn write<T>(&self, block: &[T], channels: usize)
    where
        T: Copy,
        f32: FromSample<T>,
    {
        self.write_with_publish_hook(block, channels, || {});
    }

    /// Writer implementation with a test seam at the last pre-publication instruction.
    fn write_with_publish_hook<T>(
        &self,
        block: &[T],
        channels: usize,
        before_publish: impl FnOnce(),
    ) where
        T: Copy,
        f32: FromSample<T>,
    {
        if !self.is_enabled() {
            return;
        }
        if self
            .writer_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        struct WriterGuard<'a>(&'a AtomicBool);
        impl Drop for WriterGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _active = WriterGuard(&self.writer_active);
        // A setter can disable monitoring between the optimistic cheap check and registration.
        // It waits for this guard, so acknowledge the disable before reading source state.
        if !self.is_enabled() {
            return;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let channels = channels.max(1);
        let frames = block.len() / channels;
        if frames == 0 {
            return;
        }
        let source = self.source.load(Ordering::Acquire);
        let first = (source as u32) as usize;
        let count = (source >> 32) as u32 as usize;
        let pair = monitor_pair(first, count, channels);
        let written = self.written.load(Ordering::Acquire);
        // Sequential consistency is limited to the two activity flags. It closes the only
        // check/store race in the ring: either this writer was already active and a new reader
        // backs out, or the writer observes the active reader and honours its claim.
        let reader_active = self.reader_active.load(Ordering::SeqCst);
        let read = match reader_active {
            true => {
                let claim = self.reader_claim.load(Ordering::Acquire);
                if claim == NOT_STARTED {
                    return;
                }
                claim
            }
            false => self.read.load(Ordering::Acquire),
        };
        let unread = if read == NOT_STARTED {
            // Before the first output block, valid samples begin at the generation boundary.
            // Counting from zero let ordinary input callbacks silently wrap that whole window
            // while the output callback was pre-empted before publishing its first `read`.
            written.saturating_sub(self.generation_start.load(Ordering::Acquire))
        } else {
            written.saturating_sub(read)
        };
        // Publishing a callback that spans the ring, or laps the reader's current window before
        // `written` moves, would let the reader combine old and future slots. Drop this monitor
        // block (recording has its independent pool), request a fresh seat, and touch no samples.
        if frames as u64 >= self.capacity || unread.saturating_add(frames as u64) >= self.capacity {
            self.rebuffers.fetch_add(1, Ordering::Relaxed);
            // An active reader may already have loaded part of its claimed window. Starting a
            // fresh generation here would let later callbacks wrap and replace the rest of it.
            // Dropping this monitor block preserves that window; the reader advances its floor
            // when it completes and the next input callback can normally continue.
            if !reader_active {
                self.request_reseat(written);
            }
            return;
        }
        for frame in 0..frames {
            let base = frame * channels;
            // Silent rather than substituted when the channel is not there: a track pointed at an
            // input the device does not have records silence, and hearing the first channel in
            // its place would hide that until the take was over.
            let (left, right) = match pair {
                Some((left, right)) => (
                    f32::from_sample(block[base + left]),
                    f32::from_sample(block[base + right]),
                ),
                None => (0.0, 0.0),
            };
            // Add after reducing both operands. The absolute counter saturates only after
            // millennia of audio, but even that boundary belongs outside a realtime panic path.
            let slot_frame = (written % self.capacity + frame as u64) % self.capacity;
            let slot = slot_frame as usize * CHANNELS;
            self.samples[slot].store(left.to_bits(), Ordering::Relaxed);
            self.samples[slot + 1].store(right.to_bits(), Ordering::Relaxed);
        }
        // A source/toggle change that raced this conversion makes the just-written slots
        // unpublished. The next callback starts at the same position and overwrites them.
        if !self.is_enabled() || self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        before_publish();
        // Released last, so a reader that sees this count sees the samples behind it.
        self.written
            .store(written.saturating_add(frames as u64), Ordering::Release);
    }

    /// Mixes what the input is playing into `out`, at `out_rate`.
    ///
    /// Mixes rather than replaces: the track being monitored is an ordinary track and may have
    /// clips of its own under the playhead. Silent, and cheap, when monitoring is off or the ring
    /// has not filled to its target yet.
    pub fn read_into(&self, out: &mut AudioBuffer, out_rate: f64) {
        self.read_into_with_claim_hook(out, out_rate, || {});
    }

    /// Reader implementation with a test seam after its protected window has been claimed.
    fn read_into_with_claim_hook(
        &self,
        out: &mut AudioBuffer,
        out_rate: f64,
        after_claim: impl FnOnce(),
    ) {
        if !self.is_enabled() {
            return;
        }
        let frames = out.frame_count();
        if frames == 0 {
            return;
        }
        if self
            .reader_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        struct ReaderGuard<'a>(&'a AtomicBool);
        impl Drop for ReaderGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _active = ReaderGuard(&self.reader_active);
        self.reader_claim.store(NOT_STARTED, Ordering::Release);
        // If the writer registered first, it may have decided to reset the generation without a
        // reader to protect. Skip this output block; the next one will see the completed write.
        if self.writer_active.load(Ordering::SeqCst) {
            return;
        }
        // A setter can turn monitoring off after the optimistic check. It waits only for the
        // writer because it does not modify sample slots, so acknowledge it here as well.
        if !self.is_enabled() {
            return;
        }
        let generation = self.generation.load(Ordering::Acquire);
        if self.reader_generation.load(Ordering::Relaxed) != generation {
            self.read.store(NOT_STARTED, Ordering::Release);
            self.phase.store(0, Ordering::Relaxed);
            self.reader_start.store(
                self.generation_start.load(Ordering::Acquire),
                Ordering::Relaxed,
            );
            self.reader_generation.store(generation, Ordering::Relaxed);
        }
        let Some((step, need, target)) = read_geometry(self.input_rate, out_rate, frames) else {
            return;
        };
        // `for_output` sizes the ring for the engine's advertised maximum block, but a hostile
        // backend can still deliver a larger callback and public callers can pass any buffer.
        // Refuse that one block rather than reading samples the writer may already have lapped.
        if target > self.capacity || need > self.capacity {
            return;
        }
        let written = self.written.load(Ordering::Acquire);

        let read = self.read.load(Ordering::Acquire);
        if read == NOT_STARTED
            && written.saturating_sub(self.reader_start.load(Ordering::Relaxed)) < target
        {
            return;
        }
        let (mut position, claim) = match seat(read, written, need, target, self.capacity) {
            Seat::Play => (
                read as f64 + f64::from(f32::from_bits(self.phase.load(Ordering::Relaxed))),
                read,
            ),
            Seat::Reseat(at) => {
                // A jump mid-flow is a gap somebody heard; the first one of a session is just the
                // monitor starting. Counted here rather than on each silent block of the gap, so
                // one interruption counts once however long it lasted.
                if read != NOT_STARTED {
                    self.rebuffers.fetch_add(1, Ordering::Relaxed);
                }
                (at as f64, at)
            }
            // Nothing playable: the ring has not filled yet, or the writer has stopped and the
            // reader has caught up with it. `read` is left exactly as it was, because it is also
            // the floor that stops a recovery replaying what has already been heard.
            Seat::Wait => return,
        };
        // Published before the first sample load. A writer that registered after this reader
        // either uses this floor to stay in free slots or drops the block that would cross it.
        // Keep the exact integer chosen by `seat`; converting a counter above f64's exact range
        // back to u64 could otherwise round the claim forward and expose an older slot.
        self.reader_claim.store(claim, Ordering::Release);
        after_claim();

        for channel in 0..out.channel_count().min(CHANNELS) {
            let mut at = position;
            for sample in out.channel_mut(channel)[..frames].iter_mut() {
                let frame = at as u64;
                let fraction = (at - frame as f64) as f32;
                let first = self.sample(frame, channel);
                let second = self.sample(frame.saturating_add(1), channel);
                *sample += first + (second - first) * fraction;
                at += step;
            }
        }

        position += step * frames as f64;
        let frame = position as u64;
        // A toggle that arrived during this read wins. The next callback will observe its new
        // generation and re-seat; this in-flight callback must not overwrite that request.
        if self.is_enabled() && self.generation.load(Ordering::Acquire) == generation {
            self.read.store(frame, Ordering::Release);
            self.phase.store(
                ((position - frame as f64) as f32).to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    /// One sample out of the ring, wrapping.
    fn sample(&self, frame: u64, channel: usize) -> f32 {
        let slot = (frame % self.capacity) as usize * CHANNELS + channel;
        f32::from_bits(self.samples[slot].load(Ordering::Relaxed))
    }
}

/// Input frames and target delay needed for one output block.
fn read_geometry(input_rate: f64, output_rate: f64, frames: usize) -> Option<(f64, u64, u64)> {
    if !input_rate.is_finite()
        || input_rate <= 0.0
        || !output_rate.is_finite()
        || output_rate <= 0.0
        || frames == 0
    {
        return None;
    }
    let step = input_rate / output_rate;
    let consumed = step * frames as f64;
    if !step.is_finite() || step <= 0.0 || !consumed.is_finite() || consumed > u64::MAX as f64 - 1.0
    {
        return None;
    }
    // The one frame past the end is what interpolation of the last output sample reads.
    let need = (consumed.ceil() as u64).checked_add(1)?;
    let target = need.checked_mul(TARGET_BLOCKS)?;
    Some((step, need, target))
}

/// Ring capacity that preserves the ordinary jitter margin at this rate pair.
pub(crate) fn planned_capacity(
    input_rate: f64,
    output_rate: f64,
    max_block: usize,
) -> Option<usize> {
    let (step, need, target) = read_geometry(input_rate, output_rate, max_block)?;
    let scaled_baseline = (BASE_RING_FRAMES as f64 * step.max(1.0)).ceil();
    if !scaled_baseline.is_finite() || scaled_baseline > usize::MAX as f64 {
        return None;
    }
    // Twice the target is the late-reader limit used by `seat`; one more block keeps the
    // interpolation window distinct while the writer advances concurrently.
    let policy_minimum = target.checked_mul(2)?.checked_add(need)?;
    let required = BASE_RING_FRAMES
        .max(scaled_baseline as usize)
        .max(usize::try_from(policy_minimum).ok()?);
    let capacity = required.checked_next_power_of_two()?;
    (capacity <= MAX_RING_FRAMES).then_some(capacity)
}

/// Which two samples of a frame the monitor takes, counting from the channel it was pointed at.
///
/// The pair starting at `first`, or that one channel twice where the device has nothing beside it
/// — a mono interface, or the last channel of an odd-numbered one. `None` when the device has no
/// such channel at all, which is the case worth having a rule for: an arm can name an input that
/// was there when it was made and is not there now, and the answer to that is silence rather than
/// somebody else's microphone.
///
/// A free function because it is the whole of the rule, and a rule inside a callback is a rule
/// with no test.
fn monitor_pair(first: usize, count: usize, channels: usize) -> Option<(usize, usize)> {
    (first < channels).then(|| {
        let right = if count == 1 {
            first
        } else {
            (first + 1).min(channels - 1)
        };
        (first, right)
    })
}

/// What the reader should do with the block it has been asked for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Seat {
    /// Carry on from where it was.
    Play,
    /// Start again this many frames in, because the gap to the writer is no longer usable.
    Reseat(u64),
    /// Nothing to play yet; the ring has not filled to its target.
    Wait,
}

/// Where the reader should be, given where the writer has got to.
///
/// A free function over five numbers, because this is the whole of the drift policy and every one
/// of its cases is a thing somebody *hears*: reading past the writer is a dropout, being lapped by
/// it is a jump, and re-seating too eagerly is a stutter that never settles. None of them can be
/// provoked on demand from a real device — they need two clocks and an hour — so the arithmetic
/// has to be checkable without one.
fn seat(read: u64, written: u64, need: u64, target: u64, capacity: u64) -> Seat {
    // Starting, which every monitor does once: wait for the whole target, so the reader begins
    // with all the slack it is meant to have rather than immediately under-running.
    if read == NOT_STARTED {
        return match written >= target {
            true => Seat::Reseat(written - target),
            false => Seat::Wait,
        };
    }
    // How late the reader is allowed to be. The target is where it *sits*; this is how far the
    // clocks, or a stalled input, may push it before the latency stops being a monitor's. Twice
    // the target leaves ordinary jitter alone and still catches an input that came back after a
    // gap — carrying straight on from there would leave somebody hearing themselves a fifth of a
    // second late for the rest of the session. Capped by the ring, past which it is not late audio
    // but torn audio.
    let limit = target.saturating_mul(2).min(capacity);
    if read.saturating_add(need) <= written && written.saturating_sub(read) <= limit {
        return Seat::Play;
    }
    // Recovering. As far back as the target allows, but **never behind where it already played**:
    // re-seating to `written - target` unconditionally would replay a third of a second of
    // somebody's own voice every time the writer paused, which is a far worse noise than the gap
    // that prompted it. When even that position has nothing to play, wait for it.
    let at = written.saturating_sub(target).max(read);
    match at.saturating_add(need) <= written {
        true => Seat::Reseat(at),
        false => Seat::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring running at the same rate as the graph, already switched on.
    fn ring(rate: f64) -> MonitorRing {
        let ring = MonitorRing::new(rate);
        ring.set_enabled(true);
        ring
    }

    /// Stereo output of `frames`, silent.
    fn out(frames: usize) -> AudioBuffer {
        AudioBuffer::stereo(frames, 48_000.0)
    }

    /// Writes `frames` of a mono ramp, so every frame is identifiable by its value.
    fn ramp(ring: &MonitorRing, from: usize, frames: usize) {
        let block: Vec<f32> = (from..from + frames).map(|n| n as f32).collect();
        ring.write(&block, 1);
    }

    #[test]
    fn nothing_is_heard_until_the_ring_has_filled_to_its_target() {
        // The first few milliseconds of every monitor. Playing what is there the instant the
        // first block lands would leave the reader with no slack at all, and it would then
        // under-run on the very next block.
        let ring = ring(48_000.0);
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(buffer.peak(), 0.0, "nothing has been written yet");

        ramp(&ring, 0, 64);
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(buffer.peak(), 0.0, "one block is under the target of three");

        // Three blocks in, it starts — and starting is not a rebuffer.
        ramp(&ring, 64, 192);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.peak() > 0.0, "the monitor should be running by now");
        assert_eq!(ring.rebuffers(), 0);
    }

    #[test]
    fn samples_come_through_unchanged_when_the_rates_match() {
        // The ordinary case, and the one where any arithmetic at all would be a bug: the capture
        // asks the device for the project's rate and usually gets it.
        let ring = ring(48_000.0);
        ramp(&ring, 0, 4_096);
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);

        let left = buffer.channel(0);
        // Consecutive input frames, whatever the reader seated itself at.
        for pair in left.windows(2) {
            assert!(
                (pair[1] - pair[0] - 1.0).abs() < 1.0e-3,
                "the ramp came out as {pair:?}"
            );
        }
        assert_eq!(
            buffer.channel(1),
            left,
            "a mono device belongs in the middle, not hard left"
        );
    }

    #[test]
    fn a_stereo_device_keeps_its_sides_apart() {
        let ring = ring(48_000.0);
        let block: Vec<f32> = (0..4_096)
            .map(|n| if n % 2 == 0 { -0.5 } else { 0.5 })
            .collect();
        ring.write(&block, 2);
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.channel(0).iter().all(|s| (*s + 0.5).abs() < 1.0e-3));
        assert!(buffer.channel(1).iter().all(|s| (*s - 0.5).abs() < 1.0e-3));
    }

    #[test]
    fn a_device_at_another_rate_is_played_at_the_graphs_rate() {
        // 44.1 kHz in, 48 kHz out: the monitor must last the same *time*, or the input is
        // pitched up by 9% and the ring drains steadily until it gives up.
        let ring = ring(44_100.0);
        ramp(&ring, 0, 8_192);
        let mut buffer = out(480);
        ring.read_into(&mut buffer, 48_000.0);

        let left = buffer.channel(0);
        let travelled = left[left.len() - 1] - left[0];
        let expected = 44_100.0 / 48_000.0 * (left.len() - 1) as f32;
        assert!(
            (travelled - expected).abs() < 1.0,
            "480 output frames should have consumed {expected:.1} input frames, not {travelled:.1}"
        );
    }

    #[test]
    fn high_input_rates_scale_the_preallocated_ring_before_realtime_use() {
        assert_eq!(planned_capacity(48_000.0, 48_000.0, 512), Some(16_384));
        assert_eq!(planned_capacity(192_000.0, 48_000.0, 512), Some(65_536));

        let ring = MonitorRing::for_output(192_000.0, 48_000.0, 512)
            .expect("a four-to-one device ratio is supported");
        assert_eq!(ring.capacity, 65_536);
    }

    #[test]
    fn hostile_rates_and_blocks_are_refused_before_allocating_the_ring() {
        assert_eq!(planned_capacity(f64::INFINITY, 48_000.0, 512), None);
        assert_eq!(planned_capacity(48_000.0, 0.0, 512), None);
        assert_eq!(planned_capacity(u32::MAX as f64, 8_000.0, 65_536), None);
    }

    #[test]
    fn a_block_larger_than_a_fixed_ring_can_bridge_stays_silent() {
        let ring = ring(768_000.0);
        ramp(&ring, 0, BASE_RING_FRAMES * 2);
        let mut buffer = out(512);

        ring.read_into(&mut buffer, 8_000.0);

        assert_eq!(buffer.peak(), 0.0);
    }

    #[test]
    fn monitoring_costs_nothing_at_all_while_it_is_off() {
        // The callback holds the ring for the whole time the device is open, whether anybody is
        // listening or not, so "off" has to mean the samples are never even converted.
        let ring = MonitorRing::new(48_000.0);
        ramp(&ring, 0, 4_096);
        assert_eq!(ring.written.load(Ordering::Relaxed), 0);

        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(buffer.peak(), 0.0);
    }

    #[test]
    fn switching_on_again_starts_at_the_live_edge_rather_than_at_the_backlog() {
        // Somebody who switches monitoring on wants to hear themselves *now*. Resuming where the
        // reader stopped would play the backlog first, and the monitor would stay that far behind
        // for the rest of the session.
        let ring = ring(48_000.0);
        ramp(&ring, 0, 4_096);
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        let stopped_at = buffer.channel(0)[0];

        ring.set_enabled(false);
        // Input that arrives while monitoring is off is deliberately not retained. Re-enabling
        // must wait for fresh input rather than exposing either that stale backlog or slots from
        // the previous generation.
        ramp(&ring, 4_096, 8_192);
        ring.set_enabled(true);
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(
            buffer.peak(),
            0.0,
            "old monitor samples escaped after restart"
        );

        ramp(&ring, 12_288, 4_096);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            buffer.channel(0)[0] > stopped_at + 8_000.0,
            "the monitor resumed into the backlog instead of at the live edge"
        );
        assert_eq!(ring.rebuffers(), 0, "a restart is not a dropout");
    }

    #[test]
    fn reseat_requests_are_applied_by_the_reader_generation() {
        let ring = ring(48_000.0);
        ramp(&ring, 0, 4_096);
        ring.read_into(&mut out(64), 48_000.0);
        let before = ring.read.load(Ordering::Relaxed);

        ring.set_enabled(false);
        ring.set_enabled(true);
        assert_eq!(ring.read.load(Ordering::Relaxed), before);
        assert_ne!(
            ring.generation.load(Ordering::Relaxed),
            ring.reader_generation.load(Ordering::Relaxed)
        );

        ring.read_into(&mut out(64), 48_000.0);
        assert_eq!(
            ring.generation.load(Ordering::Relaxed),
            ring.reader_generation.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn a_writer_that_stalls_costs_silence_rather_than_a_replay() {
        // The dropout case, and the one where the obvious fix is wrong: re-seating a target
        // behind the writer would replay a third of a second of the player's own voice at them
        // every time the input hiccupped.
        let ring = ring(48_000.0);
        ramp(&ring, 0, 4_096);
        let mut buffer = out(512);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.peak() > 0.0);
        let heard = buffer.channel(0)[511];

        // Nothing more arrives. What is already in the ring plays out — that is what the target
        // is for — and then the reader waits, rather than reading ahead of the writer or going
        // back over what it has played.
        let mut loudest = heard;
        let mut ran_dry = false;
        for _ in 0..16 {
            buffer.clear();
            ring.read_into(&mut buffer, 48_000.0);
            match buffer.peak() {
                0.0 => ran_dry = true,
                peak => {
                    assert!(!ran_dry, "it started again on its own");
                    loudest = loudest.max(peak);
                }
            }
        }
        assert!(ran_dry, "it never ran out of the buffer it had");
        assert!(
            loudest < 4_096.0,
            "it played frame {loudest}, which was never written"
        );
        assert_eq!(ring.rebuffers(), 0, "the gap has not ended yet");

        // When the input comes back, so does the monitor — at the live edge, and counted once
        // however long the gap was.
        ramp(&ring, 4_096, 8_192);
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            buffer.channel(0)[0] > heard,
            "it went back over what had already been played"
        );
        assert_eq!(ring.rebuffers(), 1, "one interruption, counted once");
    }

    #[test]
    fn a_reader_the_writer_has_lapped_re_seats_rather_than_playing_torn_audio() {
        // The overrun case: the graph stopped rendering for longer than the ring is long. What is
        // under the read pointer is not old audio, it is half of one second and half of another.
        let ring = ring(48_000.0);
        ramp(&ring, 0, 4_096);
        let mut buffer = out(512);
        ring.read_into(&mut buffer, 48_000.0);
        let before = buffer.channel(0)[0];

        // A callback that could lap the current read window is rejected without touching the
        // ring. The reader first acknowledges that generation, then fresh callbacks can refill
        // its target delay.
        ramp(&ring, 4_096, BASE_RING_FRAMES - 512);
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(buffer.peak(), 0.0, "an overrun exposed overwritten slots");

        ramp(&ring, BASE_RING_FRAMES, 4_096);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            buffer.channel(0)[0] > before + (BASE_RING_FRAMES / 2) as f32,
            "it went on reading from where it was lapped"
        );
        assert_eq!(ring.rebuffers(), 1);
    }

    #[test]
    fn an_oversized_writer_never_exposes_partially_published_slots() {
        use std::sync::{Arc, Barrier};

        let ring = Arc::new(MonitorRing::with_capacity(48_000.0, 256));
        ring.set_enabled(true);
        ring.write(&vec![0.25_f32; 192], 1);
        let mut buffer = out(16);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.channel(0).iter().all(|sample| *sample == 0.25));

        let barrier = Arc::new(Barrier::new(2));
        let writer_ring = Arc::clone(&ring);
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            let oversized = vec![0.9_f32; 256];
            writer_barrier.wait();
            for _ in 0..1_024 {
                writer_ring.write(&oversized, 1);
            }
        });

        barrier.wait();
        for _ in 0..1_024 {
            buffer.clear();
            ring.read_into(&mut buffer, 48_000.0);
            assert!(
                buffer
                    .channel(0)
                    .iter()
                    .all(|sample| *sample == 0.0 || *sample == 0.25),
                "a rejected callback exposed a partially published sample"
            );
        }
        writer.join().expect("writer thread");
    }

    #[test]
    fn a_preempted_first_reader_keeps_its_window_through_writer_wrap() {
        // A 16-frame output block plus its interpolation lookahead first seats 51 frames behind
        // the input. Pause it after that
        // decision, then deliver enough ordinary 16-frame input callbacks to wrap a 256-frame
        // ring. None is individually oversized; only their aggregate can reach the read window.
        let ring = MonitorRing::with_capacity(48_000.0, 256);
        ring.set_enabled(true);
        ramp(&ring, 0, 192);
        let mut buffer = out(16);

        ring.read_into_with_claim_hook(&mut buffer, 48_000.0, || {
            for block in 0..20 {
                let from = 10_000 + block * 16;
                ramp(&ring, from, 16);
            }
        });

        assert_eq!(
            buffer.channel(0),
            &(141..157).map(|frame| frame as f32).collect::<Vec<_>>(),
            "the input callback overwrote a window while the output callback was pre-empted"
        );
        assert!(
            ring.written.load(Ordering::Acquire) < 141 + ring.capacity,
            "the writer crossed the active reader's claimed floor"
        );
    }

    #[test]
    fn changing_source_waits_for_samples_from_the_new_generation() {
        let ring = ring(48_000.0);
        let old: Vec<f32> = (0..4_096).flat_map(|_| [-0.5, 0.5]).collect();
        ring.write(&old, 2);
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.channel(0).iter().all(|sample| *sample == -0.5));

        ring.set_source_channels(1, 1);
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(
            buffer.peak(),
            0.0,
            "old-source samples escaped the generation"
        );

        let fresh: Vec<f32> = (0..4_096).flat_map(|_| [-0.25, 0.75]).collect();
        ring.write(&fresh, 2);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.channel(0).iter().all(|sample| *sample == 0.75));
        assert!(buffer.channel(1).iter().all(|sample| *sample == 0.75));
    }

    #[test]
    fn source_change_cannot_adopt_a_writer_that_already_passed_its_final_check() {
        use std::sync::{Arc, Barrier};

        let ring = Arc::new(ring(48_000.0));
        let old: Vec<f32> = (0..4_096).flat_map(|_| [-0.5, 0.5]).collect();
        ring.write(&old, 2);
        ring.read_into(&mut out(64), 48_000.0);

        let checked = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let writer_ring = Arc::clone(&ring);
        let writer_checked = Arc::clone(&checked);
        let writer_release = Arc::clone(&release);
        let writer = std::thread::spawn(move || {
            let old = [-0.75_f32, 0.25].repeat(256);
            writer_ring.write_with_publish_hook(&old, 2, || {
                writer_checked.wait();
                writer_release.wait();
            });
        });
        checked.wait();

        let setter_ring = Arc::clone(&ring);
        let setter = std::thread::spawn(move || setter_ring.set_source_channels(1, 1));
        // `enabled` is cleared before the setter waits. Once observed, the setter is guaranteed
        // not to have published the new source yet while the writer guard remains held.
        while ring.enabled.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        assert_eq!(ring.source.load(Ordering::Acquire), 2_u64 << 32);

        release.wait();
        writer.join().expect("writer thread");
        setter.join().expect("source setter thread");

        let generation_start = ring.generation_start.load(Ordering::Acquire);
        assert_eq!(generation_start, ring.written.load(Ordering::Acquire));
        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(
            buffer.peak(),
            0.0,
            "old-source frames entered the new generation"
        );

        let fresh = [-0.25_f32, 0.75].repeat(4_096);
        ring.write(&fresh, 2);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(buffer.channel(0).iter().all(|sample| *sample == 0.75));
        assert!(buffer.channel(1).iter().all(|sample| *sample == 0.75));
    }

    #[test]
    fn the_drift_policy_answers_each_case_on_its_own() {
        // `seat` is where every audible failure of a monitor is decided, and none of them can be
        // provoked from a real device without two clocks and an hour.
        const CAPACITY: u64 = 1_024;
        const NEED: u64 = 65;
        const TARGET: u64 = NEED * TARGET_BLOCKS;

        // Nothing yet, and not yet a whole target: wait. Starting early would mean starting with
        // less slack than the target is *for*, and under-running on the next block.
        assert_eq!(seat(NOT_STARTED, 0, NEED, TARGET, CAPACITY), Seat::Wait);
        assert_eq!(
            seat(NOT_STARTED, TARGET - 1, NEED, TARGET, CAPACITY),
            Seat::Wait
        );
        // Enough to start: a whole target behind the writer, so there is room to keep going.
        assert_eq!(
            seat(NOT_STARTED, TARGET, NEED, TARGET, CAPACITY),
            Seat::Reseat(0)
        );
        assert_eq!(
            seat(NOT_STARTED, 500, NEED, TARGET, CAPACITY),
            Seat::Reseat(500 - TARGET)
        );
        // Comfortably behind: carry on.
        assert_eq!(seat(300, 500, NEED, TARGET, CAPACITY), Seat::Play);
        // Exactly enough for this block: still carry on, since `need` counts the interpolation's
        // one-frame lookahead. Off by one here is a click on every block.
        assert_eq!(seat(500 - NEED, 500, NEED, TARGET, CAPACITY), Seat::Play);
        // One short: the last frame of the block has not arrived. Waits where it is rather than
        // re-seating to `written - target`, which is *behind* it and would replay.
        assert_eq!(
            seat(500 - NEED + 1, 500, NEED, TARGET, CAPACITY),
            Seat::Wait
        );
        // Too late to be a monitor any more: forward to the live edge, which is ahead of where it
        // was, so nothing is replayed. This is the case an input that stalled and came back lands
        // in — the samples are all there, and playing them would leave the player hearing
        // themselves a fifth of a second behind for the rest of the session.
        assert_eq!(
            seat(10, 10 + TARGET * 2 + 1, NEED, TARGET, CAPACITY),
            Seat::Reseat(10 + TARGET * 2 + 1 - TARGET)
        );
        // Twice the target is the limit itself, and still allowed: the check is what pushes it
        // over, not what reaches it.
        assert_eq!(
            seat(10, 10 + TARGET * 2, NEED, TARGET, CAPACITY),
            Seat::Play
        );
        // A recovery never goes backwards, even when the target says it could.
        match seat(490, 500, NEED, TARGET, CAPACITY) {
            Seat::Reseat(at) => assert!(at >= 490, "it re-seated back to {at}"),
            other => assert_eq!(other, Seat::Wait),
        }
    }

    #[test]
    fn the_pair_the_monitor_takes_starts_where_it_was_pointed() {
        // The ordinary interface: the first two channels, which is where an unpointed monitor and
        // a track armed to input 1 both land.
        assert_eq!(monitor_pair(0, 2, 2), Some((0, 1)));
        assert_eq!(monitor_pair(2, 2, 8), Some((2, 3)));
        // A mono device, and the last channel of an odd-numbered one: the same channel twice, so
        // it is heard in the middle rather than hard left.
        assert_eq!(monitor_pair(0, 2, 1), Some((0, 0)));
        assert_eq!(monitor_pair(2, 2, 3), Some((2, 2)));
        assert_eq!(monitor_pair(2, 1, 8), Some((2, 2)));
        // An input that is not there.
        assert_eq!(monitor_pair(2, 2, 2), None);
        assert_eq!(monitor_pair(0, 2, 0), None);
    }

    #[test]
    fn a_monitor_pointed_at_a_later_channel_hears_that_one() {
        // A four-input interface with the take on inputs 3 and 4. Every channel carries its own
        // number, so what came through says which one it was.
        let ring = ring(48_000.0);
        ring.set_source(2);
        let block: Vec<f32> = (0..4_096).flat_map(|_| [0.1_f32, 0.2, 0.3, 0.4]).collect();
        ring.write(&block, 4);

        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            (buffer.channel(0)[0] - 0.3).abs() < 1.0e-3,
            "the left came out as {}",
            buffer.channel(0)[0]
        );
        assert!(
            (buffer.channel(1)[0] - 0.4).abs() < 1.0e-3,
            "the right came out as {}",
            buffer.channel(1)[0]
        );
    }

    #[test]
    fn a_monitor_pointed_past_the_last_channel_hears_nothing() {
        // Not the first channel in its place. An arm can name an input that was there when the
        // interface was plugged in and is not there now, and a monitor that quietly played the
        // microphone instead would pass for a working take until it was over.
        let ring = ring(48_000.0);
        ring.set_source(6);
        let block: Vec<f32> = (0..4_096).flat_map(|_| [0.5_f32, 0.5]).collect();
        ring.write(&block, 2);

        let mut buffer = out(64);
        ring.read_into(&mut buffer, 48_000.0);
        assert_eq!(buffer.peak(), 0.0);
    }
}
