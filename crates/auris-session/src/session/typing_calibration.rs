//! Measuring how late the typing keyboard's notes arrive.
//!
//! A note recorded from the typing keyboard is placed where the music being *heard* was when the
//! key reached the session — see [`super::note_take`]. The output's own latency is already read
//! off its clock; what is not is everything in front of the session: the keyboard's scan, the
//! operating system, the window's event loop, and the player's own habit of pressing a shade
//! early or late. None of it can be asked for, so it is measured the way a drummer would be:
//! play a click, have them tap along, and see where the taps land.
//!
//! Three bars of four at 100 beats a minute, with the arrangement muted at the master as for the
//! input measurement. The first bar is for listening and its taps are not counted, because
//! nobody lands the first beat of a count they have only just heard begin. Each tap after that is
//! paired with the click nearest it, and the median of the distances is kept. The median rather
//! than the mean so that a tap that missed — a beat skipped, a key pressed twice — costs one
//! reading rather than shifting all of them.

use std::time::{Duration, Instant};

use auris_core::Ticks;
use auris_engine::{CountIn, EngineCommand};

use crate::error::SessionError;
use crate::settings::TypingCalibration;

use super::Session;
use super::calibrate::{BEAT_SECONDS, SILENT_DB, median};

/// How many beats are played: three bars of four.
const BEATS: u32 = 12;

/// How many beats are listened to before taps count.
const LISTEN_BEATS: u32 = 4;

/// How many taps have to land near a click for the measurement to count.
const FEWEST_TAPS: usize = 5;

/// How far the taps may stray from their median before the measurement is refused, in
/// milliseconds. Generous, because people are: forty milliseconds is a tap that a listener
/// would call sloppy, and a run of them says nothing steady about the keyboard.
const STEADIEST_SPREAD_MS: f64 = 40.0;

/// How long after the last beat taps are still listened for, in seconds.
const TAIL_SECONDS: f64 = 0.4;

/// How long past its own length the measurement is given before the output is taken as stalled.
const STALL_SECONDS: f64 = 3.0;

/// A tap-along under way.
pub(super) struct TypingCalibrationRun {
    count: CountIn,
    /// Where the playhead was, so the measurement leaves it there.
    playhead: Ticks,
    /// The virtual frame the count began at: the playhead less the whole count.
    first_click: f64,
    /// The delay compensation the clicks are heard ahead of the music by.
    compensation: f64,
    /// Where each tap landed, as heard virtual frames.
    taps: Vec<f64>,
    started: Instant,
    counted: Option<Instant>,
}

impl Session {
    /// Plays three bars of click and measures how late taps on the keyboard arrive.
    ///
    /// The person taps a key on every click from the second bar, and the frontend passes each
    /// tap to [`Self::tap_typing_calibration`]. The result arrives through
    /// [`Self::take_typing_calibration`] and is kept in the audio preferences.
    pub fn start_typing_calibration(&mut self) -> Result<(), SessionError> {
        if self.typing_calibration_run.is_some()
            || self.calibration.is_some()
            || self.is_recording()
            || self.is_playing()
        {
            return Err(SessionError::CalibrationBusy);
        }
        self.typing_calibration_result = None;
        let rate = self.engine.sample_rate().max(1.0);
        let beat_frames = (rate * BEAT_SECONDS).round().max(1.0) as u64;
        let count = CountIn::new(BEATS, beat_frames, 4);
        let playhead = self.engine.playhead_frames();
        self.engine.expect_count_in(count.remaining_frames);
        self.send(EngineCommand::SetMasterGain(SILENT_DB));
        self.counting = Some(count);
        self.send(EngineCommand::CountIn(count));
        self.play();
        self.typing_calibration_run = Some(TypingCalibrationRun {
            count,
            playhead: self.playhead(),
            first_click: playhead as f64 - count.total_frames() as f64,
            compensation: self.graph_latency as f64,
            taps: Vec::with_capacity(BEATS as usize * 2),
            started: Instant::now(),
            counted: None,
        });
        Ok(())
    }

    /// `true` while the click of a tap-along is playing.
    pub fn calibrating_typing(&self) -> bool {
        self.typing_calibration_run.is_some()
    }

    /// Writes down one tap of a tap-along. Does nothing when none is running.
    pub fn tap_typing_calibration(&mut self) {
        if self.typing_calibration_run.is_none() {
            return;
        }
        let at = self.heard_frame();
        if let Some(run) = self.typing_calibration_run.as_mut() {
            run.taps.push(at);
        }
    }

    /// The outcome of the last tap-along, once, after it has finished.
    ///
    /// A success has already been written into [`Self::audio_preferences`].
    pub fn take_typing_calibration(&mut self) -> Option<Result<TypingCalibration, SessionError>> {
        self.typing_calibration_result.take()
    }

    /// Abandons a tap-along, leaving whatever was measured before.
    pub fn cancel_typing_calibration(&mut self) {
        if let Some(run) = self.typing_calibration_run.take() {
            self.wind_up_tap_along();
            self.seek(run.playhead);
        }
    }

    /// Forgets the measured typing latency.
    pub fn clear_typing_calibration(&mut self) {
        self.audio.typing_calibration = None;
    }

    /// The measured typing latency in milliseconds, zero when nothing has been measured.
    pub(super) fn typing_lag_ms(&self) -> f64 {
        self.audio
            .typing_calibration
            .map_or(0.0, |calibration| calibration.offset_ms)
    }

    /// Finishes a tap-along whose click has played. Called from [`Self::poll`].
    pub(super) fn service_typing_calibration(&mut self) {
        let rolled = self.engine.count_in_frames() == 0 && self.is_playing();
        let rate = self.engine.sample_rate().max(1.0);
        let Some(run) = self.typing_calibration_run.as_mut() else {
            return;
        };
        let now = Instant::now();
        let length = Duration::from_secs_f64(run.count.total_frames() as f64 / rate);
        if run.counted.is_none() && rolled {
            run.counted = Some(now);
        }
        let stalled = match run.counted {
            Some(counted) if now.duration_since(counted).as_secs_f64() >= TAIL_SECONDS => false,
            Some(_) => return,
            None if now.duration_since(run.started)
                > length + Duration::from_secs_f64(STALL_SECONDS) =>
            {
                true
            }
            None => return,
        };
        let run = self.typing_calibration_run.take().expect("checked above");
        self.wind_up_tap_along();
        let result = if stalled {
            Err(SessionError::CalibrationStalled)
        } else {
            let clicks: Vec<f64> = (LISTEN_BEATS..BEATS)
                .map(|beat| {
                    run.first_click + f64::from(beat) * run.count.beat_frames as f64
                        - run.compensation
                })
                .collect();
            measure_taps(&run.taps, &clicks, run.count.beat_frames as f64, rate)
        };
        if let Ok(found) = result {
            self.audio.typing_calibration = Some(found);
        }
        self.typing_calibration_result = Some(result);
        self.seek(run.playhead);
    }

    /// Stops the click and puts back what it disturbed.
    fn wind_up_tap_along(&mut self) {
        self.stop();
        self.send(EngineCommand::SetMasterGain(self.project.master.gain_db));
    }
}

/// Pairs each tap with the click nearest it and measures how late they were.
///
/// `clicks` are where the counted clicks were heard and `taps` where the taps landed, both as
/// virtual frames; a tap more than half a beat from every click is not a tap at any of them.
pub(crate) fn measure_taps(
    taps: &[f64],
    clicks: &[f64],
    beat_frames: f64,
    rate: f64,
) -> Result<TypingCalibration, SessionError> {
    let mut late: Vec<f64> = Vec::with_capacity(clicks.len());
    for click in clicks {
        // The tap nearest this click, if one is within half a beat of it. One per click, so a
        // key pressed twice on one beat is one reading rather than two.
        let nearest = taps
            .iter()
            .map(|tap| tap - click)
            .filter(|distance| distance.abs() < beat_frames / 2.0)
            .min_by(|a, b| a.abs().total_cmp(&b.abs()));
        if let Some(distance) = nearest {
            late.push(distance);
        }
    }
    if late.len() < FEWEST_TAPS {
        return Err(SessionError::CalibrationHeardNothing);
    }
    let offset = median(&mut late.clone()).expect("taps were found");
    let spread = median(
        &mut late
            .iter()
            .map(|late| (late - offset).abs())
            .collect::<Vec<_>>(),
    )
    .expect("taps were found");
    let to_ms = 1000.0 / rate.max(1.0);
    let spread_ms = spread * to_ms;
    if spread_ms > STEADIEST_SPREAD_MS {
        return Err(SessionError::CalibrationUnsteady { spread_ms });
    }
    Ok(TypingCalibration {
        offset_ms: offset * to_ms,
        spread_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    const RATE: f64 = 48_000.0;
    const BEAT: f64 = 28_800.0;

    fn clicks() -> Vec<f64> {
        (0..8)
            .map(|beat| 10_000.0 + f64::from(beat) * BEAT)
            .collect()
    }

    #[test]
    fn taps_that_arrive_late_measure_as_that_late() {
        // Thirty milliseconds behind every click, give or take a millisecond of human.
        let jitter = [-48.0, 0.0, 48.0, 24.0, -24.0, 0.0, 12.0, -12.0];
        let taps: Vec<f64> = clicks()
            .iter()
            .zip(jitter)
            .map(|(click, jitter)| click + 1_440.0 + jitter)
            .collect();
        let found = measure_taps(&taps, &clicks(), BEAT, RATE).unwrap();
        assert!((found.offset_ms - 30.0).abs() < 0.5, "{}", found.offset_ms);
        assert!(found.spread_ms < 1.0, "{}", found.spread_ms);
    }

    #[test]
    fn a_double_tap_and_a_missed_beat_cost_one_reading_each() {
        let mut taps: Vec<f64> = clicks().iter().map(|click| click + 960.0).collect();
        taps.remove(3);
        taps.push(clicks()[5] + 4_000.0);
        let found = measure_taps(&taps, &clicks(), BEAT, RATE).unwrap();
        assert!((found.offset_ms - 20.0).abs() < 0.5, "{}", found.offset_ms);
    }

    #[test]
    fn taps_ahead_of_the_click_measure_as_early() {
        let taps: Vec<f64> = clicks().iter().map(|click| click - 480.0).collect();
        let found = measure_taps(&taps, &clicks(), BEAT, RATE).unwrap();
        assert!((found.offset_ms + 10.0).abs() < 0.5);
    }

    #[test]
    fn too_few_taps_are_not_a_measurement() {
        let taps = [clicks()[0], clicks()[1]];
        assert!(matches!(
            measure_taps(&taps, &clicks(), BEAT, RATE),
            Err(SessionError::CalibrationHeardNothing)
        ));
    }

    #[test]
    fn taps_all_over_the_beat_are_refused() {
        let offsets = [
            0.0, 9_000.0, -9_000.0, 4_000.0, -6_000.0, 12_000.0, -3_000.0, 7_000.0,
        ];
        let taps: Vec<f64> = clicks()
            .iter()
            .zip(offsets)
            .map(|(click, offset)| click + offset)
            .collect();
        assert!(matches!(
            measure_taps(&taps, &clicks(), BEAT, RATE),
            Err(SessionError::CalibrationUnsteady { .. })
        ));
    }

    #[test]
    fn a_tap_along_runs_and_can_be_abandoned() {
        let mut session = session();
        session.start_typing_calibration().unwrap();
        assert!(session.calibrating_typing());
        assert!(matches!(
            session.start_typing_calibration(),
            Err(SessionError::CalibrationBusy)
        ));
        session.tap_typing_calibration();
        session.cancel_typing_calibration();
        assert!(!session.calibrating_typing());
        assert!(session.take_typing_calibration().is_none());
    }

    #[test]
    fn a_measured_typing_latency_moves_recorded_notes_earlier() {
        let mut session = session();
        let before = session.performed_frame();
        session.audio.typing_calibration = Some(TypingCalibration {
            offset_ms: 10.0,
            spread_ms: 1.0,
        });
        let after = session.performed_frame();
        let rate = session.engine().sample_rate();
        assert!((before - after - rate / 100.0).abs() < 1.0);
    }
}
