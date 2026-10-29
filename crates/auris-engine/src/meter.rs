//! Lock-free level meters shared between the audio thread and the UI.
//!
//! The audio thread is the only writer and the UI is the only reader, so a plain relaxed
//! load/store of an `f32` bit pattern is enough: no lock, no allocation, and a torn read is
//! impossible because every value is a single 32-bit word.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Peak levels for every track plus the master bus, one reading per channel.
#[derive(Debug)]
pub struct MeterBank {
    tracks: Vec<[AtomicU32; 2]>,
    /// For each track, how many more frames its meter keeps showing the two sides apart.
    ///
    /// Recharged by every block whose sides carried different signals and run down by every
    /// block that sounded but did not; see [`Self::STEREO_HOLD_SECONDS`]. Kept here rather than
    /// worked out by whoever draws the meter, because only the audio thread sees every block.
    stereo_hold: Vec<AtomicU32>,
    master: [AtomicU32; 2],
    /// Whether each track has been at or over full scale since the flag was last cleared.
    ///
    /// Latched here rather than worked out by whoever draws the meter, because it cannot be
    /// worked out from the reading. A peak falls at [`Self::FALL_DB_PER_SECOND`], so a single
    /// block over full scale has already fallen most of a decibel by the time a window redraws,
    /// and a clip visible for one frame at sixty hertz is a clip nobody sees. The audio thread
    /// is the only thing that looks at every block, so it is the only thing that can say this
    /// happened at all.
    clipped: Vec<AtomicBool>,
    master_clipped: [AtomicBool; 2],
    /// The master's counterpart of `stereo_hold`.
    master_hold: AtomicU32,
}

/// The reading at which a meter latches its clip indicator.
///
/// Digital full scale, exactly. Not a hair under it the way an analogue console's would be:
/// there is nothing above 1.0 to run into, and a sample that reached it may well have been
/// clamped on the way out.
const FULL_SCALE: f32 = 1.0;

/// A peak below which a block counts as silence for the stereo hold: -100 dBFS.
const SILENCE: f32 = 1e-5;

impl MeterBank {
    /// How fast a peak reading falls once the signal goes away, in decibels per second.
    ///
    /// 20 dB/s is the fallback rate of an IEC 60268-10 Type I peak programme meter, slow enough
    /// to read at 60 fps and fast enough to follow a fading note.
    pub const FALL_DB_PER_SECOND: f32 = 20.0;

    /// How long a track keeps two bars after its sides last differed, in seconds of sound.
    ///
    /// Without a hold a stereo track would drop to one bar on every block that happens to be
    /// one signal — a centred bass note in an otherwise wide mix, the first block of a reverb —
    /// and the meter would flicker between one bar and two. Silence does not run it down: a
    /// stereo track that stops is still a stereo track when it starts again.
    pub const STEREO_HOLD_SECONDS: f64 = 2.0;

    /// A bank sized for `track_capacity` tracks, all reading silence.
    ///
    /// The capacity is fixed for the life of the bank because the audio thread must never
    /// allocate; reports for indices beyond it are dropped.
    pub fn new(track_capacity: usize) -> Self {
        Self {
            tracks: (0..track_capacity)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            stereo_hold: (0..track_capacity).map(|_| AtomicU32::new(0)).collect(),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
            clipped: (0..track_capacity)
                .map(|_| AtomicBool::new(false))
                .collect(),
            master_clipped: [AtomicBool::new(false), AtomicBool::new(false)],
            master_hold: AtomicU32::new(0),
        }
    }

    /// Number of track slots this bank can hold.
    pub fn track_capacity(&self) -> usize {
        self.tracks.len()
    }

    /// Records a track's left and right peaks for a block of `frames`, applying the decay, and
    /// whether the block's two sides carried different signals.
    ///
    /// Safe to call from the audio callback: it neither allocates nor locks.
    pub fn report_track(
        &self,
        index: usize,
        peaks: [f32; 2],
        sides_differ: bool,
        frames: usize,
        sample_rate: f64,
    ) {
        let peaks = peaks.map(Self::sanitized_peak);
        if let Some(slots) = self.tracks.get(index) {
            for (slot, peak) in slots.iter().zip(peaks) {
                Self::report(slot, peak, frames, sample_rate);
            }
        }
        if let Some(hold) = self.stereo_hold.get(index) {
            Self::hold(
                hold,
                sides_differ,
                peaks[0].max(peaks[1]),
                frames,
                sample_rate,
            );
        }
        if peaks[0].max(peaks[1]) >= FULL_SCALE
            && let Some(flag) = self.clipped.get(index)
        {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Records a master bus channel's peak for a block of `frames`.
    pub fn report_master(&self, channel: usize, peak: f32, frames: usize, sample_rate: f64) {
        let peak = Self::sanitized_peak(peak);
        if let Some(slot) = self.master.get(channel) {
            Self::report(slot, peak, frames, sample_rate);
        }
        if peak >= FULL_SCALE
            && let Some(flag) = self.master_clipped.get(channel)
        {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Records whether the master's two sides carried different signals over a block of
    /// `frames`, after both of its channels have been reported for it.
    pub fn report_master_width(&self, sides_differ: bool, frames: usize, sample_rate: f64) {
        let loudest = self.master_peak();
        Self::hold(
            &self.master_hold,
            sides_differ,
            loudest,
            frames,
            sample_rate,
        );
    }

    /// Whether the master's two sides have recently carried different signals.
    pub fn master_stereo(&self) -> bool {
        self.master_hold.load(Ordering::Relaxed) > 0
    }

    /// Recharges or runs down a stereo hold; see [`Self::STEREO_HOLD_SECONDS`].
    fn hold(hold: &AtomicU32, sides_differ: bool, peak: f32, frames: usize, sample_rate: f64) {
        if sides_differ {
            let frames = (Self::STEREO_HOLD_SECONDS * sample_rate.max(0.0)) as u32;
            hold.store(frames, Ordering::Relaxed);
        } else if peak > SILENCE {
            let left = hold.load(Ordering::Relaxed);
            hold.store(left.saturating_sub(frames as u32), Ordering::Relaxed);
        }
    }

    /// Current peak reading for a track, the louder of its two channels, or 0 for an
    /// out-of-range index.
    pub fn track_peak(&self, index: usize) -> f32 {
        let [left, right] = self.track_channel_peaks(index);
        left.max(right)
    }

    /// Current left and right peak readings for a track, or silence for an out-of-range index.
    pub fn track_channel_peaks(&self, index: usize) -> [f32; 2] {
        self.tracks.get(index).map_or([0.0, 0.0], |slots| {
            [0, 1].map(|channel| f32::from_bits(slots[channel].load(Ordering::Relaxed)))
        })
    }

    /// Whether a track's two sides have recently carried different signals, so its meter
    /// shows each side rather than one bar for a single signal.
    pub fn track_stereo(&self, index: usize) -> bool {
        self.stereo_hold
            .get(index)
            .is_some_and(|hold| hold.load(Ordering::Relaxed) > 0)
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

    /// Whether a track has been over full scale since the indicator was last cleared.
    pub fn track_clipped(&self, index: usize) -> bool {
        self.clipped
            .get(index)
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Whether either master channel has been over full scale since the last clear.
    pub fn master_clipped(&self) -> bool {
        self.master_clipped
            .iter()
            .any(|flag| flag.load(Ordering::Relaxed))
    }

    /// Whether anything at all is showing a clip.
    ///
    /// What decides whether the indicator is worth offering to clear. Asked of the bank rather
    /// than of every meter on screen, because a track scrolled out of sight has clipped just as
    /// loudly as one in view.
    pub fn anything_clipped(&self) -> bool {
        self.master_clipped() || self.clipped.iter().any(|flag| flag.load(Ordering::Relaxed))
    }

    /// Puts out every clip indicator.
    ///
    /// Only ever by asking. A latch that cleared itself on the next quiet block would be a
    /// reading, not a latch, and the whole point is to still be lit when somebody looks up.
    pub fn clear_clipped(&self) {
        for flag in self.clipped.iter().chain(self.master_clipped.iter()) {
            flag.store(false, Ordering::Relaxed);
        }
    }

    /// Drops every reading to silence, for example after a panic or a stop.
    ///
    /// The clip indicators go out with it: a panic is somebody saying "stop, and let me start
    /// again", and starting again with the last mix's red lights still on is starting again
    /// with somebody else's evidence.
    pub fn reset(&self) {
        self.clear_clipped();
        for slot in self.tracks.iter().flatten() {
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
        for slot in self.tracks.iter().skip(index).flatten() {
            slot.store(0, Ordering::Relaxed);
        }
        for flag in self.clipped.iter().skip(index) {
            flag.store(false, Ordering::Relaxed);
        }
        for hold in self.stereo_hold.iter().skip(index) {
            hold.store(0, Ordering::Relaxed);
        }
    }

    /// Multiplier applied to the previous reading after `frames` have elapsed.
    pub(crate) fn decay(frames: usize, sample_rate: f64) -> f32 {
        if sample_rate <= 0.0 || frames == 0 {
            return 1.0;
        }
        // A fall of `FALL_DB_PER_SECOND` dB per second is a gain factor of
        // 10^(-FALL/20) per second, raised to the fraction of a second this block covers.
        let per_second = 10f32.powf(-Self::FALL_DB_PER_SECOND / 20.0);
        per_second.powf(frames as f32 / sample_rate as f32)
    }

    fn report(slot: &AtomicU32, peak: f32, frames: usize, sample_rate: f64) {
        let previous = f32::from_bits(slot.load(Ordering::Relaxed));
        let held = previous * Self::decay(frames, sample_rate);
        slot.store(peak.max(held).to_bits(), Ordering::Relaxed);
    }

    fn sanitized_peak(peak: f32) -> f32 {
        if peak.is_finite() { peak.abs() } else { 0.0 }
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
    fn a_non_finite_peak_is_silence_for_both_meter_and_clip_latch() {
        let bank = MeterBank::new(1);
        bank.report_track(0, [f32::INFINITY, f32::INFINITY], true, 512, 48_000.0);
        bank.report_master(0, f32::NAN, 512, 48_000.0);
        assert_eq!(bank.track_peak(0), 0.0);
        assert_eq!(bank.master_channel_peak(0), 0.0);
        assert!(!bank.track_clipped(0));
        assert!(!bank.master_clipped());
    }

    #[test]
    fn a_peak_is_held_then_falls_at_the_documented_rate() {
        let bank = MeterBank::new(2);
        bank.report_track(0, [1.0, 1.0], true, 480, 48_000.0);
        assert_eq!(bank.track_peak(0), 1.0);

        // One full second of silence at 48 kHz, in 100 blocks of 480 frames.
        for _ in 0..100 {
            bank.report_track(0, [0.0, 0.0], true, 480, 48_000.0);
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
        bank.report_track(0, [0.25, 0.25], true, 512, 48_000.0);
        bank.report_track(0, [0.75, 0.75], true, 512, 48_000.0);
        assert_eq!(bank.track_peak(0), 0.75);
    }

    #[test]
    fn out_of_range_reports_and_reads_are_ignored() {
        let bank = MeterBank::new(1);
        bank.report_track(9, [1.0, 1.0], true, 512, 48_000.0);
        assert_eq!(bank.track_peak(9), 0.0);
        assert_eq!(bank.track_capacity(), 1);
    }

    #[test]
    fn the_master_shows_two_bars_only_while_its_sides_differ() {
        let bank = MeterBank::new(0);
        bank.report_master(0, 0.5, 512, 48_000.0);
        bank.report_master(1, 0.5, 512, 48_000.0);
        bank.report_master_width(false, 512, 48_000.0);
        assert!(!bank.master_stereo(), "a mono mix is one bar");
        bank.report_master_width(true, 512, 48_000.0);
        assert!(bank.master_stereo());
        bank.report_master_width(false, 96_000, 48_000.0);
        assert!(
            !bank.master_stereo(),
            "and back to one once the hold has run out"
        );
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
        bank.report_track(0, [f32::NAN, f32::NAN], true, 512, 48_000.0);
        assert_eq!(bank.track_peak(0), 0.0);
    }

    #[test]
    fn a_clip_stays_lit_after_the_reading_that_caused_it_has_fallen_away() {
        let bank = MeterBank::new(2);
        bank.report_track(0, [1.0, 1.0], true, 512, 48_000.0);
        assert!(bank.track_clipped(0));
        assert!(bank.anything_clipped());
        assert!(!bank.track_clipped(1), "only the track that clipped");

        // A second of silence. The reading falls to nothing; the latch does not move, which is
        // the whole point — a clip seen for one frame at sixty hertz is a clip nobody sees.
        for _ in 0..94 {
            bank.report_track(0, [0.0, 0.0], true, 512, 48_000.0);
        }
        assert!(bank.track_peak(0) < 0.2);
        assert!(bank.track_clipped(0));

        bank.clear_clipped();
        assert!(!bank.track_clipped(0));
        assert!(!bank.anything_clipped());
    }

    #[test]
    fn a_level_short_of_full_scale_does_not_light_anything() {
        let bank = MeterBank::new(1);
        bank.report_track(0, [0.999, 0.999], true, 512, 48_000.0);
        bank.report_master(0, 0.999, 512, 48_000.0);
        assert!(!bank.anything_clipped());

        bank.report_master(1, 1.5, 512, 48_000.0);
        assert!(bank.master_clipped());
    }

    #[test]
    fn each_side_is_read_on_its_own_and_two_bars_are_held_while_the_sides_differ() {
        let bank = MeterBank::new(2);
        bank.report_track(0, [0.5, 0.125], true, 512, 48_000.0);
        bank.report_track(1, [0.25, 0.25], false, 512, 48_000.0);
        assert_eq!(bank.track_channel_peaks(0), [0.5, 0.125]);
        assert_eq!(bank.track_peak(0), 0.5);
        assert!(bank.track_stereo(0));
        assert!(!bank.track_stereo(1));
        // One signal for a moment does not drop the second bar, nor does silence of any length.
        bank.report_track(0, [0.5, 0.5], false, 48_000, 48_000.0);
        for _ in 0..10 {
            bank.report_track(0, [0.0, 0.0], false, 48_000, 48_000.0);
        }
        assert!(
            bank.track_stereo(0),
            "held through a second of one signal and the silence"
        );
        // Sounding as one signal for longer than the hold does.
        bank.report_track(0, [0.5, 0.5], false, 48_000, 48_000.0);
        assert!(!bank.track_stereo(0), "dropped after the hold ran out");
        bank.report_track(0, [0.5, 0.125], true, 512, 48_000.0);
        assert!(bank.track_stereo(0));
        // A deleted track forgets that it was stereo along with its readings.
        bank.clear_tracks_from(0);
        assert_eq!(bank.track_channel_peaks(0), [0.0, 0.0]);
        assert!(!bank.track_stereo(0));
    }
}
