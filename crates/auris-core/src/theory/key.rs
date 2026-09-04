//! A key: a tonic and the scale built on it.

use serde::{Deserialize, Serialize};

use super::pitch::{OCTAVE, PitchClass, Spelling};
use super::scale::ScaleId;

/// Where the music is centred.
///
/// Serialises as the text a musician writes — `"Bb minor"` — rather than as a pair of fields,
/// so the JSON form and the text form spell a key the same way.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Key {
    /// The note everything resolves to.
    pub tonic: PitchClass,
    /// The scale built on it.
    pub scale: ScaleId,
}

impl Key {
    /// A key from its parts.
    pub fn new(tonic: PitchClass, scale: ScaleId) -> Self {
        Self { tonic, scale }
    }

    /// Reads `C major`, `F# minor`, `Bb dorian` or `Eb`, where a bare letter means major.
    ///
    /// The scale word is optional because a chord chart usually writes only the letter, and
    /// major is what everyone means by that.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (tonic_text, scale_text) = match text.split_once(char::is_whitespace) {
            Some((tonic, rest)) => (tonic, rest.trim()),
            None => (text, ""),
        };
        let tonic = PitchClass::parse(tonic_text)?;
        let scale = if scale_text.is_empty() {
            ScaleId::Major
        } else {
            ScaleId::parse(scale_text)?
        };
        Some(Self::new(tonic, scale))
    }

    /// Semitones above the tonic for a zero-based scale degree.
    pub fn semitone(self, degree: i32) -> i32 {
        self.scale.semitone(degree)
    }

    /// The pitch class of a zero-based scale degree.
    pub fn class(self, degree: i32) -> PitchClass {
        self.tonic.transposed(self.semitone(degree))
    }

    /// The MIDI pitch of a zero-based scale degree in `octave`.
    pub fn midi(self, degree: i32, octave: i32) -> i32 {
        self.tonic.midi(octave) + self.semitone(degree)
    }

    /// This key moved by `semitones`, keeping its scale.
    pub fn transposed(self, semitones: i32) -> Self {
        Self::new(self.tonic.transposed(semitones), self.scale)
    }

    /// The parallel key: the same tonic with the mode flipped.
    ///
    /// This is where borrowed chords come from — the iv in a major key is the parallel minor's.
    pub fn parallel(self) -> Self {
        let scale = if self.scale.is_minor() {
            ScaleId::Major
        } else {
            ScaleId::Minor
        };
        Self::new(self.tonic, scale)
    }

    /// `true` when the tonic triad is minor.
    pub fn is_minor(self) -> bool {
        self.scale.is_minor()
    }

    /// The key with the same notes and the other mode.
    ///
    /// A minor key's relative major is a minor third above it, a major key's relative minor a
    /// minor third below. This is where a progression written in one mode goes to be read in the
    /// other: 丸サ進行 asked for in C minor is the loop centred on C minor, which is the loop of
    /// E flat major, and not its own degrees read against an aeolian scale.
    ///
    /// A mode is treated as major or minor by its third — the same approximation
    /// [`Self::spelling`] makes, and for the same reason: it only ever picks a direction.
    pub fn relative(self) -> Self {
        if self.is_minor() {
            Self::new(self.tonic.transposed(3), ScaleId::Major)
        } else {
            Self::new(self.tonic.transposed(-3), ScaleId::Minor)
        }
    }

    /// How the key is written back out.
    pub fn to_text(self) -> String {
        format!("{} {}", self.tonic.name(self.spelling()), self.scale.name())
    }

    /// Which way this key writes the notes that need an accidental.
    ///
    /// Decided by where the key sits on the circle of fifths, and so by the whole key rather than
    /// by its tonic's own name: the IV of F major is B flat, though F itself takes no accidental
    /// at all. Whichever direction needs fewer accidentals wins.
    ///
    /// The two directions tie at six, where both spellings are in real use and the mode decides:
    /// six sharps is F sharp major, six flats is E flat minor, and both are what a musician
    /// writes. A minor key takes its signature from the relative major a minor third above, which
    /// is why C sharp minor is four sharps and not eight flats.
    ///
    /// The church modes take their signature from the major scale whose notes they share. Scales
    /// that do not have such a parent use the major/minor side implied by their third.
    pub fn spelling(self) -> Spelling {
        let relative_major = match self.scale {
            ScaleId::Major | ScaleId::MajorPentatonic | ScaleId::WholeTone => self.tonic,
            ScaleId::Minor
            | ScaleId::HarmonicMinor
            | ScaleId::MelodicMinor
            | ScaleId::MinorPentatonic
            | ScaleId::Blues => self.tonic.transposed(3),
            ScaleId::Dorian => self.tonic.transposed(-2),
            ScaleId::Phrygian => self.tonic.transposed(-4),
            ScaleId::Lydian => self.tonic.transposed(-5),
            ScaleId::Mixolydian => self.tonic.transposed(-7),
            ScaleId::Locrian => self.tonic.transposed(1),
        };
        // Each step up a fifth adds a sharp, so the number of sharps is the tonic's position on
        // the circle of fifths — and since seven fifths make an octave plus a semitone, a fifth is
        // its own inverse modulo twelve.
        let sharps = (relative_major.semitones() * 7).rem_euclid(OCTAVE);
        let flats = (OCTAVE - sharps) % OCTAVE;
        if flats < sharps || (flats == sharps && self.is_minor()) {
            Spelling::Flats
        } else {
            Spelling::Sharps
        }
    }
}

impl From<Key> for String {
    fn from(key: Key) -> Self {
        key.to_text()
    }
}

impl TryFrom<String> for Key {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Key::parse(&text).ok_or_else(|| format!("`{text}` is not a key"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::pitch::Spelling;
    use super::*;

    #[test]
    fn the_relative_key_has_the_same_notes_and_the_other_mode() {
        let relative = |text: &str| Key::parse(text).unwrap().relative().to_text();
        assert_eq!(relative("C minor"), "Eb major");
        assert_eq!(relative("A minor"), "C major");
        assert_eq!(relative("C major"), "A minor");
        assert_eq!(relative("Eb major"), "C minor");
        // There and back again, for every key there is.
        for tonic in 0..12 {
            for scale in [ScaleId::Major, ScaleId::Minor] {
                let key = Key::new(PitchClass::new(tonic), scale);
                assert_eq!(key.relative().relative(), key, "{}", key.to_text());
                assert_ne!(key.is_minor(), key.relative().is_minor());
            }
        }
    }

    #[test]
    fn a_key_can_be_written_with_or_without_its_mode() {
        assert_eq!(
            Key::parse("C major"),
            Some(Key::new(PitchClass::new(0), ScaleId::Major))
        );
        assert_eq!(
            Key::parse("Eb"),
            Some(Key::new(PitchClass::new(3), ScaleId::Major)),
            "a bare letter is major"
        );
        assert_eq!(
            Key::parse("f# minor"),
            Some(Key::new(PitchClass::new(6), ScaleId::Minor))
        );
        assert_eq!(
            Key::parse("Bb dorian"),
            Some(Key::new(PitchClass::new(10), ScaleId::Dorian))
        );
        assert_eq!(Key::parse("C sideways"), None);
        assert_eq!(Key::parse("H minor"), None);
    }

    #[test]
    fn degrees_land_on_the_notes_of_the_key() {
        let a_minor = Key::parse("A minor").unwrap();
        for (degree, name) in ["A", "B", "C", "D", "E", "F", "G"].iter().enumerate() {
            assert_eq!(a_minor.class(degree as i32).sharp_name(), *name);
        }

        let d_major = Key::parse("D major").unwrap();
        assert_eq!(d_major.class(2).sharp_name(), "F#");
        assert_eq!(d_major.class(6).sharp_name(), "C#");
    }

    #[test]
    fn midi_pitches_are_absolute() {
        let c_major = Key::parse("C major").unwrap();
        assert_eq!(c_major.midi(0, 4), 60, "middle C");
        assert_eq!(c_major.midi(4, 4), 67, "the G above it");
        assert_eq!(c_major.midi(7, 4), 72, "the octave");
        assert_eq!(c_major.midi(0, 3), 48);
    }

    #[test]
    fn the_parallel_key_keeps_its_tonic_and_flips_its_mode() {
        let c_major = Key::parse("C major").unwrap();
        let c_minor = c_major.parallel();
        assert_eq!(c_minor.tonic, c_major.tonic);
        assert!(c_minor.is_minor());
        assert_eq!(c_minor.parallel().scale, ScaleId::Major);

        // A mode flips to the plain major or minor rather than to another mode.
        assert_eq!(
            Key::parse("D dorian").unwrap().parallel().scale,
            ScaleId::Major
        );
    }

    #[test]
    fn transposing_moves_the_tonic_and_nothing_else() {
        let up = Key::parse("C minor").unwrap().transposed(3);
        assert_eq!(up.tonic.sharp_name(), "D#");
        assert_eq!(up.scale, ScaleId::Minor);
    }

    #[test]
    fn a_key_round_trips_through_json_as_text() {
        let key = Key::parse("Bb minor").unwrap();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"Bb minor\"");
        assert_eq!(serde_json::from_str::<Key>(&json).unwrap(), key);
        assert!(serde_json::from_str::<Key>("\"H major\"").is_err());
    }

    #[test]
    fn a_key_is_spelled_by_where_it_sits_on_the_circle_of_fifths() {
        // Whichever direction needs fewer accidentals wins. The tonic's own name does not decide
        // it: F major takes no accidental at all and is still a flat key, which is what makes its
        // fourth degree a B flat rather than an A sharp.
        assert_eq!(Key::parse("F major").unwrap().spelling(), Spelling::Flats);
        assert_eq!(Key::parse("G major").unwrap().spelling(), Spelling::Sharps);
        assert_eq!(Key::parse("Eb major").unwrap().spelling(), Spelling::Flats);
        assert_eq!(Key::parse("E major").unwrap().spelling(), Spelling::Sharps);

        // A minor key takes its signature from the relative major a minor third above, which is
        // why C sharp minor is four sharps and not the eight flats of "D flat minor".
        assert_eq!(Key::parse("C# minor").unwrap().to_text(), "C# minor");
        assert_eq!(Key::parse("G# minor").unwrap().to_text(), "G# minor");
        assert_eq!(Key::parse("Bb minor").unwrap().to_text(), "Bb minor");

        // Six of each is a real tie, and the mode breaks it the way a musician does.
        assert_eq!(Key::parse("F# major").unwrap().to_text(), "F# major");
        assert_eq!(Key::parse("Eb minor").unwrap().to_text(), "Eb minor");

        // Modes use the major collection they actually share, not merely the side implied by
        // their third: D# Phrygian belongs to B major, C Mixolydian to F major.
        assert_eq!(Key::parse("D# phrygian").unwrap().to_text(), "D# phrygian");
        assert_eq!(
            Key::parse("C mixolydian").unwrap().spelling(),
            Spelling::Flats
        );
    }

    #[test]
    fn every_key_writes_a_name_that_reads_back_as_itself() {
        for tonic in 0..12 {
            for scale in ScaleId::ALL {
                let key = Key::new(PitchClass::new(tonic), scale);
                let text = key.to_text();
                assert_eq!(
                    Key::parse(&text),
                    Some(key),
                    "`{text}` did not read back as what wrote it"
                );
            }
        }
    }

    #[test]
    fn keys_are_written_the_way_a_musician_spells_them() {
        assert_eq!(Key::parse("Bb minor").unwrap().to_text(), "Bb minor");
        assert_eq!(Key::parse("Eb major").unwrap().to_text(), "Eb major");
        assert_eq!(Key::parse("F# minor").unwrap().to_text(), "F# minor");
        assert_eq!(Key::parse("C major").unwrap().to_text(), "C major");
    }
}
