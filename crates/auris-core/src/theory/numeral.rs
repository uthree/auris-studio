//! Roman numerals: a chord named by its position in a key rather than by its letter.
//!
//! This is what lets a progression be written once and played in any key, and it is the notation
//! the whole catalogue is stored in.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::chord::{Chord, Quality};
use super::key::Key;
use super::pitch::{OCTAVE, PitchClass};
use super::scale::ScaleId;

/// A chord named by scale degree.
///
/// Serialises as the text a musician writes — `"IVmaj7"` — like [`Key`] does, so a progression
/// reads the same in a project file as it does on a chord chart. What is written is
/// [`Self::to_text`] rather than [`Display`](fmt::Display); the two differ in one place, and the
/// difference matters (see [`Self::to_text`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Numeral {
    /// One-based scale degree, as written: `1` for I, `5` for V.
    pub degree: u8,
    /// Semitone alteration of the degree, from a leading `b` or `#`.
    pub accidental: i32,
    /// `true` when the numeral was written in lower case.
    pub minor_case: bool,
    /// The quality written after the numeral, if any.
    ///
    /// `None` means "whatever the key makes it", which is what keeps a catalogue entry usable in
    /// major and minor alike.
    pub quality: Option<Quality>,
    /// A bare arabic extension, as in the `7` of `V7`.
    ///
    /// Held separately from `quality` because it only says *add a seventh*: which seventh, and
    /// what triad it sits on, is the key's business. Collapsing it at parse time turned `vii7`
    /// into a minor seventh and quietly threw away the diminished fifth.
    pub extension: Option<u8>,
    /// The degree this chord is applied to, and that degree's accidental, from a `/V` suffix.
    ///
    /// The rest of the numeral is then read in the key that degree would be the tonic of, which is
    /// what makes `ii/V` the supertonic of the target rather than another spelling of `V7/V`.
    pub secondary_of: Option<(u8, i32)>,
    /// A bass degree from a `/3`-style suffix, one-based, and that degree's accidental.
    ///
    /// The accidental is carried for the same reason `secondary_of` carries one: a bass note is
    /// not always a note the key holds. `iii/5` quoted into A harmonic minor is E minor over G,
    /// and that G is the key's seventh *lowered* — writing it as a plain seventh would put a G
    /// sharp under the chord, a semitone from its own third.
    pub bass_degree: Option<(u8, i32)>,
}

impl Numeral {
    /// A plain numeral on `degree`.
    pub fn new(degree: u8, minor_case: bool) -> Self {
        Self {
            degree,
            accidental: 0,
            minor_case,
            quality: None,
            extension: None,
            secondary_of: None,
            bass_degree: None,
        }
    }

    /// `true` when no quality was written, so colouring is free to add one.
    ///
    /// A chord the user spelled out is never rewritten; that is what keeps a quoted progression
    /// sounding like the song it came from.
    pub fn is_colourable(self) -> bool {
        self.quality.is_none() && self.extension.is_none() && self.secondary_of.is_none()
    }

    /// The same numeral with `quality` written on it.
    pub fn with_quality(self, quality: Quality) -> Self {
        Self {
            quality: Some(quality),
            ..self
        }
    }

    /// The same numeral, cased as `key` itself builds that degree.
    ///
    /// [`Self::chord_in`] lets the case override the key's own triad, because writing `IV` in a
    /// minor key is how a borrowed major subdominant is *asked for*. That override is exactly
    /// wrong for a caller doing the borrowing rather than writing one down: carrying C major's
    /// `vi` over to C minor is a request for the sixth degree **of C minor**, which is A♭ major,
    /// and the override answered A♭ minor — a chord neither mode builds there.
    ///
    /// A degree with an accidental on it is returned untouched. `bVII` names a chromatic root the
    /// key has no triad for, so its case is the only thing saying what quality was meant.
    pub fn as_diatonic(self, key: Key) -> Self {
        if self.accidental != 0 {
            return self;
        }
        Self {
            minor_case: diatonic_quality(key, self.degree).is_minor(),
            ..self
        }
    }

    /// How the numeral is written down for storage.
    ///
    /// This is [`Display`](fmt::Display) except in one place, and the exception is the whole
    /// reason the method exists. [`Quality::Major`]'s suffix is the empty string, so a numeral
    /// someone spelled out as `IM` prints as `I` — and `I` parses back with no quality at all,
    /// which [`Self::is_colourable`] then reports as free for the composer to rewrite. A chord a
    /// person chose would quietly become a chord the machine may change, on the next save.
    ///
    /// Storage therefore writes the `M`. `Display` does not, because `IM` is not how a chord
    /// chart is read.
    pub fn to_text(self) -> String {
        let mut text = String::new();
        // Writing into a `String` cannot fail, so the error is unreachable rather than ignored.
        let _ = self.write_symbol(&mut text, Quality::numeral_suffix);
        text
    }

    /// Writes the numeral, spelling each quality with `suffix`.
    fn write_symbol<W: fmt::Write>(
        &self,
        out: &mut W,
        suffix: fn(Quality) -> &'static str,
    ) -> fmt::Result {
        for _ in 0..self.accidental.abs() {
            out.write_str(if self.accidental < 0 { "b" } else { "#" })?;
        }
        let roman = ROMAN[(self.degree.clamp(1, 7) - 1) as usize];
        if self.minor_case {
            write!(out, "{}", roman.to_lowercase())?;
        } else {
            out.write_str(roman)?;
        }
        if let Some(extension) = self.extension {
            write!(out, "{extension}")?;
        } else if let Some(quality) = self.quality {
            out.write_str(suffix(quality))?;
        }
        if let Some((target, alteration)) = self.secondary_of {
            out.write_str("/")?;
            for _ in 0..alteration.abs() {
                out.write_str(if alteration < 0 { "b" } else { "#" })?;
            }
            out.write_str(ROMAN[(target.clamp(1, 7) - 1) as usize])?;
        }
        if let Some((bass, alteration)) = self.bass_degree {
            out.write_str("/")?;
            for _ in 0..alteration.abs() {
                out.write_str(if alteration < 0 { "b" } else { "#" })?;
            }
            write!(out, "{bass}")?;
        }
        Ok(())
    }

    /// Reads a numeral: `I`, `vi`, `bVII`, `V7`, `IVmaj7`, `V7/V`, `ii/V`, `IV/5`, `v/b7`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        // A trailing `/x` is either a secondary target (a numeral) or a bass degree (a digit).
        // Either kind may be altered — `V/bVI` is the dominant of the flat sixth, `v/b7` sits on
        // the flat seventh — so the accidentals come off first and what is left of the tail is
        // what says which of the two was written.
        let (head, tail) = match text.rsplit_once('/') {
            Some((head, tail)) => (head, Some(tail)),
            None => (text, None),
        };
        let (secondary_of, bass_degree) = match tail {
            None => (None, None),
            Some(tail) => {
                let mut target = tail.trim();
                let mut alteration = 0;
                while let Some(rest) = target.strip_prefix('b') {
                    alteration -= 1;
                    target = rest;
                }
                while let Some(rest) = target.strip_prefix('#') {
                    alteration += 1;
                    target = rest;
                }
                if target.chars().all(|c| c.is_ascii_digit()) {
                    // Only a degree the scale has: `degree_class` clamps into 1..=7, so `I/8`
                    // accepted here would come out as I over the *leading tone* — a stable wrong
                    // chord, since the symbol round-trips through save and load unchanged.
                    // Refused like a malformed roman target, and for the same reason.
                    let degree = target.parse::<u8>().ok().filter(|d| (1..=7).contains(d))?;
                    (None, Some((degree, alteration)))
                } else {
                    (Some((roman_degree(target)?, alteration)), None)
                }
            }
        };

        let mut rest = head;
        let mut accidental = 0;
        while let Some(stripped) = rest.strip_prefix('b') {
            accidental -= 1;
            rest = stripped;
        }
        while let Some(stripped) = rest.strip_prefix('#') {
            accidental += 1;
            rest = stripped;
        }

        // The numeral is the longest run of roman letters; whatever follows is the quality.
        let split = rest
            .find(|c: char| !matches!(c, 'i' | 'I' | 'v' | 'V'))
            .unwrap_or(rest.len());
        let (numeral, quality_text) = rest.split_at(split);
        let degree = roman_degree(numeral)?;
        let minor_case = numeral.chars().all(|c| c.is_lowercase());
        // A bare arabic number is an *extension*, resolved against the key later. Everything
        // else is a quality the user spelled out, and is taken at face value.
        let (quality, extension) = match quality_text {
            "" => (None, None),
            "6" | "7" | "9" => (None, Some(quality_text.parse::<u8>().ok()?)),
            text => (Some(Quality::parse(text)?), None),
        };

        Some(Self {
            degree,
            accidental,
            minor_case,
            quality,
            extension,
            secondary_of,
            bass_degree,
        })
    }

    /// The same chord, named from a different key.
    ///
    /// This is what lets a progression written in one mode be asked for in the other. The chords
    /// are what a quoted progression *is* — 丸サ進行 is the loop those four chords make, not the
    /// four degrees somebody wrote them as — so the chord is resolved in the key it was written
    /// for and then renamed against the key the piece is actually in.
    ///
    /// The shape as written is kept wherever the new key reads it the same way, because `V7` is
    /// worth more to a reader than a numeral that is merely correct. Where it does not, the
    /// quality is spelled out, which is the only way to name a chord a key would not have given.
    pub fn respelled_in(self, from: Key, to: Key) -> Self {
        let chord = self.chord_in(from);
        let (degree, accidental) = degree_of(to, chord.root);
        // The bass keeps its accidental too. The new key may not hold that note at all — the G
        // under `iii/5` is not a degree of A harmonic minor — and dropping the accidental would
        // resolve it back to the key's own seventh, a semitone above the note the chord was
        // written over.
        let bass_degree = chord.bass.map(|bass| degree_of(to, bass));
        // A secondary dominant is dropped rather than carried: `/V` names a degree of the key it
        // was written for, and the chord it stands for is already in hand.
        let as_written = Self {
            degree,
            accidental,
            bass_degree,
            secondary_of: None,
            ..self
        };
        if as_written.chord_in(to) == chord {
            return as_written;
        }
        Self {
            degree,
            accidental,
            minor_case: chord.quality.is_minor(),
            quality: Some(chord.quality),
            extension: None,
            secondary_of: None,
            bass_degree,
        }
    }

    /// The chord this numeral means in `key`.
    pub fn chord_in(self, key: Key) -> Chord {
        if let Some((target, alteration)) = self.secondary_of {
            // A chord applied to a degree is read in the key that degree would be the tonic of, so
            // the numeral in front of the slash means there what it would mean anywhere: `ii/V` is
            // that key's supertonic, `vii/V` its leading-tone chord. Major, because the slash
            // *tonicises* — `/vi` borrows A as a tonic, and reading it as A minor would put the
            // dominant of C major's sixth degree there instead of the dominant of A.
            let applied = Key::new(degree_class(key, target, alteration), ScaleId::Major);
            let head = Self {
                secondary_of: None,
                // A bare `V/x` is the applied *seventh*: the whole point of the notation is the
                // tritone that pulls into the target, and nobody writing `V/vi` means a plain
                // triad. Anything the writer spelled out is left as it was written.
                extension: match (self.degree, self.minor_case, self.quality, self.extension) {
                    (5, false, None, None) => Some(7),
                    _ => self.extension,
                },
                ..self
            };
            return head.chord_in(applied);
        }

        let root = degree_class(key, self.degree, self.accidental);
        let diatonic = diatonic_quality(key, self.degree);
        let triad = self.quality.unwrap_or_else(|| {
            // Case only speaks when it disagrees with the key: writing `IV` in a minor key is how
            // a borrowed major subdominant is asked for.
            if self.accidental != 0 || diatonic.is_minor() != self.minor_case {
                if self.minor_case {
                    Quality::Minor
                } else {
                    Quality::Major
                }
            } else {
                diatonic
            }
        });
        // Whether the triad above is the one the key builds on this degree, rather than one the
        // numeral's case asked for instead. It decides where the *seventh* may come from below.
        let borrowed = self.quality.is_some() || self.accidental != 0 || triad != diatonic;
        let quality = match self.extension {
            None => triad,
            // A sixth sits on a plain triad and on nothing else. `Major6` and `Minor6` both hold a
            // *perfect* fifth, so handing one to a diminished or augmented triad on the strength
            // of its third alone quietly straightened the fifth the numeral was written for:
            // `vii6` came out with a perfect fifth and was no longer diminished at all. There is
            // no name in this vocabulary for the chord that was asked for, and the triad as
            // written is a nearer answer than a different triad.
            Some(6) => match triad {
                Quality::Minor => Quality::Minor6,
                Quality::Major => Quality::Major6,
                other => other,
            },
            // An upper-case numeral with a bare seven is a dominant — `V7`, `I7`, `IV7` — which is
            // what makes the last chord of 丸サ進行 a secondary dominant rather than a tonic.
            // Everything else takes the seventh its own triad implies, so `vii7` keeps its
            // diminished fifth and comes out half-diminished.
            Some(extension) => {
                // An upper-case numeral on a major triad takes a *dominant* seventh, which is
                // the convention. A numeral playing the key's own triad takes the seventh the
                // key itself stacks, so the leading tone of a harmonic minor comes out fully
                // diminished rather than half — a distinction `with_seventh` cannot make from
                // the triad alone.
                //
                // Anything borrowed builds its seventh on the triad it actually has. Reaching
                // for the key's seventh there used to put back the third the case had just
                // taken out: `vi` in C minor is Abm and `vi7` came out **Abmaj7**, so adding a
                // seventh silently flipped the chord from minor to major. 丸サ進行 quoted in a
                // minor key was the audible version of that.
                let seventh = if !self.minor_case && triad == Quality::Major {
                    Quality::Dominant7
                } else if borrowed {
                    triad.with_seventh()
                } else {
                    diatonic_seventh(key, self.degree).unwrap_or_else(|| triad.with_seventh())
                };
                if extension >= 9 {
                    seventh.with_ninth()
                } else {
                    seventh
                }
            }
        };

        let chord = Chord::new(root, quality);
        match self.bass_degree {
            Some((bass, alteration)) => chord.over(degree_class(key, bass, alteration)),
            None => chord,
        }
    }

    /// The chord's name in `key`, with each root spelled by the degree it sits on.
    ///
    /// A root's letter follows from its degree and not from its pitch. The `vii` of A harmonic
    /// minor is G sharp because it is a raised seventh; the `bVII` of C major is B flat because it
    /// is a lowered one; and the `bII` of C major is D flat, which is what makes it a Neapolitan
    /// rather than a chord on the sharpened tonic. Every one of those is a black key that the
    /// pitch alone would name the other way half the time.
    ///
    /// Nothing holding only a [`Chord`] can know which, so this is where a chord gets its name
    /// whenever the numeral that produced it is still to hand — which, on the harmony timeline, is
    /// always. [`Chord::name_in`] is the fallback for when it is not.
    pub fn name_in(self, key: Key) -> String {
        let chord = self.chord_in(key);
        let degree = match self.secondary_of {
            // The applied chord's own degree, counted from the degree it was applied to: a
            // dominant stands four degrees above what it resolves to, a `ii/V` one degree above.
            Some((target, _)) => target.clamp(1, 7) + self.degree.clamp(1, 7) - 1,
            None => self.degree,
        };
        let Some(root) = spell_on_degree(key, degree, chord.root) else {
            return chord.name_in(key);
        };

        let mut name = format!("{root}{}", chord.quality.suffix());
        // The accidental is not consulted: `spell_on_degree` works out for itself what the letter
        // that degree demands needs in front of it to mean the note that is actually there.
        if let Some((bass, _)) = self.bass_degree {
            let class = chord.bass_class();
            let spelled = spell_on_degree(key, bass, class)
                .unwrap_or_else(|| class.name(key.spelling()).to_string());
            name.push('/');
            name.push_str(&spelled);
        }
        name
    }
}

/// The letters in scale order, with the semitone each one stands for unaltered.
const LETTERS: [(char, i32); 7] = [
    ('C', 0),
    ('D', 2),
    ('E', 4),
    ('F', 5),
    ('G', 7),
    ('A', 9),
    ('B', 11),
];

/// The name `class` has when it sits on `degree` of `key`.
///
/// The letter is whichever one the degree lands on, counting up from the tonic's own letter, and
/// the accidental is then whatever makes that letter mean `class`.
///
/// `None` when the letter that degree demands would need more than one accidental. A chord symbol
/// is not a notehead: nobody writes D double-flat over a bar line, and a numeral that asks for one
/// is not really describing that degree.
fn spell_on_degree(key: Key, degree: u8, class: PitchClass) -> Option<String> {
    let tonic = key.tonic.name(key.spelling()).chars().next()?;
    let from = LETTERS.iter().position(|(letter, _)| *letter == tonic)?;
    let (letter, natural) = LETTERS[(from + usize::from(degree.max(1)) - 1) % LETTERS.len()];

    let mut offset = (class.semitones() - natural).rem_euclid(OCTAVE);
    if offset > OCTAVE / 2 {
        offset -= OCTAVE;
    }
    match offset {
        0 => Some(letter.to_string()),
        1 => Some(format!("{letter}#")),
        -1 => Some(format!("{letter}b")),
        _ => None,
    }
}

impl fmt::Display for Numeral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_symbol(f, Quality::suffix)
    }
}

impl From<Numeral> for String {
    fn from(numeral: Numeral) -> Self {
        numeral.to_text()
    }
}

impl TryFrom<String> for Numeral {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Numeral::parse(&text).ok_or_else(|| format!("`{text}` is not a chord"))
    }
}

/// The roman numerals, in upper case, indexed by degree minus one.
const ROMAN: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

/// The one-based degree a roman numeral names.
fn roman_degree(text: &str) -> Option<u8> {
    Some(match text.to_ascii_uppercase().as_str() {
        "I" => 1,
        "II" => 2,
        "III" => 3,
        "IV" => 4,
        "V" => 5,
        "VI" => 6,
        "VII" => 7,
        _ => return None,
    })
}

/// The seven-note scale a numeral should be read against.
///
/// A pentatonic, blues or whole-tone key has no third and fifth to stack, so degree 5 would land
/// somewhere arbitrary and every chord in the piece would be wrong. Numerals are therefore read
/// against the parent scale of the same colour, which is the scale the harmony of such a piece is
/// drawn from anyway — the melody stays pentatonic, the chords do not have to be.
fn harmonic_key(key: Key) -> Key {
    use super::scale::ScaleId;
    let scale = match key.scale {
        ScaleId::MajorPentatonic => ScaleId::Major,
        ScaleId::MinorPentatonic | ScaleId::Blues => ScaleId::Minor,
        ScaleId::WholeTone => ScaleId::Lydian,
        other => other,
    };
    Key::new(key.tonic, scale)
}

/// The pitch class of a one-based degree in `key`, altered by `accidental` semitones.
///
/// An altered degree is measured from the **major** scale, which is the convention roman numeral
/// analysis uses: `bVI` means the same note in C major and in C minor, and in a minor key — where
/// the sixth is already flat — it must not be flattened twice.
fn degree_class(key: Key, degree: u8, accidental: i32) -> PitchClass {
    let key = harmonic_key(key);
    let index = i32::from(degree.clamp(1, 7)) - 1;
    if accidental == 0 {
        key.class(index)
    } else {
        let major = Key::new(key.tonic, super::scale::ScaleId::Major);
        major.class(index).transposed(accidental)
    }
}

/// The degree of `key` that names `class`, and the accidental it needs.
///
/// The inverse of the reading [`Numeral::chord_in`] does, and it has to agree with it exactly or
/// a numeral would not survive being renamed. A note the key has is named plainly; anything else
/// is named from the degree of the **major** scale nearest to it, which is the convention roman
/// numeral analysis uses and the one that is read back.
pub fn degree_of(key: Key, class: PitchClass) -> (u8, i32) {
    let key = harmonic_key(key);
    for degree in 1..=7u8 {
        if key.class(i32::from(degree) - 1) == class {
            return (degree, 0);
        }
    }
    let major = Key::new(key.tonic, super::scale::ScaleId::Major);
    // Flat before sharp, and one accidental before two: `bVI` is how everybody writes that note.
    for accidental in [0, -1, 1, -2, 2] {
        for degree in 1..=7u8 {
            if major.class(i32::from(degree) - 1).transposed(accidental) == class {
                return (degree, accidental);
            }
        }
    }
    (1, 0)
}

/// The seventh chord the key itself builds on `degree`, by stacking four scale thirds.
///
/// Returns `None` when the stack is not a chord this crate can name, which a mode with an
/// unusual interval can produce.
pub fn diatonic_seventh(key: Key, degree: u8) -> Option<Quality> {
    let key = harmonic_key(key);
    let base = i32::from(degree.clamp(1, 7)) - 1;
    let root = key.semitone(base);
    let intervals = [
        0,
        key.semitone(base + 2) - root,
        key.semitone(base + 4) - root,
        key.semitone(base + 6) - root,
    ];
    Quality::ALL
        .into_iter()
        .find(|quality| quality.intervals() == intervals)
}

/// The triad the key itself builds on `degree`, by stacking scale thirds.
///
/// Derived rather than tabulated so that harmonic minor gets its major dominant and its
/// diminished seventh degree without a special case, and so a mode gets whatever it gets.
pub fn diatonic_quality(key: Key, degree: u8) -> Quality {
    let key = harmonic_key(key);
    let base = i32::from(degree.clamp(1, 7)) - 1;
    let root = key.semitone(base);
    let third = key.semitone(base + 2) - root;
    let fifth = key.semitone(base + 4) - root;
    match (third, fifth) {
        (4, 7) => Quality::Major,
        (3, 7) => Quality::Minor,
        (3, 6) => Quality::Diminished,
        (4, 8) => Quality::Augmented,
        (2, 7) => Quality::Sus2,
        (5, 7) => Quality::Sus4,
        // Defensive fallback for a future scale that `harmonic_key` does not normalize.
        (third, _) if third <= 3 => Quality::Minor,
        _ => Quality::Major,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    fn chord_of(numeral: &str, key_text: &str) -> String {
        Numeral::parse(numeral).unwrap().name_in(key(key_text))
    }

    #[test]
    fn the_same_black_key_is_named_by_the_degree_it_stands_on() {
        // These three are the same key of a piano, and all three names are right in their place.
        // Nothing that sees only the pitch can tell them apart, which is why the numeral names the
        // chord and `Chord` does not.
        assert_eq!(chord_of("bVI", "C major"), "Ab", "a lowered sixth");
        assert_eq!(chord_of("#V", "C major"), "G#", "a raised fifth");
        assert_eq!(
            chord_of("vii", "A harmonic-minor"),
            "G#dim",
            "a leading tone"
        );

        // And the pair that started this: the flat seven of C is a B flat, not an A sharp.
        assert_eq!(chord_of("bVII", "C major"), "Bb");
        assert_eq!(chord_of("#VI", "C major"), "A#");
    }

    #[test]
    fn a_chord_is_spelled_for_the_key_it_is_in() {
        // The same numeral in two keys, one of each side of the circle of fifths.
        assert_eq!(chord_of("I", "Eb major"), "Eb");
        assert_eq!(chord_of("IV", "Eb major"), "Ab");
        assert_eq!(chord_of("V", "Eb major"), "Bb");
        assert_eq!(chord_of("vi", "Eb major"), "Cm");

        assert_eq!(chord_of("I", "E major"), "E");
        assert_eq!(chord_of("IV", "E major"), "A");
        assert_eq!(chord_of("ii", "E major"), "F#m");
        assert_eq!(chord_of("iii", "E major"), "G#m");

        // A slash chord spells its bass the same way. The number after the slash is a degree of
        // the key, not of the chord, so `/5` is the key's fifth wherever the chord sits.
        assert_eq!(chord_of("I/3", "Eb major"), "Eb/G");
        assert_eq!(chord_of("IV/5", "Eb major"), "Ab/Bb");
    }

    #[test]
    fn a_numeral_survives_the_round_trip_to_stored_text_and_back() {
        // Every quality on every degree, in both cases, with and without an accidental: whatever
        // the composer or a chart can produce has to come back out of a file unchanged.
        for degree in 1..=7u8 {
            for minor_case in [false, true] {
                for accidental in [-1, 0, 1] {
                    let plain = Numeral {
                        accidental,
                        ..Numeral::new(degree, minor_case)
                    };
                    for quality in Quality::ALL {
                        let numeral = plain.with_quality(quality);
                        let text = numeral.to_text();
                        assert_eq!(
                            Numeral::parse(&text),
                            Some(numeral),
                            "`{text}` did not read back as what wrote it"
                        );
                    }
                    for extension in [6u8, 7, 9] {
                        let numeral = Numeral {
                            extension: Some(extension),
                            ..plain
                        };
                        let text = numeral.to_text();
                        assert_eq!(Numeral::parse(&text), Some(numeral), "`{text}`");
                    }
                    assert_eq!(Numeral::parse(&plain.to_text()), Some(plain));
                }
            }
        }
    }

    #[test]
    fn a_forced_quality_is_never_stored_as_something_the_key_would_decide() {
        // `V7` on a chart means "with a seventh, whichever the key holds": an extension. A quality
        // forced to `Dominant7` is a different chord on `ii`, and writing both as `7` would let a
        // save turn one into the other. Whatever a quality is stored as, it must not read back as
        // an extension.
        for quality in Quality::ALL {
            let stored = quality.numeral_suffix();
            assert!(
                !matches!(stored, "6" | "7" | "9"),
                "{quality:?} stores as `{stored}`, which reads back as an extension"
            );
            assert_eq!(
                Quality::parse(stored),
                Some(quality),
                "`{stored}` is not a spelling of {quality:?}"
            );
        }

        let forced = Numeral::new(2, true).with_quality(Quality::Dominant7);
        let asked = Numeral {
            extension: Some(7),
            ..Numeral::new(2, true)
        };
        assert_eq!(forced.to_string(), asked.to_string(), "they read alike");
        assert_ne!(
            forced.to_text(),
            asked.to_text(),
            "they must not store alike"
        );
        assert_eq!(
            forced.chord_in(key("C major")).to_string(),
            "D7",
            "a forced dominant on the second degree"
        );
        assert_eq!(
            asked.chord_in(key("C major")).to_string(),
            "Dm7",
            "the seventh the key actually holds there"
        );
    }

    #[test]
    fn a_chord_spelled_out_as_major_does_not_become_rewritable_by_being_saved() {
        // `Quality::Major`'s suffix is empty, so `Display` writes `IM` as `I`, and `I` parses with
        // no quality at all — which `is_colourable` reports as fair game for the composer. Storing
        // the `Display` form would hand a chord the user chose to the machine on the next save.
        let spelled_out = Numeral::parse("IM").unwrap();
        assert!(!spelled_out.is_colourable());
        assert_eq!(
            spelled_out.to_string(),
            "I",
            "reading form stays conventional"
        );
        assert_eq!(
            spelled_out.to_text(),
            "IM",
            "storage form keeps the quality"
        );

        let reloaded = Numeral::parse(&spelled_out.to_text()).unwrap();
        assert_eq!(reloaded, spelled_out);
        assert!(
            !reloaded.is_colourable(),
            "a saved choice is still a choice"
        );

        // The bare numeral is genuinely unspecified, and must stay that way.
        let bare = Numeral::parse("I").unwrap();
        assert!(bare.is_colourable());
        assert_eq!(bare.to_text(), "I");
        assert_ne!(bare, spelled_out);
    }

    #[test]
    fn every_progression_in_the_catalogue_round_trips_through_json() {
        for entry in super::super::chart::CATALOG {
            let chart = super::super::chart::Chart::parse(entry.chart).unwrap();
            for bar in &chart.bars {
                for numeral in bar {
                    let json = serde_json::to_string(numeral).unwrap();
                    assert_eq!(
                        serde_json::from_str::<Numeral>(&json).unwrap(),
                        *numeral,
                        "{} in `{}`",
                        json,
                        entry.name
                    );
                }
            }
        }
        assert!(serde_json::from_str::<Numeral>("\"H7\"").is_err());
    }

    #[test]
    fn the_diatonic_triads_of_a_major_key_are_the_ones_everybody_learns() {
        // I ii iii IV V vi vii°
        let expected = ["C", "Dm", "Em", "F", "G", "Am", "Bdim"];
        let numerals = ["I", "ii", "iii", "IV", "V", "vi", "vii"];
        for (numeral, chord) in numerals.iter().zip(expected) {
            assert_eq!(chord_of(numeral, "C major"), chord, "{numeral}");
        }
    }

    #[test]
    fn the_diatonic_triads_of_a_minor_key_are_the_ones_everybody_learns() {
        // i ii° III iv v VI VII
        let expected = ["Am", "Bdim", "C", "Dm", "Em", "F", "G"];
        let numerals = ["i", "ii", "III", "iv", "v", "VI", "VII"];
        for (numeral, chord) in numerals.iter().zip(expected) {
            assert_eq!(chord_of(numeral, "A minor"), chord, "{numeral}");
        }
    }

    #[test]
    fn harmonic_minor_has_a_major_dominant_and_a_diminished_seventh() {
        assert_eq!(chord_of("V", "A harmonic-minor"), "E");
        assert_eq!(chord_of("vii", "A harmonic-minor"), "G#dim");
        assert_eq!(chord_of("i", "A harmonic-minor"), "Am");
    }

    #[test]
    fn case_asks_for_a_chord_the_key_does_not_have() {
        // The borrowed major IV in a minor key, and the borrowed minor iv in a major key.
        assert_eq!(chord_of("IV", "A minor"), "D");
        assert_eq!(chord_of("iv", "C major"), "Fm");
        // A written quality always wins over both.
        assert_eq!(chord_of("IVmaj7", "C major"), "Fmaj7");
        assert_eq!(
            chord_of("ii7", "C major"),
            "Dm7",
            "a lower-case seven is a minor seventh"
        );
    }

    #[test]
    fn accidentals_move_the_root_and_take_the_case_at_face_value() {
        assert_eq!(chord_of("bVII", "C major"), "Bb", "the flat seven");
        assert_eq!(chord_of("bVI", "C major"), "Ab");
        assert_eq!(chord_of("bII", "C major"), "Db", "the Neapolitan");
        assert_eq!(chord_of("#iv", "C major"), "F#m");
    }

    #[test]
    fn an_accidental_is_measured_from_the_major_scale() {
        // bVI, bIII and bVII name the same notes in a key and in its parallel: in a minor key
        // those degrees are already flat, and flattening them again would be a semitone out.
        assert_eq!(chord_of("bVI", "A minor"), "F");
        assert_eq!(chord_of("bIII", "A minor"), "C");
        assert_eq!(chord_of("bVII", "A minor"), "G");
        assert_eq!(chord_of("bVI", "A major"), "F");
        assert_eq!(chord_of("bIII", "A major"), "C");
        // An unaltered numeral still comes from the key's own scale.
        assert_eq!(chord_of("VI", "A minor"), "F");
        assert_eq!(chord_of("vi", "A major"), "F#m");
    }

    #[test]
    fn a_borrowed_major_scale_degree_in_minor_needs_no_accidental() {
        let c_major = key("C major");
        assert_eq!(degree_of(key("C minor"), c_major.class(5)), (6, 0));
        assert_eq!(degree_of(key("C minor"), c_major.class(6)), (7, 0));
    }

    #[test]
    fn a_secondary_dominant_is_the_dominant_of_its_target() {
        // V/V in C is D7, the dominant of G.
        assert_eq!(chord_of("V/V", "C major"), "D7");
        assert_eq!(chord_of("V7/V", "C major"), "D7");
        assert_eq!(chord_of("V/vi", "C major"), "E7", "the dominant of A minor");
        assert_eq!(chord_of("V/IV", "C major"), "C7");
    }

    #[test]
    fn an_applied_chord_is_read_in_the_key_it_is_applied_to() {
        // The numeral in front of the slash used to be thrown away, so every one of these came
        // out D7 — a chord nobody wrote, arriving without a word.
        assert_eq!(chord_of("ii/V", "C major"), "Am", "the supertonic of G");
        assert_eq!(chord_of("ii7/V", "C major"), "Am7");
        assert_eq!(chord_of("vii/V", "C major"), "F#dim", "G's leading tone");
        assert_eq!(chord_of("IV/V", "C major"), "C");
        assert_eq!(chord_of("bVII/V", "C major"), "F");
        // And the whole ii-V of the dominant, which is the reason the notation is wanted.
        assert_eq!(chord_of("ii7/V", "C major"), "Am7");
        assert_eq!(chord_of("V7/V", "C major"), "D7");
    }

    #[test]
    fn a_sixth_never_straightens_the_fifth_under_it() {
        // A plain triad takes its sixth as written.
        assert_eq!(chord_of("I6", "C major"), "C6");
        assert_eq!(chord_of("vi6", "C major"), "Am6");
        // A diminished triad has no sixth in this vocabulary, and used to be given a *perfect*
        // fifth along with one: `vii6` came out Bm6 and was no longer diminished.
        assert_eq!(chord_of("vii6", "C major"), "Bdim");
    }

    #[test]
    fn an_applied_chord_is_named_from_its_own_degree() {
        let name = |text: &str| Numeral::parse(text).unwrap().name_in(key("C major"));
        assert_eq!(name("V7/V"), "D7");
        assert_eq!(name("ii/V"), "Am");
        assert_eq!(name("vii/V"), "F#dim");
    }

    #[test]
    fn malformed_secondary_degrees_do_not_overflow_when_named() {
        let mut numeral = Numeral::parse("V/V").unwrap();
        numeral.degree = 200;
        numeral.secondary_of = Some((200, 0));
        let _ = numeral.name_in(key("C major"));

        numeral.degree = 0;
        numeral.secondary_of = Some((0, 0));
        let _ = numeral.name_in(key("C major"));
    }

    #[test]
    fn a_slash_digit_is_a_bass_degree() {
        // The IV chord over the fifth of the key: the pedal of the 小悪魔 progression.
        let chord = Numeral::parse("IV/5").unwrap().chord_in(key("C major"));
        assert_eq!(chord.root.sharp_name(), "F");
        assert_eq!(chord.bass_class().sharp_name(), "G");
        assert_eq!(chord.to_string(), "F/G");
    }

    #[test]
    fn a_slash_bass_carries_its_own_accidental() {
        // A bass note the key does not hold needs an accidental of its own, and it has to survive
        // the trip through the stored text, because that text is the whole of what a project file
        // keeps of a chord.
        for (text, bass) in [("v/b7", (7, -1)), ("IV/#5", (5, 1)), ("I/3", (3, 0))] {
            let numeral = Numeral::parse(text).unwrap();
            assert_eq!(numeral.bass_degree, Some(bass), "{text}");
            assert_eq!(
                numeral.to_text(),
                text,
                "{text} did not write back as itself"
            );
            assert_eq!(Numeral::parse(&numeral.to_text()), Some(numeral), "{text}");
        }

        // And the accidental moves the note, by exactly the semitone it asks for.
        assert_eq!(chord_of("v/b7", "A harmonic-minor"), "Em/G");
        assert_eq!(chord_of("v/7", "A harmonic-minor"), "Em/G#");
        assert_eq!(chord_of("I/#5", "C major"), "C/G#");
    }

    #[test]
    fn a_quoted_progression_keeps_the_bass_it_was_written_over() {
        // 純情進行 is the canon laid over a bass that walks down by step, and that walk is what the
        // progression is. Quoted into A harmonic minor its fourth chord is E minor over G — a note
        // that key does not hold, its seventh being a G sharp. The bass degree used to be renamed
        // without its accidental, so the numeral that went into the document said "the seventh",
        // read back as G sharp, and sounded a minor second under the chord's own third.
        let minor = key("A harmonic-minor");
        let junjo = super::super::chart::Chart::parse("@junjo")
            .unwrap()
            .spelled_in(minor);
        let named: Vec<String> = junjo
            .bars
            .iter()
            .flatten()
            .map(|numeral| numeral.name_in(minor))
            .collect();
        assert_eq!(
            named,
            ["C", "G/B", "Am", "Em/G", "F", "C/E", "Dm", "G"],
            "the same eight chords, named for the key they were asked for in"
        );

        // Renaming is what the file records, so the symbol has to be right too.
        let stored: Vec<String> = junjo
            .bars
            .iter()
            .flatten()
            .map(|numeral| numeral.to_text())
            .collect();
        assert_eq!(stored[3], "v/b7", "the fourth chord, as it is written down");
    }

    #[test]
    fn numerals_round_trip_through_their_text() {
        for text in ["I", "vi", "bVII", "V7", "IVmaj7", "iim7b5", "#iv"] {
            let numeral = Numeral::parse(text).unwrap();
            assert_eq!(numeral.to_string(), text, "{text} did not round trip");
        }
    }

    #[test]
    fn nonsense_is_rejected() {
        assert!(Numeral::parse("").is_none());
        assert!(Numeral::parse("VIII").is_none());
        assert!(Numeral::parse("X").is_none());
        assert!(Numeral::parse("Iwhat").is_none());
        assert!(Numeral::parse("V/X").is_none());
    }

    #[test]
    fn a_bass_degree_the_scale_does_not_have_is_rejected() {
        // `degree_class` clamps into 1..=7, so `I/8` accepted at the parser would come out as
        // I over the leading tone — a wrong chord that round-trips stably through the file.
        assert!(Numeral::parse("I/0").is_none());
        assert!(Numeral::parse("I/8").is_none());
        assert!(Numeral::parse("I/9").is_none());
        assert!(Numeral::parse("IV/255").is_none());
        assert!(Numeral::parse("IV/7").is_some());
    }

    #[test]
    fn only_a_bare_numeral_may_be_coloured() {
        assert!(Numeral::parse("IV").unwrap().is_colourable());
        assert!(Numeral::parse("vi").unwrap().is_colourable());
        assert!(
            !Numeral::parse("IVmaj7").unwrap().is_colourable(),
            "a written quality is the user's decision"
        );
        assert!(!Numeral::parse("V7/V").unwrap().is_colourable());
    }

    #[test]
    fn a_bare_seven_takes_its_third_from_the_case() {
        // The convention every chart uses: V7 is a dominant, vi7 is a minor seventh, and a
        // major seventh has to say so.
        assert_eq!(chord_of("V7", "C major"), "G7");
        assert_eq!(chord_of("I7", "C major"), "C7");
        assert_eq!(chord_of("vi7", "C major"), "Am7");
        assert_eq!(chord_of("ii7", "C major"), "Dm7");
        assert_eq!(chord_of("iii7", "C major"), "Em7");
        assert_eq!(chord_of("Imaj7", "C major"), "Cmaj7");
        assert_eq!(chord_of("vi9", "C major"), "Am9");
        assert_eq!(chord_of("I6", "C major"), "C6");
        assert_eq!(chord_of("vi6", "C major"), "Am6");
    }

    #[test]
    fn a_bare_seven_keeps_the_fifth_the_key_gives_it() {
        // The bug this pins: `vii7` used to become a plain minor seventh, which threw away the
        // diminished fifth and imported an F# into C major.
        assert_eq!(chord_of("vii7", "C major"), "Bm7b5");
        assert_eq!(chord_of("ii7", "A minor"), "Bm7b5");
        assert_eq!(chord_of("vii7", "A harmonic-minor"), "G#dim7");
        // And the ordinary cases still read the way a chart writes them.
        assert_eq!(chord_of("V7", "C major"), "G7");
        assert_eq!(chord_of("I7", "C major"), "C7");
        assert_eq!(chord_of("vi7", "C major"), "Am7");
        assert_eq!(chord_of("IV7", "C major"), "F7");
    }

    #[test]
    fn adding_a_seventh_never_changes_the_triad_under_it() {
        // The bug this pins: the triad was chosen by case, and the seventh was then taken from
        // the key *regardless* — so on a degree where the two disagree, adding a seventh put the
        // third back that the case had just taken out. `vi` in C minor is Ab minor and `vi7`
        // came out Ab **major** seventh.
        //
        // 丸サ進行 quoted in a minor key is where it was audible: its `vi7` is the chord the whole
        // loop leans on, and it sounded a major third above a root the chart says is minor.
        assert_eq!(chord_of("vi", "C minor"), "Abm");
        assert_eq!(chord_of("vi7", "C minor"), "Abm7");
        assert_eq!(chord_of("iii7", "C minor"), "Ebm7");
        assert_eq!(chord_of("vii7", "A minor"), "Gm7");

        // Stated as the property, because a chord that changes quality when a seventh is added
        // is wrong wherever it happens and the catalogue is not the only thing that asks.
        for key_text in [
            "C major",
            "C minor",
            "C harmonic-minor",
            "C dorian",
            "C phrygian",
            "C lydian",
            "C mixolydian",
            "A minor",
            "F# minor",
            "Bb major",
        ] {
            for plain in [
                "I", "i", "II", "ii", "III", "iii", "IV", "iv", "V", "v", "VI", "vi", "VII", "vii",
            ] {
                let triad = Numeral::parse(plain).unwrap().chord_in(key(key_text));
                let seventh = Numeral::parse(&format!("{plain}7"))
                    .unwrap()
                    .chord_in(key(key_text));
                assert_eq!(
                    triad.quality.is_minor(),
                    seventh.quality.is_minor(),
                    "in {key_text}, `{plain}` is {triad} but `{plain}7` is {seventh}"
                );
                assert_eq!(triad.root, seventh.root, "in {key_text}, `{plain}`");
            }
        }
    }

    #[test]
    fn a_numeral_in_a_pentatonic_key_still_spells_a_chord() {
        // A five-note scale has no degree five to stack thirds on, so numerals are read against
        // the parent seven-note scale rather than landing somewhere arbitrary.
        assert_eq!(chord_of("I", "C major-pentatonic"), "C");
        assert_eq!(chord_of("V", "C major-pentatonic"), "G");
        assert_eq!(chord_of("vi", "C major-pentatonic"), "Am");
        assert_eq!(chord_of("i", "A minor-pentatonic"), "Am");
        assert_eq!(chord_of("v", "A minor-pentatonic"), "Em");
        assert_eq!(
            chord_of("V", "A minor-pentatonic"),
            "E",
            "upper case asks for the major"
        );
        assert_eq!(chord_of("i", "A blues"), "Am");
    }

    #[test]
    fn a_secondary_dominant_target_may_be_altered() {
        // V/bVI is the dominant of the flat sixth, not of the sixth.
        assert_eq!(chord_of("V/bVI", "C major"), "Eb7");
        assert_eq!(chord_of("V/VI", "C major"), "E7");
        assert_eq!(Numeral::parse("V/bVI").unwrap().to_string(), "V/bVI");
    }

    #[test]
    fn the_marusa_progression_spells_out_correctly() {
        // IVM7 III7 vi I7 in C: the Just-the-Two-of-Us / 丸サ progression.
        let chords: Vec<String> = ["IVmaj7", "III7", "vi7", "I7"]
            .iter()
            .map(|numeral| chord_of(numeral, "C major"))
            .collect();
        assert_eq!(chords, ["Fmaj7", "E7", "Am7", "C7"]);
    }
}
