//! A key: a tonic and the scale built on it.

use serde::{Deserialize, Serialize};

use super::pitch::PitchClass;
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

    /// How the key is written back out.
    pub fn to_text(self) -> String {
        let name = if self.flat_spelled() {
            self.tonic.flat_name()
        } else {
            self.tonic.sharp_name()
        };
        format!("{name} {}", self.scale.name())
    }

    /// Whether this key is conventionally written with flats.
    ///
    /// Only the accidental tonics are in question; the flat side of the circle of fifths is
    /// where a musician expects to read Bb rather than A#.
    fn flat_spelled(self) -> bool {
        matches!(self.tonic.semitones(), 1 | 3 | 8 | 10)
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
    use super::*;

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
    fn keys_are_written_the_way_a_musician_spells_them() {
        assert_eq!(Key::parse("Bb minor").unwrap().to_text(), "Bb minor");
        assert_eq!(Key::parse("Eb major").unwrap().to_text(), "Eb major");
        assert_eq!(Key::parse("F# minor").unwrap().to_text(), "F# minor");
        assert_eq!(Key::parse("C major").unwrap().to_text(), "C major");
    }
}
