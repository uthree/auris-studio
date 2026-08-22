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
//! Nothing here allocates, locks or waits. The samples are `AtomicU32` bit patterns rather than a
//! `Vec<f32>` behind a lock, which costs a relaxed load per sample and buys a data structure two
//! realtime threads may share without any `unsafe` at all.
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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use auris_core::AudioBuffer;
use cpal::{FromSample, Sample};

/// Frames the ring holds, per channel.
///
/// A third of a second at 48 kHz. It is not a latency figure — the reader deliberately runs
/// `TARGET_BLOCKS` blocks behind the writer, not a ring's length behind it — but the slack that
/// stops an output callback arriving late from being an overrun. Stereo `f32`, so 64 kB.
const RING_FRAMES: usize = 16_384;

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
    /// Interleaved stereo `f32` bit patterns, `RING_FRAMES * CHANNELS` of them.
    samples: Box<[AtomicU32]>,
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
    /// Whether anybody is listening. One relaxed load is what monitoring costs when it is off.
    enabled: AtomicBool,
    /// Times the reader had to re-seat itself: the writer stalled, or lapped it, or drift closed
    /// the gap. Each one is a short silence somebody heard.
    rebuffers: AtomicU64,
    /// The first device channel to listen to. See [`Self::set_source`].
    source: AtomicUsize,
}

impl MonitorRing {
    /// An empty ring for an input device running at `input_rate`, with monitoring off.
    pub fn new(input_rate: f64) -> Self {
        MonitorRing {
            samples: (0..RING_FRAMES * CHANNELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(NOT_STARTED),
            phase: AtomicU32::new(0),
            input_rate: input_rate.max(1.0),
            enabled: AtomicBool::new(false),
            rebuffers: AtomicU64::new(0),
            source: AtomicUsize::new(0),
        }
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
        self.source.store(first, Ordering::Relaxed);
    }

    /// Whether the renderer should be mixing this in.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Turns monitoring on or off.
    ///
    /// Switching on re-seats the reader rather than resuming where it stopped, because what is in
    /// the ring is however many seconds old the pause was. Which is also why a caller that may be
    /// switching on something already on should ask [`Self::is_enabled`] first: re-seating a ring
    /// that never stopped is a gap somebody hears for no reason at all.
    pub fn set_enabled(&self, enabled: bool) {
        if enabled {
            self.read.store(NOT_STARTED, Ordering::Relaxed);
        }
        self.enabled.store(enabled, Ordering::Relaxed);
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
        if !self.is_enabled() {
            return;
        }
        let channels = channels.max(1);
        let frames = block.len() / channels;
        if frames == 0 {
            return;
        }
        let pair = monitor_pair(self.source.load(Ordering::Relaxed), channels);
        let written = self.written.load(Ordering::Relaxed);
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
            let slot = ((written + frame as u64) % RING_FRAMES as u64) as usize * CHANNELS;
            self.samples[slot].store(left.to_bits(), Ordering::Relaxed);
            self.samples[slot + 1].store(right.to_bits(), Ordering::Relaxed);
        }
        // Released last, so a reader that sees this count sees the samples behind it.
        self.written
            .store(written + frames as u64, Ordering::Release);
    }

    /// Mixes what the input is playing into `out`, at `out_rate`.
    ///
    /// Mixes rather than replaces: the track being monitored is an ordinary track and may have
    /// clips of its own under the playhead. Silent, and cheap, when monitoring is off or the ring
    /// has not filled to its target yet.
    pub fn read_into(&self, out: &mut AudioBuffer, out_rate: f64) {
        if !self.is_enabled() {
            return;
        }
        let frames = out.frame_count();
        if frames == 0 {
            return;
        }
        let step = self.input_rate / out_rate.max(1.0);
        // The block's worth of input, plus the one frame past the end that the interpolation of
        // the last output frame reads.
        let need = (step * frames as f64).ceil() as u64 + 1;
        let target = need * TARGET_BLOCKS;
        let written = self.written.load(Ordering::Acquire);

        let read = self.read.load(Ordering::Relaxed);
        let mut position = match seat(read, written, need, target, RING_FRAMES as u64) {
            Seat::Play => {
                read as f64 + f64::from(f32::from_bits(self.phase.load(Ordering::Relaxed)))
            }
            Seat::Reseat(at) => {
                // A jump mid-flow is a gap somebody heard; the first one of a session is just the
                // monitor starting. Counted here rather than on each silent block of the gap, so
                // one interruption counts once however long it lasted.
                if read != NOT_STARTED {
                    self.rebuffers.fetch_add(1, Ordering::Relaxed);
                }
                at as f64
            }
            // Nothing playable: the ring has not filled yet, or the writer has stopped and the
            // reader has caught up with it. `read` is left exactly as it was, because it is also
            // the floor that stops a recovery replaying what has already been heard.
            Seat::Wait => return,
        };

        for channel in 0..out.channel_count().min(CHANNELS) {
            let mut at = position;
            for sample in out.channel_mut(channel)[..frames].iter_mut() {
                let frame = at as u64;
                let fraction = (at - frame as f64) as f32;
                let first = self.sample(frame, channel);
                let second = self.sample(frame + 1, channel);
                *sample += first + (second - first) * fraction;
                at += step;
            }
        }

        position += step * frames as f64;
        let frame = position as u64;
        self.read.store(frame, Ordering::Relaxed);
        self.phase.store(
            ((position - frame as f64) as f32).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// One sample out of the ring, wrapping.
    fn sample(&self, frame: u64, channel: usize) -> f32 {
        let slot = (frame % RING_FRAMES as u64) as usize * CHANNELS + channel;
        f32::from_bits(self.samples[slot].load(Ordering::Relaxed))
    }
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
fn monitor_pair(first: usize, channels: usize) -> Option<(usize, usize)> {
    (first < channels).then(|| (first, (first + 1).min(channels - 1)))
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
    let limit = (target * 2).min(capacity);
    if read + need <= written && written - read <= limit {
        return Seat::Play;
    }
    // Recovering. As far back as the target allows, but **never behind where it already played**:
    // re-seating to `written - target` unconditionally would replay a third of a second of
    // somebody's own voice every time the writer paused, which is a far worse noise than the gap
    // that prompted it. When even that position has nothing to play, wait for it.
    let at = written.saturating_sub(target).max(read);
    match at + need <= written {
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

        // Eight thousand frames go past while nothing reads them.
        ramp(&ring, 4_096, 8_192);
        ring.set_enabled(false);
        ring.set_enabled(true);
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            buffer.channel(0)[0] > stopped_at + 8_000.0,
            "the monitor resumed into the backlog instead of at the live edge"
        );
        assert_eq!(ring.rebuffers(), 0, "a restart is not a dropout");
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

        for start in 0..8 {
            ramp(&ring, 4_096 + start * RING_FRAMES, RING_FRAMES);
        }
        buffer.clear();
        ring.read_into(&mut buffer, 48_000.0);
        assert!(
            buffer.channel(0)[0] > before + RING_FRAMES as f32,
            "it went on reading from where it was lapped"
        );
        assert_eq!(ring.rebuffers(), 1);
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
        assert_eq!(monitor_pair(0, 2), Some((0, 1)));
        assert_eq!(monitor_pair(2, 8), Some((2, 3)));
        // A mono device, and the last channel of an odd-numbered one: the same channel twice, so
        // it is heard in the middle rather than hard left.
        assert_eq!(monitor_pair(0, 1), Some((0, 0)));
        assert_eq!(monitor_pair(2, 3), Some((2, 2)));
        // An input that is not there.
        assert_eq!(monitor_pair(2, 2), None);
        assert_eq!(monitor_pair(0, 0), None);
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
