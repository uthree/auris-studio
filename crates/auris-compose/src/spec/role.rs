//! What a part does, and everything that follows from it.
//!
//! A role is what a clip preset asks its writer for: the default instrument, the octave and the
//! range all follow from it. Every one of them is a table rather than a decision, and a table is
//! what somebody opens this file to edit.

use crate::rhythm::DrumVoice;

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
    pub const ALL: [Role; 8] = [
        Role::Melody,
        Role::Chords,
        Role::Pad,
        Role::Arp,
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
            "bass" => Role::Bass,
            "kick" | "bd" => Role::Kick,
            "snare" | "sd" => Role::Snare,
            "hat" | "hihat" | "hh" => Role::Hat,
            _ => return None,
        })
    }

    /// `true` when the part plays a drum rather than a pitch.
    pub fn is_drum(self) -> bool {
        self.drum_voice().is_some()
    }

    /// Which drum this role plays, or `None` for a pitched part.
    ///
    /// The one place the two vocabularies meet — a role is what a *part* is for and a voice is
    /// what a *groove* is written in — so nothing else has to know that a hat means a closed one.
    pub fn drum_voice(self) -> Option<DrumVoice> {
        Some(match self {
            Role::Kick => DrumVoice::Kick,
            Role::Snare => DrumVoice::Snare,
            Role::Hat => DrumVoice::ClosedHat,
            _ => return None,
        })
    }

    /// The musical assignment used by a stored kit map, independent of its MIDI note.
    pub fn drum_role(self) -> Option<auris_core::project::DrumRole> {
        use auris_core::project::DrumRole;
        Some(match self {
            Role::Kick => DrumRole::Kick,
            Role::Snare => DrumRole::Snare,
            Role::Hat => DrumRole::ClosedHat,
            _ => return None,
        })
    }

    /// The instrument a part of this role gets when none is named.
    pub fn default_instrument(self) -> &'static str {
        if self.is_drum() {
            "auris.synth.drumkit"
        } else if matches!(self, Role::Bass | Role::Pad) {
            "auris.synth.fm2"
        } else {
            "auris.synth.chiptune"
        }
    }

    /// The octave a part of this role sits in by default.
    pub fn default_octave(self) -> i32 {
        match self {
            Role::Melody | Role::Arp => 5,
            Role::Chords => 4,
            Role::Pad => 3,
            Role::Bass => 2,
            _ => 3,
        }
    }

    /// The MIDI range a part of this role should stay inside.
    pub fn range(self) -> (i32, i32) {
        match self {
            Role::Melody => (60, 84),
            Role::Arp => (60, 88),
            Role::Chords => (48, 72),
            // C3 to C5, which is where a pad is written, and above where the bass lives. It used
            // to start at C2 and share sixteen semitones with the bass: a voicing folded into
            // that could put a chord tone *under* the bass note, which is not a muddy mix but a
            // different chord — an inversion nobody wrote, decided by whichever note happened to
            // fold lowest. No part may read another's notes, so the ranges are the only place the
            // bass can be kept at the bottom.
            Role::Pad => (48, 72),
            Role::Bass => (28, 52),
            _ => (0, 127),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_pitched_is_written_below_the_bass() {
        // The bass is the bottom of the arrangement, and no part can look at another's notes to
        // find that out — the ranges are where it is decided. A pitched part whose floor sits
        // under the bass's own may sound below it, and a chord tone under the bass root is an
        // inversion the numeral never asked for.
        let (bass_low, bass_high) = Role::Bass.range();
        for role in Role::ALL {
            if role == Role::Bass || role.drum_voice().is_some() {
                continue;
            }
            let (low, _) = role.range();
            assert!(
                low > bass_low,
                "{} may be written below the bass's own floor",
                role.name()
            );
            assert!(
                low + 12 > bass_high,
                "{} shares more than an octave with the bass",
                role.name()
            );
        }
    }
}
