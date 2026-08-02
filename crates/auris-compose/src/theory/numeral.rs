//! Roman numerals: a chord named by its position in a key rather than by its letter.
//!
//! This is what lets a progression be written once and played in any key, and it is the notation
//! the whole catalogue is stored in.

use std::fmt;

use super::chord::{Chord, Quality};
use super::key::Key;
use super::pitch::PitchClass;

/// A chord named by scale degree.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Numeral {
    /// One-based scale degree, as written: `1` for I, `5` for V.
    pub degree: u8,
    /// Semitone alteration of the degree, from a leading `b` or `#`.
    pub accidental: i32,
    /// `true` when the numeral was written in lower case.
    pub minor_case: bool,
    /// The quality written after the numeral, if any.
    ///
    /// `None` means "whatever the key makes it", which is what keeps a catalogue entry usable in
    /// major and minor alike.
    pub quality: Option<Quality>,
    /// The degree this chord is the dominant of, from a `/V` suffix.
    pub secondary_of: Option<u8>,
    /// A bass degree from a `/3`-style suffix, one-based.
    pub bass_degree: Option<u8>,
}

impl Numeral {
    /// A plain numeral on `degree`.
    pub fn new(degree: u8, minor_case: bool) -> Self {
        Self {
            degree,
            accidental: 0,
            minor_case,
            quality: None,
            secondary_of: None,
            bass_degree: None,
        }
    }

    /// `true` when no quality was written, so colouring is free to add one.
    ///
    /// A chord the user spelled out is never rewritten; that is what keeps a quoted progression
    /// sounding like the song it came from.
    pub fn is_colourable(self) -> bool {
        self.quality.is_none() && self.secondary_of.is_none()
    }

    /// The same numeral with `quality` written on it.
    pub fn with_quality(self, quality: Quality) -> Self {
        Self {
            quality: Some(quality),
            ..self
        }
    }

    /// Reads a numeral: `I`, `vi`, `bVII`, `V7`, `IVmaj7`, `V7/V`, `IV/5`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        // A trailing `/x` is either a secondary target (a numeral) or a bass degree (a digit).
        let (head, tail) = match text.rsplit_once('/') {
            Some((head, tail)) => (head, Some(tail)),
            None => (text, None),
        };
        let (secondary_of, bass_degree) = match tail {
            None => (None, None),
            Some(tail) if tail.chars().all(|c| c.is_ascii_digit()) => {
                (None, Some(tail.parse::<u8>().ok()?))
            }
            Some(tail) => (
                Some(roman_degree(tail.trim_start_matches(['b', '#']))?),
                None,
            ),
        };

        let mut rest = head;
        let mut accidental = 0;
        while let Some(stripped) = rest.strip_prefix('b') {
            accidental -= 1;
            rest = stripped;
        }
        while let Some(stripped) = rest.strip_prefix('#') {
            accidental += 1;
            rest = stripped;
        }

        // The numeral is the longest run of roman letters; whatever follows is the quality.
        let split = rest
            .find(|c: char| !matches!(c, 'i' | 'I' | 'v' | 'V'))
            .unwrap_or(rest.len());
        let (numeral, quality_text) = rest.split_at(split);
        let degree = roman_degree(numeral)?;
        let minor_case = numeral.chars().all(|c| c.is_lowercase());
        // A bare arabic number takes its third from the numeral's case, which is the convention
        // every chart uses: `V7` is a dominant seventh and `vi7` is a minor one. A major seventh
        // has to be written out as `Imaj7`, because `I7` means the dominant.
        let quality = match (quality_text, minor_case) {
            ("", _) => None,
            ("7", true) => Some(Quality::Minor7),
            ("9", true) => Some(Quality::Minor9),
            ("6", true) => Some(Quality::Minor6),
            (text, _) => Some(Quality::parse(text)?),
        };

        Some(Self {
            degree,
            accidental,
            minor_case,
            quality,
            secondary_of,
            bass_degree,
        })
    }

    /// The chord this numeral means in `key`.
    pub fn chord_in(self, key: Key) -> Chord {
        if let Some(target) = self.secondary_of {
            // The dominant of a degree: a major-minor seventh a fifth above that degree's root.
            let target_root = degree_class(key, target, 0);
            return Chord::new(target_root.transposed(7), Quality::Dominant7);
        }

        let root = degree_class(key, self.degree, self.accidental);
        let quality = self.quality.unwrap_or_else(|| {
            let diatonic = diatonic_quality(key, self.degree);
            // Case only speaks when it disagrees with the key: writing `IV` in a minor key is how
            // a borrowed major subdominant is asked for.
            if self.accidental != 0 || diatonic.is_minor() != self.minor_case {
                if self.minor_case {
                    Quality::Minor
                } else {
                    Quality::Major
                }
            } else {
                diatonic
            }
        });

        let chord = Chord::new(root, quality);
        match self.bass_degree {
            Some(bass) => chord.over(degree_class(key, bass, 0)),
            None => chord,
        }
    }
}

impl fmt::Display for Numeral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.accidental.abs() {
            f.write_str(if self.accidental < 0 { "b" } else { "#" })?;
        }
        let roman = ROMAN[(self.degree.clamp(1, 7) - 1) as usize];
        if self.minor_case {
            write!(f, "{}", roman.to_lowercase())?;
        } else {
            f.write_str(roman)?;
        }
        // Write the bare number back when the quality is the one the case already implies, so
        // `vi7` does not come back as `vim7`.
        if let Some(quality) = self.quality {
            let bare = match (quality, self.minor_case) {
                (Quality::Minor7, true) | (Quality::Dominant7, false) => Some("7"),
                (Quality::Minor9, true) | (Quality::Dominant9, false) => Some("9"),
                (Quality::Minor6, true) | (Quality::Major6, false) => Some("6"),
                _ => None,
            };
            f.write_str(bare.unwrap_or_else(|| quality.suffix()))?;
        }
        if let Some(target) = self.secondary_of {
            write!(f, "/{}", ROMAN[(target.clamp(1, 7) - 1) as usize])?;
        }
        if let Some(bass) = self.bass_degree {
            write!(f, "/{bass}")?;
        }
        Ok(())
    }
}

/// The roman numerals, in upper case, indexed by degree minus one.
const ROMAN: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

/// The one-based degree a roman numeral names.
fn roman_degree(text: &str) -> Option<u8> {
    Some(match text.to_ascii_uppercase().as_str() {
        "I" => 1,
        "II" => 2,
        "III" => 3,
        "IV" => 4,
        "V" => 5,
        "VI" => 6,
        "VII" => 7,
        _ => return None,
    })
}

/// The pitch class of a one-based degree in `key`, altered by `accidental` semitones.
///
/// An altered degree is measured from the **major** scale, which is the convention roman numeral
/// analysis uses: `bVI` means the same note in C major and in C minor, and in a minor key — where
/// the sixth is already flat — it must not be flattened twice.
fn degree_class(key: Key, degree: u8, accidental: i32) -> PitchClass {
    let index = i32::from(degree.clamp(1, 7)) - 1;
    if accidental == 0 {
        key.class(index)
    } else {
        let major = Key::new(key.tonic, super::scale::ScaleId::Major);
        major.class(index).transposed(accidental)
    }
}

/// The triad the key itself builds on `degree`, by stacking scale thirds.
///
/// Derived rather than tabulated so that harmonic minor gets its major dominant and its
/// diminished seventh degree without a special case, and so a mode gets whatever it gets.
pub fn diatonic_quality(key: Key, degree: u8) -> Quality {
    let base = i32::from(degree.clamp(1, 7)) - 1;
    let root = key.semitone(base);
    let third = key.semitone(base + 2) - root;
    let fifth = key.semitone(base + 4) - root;
    match (third, fifth) {
        (4, 7) => Quality::Major,
        (3, 7) => Quality::Minor,
        (3, 6) => Quality::Diminished,
        (4, 8) => Quality::Augmented,
        (2, 7) => Quality::Sus2,
        (5, 7) => Quality::Sus4,
        // A scale with fewer than seven degrees, or an exotic one: the third decides.
        (third, _) if third <= 3 => Quality::Minor,
        _ => Quality::Major,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    fn chord_of(numeral: &str, key_text: &str) -> String {
        Numeral::parse(numeral)
            .unwrap()
            .chord_in(key(key_text))
            .to_string()
    }

    #[test]
    fn the_diatonic_triads_of_a_major_key_are_the_ones_everybody_learns() {
        // I ii iii IV V vi vii°
        let expected = ["C", "Dm", "Em", "F", "G", "Am", "Bdim"];
        let numerals = ["I", "ii", "iii", "IV", "V", "vi", "vii"];
        for (numeral, chord) in numerals.iter().zip(expected) {
            assert_eq!(chord_of(numeral, "C major"), chord, "{numeral}");
        }
    }

    #[test]
    fn the_diatonic_triads_of_a_minor_key_are_the_ones_everybody_learns() {
        // i ii° III iv v VI VII
        let expected = ["Am", "Bdim", "C", "Dm", "Em", "F", "G"];
        let numerals = ["i", "ii", "III", "iv", "v", "VI", "VII"];
        for (numeral, chord) in numerals.iter().zip(expected) {
            assert_eq!(chord_of(numeral, "A minor"), chord, "{numeral}");
        }
    }

    #[test]
    fn harmonic_minor_has_a_major_dominant_and_a_diminished_seventh() {
        assert_eq!(chord_of("V", "A harmonic-minor"), "E");
        assert_eq!(chord_of("vii", "A harmonic-minor"), "G#dim");
        assert_eq!(chord_of("i", "A harmonic-minor"), "Am");
    }

    #[test]
    fn case_asks_for_a_chord_the_key_does_not_have() {
        // The borrowed major IV in a minor key, and the borrowed minor iv in a major key.
        assert_eq!(chord_of("IV", "A minor"), "D");
        assert_eq!(chord_of("iv", "C major"), "Fm");
        // A written quality always wins over both.
        assert_eq!(chord_of("IVmaj7", "C major"), "Fmaj7");
        assert_eq!(
            chord_of("ii7", "C major"),
            "Dm7",
            "a lower-case seven is a minor seventh"
        );
    }

    #[test]
    fn accidentals_move_the_root_and_take_the_case_at_face_value() {
        assert_eq!(chord_of("bVII", "C major"), "A#", "the flat seven");
        assert_eq!(chord_of("bVI", "C major"), "G#");
        assert_eq!(chord_of("bII", "C major"), "C#", "the Neapolitan");
        assert_eq!(chord_of("#iv", "C major"), "F#m");
    }

    #[test]
    fn an_accidental_is_measured_from_the_major_scale() {
        // bVI, bIII and bVII name the same notes in a key and in its parallel: in a minor key
        // those degrees are already flat, and flattening them again would be a semitone out.
        assert_eq!(chord_of("bVI", "A minor"), "F");
        assert_eq!(chord_of("bIII", "A minor"), "C");
        assert_eq!(chord_of("bVII", "A minor"), "G");
        assert_eq!(chord_of("bVI", "A major"), "F");
        assert_eq!(chord_of("bIII", "A major"), "C");
        // An unaltered numeral still comes from the key's own scale.
        assert_eq!(chord_of("VI", "A minor"), "F");
        assert_eq!(chord_of("vi", "A major"), "F#m");
    }

    #[test]
    fn a_secondary_dominant_is_the_dominant_of_its_target() {
        // V/V in C is D7, the dominant of G.
        assert_eq!(chord_of("V/V", "C major"), "D7");
        assert_eq!(chord_of("V7/V", "C major"), "D7");
        assert_eq!(chord_of("V/vi", "C major"), "E7", "the dominant of A minor");
        assert_eq!(chord_of("V/IV", "C major"), "C7");
    }

    #[test]
    fn a_slash_digit_is_a_bass_degree() {
        // The IV chord over the fifth of the key: the pedal of the 小悪魔 progression.
        let chord = Numeral::parse("IV/5").unwrap().chord_in(key("C major"));
        assert_eq!(chord.root.sharp_name(), "F");
        assert_eq!(chord.bass_class().sharp_name(), "G");
        assert_eq!(chord.to_string(), "F/G");
    }

    #[test]
    fn numerals_round_trip_through_their_text() {
        for text in ["I", "vi", "bVII", "V7", "IVmaj7", "iim7b5", "#iv"] {
            let numeral = Numeral::parse(text).unwrap();
            assert_eq!(numeral.to_string(), text, "{text} did not round trip");
        }
    }

    #[test]
    fn nonsense_is_rejected() {
        assert!(Numeral::parse("").is_none());
        assert!(Numeral::parse("VIII").is_none());
        assert!(Numeral::parse("X").is_none());
        assert!(Numeral::parse("Iwhat").is_none());
        assert!(Numeral::parse("V/X").is_none());
    }

    #[test]
    fn only_a_bare_numeral_may_be_coloured() {
        assert!(Numeral::parse("IV").unwrap().is_colourable());
        assert!(Numeral::parse("vi").unwrap().is_colourable());
        assert!(
            !Numeral::parse("IVmaj7").unwrap().is_colourable(),
            "a written quality is the user's decision"
        );
        assert!(!Numeral::parse("V7/V").unwrap().is_colourable());
    }

    #[test]
    fn a_bare_seven_takes_its_third_from_the_case() {
        // The convention every chart uses: V7 is a dominant, vi7 is a minor seventh, and a
        // major seventh has to say so.
        assert_eq!(chord_of("V7", "C major"), "G7");
        assert_eq!(chord_of("I7", "C major"), "C7");
        assert_eq!(chord_of("vi7", "C major"), "Am7");
        assert_eq!(chord_of("ii7", "C major"), "Dm7");
        assert_eq!(chord_of("iii7", "C major"), "Em7");
        assert_eq!(chord_of("Imaj7", "C major"), "Cmaj7");
        assert_eq!(chord_of("vi9", "C major"), "Am9");
        assert_eq!(chord_of("I6", "C major"), "C6");
        assert_eq!(chord_of("vi6", "C major"), "Am6");
    }

    #[test]
    fn the_marusa_progression_spells_out_correctly() {
        // IVM7 III7 vi I7 in C: the Just-the-Two-of-Us / 丸サ progression.
        let chords: Vec<String> = ["IVmaj7", "III7", "vi7", "I7"]
            .iter()
            .map(|numeral| chord_of(numeral, "C major"))
            .collect();
        assert_eq!(chords, ["Fmaj7", "E7", "Am7", "C7"]);
    }
}
