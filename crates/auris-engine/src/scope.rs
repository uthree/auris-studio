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
//! # Why only one strip at a time
//!
//! Analysis exists for a window that is open, and at most one plugin editor is. Publishing every
//! strip would mean a ring each and a copy per block for readings nothing is drawing; instead the
//! UI names the strip it is looking at and the renderer fills only that one.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};

/// How many samples one window holds.
///
/// 1024 at 48 kHz is 21 ms and 47 Hz between bins: long enough to separate the notes of a chord
/// in the lower midrange, short enough that the display follows a part rather than averaging it.
pub const SCOPE_WINDOW: usize = 1024;

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

/// The most recent window of one strip's signal.
#[derive(Debug)]
pub struct Scope {
    samples: Vec<AtomicU32>,
    /// Bumped before and after a write, so an odd value means one is in progress.
    sequence: AtomicUsize,
    source: AtomicI64,
    rate: AtomicU32,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    /// An empty scope, following nothing.
    pub fn new() -> Self {
        Self {
            samples: (0..SCOPE_WINDOW).map(|_| AtomicU32::new(0)).collect(),
            sequence: AtomicUsize::new(0),
            source: AtomicI64::new(ScopeSource::Off.encode()),
            rate: AtomicU32::new(0.0f32.to_bits()),
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
        if matches!(self.watching(), ScopeSource::Off) {
            return;
        }
        self.sequence.fetch_add(1, Ordering::Relaxed);
        // Newest-last, so a reader can walk the slice forwards and get time's own order.
        let taken = samples.len().min(SCOPE_WINDOW);
        let keep = SCOPE_WINDOW - taken;
        for index in 0..keep {
            let carried = self.samples[index + taken].load(Ordering::Relaxed);
            self.samples[index].store(carried, Ordering::Relaxed);
        }
        for (offset, sample) in samples[samples.len() - taken..].iter().enumerate() {
            let value = if sample.is_finite() { *sample } else { 0.0 };
            self.samples[keep + offset].store(value.to_bits(), Ordering::Relaxed);
        }
        self.rate
            .store((sample_rate as f32).to_bits(), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Relaxed);
    }

    /// Copies the newest window into `out`, reporting whether the copy is whole.
    ///
    /// `false` means the audio thread wrote through the copy, and the caller should keep whatever
    /// it drew last rather than draw a window with a seam in it. Retrying is not worth it: the
    /// next repaint is 16 ms away and will find a settled window.
    pub fn read(&self, out: &mut [f32]) -> bool {
        let before = self.sequence.load(Ordering::Relaxed);
        if before % 2 == 1 {
            return false;
        }
        let count = out.len().min(SCOPE_WINDOW);
        // The tail of the window, so a caller with a shorter buffer sees the most recent samples
        // rather than the oldest.
        let start = SCOPE_WINDOW - count;
        for (offset, slot) in out.iter_mut().enumerate().take(count) {
            *slot = f32::from_bits(self.samples[start + offset].load(Ordering::Relaxed));
        }
        self.sequence.load(Ordering::Relaxed) == before
    }

    /// Empties the window and stops following anything.
    pub fn reset(&self) {
        self.watch(ScopeSource::Off);
        for slot in &self.samples {
            slot.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
