//! Measuring how late a recording really arrives.
//!
//! A take is moved back by the round trip the devices *report* — see `auris_engine::clock` — and
//! a report is only as good as the driver making it. It knows its own buffers; it does not know
//! about a USB interface's converters, a Bluetooth headset's radio, or a speaker on the far side
//! of the room. What is left over is a few milliseconds on a good interface and a great deal more
//! on anything wireless, and it is the same every time for the same devices. So it is measured
//! once and kept, per arrangement of devices, in
//! [`AudioPreferences::input_calibrations`](crate::AudioPreferences::input_calibrations).
//!
//! # How
//!
//! By recording a count-in. A count-in is already the one thing the engine plays whose timing is
//! known to the frame while the timeline stands still, and a take that begins during one is
//! already stamped with where each of its clicks belongs — see [`super::record`]. So the
//! measurement *is* a take, of eight clicks and nothing else, written into memory instead of a
//! file: the output plays them, the input hears them, through a cable from the output to the
//! input or a microphone in front of the speaker, and each click is found in what came back.
//!
//! Where each click *should* be in that recording, if the reported round trip were the whole
//! truth, follows from the stamp; where it actually is, [`measure_clicks`] finds. The median of
//! the differences is the correction, and the median distance from it says whether the
//! measurement can be believed. A microphone in the air adds about three milliseconds a metre,
//! which is the price of not needing a cable and less than anybody can hear.
//!
//! The arrangement is muted at the master for the length of it, past which the metronome is
//! mixed, so a count that rolls on into the song for the instant before it is stopped plays
//! nothing of it — and nothing a monitored input feeds back can ring round a loopback cable.

use std::time::{Duration, Instant};

use auris_core::Ticks;
use auris_engine::{CaptureReader, CountIn, EngineCommand};

use crate::error::SessionError;
use crate::settings::InputCalibration;

use super::Session;

/// How many clicks are played.
///
/// Enough that two spoiled by a cough still leave a median of six.
const CLICKS: u32 = 8;

/// How long one beat of the measurement lasts, in seconds: 100 beats a minute.
///
/// Long enough that a click's echo in an ordinary room has died before the next one starts, and
/// that the search window around each click can run well past any latency a device has.
pub(super) const BEAT_SECONDS: f64 = 0.6;

/// How far before its expected place a click is searched for, as a fraction of a beat.
///
/// Before, because a driver can overstate its own latency; 180 milliseconds of it would be a
/// driver that has nothing true to say.
const SEARCH_BEFORE: f64 = 0.3;

/// How far after its expected place a click is searched for, as a fraction of a beat: 420
/// milliseconds, past any Bluetooth headset there is.
const SEARCH_AFTER: f64 = 0.7;

/// How long the input goes on listening after the count has run out, in seconds.
///
/// The last click's search window runs past the end of the count by most of a beat.
const TAIL_SECONDS: f64 = 0.5;

/// How long past its own length the measurement is given before the output is taken as stalled.
const STALL_SECONDS: f64 = 3.0;

/// The fraction of a click's own peak at which it is taken to have begun.
///
/// A click starts at full amplitude on a sine starting from zero, so a quarter of its peak is
/// reached within a twentieth of a millisecond of the true start, and a quarter is still well
/// clear of whatever the previous click left ringing in the room.
const ONSET_FRACTION: f32 = 0.25;

/// The quietest a click may arrive and still be counted: −60 dBFS.
const QUIETEST_CLICK: f32 = 0.001;

/// How many times the input's own background a click has to stand above.
const ABOVE_BACKGROUND: f32 = 4.0;

/// How many clicks have to be found for the measurement to count.
const FEWEST_CLICKS: usize = 5;

/// How far the found clicks may stray from their median before the measurement is refused, in
/// milliseconds.
const STEADIEST_SPREAD_MS: f64 = 2.0;

/// The master level the arrangement is held at while the clicks play, in decibels.
pub(super) const SILENT_DB: f32 = -200.0;

/// A measurement under way.
pub(super) struct Calibration {
    /// The thread copying what the input hears into memory.
    collector: std::thread::JoinHandle<(CaptureReader, Vec<f32>)>,
    /// What was counted.
    count: CountIn,
    /// Where the playhead was, so the measurement leaves it there.
    playhead: Ticks,
    /// When the count was asked for, for noticing an output that never plays it.
    started: Instant,
    /// When the count was seen to have run out.
    counted: Option<Instant>,
    /// The devices it is being measured on.
    devices: InputCalibration,
}

/// Where the clicks should be in a recording, in the recording's own frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClickTrain {
    /// Where the first click lands if the reported round trip is the whole truth.
    pub first: f64,
    /// The distance from one click to the next.
    pub spacing: f64,
    /// How many there are.
    pub count: usize,
}

/// What [`measure_clicks`] found, in the recording's own frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClickMeasurement {
    /// How much later than expected the clicks arrived: the median.
    pub offset: f64,
    /// The median distance of a click from that.
    pub spread: f64,
    /// How many clicks were found.
    pub found: usize,
}

impl Session {
    /// Plays eight clicks and measures how late the input hears them.
    ///
    /// The result arrives through [`Self::take_input_calibration`] once the clicks have been
    /// played, about five seconds later, and is kept in the audio preferences for the devices
    /// open now. The input has to be able to hear the output: a cable from one to the other, or
    /// the microphone in front of the speakers rather than headphones.
    ///
    /// Refused while the transport is moving or a take is running, because the measurement is
    /// both of those things itself.
    pub fn start_input_calibration(&mut self) -> Result<(), SessionError> {
        if self.calibration.is_some()
            || self.typing_calibration_run.is_some()
            || self.is_recording()
            || self.is_playing()
        {
            return Err(SessionError::CalibrationBusy);
        }
        self.calibration_result = None;
        if self.input.is_none() {
            self.open_input()?;
        }
        let Some(capture) = self.input.as_mut() else {
            return Err(auris_engine::EngineError::NoInputDevice.into());
        };
        let Some(reader) = capture.take_reader() else {
            return Err(SessionError::CalibrationBusy);
        };
        let seconds = (f64::from(CLICKS) * BEAT_SECONDS + TAIL_SECONDS + STALL_SECONDS).ceil();
        let capacity = (reader.sample_rate() * seconds) as usize;
        let collector = match std::thread::Builder::new()
            .name("auris-calibrate".to_string())
            .spawn(move || collect(reader, capacity))
        {
            Ok(collector) => collector,
            Err(error) => {
                self.close_input_if_idle();
                return Err(SessionError::AudioRestart(error.to_string()));
            }
        };

        let beat_frames = (self.engine.sample_rate() * BEAT_SECONDS).round().max(1.0) as u64;
        let count = CountIn::new(CLICKS, beat_frames, 4);
        // The same order a take uses, for the same reasons: the count written down before the
        // take opens, the take open before the transport is told to count.
        self.engine.expect_count_in(count.remaining_frames);
        self.send(EngineCommand::SetMasterGain(SILENT_DB));
        if let Some(capture) = self.input.as_ref() {
            capture.begin_take();
        }
        self.counting = Some(count);
        self.send(EngineCommand::CountIn(count));
        self.play();
        self.calibration = Some(Calibration {
            collector,
            count,
            playhead: self.playhead(),
            started: Instant::now(),
            counted: None,
            devices: InputCalibration {
                host: self.audio.host.clone(),
                output: self.audio.device.clone(),
                input: self.audio.input_device.clone(),
                block_frames: self.audio.block_frames,
                offset_ms: 0.0,
                spread_ms: 0.0,
            },
        });
        Ok(())
    }

    /// `true` while the clicks of a measurement are being played and listened for.
    pub fn calibrating_input(&self) -> bool {
        self.calibration.is_some()
    }

    /// The outcome of the last measurement, once, after it has finished.
    ///
    /// A success has already been written into [`Self::audio_preferences`]; a frontend that keeps
    /// those on disk saves them when this returns one.
    pub fn take_input_calibration(&mut self) -> Option<Result<InputCalibration, SessionError>> {
        self.calibration_result.take()
    }

    /// Abandons a measurement under way, leaving whatever was measured before.
    pub fn cancel_input_calibration(&mut self) {
        if let Some(calibration) = self.calibration.take() {
            let _ = self.wind_up_calibration(calibration);
        }
    }

    /// Forgets the measured correction for the devices open now.
    pub fn clear_input_calibration(&mut self) {
        self.audio.clear_input_calibration();
    }

    /// Finishes a measurement whose clicks have all been played. Called from [`Self::poll`].
    pub(super) fn service_calibration(&mut self) {
        let rolled = self.engine.count_in_frames() == 0 && self.is_playing();
        let rate = self.engine.sample_rate().max(1.0);
        let Some(calibration) = self.calibration.as_mut() else {
            return;
        };
        let now = Instant::now();
        let length = Duration::from_secs_f64(calibration.count.total_frames() as f64 / rate);
        if calibration.counted.is_none() && rolled {
            calibration.counted = Some(now);
        }
        let outcome = match calibration.counted {
            Some(counted) if now.duration_since(counted).as_secs_f64() >= TAIL_SECONDS => None,
            Some(_) => return,
            None if now.duration_since(calibration.started)
                > length + Duration::from_secs_f64(STALL_SECONDS) =>
            {
                Some(SessionError::CalibrationStalled)
            }
            None => return,
        };
        let calibration = self.calibration.take().expect("checked above");
        let result = match (outcome, self.wind_up_calibration(calibration)) {
            (Some(error), _) | (None, Err(error)) => Err(error),
            (None, Ok(found)) => {
                self.audio.set_input_calibration(found.clone());
                Ok(found)
            }
        };
        self.calibration_result = Some(result);
    }

    /// Stops the clicks, puts back what they disturbed and measures what was heard.
    fn wind_up_calibration(
        &mut self,
        calibration: Calibration,
    ) -> Result<InputCalibration, SessionError> {
        self.stop();
        self.send(EngineCommand::SetMasterGain(self.project.master.gain_db));
        let seen = self.input.as_ref().map(|capture| {
            let seen = (
                capture.started_at(),
                capture.count_in_at_start(),
                capture.latency_at_start(),
                capture.sample_rate(),
            );
            capture.end_take();
            seen
        });
        let heard = match calibration.collector.join() {
            Ok((reader, heard)) => {
                if let Some(capture) = self.input.as_mut() {
                    capture.restore_reader(reader);
                }
                heard
            }
            Err(_) => Vec::new(),
        };
        self.close_input_if_idle();
        self.seek(calibration.playhead);

        let Some((Some(_), count_at_start, lag, input_rate)) = seen else {
            return Err(SessionError::CalibrationStalled);
        };
        // Where the first click is in the recording if the reported round trip is the truth: the
        // recording opened `count_at_start` frames before the count's end, the first click was
        // played the whole count before it, and what was played is heard `lag` later than the
        // stamp says. The clock that lag is read against is the *music's*, held back by the
        // graph's compensation, and the click is not held back — so it is heard that much
        // sooner than its position says. All in engine frames, then in the input's.
        let engine_rate = self.engine.sample_rate().max(1.0);
        let to_input = input_rate / engine_rate;
        let count = calibration.count;
        let first = (count_at_start as f64 - count.total_frames() as f64 + lag as f64
            - self.graph_latency as f64)
            * to_input;
        let train = ClickTrain {
            first,
            spacing: count.beat_frames as f64 * to_input,
            count: count.beats as usize,
        };
        let measured = measure_clicks(&heard, train)?;
        let to_ms = 1000.0 / input_rate.max(1.0);
        Ok(InputCalibration {
            offset_ms: measured.offset * to_ms,
            spread_ms: measured.spread * to_ms,
            ..calibration.devices
        })
    }
}

/// Copies what the input hears into memory until the take is stopped, as one loudness a frame.
///
/// The loudest channel of each frame, so a cable into the second input of a stereo interface is
/// heard as well as one into the first.
fn collect(mut reader: CaptureReader, capacity: usize) -> (CaptureReader, Vec<f32>) {
    let channels = reader.channel_count().max(1);
    let mut heard = Vec::with_capacity(capacity);
    let mut quiet = 0;
    loop {
        let samples = reader.drain(|block| {
            heard.extend(block.chunks(channels).map(|frame| {
                frame
                    .iter()
                    .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()))
            }));
        });
        if reader.is_finished() {
            break;
        }
        // The same two quiet passes a take's writer waits for, for the same reason.
        if !reader.is_recording() {
            quiet = if samples == 0 { quiet + 1 } else { 0 };
            if quiet >= 2 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    (reader, heard)
}

/// Finds each click of `train` in `heard` and measures how late they arrived.
///
/// `heard` is the loudness of each frame of the recording. Each click is searched for from a
/// little before where it was expected to most of a beat after, and found where the first frame
/// reaches [`ONSET_FRACTION`] of the loudest one in that window. A window whose loudest frame is
/// not a click — too quiet, or not clear of the recording's own background — is passed over
/// rather than counted, so a click that a door slammed over costs one reading rather than the
/// measurement.
pub(crate) fn measure_clicks(
    heard: &[f32],
    train: ClickTrain,
) -> Result<ClickMeasurement, SessionError> {
    let background = median(
        &mut heard
            .iter()
            .map(|&level| f64::from(level))
            .collect::<Vec<_>>(),
    )
    .unwrap_or(0.0) as f32;
    let floor = QUIETEST_CLICK.max(background * ABOVE_BACKGROUND);
    let mut late = Vec::with_capacity(train.count);
    for click in 0..train.count {
        let expected = train.first + click as f64 * train.spacing;
        let from = (expected - SEARCH_BEFORE * train.spacing).max(0.0) as usize;
        let to = ((expected + SEARCH_AFTER * train.spacing).max(0.0) as usize).min(heard.len());
        let Some(window) = heard.get(from..to) else {
            continue;
        };
        let peak = window.iter().copied().fold(0.0f32, f32::max);
        if peak < floor {
            continue;
        }
        let threshold = peak * ONSET_FRACTION;
        if let Some(onset) = window.iter().position(|&level| level >= threshold) {
            late.push((from + onset) as f64 - expected);
        }
    }
    if late.len() < FEWEST_CLICKS {
        return Err(SessionError::CalibrationHeardNothing);
    }
    let found = late.len();
    let offset = median(&mut late.clone()).expect("at least one click");
    let spread = median(
        &mut late
            .iter()
            .map(|late| (late - offset).abs())
            .collect::<Vec<_>>(),
    )
    .expect("at least one click");
    // The train is a beat apart, so a beat is what turns its frames back into time.
    let frames_per_ms = (train.spacing / (BEAT_SECONDS * 1000.0)).max(f64::MIN_POSITIVE);
    let spread_ms = spread / frames_per_ms;
    if spread_ms > STEADIEST_SPREAD_MS {
        return Err(SessionError::CalibrationUnsteady { spread_ms });
    }
    Ok(ClickMeasurement {
        offset,
        spread,
        found,
    })
}

pub(super) fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// A tenth of a millisecond. The onset is found where a click reaches a quarter of its peak,
    /// which an 800 Hz sine starting from zero does two and a half frames in.
    const TOLERANCE: f64 = 5.0;

    /// A recording of `train`'s clicks arriving `late` frames after they were expected, each a
    /// decaying 800 Hz burst like the metronome's, over a background of `noise`.
    fn recording(train: ClickTrain, late: &[f64], noise: f32) -> Vec<f32> {
        let length = (train.first + train.spacing * (train.count as f64 + 1.0)) as usize;
        let mut heard = vec![0.0f32; length];
        let mut seed = 0x2545_f491_u32;
        for level in heard.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *level = (seed as f32 / u32::MAX as f32) * noise;
        }
        for (click, late) in late.iter().enumerate() {
            let start = (train.first + click as f64 * train.spacing + late).round() as usize;
            for frame in 0..(0.04 * RATE) as usize {
                let t = frame as f64 / RATE;
                let level =
                    (0.2 * (2.0 * std::f64::consts::PI * 800.0 * t).sin() * (-t * 170.0).exp())
                        .abs() as f32;
                if let Some(slot) = heard.get_mut(start + frame) {
                    *slot = slot.max(level);
                }
            }
        }
        heard
    }

    fn train() -> ClickTrain {
        ClickTrain {
            first: 1_520.0,
            spacing: BEAT_SECONDS * RATE,
            count: 8,
        }
    }

    #[test]
    fn clicks_arriving_late_by_a_fixed_amount_measure_as_that_amount() {
        // 9.5 ms past what the devices reported: a USB interface's converters.
        let late = [456.0; 8];
        let measured = measure_clicks(&recording(train(), &late, 0.0005), train()).unwrap();
        assert_eq!(measured.found, 8);
        assert!(
            (measured.offset - 456.0).abs() <= TOLERANCE,
            "{} frames",
            measured.offset
        );
        assert!(measured.spread <= 1.0, "{} frames", measured.spread);
    }

    #[test]
    fn a_report_that_overstates_the_latency_measures_as_early() {
        let late = [-240.0; 8];
        let measured = measure_clicks(&recording(train(), &late, 0.0), train()).unwrap();
        assert!(
            (measured.offset + 240.0).abs() <= TOLERANCE,
            "{}",
            measured.offset
        );
    }

    #[test]
    fn a_bluetooth_sized_delay_is_still_inside_the_search() {
        // 250 ms, which is what a headset adds and no driver reports.
        let late = [12_000.0; 8];
        let measured = measure_clicks(&recording(train(), &late, 0.0), train()).unwrap();
        assert!(
            (measured.offset - 12_000.0).abs() <= TOLERANCE,
            "{}",
            measured.offset
        );
    }

    #[test]
    fn two_missing_clicks_still_leave_a_measurement() {
        let mut heard = recording(train(), &[480.0; 8], 0.0);
        // The third and the sixth drowned out by nothing at all: the input dropped them.
        for click in [2usize, 5] {
            let start = (train().first + click as f64 * train().spacing) as usize;
            heard[start..start + 4_000].fill(0.0);
        }
        let measured = measure_clicks(&heard, train()).unwrap();
        assert_eq!(measured.found, 6);
        assert!((measured.offset - 480.0).abs() <= TOLERANCE);
    }

    #[test]
    fn silence_is_heard_as_nothing_rather_than_as_a_latency() {
        let heard = vec![0.0001f32; (RATE * 6.0) as usize];
        assert!(matches!(
            measure_clicks(&heard, train()),
            Err(SessionError::CalibrationHeardNothing)
        ));
    }

    #[test]
    fn clicks_that_wander_are_refused_rather_than_averaged() {
        let late = [0.0, 960.0, 120.0, 720.0, 240.0, 1_200.0, 480.0, 1_440.0];
        assert!(matches!(
            measure_clicks(&recording(train(), &late, 0.0), train()),
            Err(SessionError::CalibrationUnsteady { .. })
        ));
    }

    #[test]
    fn a_headless_session_has_no_input_to_measure_with() {
        let mut session =
            Session::new(crate::SessionOptions::headless()).expect("a headless session");
        assert!(session.start_input_calibration().is_err());
        assert!(!session.calibrating_input());
    }
}
