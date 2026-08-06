//! Whole songs to start from.
//!
//! A [`SongSpec`] has around thirty dials, and the honest answer to "what should they be" is
//! *depends what you are writing*. Starting from the defaults means turning most of them before
//! anything sounds like a piece of music rather than like a demonstration of a synthesiser, and
//! knowing which ones to turn is exactly the knowledge somebody opening a composer for the first
//! time does not have.
//!
//! So: a shelf of finished specifications. Each one is a genre with its tempo, its key, its
//! groove, its progression, its form and its roster already chosen and already agreeing with each
//! other — a jazz trio at 132 with a shuffle and a ii-V-I, an orchestra at 76 in 3/4. Load one,
//! press Write, hear a song. Then change what you do not like, which is a far better place to
//! start than an empty form.
//!
//! # Why they are text
//!
//! Every preset is a `.asong` document, embedded and parsed. It could have been a `SongSpec`
//! built in Rust, and that would have been faster and unreadable: the point of a preset is that a
//! person can see what it says, and the format was designed to be the readable one. It also means
//! the presets are parser tests that fail loudly — a field renamed without renaming it here breaks
//! the build's tests rather than the user's first five minutes.
//!
//! Most of them name General MIDI sounds, so what they sound like depends on the shipped
//! SoundFont being installed. Every part still names a plugin underneath, so a build without one
//! plays the same arrangement on oscillators. `chiptune` names no programs at all and is what the
//! composer has always done.

use crate::spec::SongSpec;

/// One whole song to start from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SongPreset {
    /// The name it is chosen by, on a command line and in a menu.
    pub name: &'static str,
    /// What it is, for a listing.
    ///
    /// English; [`auris_i18n`](https://docs.rs/auris-i18n) translates it for display, the same way
    /// it does the progression catalogue's descriptions.
    pub description: &'static str,
    /// The specification itself, as the format writes it.
    pub source: &'static str,
}

impl SongPreset {
    /// The specification this preset is.
    ///
    /// # Panics
    ///
    /// Never in a build whose tests pass: every preset is parsed by one of them. A preset that
    /// did not parse would be a compile-time constant that is wrong, and returning a `Result`
    /// nobody could act on would only move the problem into every caller.
    pub fn spec(&self) -> SongSpec {
        SongSpec::parse(self.source)
            .unwrap_or_else(|errors| panic!("the {} preset does not parse: {errors:?}", self.name))
    }
}

/// The preset with this name.
pub fn preset(name: &str) -> Option<&'static SongPreset> {
    PRESETS.iter().find(|preset| preset.name == name)
}

/// Every preset, in the order a menu should list them.
///
/// The plain one first, because it is the one that needs no library and the one every existing
/// piece was written with. The rest are roughly by how far they are from it.
pub const PRESETS: &[SongPreset] = &[
    SongPreset {
        name: "chiptune",
        description: "The built-in voices, four to the floor",
        source: CHIPTUNE,
    },
    SongPreset {
        name: "pop-band",
        description: "Drums, bass, keys and a lead — the 王道進行",
        source: POP_BAND,
    },
    SongPreset {
        name: "city-pop",
        description: "Electric piano and slap bass over 丸サ進行",
        source: CITY_POP,
    },
    SongPreset {
        name: "rock",
        description: "Overdriven guitar, organ and a hard kit",
        source: ROCK,
    },
    SongPreset {
        name: "jazz-trio",
        description: "Piano, upright bass and brushes on a ii-V-I",
        source: JAZZ_TRIO,
    },
    SongPreset {
        name: "orchestral",
        description: "Strings, horns and timpani in 3/4",
        source: ORCHESTRAL,
    },
    SongPreset {
        name: "synthwave",
        description: "Saw lead, analogue bass and a TR-808",
        source: SYNTHWAVE,
    },
    SongPreset {
        name: "ambient",
        description: "Pads and a slow bell, no kit at all",
        source: AMBIENT,
    },
];

/// What the composer has always written: the built-in oscillators, no SoundFont needed.
const CHIPTUNE: &str = r#"
title  = "Chiptune"
key    = "C major"
tempo  = 140
mood   = "bright"
groove = "four-on-the-floor"
chords = "@axis"
form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 4
intensity = 0.5

[section.chorus]
intensity = 0.95

[[part]]
name = "lead"
role = "melody"

[[part]]
name = "chords"
role = "chords"

[[part]]
name = "bass"
role = "bass"

[[part]]
name = "kick"
role = "kick"

[[part]]
name = "snare"
role = "snare"

[[part]]
name = "hat"
role = "hat"
"#;

/// A four-piece with keys on top, on the progression half of J-pop is built from.
const POP_BAND: &str = r#"
title  = "Pop Band"
key    = "F major"
tempo  = 124
mood   = "bright"
groove = "eight-beat"
chords = "@royal-road"
fill   = 0.7
form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 4
intensity = 0.45
parts     = "keys bass kick hat"

[section.chorus]
intensity = 0.95

[section.outro]
intensity = 0.5

[[part]]
name    = "lead"
role    = "melody"
program = "Lead 2 (sawtooth)"

[[part]]
name    = "keys"
role    = "chords"
program = "Electric Piano 1"

[[part]]
name    = "strings"
role    = "pad"
program = "String Ensemble 1"
gain    = -19

[[part]]
name    = "bass"
role    = "bass"
program = "Electric Bass (finger)"

[[part]]
name    = "kick"
role    = "kick"
program = "Standard Kit"

[[part]]
name    = "snare"
role    = "snare"
program = "Standard Kit"

[[part]]
name    = "hat"
role    = "hat"
program = "Standard Kit"
"#;

/// The 1980s Tokyo sound: a Rhodes, a slapped bass and a sixteen-beat under 丸サ進行.
const CITY_POP: &str = r#"
title       = "City Pop"
key         = "A major"
tempo       = 106
groove      = "sixteen-beat"
chords      = "@marusa"
brightness  = 0.7
energy      = 0.55
tension     = 0.7
syncopation = 0.6
swing       = 56
humanize    = 0.45
form        = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 4
intensity = 0.5

[section.chorus]
intensity = 0.9

[[part]]
name    = "lead"
role    = "melody"
program = "Alto Sax"
octave  = 5

[[part]]
name    = "rhodes"
role    = "chords"
program = "Electric Piano 1"

[[part]]
name    = "stabs"
role    = "stab"
program = "Brass Section"
gain    = -17

[[part]]
name    = "bass"
role    = "bass"
program = "Slap Bass 1"

[[part]]
name    = "kick"
role    = "kick"
program = "Room Kit"

[[part]]
name    = "snare"
role    = "snare"
program = "Room Kit"

[[part]]
name    = "hat"
role    = "hat"
program = "Room Kit"
"#;

/// Guitars, an organ pad and a kit that is allowed to be loud.
const ROCK: &str = r#"
title    = "Rock"
key      = "E minor"
tempo    = 148
mood     = "driving"
groove   = "basic-rock"
chords   = "@axis-minor"
dynamics = 1.0
fill     = 0.8
form     = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 4
intensity = 0.6

[section.chorus]
intensity = 1.0

[[part]]
name    = "lead"
role    = "melody"
program = "Overdriven Guitar"

[[part]]
name    = "rhythm"
role    = "chords"
program = "Distortion Guitar"
octave  = 3

[[part]]
name    = "organ"
role    = "pad"
program = "Rock Organ"
gain    = -20

[[part]]
name    = "bass"
role    = "bass"
program = "Electric Bass (pick)"

[[part]]
name    = "kick"
role    = "kick"
program = "Power Kit"

[[part]]
name    = "snare"
role    = "snare"
program = "Power Kit"

[[part]]
name    = "hat"
role    = "hat"
program = "Power Kit"
"#;

/// Three players and a lot of space: brushes instead of sticks, and a swing that means it.
const JAZZ_TRIO: &str = r#"
title    = "Jazz Trio"
key      = "F major"
tempo    = 132
groove   = "shuffle"
chords   = "@ii-v-i"
swing    = 64
humanize = 0.6
dynamics = 1.0
fill     = 0.3
tension  = 0.85
energy   = 0.45
form     = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 4
intensity = 0.4
parts     = "piano bass"

[section.chorus]
intensity = 0.8

[[part]]
name    = "piano"
role    = "chords"
program = "Acoustic Grand Piano"

[[part]]
name    = "melody"
role    = "melody"
program = "Acoustic Grand Piano"
octave  = 5

[[part]]
name    = "bass"
role    = "bass"
program = "Acoustic Bass"

[[part]]
name    = "kick"
role    = "kick"
program = "Brush Kit"
gain    = -12

[[part]]
name    = "snare"
role    = "snare"
program = "Brush Kit"
gain    = -11

[[part]]
name    = "ride"
role    = "hat"
program = "Brush Kit"
"#;

/// Slow, in three, and with the drums replaced by a timpani that only marks the big moments.
const ORCHESTRAL: &str = r#"
title      = "Orchestral"
key        = "D minor"
tempo      = 76
meter      = "3/4"
mood       = "epic"
groove     = "sparse"
chords     = "@epic"
humanize   = 0.5
variation  = 0.35
form       = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 8
intensity = 0.35
parts     = "strings cellos"

[section.chorus]
intensity = 1.0

[section.outro]
bars      = 8
intensity = 0.4

[[part]]
name    = "flute"
role    = "melody"
program = "Flute"
octave  = 6

[[part]]
name    = "horns"
role    = "chords"
program = "French Horn"
octave  = 4

[[part]]
name    = "strings"
role    = "pad"
program = "String Ensemble 1"
gain    = -13

[[part]]
name    = "harp"
role    = "arp"
program = "Orchestral Harp"
gain    = -15

[[part]]
name    = "cellos"
role    = "bass"
program = "Cello"

[[part]]
name    = "timpani"
role    = "kick"
program = "Orchestra Kit"
gain    = -8
"#;

/// A saw over an eighth-note bass, and the drum machine everybody means by "eighties".
const SYNTHWAVE: &str = r#"
title       = "Synthwave"
key         = "A minor"
tempo       = 112
groove      = "four-on-the-floor"
chords      = "@sad-loop"
brightness  = 0.35
energy      = 0.75
tension     = 0.4
syncopation = 0.25
humanize    = 0.15
variation   = 0.2
form        = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.intro]
bars      = 8
intensity = 0.5
parts     = "pad bass kick"

[section.chorus]
intensity = 0.95

[[part]]
name    = "lead"
role    = "melody"
program = "Lead 2 (sawtooth)"

[[part]]
name    = "pad"
role    = "pad"
program = "Pad 1 (new age)"
gain    = -14

[[part]]
name    = "arp"
role    = "arp"
program = "Lead 5 (charang)"
gain    = -15

[[part]]
name    = "bass"
role    = "bass"
program = "Synth Bass 1"

[[part]]
name    = "kick"
role    = "kick"
program = "TR-808 Kit"

[[part]]
name    = "snare"
role    = "snare"
program = "TR-808 Kit"

[[part]]
name    = "hat"
role    = "hat"
program = "TR-808 Kit"
"#;

/// No kit, no lead: three sustained voices and a bell that is nearly a melody.
const AMBIENT: &str = r#"
title       = "Ambient"
key         = "C lydian"
tempo       = 64
mood        = "calm"
groove      = "sparse"
chords      = "| Imaj7 | Imaj7 | IVmaj7 | IVmaj7 |"
humanize    = 0.55
dynamics    = 0.5
fill        = 0.0
variation   = 0.4
form        = ["intro", "verse", "chorus", "verse", "outro"]

[section.intro]
bars      = 8
intensity = 0.25
parts     = "pad"

[section.verse]
bars      = 8
intensity = 0.45

[section.chorus]
bars      = 8
intensity = 0.7

[section.outro]
bars      = 8
intensity = 0.3
parts     = "pad"

[[part]]
name    = "pad"
role    = "pad"
program = "Pad 7 (halo)"
gain    = -12

[[part]]
name    = "bells"
role    = "melody"
program = "Music Box"
octave  = 6
density = 0.2

[[part]]
name    = "glass"
role    = "arp"
program = "FX 3 (crystal)"
gain    = -18
density = 0.25

[[part]]
name    = "cello"
role    = "bass"
program = "Cello"
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::compose;
    use crate::spec::Role;

    #[test]
    fn every_preset_parses_and_writes_a_piece() {
        // The presets are constants, so a field renamed without renaming it here is a wrong
        // program that still compiles. This is what catches it — and it goes as far as composing,
        // because a specification that parses and produces no notes is a preset that would answer
        // the button with silence.
        for preset in PRESETS {
            let spec = preset.spec();
            let piece = compose(&spec);
            assert!(
                piece.note_count() > 32,
                "{} wrote {} notes",
                preset.name,
                piece.note_count()
            );
            assert!(
                piece.tracks.len() >= 3,
                "{} has {} tracks",
                preset.name,
                piece.tracks.len()
            );
            assert!(!preset.description.is_empty());
        }
    }

    #[test]
    fn a_preset_survives_being_written_out_and_read_back() {
        // The song sheet's Save as Specification… writes what it was loaded with, so a preset
        // that did not round-trip would be a document nobody could keep.
        for preset in PRESETS {
            let spec = preset.spec();
            assert_eq!(
                SongSpec::parse(&spec.to_toml()),
                Ok(spec),
                "{} does not round-trip",
                preset.name
            );
        }
    }

    #[test]
    fn no_two_presets_share_a_name() {
        let mut names: Vec<&str> = PRESETS.iter().map(|preset| preset.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(count, names.len(), "two presets share a name");
        assert!(preset("chiptune").is_some());
        assert!(preset("no-such-style").is_none());
    }

    #[test]
    fn every_part_falls_back_to_a_plugin_when_no_font_is_installed() {
        // A preset naming General MIDI sounds has to be playable on a build that has none, or
        // choosing one on a fresh clone would be choosing silence. What makes that work is that
        // every part still carries an instrument, which is the role's default unless it says.
        for preset in PRESETS {
            for part in preset.spec().parts {
                assert!(
                    !part.instrument.is_empty(),
                    "{} · {} names no plugin",
                    preset.name,
                    part.name
                );
            }
        }
    }

    #[test]
    fn the_plain_preset_asks_for_no_soundfont_at_all() {
        // One preset has to work identically with and without the library, and be first in the
        // list, because it is what somebody who has not run the fetch script will land on.
        assert_eq!(PRESETS[0].name, "chiptune");
        for part in PRESETS[0].spec().parts {
            assert_eq!(part.program, None, "{} asks for a font", part.name);
        }
        // And every other one does use it, or it would not be worth a line in the menu.
        for preset in &PRESETS[1..] {
            assert!(
                preset
                    .spec()
                    .parts
                    .iter()
                    .any(|part| part.program.is_some()),
                "{} names no General MIDI sound",
                preset.name
            );
        }
    }

    #[test]
    fn every_preset_has_a_rhythm_section_or_a_reason_not_to() {
        // A roster with no bass and no kit is a preset that will sound like a demo, which is the
        // thing these exist to stop. The one exception is deliberate and says so by its name.
        for preset in PRESETS {
            let spec = preset.spec();
            let has = |wanted: Role| spec.parts.iter().any(|part| part.role == wanted);
            assert!(has(Role::Bass), "{} has nothing underneath it", preset.name);
            assert!(
                has(Role::Kick) || preset.name == "ambient",
                "{} has no kit",
                preset.name
            );
        }
    }
}
