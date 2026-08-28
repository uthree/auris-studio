//! How a written clip was written: what it is trying to be, and the dials it was written with.
//!
//! A [`ClipRecipe`] is what a clip carries when it came from the harmony underneath it rather
//! than from somebody playing, and it is the whole of what a regeneration reads. Its own file
//! because nothing here knows what a clip *is* — no ticks, no notes, nothing about the timeline
//! — which is what lets the composer and a picker in the interface both choose from this
//! vocabulary without either of them opening the document model.
//!
//! The `default_*` functions are here, private, and stay here. Each is named by a
//! `#[serde(default = "…")]` above it, that string resolves in the module the derive is in, and
//! a dial whose default was somewhere else would come back as zero out of an older file.

use serde::{Deserialize, Serialize};

/// What an automatically written clip is trying to be.
///
/// The vocabulary a person chooses from, and — since the drums arrived one at a time — the same
/// vocabulary the composer writes in. `Drums` is a whole kit in one clip, which is what somebody
/// filling a bar by hand wants; `Kick`, `Snare` and `Hat` are the three parts a written song keeps
/// on tracks of their own, so that a kit can be mixed at all.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipPreset {
    /// The tune.
    Lead,
    /// Chords, played rhythmically.
    Chords,
    /// A held chord bed.
    Pad,
    /// A broken chord.
    Arp,
    /// The bass line.
    Bass,
    /// Short chords hammered on the subdivision.
    Stab,
    /// Kick, snare and hat together.
    Drums,
    /// The kick drum alone.
    Kick,
    /// The snare alone.
    Snare,
    /// The hi-hat alone.
    Hat,
}

impl ClipPreset {
    /// Every preset, in the order a picker should offer them.
    ///
    /// The whole kit before the three pieces of it: a person reaching for drums usually means all
    /// of them, and the parts are what a mix wants rather than what a first choice does.
    pub const ALL: [ClipPreset; 10] = [
        ClipPreset::Lead,
        ClipPreset::Chords,
        ClipPreset::Pad,
        ClipPreset::Arp,
        ClipPreset::Stab,
        ClipPreset::Bass,
        ClipPreset::Drums,
        ClipPreset::Kick,
        ClipPreset::Snare,
        ClipPreset::Hat,
    ];

    /// The name the interface and the command line write.
    pub fn name(self) -> &'static str {
        match self {
            ClipPreset::Lead => "lead",
            ClipPreset::Chords => "chords",
            ClipPreset::Pad => "pad",
            ClipPreset::Arp => "arp",
            ClipPreset::Bass => "bass",
            ClipPreset::Stab => "stab",
            ClipPreset::Drums => "drums",
            ClipPreset::Kick => "kick",
            ClipPreset::Snare => "snare",
            ClipPreset::Hat => "hat",
        }
    }

    /// Reads a preset name, accepting the obvious synonyms.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "lead" | "melody" | "tune" => ClipPreset::Lead,
            "chords" | "comp" => ClipPreset::Chords,
            "pad" | "strings" => ClipPreset::Pad,
            "arp" | "arpeggio" => ClipPreset::Arp,
            "bass" => ClipPreset::Bass,
            "stab" | "stabs" | "release-cut" => ClipPreset::Stab,
            "drums" | "drum" | "kit" => ClipPreset::Drums,
            "kick" | "bd" => ClipPreset::Kick,
            "snare" | "sd" => ClipPreset::Snare,
            "hat" | "hihat" | "hh" => ClipPreset::Hat,
            _ => return None,
        })
    }

    /// `true` when this preset plays a drum rather than a pitch.
    ///
    /// What separates them is the harmony: a pitched part has nothing to play where no chord is
    /// written, and a kit carries on regardless.
    pub fn is_drums(self) -> bool {
        matches!(
            self,
            ClipPreset::Drums | ClipPreset::Kick | ClipPreset::Snare | ClipPreset::Hat
        )
    }
}

/// How finely a beat is divided, which is the grid everything a part plays lands on.
///
/// Two families rather than one number: a beat divides in two or it divides in three, and no
/// amount of a straight grid reaches a triplet. Sixteenths are the default because that is what a
/// drum pattern is written in and what most music is felt in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Subdivision {
    /// Two steps to the beat.
    Eighth,
    /// Four steps to the beat.
    #[default]
    Sixteenth,
    /// Three steps to the beat: the triplet.
    EighthTriplet,
    /// Six steps to the beat.
    SixteenthTriplet,
}

impl Subdivision {
    /// Every subdivision, in the order a picker should offer them: coarse to fine, straight
    /// before triplet.
    pub const ALL: [Subdivision; 4] = [
        Subdivision::Eighth,
        Subdivision::Sixteenth,
        Subdivision::EighthTriplet,
        Subdivision::SixteenthTriplet,
    ];

    /// How many steps one beat divides into.
    ///
    /// Every one of these divides [`TICKS_PER_QUARTER`](crate::time::TICKS_PER_QUARTER) exactly,
    /// which is why a triplet here is a position and not a rounding error.
    pub fn steps_per_beat(self) -> u32 {
        match self {
            Subdivision::Eighth => 2,
            Subdivision::Sixteenth => 4,
            Subdivision::EighthTriplet => 3,
            Subdivision::SixteenthTriplet => 6,
        }
    }

    /// `true` when the beat divides in three.
    pub fn is_triplet(self) -> bool {
        self.steps_per_beat().is_multiple_of(3)
    }

    /// The name the interface and the command line write.
    pub fn name(self) -> &'static str {
        match self {
            Subdivision::Eighth => "eighth",
            Subdivision::Sixteenth => "sixteenth",
            Subdivision::EighthTriplet => "eighth-triplet",
            Subdivision::SixteenthTriplet => "sixteenth-triplet",
        }
    }

    /// Reads a subdivision name, accepting the note values people actually say.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "eighth" | "8" | "1/8" => Subdivision::Eighth,
            "sixteenth" | "16" | "1/16" => Subdivision::Sixteenth,
            "eighth-triplet" | "8t" | "1/8t" | "triplet" => Subdivision::EighthTriplet,
            "sixteenth-triplet" | "16t" | "1/16t" => Subdivision::SixteenthTriplet,
            _ => return None,
        })
    }
}

/// How a clip was written, so that it can be written again.
///
/// A clip carrying one of these was produced from the harmony underneath it rather than played,
/// and can be produced again with a different seed or a different feel. Dropping the recipe is
/// what freezing a clip means: the notes stay exactly where they are and stop being derived from
/// anything, which is how a phrase somebody likes stops being at the mercy of the next
/// regeneration.
///
/// The notes are stored alongside it rather than recomputed on load, so a project plays and
/// exports without the composer ever running. What that preserves across builds is the take *as
/// saved*: asking a newer composer to write the recipe again is a redraw in the current style,
/// not a reproduction of the old take. A seed names a take within a build, not an archival
/// format — the way to keep a take is to freeze it, not to remember its number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipRecipe {
    /// What the clip is trying to be.
    pub preset: ClipPreset,
    /// The number every random choice is drawn from. A different one is a different take.
    pub seed: u64,
    /// How busy it is, from 0 for sparse to 1 for a wall of notes.
    ///
    /// A kit reads it too, around the middle: below, the groove thins from its weakest hits
    /// upward, and above, the steps it left empty start filling in with ghost notes. *Which*
    /// rhythm a kit plays is still its [`groove`](Self::groove) — that is a choice from a
    /// drummer's own vocabulary and not a number — and this is how hard the drummer is leaning
    /// on it.
    pub density: f32,
    /// How hard it is played, from 0 to 1.
    ///
    /// Every preset reads this: it sets how the notes are struck, not how many there are.
    pub intensity: f32,
    /// Which drum groove the kit plays. Ignored by everything except the drums.
    #[serde(default = "default_groove")]
    pub groove: String,
    /// How far the offbeats are delayed, as a percentage where 50 is straight.
    #[serde(default = "default_swing")]
    pub swing: u8,
    /// How finely the beat is divided, which is the grid the part's figures land on.
    ///
    /// A drum kit ignores it: a groove is written in sixteenths, and reading one on a triplet
    /// grid would scatter it rather than swing it.
    #[serde(default)]
    pub subdivision: Subdivision,
    /// How long a note is held, as a fraction of the gap to the one after it.
    ///
    /// 1 is legato — each note lasts until the next begins. Turning it down detaches them, and
    /// far down is the sound of a chord hammered on every sixteenth with the release cut off.
    #[serde(default = "default_gate")]
    pub gate: f32,
    /// How far apart the hardest and softest notes are struck, from 0 to 1.
    ///
    /// Not how hard the part is played, which is [`intensity`](Self::intensity), but how much the
    /// playing varies around that. At 0 every note is struck alike, which is a sequencer and is
    /// sometimes exactly right; at 1 the metric hierarchy is at full strength and a downbeat is
    /// half again the weight of the sixteenth before it. The mean stays where the intensity put
    /// it either way, so widening the spread does not quietly turn the part down.
    #[serde(default = "default_dynamics")]
    pub dynamics: f32,
    /// How far the figures pull off the beat, from 0 for square to 1 for wilfully awkward.
    ///
    /// Read by the parts that roll their own rhythm rather than picking a written one: it lifts
    /// the weak steps toward the strong ones instead of adding notes, so it changes *where* a
    /// part plays without changing how much.
    #[serde(default = "default_syncopation")]
    pub syncopation: f32,
    /// Octaves to move the part from where its preset sits, from -2 to 2.
    ///
    /// The register a part chooses is drawn from its seed, which is right for a take and useless
    /// when the answer is "the same thing, higher". This is that answer.
    #[serde(default)]
    pub octave: i32,
    /// How much of the last bar the snare runs as a fill, from 0 for none to 1 for two beats.
    ///
    /// Read by the drums alone. A bar that simply stops and is replaced sounds like an edit
    /// rather than an arrival, and the join is the one moment a listener is certain to notice.
    #[serde(default = "default_fill")]
    pub fill: f32,
    /// The digest of the notes as the composer last wrote them —
    /// [`notes_digest`](super::notes_digest) over the clip's text at the moment of writing.
    ///
    /// What makes a hand edit visible. A recipe promises that writing the clip again replaces
    /// its notes, and a clip whose notes no longer answer with this number has been edited
    /// since — an edit a regenerate would silently discard, which is worth saying on screen
    /// before it happens. Informational, never gating: every rewrite still proceeds.
    ///
    /// Zero means "nobody digested this text" — a file from before the field, or a recipe built
    /// by hand — and such a clip is never flagged, because a warning that cannot be trusted
    /// teaches people to ignore the one that can.
    #[serde(default, skip_serializing_if = "digest_is_unknown")]
    pub text_digest: u64,
}

/// Whether a digest is the "nobody measured" zero, for keeping it out of saved files.
fn digest_is_unknown(digest: &u64) -> bool {
    *digest == 0
}

fn default_swing() -> u8 {
    50
}

fn default_gate() -> f32 {
    1.0
}

fn default_dynamics() -> f32 {
    1.0
}

fn default_syncopation() -> f32 {
    0.3
}

fn default_fill() -> f32 {
    0.5
}

fn default_groove() -> String {
    "basic-rock".to_string()
}

impl ClipRecipe {
    /// A recipe for `preset`, with the dials where a first attempt should start.
    ///
    /// Only the stab starts anywhere unusual, and it has to: every other preset is a *part* whose
    /// identity survives the dials being moved, while a stab is nothing but a position on them —
    /// short, fast and machine-tight. Landing it on the same middling defaults as a pad would mean
    /// choosing it and hearing a pad, with the sound it was named for three dials away.
    pub fn new(preset: ClipPreset, seed: u64) -> Self {
        let mut recipe = Self {
            preset,
            seed,
            density: 0.5,
            intensity: 0.7,
            groove: default_groove(),
            swing: 50,
            subdivision: Subdivision::default(),
            gate: default_gate(),
            dynamics: default_dynamics(),
            syncopation: default_syncopation(),
            octave: 0,
            fill: default_fill(),
            text_digest: 0,
        };
        if preset == ClipPreset::Stab {
            recipe.density = 0.95;
            recipe.intensity = 0.85;
            recipe.gate = 0.3;
            // Flatter than most parts, on purpose. A stab is a rhythm played by a chord, and a
            // metric hierarchy at full strength turns the sixteenths between the beats into
            // ghost notes — which is a groove, and not this one.
            recipe.dynamics = 0.45;
        }
        recipe
    }

    /// The same recipe with a different seed, which is what "another take" means.
    pub fn with_seed(&self, seed: u64) -> Self {
        Self {
            seed,
            ..self.clone()
        }
    }
}
