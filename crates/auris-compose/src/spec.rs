//! The text a piece is asked for in.
//!
//! One document describes a whole song: its key, its tempo, its progression, its form and its
//! parts. It is deliberately a *specification* rather than a notation — it says what to write,
//! not what was written — because that is the layer where an instruction like "make the chorus
//! busier" is one word rather than four hundred edited notes.
//!
//! The format is line-oriented `field: value`, with `[block name]` headers. That shape is chosen
//! for the two callers it has: an agent writing it through MCP, which needs a grammar it cannot
//! get subtly wrong, and a person typing it into a terminal, who needs to be able to change one
//! line without understanding the rest.
//!
//! ```
//! # use auris_compose::spec::SongSpec;
//! let spec = SongSpec::parse("
//!     key:   C minor
//!     tempo: 128
//!     chords: @marusa
//! ").unwrap();
//! assert_eq!(spec.tempo, 128.0);
//! ```

use std::collections::BTreeMap;
use std::fmt;

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
    /// Which octave it sits in.
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

/// Something wrong with a document, with the line it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecError {
    /// One-based line number.
    pub line: usize,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
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
    /// field left out is answered by the defaults. What *is* rejected is a line that says
    /// something the format does not understand, because silently ignoring it would mean the
    /// piece quietly ignores an instruction.
    pub fn parse(text: &str) -> Result<Self, Vec<SpecError>> {
        let mut spec = SongSpec::default();
        let mut errors = Vec::new();

        // A document that declares any part, section or chart replaces the defaults entirely,
        // rather than adding to them: a roster half inherited from a default is impossible to
        // reason about.
        let mut declared_parts: Vec<PartSpec> = Vec::new();
        let mut declared_sections: BTreeMap<String, SectionSpec> = BTreeMap::new();
        let mut declared_charts: BTreeMap<String, Chart> = BTreeMap::new();
        let mut declared_form: Option<Vec<String>> = None;

        let mut block = Block::Song;
        // A byte-order mark is invisible in an editor and would make the first field unparseable.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }

            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                match Block::parse(header) {
                    Some(Block::Part(name)) => {
                        if declared_parts.iter().any(|part| part.name == name) {
                            errors.push(SpecError {
                                line: line_number,
                                message: format!(
                                    "`{name}` is already a part; two blocks of the same name \
                                     would silently merge"
                                ),
                            });
                        }
                        declared_parts.push(PartSpec::of_role(&name, infer_role(&name)));
                        block = Block::Part(name);
                    }
                    Some(Block::Section(name)) => {
                        declared_sections
                            .entry(name.clone())
                            .or_insert_with(|| SectionSpec::named(&name));
                        block = Block::Section(name);
                    }
                    Some(other) => block = other,
                    None => errors.push(SpecError {
                        line: line_number,
                        message: format!(
                            "`{header}` is not a block; expected part, section or harmony"
                        ),
                    }),
                }
                continue;
            }

            let Some((field, value)) = line.split_once(':') else {
                errors.push(SpecError {
                    line: line_number,
                    message: format!("`{line}` is not `field: value`"),
                });
                continue;
            };
            let field = field.trim();
            let value = value.trim();

            let outcome = match &block {
                Block::Song => apply_song_field(&mut spec, &mut declared_form, field, value),
                Block::Harmony => match Chart::parse(value) {
                    Some(chart) => {
                        declared_charts.insert(field.to_string(), chart);
                        Ok(())
                    }
                    None => Err(format!("`{value}` is not a chord chart")),
                },
                Block::Section(name) => {
                    let section = declared_sections
                        .entry(name.clone())
                        .or_insert_with(|| SectionSpec::named(name));
                    apply_section_field(section, field, value)
                }
                Block::Part(name) => {
                    // The *last* block of that name, so the duplicate reported above still gets
                    // its own fields rather than writing into its namesake.
                    match declared_parts
                        .iter_mut()
                        .rev()
                        .find(|part| &part.name == name)
                    {
                        Some(part) => apply_part_field(part, field, value),
                        None => Err(format!("no part named `{name}`")),
                    }
                }
            };
            if let Err(message) = outcome {
                errors.push(SpecError {
                    line: line_number,
                    message,
                });
            }
        }

        if !declared_parts.is_empty() {
            spec.parts = declared_parts;
        }
        if !declared_sections.is_empty() {
            spec.sections = declared_sections;
        }
        // Merged rather than replaced: a document with both a header `chords:` and a `[harmony]`
        // block used to lose whichever came first.
        for (name, chart) in declared_charts {
            spec.charts.insert(name, chart);
        }
        if let Some(form) = declared_form {
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
            errors.push(SpecError {
                line: 0,
                message: "the form is empty, so there is nothing to write".to_string(),
            });
        }

        // A name that does not resolve would otherwise be answered by a silent substitution: a
        // section would play a progression nobody asked for, or fall silent for want of a part.
        let part_names: Vec<&str> = spec.parts.iter().map(|part| part.name.as_str()).collect();
        for (name, section) in &spec.sections {
            if !spec.charts.contains_key(&section.chords) {
                errors.push(SpecError {
                    line: 0,
                    message: format!(
                        "section `{name}` plays `{}`, which is not a chart; there is {}",
                        section.chords,
                        spec.charts
                            .keys()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            for part in &section.parts {
                if !part_names.contains(&part.as_str()) {
                    errors.push(SpecError {
                        line: 0,
                        message: format!(
                            "section `{name}` names the part `{part}`, which does not exist; \
                             there is {}",
                            part_names.join(", ")
                        ),
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }

    /// The document this spec would be written as.
    ///
    /// Round-tripping matters for the agent case: a tool can read a spec back, change one line
    /// and send it again without having to hold the whole document in its head.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("title:    {}\n", self.title));
        out.push_str(&format!("key:      {}\n", self.key.to_text()));
        out.push_str(&format!("tempo:    {}\n", self.tempo));
        out.push_str(&format!(
            "meter:    {}/{}\n",
            self.meter.numerator, self.meter.denominator
        ));
        out.push_str(&format!("seed:     {}\n", self.seed));
        out.push_str(&format!("groove:   {}\n", self.groove));
        out.push_str(&format!("swing:    {}\n", self.swing));
        out.push_str(&format!("humanize: {:.2}\n", self.humanize));
        out.push_str(&format!("dynamics: {:.2}\n", self.dynamics));
        out.push_str(&format!("fill:     {:.2}\n", self.fill));
        out.push_str(&format!("variation: {:.2}\n", self.variation));
        out.push_str(&format!("brightness: {:.2}\n", self.mood.brightness));
        out.push_str(&format!("energy:     {:.2}\n", self.mood.energy));
        out.push_str(&format!("tension:    {:.2}\n", self.mood.tension));
        out.push_str(&format!("syncopation: {:.2}\n", self.mood.syncopation));
        out.push_str(&format!("form:     {}\n", self.form.join(" ")));

        out.push_str("\n[harmony]\n");
        for (name, chart) in &self.charts {
            out.push_str(&format!("{name}: {chart}\n"));
        }

        for (name, section) in &self.sections {
            out.push_str(&format!("\n[section {name}]\n"));
            out.push_str(&format!("bars:      {}\n", section.bars));
            out.push_str(&format!("chords:    {}\n", section.chords));
            out.push_str(&format!("intensity: {:.2}\n", section.intensity));
            if section.transpose != 0 {
                out.push_str(&format!("transpose: {}\n", section.transpose));
            }
            if !section.parts.is_empty() {
                out.push_str(&format!("parts:     {}\n", section.parts.join(" ")));
            }
        }

        for part in &self.parts {
            out.push_str(&format!("\n[part {}]\n", part.name));
            out.push_str(&format!("role:       {}\n", part.role.name()));
            out.push_str(&format!("instrument: {}\n", part.instrument));
            out.push_str(&format!("octave:     {}\n", part.octave));
            if let Some(density) = part.density {
                out.push_str(&format!("density:    {density:.2}\n"));
            }
            if part.subdivision != Subdivision::default() {
                out.push_str(&format!("subdivision: {}\n", part.subdivision.name()));
            }
            if part.gate != part.role.default_gate() {
                out.push_str(&format!("gate:       {:.2}\n", part.gate));
            }
            if let Some(rhythm) = &part.rhythm {
                out.push_str(&format!("rhythm:     {}\n", rhythm.to_text()));
            }
            out.push_str(&format!("gain:       {:.1}\n", part.gain_db));
            if part.pan != 0.0 {
                out.push_str(&format!("pan:        {:.2}\n", part.pan));
            }
        }
        out
    }
}

/// Everything before a comment.
///
/// `#` is also a sharp sign, so it only starts a comment where it cannot be one: at the very
/// start of a line, or surrounded by whitespace. Cutting at the first `#` regardless would turn
/// `key: F# minor` into `key: F`, and requiring only a leading space would eat the `#iv` in a
/// chord chart.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        if index == 0 {
            return "";
        }
        let after_space = matches!(bytes[index - 1], b' ' | b'\t');
        let before_space = bytes
            .get(index + 1)
            .is_none_or(|next| matches!(next, b' ' | b'\t'));
        if after_space && before_space {
            return &line[..index];
        }
    }
    line
}

/// Which block the parser is inside.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Block {
    /// The header, before any block. `[song]` returns to it.
    Song,
    /// `[harmony]`, whose fields are named charts.
    Harmony,
    /// `[section name]`.
    Section(String),
    /// `[part name]`.
    Part(String),
}

impl Block {
    fn parse(header: &str) -> Option<Self> {
        let header = header.trim();
        if header.eq_ignore_ascii_case("harmony") {
            return Some(Block::Harmony);
        }
        // `[song]` goes back to the header fields, which is what lets an override be appended
        // to a document that ends inside a block.
        if header.eq_ignore_ascii_case("song") {
            return Some(Block::Song);
        }
        let (kind, name) = header.split_once(char::is_whitespace)?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return None;
        }
        match kind.to_ascii_lowercase().as_str() {
            "section" => Some(Block::Section(name)),
            "part" => Some(Block::Part(name)),
            _ => None,
        }
    }
}

/// The role a part's name implies, so `[part bass]` needs no `role:` line.
fn infer_role(name: &str) -> Role {
    Role::parse(name).unwrap_or(Role::Melody)
}

fn number(value: &str, what: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|_| format!("`{value}` is not a number for {what}"))
}

fn fraction(value: &str, what: &str) -> Result<f32, String> {
    let parsed = number(value, what)? as f32;
    if !(0.0..=1.0).contains(&parsed) {
        return Err(format!("{what} runs from 0 to 1, not {parsed}"));
    }
    Ok(parsed)
}

fn apply_song_field(
    spec: &mut SongSpec,
    form: &mut Option<Vec<String>>,
    field: &str,
    value: &str,
) -> Result<(), String> {
    match field {
        "title" => spec.title = value.to_string(),
        "tempo" | "bpm" => {
            let tempo = number(value, "tempo")?;
            if !(20.0..=400.0).contains(&tempo) {
                return Err(format!("a tempo of {tempo} is outside 20..400"));
            }
            spec.tempo = tempo;
        }
        "meter" | "time" => {
            let (top, bottom) = value
                .split_once('/')
                .ok_or_else(|| format!("`{value}` is not a meter like 4/4"))?;
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
                    "`{value}` is not a meter this can count; the beat count runs from 1 to 32"
                ));
            }
            spec.meter = TimeSignature::new(numerator, denominator);
        }
        "key" => spec.key = Key::parse(value).ok_or_else(|| format!("`{value}` is not a key"))?,
        "scale" => {
            let scale = ScaleId::parse(value).ok_or_else(|| format!("`{value}` is not a scale"))?;
            spec.key = Key::new(spec.key.tonic, scale);
        }
        "seed" => {
            spec.seed = value
                .parse()
                .map_err(|_| format!("`{value}` is not a seed"))?
        }
        "swing" => {
            let swing = number(value, "swing")?;
            if !(20.0..=90.0).contains(&swing) {
                return Err(format!("swing runs from 20 to 90, not {swing}"));
            }
            spec.swing = swing as u8;
        }
        "humanize" => spec.humanize = fraction(value, "humanize")?,
        "dynamics" => spec.dynamics = fraction(value, "dynamics")?,
        "fill" => spec.fill = fraction(value, "fill")?,
        "variation" => spec.variation = fraction(value, "variation")?,
        "groove" => {
            if crate::rhythm::groove(value).is_none() {
                let names: Vec<&str> = crate::rhythm::GROOVES.iter().map(|g| g.name).collect();
                return Err(format!(
                    "`{value}` is not a groove; try one of {}",
                    names.join(", ")
                ));
            }
            spec.groove = value.to_string();
        }
        "mood" => {
            spec.mood = Mood::named(value).ok_or_else(|| {
                format!(
                    "`{value}` is not a mood; try one of {}",
                    Mood::NAMES.join(", ")
                )
            })?
        }
        "brightness" => spec.mood.brightness = fraction(value, "brightness")?,
        "energy" => spec.mood.energy = fraction(value, "energy")?,
        "tension" => spec.mood.tension = fraction(value, "tension")?,
        "syncopation" => spec.mood.syncopation = fraction(value, "syncopation")?,
        "form" => {
            *form = Some(
                value
                    .split_whitespace()
                    .map(|name| name.to_string())
                    .collect(),
            )
        }
        // A bare `chords:` in the header is the shortest possible way to name a progression.
        "chords" | "progression" => {
            let chart =
                Chart::parse(value).ok_or_else(|| format!("`{value}` is not a chord chart"))?;
            spec.charts.insert("main".to_string(), chart);
        }
        other => {
            return Err(format!(
                "`{other}` is not a song field; expected one of title, key, scale, tempo, \
                 meter, seed, swing, humanize, dynamics, fill, variation, groove, mood, \
                 brightness, energy, tension, syncopation, form, chords"
            ));
        }
    }
    Ok(())
}

fn apply_section_field(section: &mut SectionSpec, field: &str, value: &str) -> Result<(), String> {
    match field {
        "bars" => {
            let bars: usize = value
                .parse()
                .map_err(|_| format!("`{value}` is not a bar count"))?;
            if bars == 0 || bars > 512 {
                return Err(format!("a section of {bars} bars is not playable"));
            }
            section.bars = bars;
        }
        "chords" => section.chords = value.to_string(),
        "intensity" => section.intensity = fraction(value, "intensity")?,
        "transpose" => {
            let semitones: i32 = value
                .parse()
                .map_err(|_| format!("`{value}` is not a number of semitones"))?;
            if !(-24..=24).contains(&semitones) {
                return Err(format!(
                    "a transposition of {semitones} semitones is extreme"
                ));
            }
            section.transpose = semitones;
        }
        "parts" => {
            section.parts = value
                .split_whitespace()
                .filter(|name| *name != "*")
                .map(|name| name.to_string())
                .collect()
        }
        other => {
            return Err(format!(
                "`{other}` is not a section field; expected bars, chords, intensity, \
                 transpose or parts"
            ));
        }
    }
    Ok(())
}

fn apply_part_field(part: &mut PartSpec, field: &str, value: &str) -> Result<(), String> {
    match field {
        "role" => {
            let role = Role::parse(value).ok_or_else(|| {
                let names: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
                format!("`{value}` is not a role; try one of {}", names.join(", "))
            })?;
            // The instrument and octave follow the role unless they were set on purpose, which
            // is what makes `role: bass` alone enough to describe a bass part.
            if part.instrument == part.role.default_instrument() {
                part.instrument = role.default_instrument().to_string();
            }
            if part.octave == part.role.default_octave() {
                part.octave = role.default_octave();
            }
            if part.gain_db == part.role.default_gain_db() {
                part.gain_db = role.default_gain_db();
            }
            part.role = role;
        }
        "instrument" => part.instrument = value.to_string(),
        "octave" => {
            let octave: i32 = value
                .parse()
                .map_err(|_| format!("`{value}` is not an octave"))?;
            if !(-1..=9).contains(&octave) {
                return Err(format!("octave {octave} is outside the MIDI range"));
            }
            part.octave = octave;
        }
        "density" => part.density = Some(fraction(value, "density")?),
        "subdivision" => {
            part.subdivision = Subdivision::parse(value)
                .ok_or_else(|| format!("`{value}` is not a subdivision; use 8, 16, 8t or 16t"))?
        }
        "gate" => {
            let gate = fraction(value, "gate")?;
            if gate <= 0.0 {
                return Err("a gate of 0 would write notes nobody can hear".to_string());
            }
            part.gate = gate;
        }
        "rhythm" => {
            part.rhythm = Some(
                Pattern::parse(value)
                    .ok_or_else(|| format!("`{value}` is not a rhythm; use x, X, o and ~"))?,
            )
        }
        "gain" => {
            let gain = number(value, "gain")?;
            if !(-60.0..=12.0).contains(&gain) {
                return Err(format!("a gain of {gain} dB is outside -60..12"));
            }
            part.gain_db = gain as f32;
        }
        "pan" => {
            let pan = number(value, "pan")? as f32;
            if !(-1.0..=1.0).contains(&pan) {
                return Err(format!("pan runs from -1 to 1, not {pan}"));
            }
            part.pan = pan;
        }
        other => {
            return Err(format!(
                "`{other}` is not a part field; expected role, instrument, octave, density, \
                 subdivision, gate, rhythm, gain or pan"
            ));
        }
    }
    Ok(())
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
    fn two_lines_change_only_what_they_name() {
        let spec = SongSpec::parse("key: F# minor\ntempo: 96").unwrap();
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
            "
            # the key first
            key: D minor   # trailing comments too

            tempo: 100
            ",
        )
        .unwrap();
        assert_eq!(spec.key.to_text(), "D minor");
        assert_eq!(spec.tempo, 100.0);
    }

    #[test]
    fn a_harmony_block_adds_to_the_header_progression() {
        // These used to overwrite one another, so a document with both lost one of them.
        let spec = SongSpec::parse(
            "
            chords: @marusa

            [harmony]
            bridge: | ii7 | V7 | Imaj7 | Imaj7 |
            ",
        )
        .unwrap();
        assert_eq!(spec.charts.len(), 2);
        assert!(spec.charts.contains_key("main"));
        assert!(spec.charts.contains_key("bridge"));
    }

    #[test]
    fn two_blocks_of_the_same_name_are_a_mistake() {
        let errors = SongSpec::parse("[part lead]\n[part lead]").unwrap_err();
        assert!(
            errors[0].message.contains("already a part"),
            "{:?}",
            errors[0]
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_is_reported_rather_than_substituted() {
        let errors = SongSpec::parse("form: verse\n[section verse]\nchords: nope").unwrap_err();
        assert!(errors[0].message.contains("not a chart"), "{:?}", errors[0]);

        let errors = SongSpec::parse("form: verse\n[section verse]\nparts: nope").unwrap_err();
        assert!(
            errors[0].message.contains("does not exist"),
            "{:?}",
            errors[0]
        );
    }

    #[test]
    fn a_meter_or_a_repeat_count_that_would_exhaust_memory_is_refused() {
        assert!(SongSpec::parse("meter: 4000000/4").is_err());
        assert!(SongSpec::parse("meter: 0/4").is_err());
        assert!(SongSpec::parse("chords: @axis x99999999").is_err());
        assert!(SongSpec::parse("chords: @axis x4").is_ok());
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_field() {
        let spec = SongSpec::parse("\u{feff}tempo: 96").unwrap();
        assert_eq!(spec.tempo, 96.0);
    }

    #[test]
    fn a_groove_name_is_matched_the_way_every_other_word_is() {
        assert!(SongSpec::parse("groove: Basic-Rock").is_ok());
        assert!(SongSpec::parse("groove: SHUFFLE").is_ok());
    }

    #[test]
    fn a_fractional_tempo_survives_the_round_trip() {
        let spec = SongSpec::parse("tempo: 128.5").unwrap();
        assert_eq!(SongSpec::parse(&spec.to_text()).unwrap().tempo, 128.5);
    }

    #[test]
    fn the_default_progression_is_the_composers_own_to_colour() {
        // A chart nobody asked for may be coloured by the mood; a quoted one may not.
        use crate::theory::chart::ChartOrigin;
        assert_eq!(
            SongSpec::default().charts["main"].origin,
            ChartOrigin::Generated
        );
        assert_eq!(
            SongSpec::parse("chords: @marusa").unwrap().charts["main"].origin,
            ChartOrigin::Given
        );
    }

    #[test]
    fn a_song_block_returns_to_the_header() {
        // Without this an override appended to a document would land in whichever block the
        // document happened to end in.
        let spec = SongSpec::parse(
            "
            tempo: 100

            [section verse]
            bars: 16

            [song]
            tempo: 140
            seed: 7
            ",
        )
        .unwrap();
        assert_eq!(spec.tempo, 140.0, "the later value wins");
        assert_eq!(spec.seed, 7);
        assert_eq!(spec.sections["verse"].bars, 16);
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
    fn a_sharp_is_not_a_comment() {
        // `#` marks a comment only after whitespace, because it is also an accidental.
        let spec = SongSpec::parse("key: F# minor  # a comment").unwrap();
        assert_eq!(spec.key.to_text(), "F# minor");
        assert_eq!(strip_comment("key: F# minor"), "key: F# minor");
        assert_eq!(strip_comment("key: F# minor # why").trim(), "key: F# minor");
        assert_eq!(strip_comment("# all comment"), "");
        assert_eq!(
            strip_comment("chords: | #iv | V |"),
            "chords: | #iv | V |",
            "a chromatic numeral is not a comment"
        );
        let chart = SongSpec::parse("chords: | #iv | V |  # a comment").unwrap();
        assert_eq!(chart.charts["main"].bar_count(), 2);
    }

    #[test]
    fn a_progression_can_be_named_or_written_out() {
        let named = SongSpec::parse("chords: @marusa").unwrap();
        assert_eq!(named.charts["main"].bar_count(), 4);

        let written = SongSpec::parse("chords: | i | bVI | bIII | bVII |").unwrap();
        assert_eq!(written.charts["main"].bar_count(), 4);

        let block = SongSpec::parse(
            "
            [harmony]
            main:   @royal-road
            bridge: | ii7 | V7 | Imaj7 | Imaj7 |
            ",
        )
        .unwrap();
        assert_eq!(block.charts.len(), 2);
        assert_eq!(block.charts["bridge"].bar_count(), 4);
    }

    #[test]
    fn sections_and_form_describe_the_shape() {
        let spec = SongSpec::parse(
            "
            form: intro verse chorus

            [section intro]
            bars: 4
            intensity: 0.2

            [section chorus]
            bars: 16
            ",
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
    fn a_section_intensity_follows_its_name() {
        let spec = SongSpec::parse("form: intro chorus outro").unwrap();
        assert!(spec.sections["intro"].intensity < spec.sections["chorus"].intensity);
        assert!(spec.sections["outro"].intensity < spec.sections["chorus"].intensity);
    }

    #[test]
    fn declaring_any_part_replaces_the_default_roster() {
        let spec = SongSpec::parse(
            "
            [part bass]
            octave: 1

            [part kick]
            ",
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
    fn a_part_takes_its_instrument_and_octave_from_its_role() {
        let spec = SongSpec::parse(
            "
            [part anything]
            role: bass
            ",
        )
        .unwrap();
        assert_eq!(spec.parts[0].instrument, "auris.synth.fm2");
        assert_eq!(spec.parts[0].octave, 2);
        assert_eq!(spec.parts[0].range(), (28, 52));

        // Until it is told otherwise.
        let pinned = SongSpec::parse(
            "
            [part anything]
            role: bass
            instrument: auris.synth.chiptune
            octave: 3
            ",
        )
        .unwrap();
        assert_eq!(pinned.parts[0].instrument, "auris.synth.chiptune");
        assert_eq!(pinned.parts[0].octave, 3);
        assert_eq!(pinned.parts[0].range(), (40, 64), "the range moves with it");
    }

    #[test]
    fn a_rhythm_can_be_written_by_hand() {
        let spec = SongSpec::parse(
            "
            [part kick]
            rhythm: x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~
            ",
        )
        .unwrap();
        let rhythm = spec.parts[0].rhythm.as_ref().unwrap();
        assert_eq!(rhythm.onsets(), vec![0, 4, 8, 12]);
    }

    #[test]
    fn a_mood_word_lands_on_numbers() {
        let dark = SongSpec::parse("mood: dark").unwrap();
        let bright = SongSpec::parse("mood: bright").unwrap();
        assert!(dark.mood.brightness < bright.mood.brightness);
        assert!(dark.mood.energy < bright.mood.energy);

        // And an axis can be nudged on its own.
        let nudged = SongSpec::parse("mood: dark\nenergy: 0.9").unwrap();
        assert_eq!(nudged.mood.brightness, dark.mood.brightness);
        assert_eq!(nudged.mood.energy, 0.9);
    }

    #[test]
    fn a_line_the_format_does_not_understand_is_an_error() {
        // Silently ignoring it would mean the piece quietly ignores an instruction.
        let errors = SongSpec::parse("tempoo: 120").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 1);
        assert!(errors[0].message.contains("tempoo"), "{:?}", errors[0]);

        let errors = SongSpec::parse("key: H minor").unwrap_err();
        assert!(errors[0].message.contains("not a key"));

        let errors = SongSpec::parse("this is not a field").unwrap_err();
        assert!(errors[0].message.contains("field: value"));
    }

    #[test]
    fn every_error_is_reported_at_once_with_its_line() {
        let errors = SongSpec::parse(
            "
            key: H minor
            tempo: fast
            groove: nonsense
            ",
        )
        .unwrap_err();
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert_eq!(errors[0].line, 2);
        assert_eq!(errors[1].line, 3);
        assert_eq!(errors[2].line, 4);
    }

    #[test]
    fn numbers_are_checked_against_the_range_they_mean() {
        assert!(SongSpec::parse("tempo: 5").is_err());
        assert!(SongSpec::parse("tempo: 900").is_err());
        assert!(SongSpec::parse("energy: 2").is_err());
        assert!(SongSpec::parse("energy: -1").is_err());
        assert!(SongSpec::parse("swing: 200").is_err());
        assert!(SongSpec::parse("meter: 4/5").is_err());
        assert!(SongSpec::parse("meter: nonsense").is_err());
        assert!(SongSpec::parse("tempo: 128").is_ok());
        assert!(SongSpec::parse("meter: 3/4").is_ok());
        assert!(SongSpec::parse("meter: 6/8").is_ok());
    }

    #[test]
    fn an_error_names_what_would_have_worked() {
        let errors = SongSpec::parse("mood: sideways").unwrap_err();
        assert!(errors[0].message.contains("bright"), "{:?}", errors[0]);
        let errors = SongSpec::parse("groove: sideways").unwrap_err();
        assert!(errors[0].message.contains("basic-rock"), "{:?}", errors[0]);
        let errors = SongSpec::parse("[part x]\nrole: sideways").unwrap_err();
        assert!(errors[0].message.contains("melody"), "{:?}", errors[0]);
    }

    #[test]
    fn a_spec_round_trips_through_its_text() {
        let original = SongSpec::parse(
            "
            title: Test Piece
            key: Bb minor
            tempo: 132
            meter: 3/4
            seed: 99
            mood: dreamy
            groove: shuffle
            form: intro verse verse outro
            chords: @royal-road

            [section verse]
            bars: 16
            intensity: 0.7
            transpose: 2

            [part lead]
            role: melody
            octave: 6
            density: 0.4
            gain: -3

            [part bass]
            role: bass
            ",
        )
        .unwrap();

        let reparsed = SongSpec::parse(&original.to_text()).unwrap();
        assert_eq!(reparsed.title, original.title);
        assert_eq!(reparsed.key, original.key);
        assert_eq!(reparsed.tempo, original.tempo);
        assert_eq!(reparsed.meter, original.meter);
        assert_eq!(reparsed.seed, original.seed);
        assert_eq!(reparsed.mood, original.mood);
        assert_eq!(reparsed.humanize, original.humanize);
        assert_eq!(reparsed.dynamics, original.dynamics);
        assert_eq!(reparsed.groove, original.groove);
        assert_eq!(reparsed.form, original.form);
        assert_eq!(reparsed.parts, original.parts);
        assert_eq!(reparsed.sections, original.sections);
        assert_eq!(reparsed.total_bars(), original.total_bars());
    }

    #[test]
    fn a_subdivision_and_a_gate_survive_the_round_trip() {
        // Both are written out only when they differ from what the role implies, so that the
        // ordinary document stays short — which means the round trip has to work from both sides
        // of that condition rather than only from the side that writes a line.
        let original = SongSpec::parse(
            "form: verse\n[part lead]\n[part chords]\nsubdivision: 16t\ngate: 0.25\n[part stab]",
        )
        .unwrap();
        let reparsed = SongSpec::parse(&original.to_text()).unwrap();
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
        let errors = SongSpec::parse("[part chords]\ngate: 0").unwrap_err();
        assert!(errors.iter().any(|error| error.message.contains("gate")));
    }

    #[test]
    fn a_section_can_name_which_parts_play() {
        let spec = SongSpec::parse(
            "
            form: intro chorus

            [section intro]
            parts: bass kick
            ",
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
