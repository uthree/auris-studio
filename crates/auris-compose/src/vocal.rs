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
//! * **Rhythm** ([`vocal_rhythm`]) turns syllable counts into note slots — one syllable, one
//!   note, the syllabic default of Japanese song. It is its own public function so a richer
//!   scheme (Orpheus's rhythm trees, a learned rhythm model) can replace it without touching
//!   the pitch search.
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

use auris_core::Note;
use auris_core::harmony::Harmony;
use auris_core::rng::{Key as RngKey, Rng};
use auris_core::theory::chord::Chord;
use auris_core::theory::contour::Contour;
use auris_core::theory::pitch::PitchClass;
use auris_core::time::{TICKS_PER_QUARTER, Ticks, TimeSignature};

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

/// Lays each phrase's syllables onto the grid: one per eighth, the last held a quarter.
///
/// Each phrase starts a fresh bar, and the next starts at the first bar line that leaves at
/// least an eighth of breath after this one ends — a singer breathes between phrases, and a
/// melody with nowhere to breathe reads as wrong before it sounds wrong. The scheme is
/// Orpheus's "rhythm decision" reduced to its plainest honest form; the rhythm-tree library
/// that would vary it is future work, and lives behind this signature when it comes.
pub fn vocal_rhythm(counts: &[usize], meter: TimeSignature) -> VocalRhythm {
    let eighth = Ticks(TICKS_PER_QUARTER / 2);
    let quarter = Ticks(TICKS_PER_QUARTER);
    let bar = Ticks(meter.ticks_per_bar().raw().max(1));

    let mut phrases = Vec::with_capacity(counts.len());
    let mut at = Ticks::ZERO;
    let mut end = Ticks::ZERO;
    for count in counts.iter().copied().filter(|count| *count > 0) {
        let mut slots = Vec::with_capacity(count);
        for syllable in 0..count {
            let onset = at + eighth * syllable as i64;
            let length = match syllable + 1 == count {
                true => quarter,
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
/// Two runs with the same inputs and seed are the same melody, exactly; the seed breaks ties
/// between paths the costs cannot tell apart, and nothing else.
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
        let register = |pitch: u8| {
            let octaves = (f64::from(pitch) - centre) / 12.0;
            octaves * octaves * REGISTER_WEIGHT
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
                    register(*pitch) + harmony_cost(&slots[0], *pitch, None) + jitter(0, *pitch),
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
                    (best.0 + register(*pitch) + jitter(at, *pitch), best.1)
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
    fn the_rhythm_gives_every_syllable_an_eighth_and_the_last_a_quarter() {
        let rhythm = vocal_rhythm(&[3, 2], TimeSignature::default());
        let eighth = TICKS_PER_QUARTER / 2;
        assert_eq!(
            rhythm.phrases[0],
            [
                (Ticks(0), Ticks(eighth)),
                (Ticks(eighth), Ticks(eighth)),
                (Ticks(eighth * 2), Ticks(TICKS_PER_QUARTER)),
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
