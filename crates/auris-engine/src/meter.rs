//! Lock-free level meters shared between the audio thread and the UI.
//!
//! The audio thread is the only writer and the UI is the only reader, so a plain relaxed
//! load/store of an `f32` bit pattern is enough: no lock, no allocation, and a torn read is
//! impossible because every value is a single 32-bit word.

use std::sync::atomic::{AtomicU32, Ordering};

/// Peak levels for every track plus the master bus.
#[derive(Debug)]
pub struct MeterBank {
    tracks: Vec<AtomicU32>,
    master: [AtomicU32; 2],
}

impl MeterBank {
    /// How fast a peak reading falls once the signal goes away, in decibels per second.
    ///
    /// 20 dB/s is the fallback rate of an IEC 60268-10 Type I peak programme meter, slow enough
    /// to read at 60 fps and fast enough to follow a fading note.
    pub const FALL_DB_PER_SECOND: f32 = 20.0;

    /// A bank sized for `track_capacity` tracks, all reading silence.
    ///
    /// The capacity is fixed for the life of the bank because the audio thread must never
    /// allocate; reports for indices beyond it are dropped.
    pub fn new(track_capacity: usize) -> Self {
        Self {
            tracks: (0..track_capacity).map(|_| AtomicU32::new(0)).collect(),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    /// Number of track slots this bank can hold.
    pub fn track_capacity(&self) -> usize {
        self.tracks.len()
    }

    /// Records a track's peak for a block of `frames`, applying the decay.
    ///
    /// Safe to call from the audio callback: it neither allocates nor locks.
    pub fn report_track(&self, index: usize, peak: f32, frames: usize, sample_rate: f64) {
        if let Some(slot) = self.tracks.get(index) {
            Self::report(slot, peak, frames, sample_rate);
        }
    }

    /// Records a master bus channel's peak for a block of `frames`.
    pub fn report_master(&self, channel: usize, peak: f32, frames: usize, sample_rate: f64) {
        if let Some(slot) = self.master.get(channel) {
            Self::report(slot, peak, frames, sample_rate);
        }
    }

    /// Current peak reading for a track, or 0 for an out-of-range index.
    pub fn track_peak(&self, index: usize) -> f32 {
        self.tracks
            .get(index)
            .map_or(0.0, |slot| f32::from_bits(slot.load(Ordering::Relaxed)))
    }

    /// Current peak reading for one master channel.
    pub fn master_channel_peak(&self, channel: usize) -> f32 {
        self.master
            .get(channel)
            .map_or(0.0, |slot| f32::from_bits(slot.load(Ordering::Relaxed)))
    }

    /// Loudest master channel.
    pub fn master_peak(&self) -> f32 {
        self.master_channel_peak(0).max(self.master_channel_peak(1))
    }

    /// Drops every reading to silence, for example after a panic or a stop.
    pub fn reset(&self) {
        for slot in &self.tracks {
            slot.store(0, Ordering::Relaxed);
        }
        for slot in &self.master {
            slot.store(0, Ordering::Relaxed);
        }
    }

    /// Silences track readings from `index` onwards.
    ///
    /// Nothing reports a track that no longer exists, so without this a meter would keep the last
    /// level of a deleted track for ever. Called when a smaller graph is installed. Allocation-
    /// and lock-free, so it is safe from the audio thread.
    pub fn clear_tracks_from(&self, index: usize) {
        for slot in self.tracks.iter().skip(index) {
            slot.store(0, Ordering::Relaxed);
        }
    }

    /// Multiplier applied to the previous reading after `frames` have elapsed.
    fn decay(frames: usize, sample_rate: f64) -> f32 {
        if sample_rate <= 0.0 || frames == 0 {
            return 1.0;
        }
        // A fall of `FALL_DB_PER_SECOND` dB per second is a gain factor of
        // 10^(-FALL/20) per second, raised to the fraction of a second this block covers.
        let per_second = 10f32.powf(-Self::FALL_DB_PER_SECOND / 20.0);
        per_second.powf(frames as f32 / sample_rate as f32)
    }

    fn report(slot: &AtomicU32, peak: f32, frames: usize, sample_rate: f64) {
        let peak = if peak.is_finite() { peak.abs() } else { 0.0 };
        let previous = f32::from_bits(slot.load(Ordering::Relaxed));
        let held = previous * Self::decay(frames, sample_rate);
        slot.store(peak.max(held).to_bits(), Ordering::Relaxed);
    }
}

impl Default for MeterBank {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peak_is_held_then_falls_at_the_documented_rate() {
        let bank = MeterBank::new(2);
        bank.report_track(0, 1.0, 480, 48_000.0);
        assert_eq!(bank.track_peak(0), 1.0);

        // One full second of silence at 48 kHz, in 100 blocks of 480 frames.
        for _ in 0..100 {
            bank.report_track(0, 0.0, 480, 48_000.0);
        }
        // 20 dB down from 1.0 is 0.1.
        let level = bank.track_peak(0);
        assert!(
            (level - 0.1).abs() < 1e-3,
            "expected ~0.1 after one second, got {level}"
        );
    }

    #[test]
    fn a_louder_block_overrides_the_held_value_immediately() {
        let bank = MeterBank::new(1);
        bank.report_track(0, 0.25, 512, 48_000.0);
        bank.report_track(0, 0.75, 512, 48_000.0);
        assert_eq!(bank.track_peak(0), 0.75);
    }

    #[test]
    fn out_of_range_reports_and_reads_are_ignored() {
        let bank = MeterBank::new(1);
        bank.report_track(9, 1.0, 512, 48_000.0);
        assert_eq!(bank.track_peak(9), 0.0);
        assert_eq!(bank.track_capacity(), 1);
    }

    #[test]
    fn master_peak_is_the_louder_channel() {
        let bank = MeterBank::new(0);
        bank.report_master(0, 0.2, 512, 48_000.0);
        bank.report_master(1, 0.6, 512, 48_000.0);
        assert_eq!(bank.master_peak(), 0.6);
        bank.reset();
        assert_eq!(bank.master_peak(), 0.0);
    }

    #[test]
    fn non_finite_levels_are_treated_as_silence() {
        let bank = MeterBank::new(1);
        bank.report_track(0, f32::NAN, 512, 48_000.0);
        assert_eq!(bank.track_peak(0), 0.0);
    }
}
