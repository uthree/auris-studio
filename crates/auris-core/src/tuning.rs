//! Tunings: which frequency each key of the keyboard sounds.
//!
//! A note's [`pitch`](crate::project::Note::pitch) is a **key**, not a frequency. In the standard
//! tuning the two are the same thing — key 69 is A440 and each key up is one equal-tempered
//! semitone — and that is why the field could be read as a MIDI note number for as long as nothing
//! else existed. A [`Tuning`] is what separates them: it says which step of a scale each key plays
//! and how high that step is, so key 61 in 31-tone equal temperament is one 38.7-cent step above
//! key 60 rather than a semitone.
//!
//! The model is Scala's, because that is the format every tuning a musician might want already
//! exists in: a list of degrees above a root, ending in the period the scale repeats at (the
//! `.scl` file), and a keyboard mapping that says which key plays the root and which key is
//! pinned to a frequency (the `.kbm` file). Only the *linear* mapping is held — consecutive keys
//! play consecutive degrees — which is what a `.kbm` with an empty map, or with the identity map,
//! says. A mapping that skips or repeats degrees is refused rather than half-honoured.
//!
//! What the engine needs from all this is [`Tuning::pitch_offsets`]: for each of the 128 keys, how
//! many semitones its sound lies away from the equal-tempered key of the same number. That table is
//! built once per graph, off the audio thread, and adding one of its entries to a note-on is the
//! whole of what retuning costs while playing.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::theory::pitch::PitchClass;

/// Keys a tuning maps: the MIDI note range, which is the range a note's key is stored in.
pub const KEY_COUNT: usize = 128;

/// Cents in an octave.
pub const OCTAVE_CENTS: f64 = 1200.0;

/// The most degrees a tuning may hold.
///
/// Far beyond anything played — 128 keys reach only 128 of them — and present because a document
/// is cloned for every step of undo, and a pasted file of a hundred thousand lines should be
/// refused rather than copied that many times.
pub const MAX_DEGREES: usize = 2048;

/// The widest period a tuning may repeat at, in cents: four octaves.
///
/// The tritave of Bohlen–Pierce is 1902 cents and nothing in the Scala archive that anyone plays
/// is wider than two octaves. The cap is what keeps every key's frequency a finite number.
pub const MAX_PERIOD_CENTS: f64 = 4.0 * OCTAVE_CENTS;

/// Lowest and highest sounding pitch a key may be given, in semitones on the MIDI scale.
///
/// About 2 Hz and 85 kHz. A tuning with few, wide steps can push the outer keys of the keyboard
/// far past hearing; clamping there keeps the table finite, and every instrument already limits
/// what it renders to what its sample rate can hold.
const SOUNDING_RANGE: (f64, f64) = (-24.0, 160.0);

/// Middle C as an equal-tempered piano sounds it, in hertz: Scala's 1/1 when no mapping says
/// otherwise.
pub const MIDDLE_C_HZ: f64 = 261.625_565_300_598_6;

/// Why a tuning could not be built or read.
#[derive(Clone, Debug, PartialEq, Error)]
pub enum TuningError {
    /// A tuning with no degrees has no period to repeat at.
    #[error("a tuning needs at least one degree")]
    NoDegrees,
    /// More degrees than [`MAX_DEGREES`].
    #[error("a tuning may hold at most {MAX_DEGREES} degrees, not {0}")]
    TooManyDegrees(usize),
    /// A degree that is not a finite number of cents above the one before it.
    #[error("degree {index} ({cents} cents) does not lie above the degree before it")]
    NotAscending {
        /// 1-based position in the degree list, as a Scala file counts it.
        index: usize,
        /// The offending value.
        cents: f64,
    },
    /// A period wider than [`MAX_PERIOD_CENTS`].
    #[error("a period of {0} cents is wider than the {MAX_PERIOD_CENTS}-cent limit")]
    PeriodTooWide(f64),
    /// A reference frequency that is not a positive, finite number of hertz.
    #[error("{0} Hz is not a reference frequency")]
    Reference(f64),
    /// A key outside the MIDI range.
    #[error("key {0} is outside 0 to 127")]
    Key(i64),
    /// An equal division with no steps.
    #[error("an equal temperament needs at least one step")]
    NoSteps,
    /// A line of a Scala file that could not be read.
    #[error("line {line} of the Scala file: {message}")]
    Scala {
        /// 1-based line number.
        line: usize,
        /// What was wrong with it.
        message: String,
    },
    /// A keyboard mapping that does not play consecutive degrees on consecutive keys.
    #[error(
        "only keyboard mappings that play consecutive degrees on consecutive keys are supported"
    )]
    NonLinearMapping,
    /// A preset name that names no preset.
    #[error("`{0}` is not a tuning preset")]
    UnknownPreset(String),
}

/// Which frequency each key sounds: a scale, and where on the keyboard it sits.
///
/// Built only through constructors that check it, and checked again when read from a file, so a
/// `Tuning` that exists is always one every key of which has a finite frequency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StoredTuning", into = "StoredTuning")]
pub struct Tuning {
    name: String,
    degrees: Vec<f64>,
    root_key: u8,
    reference_key: u8,
    reference_hz: f64,
}

/// The stored shape of a [`Tuning`], checked on the way in.
#[derive(Clone, Serialize, Deserialize)]
struct StoredTuning {
    name: String,
    degrees: Vec<f64>,
    root_key: u8,
    reference_key: u8,
    reference_hz: f64,
}

impl TryFrom<StoredTuning> for Tuning {
    type Error = TuningError;

    fn try_from(stored: StoredTuning) -> Result<Self, Self::Error> {
        Tuning::new(
            stored.name,
            stored.degrees,
            stored.root_key,
            stored.reference_key,
            stored.reference_hz,
        )
    }
}

impl From<Tuning> for StoredTuning {
    fn from(tuning: Tuning) -> Self {
        Self {
            name: tuning.name,
            degrees: tuning.degrees,
            root_key: tuning.root_key,
            reference_key: tuning.reference_key,
            reference_hz: tuning.reference_hz,
        }
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Self::standard()
    }
}

impl Tuning {
    /// A tuning from its parts.
    ///
    /// `degrees` are cents above the root, ascending, and the last of them is the period the scale
    /// repeats at — Scala's convention, so a twelve-note scale lists eleven notes and then 1200.
    /// `root_key` plays the root, and `reference_key` sounds at `reference_hz`.
    pub fn new(
        name: impl Into<String>,
        degrees: Vec<f64>,
        root_key: u8,
        reference_key: u8,
        reference_hz: f64,
    ) -> Result<Self, TuningError> {
        if degrees.is_empty() {
            return Err(TuningError::NoDegrees);
        }
        if degrees.len() > MAX_DEGREES {
            return Err(TuningError::TooManyDegrees(degrees.len()));
        }
        let mut below = 0.0;
        for (index, &cents) in degrees.iter().enumerate() {
            if !cents.is_finite() || cents <= below {
                return Err(TuningError::NotAscending {
                    index: index + 1,
                    cents,
                });
            }
            below = cents;
        }
        if below > MAX_PERIOD_CENTS {
            return Err(TuningError::PeriodTooWide(below));
        }
        if !(reference_hz.is_finite() && reference_hz > 0.0) {
            return Err(TuningError::Reference(reference_hz));
        }
        for key in [root_key, reference_key] {
            if usize::from(key) >= KEY_COUNT {
                return Err(TuningError::Key(i64::from(key)));
            }
        }
        Ok(Self {
            name: name.into(),
            degrees,
            root_key,
            reference_key,
            reference_hz,
        })
    }

    /// Twelve-tone equal temperament with A4 = 440 Hz: every key sounds its MIDI note number.
    pub fn standard() -> Self {
        Self {
            name: "12-tone equal temperament".into(),
            degrees: (1..=12).map(|step| f64::from(step) * 100.0).collect(),
            root_key: 60,
            reference_key: 69,
            reference_hz: 440.0,
        }
    }

    /// `steps` equal divisions of `period_cents`, with key 60 playing the root at middle C.
    pub fn equal(
        name: impl Into<String>,
        steps: u16,
        period_cents: f64,
    ) -> Result<Self, TuningError> {
        if steps == 0 {
            return Err(TuningError::NoSteps);
        }
        let step = period_cents / f64::from(steps);
        let degrees = (1..=steps).map(|index| f64::from(index) * step).collect();
        Self::new(name, degrees, 60, 60, MIDDLE_C_HZ)
    }

    /// `steps` equal divisions of the octave.
    ///
    /// Twelve is [`Self::standard`] itself rather than a tuning that sounds the same and is pinned
    /// at a different key, so choosing "12-EDO" from a list leaves a document exactly as it was.
    pub fn edo(steps: u16) -> Result<Self, TuningError> {
        match steps {
            12 => Ok(Self::standard()),
            _ => Self::equal(
                format!("{steps}-tone equal temperament"),
                steps,
                OCTAVE_CENTS,
            ),
        }
    }

    /// Five-limit just intonation on `root`: the symmetric twelve-note scale built from the
    /// ratios 16/15, 9/8, 6/5, 5/4, 4/3, 45/32, 3/2, 8/5, 5/3, 16/9 and 15/8.
    pub fn just(root: PitchClass) -> Self {
        const RATIOS: [(u32, u32); 12] = [
            (16, 15),
            (9, 8),
            (6, 5),
            (5, 4),
            (4, 3),
            (45, 32),
            (3, 2),
            (8, 5),
            (5, 3),
            (16, 9),
            (15, 8),
            (2, 1),
        ];
        let degrees = RATIOS
            .iter()
            .map(|&(p, q)| ratio_cents(f64::from(p), f64::from(q)))
            .collect();
        Self::on_root(
            format!("Just intonation on {}", root.sharp_name()),
            degrees,
            root,
        )
    }

    /// Quarter-comma meantone on `root`: pure major thirds, the fifths narrowed to make them,
    /// laid out from the flat third to the sharp fifth as a meantone keyboard was.
    pub fn meantone(root: PitchClass) -> Self {
        let fifth = OCTAVE_CENTS * 5f64.log2() / 4.0;
        Self::chain_of_fifths(
            format!("Quarter-comma meantone on {}", root.sharp_name()),
            fifth,
            root,
        )
    }

    /// Pythagorean tuning on `root`: pure fifths, laid out like [`Self::meantone`].
    pub fn pythagorean(root: PitchClass) -> Self {
        Self::chain_of_fifths(
            format!("Pythagorean on {}", root.sharp_name()),
            ratio_cents(3.0, 2.0),
            root,
        )
    }

    /// Werckmeister III, the well temperament: four fifths narrowed by a quarter of the
    /// Pythagorean comma, so every key is playable and each has a colour of its own.
    ///
    /// Fixed on C, because the colours are what the temperament is for: transposed, it would give
    /// each key the colour of a different one.
    pub fn werckmeister_iii() -> Self {
        let degrees = vec![
            90.225, 192.180, 294.135, 390.225, 498.045, 588.270, 696.090, 792.180, 888.270,
            996.090, 1092.180, 1200.0,
        ];
        Self::on_root("Werckmeister III", degrees, PitchClass::new(0))
    }

    /// The Bohlen–Pierce scale: thirteen equal divisions of the tritave, 3:1.
    pub fn bohlen_pierce() -> Self {
        Self::equal("Bohlen–Pierce", 13, ratio_cents(3.0, 1.0))
            .expect("thirteen steps of a tritave is a valid tuning")
    }

    /// A twelve-note scale whose root is `root`'s key nearest middle C, pinned where an
    /// equal-tempered piano has that key, so the tonic agrees with every track that is not
    /// retuned.
    fn on_root(name: impl Into<String>, degrees: Vec<f64>, root: PitchClass) -> Self {
        let root_key = 60 + root.semitones() as u8;
        Self::new(
            name,
            degrees,
            root_key,
            root_key,
            Self::standard().hz(root_key),
        )
        .expect("a built-in twelve-note scale is a valid tuning")
    }

    /// Twelve notes reached by stacking `fifth` from three below the root to eight above it.
    fn chain_of_fifths(name: String, fifth: f64, root: PitchClass) -> Self {
        let mut degrees = [0.0; 12];
        for position in -3i32..=8 {
            let cents = (f64::from(position) * fifth).rem_euclid(OCTAVE_CENTS);
            let step = (position * 7).rem_euclid(12) as usize;
            degrees[step] = cents;
        }
        let mut degrees: Vec<f64> = degrees[1..].to_vec();
        degrees.push(OCTAVE_CENTS);
        Self::on_root(name, degrees, root)
    }

    /// The name shown to a person.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Cents above the root of each degree, the last being the period.
    pub fn degrees(&self) -> &[f64] {
        &self.degrees
    }

    /// The key that plays the root.
    pub fn root_key(&self) -> u8 {
        self.root_key
    }

    /// The key pinned to [`Self::reference_hz`].
    pub fn reference_key(&self) -> u8 {
        self.reference_key
    }

    /// The frequency [`Self::reference_key`] sounds at.
    pub fn reference_hz(&self) -> f64 {
        self.reference_hz
    }

    /// Notes in one period of the scale.
    pub fn steps_per_period(&self) -> usize {
        self.degrees.len()
    }

    /// The interval the scale repeats at, in cents.
    pub fn period_cents(&self) -> f64 {
        *self.degrees.last().expect("a tuning always has a degree")
    }

    /// `true` for twelve notes to an octave, which is what keys, chords and roman numerals are
    /// written in.
    ///
    /// Just intonation and the meantones answer yes: their notes are tuned differently, but there
    /// are still twelve of them and a C major chord is still keys 0, 4 and 7 above C.
    pub fn is_twelve_tone(&self) -> bool {
        self.degrees.len() == 12 && (self.period_cents() - OCTAVE_CENTS).abs() < 1e-9
    }

    /// `true` when every key sounds exactly as in [`Self::standard`], whatever it is called.
    pub fn is_standard(&self) -> bool {
        let standard = Self::standard();
        self.degrees == standard.degrees
            && self.root_key == standard.root_key
            && self.reference_key == standard.reference_key
            && self.reference_hz == standard.reference_hz
    }

    /// `true` when some key sounds between two equal-tempered keys — which is what an instrument
    /// that can only play whole MIDI keys gets wrong.
    ///
    /// A tuning that only moves keys by whole semitones answers no: a 12-step tuning with A at 440
    /// does, and so would a mapping that only shuffled keys.
    pub fn puts_notes_between_keys(&self) -> bool {
        self.pitch_offsets()
            .iter()
            .any(|offset| (offset - offset.round()).abs() > 1e-4)
    }

    /// This tuning with `key` sounding at `hz`, every interval kept — "A = 442" in the standard
    /// tuning is `with_reference(69, 442.0)`.
    pub fn with_reference(&self, key: u8, hz: f64) -> Result<Self, TuningError> {
        Self::new(
            self.name.clone(),
            self.degrees.clone(),
            self.root_key,
            key,
            hz,
        )
    }

    /// This tuning under another name.
    pub fn renamed(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Cents from the root's key to `key`, which may be below it.
    fn cents_from_root(&self, key: i64) -> f64 {
        let steps = key - i64::from(self.root_key);
        let count = self.degrees.len() as i64;
        let periods = steps.div_euclid(count);
        let degree = steps.rem_euclid(count) as usize;
        let within = match degree {
            0 => 0.0,
            _ => self.degrees[degree - 1],
        };
        periods as f64 * self.period_cents() + within
    }

    /// The frequency `key` sounds at, in hertz.
    pub fn hz(&self, key: u8) -> f64 {
        let cents = self.cents_from_root(i64::from(key))
            - self.cents_from_root(i64::from(self.reference_key));
        self.reference_hz * (cents / OCTAVE_CENTS).exp2()
    }

    /// Where `key` sounds, in semitones on the MIDI scale: 69.0 is A440, and a fraction lies
    /// between two equal-tempered keys.
    pub fn sounding_pitch(&self, key: u8) -> f64 {
        let pitch = 69.0 + 12.0 * (self.hz(key) / 440.0).log2();
        pitch.clamp(SOUNDING_RANGE.0, SOUNDING_RANGE.1)
    }

    /// For every key, the semitones its sound lies away from the equal-tempered key of the same
    /// number — what the engine adds to each note-on.
    pub fn pitch_offsets(&self) -> [f32; KEY_COUNT] {
        let mut offsets = [0.0; KEY_COUNT];
        for (key, offset) in offsets.iter_mut().enumerate() {
            *offset = (self.sounding_pitch(key as u8) - key as f64) as f32;
        }
        offsets
    }

    /// Reads a Scala `.scl` file, mapped as Scala maps one with no keyboard file: key 60 plays
    /// the root at middle C.
    pub fn from_scala(text: &str) -> Result<Self, TuningError> {
        let mut lines = text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line))
            .filter(|(_, line)| !line.starts_with('!'));
        let (_, description) = lines.next().ok_or(TuningError::Scala {
            line: 1,
            message: "the file is empty".into(),
        })?;
        let (count_line, count) = lines.next().ok_or(TuningError::Scala {
            line: 2,
            message: "the file ends before its note count".into(),
        })?;
        let count: usize = first_token(count).parse().map_err(|_| TuningError::Scala {
            line: count_line,
            message: format!("`{}` is not a note count", count.trim()),
        })?;
        if count > MAX_DEGREES {
            return Err(TuningError::TooManyDegrees(count));
        }
        let mut degrees = Vec::with_capacity(count);
        let mut last_line = count_line;
        for (line, text) in lines.by_ref().take(count) {
            last_line = line;
            degrees.push(
                scala_pitch(first_token(text))
                    .map_err(|message| TuningError::Scala { line, message })?,
            );
        }
        if degrees.len() < count {
            return Err(TuningError::Scala {
                line: last_line,
                message: format!("the file lists {} of its {count} notes", degrees.len()),
            });
        }
        let name = match description.trim() {
            "" => "Scala tuning".to_string(),
            name => name.to_string(),
        };
        Self::new(name, degrees, 60, 60, MIDDLE_C_HZ)
    }

    /// This tuning placed on the keyboard as a Scala `.kbm` file says.
    ///
    /// Only the linear mappings are read: an empty map, or a map listing every degree once in
    /// order, with the formal octave at the period. The first and last keys are ignored — every
    /// key of the keyboard plays.
    pub fn with_keyboard_map(&self, text: &str) -> Result<Self, TuningError> {
        let mut lines = text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line))
            .filter(|(_, line)| !line.starts_with('!'));
        let mut field = |what: &str| {
            lines.next().ok_or_else(|| TuningError::Scala {
                line: 0,
                message: format!("the keyboard map ends before its {what}"),
            })
        };
        let integer = |(line, text): (usize, &str), what: &str| -> Result<i64, TuningError> {
            first_token(text).parse().map_err(|_| TuningError::Scala {
                line,
                message: format!("`{}` is not a {what}", text.trim()),
            })
        };
        let size = integer(field("map size")?, "map size")?;
        let _first = integer(field("first key")?, "key")?;
        let _last = integer(field("last key")?, "key")?;
        let root = integer(field("middle key")?, "key")?;
        let reference = integer(field("reference key")?, "key")?;
        let (hz_line, hz_text) = field("reference frequency")?;
        let hz: f64 = first_token(hz_text)
            .parse()
            .map_err(|_| TuningError::Scala {
                line: hz_line,
                message: format!("`{}` is not a frequency", hz_text.trim()),
            })?;
        let octave = integer(field("formal octave")?, "degree")?;
        if size != 0 {
            let count = self.degrees.len() as i64;
            if size != count || octave != count {
                return Err(TuningError::NonLinearMapping);
            }
            for expected in 0..size {
                let entry = field("map entries")?;
                if integer(entry, "degree").ok() != Some(expected) {
                    return Err(TuningError::NonLinearMapping);
                }
            }
        }
        let key = |value: i64| {
            u8::try_from(value)
                .ok()
                .filter(|key| usize::from(*key) < KEY_COUNT)
        };
        let root_key = key(root).ok_or(TuningError::Key(root))?;
        let reference_key = key(reference).ok_or(TuningError::Key(reference))?;
        Self::new(
            self.name.clone(),
            self.degrees.clone(),
            root_key,
            reference_key,
            hz,
        )
    }

    /// This tuning's scale as a Scala `.scl` file, every degree in cents.
    pub fn to_scala(&self) -> String {
        let mut text = format!(
            "! {}\n!\n{}\n {}\n!\n",
            self.name,
            self.name,
            self.degrees.len()
        );
        for cents in &self.degrees {
            text.push_str(&format!(" {cents:.6}\n"));
        }
        text
    }

    /// Where this tuning sits on the keyboard, as a Scala `.kbm` file with a linear map.
    pub fn to_keyboard_map(&self) -> String {
        format!(
            "! Keyboard mapping for {}\n0\n0\n127\n{}\n{}\n{:.6}\n{}\n",
            self.name,
            self.root_key,
            self.reference_key,
            self.reference_hz,
            self.degrees.len()
        )
    }
}

/// Cents in the ratio `p/q`.
fn ratio_cents(p: f64, q: f64) -> f64 {
    OCTAVE_CENTS * (p / q).log2()
}

/// The first whitespace-separated token of a line; Scala allows a comment after it.
fn first_token(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// One pitch line of a `.scl` file: cents when it holds a point, a ratio or a whole number
/// otherwise.
fn scala_pitch(token: &str) -> Result<f64, String> {
    if token.is_empty() {
        return Err("a pitch line is empty".into());
    }
    if token.contains('.') {
        return token
            .parse::<f64>()
            .map_err(|_| format!("`{token}` is not a number of cents"));
    }
    let (p, q) = token.split_once('/').unwrap_or((token, "1"));
    let parse = |part: &str| {
        part.parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("`{token}` is not a ratio of positive whole numbers"))
    };
    Ok(ratio_cents(parse(p)? as f64, parse(q)? as f64))
}

/// A tuning a person can choose by name, from a list or on a command line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TuningPreset {
    /// [`Tuning::standard`].
    Standard,
    /// [`Tuning::edo`] with this many steps.
    Edo(u16),
    /// [`Tuning::just`] on this root.
    Just(PitchClass),
    /// [`Tuning::meantone`] on this root.
    Meantone(PitchClass),
    /// [`Tuning::pythagorean`] on this root.
    Pythagorean(PitchClass),
    /// [`Tuning::werckmeister_iii`].
    Werckmeister,
    /// [`Tuning::bohlen_pierce`].
    BohlenPierce,
}

impl TuningPreset {
    /// The tuning this preset names.
    pub fn tuning(self) -> Result<Tuning, TuningError> {
        Ok(match self {
            Self::Standard => Tuning::standard(),
            Self::Edo(steps) => return Tuning::edo(steps),
            Self::Just(root) => Tuning::just(root),
            Self::Meantone(root) => Tuning::meantone(root),
            Self::Pythagorean(root) => Tuning::pythagorean(root),
            Self::Werckmeister => Tuning::werckmeister_iii(),
            Self::BohlenPierce => Tuning::bohlen_pierce(),
        })
    }
}

impl fmt::Display for TuningPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Standard => f.write_str("standard"),
            Self::Edo(steps) => write!(f, "{steps}-edo"),
            Self::Just(root) => write!(f, "just:{}", root.sharp_name()),
            Self::Meantone(root) => write!(f, "meantone:{}", root.sharp_name()),
            Self::Pythagorean(root) => write!(f, "pythagorean:{}", root.sharp_name()),
            Self::Werckmeister => f.write_str("werckmeister"),
            Self::BohlenPierce => f.write_str("bohlen-pierce"),
        }
    }
}

impl FromStr for TuningPreset {
    type Err = TuningError;

    /// Reads what [`Display`](fmt::Display) writes: `standard`, `31-edo`, `just:D`,
    /// `meantone:Eb`, `pythagorean:C`, `werckmeister` or `bohlen-pierce`. A root left off is C.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let unknown = || TuningError::UnknownPreset(text.to_string());
        let lowered = text.trim().to_ascii_lowercase();
        let (name, root) = match lowered.split_once(':') {
            Some((name, root)) => (name, Some(root)),
            None => (lowered.as_str(), None),
        };
        let root = match root {
            Some(root) => parse_root(root).ok_or_else(unknown)?,
            None => PitchClass::new(0),
        };
        Ok(match name {
            "standard" | "12-tet" | "12-edo" => Self::Standard,
            "just" | "ji" => Self::Just(root),
            "meantone" => Self::Meantone(root),
            "pythagorean" => Self::Pythagorean(root),
            "werckmeister" | "werckmeister-iii" => Self::Werckmeister,
            "bohlen-pierce" | "bp" => Self::BohlenPierce,
            other => {
                let steps = other
                    .strip_suffix("-edo")
                    .or_else(|| other.strip_suffix("-tet"))
                    .or_else(|| other.strip_suffix("edo"))
                    .and_then(|steps| steps.parse::<u16>().ok())
                    .filter(|steps| *steps > 0)
                    .ok_or_else(unknown)?;
                match steps {
                    12 => Self::Standard,
                    steps => Self::Edo(steps),
                }
            }
        })
    }
}

/// A root written as a letter with an optional sharp or flat, in any case.
fn parse_root(text: &str) -> Option<PitchClass> {
    let mut chars = text.chars();
    let letter = match chars.next()?.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let accidental = match chars.as_str() {
        "" => 0,
        "#" | "s" => 1,
        "b" => -1,
        _ => return None,
    };
    Some(PitchClass::new(letter + accidental))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn the_standard_tuning_sounds_every_key_at_its_midi_number() {
        let standard = Tuning::standard();
        assert!(standard.is_standard());
        assert!(standard.is_twelve_tone());
        assert!(close(standard.hz(69), 440.0, 1e-9));
        assert!(close(standard.hz(60), MIDDLE_C_HZ, 1e-9));
        for key in 0..KEY_COUNT as u8 {
            assert!(
                close(standard.sounding_pitch(key), f64::from(key), 1e-9),
                "key {key}"
            );
        }
        assert!(
            standard
                .pitch_offsets()
                .iter()
                .all(|offset| offset.abs() < 1e-5)
        );
    }

    #[test]
    fn only_a_tuning_that_moves_a_key_off_the_grid_puts_notes_between_keys() {
        assert!(!Tuning::standard().puts_notes_between_keys());
        // Concert pitch an octave up is on the grid; 442 Hz is not.
        assert!(
            !Tuning::standard()
                .with_reference(69, 880.0)
                .unwrap()
                .puts_notes_between_keys()
        );
        assert!(
            Tuning::standard()
                .with_reference(69, 442.0)
                .unwrap()
                .puts_notes_between_keys()
        );
        assert!(Tuning::edo(24).unwrap().puts_notes_between_keys());
        assert!(Tuning::just(PitchClass::new(0)).puts_notes_between_keys());
    }

    #[test]
    fn twelve_equal_steps_is_the_standard_tuning_itself() {
        assert_eq!(Tuning::edo(12).unwrap(), Tuning::standard());
    }

    #[test]
    fn a_concert_pitch_moves_every_key_by_the_same_interval() {
        let tuning = Tuning::standard().with_reference(69, 442.0).unwrap();
        assert!(!tuning.is_standard());
        let shift = 12.0 * (442.0f64 / 440.0).log2();
        for key in [0u8, 60, 69, 127] {
            assert!(close(
                tuning.sounding_pitch(key) - f64::from(key),
                shift,
                1e-9
            ));
        }
    }

    #[test]
    fn thirty_one_equal_steps_divide_the_octave_and_keep_middle_c() {
        let tuning = Tuning::edo(31).unwrap();
        assert!(!tuning.is_twelve_tone());
        assert_eq!(tuning.steps_per_period(), 31);
        assert!(close(tuning.hz(60), MIDDLE_C_HZ, 1e-9));
        assert!(
            close(tuning.hz(91), 2.0 * MIDDLE_C_HZ, 1e-9),
            "31 keys up is an octave"
        );
        assert!(
            close(tuning.hz(29), MIDDLE_C_HZ / 2.0, 1e-9),
            "and 31 down is one below"
        );
        let step = tuning.sounding_pitch(61) - tuning.sounding_pitch(60);
        assert!(close(step, 12.0 / 31.0, 1e-9));
    }

    #[test]
    fn just_intonation_has_a_pure_major_third_and_keeps_the_tonic_in_place() {
        let tuning = Tuning::just(PitchClass::new(2));
        assert_eq!(tuning.root_key(), 62);
        assert!(close(tuning.hz(62), Tuning::standard().hz(62), 1e-9));
        assert!(close(tuning.hz(66) / tuning.hz(62), 5.0 / 4.0, 1e-12));
        assert!(close(tuning.hz(69) / tuning.hz(62), 3.0 / 2.0, 1e-12));
        assert!(close(tuning.hz(74) / tuning.hz(62), 2.0, 1e-12));
        // Below the tonic too: the degree wraps into the period below.
        assert!(close(tuning.hz(62) / tuning.hz(57), 4.0 / 3.0, 1e-12));
    }

    #[test]
    fn meantone_thirds_are_pure_and_pythagorean_fifths_are() {
        let meantone = Tuning::meantone(PitchClass::new(0));
        assert!(close(meantone.hz(64) / meantone.hz(60), 5.0 / 4.0, 1e-12));
        let pythagorean = Tuning::pythagorean(PitchClass::new(0));
        assert!(close(pythagorean.hz(67) / pythagorean.hz(60), 1.5, 1e-12));
        // The wolf sits between the ends of the chain, G sharp up to E flat.
        let wolf = ratio_cents(pythagorean.hz(75), pythagorean.hz(68));
        assert!(close(wolf, ratio_cents(3.0, 2.0) - 23.46, 0.01), "{wolf}");
    }

    #[test]
    fn bohlen_pierce_repeats_at_the_tritave() {
        let tuning = Tuning::bohlen_pierce();
        assert!(close(tuning.hz(73) / tuning.hz(60), 3.0, 1e-12));
    }

    #[test]
    fn a_tuning_that_does_not_ascend_or_has_no_period_is_refused() {
        assert_eq!(
            Tuning::new("x", vec![], 60, 60, 440.0),
            Err(TuningError::NoDegrees)
        );
        assert!(matches!(
            Tuning::new("x", vec![100.0, 100.0, 1200.0], 60, 60, 440.0),
            Err(TuningError::NotAscending { index: 2, .. })
        ));
        assert!(matches!(
            Tuning::new("x", vec![f64::NAN], 60, 60, 440.0),
            Err(TuningError::NotAscending { index: 1, .. })
        ));
        assert_eq!(
            Tuning::new("x", vec![9600.0], 60, 60, 440.0),
            Err(TuningError::PeriodTooWide(9600.0))
        );
        assert_eq!(
            Tuning::new("x", vec![1200.0], 60, 60, 0.0),
            Err(TuningError::Reference(0.0))
        );
        assert_eq!(
            Tuning::new("x", vec![1200.0], 128, 60, 440.0),
            Err(TuningError::Key(128))
        );
    }

    #[test]
    fn the_widest_tuning_still_gives_every_key_a_finite_pitch() {
        let tuning = Tuning::new("wide", vec![MAX_PERIOD_CENTS], 0, 0, 1.0).unwrap();
        for offset in tuning.pitch_offsets() {
            assert!(offset.is_finite());
        }
    }

    #[test]
    fn a_scala_file_reads_cents_ratios_whole_numbers_and_comments() {
        let text = "! meantone.scl\n!\nA test scale\n 4\n!\n 386.31371 comment\n3/2\n 5/3\n2\n";
        let tuning = Tuning::from_scala(text).unwrap();
        assert_eq!(tuning.name(), "A test scale");
        assert_eq!(tuning.steps_per_period(), 4);
        assert!(close(tuning.degrees()[0], 386.31371, 1e-9));
        assert!(close(tuning.degrees()[1], ratio_cents(3.0, 2.0), 1e-9));
        assert!(close(tuning.period_cents(), 1200.0, 1e-9));
        assert!(close(tuning.hz(60), MIDDLE_C_HZ, 1e-9));
        assert!(close(tuning.hz(62) / tuning.hz(60), 1.5, 1e-12));
    }

    #[test]
    fn a_scala_file_that_is_short_or_garbled_names_the_line() {
        assert!(matches!(
            Tuning::from_scala("name\n3\n100.0\n"),
            Err(TuningError::Scala { line: 3, .. })
        ));
        assert!(matches!(
            Tuning::from_scala("name\n2\n100.0\nfoo\n"),
            Err(TuningError::Scala { line: 4, .. })
        ));
        assert!(matches!(
            Tuning::from_scala("name\n1\n0/1\n"),
            Err(TuningError::Scala { line: 3, .. })
        ));
    }

    #[test]
    fn a_tuning_survives_being_written_as_scala_and_read_back() {
        let tuning = Tuning::just(PitchClass::new(9));
        let read = Tuning::from_scala(&tuning.to_scala())
            .unwrap()
            .with_keyboard_map(&tuning.to_keyboard_map())
            .unwrap();
        assert_eq!(read.name(), tuning.name());
        assert_eq!((read.root_key(), read.reference_key()), (69, 69));
        // Six decimals of cents is a few parts in ten billion: far below hearing, and above the
        // last bit of a high key's frequency, so the comparison is in cents.
        for key in 0..KEY_COUNT as u8 {
            let apart = ratio_cents(read.hz(key), tuning.hz(key));
            assert!(apart.abs() < 1e-5, "key {key}: {apart} cents");
        }
    }

    #[test]
    fn a_keyboard_map_that_skips_degrees_is_refused() {
        let tuning = Tuning::edo(5).unwrap();
        let skipping = "5\n0\n127\n60\n69\n440.0\n5\n0\n2\n1\n3\n4\n";
        assert_eq!(
            tuning.with_keyboard_map(skipping),
            Err(TuningError::NonLinearMapping)
        );
        let identity = "5\n0\n127\n60\n69\n440.0\n5\n0\n1\n2\n3\n4\n";
        let mapped = tuning.with_keyboard_map(identity).unwrap();
        assert!(close(mapped.hz(69), 440.0, 1e-9));
    }

    #[test]
    fn a_stored_tuning_is_checked_when_it_is_read() {
        let tuning = Tuning::edo(19).unwrap();
        let json = serde_json::to_string(&tuning).unwrap();
        assert_eq!(serde_json::from_str::<Tuning>(&json).unwrap(), tuning);
        let broken = json.replace("261.6", "-261.6");
        assert!(serde_json::from_str::<Tuning>(&broken).is_err());
    }

    #[test]
    fn a_document_writes_its_tuning_only_when_it_has_chosen_one() {
        let mut project = crate::Project::new("Tuned", 48_000.0);
        let plain = serde_json::to_value(&project).unwrap();
        assert!(
            plain.get("tuning").is_none(),
            "the standard tuning is not written"
        );
        assert!(
            serde_json::from_value::<crate::Project>(plain)
                .unwrap()
                .tuning
                .is_standard()
        );

        project.tuning = Tuning::edo(22).unwrap();
        let tuned = serde_json::to_value(&project).unwrap();
        assert!(tuned.get("tuning").is_some());
        assert_eq!(
            serde_json::from_value::<crate::Project>(tuned).unwrap(),
            project
        );
    }

    #[test]
    fn presets_read_back_what_they_write() {
        let presets = [
            TuningPreset::Standard,
            TuningPreset::Edo(31),
            TuningPreset::Just(PitchClass::new(2)),
            TuningPreset::Meantone(PitchClass::new(3)),
            TuningPreset::Pythagorean(PitchClass::new(0)),
            TuningPreset::Werckmeister,
            TuningPreset::BohlenPierce,
        ];
        for preset in presets {
            assert_eq!(preset.to_string().parse::<TuningPreset>(), Ok(preset));
            preset.tuning().unwrap();
        }
        assert_eq!("12-edo".parse(), Ok(TuningPreset::Standard));
        assert_eq!("19edo".parse(), Ok(TuningPreset::Edo(19)));
        assert_eq!(
            "just:eb".parse(),
            Ok(TuningPreset::Just(PitchClass::new(3)))
        );
        assert!("0-edo".parse::<TuningPreset>().is_err());
        assert!("just:h".parse::<TuningPreset>().is_err());
    }
}
