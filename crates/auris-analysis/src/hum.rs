//! Following a voice as it sings, and writing down the notes it meant.
//!
//! [`pitch`](crate::pitch) measures one note somebody has already cut out. This follows a
//! microphone as the samples arrive, a frame every ten milliseconds, and decides where notes
//! begin and end: what a person humming a tune means by it, as [`HumEvent`]s a note take can
//! write down the way it writes down a key going down and coming up.
//!
//! # A frame
//!
//! The input is low-passed and decimated to about 16 kHz first. A voice's fundamental is under
//! [`VOICE_HIGHEST_HZ`], and YIN's cost is the square of the longest period in samples, so at
//! 48 kHz the same search would cost nine times as much for nothing. Each frame is then the
//! [`crate::pitch`] YIN over a window one longest period long, and the frame's level in dBFS.
//!
//! # From frames to notes
//!
//! A sung pitch wanders — vibrato, scoops into a note, drift — and the semitone it is nearest to
//! flickers with it. So a note is not a frame's semitone but a decision made with hysteresis:
//!
//! * **A note begins** once [`CONFIRM`] voiced frames in a row agree with one another to within
//!   [`AGREE`] of a semitone. It begins where the first of them was, not where the agreement was
//!   reached, so being sure costs latency in deciding and none in placing.
//! * **It holds** while the voice stays within [`HOLD`] of it: further than half a semitone, so a
//!   vibrato does not split it, and closer than a whole one, so a step does.
//! * **It moves** when the voice has left and [`CONFIRM`] frames agree on somewhere else: one note
//!   ends and the next begins at the first of those frames, which is a legato step.
//! * **It is struck again** when the level climbs [`RESTRIKE_DB`] out of a dip at least that deep,
//!   which is how "dum dum dum" on one pitch is three notes rather than one long one.
//! * **It ends** after [`RELEASE`] frames with no pitch, where the first of them was.
//!
//! How hard the note was struck is the loudest of the frames that began it, mapped from
//! [`VELOCITY_FLOOR_DB`]..[`VELOCITY_CEIL_DB`] onto a velocity. That is a level at the
//! microphone, so it depends on the microphone and how far away it is; what it keeps is which
//! notes were sung louder than which.

use crate::pitch::period;

/// The lowest fundamental followed: C2, under a bass voice's comfortable bottom.
pub const VOICE_LOWEST_HZ: f32 = 65.0;

/// The highest fundamental followed: C6, over a soprano's comfortable top.
pub const VOICE_HIGHEST_HZ: f32 = 1_050.0;

/// The rate the input is decimated to, roughly; the factor is a whole number.
const ANALYSIS_RATE: f64 = 16_000.0;

/// Seconds between frames.
const HOP_SECONDS: f64 = 0.01;

/// The YIN threshold. Looser than the one for an instrument's note: a hummed voice is breathier.
const THRESHOLD: f32 = 0.2;

/// Frames quieter than this, in dBFS, have no pitch however periodic they are: room tone,
/// a fan, the speakers bleeding in at a distance.
pub const GATE_DB: f32 = -50.0;

/// Voiced frames that have to agree before a note begins or moves.
pub const CONFIRM: usize = 4;

/// How closely, in semitones either side of their median, the frames that begin a note agree.
pub const AGREE: f32 = 0.6;

/// How far, in semitones, the voice may stray from a note before it counts as leaving.
pub const HOLD: f32 = 0.8;

/// Unvoiced frames in a row that end a note.
pub const RELEASE: usize = 5;

/// How far, in decibels, the level has to fall and climb again to strike the note again.
pub const RESTRIKE_DB: f32 = 9.0;

/// The level, in dBFS, a note is struck at its softest.
pub const VELOCITY_FLOOR_DB: f32 = -48.0;

/// The level, in dBFS, a note is struck at its hardest.
pub const VELOCITY_CEIL_DB: f32 = -6.0;

/// The softest velocity a note is given: one that was heard at all was not silent.
const VELOCITY_LEAST: f32 = 0.05;

/// A note beginning or ending, at a frame of the input counted from the first sample pushed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HumEvent {
    /// A note began.
    On {
        /// MIDI pitch.
        pitch: u8,
        /// How hard, 0..1.
        velocity: f32,
        /// The input frame it began at.
        frame: u64,
    },
    /// A note ended.
    Off {
        /// MIDI pitch, the same as its [`HumEvent::On`].
        pitch: u8,
        /// The input frame it ended at.
        frame: u64,
    },
}

/// What one analysis frame heard.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// The input frame the window starts at, which is where a note is placed when this frame
    /// begins or ends it.
    ///
    /// A window is read as voiced while the voice fills most of it, so the first voiced one
    /// starts where the voice did, and the first unvoiced one starts within a few milliseconds of
    /// where it stopped — later or earlier by the pitch, since how much of a window a voice must
    /// fill for YIN to find it depends on how many periods fit.
    start: u64,
    /// The input frame at the centre of the window.
    at: u64,
    /// The pitch in fractional MIDI semitones, when it had one.
    semitone: Option<f32>,
    /// The level, in dBFS.
    db: f32,
}

/// Where the note decision stands.
#[derive(Clone, Debug)]
enum State {
    /// Nothing sounding; voiced frames waiting to agree.
    Idle { run: Vec<Frame> },
    /// A note sounding.
    Sounding {
        pitch: u8,
        /// Voiced frames somewhere else, waiting to agree on it.
        away: Vec<Frame>,
        /// The first unvoiced frame of the current gap, and how many there have been.
        quiet: Option<(u64, usize)>,
        /// The loudest level since the note was struck or last dipped.
        peak: f32,
        /// The quietest level since that peak, and where it was.
        trough: (f32, u64),
    },
}

/// A second-order low-pass section (RBJ), run before decimating.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn low_pass(cutoff: f64, rate: f64) -> Self {
        let w = std::f64::consts::TAU * cutoff / rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Self {
            b: [(b1 / 2.0) as f32, b1 as f32, (b1 / 2.0) as f32],
            a: [(-2.0 * cos / a0) as f32, ((1.0 - alpha) / a0) as f32],
            z: [0.0; 2],
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        // Transposed direct form II.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Follows a mono voice, block by block, and reports the notes it sang.
///
/// Not for the realtime thread: it allocates as its buffer grows and costs a fraction of a
/// millisecond a frame. A worker draining a capture is where it belongs.
#[derive(Clone, Debug)]
pub struct HumTracker {
    /// Input frames per analysis sample.
    factor: usize,
    /// Analysis samples a second.
    analysis_rate: f32,
    /// The anti-alias filter, two sections, unused when `factor` is one.
    filter: [Biquad; 2],
    /// Input samples seen since the last one kept, modulo `factor`.
    phase: usize,
    /// Decimated samples not yet behind every frame.
    buffer: Vec<f32>,
    /// The analysis index of `buffer[0]`.
    base: u64,
    /// The analysis index the next frame starts at.
    next: u64,
    /// Lags searched, in analysis samples.
    lags: (usize, usize),
    /// The window, in analysis samples.
    window: usize,
    /// Analysis samples between frames.
    hop: usize,
    difference: Vec<f32>,
    normalised: Vec<f32>,
    state: State,
}

impl HumTracker {
    /// A tracker for input at `rate` frames a second.
    pub fn new(rate: f64) -> Self {
        let rate = rate.max(1.0);
        let factor = ((rate / ANALYSIS_RATE).round() as usize).max(1);
        let analysis = rate / factor as f64;
        let analysis_rate = analysis as f32;
        let lag_min = ((analysis_rate / (VOICE_HIGHEST_HZ * 1.06)).floor() as usize).max(2);
        let lag_max = ((analysis_rate / VOICE_LOWEST_HZ).ceil() as usize).max(lag_min + 2);
        Self {
            factor,
            analysis_rate,
            filter: [Biquad::low_pass(0.35 * analysis, rate); 2],
            phase: 0,
            buffer: Vec::new(),
            base: 0,
            next: 0,
            lags: (lag_min, lag_max),
            window: lag_max,
            hop: ((analysis * HOP_SECONDS).round() as usize).max(1),
            difference: vec![0.0; lag_max + 1],
            normalised: vec![1.0; lag_max + 1],
            state: State::Idle { run: Vec::new() },
        }
    }

    /// Input frames from a sample going in to the frame it is centred in: how long the tracker
    /// hears before it can measure, not counting the [`CONFIRM`] frames a decision waits for.
    pub fn frame_frames(&self) -> u64 {
        ((self.window + self.lags.1) * self.factor) as u64
    }

    /// Feeds the next mono samples, and appends whatever notes they decided to `out`.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<HumEvent>) {
        for &sample in samples {
            if self.factor > 1 {
                let once = self.filter[0].run(sample);
                let twice = self.filter[1].run(once);
                if self.phase == 0 {
                    self.buffer.push(twice);
                }
                self.phase = (self.phase + 1) % self.factor;
            } else {
                self.buffer.push(sample);
            }
        }
        let span = self.window + self.lags.1;
        while self.next + span as u64 <= self.base + self.buffer.len() as u64 {
            let frame = self.measure((self.next - self.base) as usize, span);
            self.decide(frame, out);
            self.next += self.hop as u64;
        }
        // Keep only what a frame still to come will read.
        let behind = ((self.next - self.base) as usize).min(self.buffer.len());
        self.buffer.drain(..behind);
        self.base += behind as u64;
    }

    /// Ends whatever is still sounding where the input stopped.
    pub fn finish(&mut self, out: &mut Vec<HumEvent>) {
        let end = (self.base + self.buffer.len() as u64) * self.factor as u64;
        if let State::Sounding { pitch, .. } = self.state {
            out.push(HumEvent::Off { pitch, frame: end });
        }
        self.state = State::Idle { run: Vec::new() };
    }

    /// Measures the frame starting at `buffer[from]`, `span` samples long.
    fn measure(&mut self, from: usize, span: usize) -> Frame {
        let x = &self.buffer[from..from + span];
        let energy = x.iter().map(|s| s * s).sum::<f32>() / span as f32;
        let db = 10.0 * energy.max(1e-12).log10();
        let semitone = (db > GATE_DB)
            .then(|| {
                period(
                    x,
                    self.window,
                    self.lags,
                    THRESHOLD,
                    &mut self.difference,
                    &mut self.normalised,
                )
            })
            .flatten()
            .map(|period| 69.0 + 12.0 * (self.analysis_rate / period / 440.0).log2());
        Frame {
            start: self.next * self.factor as u64,
            at: (self.next + span as u64 / 2) * self.factor as u64,
            semitone,
            db,
        }
    }

    /// Moves the note decision on by one frame.
    fn decide(&mut self, frame: Frame, out: &mut Vec<HumEvent>) {
        match &mut self.state {
            State::Idle { run } => {
                if frame.semitone.is_none() {
                    run.clear();
                    return;
                }
                run.push(frame);
                agree(run);
                if run.len() >= CONFIRM {
                    let pitch = nearest(median(run));
                    let onset = run[0].start;
                    let loudest = run.iter().map(|f| f.db).fold(f32::MIN, f32::max);
                    out.push(HumEvent::On {
                        pitch,
                        velocity: velocity(loudest),
                        frame: onset,
                    });
                    self.state = sounding(pitch, frame);
                }
            }
            State::Sounding {
                pitch,
                away,
                quiet,
                peak,
                trough,
            } => {
                let pitch = *pitch;
                let Some(semitone) = frame.semitone else {
                    away.clear();
                    let (first, count) = quiet.get_or_insert((frame.start, 0));
                    *count += 1;
                    if *count >= RELEASE {
                        out.push(HumEvent::Off {
                            pitch,
                            frame: *first,
                        });
                        self.state = State::Idle { run: Vec::new() };
                        return;
                    }
                    if frame.db < trough.0 {
                        *trough = (frame.db, frame.at);
                    }
                    return;
                };
                // Struck again: out of a dip and back up, deep enough both ways.
                if *peak - trough.0 >= RESTRIKE_DB && frame.db - trough.0 >= RESTRIKE_DB {
                    let ended = quiet.map_or(trough.1, |(first, _)| first);
                    out.push(HumEvent::Off {
                        pitch,
                        frame: ended.min(frame.start),
                    });
                    let again = if (semitone - pitch as f32).abs() <= HOLD {
                        pitch
                    } else {
                        nearest(semitone)
                    };
                    out.push(HumEvent::On {
                        pitch: again,
                        velocity: velocity(frame.db),
                        frame: frame.start,
                    });
                    self.state = sounding(again, frame);
                    return;
                }
                *quiet = None;
                if frame.db > *peak {
                    *peak = frame.db;
                    *trough = (frame.db, frame.at);
                } else if frame.db < trough.0 {
                    *trough = (frame.db, frame.at);
                }
                if (semitone - pitch as f32).abs() <= HOLD {
                    away.clear();
                    return;
                }
                away.push(frame);
                agree(away);
                if away.len() >= CONFIRM {
                    let moved = nearest(median(away));
                    if moved != pitch {
                        let at = away[0].start;
                        let loudest = away.iter().map(|f| f.db).fold(f32::MIN, f32::max);
                        out.push(HumEvent::Off { pitch, frame: at });
                        out.push(HumEvent::On {
                            pitch: moved,
                            velocity: velocity(loudest),
                            frame: at,
                        });
                        self.state = sounding(moved, frame);
                    } else {
                        away.clear();
                    }
                }
            }
        }
    }
}

/// A note just struck at `pitch` by `frame`.
fn sounding(pitch: u8, frame: Frame) -> State {
    State::Sounding {
        pitch,
        away: Vec::new(),
        quiet: None,
        peak: frame.db,
        trough: (frame.db, frame.at),
    }
}

/// Drops frames off the front of `run` until every one is within [`AGREE`] of their median.
fn agree(run: &mut Vec<Frame>) {
    while run.len() > 1 {
        let middle = median(run);
        let all = run
            .iter()
            .all(|f| f.semitone.is_some_and(|s| (s - middle).abs() <= AGREE));
        if all {
            return;
        }
        run.remove(0);
    }
}

/// The median semitone of voiced frames.
fn median(run: &[Frame]) -> f32 {
    let mut semitones: Vec<f32> = run.iter().filter_map(|f| f.semitone).collect();
    semitones.sort_by(f32::total_cmp);
    match semitones.len() {
        0 => 0.0,
        n if n % 2 == 1 => semitones[n / 2],
        n => 0.5 * (semitones[n / 2 - 1] + semitones[n / 2]),
    }
}

/// The MIDI note nearest a fractional semitone.
fn nearest(semitone: f32) -> u8 {
    semitone.round().clamp(0.0, 127.0) as u8
}

/// A level in dBFS as a velocity.
pub fn velocity(db: f32) -> f32 {
    ((db - VELOCITY_FLOOR_DB) / (VELOCITY_CEIL_DB - VELOCITY_FLOOR_DB)).clamp(VELOCITY_LEAST, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sung note of a synthetic voice: harmonics falling off like a hum, a vibrato, a
    /// level, and a short fade at either end the way a voice starts and stops.
    #[derive(Clone, Copy)]
    struct Sung {
        pitch: f32,
        seconds: f64,
        level: f32,
        vibrato_cents: f32,
    }

    fn note(pitch: f32, seconds: f64) -> Sung {
        Sung {
            pitch,
            seconds,
            level: 0.2,
            vibrato_cents: 0.0,
        }
    }

    fn rest(seconds: f64) -> Sung {
        Sung {
            pitch: 0.0,
            seconds,
            level: 0.0,
            vibrato_cents: 0.0,
        }
    }

    /// Renders a phrase, phase-continuous across notes so a legato step has no click in it, with
    /// a little noise underneath, as a microphone in a room would have.
    fn sing(phrase: &[Sung], rate: f64) -> Vec<f32> {
        let mut out = Vec::new();
        let mut phase = 0.0f64;
        let mut noise = 0x1234_5678u32;
        let fade = (0.015 * rate) as usize;
        for (index, sung) in phrase.iter().enumerate() {
            let frames = (sung.seconds * rate) as usize;
            let legato_in = index > 0 && phrase[index - 1].level > 0.0;
            let legato_out = phrase.get(index + 1).is_some_and(|next| next.level > 0.0);
            for i in 0..frames {
                let t = i as f64 / rate;
                let cents = sung.vibrato_cents as f64 * (std::f64::consts::TAU * 5.5 * t).sin();
                let hz = 440.0 * 2f64.powf((sung.pitch as f64 - 69.0 + cents / 100.0) / 12.0);
                phase += hz / rate;
                let mut envelope = 1.0f32;
                if !legato_in && i < fade {
                    envelope = i as f32 / fade as f32;
                }
                if !legato_out && frames - i < fade {
                    envelope = envelope.min((frames - i) as f32 / fade as f32);
                }
                let voice: f64 = (1..=8)
                    .map(|k| (std::f64::consts::TAU * phase * k as f64).sin() / k as f64)
                    .sum();
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let hiss = (noise as f32 / u32::MAX as f32 - 0.5) * 0.002;
                out.push(sung.level * envelope * voice as f32 * 0.6 + hiss);
            }
        }
        out
    }

    fn track(samples: &[f32], rate: f64, chunk: usize) -> Vec<HumEvent> {
        let mut tracker = HumTracker::new(rate);
        let mut events = Vec::new();
        for block in samples.chunks(chunk) {
            tracker.push(block, &mut events);
        }
        tracker.finish(&mut events);
        events
    }

    /// Notes as (pitch, velocity, start seconds, end seconds).
    fn notes(events: &[HumEvent], rate: f64) -> Vec<(u8, f32, f64, f64)> {
        let mut open: Option<(u8, f32, u64)> = None;
        let mut out = Vec::new();
        for event in events {
            match *event {
                HumEvent::On {
                    pitch,
                    velocity,
                    frame,
                } => {
                    assert!(open.is_none(), "two notes at once: {events:?}");
                    open = Some((pitch, velocity, frame));
                }
                HumEvent::Off { pitch, frame } => {
                    let (on_pitch, velocity, start) = open.take().expect("an off with no on");
                    assert_eq!(on_pitch, pitch);
                    assert!(frame >= start);
                    out.push((pitch, velocity, start as f64 / rate, frame as f64 / rate));
                }
            }
        }
        assert!(open.is_none());
        out
    }

    #[test]
    fn a_hummed_scale_with_gaps_is_five_notes_where_they_were_sung() {
        let rate = 48_000.0;
        let mut phrase = vec![rest(0.2)];
        let pitches = [60.0, 62.0, 64.0, 65.0, 67.0];
        for pitch in pitches {
            phrase.push(note(pitch, 0.3));
            phrase.push(rest(0.1));
        }
        let found = notes(&track(&sing(&phrase, rate), rate, 480), rate);
        let pitches_found: Vec<u8> = found.iter().map(|n| n.0).collect();
        assert_eq!(pitches_found, vec![60, 62, 64, 65, 67], "{found:?}");
        for (index, &(_, _, start, end)) in found.iter().enumerate() {
            let sung = 0.2 + index as f64 * 0.4;
            assert!((start - sung).abs() < 0.012, "{index}: {start} vs {sung}");
            assert!(
                (end - (sung + 0.3)).abs() < 0.015,
                "{index}: {end} vs {}",
                sung + 0.3
            );
        }
    }

    #[test]
    fn a_legato_step_ends_one_note_where_the_next_begins() {
        let rate = 44_100.0;
        let phrase = [rest(0.1), note(57.0, 0.4), note(61.0, 0.4), rest(0.2)];
        let found = notes(&track(&sing(&phrase, rate), rate, 512), rate);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].0, found[1].0), (57, 61));
        assert!((found[1].2 - 0.5).abs() < 0.012, "{found:?}");
        assert_eq!(found[0].3, found[1].2);
    }

    #[test]
    fn a_vibrato_of_half_a_semitone_is_one_note() {
        let rate = 48_000.0;
        let mut sung = note(64.0, 1.2);
        sung.vibrato_cents = 50.0;
        let found = notes(
            &track(&sing(&[rest(0.1), sung, rest(0.2)], rate), rate, 256),
            rate,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, 64);
    }

    #[test]
    fn a_louder_note_is_struck_harder() {
        let rate = 48_000.0;
        let mut soft = note(62.0, 0.3);
        soft.level = 0.03;
        let loud = note(62.0, 0.3);
        let found = notes(
            &track(
                &sing(&[rest(0.1), soft, rest(0.15), loud, rest(0.1)], rate),
                rate,
                480,
            ),
            rate,
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[1].1 > found[0].1 + 0.2, "{found:?}");
        assert!(found[0].1 > VELOCITY_LEAST, "{found:?}");
    }

    #[test]
    fn a_dip_in_level_on_one_pitch_strikes_the_note_again() {
        // "Dum-dum" without a breath between: the level falls by 20 dB and comes back.
        let rate = 48_000.0;
        let mut dip = note(55.0, 0.05);
        dip.level = 0.02;
        let found = notes(
            &track(
                &sing(
                    &[rest(0.1), note(55.0, 0.3), dip, note(55.0, 0.3), rest(0.1)],
                    rate,
                ),
                rate,
                480,
            ),
            rate,
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|n| n.0 == 55));
        assert!((found[1].2 - 0.45).abs() < 0.04, "{found:?}");
    }

    #[test]
    fn a_low_hum_is_not_read_an_octave_up() {
        // G2, where a bass hums: twenty-four analysis samples of the longest lag's 247.
        let rate = 48_000.0;
        let found = notes(
            &track(
                &sing(&[rest(0.1), note(43.0, 0.5), rest(0.1)], rate),
                rate,
                480,
            ),
            rate,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, 43);
    }

    #[test]
    fn silence_and_room_noise_write_nothing() {
        let rate = 48_000.0;
        assert!(track(&sing(&[rest(1.0)], rate), rate, 480).is_empty());
    }

    #[test]
    fn how_the_input_is_cut_into_blocks_changes_nothing() {
        let rate = 48_000.0;
        let phrase = [rest(0.1), note(60.0, 0.3), note(67.0, 0.3), rest(0.1)];
        let samples = sing(&phrase, rate);
        let whole = track(&samples, rate, samples.len());
        for chunk in [1, 64, 333, 1024] {
            assert_eq!(track(&samples, rate, chunk), whole, "{chunk}");
        }
    }

    #[test]
    fn a_note_still_sounding_ends_where_the_input_did() {
        let rate = 48_000.0;
        let samples = sing(&[rest(0.1), note(70.0, 0.5)], rate);
        let found = notes(&track(&samples, rate, 480), rate);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].3 - 0.6).abs() < 0.03, "{found:?}");
    }
}
