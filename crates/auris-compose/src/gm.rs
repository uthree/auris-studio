//! General MIDI: the hundred and twenty-eight programs, and the percussion kits.
//!
//! A [`PartSpec`](crate::PartSpec) names its instrument as a plugin id, which is enough while the
//! only instruments are the built-in ones. A SoundFont's sounds have no plugin ids — they are a
//! bank and a patch inside one particular file — so a specification that wanted a violin had no
//! way to say so.
//!
//! General MIDI is the vocabulary that solves it. It is the one agreement every SoundFont keeps:
//! program 40 is a violin in every file that claims to be General MIDI, and the shipped font is
//! one. So a part says `program = "Violin"`, the composer carries the number, and whichever font
//! is installed answers it.
//!
//! # Drums are the same field read differently
//!
//! Percussion is not a program in the way a violin is. The *patch* selects a whole kit and the
//! **note number** selects the drum — 36 is a kick, 38 a snare — which is the opposite way round
//! from everything else. [`DRUM_BANK`] is where those patches live, [`KITS`] is what the patch
//! numbers mean, and the note comes from
//! [`PartSpec::drum_note`](crate::PartSpec::drum_note), which was already General MIDI's.
//!
//! So one field on a part covers both: on a pitched role it is a program, on a drum role it is a
//! kit. Which one it is read as is never a guess, because the role already says.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// The bank General MIDI percussion patches live in.
///
/// SoundFont's convention rather than MIDI's — a `.sf2` puts the kits in bank 128, where the MIDI
/// wire protocol uses channel 10 and no bank at all. The document speaks in banks and patches
/// because that is what a font is addressed by.
pub const DRUM_BANK: u8 = 128;

/// The percussion kits General MIDI defines, by the patch number that selects each one.
///
/// Not one per number: the kits are scattered across the range and everything between them is the
/// standard kit again, which is a list nobody could read. These are the eight that mean something.
pub const KITS: [(u8, &str); 8] = [
    (0, "Standard Kit"),
    (8, "Room Kit"),
    (16, "Power Kit"),
    (24, "Electronic Kit"),
    (25, "TR-808 Kit"),
    (32, "Jazz Kit"),
    (40, "Brush Kit"),
    (48, "Orchestra Kit"),
];

/// How many programs a family holds. General MIDI's own grouping, and it is exact.
pub const FAMILY_SIZE: usize = 8;

/// The sixteen families General MIDI groups its programs into, eight to a family.
///
/// Not decoration: a hundred and twenty-eight names is a menu nobody can read, and this is the
/// division the standard already made — so a picker that groups by it groups the way every chart
/// and every hardware panel a musician has ever seen already does.
pub const FAMILIES: [&str; 16] = [
    "Piano",
    "Chromatic Percussion",
    "Organ",
    "Guitar",
    "Bass",
    "Strings",
    "Ensemble",
    "Brass",
    "Reed",
    "Pipe",
    "Synth Lead",
    "Synth Pad",
    "Synth Effects",
    "Ethnic",
    "Percussive",
    "Sound Effects",
];

/// The hundred and twenty-eight General MIDI program names, in program order.
pub const PROGRAMS: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavi",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bagpipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// Where a sound sits inside a General MIDI SoundFont.
///
/// A bank and a patch rather than a font and a preset, because the composer has no font: which
/// file answers this is the session's business, and every General MIDI font answers it the same.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    /// Which bank — 0 for the pitched set, [`DRUM_BANK`] for percussion.
    pub bank: u8,
    /// Which patch within it.
    pub patch: u8,
}

/// One General MIDI sound: a program on a pitched part, a kit on a drum one.
///
/// A number in a newtype rather than a bare `u8`, because the two readings are different
/// vocabularies and a bare number would let a kit be written where a program belongs without
/// anything noticing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Program(pub u8);

impl Program {
    /// The lowest program, which is a grand piano — and the standard kit read as percussion.
    pub const FIRST: Program = Program(0);

    /// What General MIDI calls this program.
    pub fn name(self) -> &'static str {
        PROGRAMS
            .get(usize::from(self.0))
            .copied()
            // Unreachable through `parse`, which refuses anything past 127. Reachable through the
            // public field, and a name is not worth a panic.
            .unwrap_or("Acoustic Grand Piano")
    }

    /// What General MIDI calls this percussion patch.
    ///
    /// The nearest kit at or below it, because the numbers between two kits are the lower one
    /// again — a font is free to fill them in and almost none do.
    pub fn kit_name(self) -> &'static str {
        KITS.iter()
            .rev()
            .find(|(patch, _)| *patch <= self.0)
            .map_or("Standard Kit", |(_, name)| *name)
    }

    /// Which family this program belongs to, as a position in [`FAMILIES`].
    pub fn family(self) -> usize {
        usize::from(self.0) / FAMILY_SIZE
    }

    /// Every program in one family, in program order.
    ///
    /// Empty for a position past the end of [`FAMILIES`], which is a caller's mistake and not
    /// worth a panic in a menu builder.
    pub fn family_programs(family: usize) -> Vec<Program> {
        if family >= FAMILIES.len() {
            return Vec::new();
        }
        let first = family * FAMILY_SIZE;
        (first..first + FAMILY_SIZE)
            .map(|number| Program(number as u8))
            .collect()
    }

    /// What to call this number for a part of this kind: a kit for a drum part, a program for
    /// every other.
    pub fn label(self, drums: bool) -> &'static str {
        match drums {
            true => self.kit_name(),
            false => self.name(),
        }
    }

    /// Where this program lives in a font, read as a kit when `drums` and a program otherwise.
    ///
    /// The one place the two readings are chosen between, so nothing else has to remember that a
    /// drum part's number means something entirely different from every other part's.
    pub fn sound(self, drums: bool) -> Sound {
        Sound {
            bank: if drums { DRUM_BANK } else { 0 },
            patch: self.0,
        }
    }

    /// Reads a program from a name or a number.
    ///
    /// The name is matched with case, spaces and punctuation ignored, so `Electric Piano 1`,
    /// `electric-piano-1` and `electricpiano1` are one sound. A number is taken as written, which
    /// is what a person reading a MIDI implementation chart has in front of them — and is
    /// zero-based, like everything else here and unlike the one-based tables printed on hardware.
    pub fn parse(text: &str) -> Option<Program> {
        let text = text.trim();
        if let Ok(number) = text.parse::<u16>() {
            return (number < 128).then_some(Program(number as u8));
        }
        let wanted = squashed(text);
        if wanted.is_empty() {
            return None;
        }
        PROGRAMS
            .iter()
            .position(|name| squashed(name) == wanted)
            .map(|index| Program(index as u8))
            .or_else(|| {
                KITS.iter()
                    .find(|(_, name)| squashed(name) == wanted)
                    .map(|(patch, _)| Program(*patch))
            })
    }
}

/// A name with everything that is not a letter or a digit taken out, lowercased.
fn squashed(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for Program {
    /// As its name, because a specification is meant to be read.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Program {
    /// From a name or a number, because a drum kit has no program name and somebody working from
    /// a font's own listing has numbers in front of them.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ProgramVisitor)
    }
}

struct ProgramVisitor;

impl Visitor<'_> for ProgramVisitor {
    type Value = Program;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a General MIDI program name, or a number from 0 to 127")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Program, E> {
        Program::parse(value)
            .ok_or_else(|| E::custom(format!("`{value}` is not a General MIDI program")))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Program, E> {
        (value < 128)
            .then_some(Program(value as u8))
            .ok_or_else(|| E::custom(format!("{value} is outside the 0..127 program range")))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Program, E> {
        u64::try_from(value)
            .map_err(|_| E::custom(format!("{value} is outside the 0..127 program range")))
            .and_then(|value| self.visit_u64(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_general_midi_at_the_numbers_everybody_quotes() {
        // Spot checks at the boundaries and at the numbers that appear in every implementation
        // chart, which is what would catch a name shifted by one somewhere in the middle.
        assert_eq!(Program(0).name(), "Acoustic Grand Piano");
        assert_eq!(Program(24).name(), "Acoustic Guitar (nylon)");
        assert_eq!(Program(33).name(), "Electric Bass (finger)");
        assert_eq!(Program(40).name(), "Violin");
        assert_eq!(Program(48).name(), "String Ensemble 1");
        assert_eq!(Program(56).name(), "Trumpet");
        assert_eq!(Program(73).name(), "Flute");
        assert_eq!(Program(127).name(), "Gunshot");
        assert_eq!(PROGRAMS.len(), 128);
    }

    #[test]
    fn the_families_divide_the_programs_exactly() {
        // A picker is built from these, so a family that overlapped or left a gap would be a
        // sound nobody could reach through the interface.
        assert_eq!(FAMILIES.len() * FAMILY_SIZE, PROGRAMS.len());
        let mut covered: Vec<Program> = Vec::new();
        for (family, name) in FAMILIES.iter().enumerate() {
            let programs = Program::family_programs(family);
            assert_eq!(programs.len(), FAMILY_SIZE, "{name}");
            for program in &programs {
                assert_eq!(program.family(), family, "{}", program.name());
            }
            covered.extend(programs);
        }
        assert_eq!(covered.len(), PROGRAMS.len());
        covered.dedup();
        assert_eq!(
            covered.len(),
            PROGRAMS.len(),
            "a program is in two families"
        );
        assert!(Program::family_programs(FAMILIES.len()).is_empty());

        // And the names are the standard's own, which is what makes the grouping recognisable.
        assert_eq!(FAMILIES[Program(40).family()], "Strings");
        assert_eq!(FAMILIES[Program(0).family()], "Piano");
        assert_eq!(FAMILIES[Program(127).family()], "Sound Effects");
    }

    #[test]
    fn no_two_programs_share_a_name() {
        // A duplicate would make `parse` answer one of them and never the other, and which one
        // would depend on nothing a reader could see.
        let mut names: Vec<String> = PROGRAMS.iter().map(|name| squashed(name)).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(count, names.len(), "two programs share a name");
    }

    #[test]
    fn a_name_is_read_however_it_is_spaced_and_capitalised() {
        for text in [
            "Violin",
            "violin",
            "  VIOLIN  ",
            // A name written the way an identifier would be, which is how one arrives from a
            // menu or a scripted document rather than from a person typing.
            "violin",
        ] {
            assert_eq!(Program::parse(text), Some(Program(40)), "{text}");
        }
        assert_eq!(Program::parse("electric-piano-1"), Some(Program(4)));
        assert_eq!(Program::parse("ElectricPiano1"), Some(Program(4)));
        assert_eq!(Program::parse("lead 1 (square)"), Some(Program(80)));
        assert_eq!(Program::parse("lead1square"), Some(Program(80)));
    }

    #[test]
    fn a_number_is_read_as_written_and_refused_past_the_end() {
        assert_eq!(Program::parse("0"), Some(Program(0)));
        assert_eq!(Program::parse("40"), Some(Program(40)));
        assert_eq!(Program::parse("127"), Some(Program(127)));
        assert_eq!(Program::parse("128"), None, "there is no program 128");
        assert_eq!(Program::parse("-1"), None);
        assert_eq!(Program::parse(""), None);
        assert_eq!(Program::parse("no such instrument"), None);
    }

    #[test]
    fn a_kit_is_named_by_the_nearest_one_at_or_below_it() {
        // The patches between two kits are the lower kit again, which is what almost every font
        // does with them — naming those "Standard Kit" would be a listing that lied about 16.
        assert_eq!(Program(0).kit_name(), "Standard Kit");
        assert_eq!(Program(7).kit_name(), "Standard Kit");
        assert_eq!(Program(8).kit_name(), "Room Kit");
        assert_eq!(Program(25).kit_name(), "TR-808 Kit");
        assert_eq!(Program(48).kit_name(), "Orchestra Kit");
        assert_eq!(Program(127).kit_name(), "Orchestra Kit");
        // A kit is reachable by its own name too, which is the only way to write one down: it has
        // no program name, because that number means something else entirely.
        assert_eq!(Program::parse("TR-808 Kit"), Some(Program(25)));
    }

    #[test]
    fn a_program_writes_itself_as_a_name_and_reads_back_the_same_number() {
        for number in 0..128u8 {
            let program = Program(number);
            let written = serde_json::to_string(&program).expect("a program serialises");
            assert_eq!(written, format!("\"{}\"", program.name()));
            let read: Program = serde_json::from_str(&written).expect("and reads back");
            assert_eq!(read, program, "{}", program.name());
        }
        // And a number in the document is read too, for a font's own listing.
        assert_eq!(
            serde_json::from_str::<Program>("40").expect("a number is a program"),
            Program(40)
        );
        assert!(serde_json::from_str::<Program>("200").is_err());
        assert!(serde_json::from_str::<Program>("\"tuba solo\"").is_err());
    }
}
