//! How the piece should feel, as four numbers.
//!
//! The other half of the vocabulary a specification is written in, and kept apart from the model
//! for the same reason [`Role`](super::Role) is: a mood is a small table of dials, and what those
//! dials mean — how often a chord gains a seventh, how many notes a bar wants, how high the
//! melody sits — is arithmetic that nothing about reading or writing a document belongs in.

/// How the piece should feel.
///
/// Four numbers rather than a list of genre names: a genre is a point in this space, and a
/// number can be nudged. Every one runs from 0 to 1.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mood {
    /// Dark to bright. Slides the register the melodic skeleton is written in.
    ///
    /// It used to be documented as choosing the scale when none was named, through a ladder of
    /// modes from Phrygian to Lydian. Nothing ever called it: a specification always carries a
    /// key, and a clip generated against the document reads the harmony lane's — so there was no
    /// path where a scale had not been named, and the dial moved nothing at all. Overriding a key
    /// the user did choose is not a repair, it is a different and worse feature.
    ///
    /// Register is the other thing "bright" says about a melody, it is audible immediately, and
    /// it composes with the key instead of fighting it. The melodic skeleton reads it, and every
    /// part hangs off that skeleton.
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
}
