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
//! Genre presets name native physical instruments directly, including the membrane and
//! metal-plate drum kit. `chiptune` and `game-loop` retain their oscillator voices. Every preset
//! plays without external sound assets.

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
/// The chiptune arrangements first, followed by the band, ensemble and atmospheric styles.
pub const PRESETS: &[SongPreset] = &[
    SongPreset {
        name: "chiptune",
        description: "The built-in voices, four to the floor",
        source: CHIPTUNE,
    },
    SongPreset {
        name: "game-loop",
        description: "A sixteen-bar chiptune loop for game background music",
        source: include_str!("../../../examples/game-loop.asong"),
    },
    SongPreset {
        name: "pop-band",
        description: "Drums, bass, keys and a lead — the 王道進行",
        source: POP_BAND,
    },
    SongPreset {
        name: "city-pop",
        description: "Piano, clarinet and bass over 丸サ進行",
        source: CITY_POP,
    },
    SongPreset {
        name: "rock",
        description: "Electric guitars, choir and a hard kit",
        source: ROCK,
    },
    SongPreset {
        name: "jazz-trio",
        description: "Piano, bass and ride cymbal on a ii-V-I",
        source: JAZZ_TRIO,
    },
    SongPreset {
        name: "orchestral",
        description: "Strings, woodwinds and mallets in 3/4",
        source: ORCHESTRAL,
    },
    SongPreset {
        name: "synthwave",
        description: "Bowed lead, choir and bass over a steady kit",
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
performance = "chiptune"
writing_style = "chiptune"
title  = "Chiptune"
key    = "C major"
tempo  = 140
mood   = "bright"
groove = "four-on-the-floor"
chords = "@axis"
seed   = 16
form   = ["intro","verse","chorus","verse2","chorus2","outro"]

humanize = 0.384
dynamics = 0.829

[section.intro]
bars      = 4
intensity = 0.5
parts     = "chords bass kick snare hat crash"

[section.verse]
parts = "lead chords bass kick snare hat"

[section.chorus]
intensity = 0.95
parts = "lead chords arp bass kick snare hat crash"

[[part]]
name = "lead"
role = "melody"

[[part]]
name = "lead_alt"
role = "arp"
octave = 5
instrument = "auris.synth.fm2"
density = 0.25

[[part]]
name = "chords"
role = "chords"
gate = 0.65

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

[[part]]
name = "arp"
role = "arp"
density = 0.35

[[part]]
name = "counter_chords"
role = "chords"
gate = 0.45

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "lead lead_alt chords bass kick snare hat"

[section.chorus2]
intensity = 0.95
melody_from = "chorus"
parts = "lead counter_chords arp bass kick snare hat crash"

[section.outro]
parts = "lead chords bass kick snare hat crash"
"#;

/// A band with rotating keyboard and guitar layers, on a familiar J-pop progression.
///
/// The verse walks 純情進行 — the canon over a stepwise descending bass — and the chorus lifts
/// into 王道進行, which is the shape of the songs this preset is named for: an Aメロ that steps
/// quietly downhill so the サビ has somewhere to arrive from. A four-bar pre-chorus climbs
/// ii–iii–IV–V into that arrival. After the second chorus, a quieter bridge starts on vi with
/// keys and strings under the lead, making room for the full band's final chorus.
const POP_BAND: &str = r#"
performance = "pop-band"
writing_style = "pop-band"
title  = "Pop Band"
key    = "F major"
tempo  = 124
mood   = "bright"
groove = "eight-beat"
chords = "@royal-road"
fill   = 0.7
seed   = 6
form   = ["intro","verse","pre","chorus","verse2","pre2","chorus2","bridge","chorus3","outro"]

[harmony]
a-melo = "@junjo"
b-melo = "| ii7 | iii7 | IVmaj7 | V7 |"
c-melo = "| vi7 | IVmaj7 | Imaj7 | V7 |"

[section.intro]
bars      = 4
intensity = 0.45
parts     = "keys bass kick hat"

[section.verse]
chords = "a-melo"
parts = "lead keys bass kick hat"

[section.pre]
bars = 4
chords = "b-melo"
intensity = 0.75
parts = "lead_alt strings pluck bass hat riser"

[section.bridge]
bars = 8
chords = "c-melo"
intensity = 0.60
parts = "lead keys strings bass"

[section.chorus]
intensity = 0.95
parts = "lead keys strings guitar bass kick snare hat crash"

[section.outro]
intensity = 0.5
parts = "keys strings bass hat"

[[part]]
name    = "lead"
role    = "melody"
instrument = "auris.physical.violin"

[[part]]
name    = "lead_alt"
role    = "melody"
instrument = "auris.physical.electric_guitar"
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 12.0, bass_db = -2.0, treble_db = 1.5, cabinet = 1.0, output_db = -6.0 } },
]

[[part]]
name    = "keys"
role    = "chords"
instrument = "auris.physical.piano"

[[part]]
name    = "piano"
role    = "chords"
instrument = "auris.physical.piano"

[[part]]
name    = "guitar"
role    = "chords"
instrument = "auris.physical.electric_guitar"
gate    = 0.45
pan     = 0.35
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 3.0, bass_db = -2.0, treble_db = 2.0, cabinet = 1.0, output_db = -3.0 } },
    { id = "auris.fx.chorus", params = { mix = 0.18, rate_hz = 0.6, depth_ms = 2.0 } },
]

[[part]]
name    = "pluck"
role    = "arp"
instrument = "auris.physical.mallet"
density = 0.3
gain    = -18
pan     = 0.3

[[part]]
name    = "strings"
role    = "pad"
instrument = "auris.physical.violin"
gain    = -19

[[part]]
name    = "bass"
role    = "bass"
instrument = "auris.physical.bass"
gate    = 0.8

[[part]]
name    = "sub_bass"
role    = "bass"
instrument = "auris.physical.bass"
octave  = 1

[[part]]
name    = "kick"
role    = "kick"
instrument = "auris.synth.drumkit"

[[part]]
name    = "snare"
role    = "snare"
instrument = "auris.synth.drumkit"

[[part]]
name    = "hat"
role    = "hat"
instrument = "auris.synth.drumkit"

[[part]]
name    = "crash"
role    = "crash"
instrument = "auris.synth.drumkit"

[[part]]
name    = "riser"
role    = "riser"
instrument = "auris.physical.violin"

[section.verse2]
chords = "a-melo"
intensity = 0.60
melody_from = "verse"
parts = "lead_alt keys sub_bass kick hat"

[section.pre2]
bars = 4
chords = "b-melo"
intensity = 0.75
melody_from = "pre"
parts = "lead strings pluck bass hat riser"

[section.chorus2]
intensity = 0.95
melody_from = "chorus"
parts = "lead guitar strings piano bass kick snare hat crash"

[section.chorus3]
intensity = 0.95
melody_from = "chorus"
parts = "lead_alt piano strings guitar pluck bass kick snare hat crash riser"
"#;

/// Piano, clarinet and bass on a sixteen-beat under 丸サ進行.
///
/// The chorus plays 丸サ with its ii–V spelled out — `@marusa5` — which is how the genre itself
/// intensifies the loop: the same four bars, one of them now moving twice. A chart with two
/// chords in a bar was off limits until the melody learned to re-join its figure at a mid-bar
/// change; see the third pass of [`crate::melodic`].
const CITY_POP: &str = r#"
performance = "city-pop"
writing_style = "city-pop"
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
seed        = 2
form        = ["intro","verse","chorus","verse2","chorus2","outro"]

[harmony]
sabi = "@marusa5"

[section.intro]
bars      = 4
intensity = 0.5
parts     = "rhodes stabs bass kick snare hat crash riser"

[section.verse]
parts = "lead rhodes bass kick snare hat"

[section.chorus]
intensity = 0.9
chords    = "sabi"
parts = "lead rhodes stabs bass kick snare hat crash"

[[part]]
name    = "lead"
role    = "melody"
instrument = "auris.physical.clarinet"
octave  = 5

[[part]]
name    = "lead_alt"
role    = "stab"
instrument = "auris.physical.clarinet"
octave  = 5

[[part]]
name    = "rhodes"
role    = "chords"
instrument = "auris.physical.piano"

[[part]]
name    = "piano"
role    = "chords"
instrument = "auris.physical.piano"

[[part]]
name    = "guitar"
role    = "stab"
instrument = "auris.physical.electric_guitar"
gain    = -18
pan     = 0.35
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 3.0, bass_db = -2.0, treble_db = 2.0, cabinet = 1.0, output_db = -3.0 } },
    { id = "auris.fx.chorus", params = { mix = 0.22, rate_hz = 0.6, depth_ms = 2.0 } },
]

[[part]]
name    = "arp"
role    = "arp"
instrument = "auris.physical.mallet"
gain    = -18
density = 0.25
pan     = 0.3

[[part]]
name    = "stabs"
role    = "stab"
instrument = "auris.physical.clarinet"
gain    = -17

[[part]]
name    = "bass"
role    = "bass"
instrument = "auris.physical.bass"

[[part]]
name    = "kick"
role    = "kick"
instrument = "auris.synth.drumkit"

[[part]]
name    = "snare"
role    = "snare"
instrument = "auris.synth.drumkit"

[[part]]
name    = "hat"
role    = "hat"
instrument = "auris.synth.drumkit"

[[part]]
name    = "crash"
role    = "crash"
instrument = "auris.synth.drumkit"

[[part]]
name    = "riser"
role    = "riser"
instrument = "auris.physical.violin"

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "lead lead_alt piano bass kick snare hat riser"

[section.chorus2]
intensity = 0.9
chords    = "sabi"
melody_from = "chorus"
parts = "lead stabs guitar arp bass kick snare hat crash"

[section.outro]
parts = "lead rhodes stabs bass kick snare hat crash"
"#;

/// Electric guitars, a choir pad and a kit that is allowed to be loud.
///
/// The verse and the chorus play the same four chords the other way round: `@axis-minor` broods
/// from the minor tonic, and the chorus rotates the loop to open on the relative major — the
/// cheapest lift in rock and the one this form is built on, bought without a single chord the
/// verse did not already own.
const ROCK: &str = r#"
performance = "rock"
writing_style = "rock"
title    = "Rock"
key      = "E minor"
tempo    = 148
mood     = "driving"
groove   = "basic-rock"
chords   = "@axis-minor"
dynamics = 1.0
fill     = 0.8
seed     = 10
form     = ["intro","verse","chorus","verse2","chorus2","outro"]

[harmony]
lift = "@axis"

[section.intro]
bars      = 4
intensity = 0.6
parts     = "rhythm organ bass kick snare hat crash"

[section.verse]
parts = "lead rhythm bass kick snare hat"

[section.chorus]
intensity = 1.0
chords    = "lift"
parts = "lead rhythm organ bass kick snare hat crash"

[[part]]
name    = "lead"
role    = "melody"
instrument = "auris.physical.electric_guitar"
params = { pickup_position = 0.064146 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 34.0, bass_db = -3.0, mid_db = 1.5, treble_db = 2.0, cabinet = 2.0, output_db = -12.0 } },
]

[[part]]
name    = "lead_alt"
role    = "melody"
instrument = "auris.physical.electric_guitar"
params = { pickup_position = 0.064146 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 24.0, bass_db = -3.0, mid_db = 2.0, treble_db = 1.5, cabinet = 2.0, output_db = -10.0 } },
]

[[part]]
name    = "clean_guitar"
role    = "chords"
instrument = "auris.physical.electric_guitar"
gate    = 0.5
pan     = 0.35
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 3.0, bass_db = -2.0, treble_db = 2.0, cabinet = 1.0, output_db = -3.0 } },
    { id = "auris.fx.chorus", params = { mix = 0.18, rate_hz = 0.6, depth_ms = 2.0 } },
]

[[part]]
name    = "piano"
role    = "stab"
instrument = "auris.physical.piano"
gain    = -18

[[part]]
name    = "arp"
role    = "arp"
instrument = "auris.physical.electric_guitar"
density = 0.25
gain    = -18
pan     = 0.3
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 3.0, bass_db = -2.0, treble_db = 2.0, cabinet = 1.0, output_db = -3.0 } },
]

[[part]]
name    = "rhythm"
role    = "chords"
instrument = "auris.physical.electric_guitar"
octave  = 3
gate    = 0.65
params = { pickup_position = 0.064146 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 28.0, bass_db = -4.0, mid_db = 1.0, treble_db = 1.0, cabinet = 2.0, output_db = -12.0 } },
]

[[part]]
name    = "organ"
role    = "pad"
instrument = "auris.physical.choir"
gain    = -20

[[part]]
name    = "bass"
role    = "bass"
instrument = "auris.physical.bass"

[[part]]
name    = "kick"
role    = "kick"
instrument = "auris.synth.drumkit"

[[part]]
name    = "snare"
role    = "snare"
instrument = "auris.synth.drumkit"

[[part]]
name    = "hat"
role    = "hat"
instrument = "auris.synth.drumkit"

[[part]]
name    = "crash"
role    = "crash"
instrument = "auris.synth.drumkit"

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "lead_alt rhythm clean_guitar bass kick snare hat"

[section.chorus2]
intensity = 1.0
chords    = "lift"
melody_from = "chorus"
parts = "lead rhythm piano organ arp bass kick snare hat crash"

[section.outro]
parts = "lead rhythm organ bass kick snare hat crash"
"#;

/// Piano, bass and a quiet kit, with a swing that means it.
const JAZZ_TRIO: &str = r#"
performance = "jazz-trio"
writing_style = "jazz-trio"
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
seed     = 15
form     = ["intro","verse","chorus","verse2","chorus2","outro"]

[section.intro]
bars      = 4
intensity = 0.4
parts     = "piano bass"

[section.verse]
parts = "melody piano bass kick snare ride"

[section.chorus]
intensity = 0.8
parts = "melody piano bass kick snare ride"

[[part]]
name    = "piano"
role    = "chords"
instrument = "auris.physical.piano"

[[part]]
name    = "melody"
role    = "melody"
instrument = "auris.physical.piano"
octave  = 5

[[part]]
name    = "bass"
role    = "bass"
instrument = "auris.physical.bass"

[[part]]
name    = "kick"
role    = "kick"
instrument = "auris.synth.drumkit"
gain    = -12

[[part]]
name    = "snare"
role    = "snare"
instrument = "auris.synth.drumkit"
gain    = -11

[[part]]
name    = "ride"
role    = "hat"
instrument = "auris.synth.drumkit"
note    = 51

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "melody piano bass kick snare ride"

[section.chorus2]
intensity = 0.8
melody_from = "chorus"
parts = "melody piano bass kick snare ride"

[section.outro]
parts = "melody piano bass kick snare ride"
"#;

/// Slow, in three, with winds, bowed strings, plucked guitar, low mallets and a cymbal.
const ORCHESTRAL: &str = r#"
performance = "orchestral"
writing_style = "orchestral"
title      = "Orchestral"
key        = "D minor"
tempo      = 76
meter      = "3/4"
mood       = "epic"
groove     = "sparse"
chords     = "@epic"
humanize   = 0.5
variation  = 0.35
seed       = 12
form       = ["intro","verse","chorus","verse2","chorus2","outro"]

[section.intro]
bars      = 8
intensity = 0.35
parts     = "strings cellos harp"

[section.verse]
parts = "flute oboe strings cellos pizzicato"

[section.chorus]
intensity = 1.0
parts = "flute horns strings pizzicato harp cellos timpani cymbal"

[section.outro]
bars      = 8
intensity = 0.4
parts = "flute horns strings harp cellos timpani cymbal"

[[part]]
name    = "flute"
role    = "melody"
instrument = "auris.physical.tin_whistle"
octave  = 6

[[part]]
name    = "oboe"
role    = "arp"
instrument = "auris.physical.clarinet"
octave  = 5
density = 0.2

[[part]]
name    = "clarinet"
role    = "melody"
instrument = "auris.physical.clarinet"
octave  = 5

[[part]]
name    = "solo_violin"
role    = "melody"
instrument = "auris.physical.violin"
octave  = 5

[[part]]
name    = "horns"
role    = "chords"
instrument = "auris.physical.clarinet"
octave  = 4

[[part]]
name    = "trumpet"
role    = "stab"
instrument = "auris.physical.clarinet"
octave  = 5

[[part]]
name    = "strings"
role    = "pad"
instrument = "auris.physical.violin"
gain    = -13

[[part]]
name    = "pizzicato"
role    = "stab"
instrument = "auris.physical.guitar"
gain    = -18

[[part]]
name    = "harp"
role    = "arp"
instrument = "auris.physical.guitar"
gain    = -15

[[part]]
name    = "celesta"
role    = "arp"
instrument = "auris.physical.bell"
gain    = -19

[[part]]
name    = "cellos"
role    = "bass"
instrument = "auris.physical.violin"

[[part]]
name    = "timpani"
role    = "bass"
instrument = "auris.physical.mallet"
octave  = 2
gain    = -8

[[part]]
name    = "cymbal"
role    = "crash"
instrument = "auris.synth.drumkit"

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "flute clarinet strings cellos harp celesta"

[section.chorus2]
intensity = 1.0
melody_from = "chorus"
parts = "solo_violin trumpet horns strings pizzicato harp cellos timpani cymbal"
"#;

/// A bowed lead and choir over an eighth-note bass and a steady physical kit.
const SYNTHWAVE: &str = r#"
performance = "synthwave"
writing_style = "synthwave"
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
form        = ["intro","verse","chorus","verse2","chorus2","outro"]

[section.intro]
bars      = 8
intensity = 0.5
parts     = "pad bass kick"

[section.verse]
parts = "lead pad bass kick hat"

[section.chorus]
intensity = 0.95
parts = "lead pad arp chords bass kick snare hat crash"

[[part]]
name    = "lead"
role    = "melody"
instrument = "auris.physical.violin"

[[part]]
name    = "lead_alt"
role    = "melody"
instrument = "auris.physical.tin_whistle"

[[part]]
name    = "chords"
role    = "chords"
instrument = "auris.physical.piano"
gain    = -17
pan     = -0.25

[[part]]
name    = "pluck"
role    = "stab"
instrument = "auris.physical.tin_whistle"
gain    = -18
density = 0.3
pan     = 0.3

[[part]]
name    = "pad"
role    = "pad"
instrument = "auris.physical.choir"
gain    = -14

[[part]]
name    = "dark_pad"
role    = "pad"
instrument = "auris.physical.violin"
gain    = -19

[[part]]
name    = "arp"
role    = "arp"
instrument = "auris.physical.electric_guitar"
gain    = -15
params = { pickup_position = 0.12 }
effects = [
    { id = "auris.fx.guitar_amp", params = { drive_db = 6.0, bass_db = -2.0, treble_db = 2.0, cabinet = 1.0, output_db = -3.0 } },
    { id = "auris.fx.chorus", params = { mix = 0.25, rate_hz = 0.45, depth_ms = 2.5 } },
    { id = "auris.fx.delay", params = { mix = 0.12, feedback = 0.22, damping_hz = 4500.0, sync = 7.0, ping_pong = 1.0 } },
]

[[part]]
name    = "bass"
role    = "bass"
instrument = "auris.physical.bass"

[[part]]
name    = "sub_bass"
role    = "bass"
instrument = "auris.physical.bass"
octave  = 1

[[part]]
name    = "kick"
role    = "kick"
instrument = "auris.synth.drumkit"

[[part]]
name    = "snare"
role    = "snare"
instrument = "auris.synth.drumkit"

[[part]]
name    = "hat"
role    = "hat"
instrument = "auris.synth.drumkit"

[[part]]
name    = "crash"
role    = "crash"
instrument = "auris.synth.drumkit"

[[part]]
name    = "riser"
role    = "riser"
instrument = "auris.physical.violin"

[section.verse2]
intensity = 0.55
melody_from = "verse"
parts = "lead_alt dark_pad pluck sub_bass kick hat riser"

[section.chorus2]
intensity = 0.95
melody_from = "chorus"
parts = "lead dark_pad arp chords bass kick snare hat crash"

[section.outro]
parts = "lead pad arp bass kick snare hat crash"
"#;

/// No kit, no lead: three sustained voices and a bell that is nearly a melody.
///
/// Its chords are the mode's own, and they have to be: lydian raises the fourth, so `IVmaj7` here
/// is F#maj7, a tritone from the tonic with three of its four tones outside the key — and a chart
/// written out by hand is played exactly as typed, so nothing corrects it. `II` is the chord where
/// that raised fourth is a chord tone rather than a clash, and two bars of the tonic answered by
/// two of `II` is the floating that lydian was chosen for.
const AMBIENT: &str = r#"
performance = "ambient"
writing_style = "ambient"
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
seed        = 4
form        = ["intro","verse","chorus","verse2","outro"]

[section.intro]
bars      = 8
intensity = 0.25
parts     = "pad"

[section.verse]
bars      = 8
intensity = 0.45
parts = "pad bells glass cello"

[section.chorus]
bars      = 8
intensity = 0.7
parts = "pad bells glass cello"

[section.outro]
bars      = 8
intensity = 0.3
parts     = "pad"

[[part]]
name    = "pad"
role    = "pad"
instrument = "auris.physical.choir"
gain    = -12

[[part]]
name    = "bells"
role    = "melody"
instrument = "auris.physical.bell"
octave  = 6
density = 0.2

[[part]]
name    = "glass"
role    = "arp"
instrument = "auris.physical.mallet"
gain    = -18
density = 0.25

[[part]]
name    = "cello"
role    = "bass"
instrument = "auris.physical.violin"

[section.verse2]
bars      = 8
intensity = 0.45
melody_from = "verse"
parts = "pad glass cello"
"#;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn every_shipped_electric_guitar_has_an_authored_amplifier() {
        for preset in PRESETS {
            let piece = crate::compose(&preset.spec());
            for track in piece
                .tracks
                .iter()
                .filter(|track| track.instrument == "auris.physical.electric_guitar")
            {
                assert_eq!(
                    track.effects.first().map(|effect| effect.id.as_str()),
                    Some("auris.fx.guitar_amp"),
                    "{} / {}",
                    preset.name,
                    track.name
                );
            }
        }
    }

    #[test]
    fn rock_guitars_arrive_through_amplifiers_with_distinct_clean_and_driven_parts() {
        let piece = crate::compose(&crate::preset("rock").unwrap().spec());
        for (name, minimum_drive) in [("lead", 28.0), ("lead_alt", 18.0), ("rhythm", 24.0)] {
            let track = piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap();
            let amp = track.effects.first().expect("rock guitar needs an amp");
            assert_eq!(amp.id, "auris.fx.guitar_amp", "{name}");
            assert!(amp.state.params["drive_db"] >= minimum_drive, "{name}");
            assert_eq!(amp.state.params["cabinet"], 2.0, "{name}");
        }
        for name in ["clean_guitar", "arp"] {
            let track = piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap();
            let amp = track.effects.first().expect("clean DI also needs an amp");
            assert_eq!(amp.id, "auris.fx.guitar_amp", "{name}");
            assert!(amp.state.params["drive_db"] <= 6.0, "{name}");
        }
    }

    #[test]
    fn shipped_presets_use_self_contained_instruments() {
        for preset in PRESETS {
            for part in preset.spec().parts {
                assert_eq!(part.program, None, "{} · {}", preset.name, part.name);
                assert_eq!(part.source, None, "{} · {}", preset.name, part.name);
                if !matches!(preset.name, "chiptune" | "game-loop") {
                    assert!(
                        part.instrument.starts_with("auris.physical.")
                            || (part.role.is_drum() && part.instrument == "auris.synth.drumkit"),
                        "{} · {} uses {}",
                        preset.name,
                        part.name,
                        part.instrument
                    );
                }
            }
        }
    }

    #[test]
    fn preset_drums_use_the_native_kit_and_keep_percussion_notes() {
        for preset in PRESETS {
            for part in preset
                .spec()
                .parts
                .iter()
                .filter(|part| part.role.is_drum())
            {
                assert_eq!(
                    part.instrument, "auris.synth.drumkit",
                    "{} · {}",
                    preset.name, part.name
                );
                assert_eq!(part.sound(), None, "{} · {}", preset.name, part.name);
            }
        }
        let jazz = preset("jazz-trio").unwrap().spec();
        assert_eq!(
            jazz.parts
                .iter()
                .find(|part| part.name == "ride")
                .unwrap()
                .drum_note(),
            Some(51)
        );
        let orchestra = preset("orchestral").unwrap().spec();
        let timpani = orchestra
            .parts
            .iter()
            .find(|part| part.name == "timpani")
            .unwrap();
        assert!(
            !timpani.role.is_drum(),
            "the low mallet part must retain its pitched score"
        );
    }

    #[test]
    fn presets_offer_separate_later_lyrics_with_shared_melodies() {
        for preset in super::PRESETS {
            let spec = preset.spec();
            if spec.ending == crate::spec::Ending::Loop {
                continue;
            }
            let later = &spec.sections["verse2"];
            assert!(spec.form.contains(&later.name), "{}", preset.name);
            assert_eq!(later.melody_from.as_deref(), Some("verse"));
            assert_eq!(later.bars, spec.sections["verse"].bars);
            assert_eq!(later.chords, spec.sections["verse"].chords);
            assert_eq!(auris_compose_round_trip(&spec), spec);
        }
    }

    fn auris_compose_round_trip(spec: &crate::SongSpec) -> crate::SongSpec {
        crate::SongSpec::parse(&spec.to_toml()).unwrap()
    }
    use super::*;
    use crate::render::compose;
    use crate::spec::Role;
    use crate::theory::chord_scale::ChordScale;
    use auris_core::time::{TICKS_PER_QUARTER, Ticks};

    #[test]
    fn pop_band_builds_through_pre_choruses_and_returns_from_its_bridge() {
        let spec = preset("pop-band").unwrap().spec();
        for sequence in [
            ["verse", "pre", "chorus"],
            ["verse2", "pre2", "chorus2"],
            ["chorus2", "bridge", "chorus3"],
        ] {
            assert!(spec.form.windows(3).any(|window| window == sequence));
        }
        let pre = &spec.sections["pre"];
        let bridge = &spec.sections["bridge"];
        assert!(spec.sections["verse"].intensity < pre.intensity);
        assert!(pre.intensity < spec.sections["chorus"].intensity);
        assert!(bridge.intensity < spec.sections["chorus3"].intensity);
        assert_ne!(
            spec.chart_for(pre).bars,
            spec.chart_for(&spec.sections["verse"]).bars
        );
        assert_ne!(
            spec.chart_for(bridge).bars,
            spec.chart_for(&spec.sections["chorus"]).bars
        );
        for (later, original) in [("pre2", "pre"), ("chorus3", "chorus")] {
            assert_eq!(spec.sections[later].melody_from.as_deref(), Some(original));
            assert_eq!(spec.sections[later].bars, spec.sections[original].bars);
            assert_eq!(spec.sections[later].chords, spec.sections[original].chords);
        }
        let frame = crate::frame::plan(&spec);
        let piece = compose(&spec);
        for (name, bars) in [("pre", 4), ("pre2", 4), ("bridge", 8), ("chorus3", 8)] {
            let section = frame.sections.iter().find(|s| s.name == name).unwrap();
            let selected_melody = section.parts.iter().find(|part| {
                spec.parts.iter().any(|candidate| {
                    candidate.name == part.as_str() && candidate.role == Role::Melody
                })
            });
            let Some(selected_melody) = selected_melody else {
                panic!("{name} has no selected melody");
            };
            let track = piece
                .tracks
                .iter()
                .find(|track| track.name == selected_melody.as_str())
                .unwrap_or_else(|| panic!("{name} has no {} track", selected_melody));
            let clip = track
                .clips
                .iter()
                .find(|clip| clip.start == section.start)
                .unwrap_or_else(|| panic!("{name} has no selected melody clip"));
            assert!(!clip.notes.is_empty(), "{name} must contain music");
            assert_eq!(clip.length, spec.meter.ticks_per_bar() * bars);
        }
        let bridge = frame.sections.iter().find(|s| s.name == "bridge").unwrap();
        assert!(
            piece
                .tracks
                .iter()
                .filter(|t| !t.drum_parts.is_empty())
                .all(|track| track.clips.iter().all(|clip| clip.start != bridge.start))
        );
        assert_eq!(auris_compose_round_trip(&spec), spec);
    }

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
        // every piece, and hearing all eight was hearing one draw eight times. The numbers now
        // are *measured* choices — each preset's seed is the best of a sixteen-draw sweep on
        // the learned aesthetic score (`tools/eval`, 2026-08; the sweep moved mean enjoyment
        // by +0.20 where the dial search moved nothing) — but this test still asserts only
        // what it always did: no two the same, none left at the default, and a ninth preset
        // added without a seed fails here rather than quietly rejoining the pile. Choosing
        // *well* is the sweep's business, not this test's.
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
    fn every_part_names_an_instrument() {
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
    fn the_chiptune_presets_ask_for_no_soundfont_at_all() {
        assert_eq!(PRESETS[0].name, "chiptune");
        for name in ["chiptune", "game-loop"] {
            for part in preset(name).unwrap().spec().parts {
                assert_eq!(part.program, None, "{name} · {} asks for a font", part.name);
            }
        }
    }

    #[test]
    fn the_band_presets_give_the_verse_and_the_chorus_different_progressions() {
        // A verse and a chorus on one loop is a form whose arrangement changes at every join
        // over harmony that never does. The loop-built genres — synthwave, ambient, the jazz
        // trio on its one cadence — keep one progression *on purpose*, so this is asserted only
        // where the genre wants the contrast. City pop's contrast is the smallest kind: the same
        // loop with its ii–V spelled out, which is how the genre itself lifts a サビ.
        for name in ["pop-band", "rock", "city-pop"] {
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
        // Ambient has no percussion; orchestral uses pitched timpani instead of a kick.
        for preset in PRESETS {
            let spec = preset.spec();
            let has = |wanted: Role| spec.parts.iter().any(|part| part.role == wanted);
            assert!(has(Role::Bass), "{} has nothing underneath it", preset.name);
            assert!(
                has(Role::Kick) || matches!(preset.name, "ambient" | "orchestral"),
                "{} has no kit",
                preset.name
            );
        }
    }

    #[test]
    fn every_playing_section_declares_and_uses_its_arrangement() {
        for preset in PRESETS {
            let spec = preset.spec();
            let frame = crate::frame::plan(&spec);
            let declared: HashSet<&str> =
                spec.parts.iter().map(|part| part.name.as_str()).collect();
            let mut used = HashSet::new();
            for section in frame.sections.iter().filter(|section| !section.coda) {
                assert!(
                    !section.parts.is_empty(),
                    "{} · {} relies on the full roster",
                    preset.name,
                    section.name
                );
                for part in &section.parts {
                    assert!(
                        declared.contains(part.as_str()),
                        "{} names unknown part {part}",
                        preset.name
                    );
                    used.insert(part.as_str());
                }
            }
            assert_eq!(
                used, declared,
                "{} has a roster part that never plays",
                preset.name
            );
            let settings = crate::parts::ScoreSettings::from(&spec);
            let drafts = crate::parts::write_parts(&settings, &spec.parts, &frame);
            for draft in drafts {
                assert!(
                    draft.notes.iter().any(|note| {
                        frame
                            .sections
                            .get(note.section)
                            .is_some_and(|section| !section.coda)
                    }),
                    "{} · {} has no generated notes in a playing section",
                    preset.name,
                    draft.name
                );
            }
        }
    }

    #[test]
    fn band_arrangements_change_texture_and_rebuild_the_final_lift() {
        for name in ["pop-band", "city-pop", "rock", "synthwave", "chiptune"] {
            let spec = preset(name).unwrap().spec();
            let parts = |section: &str| {
                spec.sections
                    .get(section)
                    .unwrap_or_else(|| panic!("{name} has no {section}"))
                    .parts
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>()
            };
            assert_ne!(parts("verse"), parts("chorus"), "{name} has no verse lift");
            assert_ne!(
                parts("verse"),
                parts("verse2"),
                "{name} never changes its second verse"
            );
            if name == "pop-band" {
                assert!(parts("chorus3").len() >= parts("chorus").len());
            }
        }
    }

    #[test]
    fn game_loop_keeps_its_sixteen_bar_cycle_while_rotating_layers() {
        let spec = SongSpec::parse(include_str!("../../../examples/game-loop.asong")).unwrap();
        assert_eq!(spec.form, vec!["verse".to_string(), "chorus".to_string()]);
        assert_eq!(
            spec.form
                .iter()
                .map(|name| spec.sections[name].bars)
                .sum::<usize>(),
            16
        );
        assert_ne!(spec.sections["verse"].parts, spec.sections["chorus"].parts);
        let piece = compose(&spec);
        assert_eq!(piece.length, spec.meter.ticks_per_bar() * 16);
    }
}
