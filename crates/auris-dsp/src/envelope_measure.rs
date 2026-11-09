//! Attack, decay, sustain and release measured from a rendered note, in milliseconds and decibels.
//!
//! This is the inverse of [`crate::Adsr`]: instead of generating an envelope from four numbers,
//! it listens to one note and reports the four numbers a person would have set to get it. The
//! note is captured adaptively — [`EnvelopeProbe`] decides when to let go of the key and when the
//! tail has died — so a pluck finishes in a fraction of a second and a pad is not cut short.
//!
//! Every value is relative to the note's own peak, so the measurement does not depend on how
//! loud the instrument is. The level is the power summed over channels in 5 ms windows, then
//! median-smoothed over 25 ms, which removes tremolo ripple without blurring a fast attack.

use serde::{Deserialize, Serialize};

/// Width of one level window, in seconds.
const WINDOW_SECONDS: f64 = 0.005;
/// Windows in the median smoother (25 ms).
const SMOOTH_WINDOWS: usize = 5;
/// The key is released at the latest after this long.
pub const MAX_HOLD_SECONDS: f64 = 4.0;
/// The tail is recorded at most this long after the key is released.
pub const MAX_TAIL_SECONDS: f64 = 4.0;
/// How long the level must stay within [`SETTLED_DB`] before the note counts as sustaining.
const SETTLE_SECONDS: f64 = 0.3;
/// Largest swing, in dB, of a level that has settled.
const SETTLED_DB: f64 = 1.0;
/// A held level falling faster than this is still decaying rather than sustaining.
const DECAYING_DB_PER_SECOND: f64 = 3.0;
/// The dynamic range a decay or a release is measured across.
const RANGE_DB: f64 = 60.0;
/// Extrapolated times are capped here; a drone with no decay at all reports this.
pub const MAX_ESTIMATE_MS: f64 = 30_000.0;
/// Power floor, so digital silence has a finite level.
const FLOOR: f64 = 1e-14;

/// One note's amplitude envelope.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeasuredEnvelope {
    /// From the note becoming audible (peak − 40 dB) to reaching peak − 1 dB.
    pub attack_ms: f64,
    /// From the peak to settling within 1 dB of the sustain level. When the held note never
    /// settles (a piano, a pluck) this is instead the time from the peak to peak − 60 dB,
    /// extrapolated from the decay slope when the note was let go before getting there.
    pub decay_ms: f64,
    /// Level just before the key was released, relative to the peak (0 is no decay).
    pub sustain_db: f64,
    /// From the key's release to 60 dB below the level at release (an RT60).
    pub release_ms: f64,
    /// The held note was still falling when the key was released, so it has no sustain stage.
    pub decaying: bool,
    /// `decay_ms` was extrapolated from a slope rather than observed.
    pub decay_estimated: bool,
    /// `release_ms` was extrapolated from a slope rather than observed.
    pub release_estimated: bool,
}

impl MeasuredEnvelope {
    /// The per-field median of several measurements, for a summary across pitches.
    /// The flags are true when most of the inputs set them.
    pub fn median(values: &[Self]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let pick = |f: fn(&Self) -> f64| {
            let mut v: Vec<_> = values.iter().map(f).collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let most = |f: fn(&Self) -> bool| values.iter().filter(|v| f(v)).count() * 2 > values.len();
        Some(Self {
            attack_ms: pick(|v| v.attack_ms),
            decay_ms: pick(|v| v.decay_ms),
            sustain_db: pick(|v| v.sustain_db),
            release_ms: pick(|v| v.release_ms),
            decaying: most(|v| v.decaying),
            decay_estimated: most(|v| v.decay_estimated),
            release_estimated: most(|v| v.release_estimated),
        })
    }
}

/// Watches a note while it is rendered and decides when to release the key and when to stop.
///
/// Feed every rendered block to [`push`](Self::push) in order; after each, ask
/// [`should_release`](Self::should_release) while the key is held and
/// [`should_stop`](Self::should_stop) after it was released, then [`finish`](Self::finish).
#[derive(Clone, Debug)]
pub struct EnvelopeProbe {
    window: usize,
    pending: f64,
    pending_frames: usize,
    frames: usize,
    levels: Vec<f64>,
    released_at: Option<usize>,
}

impl EnvelopeProbe {
    /// A probe for audio at `sample_rate`.
    pub fn new(sample_rate: f64) -> Self {
        Self {
            window: (sample_rate * WINDOW_SECONDS).round().max(1.0) as usize,
            pending: 0.0,
            pending_frames: 0,
            frames: 0,
            levels: Vec::new(),
            released_at: None,
        }
    }

    /// Frames in one level window.
    pub fn window_frames(&self) -> usize {
        self.window
    }

    /// Adds rendered frames, given as one slice per channel of equal length.
    pub fn push(&mut self, channels: &[&[f32]]) {
        let frames = channels.first().map_or(0, |c| c.len());
        for i in 0..frames {
            self.pending += channels
                .iter()
                .map(|c| f64::from(c.get(i).copied().unwrap_or(0.0)).powi(2))
                .sum::<f64>();
            self.pending_frames += 1;
            if self.pending_frames == self.window {
                let power = self.pending / self.window as f64;
                self.levels.push(
                    10.0 * if power.is_finite() {
                        power.max(FLOOR)
                    } else {
                        FLOOR
                    }
                    .log10(),
                );
                self.pending = 0.0;
                self.pending_frames = 0;
            }
        }
        self.frames += frames;
    }

    /// Records that the key was released after every frame pushed so far.
    pub fn release(&mut self) {
        if self.released_at.is_none() {
            self.released_at = Some(self.levels.len());
        }
    }

    /// Frames pushed so far.
    pub fn frames(&self) -> usize {
        self.frames
    }

    fn seconds(&self, windows: usize) -> f64 {
        windows as f64 * WINDOW_SECONDS
    }

    /// Whether the held note has told everything it will: it settled, died, or ran out of time.
    pub fn should_release(&self) -> bool {
        let n = self.levels.len();
        if self.seconds(n) >= MAX_HOLD_SECONDS {
            return true;
        }
        let levels = smooth(&self.levels);
        let Some((peak_at, peak)) = argmax(&levels) else {
            return false;
        };
        if peak <= 10.0 * FLOOR.log10() + 1.0 {
            // Nothing has sounded yet; give a slow swell a whole second before giving up.
            return self.seconds(n) >= 1.0;
        }
        if levels[n - 1] <= peak - RANGE_DB {
            return true;
        }
        let settle = (SETTLE_SECONDS / WINDOW_SECONDS) as usize;
        if n < peak_at + settle || n < settle {
            return false;
        }
        let recent = &levels[n - settle..];
        let (low, high) = recent
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        high - low < SETTLED_DB
    }

    /// Whether the released tail has fallen 60 dB or the capture has run out of time.
    pub fn should_stop(&self) -> bool {
        let Some(off) = self.released_at else {
            return false;
        };
        let n = self.levels.len();
        if self.seconds(n - off) >= MAX_TAIL_SECONDS {
            return true;
        }
        let levels = smooth(&self.levels);
        let at_release = if off == 0 { levels[0] } else { levels[off - 1] };
        // Let the smoother see a few windows past the release before trusting a low level.
        n > off + SMOOTH_WINDOWS && levels[n - 1] <= at_release - RANGE_DB
    }

    /// Reads the envelope. `None` when the note was silent or the key was never released.
    pub fn finish(&self) -> Option<MeasuredEnvelope> {
        analyze(&self.levels, self.released_at?)
    }
}

fn smooth(levels: &[f64]) -> Vec<f64> {
    let half = SMOOTH_WINDOWS / 2;
    let mut scratch = Vec::with_capacity(SMOOTH_WINDOWS);
    (0..levels.len())
        .map(|i| {
            scratch.clear();
            scratch.extend_from_slice(
                &levels[i.saturating_sub(half)..(i + half + 1).min(levels.len())],
            );
            scratch.sort_by(f64::total_cmp);
            scratch[scratch.len() / 2]
        })
        .collect()
}

fn argmax(levels: &[f64]) -> Option<(usize, f64)> {
    levels
        .iter()
        .copied()
        .enumerate()
        .fold(None, |best, (i, v)| match best {
            Some((_, b)) if b >= v => best,
            _ => Some((i, v)),
        })
}

/// Least-squares slope of `levels` in dB per second.
fn slope(levels: &[f64]) -> Option<f64> {
    if levels.len() < 2 {
        return None;
    }
    let n = levels.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = levels.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, y) in levels.iter().enumerate() {
        let dx = i as f64 - mean_x;
        num += dx * (y - mean_y);
        den += dx * dx;
    }
    Some(num / den / WINDOW_SECONDS)
}

/// Time for a fall of `range` dB at `slope` dB/s, capped, in milliseconds.
fn extrapolate(range: f64, slope: Option<f64>) -> f64 {
    match slope {
        Some(s) if s < -1e-6 => (range / -s * 1000.0).min(MAX_ESTIMATE_MS),
        _ => MAX_ESTIMATE_MS,
    }
}

/// Measures the envelope from 5 ms levels in dB and the window index at which the key was
/// released. `None` when nothing sounded while the key was held.
fn analyze(raw: &[f64], off: usize) -> Option<MeasuredEnvelope> {
    let levels = smooth(raw);
    let off = off.min(levels.len());
    let held = &levels[..off];
    let (peak_at, peak) = argmax(held)?;
    if peak <= 10.0 * FLOOR.log10() + 1.0 {
        return None;
    }
    let ms = |windows: usize| windows as f64 * WINDOW_SECONDS * 1000.0;

    let onset = held.iter().position(|v| *v >= peak - 40.0).unwrap_or(0);
    let top = held
        .iter()
        .position(|v| *v >= peak - 1.0)
        .unwrap_or(peak_at);
    let attack_ms = ms(top.saturating_sub(onset));

    let settle = ((SETTLE_SECONDS / WINDOW_SECONDS) as usize)
        .min(off - peak_at)
        .max(1);
    let tail_of_hold = &held[off - settle..];
    let mut sorted = tail_of_hold.to_vec();
    sorted.sort_by(f64::total_cmp);
    let sustain_level = sorted[sorted.len() / 2];
    let sustain_db = (sustain_level - peak).clamp(-120.0, 0.0);
    let falling = slope(tail_of_hold).is_some_and(|s| s < -DECAYING_DB_PER_SECOND);
    let died = sustain_db <= -RANGE_DB;
    let decaying = falling || died;

    let (decay_ms, decay_estimated) = if decaying {
        match held[peak_at..].iter().position(|v| *v <= peak - RANGE_DB) {
            Some(i) => (ms(i), false),
            None => {
                // The later half of the fall is the note's own decay, not the attack's overshoot.
                let after = &held[peak_at..];
                (
                    extrapolate(RANGE_DB, slope(&after[after.len() / 2..])),
                    true,
                )
            }
        }
    } else {
        let settled = held[peak_at..]
            .iter()
            .position(|v| (v - sustain_level).abs() <= SETTLED_DB)
            .unwrap_or(0);
        (ms(settled), false)
    };

    let at_release = held[off - 1];
    let (release_ms, release_estimated) = if at_release <= peak - RANGE_DB {
        (0.0, false)
    } else {
        let tail = &levels[off..];
        match tail.iter().position(|v| *v <= at_release - RANGE_DB) {
            Some(i) => (ms(i), false),
            None => {
                // Skip the first 20 ms, where the smoother still remembers the held level.
                let fit = tail.get(4.min(tail.len())..).unwrap_or_default();
                (extrapolate(RANGE_DB, slope(fit)), true)
            }
        }
    };

    Some(MeasuredEnvelope {
        attack_ms,
        decay_ms,
        sustain_db,
        release_ms,
        decaying,
        decay_estimated,
        release_estimated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a sine through an exact linear-in-dB envelope, driving a probe as the session does.
    fn capture(
        attack: f64,
        decay_db_per_s: f64,
        sustain_db: f64,
        release: f64,
    ) -> MeasuredEnvelope {
        let rate = 16_000.0;
        let mut probe = EnvelopeProbe::new(rate);
        let block = 512;
        let mut t = 0usize;
        let mut released_at = None;
        let level_db = |time: f64, released: Option<f64>| -> f64 {
            if time < attack {
                // A linear amplitude ramp; no test releases the key during it.
                return 20.0 * (time / attack).max(1e-7).log10();
            }
            let held = (-(time - attack) * decay_db_per_s).max(sustain_db);
            match released {
                None => held,
                Some(off) => {
                    let at_off = (-(off - attack) * decay_db_per_s).max(sustain_db);
                    if release == 0.0 {
                        -200.0
                    } else {
                        at_off - (time - off) / release * 60.0
                    }
                }
            }
        };
        loop {
            let mut buf = vec![0.0f32; block];
            for (i, s) in buf.iter_mut().enumerate() {
                let time = (t + i) as f64 / rate;
                let db = level_db(time, released_at);
                let amp = 10f64.powf(db / 20.0) * 0.5;
                *s = (amp * (std::f64::consts::TAU * 440.0 * time).sin() * 2f64.sqrt()) as f32;
            }
            probe.push(&[&buf, &buf]);
            t += block;
            match released_at {
                None if probe.should_release() => {
                    probe.release();
                    released_at = Some(t as f64 / rate);
                }
                Some(_) if probe.should_stop() => break,
                _ => {}
            }
        }
        probe.finish().unwrap()
    }

    #[test]
    fn an_organ_holds_still_and_stops_at_once() {
        let e = capture(0.01, 0.0, 0.0, 0.0);
        assert!(e.attack_ms < 30.0, "{e:?}");
        assert!(!e.decaying, "{e:?}");
        assert!(e.sustain_db > -1.0, "{e:?}");
        assert!(e.release_ms < 30.0, "{e:?}");
        assert!(!e.release_estimated);
    }

    #[test]
    fn a_pad_swells_settles_and_rings_on() {
        let e = capture(0.5, 20.0, -6.0, 0.8);
        assert!((e.attack_ms - 445.0).abs() < 60.0, "{e:?}");
        assert!(!e.decaying, "{e:?}");
        assert!((e.sustain_db + 6.0).abs() < 1.0, "{e:?}");
        assert!((e.decay_ms - 300.0).abs() < 80.0, "{e:?}");
        assert!((e.release_ms - 800.0).abs() < 80.0, "{e:?}");
        assert!(!e.release_estimated, "{e:?}");
    }

    #[test]
    fn a_piano_never_sustains_and_its_decay_is_extrapolated() {
        // 10 dB/s: 6 s to fall 60 dB, longer than the four-second hold.
        let e = capture(0.003, 10.0, -200.0, 0.3);
        assert!(e.decaying, "{e:?}");
        assert!(e.decay_estimated, "{e:?}");
        assert!((e.decay_ms - 6000.0).abs() < 600.0, "{e:?}");
        assert!((e.sustain_db + 40.0).abs() < 2.0, "{e:?}");
        assert!((e.release_ms - 300.0).abs() < 60.0, "{e:?}");
    }

    #[test]
    fn a_pluck_dies_while_held_and_is_let_go_early() {
        // 200 dB/s: gone in 300 ms; the probe must not hold it for four seconds.
        let e = capture(0.002, 200.0, -200.0, 0.1);
        assert!(e.decaying && !e.decay_estimated, "{e:?}");
        assert!((e.decay_ms - 300.0).abs() < 40.0, "{e:?}");
        assert_eq!(e.release_ms, 0.0);
    }

    #[test]
    fn a_long_release_beyond_the_capture_is_estimated() {
        let e = capture(0.01, 0.0, 0.0, 8.0);
        assert!(e.release_estimated, "{e:?}");
        assert!((e.release_ms - 8000.0).abs() < 800.0, "{e:?}");
    }

    #[test]
    fn levels_do_not_depend_on_gain() {
        let mut quiet = EnvelopeProbe::new(8000.0);
        let mut loud = EnvelopeProbe::new(8000.0);
        let tone: Vec<f32> = (0..8000)
            .map(|i| (i as f32 * 0.3).sin() * (1.0 - i as f32 / 8000.0))
            .collect();
        let soft: Vec<f32> = tone.iter().map(|v| v * 0.01).collect();
        loud.push(&[&tone[..4000]]);
        quiet.push(&[&soft[..4000]]);
        loud.release();
        quiet.release();
        loud.push(&[&tone[4000..]]);
        quiet.push(&[&soft[4000..]]);
        let (a, b) = (loud.finish().unwrap(), quiet.finish().unwrap());
        assert!((a.sustain_db - b.sustain_db).abs() < 1e-6);
        assert!((a.attack_ms - b.attack_ms).abs() < 1e-6);
    }

    #[test]
    fn silence_has_no_envelope() {
        let mut probe = EnvelopeProbe::new(8000.0);
        probe.push(&[&[0.0; 4000]]);
        probe.release();
        probe.push(&[&[0.0; 4000]]);
        assert!(probe.finish().is_none());
        assert!(EnvelopeProbe::new(8000.0).finish().is_none());
    }

    #[test]
    fn the_median_summarises_pitches_field_by_field() {
        let base = capture(0.01, 0.0, 0.0, 0.2);
        let a = MeasuredEnvelope {
            attack_ms: 5.0,
            ..base
        };
        let b = MeasuredEnvelope {
            attack_ms: 50.0,
            release_estimated: true,
            ..base
        };
        let c = MeasuredEnvelope {
            attack_ms: 20.0,
            ..base
        };
        let m = MeasuredEnvelope::median(&[a, b, c]).unwrap();
        assert_eq!(m.attack_ms, 20.0);
        assert!(!m.release_estimated);
    }
}
