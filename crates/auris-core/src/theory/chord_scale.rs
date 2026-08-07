//! The notes available over one chord.

use super::chord::Chord;
use super::key::Key;
use super::pitch::{OCTAVE, PitchClass};

/// The key's scale, with any degree the chord altered replaced by the chord's own note.
///
/// A chord that borrows — a secondary dominant, a raised leading tone, anything holding a note
/// the key does not have — does not merely *add* that note. It replaces the degree the note came
/// from, and a part that goes on playing the unaltered degree puts both versions of it in the
/// air at once. That is the one dissonance an ear hears as a mistake rather than as colour: 丸サ
/// 進行 in C minor plays a G7, and a melody drawn from the plain aeolian scale answers its B
/// natural with a B flat.
///
/// So the scale is built rather than looked up. In C minor a G7 gives `D Eb F G Ab B` — the
/// harmonic minor, with the tonic out of the way as well because C stands a semitone over that
/// same B. In C major a C7 gives `C D E F G Bb`, which is C mixolydian, and an E7 gives
/// `C D E F Ab B`. A chord with nothing borrowed in it gives the key's scale back unchanged,
/// which is the ordinary case and costs nothing.
///
/// Six notes rather than seven is not a loss: those *are* six-note scales. Two degrees standing
/// either side of one borrowed note both give way to it, which is exactly what happens when a
/// dominant arrives — the notes a semitone from its third are the notes nobody plays over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordScale {
    /// Semitones above the tonic, ascending and distinct.
    steps: Vec<i32>,
    /// What those semitones are counted from.
    tonic: PitchClass,
}

impl ChordScale {
    /// The scale `chord` implies in `key`.
    pub fn new(key: Key, chord: Chord) -> Self {
        let tonic = key.tonic;
        let scale = key.scale;
        let borrowed: Vec<i32> = chord
            .classes()
            .into_iter()
            .filter(|class| !scale.contains(tonic, *class))
            .map(|class| tonic.distance_up_to(class))
            .collect();

        let mut steps: Vec<i32> = Vec::with_capacity(8);
        for degree in 0..scale.degree_count() as i32 {
            let step = scale.semitone(degree).rem_euclid(OCTAVE);
            let sounded = borrowed
                .iter()
                .copied()
                .find(|tone| gap(step, *tone) == 1)
                .unwrap_or(step);
            if !steps.contains(&sounded) {
                steps.push(sounded);
            }
        }
        // A borrowed note no degree stood beside is added rather than dropped: it is still a
        // chord tone, and a part that may not play it would be avoiding the harmony.
        for tone in chord
            .classes()
            .into_iter()
            .map(|class| tonic.distance_up_to(class))
        {
            if !steps.contains(&tone) {
                steps.push(tone);
            }
        }
        steps.sort_unstable();
        Self { steps, tonic }
    }

    /// The key's own scale, for a stretch with no chord under it.
    pub fn of_key(key: Key) -> Self {
        Self {
            steps: (0..key.scale.degree_count() as i32)
                .map(|degree| key.scale.semitone(degree).rem_euclid(OCTAVE))
                .collect(),
            tonic: key.tonic,
        }
    }

    /// What the degrees are counted from.
    pub fn tonic(&self) -> PitchClass {
        self.tonic
    }

    /// How many notes it has to the octave.
    pub fn degree_count(&self) -> usize {
        self.steps.len()
    }

    /// Semitones above the tonic for a zero-based degree, which may run past the octave.
    pub fn semitone(&self, degree: i32) -> i32 {
        let count = self.steps.len() as i32;
        if count == 0 {
            return 0;
        }
        let index = degree.rem_euclid(count);
        let octaves = degree.div_euclid(count);
        self.steps[index as usize] + octaves * OCTAVE
    }

    /// The degree nearest to `semitones`, rounding down on a tie.
    ///
    /// The same walk [`ScaleId::nearest_degree`](super::scale::ScaleId::nearest_degree) does,
    /// and for the same reason: a pitch that is not in the scale has to land on one that is.
    pub fn nearest_degree(&self, semitones: i32) -> i32 {
        let count = self.steps.len() as i32;
        if count == 0 {
            return 0;
        }
        let within = semitones.rem_euclid(OCTAVE);
        let octaves = semitones.div_euclid(OCTAVE);
        let mut best = 0;
        let mut best_distance = i32::MAX;
        let mut best_pitch = i32::MAX;
        for (index, step) in self.steps.iter().enumerate() {
            // The next octave too, so B is nearer to the C above than to the A below.
            for candidate in [*step, step + OCTAVE] {
                let distance = (candidate - within).abs();
                // The lower pitch takes a tie, for the reason `ScaleId::nearest_degree` gives:
                // the octave above the tonic is reached first and used to win ties against the
                // degree a semitone below the query. This is the copy the melody actually calls.
                if distance < best_distance || (distance == best_distance && candidate < best_pitch)
                {
                    best_distance = distance;
                    best_pitch = candidate;
                    best = index as i32 + i32::from(candidate > within && *step < within) * count;
                }
            }
        }
        best + octaves * count
    }

    /// `true` when the scale holds this pitch class.
    pub fn contains(&self, class: PitchClass) -> bool {
        self.steps.contains(&self.tonic.distance_up_to(class))
    }

    /// The pitch classes, ascending from the tonic.
    pub fn classes(&self) -> Vec<PitchClass> {
        self.steps
            .iter()
            .map(|step| self.tonic.transposed(*step))
            .collect()
    }
}

/// The distance between two positions in an octave, the short way round.
fn gap(one: i32, other: i32) -> i32 {
    let apart = (one - other).rem_euclid(OCTAVE);
    apart.min(OCTAVE - apart)
}

#[cfg(test)]
mod tests {
    use super::super::chart::Chart;
    use super::super::numeral::Numeral;
    use super::*;

    fn scale_of(key_text: &str, numeral: &str) -> String {
        let key = Key::parse(key_text).unwrap();
        let chord = Numeral::parse(numeral).unwrap().chord_in(key);
        ChordScale::new(key, chord)
            .classes()
            .iter()
            .map(|class| class.name(key.spelling()).to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_chord_that_borrows_nothing_gives_the_key_its_scale_back() {
        // The ordinary case, and it has to cost nothing: most chords of most songs are diatonic.
        assert_eq!(scale_of("C major", "I"), "C D E F G A B");
        assert_eq!(scale_of("C major", "vi7"), "C D E F G A B");
        assert_eq!(scale_of("C minor", "i7"), "C D Eb F G Ab Bb");
        assert_eq!(scale_of("C minor", "iv"), "C D Eb F G Ab Bb");
    }

    #[test]
    fn a_borrowed_note_takes_the_degree_it_altered_with_it() {
        // The G7 of a minor-key 丸サ進行. Its B replaces the B flat a semitone under it — that
        // pair sounding together is what the melody used to do — and the tonic goes too, being
        // a semitone over the same B. What is left is the harmonic minor from D upward, which
        // is what anybody plays over a dominant in a minor key.
        assert_eq!(scale_of("C minor", "V7"), "D Eb F G Ab B");

        // A dominant seventh on the tonic is mixolydian, arrived at rather than named.
        assert_eq!(scale_of("C major", "I7"), "C D E F G A#");

        // And the secondary dominant of vi takes both notes that stand beside its third.
        assert_eq!(scale_of("C major", "III7"), "C D E F G# B");
    }

    #[test]
    fn every_note_of_the_chord_is_in_the_scale() {
        // A part told to stay inside this scale must never be shut out of the harmony itself.
        for key_text in ["C major", "C minor", "F# minor", "Eb major", "A dorian"] {
            let key = Key::parse(key_text).unwrap();
            for chart in ["@marusa", "@royal-road", "@junjo", "@blues", "@epic"] {
                let chart = Chart::parse(chart).unwrap();
                for event in chart.spelled_in(key).resolve(key, crate::time::Ticks(3840)) {
                    let scale = ChordScale::new(key, event.chord);
                    for class in event.chord.classes() {
                        assert!(
                            scale.contains(class),
                            "{class} of {} is missing in {key_text}",
                            event.chord
                        );
                    }
                    assert!(scale.degree_count() >= 5, "{} in {key_text}", event.chord);
                }
            }
        }
    }

    #[test]
    fn no_two_notes_of_the_scale_stand_a_semitone_apart_over_a_borrowed_note() {
        // The property the whole type exists for: whatever the chord borrows, the scale never
        // offers both versions of the degree it borrowed from.
        for key_text in ["C major", "C minor", "Bb major", "F# minor"] {
            let key = Key::parse(key_text).unwrap();
            for chart in ["@marusa", "@royal-road", "@naki", "@blues"] {
                let chart = Chart::parse(chart).unwrap();
                for event in chart.spelled_in(key).resolve(key, crate::time::Ticks(3840)) {
                    let scale = ChordScale::new(key, event.chord);
                    for class in event.chord.classes() {
                        // A chord tone the key does not have: nothing may sit beside it.
                        if key.scale.contains(key.tonic, class) {
                            continue;
                        }
                        for other in scale.classes() {
                            let apart = gap(
                                key.tonic.distance_up_to(class),
                                key.tonic.distance_up_to(other),
                            );
                            assert!(
                                apart != 1,
                                "{other} sits a semitone from {class} of {} in {key_text}",
                                event.chord
                            );
                        }
                    }
                }
            }
        }
    }
}
