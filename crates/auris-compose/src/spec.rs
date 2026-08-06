//! The text a piece is asked for in.
//!
//! One document describes a whole song: its key, its tempo, its progression, its form and its
//! parts. It is deliberately a *specification* rather than a notation — it says what to write,
//! not what was written — because that is the layer where an instruction like "make the chorus
//! busier" is one word rather than four hundred edited notes.
//!
//! ```
//! # use auris_compose::spec::SongSpec;
//! let spec = SongSpec::parse(r#"
//!     key    = "C minor"
//!     tempo  = 128
//!     chords = "@marusa"
//! "#).unwrap();
//! assert_eq!(spec.tempo, 128.0);
//! ```
//!
//! # Why TOML
//!
//! The syntax is TOML and the extension is `.asong`, exactly as a project file is JSON inside
//! `.auris`. Three reasons it is not a format of its own:
//!
//! * A hand-written parser can only ever be read; serde can also **write**. A dialog that sets a
//!   song's dials has to be able to save what it was set to, and a format that has no serialiser
//!   makes that a second implementation of the same grammar, free to disagree with the first.
//! * It is not indentation-sensitive, which is the property a file a musician edits by hand
//!   most needs and the reason YAML was not chosen.
//! * `[section.chorus]` is a table and `[[part]]` is an array of tables, which is nearly the
//!   shape the format already had.
//!
//! Sections are a table keyed by name because their order means nothing — `form` decides what
//! plays when. Parts are an array because theirs means something: it is the order the tracks
//! are created in, and a map would sort `bass` above `lead` and quietly rearrange the mixer.
//!
//! # What is reported, and when
//!
//! Two kinds of complaint, reported differently on purpose.
//!
//! **Syntax** — a misspelt field, a number where a string belongs, a bracket that does not
//! close — is caught by `toml`, which stops at the first and says which line and column it is
//! on. Unknown fields are refused rather than ignored, because silently dropping a line would
//! mean the piece quietly ignores an instruction.
//!
//! **Meaning** — a key that is not a key, a fraction outside 0 to 1, a `form` naming a section
//! that does not exist — is caught after the document has stopped being text, and every such
//! complaint is reported at once. They have no line number for that reason; each names the
//! field, section or part it is about instead.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use auris_core::Subdivision;
use auris_core::time::TimeSignature;

use crate::rhythm::Pattern;
use crate::theory::chart::{Chart, ChartOrigin};
use crate::theory::key::Key;
use crate::theory::scale::ScaleId;

/// What a part does in the arrangement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The tune.
    Melody,
    /// Sustained or rhythmic chords.
    Chords,
    /// A held chord bed.
    Pad,
    /// A broken chord.
    Arp,
    /// Short chords hammered on the subdivision.
    Stab,
    /// The bass line.
    Bass,
    /// The kick drum.
    Kick,
    /// The snare.
    Snare,
    /// The hi-hat.
    Hat,
}

impl Role {
    /// Every role, in the order a default roster uses them.
    pub const ALL: [Role; 9] = [
        Role::Melody,
        Role::Chords,
        Role::Pad,
        Role::Arp,
        Role::Stab,
        Role::Bass,
        Role::Kick,
        Role::Snare,
        Role::Hat,
    ];

    /// The name the text format writes.
    pub fn name(self) -> &'static str {
        match self {
            Role::Melody => "melody",
            Role::Chords => "chords",
            Role::Pad => "pad",
            Role::Arp => "arp",
            Role::Stab => "stab",
            Role::Bass => "bass",
            Role::Kick => "kick",
            Role::Snare => "snare",
            Role::Hat => "hat",
        }
    }

    /// Reads a role name, accepting the obvious synonyms.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "melody" | "lead" | "tune" => Role::Melody,
            "chords" | "comp" | "harmony" => Role::Chords,
            "pad" | "strings" => Role::Pad,
            "arp" | "arpeggio" => Role::Arp,
            "stab" | "stabs" | "release-cut" => Role::Stab,
            "bass" => Role::Bass,
            "kick" | "bd" => Role::Kick,
            "snare" | "sd" => Role::Snare,
            "hat" | "hihat" | "hh" => Role::Hat,
            _ => return None,
        })
    }

    /// `true` when the part plays a drum rather than a pitch.
    pub fn is_drum(self) -> bool {
        matches!(self, Role::Kick | Role::Snare | Role::Hat)
    }

    /// The instrument a part of this role gets when none is named.
    pub fn default_instrument(self) -> &'static str {
        if self.is_drum() {
            "auris.synth.noisedrum"
        } else if matches!(self, Role::Bass | Role::Pad) {
            "auris.synth.fm2"
        } else {
            "auris.synth.chiptune"
        }
    }

    /// The octave a part of this role sits in by default.
    pub fn default_octave(self) -> i32 {
        match self {
            Role::Melody | Role::Arp | Role::Stab => 5,
            Role::Chords => 4,
            Role::Pad => 3,
            Role::Bass => 2,
            _ => 3,
        }
    }

    /// How long a note of this role is held, as a fraction of the gap to the one after it.
    ///
    /// Legato everywhere but the stab, which is nothing *but* its gate: cut the release off a
    /// chord struck on every sixteenth and the rhythm is the sound, leave it on and the sixteen
    /// chords in the bar overlap into one wash that could have been a single held note.
    pub fn default_gate(self) -> f32 {
        match self {
            Role::Stab => 0.3,
            _ => 1.0,
        }
    }

    /// Where a part of this role sits across the stereo image, from -1 to 1.
    ///
    /// Six parts stacked in the middle are six parts fighting for the same space, and the fix a
    /// mix engineer reaches for first is to move them apart. What stays in the centre is what a
    /// listener localises the song by — the tune, the bass and the kick — and what moves is the
    /// accompaniment. Nothing goes hard over: a part at the edge of the image disappears on a
    /// mono speaker, and a phone is a mono speaker.
    ///
    /// A default rather than a decision, the same way [`Self::default_gain_db`] is: a
    /// specification that writes `pan` gets what it asked for.
    pub fn default_pan(self) -> f32 {
        match self {
            Role::Melody | Role::Bass | Role::Kick | Role::Snare => 0.0,
            Role::Chords => -0.25,
            Role::Pad => 0.2,
            Role::Arp => 0.3,
            Role::Stab => -0.3,
            Role::Hat => 0.25,
        }
    }

    /// The level a part of this role sits at, in decibels.
    ///
    /// Six parts all at unity sum well past full scale. These are the rough balances a mix
    /// engineer would reach for first: the tune on top, the pad and the hat well under it.
    pub fn default_gain_db(self) -> f32 {
        match self {
            Role::Melody => -7.0,
            Role::Chords => -14.0,
            Role::Pad => -16.0,
            Role::Arp => -12.0,
            Role::Stab => -13.0,
            Role::Bass => -10.0,
            Role::Kick => -10.0,
            Role::Snare => -12.0,
            Role::Hat => -20.0,
        }
    }

    /// The MIDI range a part of this role should stay inside.
    pub fn range(self) -> (i32, i32) {
        match self {
            Role::Melody => (60, 84),
            Role::Arp => (60, 88),
            Role::Chords => (48, 72),
            // High and narrow, which is where a stab has to sit: it is competing with the tune
            // for attention rather than filling in underneath it, and a wide voicing struck
            // sixteen times a bar would bury everything else in the mix.
            Role::Stab => (60, 84),
            Role::Pad => (36, 64),
            Role::Bass => (28, 52),
            _ => (0, 127),
        }
    }
}

/// How the piece should feel.
///
/// Four numbers rather than a list of genre names: a genre is a point in this space, and a
/// number can be nudged. Every one runs from 0 to 1.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mood {
    /// Dark to bright. Chooses the scale when one is not named, and the register.
    pub brightness: f32,
    /// Calm to driving. Sets note density and how hard the drums hit.
    pub energy: f32,
    /// Plain to coloured. Governs sevenths, ninths and borrowed chords.
    pub tension: f32,
    /// Straight to syncopated.
    pub syncopation: f32,
}

impl Default for Mood {
    fn default() -> Self {
        Self {
            brightness: 0.5,
            energy: 0.5,
            tension: 0.35,
            syncopation: 0.3,
        }
    }
}

impl Mood {
    /// The mood a named feeling means.
    ///
    /// A vocabulary rather than a free-text field, because "make it sadder" has to land on
    /// numbers eventually and the mapping should be visible rather than guessed at.
    pub fn named(name: &str) -> Option<Self> {
        let base = Mood::default();
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "neutral" => base,
            "bright" | "happy" => Mood {
                brightness: 0.85,
                energy: 0.65,
                tension: 0.25,
                syncopation: 0.3,
            },
            "dark" | "sad" => Mood {
                brightness: 0.15,
                energy: 0.35,
                tension: 0.45,
                syncopation: 0.2,
            },
            "calm" | "ambient" => Mood {
                brightness: 0.6,
                energy: 0.15,
                tension: 0.3,
                syncopation: 0.1,
            },
            "driving" | "energetic" => Mood {
                brightness: 0.6,
                energy: 0.9,
                tension: 0.35,
                syncopation: 0.5,
            },
            "epic" | "heroic" => Mood {
                brightness: 0.45,
                energy: 0.85,
                tension: 0.5,
                syncopation: 0.25,
            },
            "dreamy" | "floating" => Mood {
                brightness: 0.7,
                energy: 0.3,
                tension: 0.7,
                syncopation: 0.35,
            },
            "tense" | "anxious" => Mood {
                brightness: 0.2,
                energy: 0.6,
                tension: 0.85,
                syncopation: 0.55,
            },
            "funky" | "groovy" => Mood {
                brightness: 0.6,
                energy: 0.75,
                tension: 0.55,
                syncopation: 0.85,
            },
            _ => return None,
        })
    }

    /// Every mood word, for a listing and for an error message.
    pub const NAMES: [&'static str; 9] = [
        "neutral", "bright", "dark", "calm", "driving", "epic", "dreamy", "tense", "funky",
    ];

    /// How likely a plain chord is to gain a seventh.
    pub fn seventh_rate(self) -> f32 {
        self.tension * 0.8
    }

    /// How likely a chord that has a seventh is to gain a ninth.
    pub fn ninth_rate(self) -> f32 {
        (self.tension - 0.4).max(0.0) * 0.7
    }

    /// How likely a chord is to be swapped for the parallel mode's.
    pub fn borrow_rate(self) -> f32 {
        (self.tension - 0.5).max(0.0) * 0.4
    }

    /// How many notes a bar wants, as a fraction of the available steps.
    pub fn density(self) -> f32 {
        0.15 + self.energy * 0.5
    }

    /// The scale that best matches this brightness, when none was named.
    pub fn scale(self) -> ScaleId {
        // Ordered dark to bright; the same ordering `ScaleId::brightness` reports.
        const LADDER: [ScaleId; 7] = [
            ScaleId::Phrygian,
            ScaleId::Minor,
            ScaleId::Dorian,
            ScaleId::MinorPentatonic,
            ScaleId::Mixolydian,
            ScaleId::Major,
            ScaleId::Lydian,
        ];
        let index = (self.brightness * LADDER.len() as f32) as usize;
        LADDER[index.min(LADDER.len() - 1)]
    }
}

/// One part of the arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSpec {
    /// The name it takes in the document and on its track.
    pub name: String,
    /// What it plays.
    pub role: Role,
    /// The plugin that plays it.
    pub instrument: String,
    /// Which octave it sits in, as an **absolute** MIDI octave rather than an offset.
    ///
    /// A melody's default is 5, so 6 moves it up one and 1 moves it down four. Worth saying
    /// plainly, because the other octave in this system —
    /// [`ClipRecipe::octave`](auris_core::ClipRecipe::octave), the dial on a generated clip — is
    /// a *relative* ±2 from wherever its preset sits, and the two are easy to write for each
    /// other. [`Self::range`] is where the difference from the role's default becomes a shift.
    pub octave: i32,
    /// How busy it is, as a fraction of the available steps.
    pub density: Option<f32>,
    /// How finely this part divides the beat.
    ///
    /// Per part rather than per song: a stab hammering triplets over a straight kit is a sound
    /// somebody wants, and the bar is the same length either way, so the parts still line up.
    /// A drum part ignores it — a groove is written in sixteenths.
    pub subdivision: Subdivision,
    /// How long a note is held, as a fraction of the gap to the one after it.
    pub gate: f32,
    /// A rhythm written out by hand, which overrides the generated one.
    pub rhythm: Option<Pattern>,
    /// Level trim in decibels.
    pub gain_db: f32,
    /// Stereo position from -1 to 1.
    pub pan: f32,
}

impl PartSpec {
    /// A part with everything its role implies.
    pub fn of_role(name: impl Into<String>, role: Role) -> Self {
        Self {
            name: name.into(),
            role,
            instrument: role.default_instrument().to_string(),
            octave: role.default_octave(),
            density: None,
            subdivision: Subdivision::default(),
            gate: role.default_gate(),
            rhythm: None,
            gain_db: role.default_gain_db(),
            pan: role.default_pan(),
        }
    }

    /// The MIDI range this part should stay inside, moved by its octave.
    pub fn range(&self) -> (i32, i32) {
        let (low, high) = self.role.range();
        let shift = (self.octave - self.role.default_octave()) * 12;
        (low + shift, high + shift)
    }
}

/// One section of the form.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionSpec {
    /// The name used in `form:` and on the clips.
    pub name: String,
    /// How many bars it lasts.
    pub bars: usize,
    /// Which chart it plays, by name.
    pub chords: String,
    /// How hard the section is played, from 0 to 1.
    pub intensity: f32,
    /// Which parts play; empty means all of them.
    pub parts: Vec<String>,
    /// Semitones to transpose this section by.
    pub transpose: i32,
}

impl SectionSpec {
    /// A section with the defaults its name implies.
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        let intensity = match name.as_str() {
            "intro" => 0.30,
            "verse" => 0.55,
            "pre" => 0.70,
            "chorus" => 0.90,
            "bridge" => 0.60,
            "break" => 0.35,
            "solo" => 0.75,
            "outro" => 0.25,
            _ => 0.60,
        };
        Self {
            name,
            bars: 8,
            chords: "main".to_string(),
            intensity,
            parts: Vec::new(),
            transpose: 0,
        }
    }
}

/// A whole song, as asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct SongSpec {
    /// What the piece is called.
    pub title: String,
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
    pub meter: TimeSignature,
    /// The key everything is measured from.
    pub key: Key,
    /// How the piece should feel.
    pub mood: Mood,
    /// The seed every random decision is drawn from.
    pub seed: u64,
    /// How much the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander, from 0 for a machine to 1 for a sloppy band.
    pub humanize: f32,
    /// How far apart the hardest and softest notes are struck, from 0 to 1.
    ///
    /// How much the playing varies, where [`Self::mood`]'s energy says how hard it is played at
    /// all. At 0 every note is struck alike — a sequencer, which is sometimes the point.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill, from 0 to 1.
    pub fill: f32,
    /// How much a repeat departs from what the section played the first time.
    ///
    /// At 0 a second chorus is note for note the first one, which is what makes it recognisable
    /// as the same chorus. At 1 every playing is written afresh, which is what the composer used
    /// to do always — the result had no repetition anywhere in it and so nothing to remember.
    /// The default leaves most of the material alone and rewrites the occasional bar.
    pub variation: f32,
    /// The drum groove.
    pub groove: String,
    /// The charts, by name. `main` is the one a section gets when it does not say.
    pub charts: BTreeMap<String, Chart>,
    /// The sections, by name.
    pub sections: BTreeMap<String, SectionSpec>,
    /// The order the sections play in.
    pub form: Vec<String>,
    /// The parts, in the order their tracks are created.
    pub parts: Vec<PartSpec>,
}

impl Default for SongSpec {
    fn default() -> Self {
        let mut charts = BTreeMap::new();
        // Marked generated, not quoted: a progression the user did not ask for is the composer's
        // own, so the mood is free to colour it. A chart anyone typed or named is left alone.
        let default_chart = Chart::parse("@axis")
            .map(|chart| Chart::new(chart.bars, ChartOrigin::Generated))
            .unwrap_or_else(|| Chart::new(Vec::new(), ChartOrigin::Generated));
        charts.insert("main".to_string(), default_chart);
        let mut sections = BTreeMap::new();
        for name in ["intro", "verse", "chorus", "outro"] {
            sections.insert(name.to_string(), SectionSpec::named(name));
        }
        Self {
            title: "Untitled".to_string(),
            tempo: 120.0,
            meter: TimeSignature::default(),
            key: Key::parse("C major").expect("C major is a key"),
            mood: Mood::default(),
            seed: 0,
            swing: 50,
            humanize: 0.35,
            dynamics: 1.0,
            fill: 0.5,
            variation: 0.25,
            groove: "basic-rock".to_string(),
            charts,
            sections,
            form: ["intro", "verse", "chorus", "verse", "chorus", "outro"]
                .iter()
                .map(|name| name.to_string())
                .collect(),
            parts: vec![
                PartSpec::of_role("lead", Role::Melody),
                PartSpec::of_role("chords", Role::Chords),
                PartSpec::of_role("bass", Role::Bass),
                PartSpec::of_role("kick", Role::Kick),
                PartSpec::of_role("snare", Role::Snare),
                PartSpec::of_role("hat", Role::Hat),
            ],
        }
    }
}

/// Something wrong with a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecError {
    /// One-based line number, where the reader knows one.
    ///
    /// A syntax error has a line because `toml` reports one. A complaint about *meaning* does
    /// not: it is found after the document has stopped being text. Those name the field,
    /// section or part they are about in the message instead.
    pub line: Option<usize>,
    /// What went wrong.
    pub message: String,
}

impl SpecError {
    /// A complaint about a particular line.
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    /// A complaint about what the document means, which no one line holds.
    fn about(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for SpecError {}

/// How long the whole piece is, in bars.
impl SongSpec {
    /// Total length in bars.
    pub fn total_bars(&self) -> usize {
        self.form
            .iter()
            .filter_map(|name| self.sections.get(name))
            .map(|section| section.bars)
            .sum()
    }

    /// The chart a section plays, falling back to `main` and then to anything at all.
    pub fn chart_for(&self, section: &SectionSpec) -> Chart {
        self.charts
            .get(&section.chords)
            .or_else(|| self.charts.get("main"))
            .or_else(|| self.charts.values().next())
            .cloned()
            .unwrap_or_else(|| Chart::new(Vec::new(), ChartOrigin::Given))
    }

    /// The parts that play in a section.
    pub fn parts_in(&self, section: &SectionSpec) -> Vec<&PartSpec> {
        self.parts
            .iter()
            .filter(|part| section.parts.is_empty() || section.parts.contains(&part.name))
            .collect()
    }

    /// Reads a document, filling in everything it does not say.
    ///
    /// A document is never rejected for being short: two lines are a valid song, and every
    /// field left out is answered by the defaults. What *is* rejected is a field the format
    /// does not understand, because silently ignoring it would mean the piece quietly ignores
    /// an instruction.
    pub fn parse(text: &str) -> Result<Self, Vec<SpecError>> {
        Self::parse_with_overrides(text, &[])
    }

    /// Reads a document with a handful of its top-level fields replaced.
    ///
    /// This is what `--tempo 96` on a command line does. It cannot be an extra line appended to
    /// the document, the way it was when the format was line-oriented, because TOML refuses a
    /// key written twice — so the override is applied to the document once it is read.
    ///
    /// Each value is written the way a person types it rather than the way TOML quotes it:
    /// `("key", "D minor")`, not `("key", "\"D minor\"")`. Anything TOML can read as a value on
    /// its own is that value, and anything it cannot is a string.
    pub fn parse_with_overrides(
        text: &str,
        overrides: &[(String, String)],
    ) -> Result<Self, Vec<SpecError>> {
        // A byte-order mark is invisible in an editor and would make the first field unreadable.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        // Read from the text rather than through a `toml::Table`, so a syntax error keeps the
        // line and column `toml` found it at.
        let doc: SongDoc =
            toml::from_str(text).map_err(|error| vec![syntax_error(text, &error)])?;
        let doc = if overrides.is_empty() {
            doc
        } else {
            apply_overrides(doc, overrides)?
        };
        doc.into_spec()
    }

    /// The document this specification would be written as.
    ///
    /// Only what differs from a default is written, so what comes out of a dialog is about as
    /// short as what a person would have typed. Round-tripping matters for the agent case too:
    /// a tool can read a specification back, change one field and send it again without having
    /// to hold the whole document in its head.
    pub fn to_toml(&self) -> String {
        // `to_string` rather than `to_string_pretty`, which puts every element of an array on a
        // line of its own: a six-section form is one readable line and not six.
        toml::to_string(&SongDoc::from(self))
            .expect("every field of a specification is a TOML value")
    }
}

/// The role a part's name implies, so a part called `bass` needs no `role`.
fn infer_role(name: &str) -> Role {
    Role::parse(name).unwrap_or(Role::Melody)
}

/// The one complaint `toml` stopped at, with the line it is on.
fn syntax_error(text: &str, error: &toml::de::Error) -> SpecError {
    let line = error
        .span()
        .and_then(|span| text.get(..span.start))
        // Counting newlines rather than `lines()`, which reports 2 for both `"a\nb"` and
        // `"a\nb\n"` and would put an error at the start of a line on the one before it.
        .map(|before| before.bytes().filter(|byte| *byte == b'\n').count() + 1);
    match line {
        Some(line) => SpecError::at(line, error.message()),
        None => SpecError::about(error.message()),
    }
}

/// A document with some of its top-level fields replaced.
fn apply_overrides(
    doc: SongDoc,
    overrides: &[(String, String)],
) -> Result<SongDoc, Vec<SpecError>> {
    let mut table =
        toml::Table::try_from(doc).expect("a document that was just read is representable as TOML");
    for (field, value) in overrides {
        table.insert(field.clone(), toml_value(value));
    }
    table
        .try_into()
        .map_err(|error: toml::de::Error| vec![SpecError::about(error.message())])
}

/// A value as it was typed, read as TOML where TOML can read it.
///
/// `128` is a number and `D minor` is not, and the difference is exactly whether TOML can read
/// the text as a value on its own. Anything it cannot is a string — which is what somebody
/// typing `--key "D minor"` means, and what quoting it themselves would have produced.
fn toml_value(text: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("value = {text}"))
        .ok()
        .and_then(|mut table| table.remove("value"))
        .unwrap_or_else(|| toml::Value::String(text.to_string()))
}

/// Takes a fraction that was written, or records why it could not be.
fn fraction_into(target: &mut f32, value: Option<f32>, what: &str, errors: &mut Vec<SpecError>) {
    let Some(value) = value else { return };
    if (0.0..=1.0).contains(&value) {
        *target = value;
    } else {
        errors.push(SpecError::about(format!(
            "{what} runs from 0 to 1, not {value}"
        )));
    }
}

/// Reads `4/4`.
fn parse_meter(text: &str) -> Result<TimeSignature, String> {
    let (top, bottom) = text
        .split_once('/')
        .ok_or_else(|| format!("`{text}` is not a meter like 4/4"))?;
    let numerator: u32 = top
        .trim()
        .parse()
        .map_err(|_| format!("`{top}` is not a beat count"))?;
    let denominator: u32 = bottom
        .trim()
        .parse()
        .map_err(|_| format!("`{bottom}` is not a beat value"))?;
    if !(1..=32).contains(&numerator) || !matches!(denominator, 1 | 2 | 4 | 8 | 16) {
        return Err(format!(
            "`{text}` is not a meter this can count; the beat count runs from 1 to 32"
        ));
    }
    Ok(TimeSignature::new(numerator, denominator))
}

/// A list, or the same list written as one string.
///
/// `["intro", "verse"]` is what TOML wants and what [`SongSpec::to_toml`] writes. `"intro
/// verse"` is accepted as well, because that is the shape an override arrives from a command
/// line in, and because a six-word form reads better on one line than as six quoted strings.
fn words_or_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Words(String),
        List(Vec<String>),
    }

    Ok(match Option::<Either>::deserialize(deserializer)? {
        None => None,
        Some(Either::List(list)) => Some(list),
        Some(Either::Words(words)) => Some(words.split_whitespace().map(str::to_string).collect()),
    })
}

/// The TOML document, and nothing else.
///
/// Kept apart from [`SongSpec`] on purpose. Everything the format does that serde cannot — a
/// value spelt the way a musician spells it, a field whose default comes from another field, a
/// mood word that sets four dials at once — happens in the conversion between the two, and so
/// happens in one readable place rather than spread through attributes.
///
/// Field order is the order it is written in, and TOML wants every plain value before the first
/// table, which is why `harmony`, `section` and `part` are last.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SongDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scale: Option<String>,
    #[serde(default, alias = "bpm", skip_serializing_if = "Option::is_none")]
    tempo: Option<f64>,
    #[serde(default, alias = "time", skip_serializing_if = "Option::is_none")]
    meter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    groove: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    swing: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    humanize: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dynamics: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fill: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mood: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brightness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    energy: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tension: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    syncopation: Option<f32>,
    #[serde(
        default,
        alias = "progression",
        skip_serializing_if = "Option::is_none"
    )]
    chords: Option<String>,
    #[serde(
        default,
        deserialize_with = "words_or_list",
        skip_serializing_if = "Option::is_none"
    )]
    form: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    harmony: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    section: BTreeMap<String, SectionDoc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    part: Vec<PartDoc>,
}

/// One `[section.name]` table.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chords: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intensity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transpose: Option<i32>,
    #[serde(
        default,
        deserialize_with = "words_or_list",
        skip_serializing_if = "Option::is_none"
    )]
    parts: Option<Vec<String>>,
}

/// One `[[part]]` table.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartDoc {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instrument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    octave: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdivision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rhythm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gain: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pan: Option<f32>,
}

impl SongDoc {
    /// What the document means, or every reason it means nothing.
    fn into_spec(self) -> Result<SongSpec, Vec<SpecError>> {
        let mut spec = SongSpec::default();
        let mut errors = Vec::new();

        if let Some(title) = self.title {
            spec.title = title;
        }
        if let Some(text) = &self.key {
            match Key::parse(text) {
                Some(key) => spec.key = key,
                None => errors.push(SpecError::about(format!("`{text}` is not a key"))),
            }
        }
        // After the key, so `key = "C minor"` beside `scale = "dorian"` is C dorian whichever
        // order the two are written in. A table has no line order to lean on.
        if let Some(text) = &self.scale {
            match ScaleId::parse(text) {
                Some(scale) => spec.key = Key::new(spec.key.tonic, scale),
                None => errors.push(SpecError::about(format!("`{text}` is not a scale"))),
            }
        }
        if let Some(tempo) = self.tempo {
            if (20.0..=400.0).contains(&tempo) {
                spec.tempo = tempo;
            } else {
                errors.push(SpecError::about(format!(
                    "a tempo of {tempo} is outside 20..400"
                )));
            }
        }
        if let Some(text) = &self.meter {
            match parse_meter(text) {
                Ok(meter) => spec.meter = meter,
                Err(message) => errors.push(SpecError::about(message)),
            }
        }
        if let Some(seed) = self.seed {
            spec.seed = seed;
        }
        if let Some(text) = &self.groove {
            if crate::rhythm::groove(text).is_some() {
                spec.groove = text.clone();
            } else {
                let names: Vec<&str> = crate::rhythm::GROOVES.iter().map(|g| g.name).collect();
                errors.push(SpecError::about(format!(
                    "`{text}` is not a groove; try one of {}",
                    names.join(", ")
                )));
            }
        }
        if let Some(swing) = self.swing {
            if (20..=90).contains(&swing) {
                spec.swing = swing as u8;
            } else {
                errors.push(SpecError::about(format!(
                    "swing runs from 20 to 90, not {swing}"
                )));
            }
        }
        fraction_into(&mut spec.humanize, self.humanize, "humanize", &mut errors);
        fraction_into(&mut spec.dynamics, self.dynamics, "dynamics", &mut errors);
        fraction_into(&mut spec.fill, self.fill, "fill", &mut errors);
        fraction_into(
            &mut spec.variation,
            self.variation,
            "variation",
            &mut errors,
        );

        // The word is the base and the dials are the trim, whatever order they appear in: a
        // mood word means four numbers, and naming one of them says which of the four to move.
        if let Some(text) = &self.mood {
            match Mood::named(text) {
                Some(mood) => spec.mood = mood,
                None => errors.push(SpecError::about(format!(
                    "`{text}` is not a mood; try one of {}",
                    Mood::NAMES.join(", ")
                ))),
            }
        }
        let mood = &mut spec.mood;
        fraction_into(
            &mut mood.brightness,
            self.brightness,
            "brightness",
            &mut errors,
        );
        fraction_into(&mut mood.energy, self.energy, "energy", &mut errors);
        fraction_into(&mut mood.tension, self.tension, "tension", &mut errors);
        fraction_into(
            &mut mood.syncopation,
            self.syncopation,
            "syncopation",
            &mut errors,
        );

        // `chords` is the shortest possible way to name a progression, and `[harmony]` the
        // general one. Both are merged into the defaults rather than replacing them, so a
        // document with one of each keeps both.
        if let Some(text) = &self.chords {
            match Chart::parse(text) {
                Some(chart) => {
                    spec.charts.insert("main".to_string(), chart);
                }
                None => errors.push(SpecError::about(format!("`{text}` is not a chord chart"))),
            }
        }
        for (name, text) in &self.harmony {
            match Chart::parse(text) {
                Some(chart) => {
                    spec.charts.insert(name.clone(), chart);
                }
                None => errors.push(SpecError::about(format!(
                    "harmony `{name}`: `{text}` is not a chord chart"
                ))),
            }
        }

        // A document that declares any section or part replaces the defaults entirely rather
        // than adding to them: a roster half inherited from a default is impossible to reason
        // about.
        if !self.section.is_empty() {
            spec.sections = self
                .section
                .into_iter()
                .map(|(name, doc)| {
                    let section = doc.into_spec(&name, &mut errors);
                    (name, section)
                })
                .collect();
        }
        if !self.part.is_empty() {
            let mut parts: Vec<PartSpec> = Vec::new();
            for doc in self.part {
                if parts.iter().any(|part| part.name == doc.name) {
                    errors.push(SpecError::about(format!(
                        "`{}` is already a part; two of the same name would silently merge",
                        doc.name
                    )));
                    continue;
                }
                parts.push(doc.into_spec(&mut errors));
            }
            spec.parts = parts;
        }
        if let Some(form) = self.form {
            spec.form = form;
        }

        // A section named in the form but never described still has to exist, or the form would
        // silently skip it.
        for name in &spec.form {
            spec.sections
                .entry(name.clone())
                .or_insert_with(|| SectionSpec::named(name));
        }
        if spec.form.is_empty() {
            errors.push(SpecError::about(
                "the form is empty, so there is nothing to write",
            ));
        }

        // A name that does not resolve would otherwise be answered by a silent substitution: a
        // section would play a progression nobody asked for, or fall silent for want of a part.
        let part_names: Vec<&str> = spec.parts.iter().map(|part| part.name.as_str()).collect();
        for (name, section) in &spec.sections {
            if !spec.charts.contains_key(&section.chords) {
                errors.push(SpecError::about(format!(
                    "section `{name}` plays `{}`, which is not a chart; there is {}",
                    section.chords,
                    spec.charts
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            for part in &section.parts {
                if !part_names.contains(&part.as_str()) {
                    errors.push(SpecError::about(format!(
                        "section `{name}` names the part `{part}`, which does not exist; there \
                         is {}",
                        part_names.join(", ")
                    )));
                }
            }
        }

        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }
}

impl SectionDoc {
    fn into_spec(self, name: &str, errors: &mut Vec<SpecError>) -> SectionSpec {
        let mut section = SectionSpec::named(name);
        if let Some(bars) = self.bars {
            if (1..=512).contains(&bars) {
                section.bars = bars;
            } else {
                errors.push(SpecError::about(format!(
                    "section `{name}`: a section of {bars} bars is not playable"
                )));
            }
        }
        if let Some(chords) = self.chords {
            section.chords = chords;
        }
        fraction_into(
            &mut section.intensity,
            self.intensity,
            &format!("section `{name}`: intensity"),
            errors,
        );
        if let Some(semitones) = self.transpose {
            if (-24..=24).contains(&semitones) {
                section.transpose = semitones;
            } else {
                errors.push(SpecError::about(format!(
                    "section `{name}`: a transposition of {semitones} semitones is extreme"
                )));
            }
        }
        if let Some(parts) = self.parts {
            // `*` is how the line-oriented format said "everything", which is what an empty
            // list already means. Still accepted, so nobody's document breaks over a star.
            section.parts = parts.into_iter().filter(|name| name != "*").collect();
        }
        section
    }
}

impl PartDoc {
    fn into_spec(self, errors: &mut Vec<SpecError>) -> PartSpec {
        let name = self.name;
        let role = match &self.role {
            None => infer_role(&name),
            Some(text) => Role::parse(text).unwrap_or_else(|| {
                let names: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
                errors.push(SpecError::about(format!(
                    "part `{name}`: `{text}` is not a role; try one of {}",
                    names.join(", ")
                )));
                infer_role(&name)
            }),
        };
        // Everything the role implies first, then whatever the document said instead — which is
        // what makes `role = "bass"` on its own enough to describe a bass part.
        let mut part = PartSpec::of_role(name.as_str(), role);
        if let Some(instrument) = self.instrument {
            part.instrument = instrument;
        }
        if let Some(octave) = self.octave {
            if (-1..=9).contains(&octave) {
                part.octave = octave;
            } else {
                errors.push(SpecError::about(format!(
                    "part `{name}`: octave {octave} is outside the MIDI range"
                )));
            }
        }
        if let Some(density) = self.density {
            if (0.0..=1.0).contains(&density) {
                part.density = Some(density);
            } else {
                errors.push(SpecError::about(format!(
                    "part `{name}`: density runs from 0 to 1, not {density}"
                )));
            }
        }
        if let Some(text) = &self.subdivision {
            match Subdivision::parse(text) {
                Some(subdivision) => part.subdivision = subdivision,
                None => errors.push(SpecError::about(format!(
                    "part `{name}`: `{text}` is not a subdivision; use 8, 16, 8t or 16t"
                ))),
            }
        }
        if let Some(gate) = self.gate {
            if !(0.0..=1.0).contains(&gate) {
                errors.push(SpecError::about(format!(
                    "part `{name}`: gate runs from 0 to 1, not {gate}"
                )));
            } else if gate <= 0.0 {
                // Zero would write a note of no length at every onset: a part that is silent
                // and still costs a voice, which reads as a bug wherever it is met.
                errors.push(SpecError::about(format!(
                    "part `{name}`: a gate of {gate} would write notes nobody can hear"
                )));
            } else {
                part.gate = gate;
            }
        }
        if let Some(text) = &self.rhythm {
            match Pattern::parse(text) {
                Some(pattern) => part.rhythm = Some(pattern),
                None => errors.push(SpecError::about(format!(
                    "part `{name}`: `{text}` is not a rhythm; use x, X, o and ~"
                ))),
            }
        }
        if let Some(gain) = self.gain {
            if (-60.0..=12.0).contains(&gain) {
                part.gain_db = gain as f32;
            } else {
                errors.push(SpecError::about(format!(
                    "part `{name}`: a gain of {gain} dB is outside -60..12"
                )));
            }
        }
        if let Some(pan) = self.pan {
            if (-1.0..=1.0).contains(&pan) {
                part.pan = pan;
            } else {
                errors.push(SpecError::about(format!(
                    "part `{name}`: pan runs from -1 to 1, not {pan}"
                )));
            }
        }
        part
    }
}

impl From<&SongSpec> for SongDoc {
    fn from(spec: &SongSpec) -> Self {
        Self {
            title: Some(spec.title.clone()),
            key: Some(spec.key.to_text()),
            // The key already carries the scale, and writing both would be two chances to
            // disagree.
            scale: None,
            tempo: Some(spec.tempo),
            meter: Some(format!(
                "{}/{}",
                spec.meter.numerator, spec.meter.denominator
            )),
            seed: Some(spec.seed),
            groove: Some(spec.groove.clone()),
            swing: Some(u32::from(spec.swing)),
            humanize: Some(spec.humanize),
            dynamics: Some(spec.dynamics),
            fill: Some(spec.fill),
            variation: Some(spec.variation),
            // The four numbers are what a mood word *means*; writing the word too would let a
            // document say `mood = "dark"` beside a brightness the word does not have.
            mood: None,
            brightness: Some(spec.mood.brightness),
            energy: Some(spec.mood.energy),
            tension: Some(spec.mood.tension),
            syncopation: Some(spec.mood.syncopation),
            // Written under `[harmony]`, which can hold all of them rather than only `main`.
            chords: None,
            form: Some(spec.form.clone()),
            harmony: spec
                .charts
                .iter()
                // A chart the composer invented is not something the document said. Leaving it
                // out is what gives the composer the same freedom when this is read back — a
                // written one would come back marked as quoted, and never be coloured again.
                .filter(|(_, chart)| chart.origin == ChartOrigin::Given)
                // A quotation is written back as the quotation. Spelling its bars out would be
                // longer to read and would lose the *mode* it was written in, which is what
                // lets 丸サ進行 be asked for in a minor key and still be 丸サ進行.
                .map(|(name, chart)| {
                    let text = match &chart.quoted_as {
                        Some(quoted) => format!("@{quoted}"),
                        None => chart.to_string(),
                    };
                    (name.clone(), text)
                })
                .collect(),
            section: spec
                .sections
                .iter()
                .map(|(name, section)| (name.clone(), SectionDoc::from_spec(name, section)))
                .collect(),
            part: spec.parts.iter().map(PartDoc::from_spec).collect(),
        }
    }
}

impl SectionDoc {
    /// Only what a section's own name does not already imply.
    fn from_spec(name: &str, section: &SectionSpec) -> Self {
        let plain = SectionSpec::named(name);
        Self {
            bars: (section.bars != plain.bars).then_some(section.bars),
            chords: (section.chords != plain.chords).then(|| section.chords.clone()),
            intensity: (section.intensity != plain.intensity).then_some(section.intensity),
            transpose: (section.transpose != 0).then_some(section.transpose),
            parts: (!section.parts.is_empty()).then(|| section.parts.clone()),
        }
    }
}

impl PartDoc {
    /// Only what a part's own name and role do not already imply.
    fn from_spec(part: &PartSpec) -> Self {
        let plain = PartSpec::of_role(part.name.as_str(), part.role);
        Self {
            name: part.name.clone(),
            role: (infer_role(&part.name) != part.role).then(|| part.role.name().to_string()),
            instrument: (part.instrument != plain.instrument).then(|| part.instrument.clone()),
            octave: (part.octave != plain.octave).then_some(part.octave),
            density: part.density,
            subdivision: (part.subdivision != plain.subdivision)
                .then(|| part.subdivision.name().to_string()),
            gate: (part.gate != plain.gate).then_some(part.gate),
            rhythm: part.rhythm.as_ref().map(Pattern::to_text),
            gain: (part.gain_db != plain.gain_db).then_some(f64::from(part.gain_db)),
            pan: (part.pan != plain.pan).then_some(part.pan),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_document_is_a_whole_song() {
        // The shortest possible spec: nothing at all. Every field is answered by a default, and
        // the result is playable rather than empty.
        let spec = SongSpec::parse("").unwrap();
        assert_eq!(spec.tempo, 120.0);
        assert_eq!(spec.key.to_text(), "C major");
        assert!(!spec.form.is_empty());
        assert!(!spec.parts.is_empty());
        assert_eq!(spec.total_bars(), 48, "six eight-bar sections");
    }

    #[test]
    fn two_fields_change_only_what_they_name() {
        let spec = SongSpec::parse("key = \"F# minor\"\ntempo = 96").unwrap();
        assert_eq!(spec.key.to_text(), "F# minor");
        assert_eq!(spec.tempo, 96.0);
        assert_eq!(
            spec.groove,
            SongSpec::default().groove,
            "an unmentioned field keeps its default"
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let spec = SongSpec::parse(
            r#"
            # the key first
            key = "D minor"   # trailing comments too

            tempo = 100
            "#,
        )
        .unwrap();
        assert_eq!(spec.key.to_text(), "D minor");
        assert_eq!(spec.tempo, 100.0);
    }

    #[test]
    fn a_harmony_table_adds_to_the_header_progression() {
        // These used to overwrite one another, so a document with both lost one of them.
        let spec = SongSpec::parse(
            r#"
            chords = "@marusa"

            [harmony]
            bridge = "| ii7 | V7 | Imaj7 | Imaj7 |"
            "#,
        )
        .unwrap();
        assert_eq!(spec.charts.len(), 2);
        assert!(spec.charts.contains_key("main"));
        assert!(spec.charts.contains_key("bridge"));
    }

    #[test]
    fn two_parts_of_the_same_name_are_a_mistake() {
        let errors =
            SongSpec::parse("[[part]]\nname = \"lead\"\n[[part]]\nname = \"lead\"").unwrap_err();
        assert!(
            errors[0].message.contains("already a part"),
            "{:?}",
            errors[0]
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_is_reported_rather_than_substituted() {
        let errors =
            SongSpec::parse("form = [\"verse\"]\n[section.verse]\nchords = \"nope\"").unwrap_err();
        assert!(errors[0].message.contains("not a chart"), "{:?}", errors[0]);

        let errors =
            SongSpec::parse("form = [\"verse\"]\n[section.verse]\nparts = [\"nope\"]").unwrap_err();
        assert!(
            errors[0].message.contains("does not exist"),
            "{:?}",
            errors[0]
        );
    }

    #[test]
    fn a_meter_or_a_repeat_count_that_would_exhaust_memory_is_refused() {
        assert!(SongSpec::parse("meter = \"4000000/4\"").is_err());
        assert!(SongSpec::parse("meter = \"0/4\"").is_err());
        assert!(SongSpec::parse("chords = \"@axis x99999999\"").is_err());
        assert!(SongSpec::parse("chords = \"@axis x4\"").is_ok());
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_field() {
        let spec = SongSpec::parse("\u{feff}tempo = 96").unwrap();
        assert_eq!(spec.tempo, 96.0);
    }

    #[test]
    fn a_groove_name_is_matched_the_way_every_other_word_is() {
        assert!(SongSpec::parse("groove = \"Basic-Rock\"").is_ok());
        assert!(SongSpec::parse("groove = \"SHUFFLE\"").is_ok());
    }

    #[test]
    fn a_fractional_tempo_survives_the_round_trip() {
        let spec = SongSpec::parse("tempo = 128.5").unwrap();
        assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap().tempo, 128.5);
    }

    #[test]
    fn the_default_progression_is_the_composers_own_to_colour() {
        // A chart nobody asked for may be coloured by the mood; a quoted one may not.
        assert_eq!(
            SongSpec::default().charts["main"].origin,
            ChartOrigin::Generated
        );
        assert_eq!(
            SongSpec::parse("chords = \"@marusa\"").unwrap().charts["main"].origin,
            ChartOrigin::Given
        );

        // And it stays the composer's own across a round trip, which is why an invented chart is
        // left out of the document rather than written into it: a written one would come back
        // marked as quoted and never be coloured again.
        let written = SongSpec::default().to_toml();
        assert!(!written.contains("[harmony]"), "{written}");
        assert_eq!(
            SongSpec::parse(&written).unwrap().charts["main"].origin,
            ChartOrigin::Generated
        );
    }

    #[test]
    fn the_whole_default_specification_round_trips_unchanged() {
        let original = SongSpec::default();
        assert_eq!(SongSpec::parse(&original.to_toml()).unwrap(), original);
    }

    #[test]
    fn an_override_replaces_a_field_the_document_already_set() {
        // TOML refuses a key written twice, so this cannot be an extra line appended to the
        // document the way it was when the format was line-oriented.
        let spec = SongSpec::parse_with_overrides(
            "tempo = 100\n\n[section.verse]\nbars = 16\n",
            &[
                ("tempo".to_string(), "140".to_string()),
                ("seed".to_string(), "7".to_string()),
                ("key".to_string(), "D minor".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(spec.tempo, 140.0, "the override wins");
        assert_eq!(spec.seed, 7);
        assert_eq!(
            spec.key.to_text(),
            "D minor",
            "a value is read as TOML where TOML can read it, and as a string where it cannot"
        );
        assert_eq!(spec.sections["verse"].bars, 16, "the rest is untouched");
    }

    #[test]
    fn an_override_of_a_field_that_does_not_exist_is_refused() {
        let errors =
            SongSpec::parse_with_overrides("", &[("tempoo".to_string(), "120".to_string())])
                .unwrap_err();
        assert!(errors[0].message.contains("tempoo"), "{:?}", errors[0]);
    }

    #[test]
    fn a_default_roster_is_mixed_rather_than_stacked() {
        // Six parts at unity sum past full scale; the defaults have to leave headroom.
        let spec = SongSpec::default();
        assert!(spec.parts.iter().all(|part| part.gain_db < 0.0));
        let lead = spec.parts.iter().find(|p| p.role == Role::Melody).unwrap();
        let hat = spec.parts.iter().find(|p| p.role == Role::Hat).unwrap();
        assert!(
            hat.gain_db < lead.gain_db,
            "the hat should sit under the tune"
        );
    }

    #[test]
    fn a_sharp_inside_a_value_is_not_a_comment() {
        // `#` is both an accidental and a comment marker. The line-oriented format had to tell
        // them apart by hand; a quoted string does it by construction.
        let spec = SongSpec::parse("key = \"F# minor\"  # a comment").unwrap();
        assert_eq!(spec.key.to_text(), "F# minor");
        let chart = SongSpec::parse("chords = \"| #iv | V |\"  # a comment").unwrap();
        assert_eq!(chart.charts["main"].bar_count(), 2);
    }

    #[test]
    fn a_quotation_is_written_back_as_the_quotation() {
        // Spelling its bars out would be longer to read and would lose the *mode* the
        // progression was written in — which is what lets 丸サ進行 be asked for in a minor key
        // and still come out 丸サ進行 rather than four degrees read against an aeolian scale.
        let spec = SongSpec::parse("chords = \"@marusa\"").unwrap();
        let written = spec.to_toml();
        assert!(written.contains("main = \"@marusa\""), "{written}");
        assert_eq!(SongSpec::parse(&written).unwrap(), spec);

        // A repeat count comes with it, or the chart would come back half as long.
        let doubled = SongSpec::parse("chords = \"@axis x2\"").unwrap();
        assert_eq!(doubled.charts["main"].bar_count(), 8);
        assert_eq!(
            SongSpec::parse(&doubled.to_toml()).unwrap().charts["main"].bar_count(),
            8
        );
    }

    #[test]
    fn a_progression_can_be_named_or_written_out() {
        let named = SongSpec::parse("chords = \"@marusa\"").unwrap();
        assert_eq!(named.charts["main"].bar_count(), 4);

        let written = SongSpec::parse("chords = \"| i | bVI | bIII | bVII |\"").unwrap();
        assert_eq!(written.charts["main"].bar_count(), 4);

        let table = SongSpec::parse(
            r#"
            [harmony]
            main   = "@royal-road"
            bridge = "| ii7 | V7 | Imaj7 | Imaj7 |"
            "#,
        )
        .unwrap();
        assert_eq!(table.charts.len(), 2);
        assert_eq!(table.charts["bridge"].bar_count(), 4);
    }

    #[test]
    fn sections_and_form_describe_the_shape() {
        let spec = SongSpec::parse(
            r#"
            form = ["intro", "verse", "chorus"]

            [section.intro]
            bars      = 4
            intensity = 0.2

            [section.chorus]
            bars = 16
            "#,
        )
        .unwrap();
        assert_eq!(spec.form, ["intro", "verse", "chorus"]);
        assert_eq!(spec.sections["intro"].bars, 4);
        assert_eq!(spec.sections["chorus"].bars, 16);
        assert_eq!(
            spec.sections["verse"].bars, 8,
            "a section named in the form but not described still exists"
        );
        assert_eq!(spec.total_bars(), 4 + 8 + 16);
    }

    #[test]
    fn a_form_can_be_a_list_or_the_words_on_one_line() {
        let list = SongSpec::parse("form = [\"intro\", \"verse\", \"chorus\"]").unwrap();
        let words = SongSpec::parse("form = \"intro verse chorus\"").unwrap();
        assert_eq!(list.form, words.form);
        assert_eq!(list.form, ["intro", "verse", "chorus"]);

        // The same on a section's roster, which arrives from a command line the same way.
        let spec =
            SongSpec::parse("form = [\"intro\"]\n[section.intro]\nparts = \"bass kick\"").unwrap();
        assert_eq!(spec.sections["intro"].parts, ["bass", "kick"]);
    }

    #[test]
    fn a_section_intensity_follows_its_name() {
        let spec = SongSpec::parse("form = [\"intro\", \"chorus\", \"outro\"]").unwrap();
        assert!(spec.sections["intro"].intensity < spec.sections["chorus"].intensity);
        assert!(spec.sections["outro"].intensity < spec.sections["chorus"].intensity);
    }

    #[test]
    fn declaring_any_part_replaces_the_default_roster() {
        let spec = SongSpec::parse(
            r#"
            [[part]]
            name   = "bass"
            octave = 1

            [[part]]
            name = "kick"
            "#,
        )
        .unwrap();
        assert_eq!(
            spec.parts.len(),
            2,
            "the defaults are replaced, not added to"
        );
        assert_eq!(spec.parts[0].name, "bass");
        assert_eq!(spec.parts[0].role, Role::Bass, "the name implies the role");
        assert_eq!(spec.parts[0].octave, 1);
        assert_eq!(spec.parts[1].role, Role::Kick);
    }

    #[test]
    fn the_order_parts_are_written_in_is_the_order_they_keep() {
        // An array rather than a table for exactly this: a map would sort `bass` above `lead`
        // and quietly rearrange the mixer.
        let spec = SongSpec::parse(
            "[[part]]\nname = \"lead\"\n[[part]]\nname = \"bass\"\n[[part]]\nname = \"kick\"",
        )
        .unwrap();
        let names: Vec<&str> = spec.parts.iter().map(|part| part.name.as_str()).collect();
        assert_eq!(names, ["lead", "bass", "kick"]);

        let reparsed = SongSpec::parse(&spec.to_toml()).unwrap();
        let names: Vec<&str> = reparsed
            .parts
            .iter()
            .map(|part| part.name.as_str())
            .collect();
        assert_eq!(names, ["lead", "bass", "kick"], "and keeps it in writing");
    }

    #[test]
    fn a_part_takes_its_instrument_and_octave_from_its_role() {
        let spec = SongSpec::parse("[[part]]\nname = \"anything\"\nrole = \"bass\"").unwrap();
        assert_eq!(spec.parts[0].instrument, "auris.synth.fm2");
        assert_eq!(spec.parts[0].octave, 2);
        assert_eq!(spec.parts[0].range(), (28, 52));

        // Until it is told otherwise — and whichever order the two are written in, which the
        // line-oriented format could not promise.
        let pinned = SongSpec::parse(
            r#"
            [[part]]
            name       = "anything"
            octave     = 3
            instrument = "auris.synth.chiptune"
            role       = "bass"
            "#,
        )
        .unwrap();
        assert_eq!(pinned.parts[0].instrument, "auris.synth.chiptune");
        assert_eq!(pinned.parts[0].octave, 3);
        assert_eq!(pinned.parts[0].range(), (40, 64), "the range moves with it");
    }

    #[test]
    fn a_rhythm_can_be_written_by_hand() {
        let spec = SongSpec::parse(
            "[[part]]\nname = \"kick\"\nrhythm = \"x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~\"",
        )
        .unwrap();
        let rhythm = spec.parts[0].rhythm.as_ref().unwrap();
        assert_eq!(rhythm.onsets(), vec![0, 4, 8, 12]);
    }

    #[test]
    fn a_mood_word_lands_on_numbers() {
        let dark = SongSpec::parse("mood = \"dark\"").unwrap();
        let bright = SongSpec::parse("mood = \"bright\"").unwrap();
        assert!(dark.mood.brightness < bright.mood.brightness);
        assert!(dark.mood.energy < bright.mood.energy);

        // And an axis can be nudged on its own. The word is the base and the dial is the trim
        // whichever order they are written in: a table has no line order to lean on.
        let nudged = SongSpec::parse("energy = 0.9\nmood = \"dark\"").unwrap();
        assert_eq!(nudged.mood.brightness, dark.mood.brightness);
        assert_eq!(nudged.mood.energy, 0.9);
    }

    #[test]
    fn a_field_the_format_does_not_understand_is_an_error_with_its_line() {
        // Silently ignoring it would mean the piece quietly ignores an instruction. This one is
        // syntax, so `toml` catches it and says where.
        let errors = SongSpec::parse("title = \"x\"\ntempoo = 120").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(2));
        assert!(errors[0].message.contains("tempoo"), "{:?}", errors[0]);
        assert!(
            errors[0].to_string().starts_with("line 2: "),
            "{}",
            errors[0]
        );

        let errors = SongSpec::parse("this is not a field").unwrap_err();
        assert_eq!(errors[0].line, Some(1), "{:?}", errors[0]);
    }

    #[test]
    fn every_complaint_about_meaning_is_reported_at_once() {
        // A complaint that only meaning can catch has no line — it is found after the document
        // has stopped being text — so each names what it is about instead.
        let errors = SongSpec::parse(
            r#"
            key    = "H minor"
            groove = "nonsense"
            energy = 2.0
            "#,
        )
        .unwrap_err();
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().all(|error| error.line.is_none()));
        assert!(errors.iter().any(|e| e.message.contains("not a key")));
        assert!(errors.iter().any(|e| e.message.contains("not a groove")));
        assert!(errors.iter().any(|e| e.message.contains("energy runs")));
    }

    #[test]
    fn a_complaint_says_which_part_or_section_it_is_about() {
        let errors = SongSpec::parse(
            r#"
            [section.chorus]
            bars = 0

            [[part]]
            name = "lead"
            pan  = 4.0
            "#,
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("section `chorus`")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.message.contains("part `lead`")),
            "{errors:?}"
        );
    }

    #[test]
    fn numbers_are_checked_against_the_range_they_mean() {
        assert!(SongSpec::parse("tempo = 5").is_err());
        assert!(SongSpec::parse("tempo = 900").is_err());
        assert!(SongSpec::parse("energy = 2.0").is_err());
        assert!(SongSpec::parse("energy = -1.0").is_err());
        assert!(SongSpec::parse("swing = 200").is_err());
        assert!(SongSpec::parse("meter = \"4/5\"").is_err());
        assert!(SongSpec::parse("meter = \"nonsense\"").is_err());
        assert!(SongSpec::parse("tempo = 128").is_ok());
        assert!(SongSpec::parse("meter = \"3/4\"").is_ok());
        assert!(SongSpec::parse("meter = \"6/8\"").is_ok());
    }

    #[test]
    fn an_error_names_what_would_have_worked() {
        let errors = SongSpec::parse("mood = \"sideways\"").unwrap_err();
        assert!(errors[0].message.contains("bright"), "{:?}", errors[0]);
        let errors = SongSpec::parse("groove = \"sideways\"").unwrap_err();
        assert!(errors[0].message.contains("basic-rock"), "{:?}", errors[0]);
        let errors = SongSpec::parse("[[part]]\nname = \"x\"\nrole = \"sideways\"").unwrap_err();
        assert!(errors[0].message.contains("melody"), "{:?}", errors[0]);
    }

    #[test]
    fn a_spec_round_trips_through_its_document() {
        let original = SongSpec::parse(
            r#"
            title  = "Test Piece"
            key    = "Bb minor"
            tempo  = 132
            meter  = "3/4"
            seed   = 99
            mood   = "dreamy"
            groove = "shuffle"
            form   = ["intro", "verse", "verse", "outro"]
            chords = "@royal-road"

            [section.verse]
            bars      = 16
            intensity = 0.7
            transpose = 2

            [[part]]
            name    = "lead"
            role    = "melody"
            octave  = 6
            density = 0.4
            gain    = -3.0

            [[part]]
            name = "bass"
            role = "bass"
            "#,
        )
        .unwrap();

        assert_eq!(
            SongSpec::parse(&original.to_toml()).unwrap(),
            original,
            "written out and read back is the same song, field for field"
        );
    }

    #[test]
    fn a_subdivision_and_a_gate_survive_the_round_trip() {
        // Both are written out only when they differ from what the role implies, so that the
        // ordinary document stays short — which means the round trip has to work from both sides
        // of that condition rather than only from the side that writes a line.
        let original = SongSpec::parse(
            r#"
            form = ["verse"]

            [[part]]
            name = "lead"

            [[part]]
            name        = "chords"
            subdivision = "16t"
            gate        = 0.25

            [[part]]
            name = "stab"
            "#,
        )
        .unwrap();
        let reparsed = SongSpec::parse(&original.to_toml()).unwrap();
        assert_eq!(reparsed.parts, original.parts);

        let named = |spec: &SongSpec, name: &str| {
            spec.parts
                .iter()
                .find(|part| part.name == name)
                .expect("the part is in the roster")
                .clone()
        };
        let chords = named(&reparsed, "chords");
        assert_eq!(chords.subdivision, Subdivision::SixteenthTriplet);
        assert!((chords.gate - 0.25).abs() < 1e-6);

        // The stab wrote neither line, because a stab's gate is already what a stab's gate is —
        // and it comes back regardless, because the role is what puts it there.
        let stab = named(&reparsed, "stab");
        assert_eq!(stab.gate, Role::Stab.default_gate());
        assert!(stab.gate < 1.0, "a stab that is not short is a chord part");
        assert_eq!(
            named(&reparsed, "lead").gate,
            1.0,
            "everything else is legato"
        );
    }

    #[test]
    fn a_subdivision_is_named_the_way_a_musician_would_say_it() {
        for (text, expected) in [
            ("16", Subdivision::Sixteenth),
            ("1/8", Subdivision::Eighth),
            ("8t", Subdivision::EighthTriplet),
            ("triplet", Subdivision::EighthTriplet),
            ("sixteenth-triplet", Subdivision::SixteenthTriplet),
        ] {
            assert_eq!(Subdivision::parse(text), Some(expected), "`{text}`");
        }
        assert!(Subdivision::parse("fifth").is_none());
        // And every name it writes is one it reads back.
        for subdivision in Subdivision::ALL {
            assert_eq!(Subdivision::parse(subdivision.name()), Some(subdivision));
        }
    }

    #[test]
    fn a_gate_of_nothing_is_refused_rather_than_written() {
        // Zero would write a note of no length at every onset: a part that is silent but still
        // costs a voice, which reads as a bug wherever it is met.
        let errors = SongSpec::parse("[[part]]\nname = \"chords\"\ngate = 0.0").unwrap_err();
        assert!(errors.iter().any(|error| error.message.contains("gate")));
    }

    #[test]
    fn a_section_can_name_which_parts_play() {
        let spec = SongSpec::parse(
            r#"
            form = ["intro", "chorus"]

            [section.intro]
            parts = ["bass", "kick"]
            "#,
        )
        .unwrap();
        let intro = &spec.sections["intro"];
        let playing: Vec<&str> = spec
            .parts_in(intro)
            .iter()
            .map(|part| part.name.as_str())
            .collect();
        assert_eq!(playing, ["bass", "kick"]);

        let chorus = &spec.sections["chorus"];
        assert_eq!(
            spec.parts_in(chorus).len(),
            spec.parts.len(),
            "a section that does not say plays everything"
        );
    }
}
