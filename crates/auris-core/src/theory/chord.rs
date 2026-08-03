//! Chords: a root, a quality, and the notes that follow from them.

use std::fmt;

use super::pitch::{OCTAVE, PitchClass};

/// The shape of a chord above its root.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Quality {
    /// A major triad.
    Major,
    /// A minor triad.
    Minor,
    /// A diminished triad.
    Diminished,
    /// An augmented triad.
    Augmented,
    /// The third replaced by a second.
    Sus2,
    /// The third replaced by a fourth.
    Sus4,
    /// Major triad with a major seventh.
    Major7,
    /// Major triad with a minor seventh: the dominant.
    Dominant7,
    /// Minor triad with a minor seventh.
    Minor7,
    /// Diminished triad with a minor seventh: half-diminished.
    HalfDiminished7,
    /// Diminished triad with a diminished seventh.
    Diminished7,
    /// Minor triad with a major seventh.
    MinorMajor7,
    /// Major triad with an added sixth.
    Major6,
    /// Minor triad with an added sixth.
    Minor6,
    /// Major triad with an added ninth and no seventh.
    Add9,
    /// Dominant seventh with a ninth.
    Dominant9,
    /// Major seventh with a ninth.
    Major9,
    /// Minor seventh with a ninth.
    Minor9,
    /// Dominant seventh with a thirteenth.
    Dominant13,
    /// Suspended fourth with a minor seventh.
    Dominant7Sus4,
}

impl Quality {
    /// Every quality, for tests and pickers.
    pub const ALL: [Quality; 20] = [
        Quality::Major,
        Quality::Minor,
        Quality::Diminished,
        Quality::Augmented,
        Quality::Sus2,
        Quality::Sus4,
        Quality::Major7,
        Quality::Dominant7,
        Quality::Minor7,
        Quality::HalfDiminished7,
        Quality::Diminished7,
        Quality::MinorMajor7,
        Quality::Major6,
        Quality::Minor6,
        Quality::Add9,
        Quality::Dominant9,
        Quality::Major9,
        Quality::Minor9,
        Quality::Dominant13,
        Quality::Dominant7Sus4,
    ];

    /// Semitones above the root, ascending.
    pub fn intervals(self) -> &'static [i32] {
        match self {
            Quality::Major => &[0, 4, 7],
            Quality::Minor => &[0, 3, 7],
            Quality::Diminished => &[0, 3, 6],
            Quality::Augmented => &[0, 4, 8],
            Quality::Sus2 => &[0, 2, 7],
            Quality::Sus4 => &[0, 5, 7],
            Quality::Major7 => &[0, 4, 7, 11],
            Quality::Dominant7 => &[0, 4, 7, 10],
            Quality::Minor7 => &[0, 3, 7, 10],
            Quality::HalfDiminished7 => &[0, 3, 6, 10],
            Quality::Diminished7 => &[0, 3, 6, 9],
            Quality::MinorMajor7 => &[0, 3, 7, 11],
            Quality::Major6 => &[0, 4, 7, 9],
            Quality::Minor6 => &[0, 3, 7, 9],
            Quality::Add9 => &[0, 4, 7, 14],
            Quality::Dominant9 => &[0, 4, 7, 10, 14],
            Quality::Major9 => &[0, 4, 7, 11, 14],
            Quality::Minor9 => &[0, 3, 7, 10, 14],
            Quality::Dominant13 => &[0, 4, 7, 10, 21],
            Quality::Dominant7Sus4 => &[0, 5, 7, 10],
        }
    }

    /// The suffix written after the root.
    pub fn suffix(self) -> &'static str {
        match self {
            Quality::Major => "",
            Quality::Minor => "m",
            Quality::Diminished => "dim",
            Quality::Augmented => "aug",
            Quality::Sus2 => "sus2",
            Quality::Sus4 => "sus4",
            Quality::Major7 => "maj7",
            Quality::Dominant7 => "7",
            Quality::Minor7 => "m7",
            Quality::HalfDiminished7 => "m7b5",
            Quality::Diminished7 => "dim7",
            Quality::MinorMajor7 => "mMaj7",
            Quality::Major6 => "6",
            Quality::Minor6 => "m6",
            Quality::Add9 => "add9",
            Quality::Dominant9 => "9",
            Quality::Major9 => "maj9",
            Quality::Minor9 => "m9",
            Quality::Dominant13 => "13",
            Quality::Dominant7Sus4 => "7sus4",
        }
    }

    /// The suffix to write after a roman numeral, where the plain one would be misread.
    ///
    /// After a chord *letter*, `C7` can only be a dominant seventh. After a *numeral*, `V7` means
    /// "with a seventh, whichever one the key holds" — an extension, which the key resolves —
    /// and a quality that was forced to `Dominant7` has to be spelled out to survive being read
    /// back. The same goes for `6` and `9`, and for a plain major, whose suffix is empty and
    /// would vanish entirely.
    ///
    /// Every spelling here is one [`Self::parse`] already accepts; nothing new is invented.
    pub fn numeral_suffix(self) -> &'static str {
        match self {
            Quality::Major => "M",
            Quality::Major6 => "maj6",
            Quality::Dominant7 => "dom7",
            Quality::Dominant9 => "dom9",
            other => other.suffix(),
        }
    }

    /// Reads a quality suffix, accepting the spellings chord charts actually use.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "" | "M" | "maj" | "major" => Quality::Major,
            "m" | "min" | "-" | "minor" => Quality::Minor,
            "dim" | "o" | "°" => Quality::Diminished,
            "aug" | "+" => Quality::Augmented,
            "sus2" => Quality::Sus2,
            "sus" | "sus4" => Quality::Sus4,
            "maj7" | "M7" | "Maj7" | "Δ" | "Δ7" | "ma7" => Quality::Major7,
            "7" | "dom7" => Quality::Dominant7,
            "m7" | "min7" | "-7" => Quality::Minor7,
            "m7b5" | "min7b5" | "ø" | "ø7" | "halfdim" => Quality::HalfDiminished7,
            "dim7" | "o7" | "°7" => Quality::Diminished7,
            "mMaj7" | "mM7" | "minmaj7" => Quality::MinorMajor7,
            "6" | "maj6" | "M6" => Quality::Major6,
            "m6" | "min6" => Quality::Minor6,
            "add9" | "add2" => Quality::Add9,
            "9" | "dom9" => Quality::Dominant9,
            "maj9" | "M9" | "Δ9" => Quality::Major9,
            "m9" | "min9" | "-9" => Quality::Minor9,
            "13" | "dom13" => Quality::Dominant13,
            "7sus4" | "7sus" => Quality::Dominant7Sus4,
            _ => return None,
        })
    }

    /// `true` when the chord has a minor third.
    pub fn is_minor(self) -> bool {
        self.intervals().contains(&3)
    }

    /// `true` when the chord already has a seventh or a sixth of some kind.
    pub fn has_seventh(self) -> bool {
        self.intervals()
            .iter()
            .any(|interval| matches!(interval, 9..=11))
    }

    /// The same chord with a seventh added, or itself when that means nothing.
    ///
    /// Which seventh a chord takes is decided by the triad underneath: a major triad takes a
    /// major seventh, a minor triad a minor one. A chord that already has one is left alone,
    /// which is what keeps a dominant a dominant.
    pub fn with_seventh(self) -> Self {
        match self {
            Quality::Major => Quality::Major7,
            Quality::Minor => Quality::Minor7,
            Quality::Diminished => Quality::HalfDiminished7,
            Quality::Sus4 => Quality::Dominant7Sus4,
            Quality::Major6 => Quality::Major7,
            Quality::Minor6 => Quality::Minor7,
            other => other,
        }
    }

    /// The same chord with a ninth added, or itself when that means nothing.
    pub fn with_ninth(self) -> Self {
        match self {
            Quality::Major => Quality::Add9,
            Quality::Major7 => Quality::Major9,
            Quality::Dominant7 => Quality::Dominant9,
            Quality::Minor7 => Quality::Minor9,
            other => other,
        }
    }
}

/// A chord: a root, a quality, and possibly a bass note that is not the root.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    /// The note the chord is built on.
    pub root: PitchClass,
    /// Its shape above that root.
    pub quality: Quality,
    /// The note in the bass, when it is not the root — the `/E` of `C/E`.
    pub bass: Option<PitchClass>,
}

impl Chord {
    /// A chord in root position.
    pub fn new(root: PitchClass, quality: Quality) -> Self {
        Self {
            root,
            quality,
            bass: None,
        }
    }

    /// This chord over `bass`.
    pub fn over(self, bass: PitchClass) -> Self {
        Self {
            bass: Some(bass),
            ..self
        }
    }

    /// The note that actually sounds lowest: the slash bass when there is one.
    pub fn bass_class(self) -> PitchClass {
        self.bass.unwrap_or(self.root)
    }

    /// The chord's pitch classes, root first.
    pub fn classes(self) -> Vec<PitchClass> {
        self.quality
            .intervals()
            .iter()
            .map(|interval| self.root.transposed(*interval))
            .collect()
    }

    /// The chord voiced upward from `low`, one note per interval.
    ///
    /// The root lands on the first pitch at or above `low` and the rest follow at their written
    /// intervals, so a ninth really does sound an octave and a tone above the root rather than
    /// being folded into the triad.
    pub fn voiced_from(self, low: i32) -> Vec<i32> {
        let mut root = low - low.rem_euclid(OCTAVE) + self.root.semitones();
        if root < low {
            root += OCTAVE;
        }
        self.quality
            .intervals()
            .iter()
            .map(|interval| root + interval)
            .collect()
    }

    /// `true` when `pitch` is one of the chord's notes, in any octave.
    pub fn contains(self, pitch: PitchClass) -> bool {
        self.classes().contains(&pitch)
    }

    /// `true` when the MIDI pitch is one of the chord's notes.
    pub fn contains_midi(self, midi: i32) -> bool {
        self.contains(PitchClass::new(midi))
    }

    /// This chord moved by `semitones`.
    pub fn transposed(self, semitones: i32) -> Self {
        Self {
            root: self.root.transposed(semitones),
            quality: self.quality,
            bass: self.bass.map(|bass| bass.transposed(semitones)),
        }
    }

    /// The chord tone nearest to `midi`, preferring the lower one on a tie.
    pub fn nearest_tone(self, midi: i32) -> i32 {
        let mut best = midi;
        let mut best_distance = i32::MAX;
        for class in self.classes() {
            // Searching the octave either side is always enough to find the nearest.
            let base = midi - midi.rem_euclid(OCTAVE) + class.semitones();
            for candidate in [base - OCTAVE, base, base + OCTAVE] {
                let distance = (candidate - midi).abs();
                if distance < best_distance || (distance == best_distance && candidate < best) {
                    best_distance = distance;
                    best = candidate;
                }
            }
        }
        best
    }

    /// Reads a chord symbol such as `C`, `Am7`, `F#m7b5` or `G7/B`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (body, bass) = match text.split_once('/') {
            Some((body, bass)) => (body, Some(PitchClass::parse(bass.trim())?)),
            None => (text, None),
        };
        if body.is_empty() {
            return None;
        }

        // The root is the letter and every accidental after it; whatever is left is the quality.
        let mut split = body
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(body.len());
        for (index, character) in body.char_indices().skip(1) {
            if matches!(character, '#' | 'b' | '♯' | '♭') {
                split = index + character.len_utf8();
            } else {
                break;
            }
        }
        let (root_text, quality_text) = body.split_at(split.min(body.len()));
        Some(Self {
            root: PitchClass::parse(root_text)?,
            quality: Quality::parse(quality_text)?,
            bass,
        })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.root.sharp_name(), self.quality.suffix())?;
        if let Some(bass) = self.bass {
            write!(f, "/{}", bass.sharp_name())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> PitchClass {
        PitchClass::parse(name).unwrap()
    }

    fn names(classes: &[PitchClass]) -> Vec<&'static str> {
        classes.iter().map(|class| class.sharp_name()).collect()
    }

    #[test]
    fn triads_and_sevenths_spell_the_notes_they_should() {
        assert_eq!(
            names(&Chord::parse("C").unwrap().classes()),
            ["C", "E", "G"]
        );
        assert_eq!(
            names(&Chord::parse("Am").unwrap().classes()),
            ["A", "C", "E"]
        );
        assert_eq!(
            names(&Chord::parse("G7").unwrap().classes()),
            ["G", "B", "D", "F"]
        );
        assert_eq!(
            names(&Chord::parse("Cmaj7").unwrap().classes()),
            ["C", "E", "G", "B"]
        );
        assert_eq!(
            names(&Chord::parse("Bm7b5").unwrap().classes()),
            ["B", "D", "F", "A"],
            "the half-diminished chord of a major key"
        );
        assert_eq!(
            names(&Chord::parse("Bdim7").unwrap().classes()),
            ["B", "D", "F", "G#"]
        );
    }

    #[test]
    fn chord_symbols_parse_the_way_they_are_written() {
        assert_eq!(Chord::parse("C").unwrap().quality, Quality::Major);
        assert_eq!(Chord::parse("Cm").unwrap().quality, Quality::Minor);
        assert_eq!(Chord::parse("F#m7").unwrap().root, class("F#"));
        assert_eq!(Chord::parse("Bbmaj7").unwrap().root, class("Bb"));
        assert_eq!(Chord::parse("Ebm9").unwrap().quality, Quality::Minor9);
        assert_eq!(
            Chord::parse("G7sus4").unwrap().quality,
            Quality::Dominant7Sus4
        );
        assert_eq!(Chord::parse("Am/C").unwrap().bass, Some(class("C")));
        assert_eq!(Chord::parse("C/G").unwrap().bass_class(), class("G"));
        assert_eq!(
            Chord::parse("C").unwrap().bass_class(),
            class("C"),
            "no slash means the root sounds lowest"
        );
        assert!(Chord::parse("Xm7").is_none());
        assert!(Chord::parse("Cwhat").is_none());
        assert!(Chord::parse("").is_none());
    }

    #[test]
    fn every_quality_round_trips_and_ascends() {
        for quality in Quality::ALL {
            assert_eq!(
                Quality::parse(quality.suffix()),
                Some(quality),
                "{quality:?}"
            );
            let chord = Chord::new(class("C"), quality);
            assert_eq!(Chord::parse(&chord.to_string()), Some(chord), "{chord}");

            let intervals = quality.intervals();
            assert_eq!(intervals[0], 0, "{quality:?} does not start on its root");
            assert!(
                intervals.windows(2).all(|pair| pair[0] < pair[1]),
                "{quality:?} is not ascending"
            );
        }
    }

    #[test]
    fn voicing_upward_puts_the_root_at_or_above_the_floor() {
        let c = Chord::parse("C").unwrap();
        assert_eq!(c.voiced_from(60), vec![60, 64, 67]);
        assert_eq!(c.voiced_from(61), vec![72, 76, 79], "the next C above");
        assert_eq!(c.voiced_from(48), vec![48, 52, 55]);

        // A ninth really is an octave and a tone up, not folded into the triad.
        assert_eq!(
            Chord::parse("C9").unwrap().voiced_from(48),
            vec![48, 52, 55, 58, 62]
        );
    }

    #[test]
    fn the_nearest_chord_tone_is_the_nearest_one() {
        let c = Chord::parse("C").unwrap();
        assert_eq!(c.nearest_tone(60), 60, "already a chord tone");
        assert_eq!(c.nearest_tone(61), 60, "C# falls to C");
        assert_eq!(
            c.nearest_tone(62),
            60,
            "D is a tone from both, and ties low"
        );
        assert_eq!(c.nearest_tone(63), 64, "D# rises to E");
        assert_eq!(c.nearest_tone(66), 67, "F# rises to G");
    }

    #[test]
    fn adding_a_seventh_follows_the_triad_underneath() {
        assert_eq!(Quality::Major.with_seventh(), Quality::Major7);
        assert_eq!(Quality::Minor.with_seventh(), Quality::Minor7);
        assert_eq!(Quality::Diminished.with_seventh(), Quality::HalfDiminished7);
        assert_eq!(
            Quality::Dominant7.with_seventh(),
            Quality::Dominant7,
            "a chord that already has one is left alone"
        );
        assert!(Quality::Major7.has_seventh());
        assert!(!Quality::Major.has_seventh());
        assert!(Quality::Diminished7.has_seventh());
    }

    #[test]
    fn adding_a_ninth_keeps_the_seventh_that_was_there() {
        assert_eq!(Quality::Major7.with_ninth(), Quality::Major9);
        assert_eq!(Quality::Dominant7.with_ninth(), Quality::Dominant9);
        assert_eq!(Quality::Minor7.with_ninth(), Quality::Minor9);
        assert_eq!(Quality::Major.with_ninth(), Quality::Add9);
        assert_eq!(
            Quality::Diminished7.with_ninth(),
            Quality::Diminished7,
            "nothing sensible to add"
        );
    }

    #[test]
    fn transposing_moves_the_bass_with_the_root() {
        let up = Chord::parse("Am/C").unwrap().transposed(2);
        assert_eq!(up.root, class("B"));
        assert_eq!(up.bass, Some(class("D")));
        assert_eq!(up.quality, Quality::Minor);
    }

    #[test]
    fn membership_ignores_the_octave() {
        let g7 = Chord::parse("G7").unwrap();
        assert!(g7.contains(class("F")));
        assert!(g7.contains_midi(53), "F3");
        assert!(g7.contains_midi(77), "F5");
        assert!(!g7.contains_midi(60), "C is not in G7");
    }
}
