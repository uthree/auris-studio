//! How each row of a keyboard looks under the document's tuning.
//!
//! The piano roll's rows are keys, one to a row, in every tuning — that is what lets a note stay
//! a key while the tuning moves where it sounds. What a tuning changes is how a row is drawn: which
//! rows are dark, where the strong line that marks a period falls, and what the strip names.
//!
//! In a twelve-note tuning the answer is the piano's, key for key: just intonation and the
//! meantones still have twelve notes to an octave, key 61 is still C sharp, and only its pitch has
//! moved. In any other tuning the keyboard follows the scale instead: a period line under each key
//! that plays the root, the name of the note each of those *sounds* nearest, and a row shaded dark
//! when the equal-tempered key nearest its sound is a black one — so the familiar pattern shows
//! where the notes really lie, a 31-tone scale drawing two or three rows for every key of a piano.
//!
//! Free functions, because a view cannot be asked what it painted.

use auris_session::prelude::Tuning;

use super::timeline::{is_black_key, pitch_name};

/// How one key's row is drawn.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyFace {
    /// Shaded as a black key.
    pub black: bool,
    /// The first key of a period — a C on a piano — which gets the strong line beneath it.
    pub period_start: bool,
    /// What the keyboard strip writes on the key, where it writes anything.
    pub label: Option<String>,
}

/// The faces of all 128 keys under `tuning`, lowest key first.
pub fn key_faces(tuning: &Tuning) -> Vec<KeyFace> {
    (0..=127u8).map(|key| key_face(tuning, key)).collect()
}

/// How `key` is drawn under `tuning`.
pub fn key_face(tuning: &Tuning, key: u8) -> KeyFace {
    if tuning.is_twelve_tone() {
        let period_start = key.is_multiple_of(12);
        return KeyFace {
            black: is_black_key(key),
            period_start,
            label: period_start.then(|| pitch_name(key)),
        };
    }
    let nearest = nearest_key(tuning.sounding_pitch(key));
    let steps = tuning.steps_per_period() as i64;
    let period_start = (i64::from(key) - i64::from(tuning.root_key())).rem_euclid(steps) == 0;
    KeyFace {
        black: is_black_key(nearest),
        period_start,
        label: period_start.then(|| pitch_name(nearest)),
    }
}

/// Where `key` sounds, named: the nearest equal-tempered note, and how many cents off it, when it
/// is not on one — `E4 −14¢` for the third of just intonation on C.
///
/// The standard tuning gives the plain name, so nothing reads differently until a tuning is
/// chosen.
pub fn sounding_name(tuning: &Tuning, key: u8) -> String {
    if tuning.is_standard() {
        return pitch_name(key);
    }
    let pitch = tuning.sounding_pitch(key);
    let nearest = nearest_key(pitch);
    let cents = ((pitch - f64::from(nearest)) * 100.0).round() as i64;
    match cents {
        0 => pitch_name(nearest),
        // A real minus sign, as a cents offset is printed, rather than a hyphen.
        cents if cents < 0 => format!("{} \u{2212}{}¢", pitch_name(nearest), -cents),
        cents => format!("{} +{cents}¢", pitch_name(nearest)),
    }
}

/// The equal-tempered key nearest a sounding pitch.
fn nearest_key(pitch: f64) -> u8 {
    pitch.round().clamp(0.0, 127.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::prelude::PitchClass;

    #[test]
    fn the_standard_tuning_draws_the_piano_it_always_did() {
        let standard = Tuning::standard();
        for key in 0..=127u8 {
            let face = key_face(&standard, key);
            assert_eq!(face.black, is_black_key(key), "key {key}");
            assert_eq!(face.period_start, key % 12 == 0, "key {key}");
            assert_eq!(face.label, (key % 12 == 0).then(|| pitch_name(key)));
            assert_eq!(sounding_name(&standard, key), pitch_name(key));
        }
    }

    #[test]
    fn a_twelve_note_tuning_keeps_the_keyboard_and_names_the_cents() {
        let just = Tuning::just(PitchClass::new(0));
        assert_eq!(key_face(&just, 61), key_face(&Tuning::standard(), 61));
        assert_eq!(key_face(&just, 60), key_face(&Tuning::standard(), 60));
        // The pure major third sits fourteen cents under the equal-tempered one.
        assert_eq!(sounding_name(&just, 64), "E4 \u{2212}14¢");
        assert_eq!(sounding_name(&just, 60), "C4");
    }

    #[test]
    fn thirty_one_tones_mark_each_period_and_shade_by_where_a_key_sounds() {
        let tuning = Tuning::edo(31).unwrap();
        let faces = key_faces(&tuning);
        // Key 60 is the root on middle C, and the next period starts 31 keys up, on C5.
        assert!(faces[60].period_start);
        assert_eq!(faces[60].label.as_deref(), Some("C4"));
        assert!(faces[91].period_start);
        assert_eq!(faces[91].label.as_deref(), Some("C5"));
        assert!(!faces[61].period_start);
        assert_eq!(faces[61].label, None);
        // Three steps up is 116 cents, nearest C sharp: a dark row.
        assert!(faces[63].black);
        // Five steps up is 194 cents, nearest D: a light one.
        assert!(!faces[65].black);
        assert_eq!(sounding_name(&tuning, 61), "C4 +39¢");
    }

    #[test]
    fn a_scale_repeating_at_the_tritave_names_where_each_period_lands() {
        let tuning = Tuning::bohlen_pierce();
        let faces = key_faces(&tuning);
        // Thirteen steps make a twelfth: middle C, then the G an octave and a fifth above it.
        assert_eq!(faces[60].label.as_deref(), Some("C4"));
        assert_eq!(faces[73].label.as_deref(), Some("G5"));
    }
}
