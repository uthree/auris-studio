//! A window of the signal, carried off the audio thread for something to look at.
//!
//! The meters next door publish one number per strip, which is small enough that a relaxed store
//! of an `f32` bit pattern is the whole mechanism. A spectrum needs the *samples*, and a thousand
//! of them cannot be written atomically — so this is a fixed ring the audio thread writes into
//! and a sequence counter that says when a reader's copy was torn.
//!
//! # Why a sequence counter rather than a lock
//!
//! The audio thread must never wait for a reader. Here it never does: it writes samples, then
//! bumps a counter. A reader records the counter, copies, and reads it again — if it moved, the
//! copy straddled a write and is thrown away. The cost of losing one is a repaint that shows the
//! previous window, which nobody can see at 60 frames a second, and the audio thread pays nothing
//! at all for it.
//!
//! # Independent readers
//!
//! The plugin editor and the utility visualizer each select one strip. The track overview
//! attaches a separate scope to each render track and enables publication only while displayed.
//! Each scope has one audio-thread producer; independent readers never change one another's source.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// How many samples one spectrum or oscilloscope window displays.
///
/// 1024 at 48 kHz is 21 ms and 47 Hz between bins: long enough to separate the notes of a chord
/// in the lower midrange, short enough that the display follows a part rather than averaging it.
pub const SCOPE_WINDOW: usize = 1024;

/// How many recent samples a [`Scope`] retains for triggered displays.
///
/// The extra window is search space for a zero-crossing trigger. Readers that ask for only
/// [`SCOPE_WINDOW`] samples still receive the newest display-sized slice.
pub const SCOPE_HISTORY: usize = SCOPE_WINDOW * 2;

/// Which strip a [`Scope`] is following.
///
/// The master bus is a strip like any other here, so a plugin editor open on a master insert
/// draws from the same mechanism as one open on a track.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScopeSource {
    /// Nothing is being analysed; the renderer skips the copy entirely.
    Off,
    /// A track, by its position in the project.
    Track(usize),
    /// The master bus.
    Master,
}

impl ScopeSource {
    /// The wire form, so the source can live in one atomic.
    fn encode(self) -> i64 {
        match self {
            ScopeSource::Off => -1,
            ScopeSource::Master => -2,
            ScopeSource::Track(index) => index as i64,
        }
    }

    fn decode(raw: i64) -> Self {
        match raw {
            -2 => ScopeSource::Master,
            index if index >= 0 => ScopeSource::Track(index as usize),
            _ => ScopeSource::Off,
        }
    }
}

/// Where on a strip a [`Scope`] listens: before its fader and pan, or after them.
///
/// After is what the strip sends to the bus, and so what is heard; before is what its chain made,
/// which is the signal a level is *set* from. The plugin editor and the overview always listen
/// after; the utility visualizer lets the user choose.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ScopeTap {
    /// After the chain, before the fader, pan and mute.
    PreFader,
    /// What the strip sends on: chain, fader, pan and mute applied.
    #[default]
    PostFader,
}

/// The most recent window of one strip's signal.
#[derive(Debug)]
pub struct Scope {
    samples: Vec<AtomicU32>,
    right: Vec<AtomicU32>,
    /// Bumped before and after a write, so an odd value means one is in progress.
    sequence: AtomicUsize,
    /// The next slot the audio thread writes; it is also the oldest retained sample.
    write: AtomicUsize,
    /// Every frame ever published, so a reader can take exactly the ones it has not seen.
    written: AtomicU64,
    source: AtomicI64,
    pre_fader: AtomicBool,
    /// [`Scope::identity`] of what the ring currently holds.
    published_source: AtomicI64,
    rate: AtomicU32,
}

/// What [`Scope::read_new`] found.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FreshSamples {
    /// How many frames were copied, oldest first, into the front of each buffer.
    pub frames: usize,
    /// Whether more was written than the ring holds since the last read, so some was lost.
    pub gap: bool,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    /// An empty scope, following nothing, retaining [`SCOPE_HISTORY`] frames.
    pub fn new() -> Self {
        Self::with_history(SCOPE_HISTORY)
    }

    /// An empty scope retaining `frames` frames, never fewer than [`SCOPE_HISTORY`].
    ///
    /// A longer ring costs the audio thread nothing — it writes each block once whatever the
    /// ring's length — and buys a reader that measures *every* sample, such as a loudness meter,
    /// the slack to miss a repaint or two without losing any.
    pub fn with_history(frames: usize) -> Self {
        let frames = frames.max(SCOPE_HISTORY);
        Self {
            samples: (0..frames).map(|_| AtomicU32::new(0)).collect(),
            right: (0..frames).map(|_| AtomicU32::new(0)).collect(),
            sequence: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            written: AtomicU64::new(0),
            source: AtomicI64::new(ScopeSource::Off.encode()),
            pre_fader: AtomicBool::new(false),
            published_source: AtomicI64::new(Self::identity(ScopeSource::Off, false)),
            rate: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    /// How many frames the ring retains.
    pub fn history(&self) -> usize {
        self.samples.len()
    }

    /// One number for a source and a tap, so the ring can be cleared when either changes.
    fn identity(source: ScopeSource, pre_fader: bool) -> i64 {
        source.encode() * 2 + i64::from(pre_fader)
    }

    /// Chooses where on the strip to listen. Takes effect on the renderer's next block, which
    /// clears the ring the way a change of source does.
    pub fn set_tap(&self, tap: ScopeTap) {
        self.pre_fader
            .store(tap == ScopeTap::PreFader, Ordering::Relaxed);
    }

    /// Where on the strip this scope listens.
    pub fn tap(&self) -> ScopeTap {
        if self.pre_fader.load(Ordering::Relaxed) {
            ScopeTap::PreFader
        } else {
            ScopeTap::PostFader
        }
    }

    /// Asks for a strip to be analysed, or for the work to stop.
    ///
    /// Called from the UI. The renderer picks it up on its next block, so a window that has just
    /// been opened shows its first spectrum within one callback.
    pub fn watch(&self, source: ScopeSource) {
        self.source.store(source.encode(), Ordering::Relaxed);
    }

    /// What is currently being analysed.
    pub fn watching(&self) -> ScopeSource {
        ScopeSource::decode(self.source.load(Ordering::Relaxed))
    }

    /// Rate the published samples were rendered at, so a reader can label its axis.
    pub fn sample_rate(&self) -> f64 {
        f64::from(f32::from_bits(self.rate.load(Ordering::Relaxed)))
    }

    /// Publishes `samples` as the newest window.
    ///
    /// Safe from the audio callback: no allocation, no locking, and no waiting on a reader. A
    /// block shorter than the window is padded from what came before it by leaving the rest of
    /// the ring alone, which is what makes the display continuous rather than strobing.
    pub fn publish(&self, samples: &[f32], sample_rate: f64) {
        self.publish_stereo(samples, samples, sample_rate);
    }

    /// Publishes paired channels without allocating or waiting on the audio thread.
    pub fn publish_stereo(&self, samples: &[f32], right: &[f32], sample_rate: f64) {
        self.publish_tapped(self.watching(), self.tap(), samples, right, sample_rate);
    }

    /// Publishes an explicitly identified strip, rejecting a concurrently changed selection.
    pub fn publish_from(
        &self,
        source: ScopeSource,
        samples: &[f32],
        right: &[f32],
        sample_rate: f64,
    ) {
        self.publish_tapped(source, ScopeTap::PostFader, samples, right, sample_rate);
    }

    /// Publishes one point on an explicitly identified strip. A block from the other tap, or for
    /// a strip nobody is watching any more, is ignored.
    pub fn publish_tapped(
        &self,
        source: ScopeSource,
        tap: ScopeTap,
        samples: &[f32],
        right: &[f32],
        sample_rate: f64,
    ) {
        if source == ScopeSource::Off || source != self.watching() || tap != self.tap() {
            return;
        }
        let history = self.samples.len();
        let source = Self::identity(source, tap == ScopeTap::PreFader);
        // The odd count has to become visible before any store below it, and the even one
        // after every store — that ordering *is* the tear detection. With everything Relaxed
        // there was none: a weakly ordered machine was free to let the sample stores overtake
        // the first increment, so a reader could copy half a new window under a counter that
        // never appeared to move. Fences on this side, acquire on the reader's.
        self.sequence.fetch_add(1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        let source_changed = self.published_source.load(Ordering::Relaxed) != source;
        if source_changed {
            for sample in self.samples.iter().chain(&self.right) {
                sample.store(0, Ordering::Relaxed);
            }
            self.write.store(0, Ordering::Relaxed);
        }
        // Only the new block is written on the audio thread. The reader starts from the cursor
        // and reconstructs chronological order away from the realtime path.
        let frames = samples.len().min(right.len());
        let taken = frames.min(history);
        let write = self.write.load(Ordering::Relaxed);
        for (offset, sample) in right[right.len() - taken..].iter().enumerate() {
            let value = if sample.is_finite() { *sample } else { 0.0 };
            self.right[(write + offset) % history].store(value.to_bits(), Ordering::Relaxed);
        }
        for (offset, sample) in samples[samples.len() - taken..].iter().enumerate() {
            let value = if sample.is_finite() { *sample } else { 0.0 };
            self.samples[(write + offset) % history].store(value.to_bits(), Ordering::Relaxed);
        }
        self.write
            .store((write + taken) % history, Ordering::Relaxed);
        self.written.fetch_add(frames as u64, Ordering::Relaxed);
        self.rate
            .store((sample_rate as f32).to_bits(), Ordering::Relaxed);
        self.published_source.store(source, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Copies the newest window into `out`, reporting whether the copy is whole.
    ///
    /// `false` means the audio thread wrote through the copy, and the caller should keep whatever
    /// it drew last rather than draw a window with a seam in it. Retrying is not worth it: the
    /// next repaint is 16 ms away and will find a settled window.
    pub fn read(&self, out: &mut [f32]) -> bool {
        self.read_stereo(out, &mut [])
    }

    /// Copies both channels under one sequence check; a torn pair must be discarded.
    pub fn read_stereo(&self, out: &mut [f32], right: &mut [f32]) -> bool {
        let before = self.sequence.load(Ordering::Acquire);
        if before % 2 == 1 {
            return false;
        }
        let write = self.write.load(Ordering::Relaxed);
        // The tail of the window, so a caller with a shorter buffer sees the most recent samples
        // rather than the oldest.
        self.copy_tail(write, out, out.len());
        let count = right.len();
        self.copy_tail_right(write, right, count);
        self.settled(before)
    }

    /// Copies every frame published since `cursor` into the front of `left` and `right`, which
    /// must each hold [`Scope::history`] frames, and moves the cursor past them.
    ///
    /// Nothing is copied and the cursor stays put when the copy was torn; the next call picks up
    /// the same frames. A cursor from before a reset or from another scope is moved to now.
    pub fn read_new(
        &self,
        cursor: &mut u64,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Option<FreshSamples> {
        let before = self.sequence.load(Ordering::Acquire);
        if before % 2 == 1 {
            return None;
        }
        let written = self.written.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Relaxed);
        let fresh = written.saturating_sub(*cursor);
        let count = (fresh.min(self.history() as u64) as usize)
            .min(left.len())
            .min(right.len());
        self.copy_tail(write, left, count);
        self.copy_tail_right(write, right, count);
        if !self.settled(before) {
            return None;
        }
        let gap = fresh > count as u64;
        *cursor = written;
        Some(FreshSamples { frames: count, gap })
    }

    fn copy_tail(&self, write: usize, out: &mut [f32], count: usize) {
        let history = self.samples.len();
        let count = count.min(history).min(out.len());
        let start = (write + history - count) % history;
        for (offset, slot) in out.iter_mut().enumerate().take(count) {
            *slot =
                f32::from_bits(self.samples[(start + offset) % history].load(Ordering::Relaxed));
        }
    }

    fn copy_tail_right(&self, write: usize, out: &mut [f32], count: usize) {
        let history = self.right.len();
        let count = count.min(history).min(out.len());
        let start = (write + history - count) % history;
        for (offset, slot) in out.iter_mut().enumerate().take(count) {
            *slot = f32::from_bits(self.right[(start + offset) % history].load(Ordering::Relaxed));
        }
    }

    /// Whether a copy that began at sequence `before` is whole and of what is being watched.
    fn settled(&self, before: usize) -> bool {
        // The fence keeps the sample loads above from drifting past the re-check below — the
        // reader's half of the ordering the writer's fence promises.
        std::sync::atomic::fence(Ordering::Acquire);
        self.sequence.load(Ordering::Relaxed) == before
            && self.published_source.load(Ordering::Relaxed)
                == Self::identity(self.watching(), self.tap() == ScopeTap::PreFader)
    }

    /// Empties the window and stops following anything.
    pub fn reset(&self) {
        self.watch(ScopeSource::Off);
        self.published_source
            .store(Self::identity(ScopeSource::Off, false), Ordering::Relaxed);
        self.write.store(0, Ordering::Relaxed);
        for slot in &self.samples {
            slot.store(0, Ordering::Relaxed);
        }
        for slot in &self.right {
            slot.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_channels_share_a_window_and_source_changes_discard_history() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        scope.publish_stereo(&[1.; 1024], &[-1.; 1024], 48000.);
        let (mut left, mut right) = ([0.; 1024], [0.; 1024]);
        assert!(scope.read_stereo(&mut left, &mut right));
        assert_eq!(left, [1.; 1024]);
        assert_eq!(right, [-1.; 1024]);
        scope.watch(ScopeSource::Track(0));
        assert!(!scope.read_stereo(&mut left, &mut right));
        scope.publish_from(ScopeSource::Master, &[1.; 1024], &[-1.; 1024], 48000.);
        assert!(!scope.read_stereo(&mut left, &mut right));
        scope.publish_stereo(&[0.25; 64], &[0.5; 64], 48000.);
        assert!(scope.read_stereo(&mut left, &mut right));
        assert_eq!(&left[..960], &[0.; 960]);
        assert_eq!(&right[..960], &[0.; 960]);
        assert_eq!(&left[960..], &[0.25; 64]);
        assert_eq!(&right[960..], &[0.5; 64]);
    }

    #[test]
    fn nothing_is_published_while_nothing_is_watching() {
        // The renderer calls this every block; the check is what keeps it free when no editor is
        // open, which is nearly always.
        let scope = Scope::new();
        assert_eq!(scope.watching(), ScopeSource::Off);
        scope.publish(&[1.0; 64], 48_000.0);

        let mut out = vec![0.0; 64];
        assert!(scope.read(&mut out));
        assert!(out.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_published_window_comes_back_newest_last() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        let block: Vec<f32> = (0..SCOPE_WINDOW).map(|n| n as f32).collect();
        scope.publish(&block, 48_000.0);

        let mut out = vec![0.0; SCOPE_WINDOW];
        assert!(scope.read(&mut out));
        assert_eq!(out.first().copied(), Some(0.0));
        assert_eq!(out.last().copied(), Some((SCOPE_WINDOW - 1) as f32));
        assert_eq!(scope.sample_rate(), 48_000.0);
    }

    #[test]
    fn a_history_reader_gets_trigger_search_space_before_the_display_window() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        scope.publish(&vec![1.0; SCOPE_WINDOW], 48_000.0);

        let mut out = vec![0.0; SCOPE_HISTORY];
        assert!(scope.read(&mut out));
        assert_eq!(&out[..SCOPE_WINDOW], &[0.0; SCOPE_WINDOW]);
        assert_eq!(&out[SCOPE_WINDOW..], &[1.0; SCOPE_WINDOW]);
    }

    #[test]
    fn short_blocks_scroll_the_window_rather_than_replacing_it() {
        // A 512-frame callback must not leave half the display empty, or the spectrum would
        // strobe at the block rate instead of following the signal.
        let scope = Scope::new();
        scope.watch(ScopeSource::Track(0));
        scope.publish(&vec![1.0; SCOPE_WINDOW], 48_000.0);
        scope.publish(&vec![2.0; 256], 48_000.0);

        let mut out = vec![0.0; SCOPE_WINDOW];
        assert!(scope.read(&mut out));
        assert_eq!(out[0], 1.0, "the older part of the window was dropped");
        assert_eq!(out[SCOPE_WINDOW - 257], 1.0);
        assert_eq!(out[SCOPE_WINDOW - 256], 2.0);
        assert_eq!(out[SCOPE_WINDOW - 1], 2.0);
    }

    #[test]
    fn a_shorter_reader_gets_the_most_recent_samples() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        let block: Vec<f32> = (0..SCOPE_WINDOW).map(|n| n as f32).collect();
        scope.publish(&block, 48_000.0);

        let mut out = vec![0.0; 4];
        assert!(scope.read(&mut out));
        assert_eq!(out, vec![1020.0, 1021.0, 1022.0, 1023.0]);
    }

    #[test]
    fn a_read_that_straddles_a_write_reports_itself_torn() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        // Leave the counter odd, which is what a write in progress looks like.
        scope.sequence.fetch_add(1, Ordering::Relaxed);
        let mut out = vec![0.0; 16];
        assert!(!scope.read(&mut out));
    }

    #[test]
    fn the_source_survives_the_round_trip_through_one_atomic() {
        let scope = Scope::new();
        for source in [
            ScopeSource::Off,
            ScopeSource::Master,
            ScopeSource::Track(0),
            ScopeSource::Track(7),
        ] {
            scope.watch(source);
            assert_eq!(scope.watching(), source);
        }
    }

    #[test]
    fn a_block_longer_than_the_window_keeps_its_tail() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        let long: Vec<f32> = (0..SCOPE_WINDOW * 3).map(|n| n as f32).collect();
        scope.publish(&long, 48_000.0);

        let mut out = vec![0.0; SCOPE_WINDOW];
        assert!(scope.read(&mut out));
        assert_eq!(out.last().copied(), Some((SCOPE_WINDOW * 3 - 1) as f32));
    }

    #[test]
    fn a_misbehaving_plugin_cannot_publish_a_nan() {
        // One would reach the display and turn a whole spectrum into nothing.
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        scope.publish(&[f32::NAN, f32::INFINITY, 0.5], 48_000.0);

        let mut out = vec![0.0; SCOPE_WINDOW];
        assert!(scope.read(&mut out));
        assert!(out.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn resetting_stops_the_analysis_and_empties_the_window() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        scope.publish(&vec![1.0; SCOPE_WINDOW], 48_000.0);
        scope.reset();

        assert_eq!(scope.watching(), ScopeSource::Off);
        let mut out = vec![0.0; SCOPE_WINDOW];
        assert!(scope.read(&mut out));
        assert!(out.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_continuous_reader_gets_every_frame_exactly_once() {
        let scope = Scope::with_history(SCOPE_HISTORY * 4);
        scope.watch(ScopeSource::Master);
        let mut cursor = 0;
        let mut left = vec![0.0; scope.history()];
        let mut right = left.clone();
        let mut seen = Vec::new();
        let mut next = 0.0f32;
        for block in [256, 300, 1, 1000, 512] {
            let samples: Vec<f32> = (0..block).map(|n| next + n as f32).collect();
            next += block as f32;
            scope.publish(&samples, 48_000.0);
            let fresh = scope.read_new(&mut cursor, &mut left, &mut right).unwrap();
            assert!(!fresh.gap);
            seen.extend_from_slice(&left[..fresh.frames]);
        }
        let expected: Vec<f32> = (0..next as usize).map(|n| n as f32).collect();
        assert_eq!(seen, expected);
        let fresh = scope.read_new(&mut cursor, &mut left, &mut right).unwrap();
        assert_eq!(fresh.frames, 0, "nothing new was published");
    }

    #[test]
    fn a_reader_that_falls_behind_is_told_it_missed_some() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        let mut cursor = 0;
        let mut left = vec![0.0; scope.history()];
        let mut right = left.clone();
        let samples: Vec<f32> = (0..SCOPE_HISTORY * 2).map(|n| n as f32).collect();
        scope.publish(&samples[..SCOPE_HISTORY], 48_000.0);
        scope.publish(&samples[SCOPE_HISTORY..], 48_000.0);
        let fresh = scope.read_new(&mut cursor, &mut left, &mut right).unwrap();
        assert!(fresh.gap);
        assert_eq!(fresh.frames, SCOPE_HISTORY);
        assert_eq!(
            left,
            &samples[SCOPE_HISTORY..],
            "the newest frames are the ones kept"
        );
    }

    #[test]
    fn a_block_from_the_other_tap_is_ignored_and_switching_clears_the_ring() {
        let scope = Scope::new();
        scope.watch(ScopeSource::Master);
        scope.set_tap(ScopeTap::PreFader);
        scope.publish_tapped(
            ScopeSource::Master,
            ScopeTap::PostFader,
            &[1.0; 8],
            &[1.0; 8],
            48_000.0,
        );
        let mut out = vec![0.0; 8];
        assert!(!scope.read(&mut out), "nothing from the chosen tap yet");
        scope.publish_tapped(
            ScopeSource::Master,
            ScopeTap::PreFader,
            &[2.0; 8],
            &[2.0; 8],
            48_000.0,
        );
        assert!(scope.read(&mut out));
        assert_eq!(out, vec![2.0; 8]);
        scope.set_tap(ScopeTap::PostFader);
        assert!(
            !scope.read(&mut out),
            "a copy of the old tap is not the new one"
        );
        scope.publish_from(ScopeSource::Master, &[3.0; 4], &[3.0; 4], 48_000.0);
        let mut window = vec![0.0; 8];
        assert!(scope.read(&mut window));
        assert_eq!(window, vec![0.0, 0.0, 0.0, 0.0, 3.0, 3.0, 3.0, 3.0]);
    }
}
