//! Pitch classes, note names and MIDI numbers.
//!
//! Everything downstream works in semitones from a tonic, so this is the only module that knows
//! what a letter name means.

use std::fmt;

/// A pitch class: one of the twelve notes, with no octave.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PitchClass(u8);

/// Semitones in an octave.
pub const OCTAVE: i32 = 12;

/// The MIDI number of middle C, which is C4 in scientific pitch notation.
pub const MIDDLE_C: i32 = 60;

impl PitchClass {
    /// The pitch class `semitones` above C, wrapping.
    pub fn new(semitones: i32) -> Self {
        Self(semitones.rem_euclid(OCTAVE) as u8)
    }

    /// Semitones above C, always in `0..12`.
    pub fn semitones(self) -> i32 {
        i32::from(self.0)
    }

    /// This class transposed by `semitones`.
    pub fn transposed(self, semitones: i32) -> Self {
        Self::new(self.semitones() + semitones)
    }

    /// The shortest distance up from `self` to `other`, in `0..12`.
    pub fn distance_up_to(self, other: Self) -> i32 {
        (other.semitones() - self.semitones()).rem_euclid(OCTAVE)
    }

    /// The MIDI number of this class in `octave`, where octave 4 holds middle C.
    pub fn midi(self, octave: i32) -> i32 {
        (octave + 1) * OCTAVE + self.semitones()
    }

    /// The name a key signature would write, preferring sharps.
    pub fn sharp_name(self) -> &'static str {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        NAMES[self.0 as usize]
    }

    /// The name a key signature would write, preferring flats.
    pub fn flat_name(self) -> &'static str {
        const NAMES: [&str; 12] = [
            "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
        ];
        NAMES[self.0 as usize]
    }

    /// Reads a note name: a letter, then any number of `#`/`b`/`♯`/`♭`.
    ///
    /// Case-insensitive on the letter, because a chord chart is typed in a hurry.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        let letter = chars.next()?;
        let base = match letter.to_ascii_uppercase() {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        let mut accidental = 0;
        for mark in chars {
            match mark {
                '#' | '♯' => accidental += 1,
                'b' | '♭' => accidental -= 1,
                _ => return None,
            }
        }
        Some(Self::new(base + accidental))
    }
}

impl fmt::Display for PitchClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.sharp_name())
    }
}

/// The name of a MIDI pitch in scientific notation, such as `C4` or `F#2`.
pub fn midi_name(midi: i32) -> String {
    let class = PitchClass::new(midi);
    let octave = midi.div_euclid(OCTAVE) - 1;
    format!("{}{octave}", class.sharp_name())
}

/// Folds `midi` into `low..=high` by octaves, staying as close to where it started as possible.
///
/// Returns the nearest edge when the window is narrower than an octave, since no octave of the
/// pitch can fit inside it.
pub fn fold_into(midi: i32, low: i32, high: i32) -> i32 {
    if low > high {
        return midi;
    }
    let mut folded = midi;
    while folded < low {
        folded += OCTAVE;
    }
    while folded > high {
        folded -= OCTAVE;
    }
    // One octave down may have overshot the bottom of a window narrower than an octave.
    if folded < low {
        if high - midi.rem_euclid(OCTAVE) >= low {
            return folded + OCTAVE;
        }
        return low;
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_map_to_the_right_semitones() {
        assert_eq!(PitchClass::parse("C").unwrap().semitones(), 0);
        assert_eq!(PitchClass::parse("E").unwrap().semitones(), 4);
        assert_eq!(PitchClass::parse("F#").unwrap().semitones(), 6);
        assert_eq!(PitchClass::parse("Gb").unwrap().semitones(), 6);
        assert_eq!(
            PitchClass::parse("B#").unwrap().semitones(),
            0,
            "wraps the octave"
        );
        assert_eq!(PitchClass::parse("Cb").unwrap().semitones(), 11);
        assert_eq!(PitchClass::parse("Bbb").unwrap().semitones(), 9);
        assert_eq!(PitchClass::parse("c").unwrap().semitones(), 0);
        assert!(PitchClass::parse("H").is_none());
        assert!(PitchClass::parse("C$").is_none());
        assert!(PitchClass::parse("").is_none());
    }

    #[test]
    fn middle_c_is_where_everyone_agrees_it_is() {
        assert_eq!(PitchClass::parse("C").unwrap().midi(4), MIDDLE_C);
        assert_eq!(midi_name(MIDDLE_C), "C4");
        assert_eq!(midi_name(69), "A4", "concert A");
        assert_eq!(midi_name(21), "A0", "the bottom of a piano");
        assert_eq!(midi_name(0), "C-1");
    }

    #[test]
    fn distance_up_is_measured_the_short_way_round() {
        let c = PitchClass::new(0);
        let a = PitchClass::new(9);
        assert_eq!(c.distance_up_to(a), 9);
        assert_eq!(a.distance_up_to(c), 3, "up from A to C is a minor third");
        assert_eq!(c.distance_up_to(c), 0);
    }

    #[test]
    fn folding_lands_in_the_window_at_the_nearest_octave() {
        // C1 folded into a treble window arrives as C4, not C7.
        assert_eq!(fold_into(24, 55, 79), 60);
        assert_eq!(fold_into(96, 55, 79), 72);
        assert_eq!(fold_into(60, 55, 79), 60, "already inside, so untouched");
        assert_eq!(fold_into(55, 55, 79), 55, "the edges are inside");
        assert_eq!(fold_into(79, 55, 79), 79);
    }

    #[test]
    fn folding_into_an_impossible_window_gives_an_edge_rather_than_a_loop() {
        // A window narrower than an octave may contain no octave of the pitch at all.
        let folded = fold_into(61, 60, 64);
        assert!((60..=64).contains(&folded), "{folded} escaped the window");
        let hopeless = fold_into(66, 60, 64);
        assert!(
            (60..=64).contains(&hopeless),
            "{hopeless} escaped the window"
        );
    }

    #[test]
    fn transposing_wraps_within_the_octave() {
        let b = PitchClass::new(11);
        assert_eq!(b.transposed(1).semitones(), 0);
        assert_eq!(b.transposed(-12).semitones(), 11);
        assert_eq!(b.transposed(-23).semitones(), 0);
    }
}
