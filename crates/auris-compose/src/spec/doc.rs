//! The file on disk: the TOML document, and the conversions to and from a specification.
//!
//! `SongDoc` and its tables are the shape of a `.asong` file and [`SongSpec`] is the shape of a
//! song, and this is the one place the two meet. It is a file of its own because that meeting is
//! where the format's whole vocabulary lives — every value spelt the way a musician spells it,
//! every default that comes from another field, every range a number is checked against — and
//! because changing what a song *is* should not mean reading past all of it.
//!
//! [`SpecError`] is here too. Every complaint is raised by these conversions and by nothing else,
//! which is what lets the two constructors that make one stay private.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use auris_core::Subdivision;
use auris_core::time::TimeSignature;

use crate::gm;
use crate::rhythm::Pattern;
use crate::theory::chart::{Chart, ChartOrigin};
use crate::theory::key::Key;
use crate::theory::scale::ScaleId;

use super::{Ending, LeadIn, Mood, PartSpec, PartTweak, Role, SectionSpec, SongSpec};

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

impl SongSpec {
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
    ending: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tempo: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead_in: Option<String>,
    #[serde(
        default,
        deserialize_with = "words_or_list",
        skip_serializing_if = "Option::is_none"
    )]
    parts: Option<Vec<String>>,
    /// `[section.chorus.part.lead]`: what this section changes about one part.
    ///
    /// A table keyed by name, unlike the roster's `[[part]]`, which is an array. The order of the
    /// roster is the order the tracks are created in and means something; the order of a
    /// section's tweaks means nothing at all — each one names the part it patches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    part: BTreeMap<String, PartTweakDoc>,
}

/// One `[section.name.part.name]` table: the fields a section may change about a part.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartTweakDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    octave: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdivision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rhythm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<u8>,
}

/// The `program = …` field: a name or a number on the way in, and on the way out whichever name
/// suits the part's role.
///
/// The flag is consulted only when writing. On the way in it does not matter, because
/// [`gm::Program::parse`] reads a kit's name, a program's name and a bare number all into the same
/// number — which of the three the document happened to use is not a fact worth keeping.
///
/// It exists because the same number means two unrelated things and serde cannot see the role
/// from inside a field. Writing `program = "Acoustic Guitar (nylon)"` on a snare part would be a
/// document that parses back correctly and reads as nonsense, which is the worse kind of wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProgramField {
    program: gm::Program,
    drums: bool,
}

impl Serialize for ProgramField {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.program.label(self.drums))
    }
}

impl<'de> Deserialize<'de> for ProgramField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            program: gm::Program::deserialize(deserializer)?,
            drums: false,
        })
    }
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
    program: Option<ProgramField>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<u8>,
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
        if let Some(text) = &self.ending {
            match Ending::parse(text) {
                Some(ending) => spec.ending = ending,
                None => errors.push(SpecError::about(format!(
                    "`{text}` is not an ending; try held or none"
                ))),
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
            for part in section.parts.iter().chain(section.tweaks.keys()) {
                if !part_names.contains(&part.as_str()) {
                    errors.push(SpecError::about(format!(
                        "section `{name}` names the part `{part}`, which does not exist; there \
                         is {}",
                        part_names.join(", ")
                    )));
                }
            }
            // The same complaint the roster raises about a `note` on a pitched part, and it has
            // to be raised here because it is the only place that can: a tweak is read knowing
            // the section and the part's *name*, and which role answers to that name is a fact
            // about the roster. Without it one half of the format refused an instruction and the
            // other half took it and dropped it.
            for (part, tweak) in &section.tweaks {
                let Some(played) = spec.parts.iter().find(|entry| entry.name == *part) else {
                    continue;
                };
                if tweak.note.is_some() && played.role.drum_voice().is_none() {
                    errors.push(SpecError::about(format!(
                        "section `{name}`, part `{part}` plays {}, whose notes come from the \
                         harmony; `note` is for a drum part, which strikes one",
                        played.role.name()
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
        if let Some(tempo) = self.tempo {
            // The same range the song's own tempo is held to, said the same way: a section that
            // could be asked for 900 BPM would be a change the transport cannot follow.
            if (20.0..=400.0).contains(&tempo) {
                section.tempo = Some(tempo);
            } else {
                errors.push(SpecError::about(format!(
                    "section `{name}`: a tempo of {tempo} is outside 20..400"
                )));
            }
        }
        if let Some(text) = self.lead_in {
            match LeadIn::parse(&text) {
                Some(lead_in) => section.lead_in = lead_in,
                None => errors.push(SpecError::about(format!(
                    "section `{name}`: `{text}` is not a lead-in; there is {}, {}",
                    LeadIn::Dominant.name(),
                    LeadIn::None.name()
                ))),
            }
        }
        if let Some(parts) = self.parts {
            // `*` is how the line-oriented format said "everything", which is what an empty
            // list already means. Still accepted, so nobody's document breaks over a star.
            section.parts = parts.into_iter().filter(|name| name != "*").collect();
        }
        for (part, tweak) in self.part {
            let tweak = tweak.into_spec(name, &part, errors);
            if !tweak.is_empty() {
                section.tweaks.insert(part, tweak);
            }
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
        // Nothing to check: `Program` refuses anything outside 0..127 as it is read, which is the
        // one thing that could be wrong about it.
        part.program = self.program.map(|field| field.program);
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
        if let Some(note) = self.note {
            if note > 127 {
                errors.push(SpecError::about(format!(
                    "part `{name}`: {note} is not a MIDI note, which runs 0 to 127"
                )));
            } else if part.role.drum_voice().is_none() {
                // Not a warning to be ignored: a pitched part draws its notes from the harmony,
                // so a `note` on one is an instruction that would have been silently dropped —
                // and the person who wrote it would go looking for why the melody ignored them.
                errors.push(SpecError::about(format!(
                    "part `{name}` plays {}, whose notes come from the harmony; `note` is for a \
                     drum part, which strikes one",
                    part.role.name()
                )));
            } else {
                part.note = Some(note);
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
            ending: Some(spec.ending.name().to_string()),
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
            tempo: section.tempo,
            lead_in: (section.lead_in != plain.lead_in).then(|| section.lead_in.name().to_string()),
            parts: (!section.parts.is_empty()).then(|| section.parts.clone()),
            part: section
                .tweaks
                .iter()
                .map(|(name, tweak)| (name.clone(), PartTweakDoc::from_spec(tweak)))
                .collect(),
        }
    }
}

impl PartTweakDoc {
    /// A patch on one part, with every value checked against the range it means.
    ///
    /// The same ranges the part itself is held to, and complained about the same way: a density of
    /// 3 is no more writable here than it is in the roster, and a person who typed one wants to be
    /// told rather than to be given 1.
    fn into_spec(self, section: &str, part: &str, errors: &mut Vec<SpecError>) -> PartTweak {
        let mut tweak = PartTweak::default();
        let what = format!("section `{section}`, part `{part}`");
        if let Some(density) = self.density {
            if (0.0..=1.0).contains(&density) {
                tweak.density = Some(density);
            } else {
                errors.push(SpecError::about(format!(
                    "{what}: density runs from 0 to 1, not {density}"
                )));
            }
        }
        if let Some(octave) = self.octave {
            if (-1..=9).contains(&octave) {
                tweak.octave = Some(octave);
            } else {
                errors.push(SpecError::about(format!(
                    "{what}: octave {octave} is outside the MIDI range"
                )));
            }
        }
        if let Some(gate) = self.gate {
            // Zero would write a note of no length at every onset, which is a part that is silent
            // and looks like one that is not.
            if (0.0..=1.0).contains(&gate) && gate > 0.0 {
                tweak.gate = Some(gate);
            } else {
                errors.push(SpecError::about(format!(
                    "{what}: gate runs from just above 0 to 1, not {gate}"
                )));
            }
        }
        if let Some(text) = &self.subdivision {
            match Subdivision::parse(text) {
                Some(subdivision) => tweak.subdivision = Some(subdivision),
                None => errors.push(SpecError::about(format!(
                    "{what}: `{text}` is not a subdivision; use 8, 16, 8t or 16t"
                ))),
            }
        }
        if let Some(text) = &self.rhythm {
            match Pattern::parse(text) {
                Some(pattern) => tweak.rhythm = Some(pattern),
                None => errors.push(SpecError::about(format!(
                    "{what}: `{text}` is not a rhythm; use x, X, o and ~"
                ))),
            }
        }
        // A note above 127 cannot be written: `u8` stops at 255 and serde has already refused
        // anything larger, so this is the only half of the range left to check.
        //
        // Whether the part it names strikes a note at all is *not* checked here and cannot be: a
        // tweak knows the part's name and the roster is what knows its role. `SongSpec::into_spec`
        // raises that one, beside the complaint about a name no part answers to.
        if let Some(note) = self.note {
            if note > 127 {
                errors.push(SpecError::about(format!(
                    "{what}: {note} is not a MIDI note, which runs 0 to 127"
                )));
            } else {
                tweak.note = Some(note);
            }
        }
        tweak
    }

    /// The document a tweak would be written as.
    fn from_spec(tweak: &PartTweak) -> Self {
        Self {
            density: tweak.density,
            octave: tweak.octave,
            gate: tweak.gate,
            subdivision: tweak.subdivision.map(|s| s.name().to_string()),
            rhythm: tweak.rhythm.as_ref().map(Pattern::to_text),
            note: tweak.note,
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
            program: part.program.map(|program| ProgramField {
                program,
                drums: part.role.is_drum(),
            }),
            octave: (part.octave != plain.octave).then_some(part.octave),
            density: part.density,
            subdivision: (part.subdivision != plain.subdivision)
                .then(|| part.subdivision.name().to_string()),
            gate: (part.gate != plain.gate).then_some(part.gate),
            rhythm: part.rhythm.as_ref().map(Pattern::to_text),
            gain: (part.gain_db != plain.gain_db).then_some(f64::from(part.gain_db)),
            pan: (part.pan != plain.pan).then_some(part.pan),
            note: part.note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::rhythm::DrumVoice;

    #[test]
    fn a_drum_part_strikes_the_note_it_names_and_general_midi_otherwise() {
        // General MIDI is the only agreement there is about which number is a kick, and a
        // SoundFont is under no obligation to keep it: a kit that puts its snare somewhere else
        // came out silent or playing a cowbell, and there was nothing to say otherwise.
        let plain = PartSpec::of_role("kick", Role::Kick);
        assert_eq!(plain.drum_note(), Some(DrumVoice::Kick.pitch()));
        let moved = PartSpec {
            note: Some(60),
            ..PartSpec::of_role("kick", Role::Kick)
        };
        assert_eq!(moved.drum_note(), Some(60));

        // A pitched part has no such note at all — its notes come from the harmony — and the
        // format says so rather than dropping the instruction where nobody would find it.
        assert_eq!(PartSpec::of_role("lead", Role::Melody).drum_note(), None);
        let complaint = SongSpec::parse(
            r#"
            form = "verse"
            [[part]]
            name = "lead"
            role = "melody"
            note = 60
            "#,
        )
        .expect_err("a note on a melody is an instruction that cannot be obeyed");
        assert!(
            complaint[0].to_string().contains("harmony"),
            "{complaint:?}"
        );

        // And it survives the trip through the file, which is what makes it a setting rather
        // than a thing to type again every time.
        let spec = SongSpec::parse(
            r#"
            form = "verse"
            [[part]]
            name = "kick"
            note = 24
            "#,
        )
        .unwrap();
        assert_eq!(spec.parts[0].drum_note(), Some(24));
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));

        assert!(
            SongSpec::parse("form = \"verse\"\n[[part]]\nname = \"kick\"\nnote = 200").is_err(),
            "200 is not a MIDI note"
        );
    }

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
    fn a_section_can_name_a_tempo_of_its_own() {
        let spec = SongSpec::parse(
            r#"
            tempo = 120
            form  = "verse chorus"

            [section.chorus]
            tempo = 132
            "#,
        )
        .unwrap();
        assert_eq!(spec.sections["chorus"].tempo, Some(132.0));
        assert_eq!(
            spec.sections["verse"].tempo, None,
            "a section that does not say follows the song"
        );
        assert_eq!(spec.tempo_of(&spec.sections["chorus"]), 132.0);
        assert_eq!(spec.tempo_of(&spec.sections["verse"]), 120.0);
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));

        // The same range the song's own tempo is held to, and complained about the same way.
        let errors = SongSpec::parse("form = \"verse\"\n[section.verse]\ntempo = 900").unwrap_err();
        assert!(
            errors.iter().any(|error| error.to_string().contains("900")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_section_can_patch_how_a_part_plays() {
        let spec = SongSpec::parse(
            r#"
            form = "verse chorus"

            [section.chorus.part.lead]
            octave      = 6
            density     = 0.85
            subdivision = "16"
            "#,
        )
        .unwrap();
        let tweak = &spec.sections["chorus"].tweaks["lead"];
        assert_eq!(tweak.octave, Some(6));
        assert_eq!(tweak.density, Some(0.85));
        assert_eq!(tweak.gate, None, "what it did not name it does not touch");
        assert!(spec.sections["verse"].tweaks.is_empty());
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));
    }

    #[test]
    fn a_tweak_is_held_to_the_same_ranges_the_part_is() {
        // A density of 3 is no more writable here than in the roster, and a person who typed one
        // wants to be told rather than to be handed 1. Both complaints at once, which is what the
        // format promises about meaning.
        let errors = SongSpec::parse(
            r#"
            form = "verse"

            [section.verse.part.lead]
            density = 3
            octave  = 40
            "#,
        )
        .unwrap_err();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .all(|error| error.to_string().contains("lead"))
        );
    }

    #[test]
    fn a_tweak_may_only_name_a_note_where_the_part_strikes_one() {
        // The roster refuses this and says why: a pitched part's notes come from the harmony, so
        // a `note` on one is an instruction that would be silently dropped, and the person who
        // wrote it would go looking for why the melody ignored them. A tweak used to take it and
        // do exactly that — the half of the format that could not see the role was the half that
        // said nothing.
        let errors = SongSpec::parse(
            r#"
            form = "verse"

            [section.verse.part.lead]
            note = 60
            "#,
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("melody")),
            "{errors:?}"
        );

        // And on a drum part it is the point of the field: a kit that puts its snare somewhere
        // General MIDI does not comes out silent, and one section may want the other one.
        let spec = SongSpec::parse(
            r#"
            form = "verse"

            [section.verse.part.snare]
            note = 40
            "#,
        )
        .expect("a drum part strikes a note");
        assert_eq!(spec.sections["verse"].tweaks["snare"].note, Some(40));
    }

    #[test]
    fn a_tweak_naming_a_part_that_does_not_exist_is_a_mistake() {
        // The same complaint `parts` raises, for the same reason: a name nothing answers to is an
        // instruction that would be silently dropped, and the person who wrote it would go looking
        // for why the chorus sounded like the verse.
        let errors = SongSpec::parse(
            r#"
            form = "verse"

            [section.verse.part.trombone]
            octave = 3
            "#,
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("trombone")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_section_can_refuse_to_be_led_into() {
        let spec = SongSpec::parse(
            r#"
            form = "verse chorus"

            [section.chorus]
            transpose = 2
            lead_in   = "none"
            "#,
        )
        .unwrap();
        assert_eq!(spec.sections["chorus"].lead_in, LeadIn::None);
        assert_eq!(
            spec.sections["verse"].lead_in,
            LeadIn::Dominant,
            "a section that does not say is prepared"
        );
        // Only written where it is not the default, and read back where it is.
        assert!(spec.to_toml().contains("lead_in"));
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));
        assert!(
            !SongSpec::parse("form = \"verse\"")
                .unwrap()
                .to_toml()
                .contains("lead_in")
        );

        let errors =
            SongSpec::parse("form = \"verse\"\n[section.verse]\nlead_in = \"pivot\"").unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("pivot")),
            "{errors:?}"
        );
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
    fn a_program_survives_the_round_trip_by_name() {
        let original = SongSpec::parse(
            r#"
            form = ["verse"]

            [[part]]
            name    = "lead"
            program = "Violin"

            [[part]]
            name    = "kick"
            role    = "kick"
            program = 24

            [[part]]
            name = "pad"
            "#,
        )
        .unwrap();

        let named = |spec: &SongSpec, name: &str| {
            spec.parts
                .iter()
                .find(|part| part.name == name)
                .expect("the part is in the roster")
                .clone()
        };
        assert_eq!(named(&original, "lead").program, Some(gm::Program(40)));
        assert_eq!(named(&original, "kick").program, Some(gm::Program(24)));
        assert_eq!(named(&original, "pad").program, None);

        // Written back as a name whichever way it arrived, because a specification is meant to be
        // read — `program = 24` is a fact about a MIDI chart and `"Electronic Kit"` is a fact
        // about the music.
        let written = original.to_toml();
        assert!(written.contains(r#"program = "Violin""#), "{written}");
        assert!(
            written.contains(r#"program = "Electronic Kit""#),
            "a kit is written as the kit it is: {written}"
        );
        assert_eq!(SongSpec::parse(&written).unwrap().parts, original.parts);
    }

    #[test]
    fn a_program_nobody_recognises_is_an_error_rather_than_a_grand_piano() {
        // Silently falling back to program 0 would give a piece full of pianos and no clue why.
        let errors = SongSpec::parse(
            r#"
            [[part]]
            name    = "lead"
            program = "tuba solo"
            "#,
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("tuba")),
            "{errors:?}"
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
}
