//! What a writer is asked for: one part, its role and the mood it is played in.
//!
//! [`Role`] and [`Mood`] are the vocabulary, and each keeps a file of its own because each is
//! mostly a table — the ranges, the octaves, the four dials a mood word means. [`PartSpec`] is
//! one part as [`write_phrase`](crate::write_phrase) builds it out of a clip's recipe.

mod mood;
mod role;

use auris_core::Subdivision;

use crate::rhythm::Pattern;

pub use self::mood::Mood;
pub use self::role::Role;

/// One part of the arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSpec {
    /// The writer's name; a drum kit's voices keep theirs inside the one clip.
    pub name: String,
    /// What it plays.
    pub role: Role,
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
    /// Which MIDI note a drum part strikes, when it is not the General MIDI one.
    ///
    /// GM is the only agreement there is about which number is a kick, and a SoundFont is under
    /// no obligation to keep it — a kit that puts its snare somewhere else comes out silent or
    /// playing a cowbell, and there was nothing to say otherwise. A pitched part ignores this:
    /// its notes come from the harmony.
    pub note: Option<u8>,
    /// Whether a chord part adds the root an octave or more below its voicing.
    ///
    /// Read by the chords and the pad; see [`ClipRecipe::low_root`](auris_core::ClipRecipe::low_root).
    pub low_root: bool,
    /// The note value between successive notes of a chord, bottom first: 32 is a thirty-second
    /// apart, and 0 strikes them together.
    ///
    /// Read by the chords and the pad; see [`ClipRecipe::strum`](auris_core::ClipRecipe::strum).
    pub strum: u16,
    /// The MIDI note a chord part's voicings average to, or `None` for the middle of its range.
    ///
    /// Read by the chords and the pad; see [`ClipRecipe::centre`](auris_core::ClipRecipe::centre).
    pub centre: Option<i32>,
    /// How many notes each chord sounds, or 0 for the chord's own tones.
    ///
    /// Read by the chords and the pad; see [`ClipRecipe::voices`](auris_core::ClipRecipe::voices).
    pub voices: u8,
}

impl PartSpec {
    /// A part with everything its role implies.
    pub fn of_role(name: impl Into<String>, role: Role) -> Self {
        Self {
            name: name.into(),
            role,
            octave: role.default_octave(),
            density: None,
            subdivision: Subdivision::default(),
            gate: 1.0,
            rhythm: None,
            note: None,
            low_root: false,
            strum: 0,
            centre: None,
            voices: 0,
        }
    }

    /// The MIDI note this part strikes, or `None` when it is not a drum.
    ///
    /// What the part says, or what General MIDI says its role is. One place, so the sheet's
    /// picker and the writer cannot disagree about which note a kit is about to play.
    pub fn drum_note(&self) -> Option<u8> {
        self.role
            .drum_voice()
            .map(|voice| self.note.unwrap_or_else(|| voice.pitch()))
    }

    /// The MIDI range this part should stay inside, moved by its octave.
    pub fn range(&self) -> (i32, i32) {
        let (low, high) = self.role.range();
        let shift = (self.octave - self.role.default_octave()) * 12;
        ((low + shift).clamp(0, 127), (high + shift).clamp(0, 127))
    }

    /// The note a chord part's voicings average to: the one asked for, or else the middle of
    /// [`Self::range`], which is where the octave dial puts it.
    pub fn chord_centre(&self) -> i32 {
        self.centre.map_or_else(
            || {
                let (low, high) = self.range();
                (low + high) / 2
            },
            |centre| centre.clamp(0, 127),
        )
    }

    /// The window a chord part's voicings are chosen inside: [`CHORD_REACH`] either side of
    /// [`Self::chord_centre`], slid back inside the keyboard when it would fall off an end.
    ///
    /// Wider than the role's own range on purpose. The voicing is pulled toward the centre by
    /// its cost, not held there by walls, and six voices spread open need more than two octaves
    /// to find an inversion whose middle is the note that was asked for.
    pub fn chord_range(&self) -> (i32, i32) {
        let low = (self.chord_centre() - CHORD_REACH).clamp(0, 127 - 2 * CHORD_REACH);
        (low, low + 2 * CHORD_REACH)
    }
}

/// How far either side of its centre a chord part's voicing may reach: an octave and a fifth.
pub const CHORD_REACH: i32 = 19;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_part_centres_where_its_recipe_does() {
        // Two defaults that must be one: the recipe's, which the window shows, and the role's,
        // which the composer writes a whole song in.
        for (role, preset) in [
            (Role::Chords, auris_core::ClipPreset::Chords),
            (Role::Pad, auris_core::ClipPreset::Pad),
        ] {
            for octave in -2..=2 {
                let mut part = PartSpec::of_role("chords", role);
                part.octave += octave;
                let recipe = auris_core::ClipRecipe {
                    octave,
                    ..auris_core::ClipRecipe::new(preset, 0)
                };
                assert_eq!(part.chord_centre(), i32::from(recipe.chord_centre()));
            }
        }
    }
}
