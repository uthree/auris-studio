//! Scales, as interval patterns above a tonic.
//!
//! A scale here is a list of semitone offsets, not a list of notes: the tonic arrives separately
//! so the same table serves every key. Degrees are zero-based inside the crate — degree 0 is the
//! tonic — and only the text format writes them one-based, because that is how people speak.

use serde::{Deserialize, Serialize};

use super::pitch::{OCTAVE, PitchClass};

/// A scale the composer knows.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScaleId {
    /// The major scale, and the Ionian mode.
    Major,
    /// Minor with a flat sixth and seventh: the natural minor, and the Aeolian mode.
    Minor,
    /// Minor with a raised seventh, which is what gives a minor key its dominant chord.
    HarmonicMinor,
    /// Minor with a raised sixth and seventh, for lines that ascend.
    MelodicMinor,
    /// Minor with a raised sixth: brighter than natural minor without leaving it.
    Dorian,
    /// Minor with a flat second, the darkest of the common modes.
    Phrygian,
    /// Major with a raised fourth.
    Lydian,
    /// Major with a flat seventh, which is most of what "bluesy major" means.
    Mixolydian,
    /// The mode on the seventh degree, with a diminished tonic triad.
    Locrian,
    /// Major without its fourth and seventh: five notes, no semitone clashes.
    MajorPentatonic,
    /// Minor without its second and sixth.
    MinorPentatonic,
    /// Minor pentatonic plus the flat fifth.
    Blues,
    /// Six notes a whole tone apart, with no tonic gravity at all.
    WholeTone,
}

impl ScaleId {
    /// Every scale, in the order a picker should list them.
    pub const ALL: [ScaleId; 13] = [
        ScaleId::Major,
        ScaleId::Minor,
        ScaleId::HarmonicMinor,
        ScaleId::MelodicMinor,
        ScaleId::Dorian,
        ScaleId::Phrygian,
        ScaleId::Lydian,
        ScaleId::Mixolydian,
        ScaleId::Locrian,
        ScaleId::MajorPentatonic,
        ScaleId::MinorPentatonic,
        ScaleId::Blues,
        ScaleId::WholeTone,
    ];

    /// Semitones above the tonic, ascending, one per degree.
    pub fn steps(self) -> &'static [i32] {
        match self {
            ScaleId::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleId::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleId::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleId::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleId::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            ScaleId::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            ScaleId::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleId::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            ScaleId::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            ScaleId::MajorPentatonic => &[0, 2, 4, 7, 9],
            ScaleId::MinorPentatonic => &[0, 3, 5, 7, 10],
            ScaleId::Blues => &[0, 3, 5, 6, 7, 10],
            ScaleId::WholeTone => &[0, 2, 4, 6, 8, 10],
        }
    }

    /// The name the text format writes.
    pub fn name(self) -> &'static str {
        match self {
            ScaleId::Major => "major",
            ScaleId::Minor => "minor",
            ScaleId::HarmonicMinor => "harmonic-minor",
            ScaleId::MelodicMinor => "melodic-minor",
            ScaleId::Dorian => "dorian",
            ScaleId::Phrygian => "phrygian",
            ScaleId::Lydian => "lydian",
            ScaleId::Mixolydian => "mixolydian",
            ScaleId::Locrian => "locrian",
            ScaleId::MajorPentatonic => "major-pentatonic",
            ScaleId::MinorPentatonic => "minor-pentatonic",
            ScaleId::Blues => "blues",
            ScaleId::WholeTone => "whole-tone",
        }
    }

    /// Reads a scale name, accepting the common aliases.
    pub fn parse(text: &str) -> Option<Self> {
        let normalised = text.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        Some(match normalised.as_str() {
            "major" | "ionian" | "maj" => ScaleId::Major,
            "minor" | "aeolian" | "min" | "natural-minor" => ScaleId::Minor,
            "harmonic-minor" | "harmonic" => ScaleId::HarmonicMinor,
            "melodic-minor" | "melodic" => ScaleId::MelodicMinor,
            "dorian" => ScaleId::Dorian,
            "phrygian" => ScaleId::Phrygian,
            "lydian" => ScaleId::Lydian,
            "mixolydian" | "mixo" => ScaleId::Mixolydian,
            "locrian" => ScaleId::Locrian,
            "major-pentatonic" | "pentatonic" | "pent-major" => ScaleId::MajorPentatonic,
            "minor-pentatonic" | "pent-minor" => ScaleId::MinorPentatonic,
            "blues" => ScaleId::Blues,
            "whole-tone" | "wholetone" => ScaleId::WholeTone,
            _ => return None,
        })
    }

    /// How many degrees the scale has before it repeats.
    pub fn degree_count(self) -> usize {
        self.steps().len()
    }

    /// `true` when the scale has a minor third above its tonic.
    pub fn is_minor(self) -> bool {
        self.steps().contains(&3)
    }

    // There was a `brightness` here, ordering the scales by how flat their degrees sit against
    // the major scale. It compared the two step lists *by index*, so a scale with fewer than
    // seven degrees was lined up against the wrong reference from its first gap onward: minor
    // pentatonic came out brighter than the major scale and tied with Lydian. Its one caller was
    // `Mood::scale`, which chose a mode for a brightness in a case that could not arise and has
    // since gone. A wrong answer nothing asks for is a trap for whoever asks next, so it is gone
    // too — the ordering it was reaching for is a real idea, and worth writing properly against
    // scale degrees rather than array positions when something needs it.

    /// Semitones above the tonic for `degree`, which may run past the octave or below zero.
    pub fn semitone(self, degree: i32) -> i32 {
        let steps = self.steps();
        let count = steps.len() as i32;
        let index = degree.rem_euclid(count);
        let octaves = degree.div_euclid(count);
        steps[index as usize] + octaves * OCTAVE
    }

    /// The degree whose semitone offset is `semitones`, if the scale contains it.
    pub fn degree_of(self, semitones: i32) -> Option<i32> {
        let steps = self.steps();
        let count = steps.len() as i32;
        let within = semitones.rem_euclid(OCTAVE);
        let octaves = semitones.div_euclid(OCTAVE);
        steps
            .iter()
            .position(|step| *step == within)
            .map(|index| index as i32 + octaves * count)
    }

    /// The degree nearest to `semitones`, rounding down on a tie.
    ///
    /// Used to pull a note that is not in the scale onto the nearest one that is, which is what
    /// keeps a transposed motif inside the key.
    pub fn nearest_degree(self, semitones: i32) -> i32 {
        let steps = self.steps();
        let count = steps.len() as i32;
        let within = semitones.rem_euclid(OCTAVE);
        let octaves = semitones.div_euclid(OCTAVE);
        let mut best = 0;
        let mut best_distance = i32::MAX;
        let mut best_pitch = i32::MAX;
        for (index, step) in steps.iter().enumerate() {
            // Compare against the next octave too, so B is nearer to C above than to A below.
            for candidate in [*step, step + OCTAVE] {
                let distance = (candidate - within).abs();
                // Rounding down on a tie is the lower *pitch*, not whichever degree the loop
                // happened to reach first. The two agree everywhere except at the top of the
                // scale: the octave above the tonic is examined at index 0, long before the
                // degree sitting a semitone below the query, so the octave used to win a tie it
                // should have lost. In C minor a passing B snapped up to C instead of down to B
                // flat, in the eight of thirteen scales whose top step is a flat seventh.
                if distance < best_distance || (distance == best_distance && candidate < best_pitch)
                {
                    best_distance = distance;
                    best_pitch = candidate;
                    best = index as i32
                        + if candidate > within && *step < within {
                            count
                        } else {
                            0
                        };
                }
            }
        }
        best + octaves * count
    }

    /// `true` when `pitch` is in the scale built on `tonic`.
    pub fn contains(self, tonic: PitchClass, pitch: PitchClass) -> bool {
        self.degree_of(tonic.distance_up_to(pitch)).is_some()
    }

    /// Every pitch class of the scale built on `tonic`, ascending from it.
    pub fn classes(self, tonic: PitchClass) -> Vec<PitchClass> {
        self.steps()
            .iter()
            .map(|step| tonic.transposed(*step))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_is_ascending_and_inside_one_octave() {
        for scale in ScaleId::ALL {
            let steps = scale.steps();
            assert_eq!(steps[0], 0, "{scale:?} does not start on its tonic");
            assert!(
                steps.windows(2).all(|pair| pair[0] < pair[1]),
                "{scale:?} is not ascending"
            );
            assert!(
                steps.iter().all(|step| (0..OCTAVE).contains(step)),
                "{scale:?} leaves the octave"
            );
        }
    }

    #[test]
    fn every_scale_name_round_trips() {
        for scale in ScaleId::ALL {
            assert_eq!(ScaleId::parse(scale.name()), Some(scale));
        }
        assert_eq!(
            ScaleId::parse("Harmonic Minor"),
            Some(ScaleId::HarmonicMinor)
        );
        assert_eq!(ScaleId::parse("AEOLIAN"), Some(ScaleId::Minor));
        assert_eq!(ScaleId::parse("nonsense"), None);
    }

    #[test]
    fn the_modes_have_the_intervals_they_are_named_for() {
        // Dorian is minor with a natural sixth; Phrygian is minor with a flat second.
        assert_eq!(ScaleId::Dorian.steps()[5], 9);
        assert_eq!(ScaleId::Minor.steps()[5], 8);
        assert_eq!(ScaleId::Phrygian.steps()[1], 1);
        // Lydian raises the fourth; Mixolydian flattens the seventh.
        assert_eq!(ScaleId::Lydian.steps()[3], 6);
        assert_eq!(ScaleId::Mixolydian.steps()[6], 10);
        // Harmonic minor's whole point is the major seventh over a minor triad.
        assert_eq!(ScaleId::HarmonicMinor.steps()[6], 11);
        assert_eq!(ScaleId::HarmonicMinor.steps()[2], 3);
    }

    #[test]
    fn degrees_run_past_the_octave_in_both_directions() {
        let major = ScaleId::Major;
        assert_eq!(major.semitone(0), 0);
        assert_eq!(major.semitone(7), 12, "the octave is degree seven");
        assert_eq!(major.semitone(8), 14);
        assert_eq!(major.semitone(-1), -1, "the leading tone below the tonic");
        assert_eq!(major.semitone(-7), -12);

        let pentatonic = ScaleId::MajorPentatonic;
        assert_eq!(pentatonic.semitone(5), 12, "five degrees to the octave");
        assert_eq!(pentatonic.semitone(-1), -3);
    }

    #[test]
    fn semitones_map_back_to_the_degree_they_came_from() {
        for scale in ScaleId::ALL {
            for degree in -9..17 {
                let semitones = scale.semitone(degree);
                assert_eq!(
                    scale.degree_of(semitones),
                    Some(degree),
                    "{scale:?} lost degree {degree}"
                );
            }
        }
    }

    #[test]
    fn a_semitone_outside_the_scale_has_no_degree() {
        assert_eq!(ScaleId::Major.degree_of(1), None, "C# is not in C major");
        assert_eq!(ScaleId::Major.degree_of(6), None);
        assert_eq!(ScaleId::MajorPentatonic.degree_of(5), None, "no fourth");
    }

    #[test]
    fn the_nearest_degree_is_the_nearest_one() {
        let major = ScaleId::Major;
        // C# is a semitone above C and a semitone below D; the tie rounds down.
        assert_eq!(
            major.semitone(major.nearest_degree(1)).rem_euclid(OCTAVE),
            0
        );
        // F# is nearer to F than to G.
        assert_eq!(
            major.semitone(major.nearest_degree(6)).rem_euclid(OCTAVE),
            5
        );
        // A note already in the scale is its own nearest.
        for degree in 0..7 {
            let semitones = major.semitone(degree);
            assert_eq!(major.nearest_degree(semitones), degree);
        }
    }

    #[test]
    fn a_tie_at_the_top_of_the_scale_rounds_down_like_every_other_tie() {
        // Eight of the thirteen scales end on a flat seventh, so a semitone below the tonic is
        // exactly as far from that seventh as from the octave above. The walk examines the octave
        // first — it belongs to index 0 — and used to keep it, which is the opposite of the
        // documented tie-break and of what the in-octave case does. In C minor a passing B jumped
        // up to C instead of settling down onto B flat.
        for scale in [
            ScaleId::Minor,
            ScaleId::Dorian,
            ScaleId::Phrygian,
            ScaleId::Mixolydian,
            ScaleId::Locrian,
            ScaleId::MinorPentatonic,
            ScaleId::Blues,
            ScaleId::WholeTone,
        ] {
            let degree = scale.nearest_degree(11);
            assert_eq!(
                scale.semitone(degree),
                10,
                "{} rounded a tie upward",
                scale.name()
            );
        }

        // The scales that end on a leading tone have no tie to break there, and answer with it.
        for scale in [ScaleId::Major, ScaleId::HarmonicMinor, ScaleId::Lydian] {
            assert_eq!(scale.semitone(scale.nearest_degree(11)), 11);
        }

        // And the in-octave tie the rule was always right about is still right.
        assert_eq!(ScaleId::Major.semitone(ScaleId::Major.nearest_degree(1)), 0);
        assert_eq!(ScaleId::Major.semitone(ScaleId::Major.nearest_degree(6)), 5);
    }

    #[test]
    fn membership_is_measured_from_the_tonic() {
        let d = PitchClass::parse("D").unwrap();
        // D major contains F# and C#, and does not contain F.
        assert!(ScaleId::Major.contains(d, PitchClass::parse("F#").unwrap()));
        assert!(ScaleId::Major.contains(d, PitchClass::parse("C#").unwrap()));
        assert!(!ScaleId::Major.contains(d, PitchClass::parse("F").unwrap()));

        let classes = ScaleId::Major.classes(d);
        assert_eq!(classes.len(), 7);
        assert_eq!(classes[0], d);
        assert_eq!(classes[2].sharp_name(), "F#");
    }
}
