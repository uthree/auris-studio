//! The click, and how it finds the beats.
//!
//! A metronome is not a track. It plays nothing the document holds, it is never exported, and it
//! must be audible when every strip in the project is muted — so it is a voice the renderer mixes
//! into the master bus *after* the master strip has had its say, past the fader, past the mute and
//! past the meters. Turning it on cannot change one sample of what a bounce contains.
//!
//! # Where the beats are
//!
//! Nowhere, until they are asked for. [`clicks_in_block`] converts the block being rendered into
//! ticks, walks the beats inside that range, and converts each one back to a frame — which is a
//! handful of walks over a few tempo points per block and no state at all. A list built when the
//! graph was, in the way clip notes are, would have to stop somewhere, and the timeline does not:
//! playing past the end of an arrangement, or practising over an empty project, is exactly when a
//! person wants the click and exactly where a precomputed list would fall silent.
//!
//! It also means a loop needs no special handling. The renderer never lets a segment straddle the
//! loop end, so every segment covers one contiguous stretch of ticks, and the segment after a wrap
//! asks about the loop start the same way any other asks about anywhere.
//!
//! # Counting somebody in
//!
//! A count-in is the one thing here that does not come off the timeline at all. The playhead is
//! held still for the length of it — see [`Transport::count_in`] — so there is nothing to walk;
//! [`count_in_clicks`] counts beats of a fixed length from the moment the count began. That is
//! what lets bar one be counted in, where a click read off the timeline would have to run
//! backwards past the start of the song to find its beats.
//!
//! It also sounds whether or not the click is switched on, which is the other half of the same
//! thought: somebody who wanted the click for the take would have turned the click on.
//!
//! # Which beat
//!
//! The *felt* beat — [`TimeSignature::beat_ticks`] — so a bar of 6/8 is counted in two dotted
//! quarters rather than in six eighths. That is the whole reason the distinction exists in
//! [`auris_core::time`], and a metronome is the one place where getting it wrong is audible on the
//! first bar.

use auris_core::AudioBuffer;
use auris_core::time::{Samples, SignatureMap, TempoMap, Ticks, TimeSignature};

use crate::transport::{CountIn, Transport};

/// Pitch of the click on a bar line, in hertz.
const ACCENT_HZ: f64 = 1_600.0;

/// Pitch of the click on every other beat.
///
/// An octave below the accent rather than a shade off it. The two have to be told apart while
/// something else is playing over them, and an octave is the interval a listener never has to
/// think about.
const BEAT_HZ: f64 = 800.0;

/// Peak level of the click on a bar line.
///
/// −12 dBFS, and the downbeat is the loudest thing the metronome ever produces. It sits over a mix
/// rather than in it, so it is placed where it can be heard against material that has been mastered
/// and not where it would win against material that has not been.
const ACCENT_AMPLITUDE: f32 = 0.25;

/// Peak level of the click on every other beat, four decibels under the accent.
const BEAT_AMPLITUDE: f32 = 0.16;

/// How long one click lasts.
///
/// Short enough to be a click rather than a note, long enough that the pitch is heard as a pitch —
/// forty milliseconds is thirty-two cycles of the beat tone.
const CLICK_SECONDS: f64 = 0.04;

/// How far the click's envelope falls over [`CLICK_SECONDS`], as an amplitude ratio.
///
/// Sixty decibels, so what is left when the count runs out is below anything an ear or a meter
/// will find, and the end of the click needs no ramp of its own to avoid a step.
const CLICK_DECAY: f32 = 0.001;

/// How many clicks one block is allowed to hold.
///
/// A block is at most a few thousand frames and a beat at the fastest tempo the tempo map accepts
/// is a good deal longer than that, so two is already generous and eight can only be reached by a
/// caller asking for a block far larger than the graph was prepared for. It is a cap on a
/// pre-allocated buffer rather than a limit anybody meets: what it buys is that the walk below
/// cannot allocate, whatever it is handed.
const MAX_CLICKS: usize = 8;

/// One click a block has to sound.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Click {
    /// Where it lands, in frames from the start of the block.
    pub frame: usize,
    /// Whether it falls on a bar line rather than on a beat inside one.
    pub accent: bool,
}

/// The clicks falling inside a block of `frames` starting at `position`.
///
/// `out` is emptied first and never grown, so this allocates nothing as long as it was built with
/// `MAX_CLICKS` of capacity — which [`Metronome`] does.
///
/// The range is half-open, exactly as the note scheduler's is: a beat landing on the first frame of
/// a block belongs to that block and a beat landing on the frame just past its end belongs to the
/// next. Both edges are converted with the same tempo map, so a beat cannot be counted twice or
/// dropped between two consecutive blocks.
pub fn clicks_in_block(
    signatures: &SignatureMap,
    tempo_map: &TempoMap,
    position: u64,
    frames: usize,
    sample_rate: f64,
    out: &mut Vec<Click>,
) {
    out.clear();
    if frames == 0 {
        return;
    }
    let start = tempo_map.samples_to_ticks(Samples(position), sample_rate);
    let end = tempo_map.samples_to_ticks(Samples(position + frames as u64), sample_rate);

    let mut tick = first_beat_from(signatures, start);
    while tick < end {
        if out.len() == MAX_CLICKS {
            return;
        }
        let frame = tempo_map.ticks_to_samples(tick, sample_rate).raw();
        // Clamped rather than trusted. The two conversions are not exact inverses of each other
        // once a tempo point sits inside the block, and a beat that came back one frame outside it
        // would index past the end of the buffer.
        let offset = frame.saturating_sub(position).min(frames as u64 - 1) as usize;
        let bar = signatures.bar_floor(tick);
        out.push(Click {
            frame: offset,
            accent: bar == tick,
        });
        tick += beat_of(signatures.signature_at(bar));
    }
}

/// The clicks of a count-in falling inside the next `frames`.
///
/// Nothing here reads the timeline. A count-in is a fixed number of beats of a fixed length —
/// [`CountIn`] says why — so the beats are `count.beat_frames` apart from the moment the count
/// began, and where that moment was is [`CountIn::elapsed_frames`]. The first beat lands on the
/// first frame of the count and the last one beat before the transport rolls, which is what makes
/// "one, two, three, four" arrive with the downbeat on the *next* bar rather than on the fourth.
///
/// `out` is emptied first and never grown, exactly as [`clicks_in_block`] leaves it.
pub fn count_in_clicks(count: CountIn, frames: usize, out: &mut Vec<Click>) {
    out.clear();
    if frames == 0 || count.beat_frames == 0 {
        return;
    }
    let from = count.elapsed_frames();
    let to = from + frames as u64;
    let mut beat = from.div_ceil(count.beat_frames);
    while beat < count.beats as u64 && beat * count.beat_frames < to {
        if out.len() == MAX_CLICKS {
            return;
        }
        out.push(Click {
            frame: (beat * count.beat_frames - from) as usize,
            accent: beat.is_multiple_of(count.per_bar.max(1) as u64),
        });
        beat += 1;
    }
}

/// The first beat at or after `tick`.
fn first_beat_from(signatures: &SignatureMap, tick: Ticks) -> Ticks {
    let tick = tick.max_zero();
    let bar = signatures.bar_floor(tick);
    let beat = beat_of(signatures.signature_at(bar)).raw();
    // Measured from the bar line rather than from tick zero, so a meter change part way through
    // the song does not leave every beat after it counted from the wrong place.
    match (tick.raw() - bar.raw()) % beat {
        0 => tick,
        into => tick + Ticks(beat - into),
    }
}

/// The felt beat of a signature, never zero.
fn beat_of(signature: TimeSignature) -> Ticks {
    Ticks(signature.beat_ticks().raw().max(1))
}

/// The click voice, and the bar lines it accents.
///
/// Holds the signature map because the tempo map — which the graph already carries — says how long
/// a beat *lasts* and never how many of them make a bar. Only this needs to know which beat is the
/// strong one, so only this carries the map.
pub struct Metronome {
    /// Whether a click is heard at all.
    enabled: bool,
    /// Where the bar lines are.
    signatures: SignatureMap,
    /// The click currently sounding, if one is.
    voice: Voice,
    /// Where the clicks of the block being rendered fall, reused so the walk never allocates.
    block: Vec<Click>,
}

/// A sine with an exponential envelope: one click, from the moment it is struck.
#[derive(Copy, Clone, Debug, Default)]
struct Voice {
    /// Frames left to sound. Zero when nothing is.
    remaining: usize,
    /// How far round the sine the voice has got, in radians.
    ///
    /// Struck at zero, where a sine is zero, so the onset is continuous and the click does not
    /// arrive with a click of its own.
    phase: f64,
    /// How far the phase moves each frame.
    step: f64,
    /// Where the envelope currently sits.
    level: f32,
    /// What the envelope is multiplied by each frame.
    decay: f32,
}

impl Metronome {
    /// A metronome for a project's meter, switched off.
    pub fn new(signatures: SignatureMap) -> Self {
        Self {
            enabled: false,
            signatures,
            voice: Voice::default(),
            block: Vec::with_capacity(MAX_CLICKS),
        }
    }

    /// Whether the click is heard.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turns the click on or off.
    ///
    /// Whatever is sounding is left to finish. A click cut off mid-envelope is a step to silence,
    /// which is a louder noise than the forty milliseconds it saves.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Silences the click at once, for a panic.
    pub fn reset(&mut self) {
        self.voice = Voice::default();
    }

    /// Adds this block's clicks to `out`, which is the mix bus past the master strip.
    ///
    /// Struck only while the transport rolls — a metronome is for playing along to — but a click
    /// already sounding when the transport stops is allowed to ring out, for the same reason
    /// switching the metronome off does not cut one short.
    ///
    /// A count-in is the exception to every part of that. It sounds whether or not the click is
    /// switched on, because counting somebody in is what they asked for when they set it and not
    /// a side effect of the click being on; and it is counted from the count itself rather than
    /// from the timeline, because the playhead does not move until it is over.
    pub fn render(
        &mut self,
        out: &mut AudioBuffer,
        transport: &Transport,
        tempo_map: &TempoMap,
        sample_rate: f64,
    ) {
        let frames = out.frame_count();
        let counting = transport.count_in.filter(|_| transport.playing);
        if frames == 0 || (!self.enabled && self.voice.remaining == 0 && counting.is_none()) {
            return;
        }
        if let Some(count) = counting {
            count_in_clicks(count, frames, &mut self.block);
        } else if self.enabled && transport.rolling() {
            clicks_in_block(
                &self.signatures,
                tempo_map,
                transport.position_frames,
                frames,
                sample_rate,
                &mut self.block,
            );
        } else {
            self.block.clear();
        }

        // Rendered in pieces so that two clicks inside one block both land where they belong.
        let mut cursor = 0;
        for index in 0..self.block.len() {
            let click = self.block[index];
            self.sound(out, cursor, click.frame);
            self.strike(click.accent, sample_rate);
            cursor = click.frame;
        }
        self.sound(out, cursor, frames);
    }

    /// Starts a click.
    fn strike(&mut self, accent: bool, sample_rate: f64) {
        let frames = (CLICK_SECONDS * sample_rate).round().max(1.0) as usize;
        let (hz, amplitude) = match accent {
            true => (ACCENT_HZ, ACCENT_AMPLITUDE),
            false => (BEAT_HZ, BEAT_AMPLITUDE),
        };
        self.voice = Voice {
            remaining: frames,
            phase: 0.0,
            step: std::f64::consts::TAU * hz / sample_rate.max(1.0),
            level: amplitude,
            decay: CLICK_DECAY.powf(1.0 / frames as f32),
        };
    }

    /// Adds `from..to` of whatever is sounding into every channel of `out`.
    ///
    /// Each channel is written from the same starting point and the voice is advanced once at the
    /// end, so the click is the same in both — a metronome that moved with a pan control would be
    /// one nobody could rely on.
    fn sound(&mut self, out: &mut AudioBuffer, from: usize, to: usize) {
        let voice = &mut self.voice;
        let count = voice.remaining.min(to.saturating_sub(from));
        if count == 0 {
            return;
        }
        for channel in out.channels_mut() {
            let (mut phase, mut level) = (voice.phase, voice.level);
            for sample in &mut channel[from..from + count] {
                *sample += level * phase.sin() as f32;
                phase += voice.step;
                level *= voice.decay;
            }
        }
        voice.phase += voice.step * count as f64;
        voice.level *= voice.decay.powi(count as i32);
        voice.remaining -= count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::SignaturePoint;

    const RATE: f64 = 48_000.0;

    /// At 120 BPM a quarter note is half a second: 24 000 frames.
    const BEAT_FRAMES: u64 = 24_000;

    /// A transport at `position`, rolling or not, with no count-in in front of it.
    fn at(position: u64, playing: bool) -> Transport {
        Transport {
            playing,
            position_frames: position,
            ..Transport::new()
        }
    }

    fn counting(count: CountIn) -> Transport {
        let mut transport = at(0, true);
        transport.set_count_in(count);
        transport
    }

    fn count_clicks(count: CountIn, frames: usize) -> Vec<Click> {
        let mut out = Vec::with_capacity(MAX_CLICKS);
        count_in_clicks(count, frames, &mut out);
        out
    }

    fn clicks(signatures: &SignatureMap, position: u64, frames: usize) -> Vec<Click> {
        let mut out = Vec::with_capacity(MAX_CLICKS);
        clicks_in_block(
            signatures,
            &TempoMap::constant(120.0),
            position,
            frames,
            RATE,
            &mut out,
        );
        out
    }

    #[test]
    fn the_downbeat_is_the_accent_and_the_rest_are_not() {
        let four_four = SignatureMap::default();
        // A whole bar at 120 BPM: four beats, the first of them the bar line.
        let bar = clicks(&four_four, 0, BEAT_FRAMES as usize * 4);
        assert_eq!(
            bar,
            vec![
                Click {
                    frame: 0,
                    accent: true
                },
                Click {
                    frame: 24_000,
                    accent: false
                },
                Click {
                    frame: 48_000,
                    accent: false
                },
                Click {
                    frame: 72_000,
                    accent: false
                },
            ]
        );
        // And the next bar line accents again.
        let next = clicks(&four_four, BEAT_FRAMES * 4, 512);
        assert_eq!(
            next,
            vec![Click {
                frame: 0,
                accent: true
            }]
        );
    }

    #[test]
    fn a_beat_is_counted_once_across_two_touching_blocks() {
        // The half-open range is the whole of this: a beat on the first frame of a block belongs
        // to that block, and the block before it — which ends on that frame — must not have
        // claimed it as well.
        let four_four = SignatureMap::default();
        let before = clicks(&four_four, BEAT_FRAMES - 512, 512);
        assert!(before.is_empty(), "{before:?}");
        let after = clicks(&four_four, BEAT_FRAMES, 512);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].frame, 0);
    }

    #[test]
    fn a_block_starting_mid_beat_waits_for_the_next_one() {
        let four_four = SignatureMap::default();
        let straddling = clicks(&four_four, BEAT_FRAMES - 100, 512);
        assert_eq!(straddling.len(), 1);
        assert_eq!(straddling[0].frame, 100);
        assert!(!straddling[0].accent);
    }

    #[test]
    fn a_compound_bar_is_counted_in_the_beats_it_is_felt_in() {
        // Six eighths in a bar and two clicks in it, not six. Written in eighths, felt in dotted
        // quarters — which is the distinction `beat_ticks` exists for and the one place a
        // metronome getting it wrong is audible on the first bar.
        let six_eight = SignatureMap::constant(TimeSignature::new(6, 8));
        let bar_frames = 3 * BEAT_FRAMES as usize; // six eighths = three quarters
        let bar = clicks(&six_eight, 0, bar_frames);
        assert_eq!(bar.len(), 2, "{bar:?}");
        assert!(bar[0].accent);
        assert!(!bar[1].accent);
        assert_eq!(bar[1].frame, bar_frames / 2);
    }

    #[test]
    fn a_meter_change_moves_the_accents_and_nothing_else() {
        // Two bars of 4/4 then 3/4 from bar three. The beats stay a quarter apart throughout; what
        // moves is which of them is the strong one.
        let signatures = SignatureMap::from_points(vec![
            SignaturePoint {
                tick: Ticks::ZERO,
                signature: TimeSignature::new(4, 4),
            },
            SignaturePoint {
                tick: Ticks::QUARTER * 8,
                signature: TimeSignature::new(3, 4),
            },
        ]);
        // From bar three onwards: three-beat bars.
        let after = clicks(&signatures, BEAT_FRAMES * 8, BEAT_FRAMES as usize * 6);
        let accents: Vec<usize> = after
            .iter()
            .filter(|click| click.accent)
            .map(|click| click.frame)
            .collect();
        assert_eq!(after.len(), 6);
        assert_eq!(
            accents,
            vec![0, BEAT_FRAMES as usize * 3],
            "the bar lines did not follow the meter"
        );
    }

    #[test]
    fn nothing_is_counted_in_a_block_of_no_length() {
        assert!(clicks(&SignatureMap::default(), 0, 0).is_empty());
    }

    #[test]
    fn a_stopped_transport_is_silent_and_a_ringing_click_still_finishes() {
        let mut metronome = Metronome::new(SignatureMap::default());
        metronome.set_enabled(true);
        let tempo = TempoMap::constant(120.0);
        let mut out = AudioBuffer::new(2, 512, RATE);

        // Stopped from the start: nothing at all, and no state left behind.
        metronome.render(&mut out, &at(0, false), &tempo, RATE);
        assert_eq!(out.peak(), 0.0);

        // Rolling from the downbeat: the accent strikes.
        out.clear();
        metronome.render(&mut out, &at(0, true), &tempo, RATE);
        assert!(out.peak() > 0.1, "got {}", out.peak());

        // The transport stops mid-click. What is already sounding rings out rather than stepping
        // to silence, which would be a louder noise than the click it cut short.
        out.clear();
        metronome.render(&mut out, &at(512, false), &tempo, RATE);
        assert!(out.peak() > 0.0, "the click was cut off at the stop");
    }

    #[test]
    fn a_click_never_leaves_the_level_it_was_struck_at() {
        // The accent is the loudest thing a metronome produces, and it is mixed in past the master
        // fader — so if it were hotter than this, nothing downstream could turn it down.
        let mut metronome = Metronome::new(SignatureMap::default());
        metronome.set_enabled(true);
        let tempo = TempoMap::constant(120.0);
        let mut out = AudioBuffer::new(2, BEAT_FRAMES as usize * 4, RATE);
        metronome.render(&mut out, &at(0, true), &tempo, RATE);
        assert!(
            out.peak() <= ACCENT_AMPLITUDE + 1e-6,
            "the click peaked at {}",
            out.peak()
        );
        assert!(out.peak() > BEAT_AMPLITUDE);
        // Both channels get the same click: a metronome does not sit anywhere in the image.
        assert_eq!(out.channel(0), out.channel(1));
    }

    #[test]
    fn the_click_dies_away_instead_of_stopping() {
        // A voice that ended at whatever level it had reached would step to silence, which is a
        // discontinuity — the one noise a click is supposed not to make.
        let mut metronome = Metronome::new(SignatureMap::default());
        metronome.set_enabled(true);
        let tempo = TempoMap::constant(120.0);
        let frames = (CLICK_SECONDS * RATE) as usize + 16;
        let mut out = AudioBuffer::new(2, frames, RATE);
        metronome.render(&mut out, &at(0, true), &tempo, RATE);

        let tail = out.channel(0)[frames - 20..frames - 16]
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(
            tail < ACCENT_AMPLITUDE * CLICK_DECAY * 4.0,
            "the click was still at {tail} when it ran out"
        );
    }

    #[test]
    fn a_switched_off_metronome_costs_nothing_and_adds_nothing() {
        let mut metronome = Metronome::new(SignatureMap::default());
        let tempo = TempoMap::constant(120.0);
        let mut out = AudioBuffer::new(2, 512, RATE);
        metronome.render(&mut out, &at(0, true), &tempo, RATE);
        assert_eq!(out.peak(), 0.0);
        assert!(!metronome.is_enabled());
    }

    #[test]
    fn two_clicks_in_one_block_each_land_where_they_belong() {
        // A block longer than a beat — which an offline render asks for routinely — must not
        // collapse into one click at the end of it.
        let mut metronome = Metronome::new(SignatureMap::default());
        metronome.set_enabled(true);
        let tempo = TempoMap::constant(120.0);
        let mut out = AudioBuffer::new(2, BEAT_FRAMES as usize * 2, RATE);
        metronome.render(&mut out, &at(0, true), &tempo, RATE);

        let struck = |at: usize| out.channel(0)[at..at + 64].iter().any(|s| s.abs() > 0.01);
        assert!(struck(0), "no click on the downbeat");
        assert!(struck(BEAT_FRAMES as usize), "no click on the second beat");
        // And silence between them, where the envelope has long since run out.
        assert!(!struck(BEAT_FRAMES as usize / 2));
    }

    #[test]
    fn a_count_in_is_one_click_a_beat_ending_before_the_downbeat() {
        // Two bars of four at 120: eight clicks, the first of each bar accented, and the eighth
        // one beat before the transport rolls rather than on the downbeat itself.
        let count = CountIn::new(8, BEAT_FRAMES, 4);
        let all = count_clicks(count, count.total_frames() as usize);
        assert_eq!(all.len(), 8);
        assert_eq!(all[0].frame, 0);
        assert_eq!(all[7].frame, BEAT_FRAMES as usize * 7);
        let accents: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, click)| click.accent)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(accents, vec![0, 4]);
    }

    #[test]
    fn a_count_in_beat_is_claimed_by_exactly_one_block() {
        let mut count = CountIn::new(4, BEAT_FRAMES, 4);
        // The block ending on the second beat does not claim it...
        count.remaining_frames = count.total_frames() - (BEAT_FRAMES - 512);
        assert!(count_clicks(count, 512).is_empty());
        // ...and the block beginning on it does.
        count.remaining_frames = count.total_frames() - BEAT_FRAMES;
        let struck = count_clicks(count, 512);
        assert_eq!(struck.len(), 1);
        assert_eq!(struck[0].frame, 0);
        assert!(!struck[0].accent);
    }

    #[test]
    fn nothing_is_counted_once_the_count_has_run_out() {
        let mut count = CountIn::new(4, BEAT_FRAMES, 4);
        count.remaining_frames = 0;
        assert!(count_clicks(count, 4_096).is_empty());
    }

    #[test]
    fn a_count_in_sounds_whether_or_not_the_click_is_switched_on() {
        // The click being off is why somebody set a count-in in the first place: they want to be
        // counted in and then left alone.
        let mut metronome = Metronome::new(SignatureMap::default());
        assert!(!metronome.is_enabled());
        let tempo = TempoMap::constant(120.0);
        let mut out = AudioBuffer::new(2, 512, RATE);
        metronome.render(
            &mut out,
            &counting(CountIn::new(4, BEAT_FRAMES, 4)),
            &tempo,
            RATE,
        );
        assert!(out.peak() > 0.1, "got {}", out.peak());
    }

    #[test]
    fn the_beats_of_a_count_in_are_not_the_beats_of_the_timeline() {
        // A count-in at a position that is nowhere near a bar line still counts from its own
        // first beat: the playhead is not moving, so the timeline has nothing to say about it.
        let mut metronome = Metronome::new(SignatureMap::default());
        metronome.set_enabled(true);
        let tempo = TempoMap::constant(120.0);
        let mut transport = at(BEAT_FRAMES * 3 + 7_000, true);
        transport.set_count_in(CountIn::new(4, BEAT_FRAMES, 4));
        let mut out = AudioBuffer::new(2, BEAT_FRAMES as usize * 2, RATE);
        metronome.render(&mut out, &transport, &tempo, RATE);

        let struck = |at: usize| out.channel(0)[at..at + 64].iter().any(|s| s.abs() > 0.01);
        assert!(struck(0), "the count did not begin on its own first frame");
        assert!(struck(BEAT_FRAMES as usize));
        assert!(!struck(BEAT_FRAMES as usize / 2));
    }
}
