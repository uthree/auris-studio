//! What a part does, and everything that follows from it.
//!
//! A role is the one word a specification has to say to be given a whole part: the instrument,
//! the octave, the level, the pan, the range and the colour all follow from it. Every one of them
//! is a table rather than a decision, and a table is what somebody opens this file to edit —
//! "make the hat quieter" should be one number in one short file, not a read of the whole format.

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
    /// The crash cymbal, struck where one section arrives at the next.
    Crash,
}

impl Role {
    /// Every role, in the order a default roster uses them.
    pub const ALL: [Role; 10] = [
        Role::Melody,
        Role::Chords,
        Role::Pad,
        Role::Arp,
        Role::Stab,
        Role::Bass,
        Role::Kick,
        Role::Snare,
        Role::Hat,
        Role::Crash,
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
            Role::Crash => "crash",
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
            "crash" | "cymbal" | "cym" => Role::Crash,
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
            Role::Crash => DrumVoice::Crash,
            _ => return None,
        })
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
            // Opposite the hat, which is the one thing in the kit it would otherwise sit on top
            // of: both are bright, both are mostly noise, and a crash landing in the hat's place
            // reads as the hat having got louder rather than as a cymbal.
            Role::Crash => -0.2,
        }
    }

    /// The level a part of this role sits at, in decibels.
    ///
    /// Six parts all at unity sum well past full scale. These are the rough balances a mix
    /// engineer would reach for first: the tune on top, the pad well under it.
    ///
    /// The kit sits *above* the tune, which is where a kit sits in almost every record made
    /// since about 1980. It used to sit five decibels under one, which is a demo of an
    /// arrangement rather than a piece of music — the drums were audibly holding back, and the
    /// hat at −20 was a rumour.
    pub fn default_gain_db(self) -> f32 {
        match self {
            Role::Melody => -7.0,
            Role::Chords => -14.0,
            Role::Pad => -16.0,
            Role::Arp => -12.0,
            Role::Stab => -13.0,
            Role::Bass => -10.0,
            Role::Kick => -5.0,
            Role::Snare => -6.0,
            Role::Hat => -15.0,
            // Under the snare, which is the one it is most often heard next to — they are struck
            // together on the downbeat of a chorus, and a cymbal that arrives louder than the
            // backbeat swallows it. Loud enough that the join is unmistakable; a crash nobody
            // notices is a crash nobody wrote.
            Role::Crash => -9.0,
        }
    }

    /// How loud a part of this role should end up, in LUFS, measured on its own.
    ///
    /// The balance [`Self::default_gain_db`] is trying to describe, said in the units it is
    /// actually heard in. A fader position only means something if every instrument is equally
    /// loud at unity, and none of them are: the same number on the same fader is a General MIDI
    /// piano out of one font and a square wave out of the built-in synth, which are not within ten
    /// decibels of each other. `auris_session::Session::balance_levels` renders each part alone,
    /// measures it and moves the fader until it reads the number here — so the mix is the same mix
    /// whatever answered the call for a sound.
    ///
    /// **Calibrated, not chosen.** These are what the eight presets measure today, per role,
    /// through the faders that were set by ear; a piece balanced against them comes out where it
    /// already came out, and the arithmetic only bites when an instrument is not the one the
    /// numbers were taken on. The measurement is `Session::balance_levels` itself — a role whose
    /// target is what it already measures is a fixed point, which is the property to check when
    /// any of this moves.
    ///
    /// They are absolute rather than relative to the tune, and that is deliberate: a piece with
    /// three parts and a piece with eight would sit at different levels if these were relative,
    /// and the whole mix is moved onto `auris_session::TARGET_LUFS` afterwards anyway.
    pub fn target_lufs(self) -> f32 {
        match self {
            Role::Melody => -23.2,
            Role::Chords => -26.3,
            Role::Pad => -31.6,
            Role::Arp => -32.6,
            Role::Stab => -29.0,
            Role::Bass => -27.1,
            Role::Kick => -23.9,
            Role::Snare => -27.4,
            Role::Hat => -43.6,
            Role::Crash => -29.6,
        }
    }

    /// The colour a track of this role is drawn in.
    ///
    /// A composed song used to take the palette in order, so which colour a part got depended on
    /// how many parts were declared before it — the bass was green in one piece and pink in the
    /// next, and a colour that means nothing is a colour nobody reads. By role, the arrangement
    /// can be read at a glance and reads the same way every time.
    ///
    /// The kit is one family in four weights, because the four of them are one instrument.
    /// Nothing else shares a hue with anything else.
    pub fn color(self) -> auris_core::project::Color {
        auris_core::project::Color(match self {
            Role::Melody => 0xe0b452,
            Role::Chords => 0x5fc9a3,
            Role::Pad => 0xb07cc6,
            Role::Arp => 0x4f9dde,
            Role::Stab => 0xd16b8a,
            Role::Bass => 0x6f7fd6,
            Role::Kick => 0xc0554a,
            Role::Snare => 0xd97b6c,
            Role::Hat => 0xe8a396,
            // The lightest of the family, next to the hat: the two cymbals read as a pair, which
            // is what they are, and the weights still run heaviest to brightest down the kit.
            Role::Crash => 0xf2c9b4,
        })
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
    fn every_role_has_a_colour_of_its_own_and_it_can_be_seen() {
        // Two roles sharing a colour would fail in exactly the place colour is for: telling the
        // bass from the pad at a glance, across forty bars, without reading a single name.
        let mut seen: Vec<u32> = Role::ALL.iter().map(|role| role.color().0).collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(count, seen.len(), "two roles share a colour");

        for role in Role::ALL {
            let (r, g, b) = role.color().rgb();
            let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            assert!(
                (70.0..=225.0).contains(&luma),
                "{} is {luma:.0} bright, which is a track nobody can pick out of the lanes",
                role.name()
            );
        }
    }

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

    #[test]
    fn the_kit_carries_the_mix_rather_than_hiding_under_it() {
        // The kick and the snare are the two loudest things in most records made since about
        // 1980. They used to sit five and six decibels under the tune, which is a demo of an
        // arrangement rather than a piece of music.
        let melody = Role::Melody.default_gain_db();
        assert!(Role::Kick.default_gain_db() > melody);
        assert!(Role::Snare.default_gain_db() > melody);
        // The hat is the exception and stays under: loud enough to be heard keeping time, never
        // the thing being listened to. Under the tune, and still clear of the pad.
        assert!(Role::Hat.default_gain_db() < melody);
        assert!(Role::Hat.default_gain_db() > Role::Pad.default_gain_db());
    }
}
