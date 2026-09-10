//! The sung line: a melody searched under the words rather than walked over the chords.
//!
//! Every other part writer in this crate answers to the harmony alone. A vocal melody answers
//! to the *lyric* as well: spoken Japanese already gives every phrase a pitch shape, and a
//! tune that contradicts it sings one word while meaning another. This module is the search
//! that honours both, modelled on Orpheus (Fukayama & Sagayama et al., IPSJ Journal 54(5),
//! 2013): melody as a best path through a lattice of candidate pitches, scored by a handful
//! of independent, hand-made cost terms and solved by dynamic programming — no corpus, no
//! model, every number arguable in place.
//!
//! The stages are deliberately separable, because each is a seam something better can walk
//! in through:
//!
//! * **Rhythm** ([`vocal_rhythm_expressive_in_bars`]) turns spoken groups and contours into
//!   short/long rhythmic cells — one syllable, one note, inside a fixed span. The count-only
//!   [`vocal_rhythm`] and [`vocal_rhythm_in_bars`] also serve the editor's length estimates.
//! * **Pitch** ([`write_vocal`]) fills the slots. It reads only [`Contour`] — a vocabulary
//!   that names no language — and the document's own harmony, so another language's prosody
//!   changes nothing here, and a learned melody engine would be a *sibling* of this function
//!   behind the same session command, chosen the way an instrument is, never a rewrite of it.
//!
//! The cost terms are Orpheus's, by name: register (distance from the voice's centre),
//! leap (small steps cheap, the tritone and anything past an octave forbidden), prosody
//! (breaching a syllable's [`Contour`] is expensive but not impossible — a cadence may
//! overrule a word, which is the trade Orpheus reports making about six times in a hundred),
//! and harmony (chord tones free, non-chord tones admitted the classical way: diatonic,
//! reached by step, and never on a phrase's final note). What it does not copy is Orpheus's
//! bass counterpoint term, which would entangle this writer with whichever part writes the
//! bass; the risk is an occasional parallel octave, and the account is here so nobody thinks
//! it was forgotten.

use auris_core::harmony::Harmony;
use auris_core::rng::{Key as RngKey, Rng};
use auris_core::theory::chord::Chord;
use auris_core::theory::contour::Contour;
use auris_core::theory::pitch::PitchClass;
use auris_core::time::{TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignature};
use auris_core::{Fall, Note, Scoop, Vibrato};

/// Where the voice is comfortable, in MIDI notes, inclusive at both ends.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VocalRange {
    /// The lowest note the line may touch.
    pub low: u8,
    /// The highest.
    pub high: u8,
}

impl Default for VocalRange {
    /// A3 to E5 — the octave and a half where most untrained voices, and most J-pop
    /// melodies, actually live.
    fn default() -> Self {
        Self { low: 57, high: 76 }
    }
}

impl VocalRange {
    /// The centre the register cost measures from.
    fn centre(&self) -> f64 {
        f64::from(self.low) / 2.0 + f64::from(self.high) / 2.0
    }
}

/// The note slots a lyric's syllables will occupy, and how much timeline they take.
#[derive(Clone, Debug, PartialEq)]
pub struct VocalRhythm {
    /// One `(onset, length)` per syllable, per phrase, in ticks from the melody's start.
    pub phrases: Vec<Vec<(Ticks, Ticks)>>,
    /// The whole melody's span, rounded up to whole bars — what a clip wants to be.
    pub length: Ticks,
}

/// Lays each phrase's syllables onto the grid: one per eighth, the last held a half note.
///
/// Each phrase starts a fresh bar, and the next starts at the first bar line that leaves at
/// least an eighth of breath after this one ends — a singer breathes between phrases, and a
/// melody with nowhere to breathe reads as wrong before it sounds wrong. The last syllable
/// is *held*, because that is what a sung phrase does — and the held note is where the
/// vibrato rule below finds room to sway. This count-only layout estimates a standalone
/// lyric's span; [`vocal_rhythm_expressive_in_bars`] writes the actual rhythmic phrasing
/// inside that span.
pub fn vocal_rhythm(counts: &[usize], meter: TimeSignature) -> VocalRhythm {
    let eighth = Ticks(TICKS_PER_QUARTER / 2);
    let half = Ticks(TICKS_PER_QUARTER * 2);
    let bar = Ticks(meter.ticks_per_bar().raw().max(1));

    let mut phrases = Vec::with_capacity(counts.len());
    let mut at = Ticks::ZERO;
    let mut end = Ticks::ZERO;
    for count in counts.iter().copied().filter(|count| *count > 0) {
        let mut slots = Vec::with_capacity(count);
        for syllable in 0..count {
            let onset = at + eighth * syllable as i64;
            let length = match syllable + 1 == count {
                true => half,
                false => eighth,
            };
            slots.push((onset, length));
            end = onset + length;
        }
        phrases.push(slots);
        // The next bar line at least a breath away. Signed div_ceil is not stable, and
        // every tick here is non-negative, so the textbook form serves.
        let next = (end + eighth).raw();
        at = Ticks((next + bar.raw() - 1) / bar.raw() * bar.raw());
    }

    let length = Ticks(((end.raw() + bar.raw() - 1) / bar.raw()).max(1) * bar.raw());
    VocalRhythm { phrases, length }
}

/// Fits every syllable into a fixed number of bars, preserving phrase order and breaths.
///
/// Phrases receive time in proportion to their syllables, with extra weight for a held
/// ending and a breath. Boundaries use beats when possible, otherwise sixteenths. Notes
/// use quarters or eighths where space permits, and sixteenths for denser phrases. Within
/// each phrase the final syllable is held longer; sparse lyrics leave space between notes
/// rather than requiring a single syllable to be sustained for several bars.
/// Returns `None` when even sixteenth notes plus phrase endings and breaths will not fit.
pub fn vocal_rhythm_in_bars(
    counts: &[usize],
    meter: TimeSignature,
    bars: usize,
) -> Option<VocalRhythm> {
    let length = Ticks(
        meter
            .ticks_per_bar()
            .raw()
            .checked_mul(i64::try_from(bars).ok()?)?,
    );
    let sixteenth = TICKS_PER_QUARTER / 4;
    let total = usize::try_from(length.raw() / sixteenth).ok()?;
    let counts: Vec<_> = counts.iter().copied().filter(|&count| count > 0).collect();
    let weights: Vec<_> = counts
        .iter()
        .map(|count| count.checked_add(2))
        .collect::<Option<_>>()?;
    let needed = weights
        .iter()
        .try_fold(0usize, |sum, weight| sum.checked_add(*weight))?;
    if total == 0 || needed > total {
        return None;
    }
    if counts.is_empty() {
        return Some(VocalRhythm {
            phrases: Vec::new(),
            length,
        });
    }

    // Prefer phrase boundaries on the meter's beats, including compound-meter eighths.
    let beat = (meter.ticks_per_beat().raw() / sixteenth).max(1) as usize;
    let boundary = if weights.iter().map(|w| w.div_ceil(beat)).sum::<usize>() <= total / beat {
        beat
    } else {
        1
    };
    let minimum: Vec<_> = weights.iter().map(|w| w.div_ceil(boundary)).collect();
    let spare = total / boundary - minimum.iter().sum::<usize>();
    let mut cumulative = 0;
    let mut assigned = 0;
    let mut at = 0;
    let mut phrases = Vec::with_capacity(counts.len());
    for ((count, weight), minimum) in counts.iter().zip(&weights).zip(minimum) {
        cumulative += weight;
        let share = (spare as u128 * cumulative as u128 / needed as u128) as usize;
        let span = (minimum + share - assigned) * boundary;
        assigned = share;
        let step = if span / weight >= 4 {
            4
        } else if span / weight >= 2 {
            2
        } else {
            1
        };
        let available = span / step;
        let breath = (available / weight).clamp(1, (beat / step).max(1));
        let sung = available - breath;
        let mut slots = Vec::with_capacity(*count);
        for syllable in 0..*count {
            let onset = (sung as u128 * syllable as u128 / (count + 1) as u128) as usize;
            let end = if syllable + 1 == *count {
                sung
            } else {
                (sung as u128 * (syllable + 1) as u128 / (count + 1) as u128) as usize
            };
            let duration =
                ((end - onset) as i64 * step as i64 * sixteenth).min(meter.ticks_per_bar().raw());
            slots.push((
                Ticks((at + onset * step) as i64 * sixteenth),
                Ticks(duration),
            ));
        }
        phrases.push(slots);
        at += span;
    }
    Some(VocalRhythm { phrases, length })
}

/// Spoken grouping available to the rhythm writer, without a language dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VocalPhraseProsody {
    /// One contour per syllable; its length is the required note count.
    pub contours: Vec<Contour>,
    /// Syllable indices starting a word or accent group, including zero when known.
    pub group_starts: Vec<usize>,
}

/// Fits a lyric using a bounded search over rhythmic cells and spoken group boundaries.
///
/// Capacity is identical to [`vocal_rhythm_in_bars`]: a sixteenth per syllable, an extra
/// sixteenth for each ending, and a breath. Within that budget, candidates vary pickups,
/// short/long cells and group endings. Pitch-accent nuclei may hold, while group starts
/// favour the beat or its offbeat according to `style`. A phrase recalls the opening
/// rhythm of the first phrase when its length permits. The seed selects a reproducible
/// take; no note is dropped to improve a score and no candidate extends the given bars.
pub fn vocal_rhythm_expressive_in_bars(
    phrases: &[VocalPhraseProsody],
    meter: TimeSignature,
    bars: usize,
    style: Option<crate::PerformanceStyle>,
    seed: u64,
) -> Option<VocalRhythm> {
    let counts: Vec<_> = phrases.iter().map(|phrase| phrase.contours.len()).collect();
    let mut rhythm = vocal_rhythm_in_bars(&counts, meter, bars)?;
    let sixteenth = TICKS_PER_QUARTER / 4;
    let beat = (meter.ticks_per_beat().raw() / sixteenth).max(1) as usize;
    let bar = (meter.ticks_per_bar().raw() / sixteenth).max(1) as usize;
    let syncopation = match style {
        Some(crate::PerformanceStyle::CityPop | crate::PerformanceStyle::JazzTrio) => 0.55,
        Some(crate::PerformanceStyle::Rock | crate::PerformanceStyle::Chiptune) => 0.2,
        Some(crate::PerformanceStyle::Ambient | crate::PerformanceStyle::Orchestral) => 0.1,
        _ => 0.35,
    };
    let mut hook = Vec::new();
    for (index, phrase) in phrases
        .iter()
        .filter(|phrase| !phrase.contours.is_empty())
        .enumerate()
    {
        let start = rhythm.phrases[index][0].0.raw() / sixteenth;
        let end = rhythm
            .phrases
            .get(index + 1)
            .map_or(rhythm.length.raw(), |next| next[0].0.raw())
            / sixteenth;
        let span = (end - start) as usize;
        let count = phrase.contours.len();
        let spare = span - count - 2;
        let mut best = (f64::INFINITY, Vec::new());
        // Fixed work per syllable, independent of the duration or rejection rate.
        for candidate in 0..24 {
            let mut rng = Rng::stream(
                seed,
                &[
                    RngKey::Word("vocal-rhythm"),
                    RngKey::Index(index as u64),
                    RngKey::Index(candidate),
                ],
            );
            let pickup = if candidate % 3 == 0 {
                (beat / 2).min(spare)
            } else {
                0
            };
            let breath = 1 + spare.saturating_sub(pickup).min(beat.saturating_sub(1));
            let sung = span - pickup - breath;
            let cell = match candidate % 6 {
                0 => [2usize, 1, 1, 2],
                1 => [1, 1, 2, 2],
                2 => [3, 1, 2, 2],
                3 => [1, 3, 2, 2],
                4 => [2, 2, 1, 3],
                _ => [2, 2, 2, 2],
            };
            let rotation = rng.below(cell.len());
            let weights: Vec<_> = (0..count)
                .map(|at| {
                    let nucleus = phrase.contours.get(at + 1) == Some(&Contour::Fall);
                    let group_end = phrase.group_starts.contains(&(at + 1));
                    cell[(at + rotation) % cell.len()]
                        + usize::from(nucleus || group_end)
                        + usize::from(at + 1 == count) * 3
                })
                .collect();
            let total_weight = weights.iter().sum::<usize>();
            let extra = sung - count - 1;
            let mut accumulated = 0;
            let mut assigned = 0;
            let mut onset = start as usize + pickup;
            let mut slots = Vec::with_capacity(count);
            for (at, weight) in weights.into_iter().enumerate() {
                accumulated += weight;
                // Use u128 so a long but valid timeline cannot overflow intermediate shares.
                let share = (extra as u128 * accumulated as u128 / total_weight as u128) as usize;
                let interval = 1 + usize::from(at + 1 == count) + share - assigned;
                assigned = share;
                let separation =
                    usize::from(phrase.group_starts.contains(&(at + 1)) && interval >= 3);
                let duration = (interval - separation).min(bar);
                // A very sparse lyric leaves rests, not an early cadence followed by
                // several empty bars. Place its final held syllable at the phrase's end.
                let entry = onset
                    + if at + 1 == count {
                        interval - duration
                    } else {
                        0
                    };
                slots.push((
                    Ticks(entry as i64 * sixteenth),
                    Ticks(duration as i64 * sixteenth),
                ));
                onset += interval;
            }
            let score =
                rhythm_cost(&slots, phrase, beat, syncopation, &hook) + f64::from(rng.unit()) * 0.3;
            if score < best.0 {
                best = (score, slots);
            }
        }
        if hook.is_empty() {
            hook = best.1.clone();
        }
        rhythm.phrases[index] = best.1;
    }
    Some(rhythm)
}

fn rhythm_cost(
    slots: &[(Ticks, Ticks)],
    phrase: &VocalPhraseProsody,
    beat: usize,
    syncopation: f64,
    hook: &[(Ticks, Ticks)],
) -> f64 {
    let unit = TICKS_PER_QUARTER / 4;
    let beat_ticks = beat as i64 * unit;
    let mut cost = 0.0;
    let mut offbeats = 0;
    for (at, &(onset, length)) in slots.iter().enumerate() {
        let position = onset.raw().rem_euclid(beat_ticks);
        offbeats += usize::from(position != 0);
        if phrase.group_starts.contains(&at) {
            cost += if position == 0 {
                syncopation * 0.25
            } else if position == beat_ticks / 2 {
                (1.0 - syncopation) * 0.25
            } else {
                0.7
            };
        }
        if phrase.contours.get(at + 1) == Some(&Contour::Fall)
            && let Some((_, following)) = slots.get(at + 1)
            && length < *following
        {
            cost += 0.35;
        }
    }
    cost += (offbeats as f64 / slots.len() as f64 - syncopation).abs() * 3.0;
    let intervals: Vec<_> = slots.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
    if intervals.len() >= 3 && intervals.windows(2).all(|pair| pair[0] == pair[1]) {
        cost += 1.2;
    }
    if hook.len() == slots.len() {
        // Recall the hook's first few onsets while leaving the cadence free to answer it.
        for at in 1..slots.len().saturating_sub(2).min(5) {
            let original = hook[at].0 - hook[0].0;
            let recalled = slots[at].0 - slots[0].0;
            cost += ((original.raw() - recalled.raw()).abs() as f64 / beat_ticks as f64).min(2.0)
                * 0.45;
        }
    }
    cost
}

/// The shortest note the vibrato rule sways, in seconds.
///
/// Under half a second there is no room for the sway to grow before the note is over, and a
/// vibrato that never reaches depth reads as a wobble. With the phrase-final half note this
/// estimate writes, the held syllable clears the bar at any tempo under ~260 BPM. Phrase position
/// separately keeps passing notes out at slower tempos, where an eighth can cross this duration.
pub const VIBRATO_FROM_SECONDS: f64 = 0.45;

/// Dresses a written vocal line in the ornaments a singer would add, by rule.
///
/// Three rules, each the plainest reading of what singers actually do:
///
/// * **A phrase is entered from below.** Its first note gets the stock scoop — the voice
///   finds the pitch rather than starting on it.
/// * **A held note sways.** Any note at least [`VIBRATO_FROM_SECONDS`] long gets a vibrato
///   that waits out roughly the first third of the note and fades in over the next — the
///   straight-then-sway shape of a sung long tone.
/// * **The song lets go at the end.** The very last note, and only that one, gets the stock
///   fall; a fall on every phrase would make a manner out of a gesture.
///
/// The ornaments land on the notes as ordinary [`Note::scoop`]-family data — the same fields
/// a hand sets from the piano roll, visible on the drawn pitch curve, adjustable and
/// removable one by one. Rule-based on purpose: a learned ornament model would be another
/// *sibling* of this function, chosen the way a melody engine would be.
pub fn ornament_vocal(notes: &mut [Note], rhythm: &VocalRhythm, tempo: &TempoMap, start: Ticks) {
    let firsts: Vec<Ticks> = rhythm
        .phrases
        .iter()
        .filter_map(|slots| slots.first().map(|(onset, _)| *onset))
        .collect();
    let last = rhythm
        .phrases
        .last()
        .and_then(|slots| slots.last())
        .map(|(onset, _)| *onset);
    let helds: Vec<Ticks> = rhythm
        .phrases
        .iter()
        .filter_map(|slots| slots.last().map(|(onset, _)| *onset))
        .collect();

    for note in notes {
        let seconds = tempo.ticks_to_seconds(start + note.end()).0
            - tempo.ticks_to_seconds(start + note.start).0;
        if firsts.contains(&note.start) {
            note.scoop = Some(Scoop::default());
        }
        if Some(note.start) == last {
            note.fall = Some(Fall::default());
        }
        if helds.contains(&note.start) && seconds >= VIBRATO_FROM_SECONDS {
            note.vibrato = Some(Vibrato {
                depth: 0.3,
                rate: 5.8,
                delay: (seconds * 0.35).clamp(0.12, 0.6),
                fade_in: (seconds * 0.3).min(0.3),
            });
        }
    }
}

/// Breaching a syllable's contour — expensive, and deliberately not impossible.
///
/// Orpheus reports its own melodies overruling the accent about six times in a hundred,
/// nearly always where a cadence outranks a word; a hard constraint would instead refuse to
/// end phrases.
const CONTOUR_BREACH: f64 = 8.0;

/// How hard the line is pulled toward the register's centre, per octave of distance, squared.
const REGISTER_WEIGHT: f64 = 1.5;

/// A non-chord tone reached by step — a passing or neighbour note, the two the classical
/// rule admits.
const NONCHORD_STEP: f64 = 0.7;

/// A non-chord tone reached by leap, which the classical rule does not admit at all.
const NONCHORD_LEAP: f64 = 3.0;

/// A non-chord tone landing on a beat, where the harmony is most audible.
const NONCHORD_ON_BEAT: f64 = 0.8;

/// A phrase ending anywhere but on the chord.
const CADENCE_NONCHORD: f64 = 5.0;

/// A first note off the chord — a phrase should announce where it stands.
const OPENING_NONCHORD: f64 = 1.0;

/// How loudly the seed speaks: enough to break ties between equally good paths, far too
/// little to outvote any real cost term. This is what makes a seed name a take.
const JITTER: f64 = 0.05;

/// The price of moving by so many semitones — steps free, thirds cheap, the tritone and
/// anything past the octave unsingable.
fn leap_cost(semitones: i64) -> f64 {
    match semitones.abs() {
        0 => 0.4,
        1 | 2 => 0.0,
        3 | 4 => 0.35,
        5 => 0.6,
        6 => f64::INFINITY,
        7 => 0.8,
        8..=12 => 1.6,
        _ => f64::INFINITY,
    }
}

/// What one syllable's slot knows: where it sounds, and what harmony stands under it.
struct Slot {
    onset: Ticks,
    length: Ticks,
    chord: Option<Chord>,
    candidates: Vec<u8>,
    on_beat: bool,
}

/// Writes the sung line: one note per syllable, chosen by the best path through the lattice.
///
/// `phrases` carries each syllable's [`Contour`] and must line up with `rhythm` — both come
/// from the same lyric, and where their lengths disagree the shorter is trusted. Notes come
/// back positioned from the melody's own start (clip-relative); `start` is where that melody
/// will sit on the timeline, which is where its chords are looked up. A stretch with no
/// chords written under it constrains nothing harmonically rather than refusing: a lyric is
/// singable over silence, and the session decides whether to write chords first.
///
/// Two runs with the same inputs and seed are the same melody, exactly. The seed chooses
/// an initial melodic gesture, which later phrases recall; accent and harmony costs remain
/// stronger than that gesture. Consecutive phrases share a register and a singable entry.
pub fn write_vocal(
    harmony: &Harmony,
    start: Ticks,
    rhythm: &VocalRhythm,
    phrases: &[Vec<Contour>],
    range: VocalRange,
    seed: u64,
) -> Vec<Note> {
    let centre = range.centre();
    let mut notes = Vec::new();
    let mut hook: Vec<u8> = Vec::new();
    let mut previous_end: Option<u8> = None;
    const GESTURES: [[i8; 8]; 4] = [
        [0, 0, 2, 4, 2, 0, -2, 0],
        [0, 2, 4, 2, 0, 2, 0, -2],
        [2, 0, -2, 0, 2, 4, 2, 0],
        [0, 2, 0, -2, 0, 2, 4, 2],
    ];
    let gesture = GESTURES[Rng::stream(seed, &[RngKey::Word("vocal-hook")]).below(GESTURES.len())];

    for (index, (slots, contours)) in rhythm.phrases.iter().zip(phrases).enumerate() {
        let count = slots.len().min(contours.len());
        if count == 0 {
            continue;
        }
        let slots: Vec<Slot> = slots[..count]
            .iter()
            .map(|(onset, length)| {
                let tick = start + *onset;
                let key = harmony.key_at(tick);
                let chord = harmony.chord_at(tick);
                let candidates = (range.low..=range.high)
                    .filter(|midi| {
                        let class = PitchClass::new(i32::from(*midi));
                        key.scale.contains(key.tonic, class)
                            || chord.is_some_and(|chord| chord.contains_midi(i32::from(*midi)))
                    })
                    .collect();
                Slot {
                    onset: *onset,
                    length: *length,
                    chord,
                    candidates,
                    on_beat: tick.raw().rem_euclid(TICKS_PER_QUARTER) == 0,
                }
            })
            .collect();
        if slots.iter().any(|slot| slot.candidates.is_empty()) {
            // A range so narrow no scale note fits it is nothing to sing in.
            continue;
        }

        let jitter = |slot: usize, pitch: u8| {
            let mut stream = Rng::stream(
                seed,
                &[
                    RngKey::Word("vocal"),
                    RngKey::Index(index as u64),
                    RngKey::Index(slot as u64),
                    RngKey::Index(u64::from(pitch)),
                ],
            );
            f64::from(stream.unit()) * JITTER
        };
        let register = |at: usize, pitch: u8| {
            let octaves = (f64::from(pitch) - centre) / 12.0;
            let target = if hook.is_empty() {
                centre + f64::from(gesture[at * gesture.len() / count])
            } else {
                f64::from(hook[at * hook.len() / count])
            };
            // A remembered pitch is a preference, never a reason to mispronounce a word.
            // The final two syllables remain free to find this phrase's own cadence.
            let recall = if !hook.is_empty() && at + 2 >= count {
                0.1
            } else {
                0.45
            };
            octaves * octaves * REGISTER_WEIGHT + (f64::from(pitch) - target).abs() * recall
        };
        let harmony_cost = |slot: &Slot, pitch: u8, arrived_by: Option<i64>| {
            let Some(chord) = slot.chord else { return 0.0 };
            if chord.contains_midi(i32::from(pitch)) {
                return 0.0;
            }
            let mut cost = match arrived_by {
                Some(step) if step.abs() <= 2 => NONCHORD_STEP,
                Some(_) => NONCHORD_LEAP,
                None => OPENING_NONCHORD,
            };
            if slot.on_beat {
                cost += NONCHORD_ON_BEAT;
            }
            cost
        };

        // Viterbi over (slot, candidate): cost so far and the predecessor that paid it.
        let mut paths: Vec<Vec<(f64, usize)>> = Vec::with_capacity(count);
        let first: Vec<(f64, usize)> = slots[0]
            .candidates
            .iter()
            .map(|pitch| {
                (
                    register(0, *pitch)
                        + harmony_cost(&slots[0], *pitch, None)
                        + previous_end.map_or(0.0, |previous| {
                            leap_cost(i64::from(*pitch) - i64::from(previous))
                        })
                        + jitter(0, *pitch),
                    0,
                )
            })
            .collect();
        paths.push(first);

        for at in 1..count {
            let final_note = at + 1 == count;
            let row: Vec<(f64, usize)> = slots[at]
                .candidates
                .iter()
                .map(|pitch| {
                    let mut best = (f64::INFINITY, 0usize);
                    for (from, previous) in slots[at - 1].candidates.iter().enumerate() {
                        let standing = paths[at - 1][from].0;
                        if standing >= best.0 {
                            continue;
                        }
                        let step = i64::from(*pitch) - i64::from(*previous);
                        let contour = match contours[at] {
                            Contour::Rise if step <= 0 => CONTOUR_BREACH,
                            Contour::Fall if step >= 0 => CONTOUR_BREACH,
                            Contour::NoFall if step < 0 => CONTOUR_BREACH,
                            _ => 0.0,
                        };
                        let cadence = match final_note
                            && slots[at]
                                .chord
                                .is_some_and(|chord| !chord.contains_midi(i32::from(*pitch)))
                        {
                            true => CADENCE_NONCHORD,
                            false => 0.0,
                        };
                        let cost = standing
                            + leap_cost(step)
                            + contour
                            + cadence
                            + harmony_cost(&slots[at], *pitch, Some(step));
                        if cost < best.0 {
                            best = (cost, from);
                        }
                    }
                    (best.0 + register(at, *pitch) + jitter(at, *pitch), best.1)
                })
                .collect();
            paths.push(row);
        }

        // Walk the best path back out.
        let mut chosen = vec![0usize; count];
        chosen[count - 1] = paths[count - 1]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
            .map(|(at, _)| at)
            .unwrap_or(0);
        for at in (1..count).rev() {
            chosen[at - 1] = paths[at][chosen[at]].1;
        }
        for (at, slot) in slots.iter().enumerate() {
            notes.push(Note::new(
                slot.candidates[chosen[at]],
                slot.onset,
                slot.length,
            ));
        }
        if hook.is_empty() {
            hook = slots
                .iter()
                .enumerate()
                .map(|(at, slot)| slot.candidates[chosen[at]])
                .collect();
        }
        previous_end = notes.last().map(|note| note.pitch);
    }

    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::theory::key::Key;
    use auris_core::theory::numeral::Numeral;
    use auris_core::theory::pitch::PitchClass;
    use auris_core::theory::scale::ScaleId;

    fn prosody(count: usize, group_starts: &[usize]) -> VocalPhraseProsody {
        VocalPhraseProsody {
            contours: vec![Contour::Free; count],
            group_starts: group_starts.to_vec(),
        }
    }

    #[test]
    fn expressive_rhythm_preserves_capacity_moras_and_breaths_in_every_meter() {
        for meter in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(6, 8),
            TimeSignature::new(7, 8),
        ] {
            for counts in [&[3][..], &[6, 5], &[14], &[15], &[24, 36, 9], &[0, 4, 0, 5]] {
                let phrases: Vec<_> = counts.iter().map(|&count| prosody(count, &[0])).collect();
                for bars in [1, 4, 8] {
                    let capacity = vocal_rhythm_in_bars(counts, meter, bars).is_some();
                    for seed in 0..4 {
                        let result = vocal_rhythm_expressive_in_bars(
                            &phrases,
                            meter,
                            bars,
                            Some(crate::PerformanceStyle::CityPop),
                            seed,
                        );
                        assert_eq!(result.is_some(), capacity);
                        let Some(rhythm) = result else { continue };
                        assert_eq!(rhythm.length, meter.ticks_per_bar() * bars as i64);
                        assert_eq!(
                            rhythm.phrases.len(),
                            counts.iter().filter(|&&n| n > 0).count()
                        );
                        for (slots, &count) in
                            rhythm.phrases.iter().zip(counts.iter().filter(|&&n| n > 0))
                        {
                            assert_eq!(slots.len(), count);
                            assert!(slots.iter().all(|&(at, length)| {
                                at >= Ticks::ZERO
                                    && length >= Ticks(TICKS_PER_QUARTER / 4)
                                    && at + length <= rhythm.length
                            }));
                            assert!(
                                slots
                                    .windows(2)
                                    .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0)
                            );
                        }
                        assert!(rhythm.phrases.windows(2).all(|pair| {
                            let &(at, length) = pair[0].last().unwrap();
                            pair[1][0].0 - (at + length) >= Ticks(TICKS_PER_QUARTER / 4)
                        }));
                        let &(at, length) = rhythm.phrases.last().unwrap().last().unwrap();
                        assert!((at + length).raw() > rhythm.length.raw() / 2);
                    }
                }
            }
        }
    }

    #[test]
    fn expressive_capacity_handles_empty_and_overflowing_spans_without_panicking() {
        let meter = TimeSignature::default();
        assert!(
            vocal_rhythm_expressive_in_bars(&[prosody(4, &[0])], meter, usize::MAX, None, 0)
                .is_none()
        );
        let empty = vocal_rhythm_expressive_in_bars(&[], meter, 4, None, 0).unwrap();
        assert!(empty.phrases.is_empty());
        let bars = (i64::MAX / meter.ticks_per_bar().raw()) as usize;
        let rhythm = vocal_rhythm_expressive_in_bars(
            &[prosody(1000, &[0]), prosody(1000, &[0])],
            meter,
            bars,
            None,
            0,
        )
        .unwrap();
        assert_eq!(rhythm.phrases.iter().map(Vec::len).sum::<usize>(), 2000);
        assert!(
            rhythm
                .phrases
                .iter()
                .flatten()
                .all(|(at, length)| *at + *length <= rhythm.length)
        );
    }

    #[test]
    fn expressive_rhythm_has_short_long_cells_seed_variation_and_style_control() {
        let phrases = [prosody(10, &[0, 3, 6])];
        let meter = TimeSignature::default();
        let take =
            |seed, style| vocal_rhythm_expressive_in_bars(&phrases, meter, 4, style, seed).unwrap();
        let original = take(2, None);
        assert_eq!(original, take(2, None));
        let intervals: Vec<_> = original.phrases[0]
            .windows(2)
            .map(|pair| pair[1].0 - pair[0].0)
            .collect();
        assert!(
            intervals.windows(2).any(|pair| pair[0] != pair[1]),
            "a sung phrase needs more than an even walk"
        );
        assert!((0..16).any(|seed| take(seed, None) != original));
        let offbeats = |style| -> usize {
            (0..16)
                .map(|seed| {
                    take(seed, Some(style)).phrases[0]
                        .iter()
                        .filter(|(onset, _)| onset.raw() % TICKS_PER_QUARTER != 0)
                        .count()
                })
                .sum()
        };
        assert!(
            offbeats(crate::PerformanceStyle::CityPop)
                > offbeats(crate::PerformanceStyle::Orchestral)
        );
    }

    #[test]
    fn grouping_and_accent_shape_rhythm_without_changing_capacity() {
        let plain = prosody(9, &[0]);
        let grouped = prosody(9, &[0, 3, 6]);
        let mut accented = grouped.clone();
        accented.contours[4] = Contour::Fall;
        let write = |phrase: &VocalPhraseProsody, seed| {
            vocal_rhythm_expressive_in_bars(
                std::slice::from_ref(phrase),
                TimeSignature::default(),
                3,
                None,
                seed,
            )
            .unwrap()
        };
        assert!((0..16).any(|seed| write(&plain, seed) != write(&grouped, seed)));
        assert!((0..16).any(|seed| write(&grouped, seed) != write(&accented, seed)));
        for seed in 0..16 {
            assert_eq!(write(&accented, seed).phrases[0].len(), 9);
        }
    }

    #[test]
    fn vocal_phrases_recall_the_hook_and_reenter_without_an_unsingable_jump() {
        let rhythm = vocal_rhythm(&[8, 8, 8], TimeSignature::default());
        let phrases = vec![vec![Contour::Free; 8]; 3];
        for seed in 0..8 {
            let notes = write_vocal(
                &c_major(),
                Ticks::ZERO,
                &rhythm,
                &phrases,
                VocalRange::default(),
                seed,
            );
            assert_eq!(notes.len(), 24);
            for next in [8, 16] {
                let leap = (i16::from(notes[next].pitch) - i16::from(notes[next - 1].pitch)).abs();
                assert!(leap <= 12 && leap != 6);
                let opening_distance: i16 = (0..4)
                    .map(|at| {
                        (i16::from(notes[at].pitch) - i16::from(notes[next + at].pitch)).abs()
                    })
                    .sum();
                assert!(
                    opening_distance <= 8,
                    "the hook's first four pitches drifted by {opening_distance} semitones"
                );
            }
        }
    }

    #[test]
    fn fitted_rhythm_uses_fixed_bars_and_preserves_every_syllable() {
        for meter in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(6, 8),
            TimeSignature::new(7, 8),
        ] {
            for counts in [
                &[3][..],
                &[6, 5],
                &[24, 36, 9],
                &[1, 1, 1, 1, 1, 1, 1, 1, 1],
            ] {
                let rhythm = vocal_rhythm_in_bars(counts, meter, 8).unwrap();
                assert_eq!(rhythm.length, meter.ticks_per_bar() * 8);
                assert_eq!(rhythm.phrases.len(), counts.len());
                for (phrase, &count) in rhythm.phrases.iter().zip(counts) {
                    assert_eq!(phrase.len(), count);
                    assert!(
                        phrase
                            .iter()
                            .all(|&(at, length)| length >= Ticks(TICKS_PER_QUARTER / 4)
                                && at + length <= rhythm.length)
                    );
                    assert!(
                        phrase
                            .windows(2)
                            .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0)
                    );
                }
                assert!(rhythm.phrases.windows(2).all(|pair| {
                    let &(at, length) = pair[0].last().unwrap();
                    pair[1][0].0 - (at + length) >= Ticks(TICKS_PER_QUARTER / 4)
                }));
                let &(at, length) = rhythm.phrases.last().unwrap().last().unwrap();
                assert!((at + length).raw() > rhythm.length.raw() / 2);
                assert_eq!(rhythm, vocal_rhythm_in_bars(counts, meter, 8).unwrap());
            }
        }
    }

    #[test]
    fn fitted_rhythm_holds_endings_and_compresses_dense_phrases() {
        let meter = TimeSignature::default();
        let short = vocal_rhythm_in_bars(&[6, 5], meter, 4).unwrap();
        let dense = vocal_rhythm_in_bars(&[24, 24], meter, 4).unwrap();
        assert_eq!(short.length, dense.length);
        assert!(short.phrases[0][0].1 > dense.phrases[0][0].1);
        assert!(
            short
                .phrases
                .iter()
                .flatten()
                .all(|(at, length)| at.raw() % TICKS_PER_QUARTER == 0
                    && length.raw() % TICKS_PER_QUARTER == 0)
        );
        for phrase in &dense.phrases {
            assert!(phrase.last().unwrap().1 > phrase[0].1);
        }
        assert!(vocal_rhythm_in_bars(&[15], meter, 1).is_none());
        assert!(vocal_rhythm_in_bars(&[1], meter, 0).is_none());
        assert!(vocal_rhythm_in_bars(&[usize::MAX], meter, 8).is_none());
        assert!(
            vocal_rhythm_in_bars(&[0, 0], meter, 4)
                .unwrap()
                .phrases
                .is_empty()
        );
    }

    /// C major, tonic chords throughout — the flattest ground to measure on.
    fn c_major() -> Harmony {
        let mut harmony = Harmony::in_key(Key::new(PitchClass::new(0), ScaleId::Major));
        harmony
            .chords
            .set_point(Ticks::ZERO, Some(Numeral::new(1, false)));
        harmony
    }

    fn contours(spec: &[Contour]) -> Vec<Vec<Contour>> {
        vec![spec.to_vec()]
    }

    fn sung(harmony: &Harmony, spec: &[Contour], seed: u64) -> Vec<Note> {
        let rhythm = vocal_rhythm(&[spec.len()], TimeSignature::default());
        write_vocal(
            harmony,
            Ticks::ZERO,
            &rhythm,
            &contours(spec),
            VocalRange::default(),
            seed,
        )
    }

    #[test]
    fn the_rhythm_gives_every_syllable_an_eighth_and_holds_the_last() {
        let rhythm = vocal_rhythm(&[3, 2], TimeSignature::default());
        let eighth = TICKS_PER_QUARTER / 2;
        assert_eq!(
            rhythm.phrases[0],
            [
                (Ticks(0), Ticks(eighth)),
                (Ticks(eighth), Ticks(eighth)),
                (Ticks(eighth * 2), Ticks(TICKS_PER_QUARTER * 2)),
            ]
        );
        // The second phrase starts on the next bar line, a breath after the first ends.
        assert_eq!(rhythm.phrases[1][0].0, Ticks(TICKS_PER_QUARTER * 4));
        // And the whole thing is whole bars.
        assert_eq!(rhythm.length, Ticks(TICKS_PER_QUARTER * 8));
        // An empty phrase takes no bar with it.
        assert_eq!(
            vocal_rhythm(&[0, 1], TimeSignature::default())
                .phrases
                .len(),
            1
        );
    }

    #[test]
    fn the_line_obeys_the_accent() {
        // 中高: free, rise, no-fall, fall, no-fall — every step must match on easy ground.
        let spec = [
            Contour::Free,
            Contour::Rise,
            Contour::NoFall,
            Contour::Fall,
            Contour::NoFall,
        ];
        let notes = sung(&c_major(), &spec, 0);
        assert_eq!(notes.len(), 5);
        let pitch = |at: usize| i32::from(notes[at].pitch);
        assert!(pitch(1) > pitch(0), "the voice rises onto the second mora");
        assert!(pitch(2) >= pitch(1), "and does not fall before the nucleus");
        assert!(pitch(3) < pitch(2), "the nucleus falls");
        assert!(pitch(4) >= pitch(3), "and nothing falls after it");
    }

    #[test]
    fn the_line_stays_diatonic_in_range_and_never_leaps_a_tritone() {
        let spec = vec![Contour::Free; 12];
        let notes = sung(&c_major(), &spec, 3);
        let range = VocalRange::default();
        const C_MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        for note in &notes {
            assert!(note.pitch >= range.low && note.pitch <= range.high);
            assert!(
                C_MAJOR.contains(&(i32::from(note.pitch) % 12)),
                "{}",
                note.pitch
            );
        }
        for pair in notes.windows(2) {
            let step = (i32::from(pair[1].pitch) - i32::from(pair[0].pitch)).abs();
            assert_ne!(step, 6, "the tritone is forbidden");
            assert!(step <= 12, "an octave is the widest leap");
        }
    }

    #[test]
    fn a_phrase_ends_on_the_chord() {
        let spec = vec![Contour::Free; 6];
        let notes = sung(&c_major(), &spec, 5);
        let last = i32::from(notes.last().unwrap().pitch) % 12;
        assert!(
            [0, 4, 7].contains(&last),
            "C major owns the cadence, got {last}"
        );
    }

    #[test]
    fn the_rules_dress_the_line_the_way_a_singer_would() {
        let harmony = c_major();
        let rhythm = vocal_rhythm(&[3, 2], TimeSignature::default());
        let phrases = vec![vec![Contour::Free; 3], vec![Contour::Free; 2]];
        let mut notes = write_vocal(
            &harmony,
            Ticks::ZERO,
            &rhythm,
            &phrases,
            VocalRange::default(),
            0,
        );
        let tempo = TempoMap::constant(120.0);
        ornament_vocal(&mut notes, &rhythm, &tempo, Ticks::ZERO);

        // Each phrase is entered from below; nothing mid-phrase is.
        assert!(notes[0].scoop.is_some(), "the first phrase scoops in");
        assert!(notes[3].scoop.is_some(), "and so does the second");
        assert!(notes[1].scoop.is_none() && notes[2].scoop.is_none());

        // The held finals sway — a half note at 120 BPM is a second — and the sway waits
        // out the front of the note and still has the back half to be heard in.
        for held in [2usize, 4] {
            let vibrato = notes[held].vibrato.expect("a held note sways");
            assert!(vibrato.delay >= 0.12 && vibrato.delay <= 0.6);
            assert!(
                vibrato.delay + vibrato.fade_in < 1.0,
                "the sway reaches depth inside the note"
            );
        }
        // The passing eighths — a quarter of a second — never do.
        assert!(notes[0].vibrato.is_none() && notes[1].vibrato.is_none());

        // Only the song's last note lets go.
        assert!(notes[4].fall.is_some(), "the end falls away");
        assert!(
            notes[..4].iter().all(|note| note.fall.is_none()),
            "and nothing before it does"
        );
    }

    #[test]
    fn a_slow_passing_eighth_is_not_mistaken_for_a_held_note() {
        let harmony = c_major();
        let rhythm = vocal_rhythm(&[3], TimeSignature::default());
        let mut notes = write_vocal(
            &harmony,
            Ticks::ZERO,
            &rhythm,
            &[vec![Contour::Free; 3]],
            VocalRange::default(),
            0,
        );

        ornament_vocal(&mut notes, &rhythm, &TempoMap::constant(60.0), Ticks::ZERO);

        assert!(notes[0].vibrato.is_none() && notes[1].vibrato.is_none());
        assert!(
            notes[2].vibrato.is_some(),
            "the phrase-final hold still sways"
        );
    }

    #[test]
    fn the_seed_names_the_take() {
        let spec = vec![Contour::Free; 8];
        let harmony = c_major();
        assert_eq!(sung(&harmony, &spec, 9), sung(&harmony, &spec, 9));
        let takes: Vec<Vec<Note>> = (0..8).map(|seed| sung(&harmony, &spec, seed)).collect();
        assert!(
            takes.windows(2).any(|pair| pair[0] != pair[1]),
            "eight seeds sang eight identical lines"
        );
    }

    #[test]
    fn no_chords_still_sings_and_an_empty_lyric_does_not() {
        let harmony = Harmony::in_key(Key::new(PitchClass::new(0), ScaleId::Major));
        let notes = sung(&harmony, &[Contour::Free, Contour::Rise], 0);
        assert_eq!(notes.len(), 2, "a lyric is singable over silence");
        assert!(sung(&harmony, &[], 0).is_empty());
    }
}
