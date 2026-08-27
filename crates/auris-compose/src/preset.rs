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
seed   = 1
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

[[part]]
name = "crash"
role = "crash"
"#;

/// A four-piece with keys on top, on the progression half of J-pop is built from.
///
/// The verse walks 純情進行 — the canon over a stepwise descending bass — and the chorus lifts
/// into 王道進行, which is the shape of the songs this preset is named for: an Aメロ that steps
/// quietly downhill so the サビ has somewhere to arrive from. One progression for the whole form
/// was the arrangement changing at every join over harmony that never did.
const POP_BAND: &str = r#"
title  = "Pop Band"
key    = "F major"
tempo  = 124
mood   = "bright"
groove = "eight-beat"
chords = "@royal-road"
fill   = 0.7
seed   = 2
form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[harmony]
a-melo = "@junjo"

[section.intro]
bars      = 4
intensity = 0.45
parts     = "keys bass kick hat"

[section.verse]
chords = "a-melo"

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

[[part]]
name    = "crash"
role    = "crash"
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
seed        = 3
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

[[part]]
name    = "crash"
role    = "crash"
program = "Room Kit"
"#;

/// Guitars, an organ pad and a kit that is allowed to be loud.
///
/// The verse and the chorus play the same four chords the other way round: `@axis-minor` broods
/// from the minor tonic, and the chorus rotates the loop to open on the relative major — the
/// cheapest lift in rock and the one this form is built on, bought without a single chord the
/// verse did not already own.
const ROCK: &str = r#"
title    = "Rock"
key      = "E minor"
tempo    = 148
mood     = "driving"
groove   = "basic-rock"
chords   = "@axis-minor"
dynamics = 1.0
fill     = 0.8
seed     = 4
form     = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[harmony]
lift = "@axis"

[section.intro]
bars      = 4
intensity = 0.6

[section.chorus]
intensity = 1.0
chords    = "lift"

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

[[part]]
name    = "crash"
role    = "crash"
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
seed     = 5
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

/// Slow, in three, and with the drums replaced by a timpani and a cymbal that only mark the big
/// moments.
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
seed       = 6
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

[[part]]
name    = "cymbal"
role    = "crash"
program = "Orchestra Kit"
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
seed        = 7
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

[[part]]
name    = "crash"
role    = "crash"
program = "TR-808 Kit"
"#;

/// No kit, no lead: three sustained voices and a bell that is nearly a melody.
///
/// Its chords are the mode's own, and they have to be: lydian raises the fourth, so `IVmaj7` here
/// is F#maj7, a tritone from the tonic with three of its four tones outside the key — and a chart
/// written out by hand is played exactly as typed, so nothing corrects it. `II` is the chord where
/// that raised fourth is a chord tone rather than a clash, and two bars of the tonic answered by
/// two of `II` is the floating that lydian was chosen for.
const AMBIENT: &str = r#"
title       = "Ambient"
key         = "C lydian"
tempo       = 64
mood        = "calm"
groove      = "sparse"
chords      = "| Imaj7 | Imaj7 | II | II |"
humanize    = 0.55
dynamics    = 0.5
fill        = 0.0
variation   = 0.4
seed        = 8
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
    use crate::theory::chord_scale::ChordScale;
    use auris_core::time::{TICKS_PER_QUARTER, Ticks};

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
    fn every_preset_is_a_draw_of_its_own() {
        // Every one of them used to leave the seed at its default, so eight presets were eight
        // arrangements over one set of random numbers: the same figure fell in the same bar of
        // every piece, and hearing all eight was hearing one draw eight times. Which numbers
        // these are does not matter and nothing here says it does — what matters is that no two
        // are the same, and that a ninth preset added without a seed fails this rather than
        // quietly rejoining the pile.
        let mut seeds: Vec<u64> = PRESETS.iter().map(|preset| preset.spec().seed).collect();
        let count = seeds.len();
        seeds.sort_unstable();
        seeds.dedup();
        assert_eq!(
            count,
            seeds.len(),
            "two presets are the same draw: {seeds:?}"
        );
        assert!(
            !seeds.contains(&SongSpec::default().seed),
            "a preset left the seed where it found it"
        );
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
    fn every_preset_writes_chords_its_own_key_can_hold() {
        // A preset chooses a key and a progression in two separate lines and nothing makes them
        // agree: a chart written out by hand is played exactly as typed, so a degree that means
        // something else in the declared mode is simply played wrong and never corrected. The
        // ambient preset asked for IVmaj7 in C lydian, where the fourth is raised — F#maj7, a
        // tritone from the tonic, three of whose four tones the key does not have.
        //
        // One chromatic tone is the ordinary colour of a secondary dominant, which is what 丸サ
        // 進行's III7 is for. Two or three is a chord out of another key, and the scale a part
        // would improvise on collapses with it: the six notes below are what is left of the seven
        // after the alteration takes the degree it altered, and five means two degrees went.
        //
        // The bar length only decides where the chords fall, so any of them does — this is about
        // what the chords are.
        let bar = Ticks(TICKS_PER_QUARTER * 4);
        for preset in PRESETS {
            let spec = preset.spec();
            let key = spec.key;
            for (name, chart) in &spec.charts {
                for event in chart.spelled_in(key).resolve(key, bar) {
                    let outside = event
                        .chord
                        .classes()
                        .into_iter()
                        .filter(|class| !key.scale.contains(key.tonic, *class))
                        .count();
                    assert!(
                        outside <= 1,
                        "{} · {name}: {} has {outside} tones outside {}",
                        preset.name,
                        event.name(),
                        key.to_text()
                    );
                    let scale = ChordScale::new(key, event.chord);
                    assert!(
                        scale.degree_count() >= 6,
                        "{} · {name}: {} leaves {} playable notes in {}",
                        preset.name,
                        event.name(),
                        scale.degree_count(),
                        key.to_text()
                    );
                }
            }
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
    fn the_band_presets_give_the_verse_and_the_chorus_different_progressions() {
        // A verse and a chorus on one loop is a form whose arrangement changes at every join
        // over harmony that never does. The loop-built genres — city pop on 丸サ, synthwave on
        // its own four bars — keep one progression *on purpose*, so this is asserted only where
        // the genre wants the contrast.
        for name in ["pop-band", "rock"] {
            let spec = preset(name).expect("listed").spec();
            let chart_of = |section: &str| {
                spec.chart_for(
                    spec.sections
                        .get(section)
                        .unwrap_or_else(|| panic!("{name} has no {section}")),
                )
            };
            let (verse, chorus) = (chart_of("verse"), chart_of("chorus"));
            assert_ne!(verse.bars, chorus.bars, "{name} plays one loop throughout");
            // Both are quoted, so the mood colours neither: the contrast is the preset's own
            // choice, not something tension can rewrite.
            assert_eq!(verse.origin, auris_core::theory::chart::ChartOrigin::Given);
            assert_eq!(chorus.origin, auris_core::theory::chart::ChartOrigin::Given);
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
