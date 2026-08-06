//! The serialisable document model.
//!
//! A [`Project`] holds everything the user edits: tempo, tracks, clips, notes and mixer state.
//! It contains no audio samples and no plugin instances — only ids and parameter values — which
//! keeps it cheap to clone for undo and trivially serialisable to JSON.
//!
//! Two indirections make that work:
//!
//! * A track names its instrument by plugin id plus a [`PluginState`]; the engine asks the
//!   [`PluginRegistry`](crate::registry::PluginRegistry) to build the real object.
//! * An audio clip names its samples by [`SourceId`]; the decoded audio lives in a separate
//!   runtime [`AudioSourceBank`], so the project stays small and a file imported once can back
//!   any number of clips.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use std::path::Path;

use crate::asset::AssetPath;
use crate::buffer::AudioBuffer;
use crate::harmony::Harmony;
use crate::plugin::PluginState;
use crate::time::{SignatureMap, TempoMap, Ticks};

/// Identifies a track within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TrackId(pub u64);

/// Identifies a clip within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ClipId(pub u64);

/// Identifies an imported audio file within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SourceId(pub u64);

/// Identifies one slot in an effect chain.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EffectSlotId(pub u64);

/// Identifies one send within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SendId(pub u64);

/// Identifies an imported SoundFont within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SoundFontId(pub u64);

/// Metadata about an imported SoundFont, stored in the project.
///
/// The samples are not here, for the same reason a decoded audio file is not: a font runs to
/// hundreds of megabytes and a document has to stay small enough to read, to diff and to keep in
/// an undo history. What is stored is what finds the file again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoundFontRef {
    /// Unique within the project.
    pub id: SoundFontId,
    /// Display name, from the font itself where it has a usable one.
    pub name: String,
    /// Where the file is, so a project can be re-opened later.
    ///
    /// Normally [`AssetPath::External`]: a font is a library shared by every project that uses
    /// it, and copying a hundred and fifty megabytes into each one would be a poor trade for a
    /// shorter path.
    pub path: AssetPath,
    /// Size of the file in bytes, or 0 when it was recorded before this field existed.
    ///
    /// Not for reading the file — for recognising it. When the stored path stops being true, the
    /// file name alone is a weak match, and this is what separates the font that moved from a
    /// different font someone happened to give the same name.
    #[serde(default)]
    pub byte_size: u64,
}

/// Which sound of a font a track plays.
///
/// Bank and patch rather than a position in the preset list, because that pair is what identifies
/// a sound across reloads — a position would move the moment anyone edited the file, and a
/// project saved last week would come back playing a different instrument.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetRef {
    /// Which font, by its id in this project.
    pub font: SoundFontId,
    /// MIDI bank, 0 for the standard set and 128 for percussion.
    pub bank: i32,
    /// MIDI program number within that bank.
    pub patch: i32,
}

/// An RGB colour used for track and clip tinting.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub u32);

impl Color {
    /// The palette new tracks cycle through.
    pub const PALETTE: [Color; 8] = [
        Color(0x4f9dde),
        Color(0x5fc9a3),
        Color(0xe0b452),
        Color(0xd97b6c),
        Color(0xb07cc6),
        Color(0xe0a458),
        Color(0x7fb069),
        Color(0xd16b8a),
    ];

    /// Picks a palette entry by index, wrapping around.
    pub fn from_palette(index: usize) -> Color {
        Self::PALETTE[index % Self::PALETTE.len()]
    }

    /// Red, green and blue components.
    pub fn rgb(self) -> (u8, u8, u8) {
        (
            ((self.0 >> 16) & 0xff) as u8,
            ((self.0 >> 8) & 0xff) as u8,
            (self.0 & 0xff) as u8,
        )
    }
}

/// A single note inside a [`MidiClip`].
///
/// `start` is relative to the clip's start, so moving a clip does not touch its notes.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI note number, 0..=127.
    pub pitch: u8,
    /// Attack strength, 0.0..=1.0.
    pub velocity: f32,
    /// Offset from the clip start.
    pub start: Ticks,
    /// How long the note is held.
    pub length: Ticks,
}

impl Note {
    /// A note with a default velocity.
    pub fn new(pitch: u8, start: Ticks, length: Ticks) -> Self {
        Self {
            pitch,
            velocity: 0.8,
            start,
            length,
        }
    }

    /// Position just past the end of the note.
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// `true` when `tick` falls inside the note.
    pub fn contains(&self, tick: Ticks) -> bool {
        tick >= self.start && tick < self.end()
    }
}

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
/// exports without the composer ever running, and so a file opened by a build whose composer has
/// changed still sounds like the piece that was saved.
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
    /// How far timing and velocity wander, from 0 for a machine to 1 for a sloppy band.
    #[serde(default)]
    pub humanize: f32,
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
            humanize: 0.25,
            subdivision: Subdivision::default(),
            gate: default_gate(),
            dynamics: default_dynamics(),
            syncopation: default_syncopation(),
            octave: 0,
            fill: default_fill(),
        };
        if preset == ClipPreset::Stab {
            recipe.density = 0.95;
            recipe.intensity = 0.85;
            recipe.gate = 0.3;
            recipe.humanize = 0.1;
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

/// One point on a clip's pitch bend curve.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BendPoint {
    /// Where it sits, measured from the clip's own start.
    pub at: Ticks,
    /// How far the sounding notes are bent, in semitones.
    pub semitones: f32,
}

/// How far a bend may be written, either way.
///
/// An octave. MIDI's own default range is two semitones and a hardware synth has to be told
/// otherwise, but [`NoteEvent::PitchBend`](crate::NoteEvent::PitchBend) carries *semitones* rather
/// than a fourteen-bit fraction, so nothing here has to agree with anything about a range — and a
/// dive of an octave is a sound people want.
pub const BEND_LIMIT: f32 = 12.0;

/// How finely a bend is sampled into events.
///
/// A ninety-sixth note, which at 120 bpm is about twenty milliseconds: fine enough that a slide
/// sounds continuous and coarse enough that a bar of it is a few dozen events rather than a few
/// thousand. Only the stretch a curve was actually written over is sampled at all.
pub const BEND_STEP: Ticks = Ticks(crate::time::TICKS_PER_QUARTER / 24);

/// A block of notes placed on an instrument track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    /// Unique within the project.
    pub id: ClipId,
    /// Label shown on the clip.
    pub name: String,
    /// Position on the timeline.
    pub start: Ticks,
    /// Length on the timeline. Notes past this point are not played.
    pub length: Ticks,
    /// Notes, in no particular order.
    pub notes: Vec<Note>,
    /// Whether the clip is skipped during playback.
    #[serde(default)]
    pub muted: bool,
    /// How the clip was written, when it was written rather than played.
    ///
    /// Added the way [`Self::muted`] was — an optional field with a default — so a document from
    /// before this existed opens unchanged, and so a clip somebody played by hand is stored
    /// without a word about a composer that had nothing to do with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<ClipRecipe>,
    /// The pitch bend written across the clip, in time order.
    ///
    /// On the *clip* rather than in [`Project::automation`], for two reasons. A bend is not a
    /// plugin parameter — it is a message every instrument answers, which is why
    /// [`NoteEvent::PitchBend`](crate::NoteEvent::PitchBend) has always existed — and it belongs
    /// to the phrase: a clip dragged four bars later takes its bends with it, which a lane over
    /// the timeline could not do.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bend: Vec<BendPoint>,
    /// Whether the length above was chosen by hand rather than grown to fit the notes.
    ///
    /// Once it has been, [`Self::fit_length_to_notes`] leaves it alone. Without this a clip
    /// dragged shorter to hide its tail grew straight back on the next note edit, and the
    /// material the user had just trimmed away started sounding again — data resurrecting
    /// itself, with nothing on screen to explain it.
    #[serde(default)]
    pub length_is_explicit: bool,
}

impl MidiClip {
    /// An empty clip.
    pub fn new(id: ClipId, name: impl Into<String>, start: Ticks, length: Ticks) -> Self {
        Self {
            id,
            name: name.into(),
            start,
            length,
            notes: Vec::new(),
            muted: false,
            recipe: None,
            bend: Vec::new(),
            // A new clip's length is a default, not a decision, so notes written past it still
            // grow it. Dragging its edge is what makes it a decision.
            length_is_explicit: false,
        }
    }

    /// `true` when the clip was written by the composer rather than played.
    pub fn is_generated(&self) -> bool {
        self.recipe.is_some()
    }

    /// The notes this clip actually plays, each trimmed to the clip's length.
    ///
    /// A clip is a window onto its notes rather than a container of them: one starting at or past
    /// the end never sounds, and one running past the end is cut off there. Dragging an edge in is
    /// how a clip comes to hold notes it does not play, and dragging it back out brings them back.
    ///
    /// Both the renderer and the MIDI writer ask this, which is why it is here rather than in
    /// either of them. "Which notes are heard" is one rule, and a second copy of it is a file that
    /// exports something other than what plays.
    ///
    /// Positions stay relative to the clip. The caller knows whether it wants them on the
    /// timeline, and only it knows what to add.
    pub fn playable_notes(&self) -> impl Iterator<Item = Note> + '_ {
        self.notes
            .iter()
            .filter(move |note| note.start >= Ticks::ZERO && note.start < self.length)
            .map(move |note| Note {
                length: note.end().min(self.length) - note.start,
                ..*note
            })
    }

    /// Position just past the end of the clip.
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// How far the clip bends at `at`, measured from its own start.
    ///
    /// Straight lines between the points, and the nearest value held flat outside them — the rule
    /// an automation lane already follows, and for the same reason: a curve makes a claim about
    /// the stretch it was written over and none about the rest.
    pub fn bend_at(&self, at: Ticks) -> f32 {
        let Some(first) = self.bend.first() else {
            return 0.0;
        };
        if at <= first.at {
            return first.semitones;
        }
        let Some(last) = self.bend.last() else {
            return 0.0;
        };
        if at >= last.at {
            return last.semitones;
        }
        // The pair `at` falls between. A handful of points per clip, so a walk is the whole of it.
        let (from, to) = match self.bend.windows(2).find(|pair| at < pair[1].at) {
            Some(pair) => (pair[0], pair[1]),
            None => (*first, *last),
        };
        let span = (to.at - from.at).raw().max(1) as f32;
        let through = (at - from.at).raw() as f32 / span;
        from.semitones + (to.semitones - from.semitones) * through
    }

    /// The bend curve sampled into the events an instrument reads, from the clip's own start.
    ///
    /// Every `step` across the stretch the curve was written over, plus the points themselves so
    /// a corner lands exactly where it was drawn. A clip with no bend produces nothing at all,
    /// which is what keeps this off every project that has never used one.
    ///
    /// It **ends at zero** whenever the curve does not. A bend is channel state that an instrument
    /// holds until it is told otherwise, so a clip that finishes two semitones sharp would detune
    /// everything that came after it — including a clip somebody else wrote, on the far side of a
    /// gap, with nothing on screen to say why.
    pub fn bend_events(&self, step: Ticks) -> Vec<(Ticks, f32)> {
        let (Some(first), Some(last)) = (self.bend.first(), self.bend.last()) else {
            return Vec::new();
        };
        let step = Ticks(step.raw().max(1));
        let mut out: Vec<(Ticks, f32)> = Vec::new();
        let mut at = first.at.max_zero();
        while at < last.at {
            out.push((at, self.bend_at(at)));
            at += step;
        }
        out.push((last.at, last.semitones));
        if last.semitones != 0.0 {
            out.push((self.length, 0.0));
        }
        // A point past the end of the clip would be scheduled outside the window the notes were
        // cut to, so the whole curve is kept inside it — including the release above.
        out.retain(|(at, _)| *at <= self.length);
        out
    }

    /// Grows the clip so that every note fits inside it, rounded up to `grid`.
    pub fn fit_length_to_notes(&mut self, grid: Ticks) {
        if self.length_is_explicit {
            return;
        }
        let needed = self
            .notes
            .iter()
            .map(Note::end)
            .max()
            .unwrap_or(Ticks::ZERO);
        if needed > self.length {
            // `i64::div_ceil` is still unstable, and `needed` is never negative here.
            let grid = grid.0.max(1);
            self.length = Ticks((needed.0 + grid - 1) / grid * grid);
        }
    }

    /// Lowest and highest pitch present, for auto-scrolling the piano roll.
    pub fn pitch_range(&self) -> Option<(u8, u8)> {
        let mut iter = self.notes.iter();
        let first = iter.next()?;
        let mut range = (first.pitch, first.pitch);
        for note in iter {
            range.0 = range.0.min(note.pitch);
            range.1 = range.1.max(note.pitch);
        }
        Some(range)
    }
}

/// A reference to decoded audio placed on an audio track.
///
/// Trimming is expressed in *source frames* rather than ticks because the underlying file has a
/// fixed sample rate: a tempo change must move the clip, not resample it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioClip {
    /// Unique within the project.
    pub id: ClipId,
    /// Label shown on the clip.
    pub name: String,
    /// Position on the timeline.
    pub start: Ticks,
    /// Which imported file this clip plays.
    pub source: SourceId,
    /// First source frame to play.
    pub offset_frames: u64,
    /// How many source frames to play.
    pub length_frames: u64,
    /// Clip-level trim, in decibels.
    #[serde(default)]
    pub gain_db: f32,
    /// Fade-in length in frames.
    #[serde(default)]
    pub fade_in_frames: u64,
    /// Fade-out length in frames.
    #[serde(default)]
    pub fade_out_frames: u64,
    /// Whether the clip is skipped during playback.
    #[serde(default)]
    pub muted: bool,
}

impl AudioClip {
    /// A clip playing a whole source from the beginning.
    pub fn new(id: ClipId, name: impl Into<String>, start: Ticks, source: &AudioSource) -> Self {
        Self {
            id,
            name: name.into(),
            start,
            source: source.id,
            offset_frames: 0,
            length_frames: source.frame_count,
            gain_db: 0.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            muted: false,
        }
    }

    /// Gain multiplier for a frame `position` into the clip, including both fades.
    pub fn fade_gain_at(&self, position: u64) -> f32 {
        let mut gain = 1.0f32;
        if self.fade_in_frames > 0 && position < self.fade_in_frames {
            gain *= position as f32 / self.fade_in_frames as f32;
        }
        if self.fade_out_frames > 0 {
            let fade_start = self.length_frames.saturating_sub(self.fade_out_frames);
            if position >= fade_start {
                let into_fade = position - fade_start;
                gain *= 1.0 - (into_fade as f32 / self.fade_out_frames as f32).min(1.0);
            }
        }
        gain
    }
}

/// Metadata about an imported audio file, stored in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSource {
    /// Unique within the project.
    pub id: SourceId,
    /// Display name, usually the file stem.
    pub name: String,
    /// Where the file is, so a project can be re-opened later.
    ///
    /// Normally [`AssetPath::Inside`] once the project has been saved: imported audio belongs to
    /// one song, so it is copied into the project folder and travels with it.
    pub path: AssetPath,
    /// Length of the decoded audio.
    pub frame_count: u64,
    /// Sample rate of the decoded audio; equals the project rate after import resampling.
    pub sample_rate: f64,
    /// Channel count of the decoded audio.
    pub channel_count: usize,
}

/// Decoded audio for every [`AudioSource`], kept out of the serialised project.
///
/// Buffers are shared by `Arc`, so handing the render graph a clip costs a refcount bump rather
/// than a copy of the samples.
#[derive(Clone, Debug, Default)]
pub struct AudioSourceBank {
    buffers: BTreeMap<SourceId, Arc<AudioBuffer>>,
}

impl AudioSourceBank {
    /// An empty bank.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores decoded audio for a source.
    pub fn insert(&mut self, id: SourceId, buffer: Arc<AudioBuffer>) {
        self.buffers.insert(id, buffer);
    }

    /// Looks up decoded audio.
    pub fn get(&self, id: SourceId) -> Option<&Arc<AudioBuffer>> {
        self.buffers.get(&id)
    }

    /// Drops decoded audio for a source.
    pub fn remove(&mut self, id: SourceId) {
        self.buffers.remove(&id);
    }

    /// Every loaded source and its audio, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (SourceId, &Arc<AudioBuffer>)> {
        self.buffers.iter().map(|(id, buffer)| (*id, buffer))
    }

    /// Number of loaded sources.
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// `true` when nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Drops everything not referenced by `keep`.
    pub fn retain(&mut self, keep: impl Fn(SourceId) -> bool) {
        self.buffers.retain(|id, _| keep(*id));
    }
}

/// One effect in a chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    /// Unique within the project.
    pub id: EffectSlotId,
    /// Registry id of the effect to instantiate.
    pub effect_id: String,
    /// Bypass switch. A bypassed effect is still instantiated so its state survives.
    pub enabled: bool,
    /// Saved parameter values.
    pub state: PluginState,
}

impl EffectSlot {
    /// An enabled slot with default parameters.
    pub fn new(id: EffectSlotId, effect_id: impl Into<String>) -> Self {
        Self {
            id,
            effect_id: effect_id.into(),
            enabled: true,
            state: PluginState::empty(),
        }
    }
}

/// Where a track's output goes once its own strip has finished with it.
///
/// Two destinations rather than one field of [`Option<TrackId>`], because "no bus" is not an
/// absence: every track feeds *something*, and the master bus is a real place. A track whose
/// output is a bus is not routed to the master at all — the bus is, on its behalf.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Straight to the master bus, which is where a track starts.
    #[default]
    Master,
    /// Into a bus track, mixed with everything else routed there.
    Bus(TrackId),
}

impl Output {
    /// The bus this names, or `None` for the master.
    pub fn bus(self) -> Option<TrackId> {
        match self {
            Output::Master => None,
            Output::Bus(id) => Some(id),
        }
    }
}

/// A copy of a track's signal, fed to a bus at a level of its own.
///
/// The difference between a send and an [`Output`] is that a send is a *tap*: the track carries on
/// to its own destination as well. That is what makes one reverb shared — six tracks send to it at
/// six different levels and all six are still heard dry.
///
/// A mixing desk calls this an *aux send*, and so does this type, for a reason that has nothing to
/// do with consoles: a document type called `Send` would shadow [`std::marker::Send`] in every file
/// that glob-imports the session's prelude, which is every file in both frontends. The error that
/// produces names a trait nobody mentioned, in a file that never touched the mixer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuxSend {
    /// Unique within the project.
    pub id: SendId,
    /// The bus this feeds.
    pub target: TrackId,
    /// How much of the signal is sent, in decibels.
    pub level_db: f32,
    /// Whether the copy is taken before the track's fader rather than after it.
    ///
    /// After it is the default and is what a reverb wants: pulling a track down should take its
    /// reverb with it. Before it is what a headphone mix wants, where the point is a balance that
    /// does not follow the one in the room.
    #[serde(default)]
    pub pre_fader: bool,
}

impl AuxSend {
    /// A post-fader send at unity.
    pub fn new(id: SendId, target: TrackId) -> Self {
        Self {
            id,
            target,
            level_db: 0.0,
            pre_fader: false,
        }
    }
}

/// Volume, pan, mute/solo and the effect chain for a track or the master bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerStrip {
    /// Fader position in decibels.
    pub gain_db: f32,
    /// Stereo position, -1.0 (left) to 1.0 (right).
    pub pan: f32,
    /// Silences this strip.
    pub mute: bool,
    /// Silences every strip that is not soloed.
    pub solo: bool,
    /// Effects, applied in order before the fader.
    pub effects: Vec<EffectSlot>,
}

impl Default for MixerStrip {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            effects: Vec::new(),
        }
    }
}

/// An instrument track: notes rendered by a software instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentTrack {
    /// Registry id of the instrument.
    pub instrument_id: String,
    /// Saved instrument parameters.
    pub instrument_state: PluginState,
    /// Note clips on the timeline.
    pub clips: Vec<MidiClip>,
}

/// An audio track: references to imported audio.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Audio clips on the timeline.
    pub clips: Vec<AudioClip>,
}

/// What kind of material a track holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackKind {
    /// Notes played by a software instrument.
    Instrument(InstrumentTrack),
    /// Recorded or imported audio.
    Audio(AudioTrack),
    /// A mixing point: whatever is routed here, summed, and put through one strip.
    ///
    /// A bus holds no clips of its own — its material arrives from the tracks that name it, as an
    /// [`Output`] or through a [`AuxSend`]. It is a track rather than a thing of its own so that it
    /// gets a fader, a pan, a mute, an effect chain, a colour and an automation lane without any
    /// of them being written twice.
    Bus,
}

impl TrackKind {
    /// Short label for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            TrackKind::Instrument(_) => "Instrument",
            TrackKind::Audio(_) => "Audio",
            TrackKind::Bus => "Bus",
        }
    }

    /// `true` when this track holds notes rather than audio.
    ///
    /// The two kinds are the one thing that decides whether a clip may move to a track, so the
    /// question is asked directly rather than through a pattern match at each call site.
    pub fn is_instrument(&self) -> bool {
        matches!(self, TrackKind::Instrument(_))
    }

    /// `true` when this track is a mixing point rather than a source of material.
    pub fn is_bus(&self) -> bool {
        matches!(self, TrackKind::Bus)
    }

    /// The instrument track data, when this is one.
    pub fn as_instrument(&self) -> Option<&InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            _ => None,
        }
    }

    /// The instrument track data mutably, when this is one.
    pub fn as_instrument_mut(&mut self) -> Option<&mut InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            _ => None,
        }
    }

    /// The audio track data, when this is one.
    pub fn as_audio(&self) -> Option<&AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            _ => None,
        }
    }

    /// The audio track data mutably, when this is one.
    pub fn as_audio_mut(&mut self) -> Option<&mut AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            _ => None,
        }
    }
}

/// One track in the arrangement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Unique within the project.
    pub id: TrackId,
    /// Name shown in the track header.
    pub name: String,
    /// Tint for the header and its clips.
    pub color: Color,
    /// Height of the lane in the arrangement, in pixels.
    #[serde(default = "default_track_height")]
    pub height: f32,
    /// Instrument or audio content.
    pub kind: TrackKind,
    /// Volume, pan and effects.
    pub mixer: MixerStrip,
    /// Where this track's output goes.
    #[serde(default)]
    pub output: Output,
    /// Copies of this track's signal, fed to buses alongside its own output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<AuxSend>,
}

fn default_track_height() -> f32 {
    72.0
}

impl Track {
    /// Position just past the last clip on this track, in ticks.
    ///
    /// Audio clip lengths depend on the tempo map, so it is passed in.
    pub fn end_tick(&self, tempo_map: &TempoMap, sample_rate: f64) -> Ticks {
        match &self.kind {
            TrackKind::Instrument(track) => track
                .clips
                .iter()
                .map(MidiClip::end)
                .max()
                .unwrap_or(Ticks::ZERO),
            TrackKind::Audio(track) => track
                .clips
                .iter()
                .map(|clip| {
                    // The shared helper, not inline arithmetic: it guards the sample rate, and
                    // a second copy of the conversion is a second place for the guard to be
                    // forgotten — which is exactly how this one came to divide by zero.
                    clip.start
                        + audio_length_ticks(tempo_map, sample_rate, clip.start, clip.length_frames)
                })
                .max()
                .unwrap_or(Ticks::ZERO),
            // A bus holds nothing of its own; what it plays ends when its feeders do.
            TrackKind::Bus => Ticks::ZERO,
        }
    }

    /// Every bus this track feeds: its output when that is one, then each send's target.
    ///
    /// The routing graph's out-edges for one node, in one place, because every question about the
    /// routing — the order to render in, whether an edit would make a loop, how far behind a chain
    /// runs — is a walk over exactly these.
    pub fn feeds(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.output
            .bus()
            .into_iter()
            .chain(self.sends.iter().map(|send| send.target))
    }
}

/// The whole document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Format version, bumped when the schema changes incompatibly.
    #[serde(default = "current_format_version")]
    pub format_version: u32,
    /// Document name.
    pub name: String,
    /// Rate everything renders at.
    pub sample_rate: f64,
    /// Tempo over the timeline.
    pub tempo_map: TempoMap,
    /// The bar/beat grid, over the timeline.
    ///
    /// A map rather than one signature, and beside the tempo map for the same reason the harmony
    /// is: it changes as the song goes on, and at any one moment every track obeys the same one.
    /// Unlike the tempo it reaches nothing that is heard — a meter is notation, and moving the
    /// bar lines moves no note.
    ///
    /// Defaulted rather than required, so a document written before the field existed opens in
    /// 4/4 instead of failing on a missing key. That is not a migration — a version 3 file
    /// written in 3/4 comes up in 4/4, and the version number is why that is allowed to happen —
    /// but every note, clip and chord in it survives, and an honest 4/4 beats a sentence about
    /// serde.
    #[serde(default)]
    pub signatures: SignatureMap,
    /// The key and the chords, over the timeline.
    ///
    /// Beside the tempo map rather than inside a track, because it is the same kind of thing: it
    /// changes as the song goes on, and at any one moment every track obeys the same one.
    #[serde(default)]
    pub harmony: Harmony,
    /// The song's structure over the timeline: イントロ, Aメロ, サビ, each in force until the
    /// next. Beside the harmony because it is the same kind of thing — and the composer reads
    /// it as a hint, so that material generated inside two stretches with the same label is
    /// recognisably the same material.
    #[serde(default)]
    pub sections: crate::structure::SectionMap,
    /// Parameter values that move along the timeline, one lane per automated parameter.
    ///
    /// The fifth timeline map, and the only one that is a collection: there is a curve per
    /// parameter rather than one per project. A parameter with no lane here is not automated at
    /// all and keeps the static value on its strip or in its plugin state, which is what lets a
    /// mix be automated one control at a time.
    #[serde(default)]
    pub automation: crate::automation::Automation,
    /// Tracks, top to bottom.
    pub tracks: Vec<Track>,
    /// Master bus strip.
    pub master: MixerStrip,
    /// Imported file metadata by id.
    pub audio_sources: BTreeMap<SourceId, AudioSource>,
    /// Imported SoundFont metadata by id.
    ///
    /// `default` so a project written before fonts existed still opens, which is the whole reason
    /// every optional field in this document carries one.
    #[serde(default)]
    pub soundfonts: BTreeMap<SoundFontId, SoundFontRef>,
    /// Loop region, when looping is enabled.
    #[serde(default)]
    pub loop_region: Option<(Ticks, Ticks)>,
    /// Whether playback loops over [`Self::loop_region`].
    #[serde(default)]
    pub loop_enabled: bool,
    /// Editing grid size, in ticks.
    #[serde(default = "default_grid")]
    pub grid: Ticks,
    /// The specification a composed document was written from, if it was composed.
    ///
    /// Text rather than a typed field, because the type lives in a crate this one may not name and
    /// because that text is already the canonical way of writing a song specification down. It is
    /// what lets the song sheet refill itself after a save and a reload — otherwise a piece could
    /// be composed, saved, reopened, and there would be no way back to the dials that made it.
    ///
    /// Not a bump: a build that has never heard of this field opens the document, plays every note
    /// of it correctly, and writes it back without the memory. That costs a dialog its history and
    /// nothing that is heard, which is not worth refusing the file over.
    #[serde(default)]
    pub song_spec: Option<String>,
    next_id: u64,
}

fn current_format_version() -> u32 {
    Project::FORMAT_VERSION
}

fn default_grid() -> Ticks {
    Ticks(crate::time::TICKS_PER_QUARTER / 4)
}

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled", 48_000.0)
    }
}

impl Project {
    /// Schema version written into saved files.
    ///
    /// 2 since asset references gained the [`AssetPath::Inside`] form. A version 1 document still
    /// opens — its bare paths are exactly what `External` means — but the reverse cannot work, so
    /// the version has to move for an older build to refuse the file instead of losing its audio.
    ///
    /// 3 since [`ClipPreset`] gained [`Stab`](ClipPreset::Stab). The recipe's new dials carry
    /// backwards on a `serde` default, but a variant an older build has never heard of does not:
    /// it would fail to parse the whole document rather than the one clip, so the version moves to
    /// turn that into the refusal it is.
    ///
    /// 4 since the one `time_signature` became a [`SignatureMap`] over the timeline. The field
    /// changed shape rather than gaining a sibling, so there is nothing for a `serde` default to
    /// carry in either direction: an older build reading a 4/4-throughout document would find an
    /// object where it wanted a signature and give up on the whole file. The version turns that
    /// into a sentence about updating.
    /// 5 since the document gained automation. This one *is* a plain new field with a default,
    /// which normally does not move the version — but the direction that matters here is the
    /// other one. An older build ignores a field it does not know, so it would open an automated
    /// mix, play it at the wrong levels, and write those levels back on the next save, silently
    /// destroying every curve in it. Refusing to open is the only honest answer, and the version
    /// is what produces it.
    ///
    /// 6 since [`TrackKind`] gained [`Bus`](TrackKind::Bus) and a track gained an [`Output`] and
    /// its [`AuxSend`]s. A new variant of a stored enum never carries backwards — an older build
    /// would fail on the whole document rather than the one track — and the fields are worse than
    /// that: they *would* be ignored, so a mix where six tracks feed one reverb would open with
    /// all six routed dry to the master and be saved back that way.
    ///
    /// 7 since [`ClipPreset`] gained [`Kick`](ClipPreset::Kick), [`Snare`](ClipPreset::Snare) and
    /// [`Hat`](ClipPreset::Hat), so that a composed song's drum clips carry a recipe. Another
    /// stored enum, and the same one-way street.
    pub const FORMAT_VERSION: u32 = 7;

    /// An empty project.
    ///
    /// A `sample_rate` that is not a positive finite number becomes 48 kHz: every duration in
    /// the project divides by this field, and storing `inf` or `NaN` would serialise as JSON
    /// `null` — a document that can never be opened again.
    pub fn new(name: impl Into<String>, sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            format_version: Self::FORMAT_VERSION,
            name: name.into(),
            sample_rate,
            tempo_map: TempoMap::constant(120.0),
            signatures: SignatureMap::default(),
            harmony: Harmony::default(),
            sections: crate::structure::SectionMap::default(),
            automation: crate::automation::Automation::new(),
            tracks: Vec::new(),
            master: MixerStrip::default(),
            audio_sources: BTreeMap::new(),
            soundfonts: BTreeMap::new(),
            loop_region: None,
            loop_enabled: false,
            grid: default_grid(),
            song_spec: None,
            next_id: 1,
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Project tempo at the timeline start.
    pub fn bpm(&self) -> f64 {
        self.tempo_map.initial_bpm()
    }

    /// Sets the project tempo at the timeline start.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.tempo_map.set_initial_bpm(bpm);
    }

    /// Appends a track of any kind, routed to the master with no sends.
    fn push_track(&mut self, name: impl Into<String>, kind: TrackKind) -> TrackId {
        let id = TrackId(self.allocate_id());
        let color = Color::from_palette(self.tracks.len());
        self.tracks.push(Track {
            id,
            name: name.into(),
            color,
            height: default_track_height(),
            kind,
            mixer: MixerStrip::default(),
            output: Output::Master,
            sends: Vec::new(),
        });
        id
    }

    /// Appends an instrument track playing `instrument_id`.
    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: impl Into<String>,
    ) -> TrackId {
        self.push_track(
            name,
            TrackKind::Instrument(InstrumentTrack {
                instrument_id: instrument_id.into(),
                instrument_state: PluginState::empty(),
                clips: Vec::new(),
            }),
        )
    }

    /// Appends an empty audio track.
    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        self.push_track(name, TrackKind::Audio(AudioTrack::default()))
    }

    /// Appends a bus, which nothing is routed to yet.
    pub fn add_bus_track(&mut self, name: impl Into<String>) -> TrackId {
        self.push_track(name, TrackKind::Bus)
    }

    /// Copies a track, inserting the copy directly below the original.
    ///
    /// Every nested id is reissued. A shallow clone would leave two clips answering to one
    /// [`ClipId`], and every lookup here returns the *first* match — so an edit aimed at the
    /// copy would silently land on the original.
    pub fn duplicate_track(&mut self, id: TrackId) -> Option<TrackId> {
        let index = self.track_index(id)?;
        // Cloned out first so the id allocator is free to borrow `self` again.
        let mut copy = self.tracks[index].clone();

        copy.id = TrackId(self.allocate_id());
        copy.name = format!("{} copy", copy.name);
        for slot in &mut copy.mixer.effects {
            slot.id = EffectSlotId(self.allocate_id());
        }
        for send in &mut copy.sends {
            send.id = SendId(self.allocate_id());
        }
        match &mut copy.kind {
            TrackKind::Instrument(inner) => {
                for clip in &mut inner.clips {
                    clip.id = ClipId(self.allocate_id());
                }
            }
            TrackKind::Audio(inner) => {
                for clip in &mut inner.clips {
                    clip.id = ClipId(self.allocate_id());
                }
            }
            TrackKind::Bus => {}
        }

        let new_id = copy.id;
        self.tracks.insert(index + 1, copy);
        Some(new_id)
    }

    /// Copies a clip onto its own track, placed immediately after the original.
    ///
    /// Butting the copy up against the original is what makes repeated duplication lay out a
    /// loop, which is the reason the command exists.
    pub fn duplicate_clip(&mut self, id: ClipId) -> Option<ClipId> {
        let new_id = ClipId(self.allocate_id());
        for track in &mut self.tracks {
            match &mut track.kind {
                TrackKind::Instrument(inner) => {
                    if let Some(source) = inner.clips.iter().find(|clip| clip.id == id) {
                        let mut copy = source.clone();
                        copy.id = new_id;
                        copy.start = source.end();
                        inner.clips.push(copy);
                        return Some(new_id);
                    }
                }
                TrackKind::Audio(inner) => {
                    if let Some(index) = inner.clips.iter().position(|clip| clip.id == id) {
                        // The length of an audio clip in ticks depends on the tempo map, which
                        // this loop is holding a mutable borrow across — so measure it up front.
                        let source = inner.clips[index].clone();
                        let start = source.start;
                        let length = audio_length_ticks(
                            &self.tempo_map,
                            self.sample_rate,
                            start,
                            source.length_frames,
                        );
                        let mut copy = source;
                        copy.id = new_id;
                        copy.start = start + length;
                        inner.clips.push(copy);
                        return Some(new_id);
                    }
                }
                TrackKind::Bus => {}
            }
        }
        // Nothing was inserted, so the reserved id is simply never used; ids are only required
        // to be unique, not contiguous.
        None
    }

    /// Splits a clip in two at a timeline position, returning the new right-hand piece.
    ///
    /// Returns `None` when `at` is not strictly inside the clip: a split at either edge would
    /// produce an empty piece, which is a worse outcome than doing nothing.
    pub fn split_clip(&mut self, id: ClipId, at: Ticks) -> Option<ClipId> {
        // Both branches take an owned copy before touching the id allocator: holding a borrow
        // of the original clip across `allocate_id` would borrow `self` twice.
        if let Some((track_id, clip)) = self
            .midi_clip(id)
            .map(|(track, clip)| (track, clip.clone()))
        {
            if at <= clip.start || at >= clip.end() {
                return None;
            }
            let offset = at - clip.start;
            let mut right = clip.clone();
            right.id = ClipId(self.allocate_id());
            right.start = at;
            right.length = clip.end() - at;
            right.notes = split_notes_right(&clip.notes, offset);

            let new_id = right.id;
            let left = self.midi_clip_mut(id)?;
            left.notes = split_notes_left(&clip.notes, offset);
            left.length = offset;
            self.track_mut(track_id)?
                .kind
                .as_instrument_mut()?
                .clips
                .push(right);
            return Some(new_id);
        }

        let (track_id, clip) = self.tracks.iter().find_map(|track| {
            track
                .kind
                .as_audio()?
                .clips
                .iter()
                .find(|clip| clip.id == id)
                .map(|clip| (track.id, clip.clone()))
        })?;
        let end = clip.start + self.audio_clip_length_ticks(&clip);
        if at <= clip.start || at >= end || clip.length_frames < 2 {
            return None;
        }
        // Trimming is expressed in source frames, so the split point goes back through the
        // tempo map rather than being stored as a tick.
        let seconds =
            self.tempo_map.ticks_to_seconds(at).0 - self.tempo_map.ticks_to_seconds(clip.start).0;
        let frames = ((seconds * self.sample_rate) as u64).clamp(1, clip.length_frames - 1);

        let mut right = clip.clone();
        right.id = ClipId(self.allocate_id());
        right.start = at;
        right.offset_frames = clip.offset_frames + frames;
        right.length_frames = clip.length_frames - frames;
        // Each fade belongs to the edge it was drawn on: the left piece keeps the fade-in, the
        // right piece keeps the fade-out, and neither inherits a fade at the cut — a fade there
        // would be an artefact the user never asked for.
        right.fade_in_frames = 0;
        right.fade_out_frames = clip.fade_out_frames.min(right.length_frames);

        let new_id = right.id;
        let left = self.audio_clip_mut(id)?;
        left.length_frames = frames;
        left.fade_out_frames = 0;
        left.fade_in_frames = left.fade_in_frames.min(frames);
        self.track_mut(track_id)?
            .kind
            .as_audio_mut()?
            .clips
            .push(right);
        Some(new_id)
    }

    /// Length of an audio clip on the musical timeline.
    ///
    /// A clip's trim is in source frames, so its length in ticks depends on where it sits: the
    /// same number of frames spans fewer ticks in a faster passage.
    pub fn audio_clip_length_ticks(&self, clip: &AudioClip) -> Ticks {
        audio_length_ticks(
            &self.tempo_map,
            self.sample_rate,
            clip.start,
            clip.length_frames,
        )
    }

    /// Removes a track, returning `true` when it existed.
    pub fn remove_track(&mut self, id: TrackId) -> bool {
        let before = self.tracks.len();
        self.tracks.retain(|track| track.id != id);
        let removed = self.tracks.len() != before;
        if removed {
            // Its automation leaves with it. A lane left behind names a track that is not there,
            // and ids are handed out again — so it would come back to life driving a parameter on
            // whichever track was created next.
            self.automation.remove_track(id);
            // And so does everything that fed it. A deleted bus takes the *routing* with it, not
            // the tracks: what was going through it goes straight to the master instead, which is
            // where it would have been had the bus never existed.
            for track in &mut self.tracks {
                if track.output == Output::Bus(id) {
                    track.output = Output::Master;
                }
                track.sends.retain(|send| send.target != id);
            }
        }
        removed
    }

    /// Moves a track to a new index, clamping into range.
    pub fn move_track(&mut self, id: TrackId, to_index: usize) {
        let Some(from) = self.track_index(id) else {
            return;
        };
        let to = to_index.min(self.tracks.len().saturating_sub(1));
        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
    }

    /// Index of a track by id.
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|track| track.id == id)
    }

    /// A track by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// A track by id, mutably.
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }

    /// Adds a MIDI clip to an instrument track.
    pub fn add_midi_clip(
        &mut self,
        track_id: TrackId,
        name: impl Into<String>,
        start: Ticks,
        length: Ticks,
    ) -> Option<ClipId> {
        let id = ClipId(self.allocate_id());
        let name = name.into();
        let track = self.track_mut(track_id)?;
        let instrument = track.kind.as_instrument_mut()?;
        instrument
            .clips
            .push(MidiClip::new(id, name, start, length));
        Some(id)
    }

    /// Adds an audio clip referencing an already-registered source.
    pub fn add_audio_clip(
        &mut self,
        track_id: TrackId,
        source_id: SourceId,
        start: Ticks,
    ) -> Option<ClipId> {
        let source = self.audio_sources.get(&source_id)?.clone();
        let id = ClipId(self.allocate_id());
        let track = self.track_mut(track_id)?;
        let audio = track.kind.as_audio_mut()?;
        audio
            .clips
            .push(AudioClip::new(id, source.name.clone(), start, &source));
        Some(id)
    }

    /// The font already referring to `file`, resolved against the folder holding the document.
    ///
    /// Resolution rather than string comparison, because the same file can be named two ways:
    /// `Audio/GM.sf2` inside a collected project and an absolute path to the same bytes. Only
    /// this can tell that they are one font.
    pub fn soundfont_at(&self, project_folder: Option<&Path>, file: &Path) -> Option<SoundFontId> {
        self.soundfonts
            .values()
            .find(|font| font.path.resolve(project_folder).as_deref() == Some(file))
            .map(|font| font.id)
    }

    /// Registers an imported SoundFont and returns its new id.
    ///
    /// A font already referred to the same way is returned rather than added again: importing the
    /// same file twice is something a person does by accident, and the cost of not noticing is a
    /// second copy of a very large object in memory. That check is on the stored reference, so
    /// callers that can resolve paths should ask [`Self::soundfont_at`] first.
    pub fn add_soundfont(
        &mut self,
        name: impl Into<String>,
        path: AssetPath,
        byte_size: u64,
    ) -> SoundFontId {
        if let Some(existing) = self
            .soundfonts
            .values()
            .find(|font| font.path == path)
            .map(|font| font.id)
        {
            return existing;
        }
        let id = SoundFontId(self.allocate_id());
        self.soundfonts.insert(
            id,
            SoundFontRef {
                id,
                name: name.into(),
                path,
                byte_size,
            },
        );
        id
    }

    /// Registers imported file metadata and returns its new id.
    pub fn add_audio_source(
        &mut self,
        name: impl Into<String>,
        path: AssetPath,
        frame_count: u64,
        sample_rate: f64,
        channel_count: usize,
    ) -> SourceId {
        let id = SourceId(self.allocate_id());
        self.audio_sources.insert(
            id,
            AudioSource {
                id,
                name: name.into(),
                path,
                frame_count,
                sample_rate,
                channel_count,
            },
        );
        id
    }

    /// Adds an effect to a track's chain, or to the master bus when `track_id` is `None`.
    pub fn add_effect(
        &mut self,
        track_id: Option<TrackId>,
        effect_id: impl Into<String>,
    ) -> Option<EffectSlotId> {
        let slot_id = EffectSlotId(self.allocate_id());
        let strip = match track_id {
            Some(id) => &mut self.track_mut(id)?.mixer,
            None => &mut self.master,
        };
        strip.effects.push(EffectSlot::new(slot_id, effect_id));
        Some(slot_id)
    }

    /// Removes an effect slot from anywhere in the project.
    pub fn remove_effect(&mut self, slot_id: EffectSlotId) -> bool {
        let mut removed = false;
        for strip in self
            .tracks
            .iter_mut()
            .map(|track| &mut track.mixer)
            .chain(std::iter::once(&mut self.master))
        {
            let before = strip.effects.len();
            strip.effects.retain(|slot| slot.id != slot_id);
            removed |= strip.effects.len() != before;
        }
        if removed {
            // Same reason a deleted track drops its lanes: the slot id outlives nothing, and a
            // curve addressed to a chain position that no longer exists is a curve that will
            // eventually be applied to whatever lands there.
            self.automation.remove_lanes_where(|target| {
                matches!(target, crate::param::ParamTarget::Effect { slot, .. } if slot == slot_id)
            });
        }
        removed
    }

    /// Removes a send from a track, returning `true` when it was there.
    ///
    /// Its automation goes with it, for the reason a deleted track's and a deleted effect slot's
    /// do: a lane addressed to a send that no longer exists would come back to life driving
    /// whichever send is created next.
    pub fn remove_send(&mut self, track: TrackId, send: SendId) -> bool {
        let Some(entry) = self.track_mut(track) else {
            return false;
        };
        let before = entry.sends.len();
        entry.sends.retain(|existing| existing.id != send);
        let removed = entry.sends.len() != before;
        if removed {
            self.automation.remove_lanes_where(|target| {
                matches!(target, crate::param::ParamTarget::Send { send: id, .. } if id == send)
            });
        }
        removed
    }

    /// A MIDI clip anywhere in the project.
    pub fn midi_clip(&self, clip_id: ClipId) -> Option<(TrackId, &MidiClip)> {
        self.tracks.iter().find_map(|track| {
            track
                .kind
                .as_instrument()?
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
                .map(|clip| (track.id, clip))
        })
    }

    /// A MIDI clip anywhere in the project, mutably.
    pub fn midi_clip_mut(&mut self, clip_id: ClipId) -> Option<&mut MidiClip> {
        self.tracks.iter_mut().find_map(|track| {
            track
                .kind
                .as_instrument_mut()?
                .clips
                .iter_mut()
                .find(|clip| clip.id == clip_id)
        })
    }

    /// An audio clip anywhere in the project.
    pub fn audio_clip(&self, clip_id: ClipId) -> Option<&AudioClip> {
        self.tracks.iter().find_map(|track| {
            track
                .kind
                .as_audio()?
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
        })
    }

    /// An audio clip anywhere in the project, mutably.
    pub fn audio_clip_mut(&mut self, clip_id: ClipId) -> Option<&mut AudioClip> {
        self.tracks.iter_mut().find_map(|track| {
            track
                .kind
                .as_audio_mut()?
                .clips
                .iter_mut()
                .find(|clip| clip.id == clip_id)
        })
    }

    /// Removes a clip of either kind, returning `true` when it existed.
    pub fn remove_clip(&mut self, clip_id: ClipId) -> bool {
        for track in &mut self.tracks {
            match &mut track.kind {
                TrackKind::Instrument(inner) => {
                    let before = inner.clips.len();
                    inner.clips.retain(|clip| clip.id != clip_id);
                    if inner.clips.len() != before {
                        return true;
                    }
                }
                TrackKind::Audio(inner) => {
                    let before = inner.clips.len();
                    inner.clips.retain(|clip| clip.id != clip_id);
                    if inner.clips.len() != before {
                        return true;
                    }
                }
                TrackKind::Bus => {}
            }
        }
        false
    }

    /// Which track a clip of either kind sits on.
    pub fn track_of_clip(&self, clip_id: ClipId) -> Option<TrackId> {
        self.tracks
            .iter()
            .find(|track| match &track.kind {
                TrackKind::Instrument(inner) => inner.clips.iter().any(|clip| clip.id == clip_id),
                TrackKind::Audio(inner) => inner.clips.iter().any(|clip| clip.id == clip_id),
                TrackKind::Bus => false,
            })
            .map(|track| track.id)
    }

    /// Moves a clip onto another track, keeping its position and its id.
    ///
    /// Only between tracks of the same kind: a block of notes has no meaning on an audio track
    /// and a reference to a decoded file has none on an instrument track. Returns `false` when
    /// the clip or the track does not exist, or when the two do not match — the caller is
    /// expected to have checked, and a silent no-op is better than a half-moved document.
    ///
    /// Moving a clip to the track it is already on succeeds and changes nothing, which is what
    /// lets a pointer drag call this on every move without special-casing the common one.
    pub fn move_clip_to_track(&mut self, clip_id: ClipId, track_id: TrackId) -> bool {
        let Some(source) = self.track_of_clip(clip_id) else {
            return false;
        };
        if source == track_id {
            return self.track(track_id).is_some();
        }
        let Some(destination) = self.track_index(track_id) else {
            return false;
        };
        let Some(origin) = self.track_index(source) else {
            return false;
        };

        // A bus holds no clips, so it is neither a source nor a destination for one. Asked here
        // rather than left to the arms below, where "not an instrument" would have counted a bus
        // as an audio track: the clip would have been taken off its own track and then found
        // nowhere to land.
        if self.tracks[origin].kind.is_bus() || self.tracks[destination].kind.is_bus() {
            return false;
        }

        // Taken out only once the destination is known to accept it, so a refused move leaves the
        // document exactly as it was rather than losing the clip.
        match (
            self.tracks[origin].kind.is_instrument(),
            self.tracks[destination].kind.is_instrument(),
        ) {
            (true, true) => {
                let Some(inner) = self.tracks[origin].kind.as_instrument_mut() else {
                    return false;
                };
                let Some(at) = inner.clips.iter().position(|clip| clip.id == clip_id) else {
                    return false;
                };
                let clip = inner.clips.remove(at);
                if let Some(inner) = self.tracks[destination].kind.as_instrument_mut() {
                    inner.clips.push(clip);
                    inner.clips.sort_by_key(|clip| clip.start);
                }
                true
            }
            (false, false) => {
                let Some(inner) = self.tracks[origin].kind.as_audio_mut() else {
                    return false;
                };
                let Some(at) = inner.clips.iter().position(|clip| clip.id == clip_id) else {
                    return false;
                };
                let clip = inner.clips.remove(at);
                if let Some(inner) = self.tracks[destination].kind.as_audio_mut() {
                    inner.clips.push(clip);
                    inner.clips.sort_by_key(|clip| clip.start);
                }
                true
            }
            _ => false,
        }
    }

    /// `true` when any track is soloed, meaning non-soloed tracks must be silenced.
    pub fn has_solo(&self) -> bool {
        self.tracks.iter().any(|track| track.mixer.solo)
    }

    /// `true` when this track should be audible given the current mute/solo state.
    ///
    /// A convenience over [`Self::solo_resolution`] for a caller asking about one track; anything
    /// asking about all of them should call that instead and read the answers out of the vector,
    /// because a bus's answer depends on every track feeding it.
    pub fn track_is_audible(&self, track: &Track) -> bool {
        match self.track_index(track.id) {
            Some(index) => !track.mixer.mute && self.solo_resolution()[index],
            None => !track.mixer.mute && (!self.has_solo() || track.mixer.solo),
        }
    }

    /// Track indices ordered so that everything feeding a bus comes before the bus.
    ///
    /// This is the order audio has to be mixed in: a bus cannot be put through its own strip until
    /// everything routed into it has arrived. Every track appears exactly once even if the routing
    /// somehow holds a loop — see [`Self::repair_routing`] for why it should not — so a caller can
    /// walk this instead of the track list and know it has covered the project.
    pub fn routing_order(&self) -> Vec<usize> {
        // A depth-first post-order over the out-edges puts each node after everything it feeds;
        // reversing that puts it before them, which is the order wanted. A back edge is stepped
        // over rather than followed, so a loop costs an arbitrary starting point and not a hang.
        #[derive(Copy, Clone, PartialEq)]
        enum Mark {
            Unseen,
            Walking,
            Done,
        }
        let mut mark = vec![Mark::Unseen; self.tracks.len()];
        let mut order = Vec::with_capacity(self.tracks.len());
        // An explicit stack rather than recursion: the depth is the length of a routing chain,
        // which a document is free to make as long as it has tracks.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for start in 0..self.tracks.len() {
            if mark[start] != Mark::Unseen {
                continue;
            }
            mark[start] = Mark::Walking;
            stack.push((start, 0));
            while let Some((node, edge)) = stack.pop() {
                match self.tracks[node].feeds().nth(edge) {
                    Some(target) => {
                        stack.push((node, edge + 1));
                        if let Some(next) = self.track_index(target)
                            && mark[next] == Mark::Unseen
                        {
                            mark[next] = Mark::Walking;
                            stack.push((next, 0));
                        }
                    }
                    None => {
                        mark[node] = Mark::Done;
                        order.push(node);
                    }
                }
            }
        }
        order.reverse();
        order
    }

    /// `true` when routing `from` into `to` would make a signal loop back on itself.
    ///
    /// Asked before an output is changed or a send is added, because a loop is not a strange mix —
    /// it is a bus waiting for itself, and there is no order to render it in.
    pub fn routing_would_cycle(&self, from: TrackId, to: TrackId) -> bool {
        if from == to {
            return true;
        }
        // The new edge closes a loop exactly when `from` is already downstream of `to`.
        let mut seen = vec![to];
        let mut queue = vec![to];
        while let Some(node) = queue.pop() {
            let Some(track) = self.track(node) else {
                continue;
            };
            for next in track.feeds() {
                if next == from {
                    return true;
                }
                if !seen.contains(&next) {
                    seen.push(next);
                    queue.push(next);
                }
            }
        }
        false
    }

    /// Points every output and every send at a bus that exists, breaking any loop it finds.
    ///
    /// Called when a document is loaded, for the same reason [`Self::repair_id_counter`] is: the
    /// editing commands refuse to create either fault, so a file carrying one was either written
    /// by another tool or edited by hand, and the alternative to repairing it is a project that
    /// cannot be rendered at all. Returns `true` when anything had to be changed.
    pub fn repair_routing(&mut self) -> bool {
        let is_bus: Vec<(TrackId, bool)> = self
            .tracks
            .iter()
            .map(|track| (track.id, track.kind.is_bus()))
            .collect();
        let usable = |id: TrackId, owner: TrackId| {
            id != owner && is_bus.iter().any(|(bus, yes)| *bus == id && *yes)
        };

        let mut repaired = false;
        for index in 0..self.tracks.len() {
            let owner = self.tracks[index].id;
            if let Output::Bus(target) = self.tracks[index].output
                && !usable(target, owner)
            {
                self.tracks[index].output = Output::Master;
                repaired = true;
            }
            let before = self.tracks[index].sends.len();
            self.tracks[index]
                .sends
                .retain(|send| usable(send.target, owner));
            repaired |= self.tracks[index].sends.len() != before;
        }

        // Now every edge points at a real bus, so what is left is loops. Each edge is put back one
        // at a time and dropped if it closes one, which terminates and does not depend on where in
        // the document the loop happened to start.
        let edges: Vec<(usize, Output, Vec<AuxSend>)> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (index, track.output, track.sends.clone()))
            .collect();
        for track in &mut self.tracks {
            track.output = Output::Master;
            track.sends.clear();
        }
        for (index, output, sends) in edges {
            let owner = self.tracks[index].id;
            if let Output::Bus(target) = output {
                if self.routing_would_cycle(owner, target) {
                    repaired = true;
                } else {
                    self.tracks[index].output = output;
                }
            }
            for send in sends {
                if self.routing_would_cycle(owner, send.target) {
                    repaired = true;
                } else {
                    self.tracks[index].sends.push(send);
                }
            }
        }
        repaired
    }

    /// Which tracks the solo switches leave audible, in project order.
    ///
    /// This is the solo resolution alone; a track's own mute is separate, because the engine keeps
    /// the two apart so that toggling a mute is a command rather than a rebuild.
    ///
    /// A bus is what makes this more than one line. Solo has to travel **both ways along the
    /// routing**, or half of what a person means by it produces silence:
    ///
    /// * *Downstream*, because soloing a drum track routed through a drum bus must leave the bus
    ///   open — the track's audio has nowhere else to go.
    /// * *Upstream*, because soloing the drum bus must leave the drum tracks open — a bus has
    ///   nothing of its own to play, so a soloed bus with silenced feeders is silence.
    ///
    /// So a track is audible exactly when it lies on a path through something soloed. Two passes
    /// over [`Self::routing_order`] decide it: one forward for what a soloed track feeds, one
    /// backward for what feeds a soloed track. They stay separate on purpose — merging them would
    /// let a bus made audible from below drag in its *other* feeders, and soloing one drum track
    /// would quietly play the whole kit.
    pub fn solo_resolution(&self) -> Vec<bool> {
        if !self.has_solo() {
            return vec![true; self.tracks.len()];
        }
        let order = self.routing_order();
        let index_of = |id: TrackId| self.track_index(id);

        // Backward: `reaches[t]` is true when something soloed sits downstream of `t`.
        let mut reaches = vec![false; self.tracks.len()];
        for &index in order.iter().rev() {
            reaches[index] = self.tracks[index].feeds().any(|target| {
                index_of(target)
                    .is_some_and(|target| self.tracks[target].mixer.solo || reaches[target])
            });
        }

        // Forward: `fed[t]` is true when something soloed sits upstream of `t`.
        let mut fed = vec![false; self.tracks.len()];
        for &index in &order {
            let id = self.tracks[index].id;
            fed[index] = self.tracks.iter().enumerate().any(|(feeder, other)| {
                (other.mixer.solo || fed[feeder]) && other.feeds().any(|target| target == id)
            });
        }

        (0..self.tracks.len())
            .map(|index| self.tracks[index].mixer.solo || reaches[index] || fed[index])
            .collect()
    }

    /// Position just past the last clip in the project.
    pub fn end_tick(&self) -> Ticks {
        self.tracks
            .iter()
            .map(|track| track.end_tick(&self.tempo_map, self.sample_rate))
            .max()
            .unwrap_or(Ticks::ZERO)
    }

    /// Total length in seconds, ignoring effect tails.
    pub fn duration_seconds(&self) -> f64 {
        self.tempo_map.ticks_to_seconds(self.end_tick()).0
    }

    /// Reserves an id from the project's counter, for callers that build objects themselves.
    pub fn next_clip_id(&mut self) -> ClipId {
        ClipId(self.allocate_id())
    }

    /// Reserves an effect slot id.
    pub fn next_effect_slot_id(&mut self) -> EffectSlotId {
        EffectSlotId(self.allocate_id())
    }

    /// Reserves a send id.
    pub fn next_send_id(&mut self) -> SendId {
        SendId(self.allocate_id())
    }

    /// Repairs a project loaded from disk: makes sure the id counter is past every id in use,
    /// so ids handed out later cannot collide with existing ones.
    pub fn repair_id_counter(&mut self) {
        let mut highest = 0u64;
        for track in &self.tracks {
            highest = highest.max(track.id.0);
            for slot in &track.mixer.effects {
                highest = highest.max(slot.id.0);
            }
            for send in &track.sends {
                highest = highest.max(send.id.0);
            }
            match &track.kind {
                TrackKind::Instrument(inner) => {
                    for clip in &inner.clips {
                        highest = highest.max(clip.id.0);
                    }
                }
                TrackKind::Audio(inner) => {
                    for clip in &inner.clips {
                        highest = highest.max(clip.id.0);
                    }
                }
                TrackKind::Bus => {}
            }
        }
        for slot in &self.master.effects {
            highest = highest.max(slot.id.0);
        }
        for id in self.audio_sources.keys() {
            highest = highest.max(id.0);
        }
        for id in self.soundfonts.keys() {
            highest = highest.max(id.0);
        }
        self.next_id = self.next_id.max(highest + 1);
    }
}

/// Length of `length_frames` source frames, placed at `start`, measured in ticks.
fn audio_length_ticks(
    tempo_map: &TempoMap,
    sample_rate: f64,
    start: Ticks,
    length_frames: u64,
) -> Ticks {
    let rate = sample_rate.max(1.0);
    let start_seconds = tempo_map.ticks_to_seconds(start).0;
    let end_seconds = start_seconds + length_frames as f64 / rate;
    tempo_map.seconds_to_ticks(crate::time::Seconds(end_seconds)) - start
}

/// The notes left of a split at `offset`, clip-relative.
///
/// A note straddling the cut is truncated rather than dropped: the split is a timeline edit,
/// and losing the sounding half of a held note is not what anyone means by it.
fn split_notes_left(notes: &[Note], offset: Ticks) -> Vec<Note> {
    notes
        .iter()
        .filter(|note| note.start < offset)
        .map(|note| Note {
            length: (offset - note.start).min(note.length),
            ..*note
        })
        .collect()
}

/// The notes of a clip whose front has been trimmed by `by`, rebased onto the new start.
///
/// The same rule a split's right half follows, and the same function would do — this one exists
/// so the name says what the caller means. A note the trim runs through keeps its sounding half
/// rather than vanishing, and one entirely before the new start is gone: that is what trimming
/// the front of a region is, and undo is what puts it back.
pub fn notes_trimmed_from_front(notes: &[Note], by: Ticks) -> Vec<Note> {
    split_notes_right(notes, by)
}

/// The notes right of a split at `offset`, rebased so they are relative to the new clip.
fn split_notes_right(notes: &[Note], offset: Ticks) -> Vec<Note> {
    notes
        .iter()
        .filter(|note| note.end() > offset)
        .map(|note| {
            let start = note.start.max(offset);
            Note {
                start: start - offset,
                length: note.end() - start,
                ..*note
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::TICKS_PER_QUARTER;

    fn bent(points: &[(i64, f32)], length: i64) -> MidiClip {
        MidiClip {
            bend: points
                .iter()
                .map(|(at, semitones)| BendPoint {
                    at: Ticks(*at),
                    semitones: *semitones,
                })
                .collect(),
            ..MidiClip::new(ClipId(1), "bent", Ticks::ZERO, Ticks(length))
        }
    }

    #[test]
    fn a_bend_runs_in_straight_lines_and_holds_flat_outside_itself() {
        // The rule an automation lane already follows, and for the same reason: a curve makes a
        // claim about the stretch it was written over and none at all about the rest.
        let clip = bent(&[(480, 0.0), (960, 2.0)], 1920);
        assert_eq!(clip.bend_at(Ticks(0)), 0.0, "flat before the first point");
        assert_eq!(clip.bend_at(Ticks(480)), 0.0);
        assert!((clip.bend_at(Ticks(720)) - 1.0).abs() < 0.001, "halfway up");
        assert_eq!(clip.bend_at(Ticks(960)), 2.0);
        assert_eq!(clip.bend_at(Ticks(1900)), 2.0, "and flat after the last");

        // A clip nobody has bent is not bent, which is what keeps this off every project that has
        // never used one.
        assert_eq!(bent(&[], 1920).bend_at(Ticks(500)), 0.0);
        assert!(bent(&[], 1920).bend_events(BEND_STEP).is_empty());
    }

    #[test]
    fn a_bend_comes_back_to_nothing_before_the_clip_ends() {
        // A bend is channel state an instrument holds until it is told otherwise. A clip that
        // finished two semitones sharp would detune everything after it — including a clip
        // somebody else wrote, on the far side of a gap, with nothing on screen to say why.
        let events = bent(&[(0, 0.0), (960, 2.0)], 1920).bend_events(BEND_STEP);
        let (last_at, last_value) = *events.last().expect("it was bent");
        assert_eq!(last_value, 0.0, "the bend was left hanging");
        assert_eq!(last_at, Ticks(1920), "and it is released at the clip's end");

        // A curve that already ends at nothing needs no such release.
        let settled = bent(&[(0, 2.0), (960, 0.0)], 1920).bend_events(BEND_STEP);
        assert_eq!(settled.last().map(|(at, _)| *at), Some(Ticks(960)));

        // Nothing is scheduled past the window the notes were cut to.
        let past = bent(&[(0, 1.0), (4000, 2.0)], 960).bend_events(BEND_STEP);
        assert!(
            past.iter().all(|(at, _)| *at <= Ticks(960)),
            "a point ran past the clip: {past:?}"
        );
    }

    #[test]
    fn a_bend_is_sampled_finely_enough_to_hear_as_a_slide() {
        // Every step across the stretch the curve covers, so a rise sounds continuous rather than
        // as a staircase — and the corners land exactly where they were drawn.
        let events = bent(&[(0, 0.0), (960, 2.0)], 1920).bend_events(BEND_STEP);
        let across = events.iter().filter(|(at, _)| *at <= Ticks(960)).count();
        assert!(across >= 20, "half a bar gave only {across} events");
        assert!(
            events
                .iter()
                .any(|(at, value)| *at == Ticks(960) && *value == 2.0)
        );
        // In time order, which is what the scheduler sorts against.
        for pair in events.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "{events:?} goes back in time");
        }
    }

    #[test]
    fn a_rate_that_is_not_a_rate_becomes_the_default() {
        // Every duration in a project divides by this field, and a non-finite value would
        // serialise as JSON null — a document that can never be opened again.
        for bad in [0.0, -44_100.0, f64::NAN, f64::INFINITY] {
            let project = Project::new("Bad", bad);
            assert_eq!(project.sample_rate, 48_000.0, "for {bad}");
        }
        assert_eq!(Project::new("Good", 44_100.0).sample_rate, 44_100.0);
    }

    fn demo_project() -> Project {
        let mut project = Project::new("Demo", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.pulse");
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
        midi.notes
            .push(Note::new(64, Ticks::QUARTER, Ticks::QUARTER));
        project
    }

    #[test]
    fn a_clip_moves_to_another_track_of_its_own_kind() {
        let mut project = demo_project();
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        let source = project.tracks[0].id;
        let destination = project.add_instrument_track("Second", "auris.synth.pulse");

        assert!(project.move_clip_to_track(clip, destination));
        assert_eq!(project.track_of_clip(clip), Some(destination));
        assert!(
            project
                .track(source)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .clips
                .is_empty(),
            "the clip is still on the track it left"
        );
        // Its id, position and contents survive: this is a move, not a copy and a delete.
        let moved = project.midi_clip(clip).unwrap().1;
        assert_eq!(moved.start, Ticks::ZERO);
        assert_eq!(moved.notes.len(), 2);
    }

    #[test]
    fn a_clip_cannot_move_to_a_track_of_the_other_kind() {
        // A block of notes has no meaning on an audio track. Refusing has to leave the document
        // exactly as it was rather than losing the clip on the way across.
        let mut project = demo_project();
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        let audio = project.add_audio_track("Sample");
        let before = project.clone();

        assert!(!project.move_clip_to_track(clip, audio));
        assert_eq!(project, before, "a refused move changed the document");
    }

    #[test]
    fn moving_a_clip_to_the_track_it_is_on_succeeds_and_changes_nothing() {
        // A pointer drag asks for this on most of its moves, so it must not be an error.
        let mut project = demo_project();
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        let track = project.tracks[0].id;
        let before = project.clone();

        assert!(project.move_clip_to_track(clip, track));
        assert_eq!(project, before);
    }

    #[test]
    fn moving_a_clip_that_does_not_exist_is_refused() {
        let mut project = demo_project();
        let track = project.tracks[0].id;
        assert!(!project.move_clip_to_track(ClipId(9_999), track));
        assert!(!project.move_clip_to_track(ClipId(9_999), TrackId(9_999)));
    }

    #[test]
    fn a_moved_clip_lands_in_timeline_order() {
        // The lanes are drawn from the clip list in order, so an arrival at the end of it would
        // paint over whatever it overlaps rather than under.
        let mut project = demo_project();
        let first = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        let destination = project.add_instrument_track("Second", "auris.synth.pulse");
        project
            .add_midi_clip(destination, "Later", Ticks::from_beats(8.0), Ticks::QUARTER)
            .unwrap();

        assert!(project.move_clip_to_track(first, destination));
        let starts: Vec<Ticks> = project
            .track(destination)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips
            .iter()
            .map(|clip| clip.start)
            .collect();
        assert_eq!(starts, vec![Ticks::ZERO, Ticks::from_beats(8.0)]);
    }

    /// Two instrument tracks routed into one bus, which goes to the master.
    fn bussed_project() -> (Project, TrackId, TrackId, TrackId) {
        let mut project = Project::new("Routing", 48_000.0);
        let kick = project.add_instrument_track("Kick", "auris.synth.pulse");
        let snare = project.add_instrument_track("Snare", "auris.synth.pulse");
        let bus = project.add_bus_track("Drums");
        project.track_mut(kick).unwrap().output = Output::Bus(bus);
        project.track_mut(snare).unwrap().output = Output::Bus(bus);
        (project, kick, snare, bus)
    }

    #[test]
    fn a_bus_is_ordered_after_everything_that_feeds_it() {
        // The order audio has to be mixed in: a bus cannot go through its own strip until every
        // track routed into it has arrived.
        let (mut project, kick, snare, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        let send = project.next_send_id();
        project
            .track_mut(bus)
            .unwrap()
            .sends
            .push(AuxSend::new(send, reverb));

        let order = project.routing_order();
        assert_eq!(order.len(), project.tracks.len());
        let at = |id: TrackId| {
            let index = project.track_index(id).unwrap();
            order.iter().position(|slot| *slot == index).unwrap()
        };
        assert!(at(kick) < at(bus));
        assert!(at(snare) < at(bus));
        assert!(at(bus) < at(reverb), "a send is a routing edge too");
    }

    #[test]
    fn routing_refuses_to_close_a_loop() {
        let (mut project, kick, _, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        project
            .track_mut(bus)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(500), reverb));

        // A bus into itself, and the two ways round the existing chain.
        assert!(project.routing_would_cycle(bus, bus));
        assert!(
            project.routing_would_cycle(reverb, bus),
            "reverb -> drums -> reverb"
        );
        assert!(
            project.routing_would_cycle(reverb, kick),
            "kick is upstream"
        );
        // And the edges that are simply new.
        assert!(!project.routing_would_cycle(kick, reverb));
    }

    #[test]
    fn a_document_carrying_a_loop_is_repaired_rather_than_refused() {
        // The editing commands cannot make one, so a file with a loop came from somewhere else.
        // The alternative to repairing it is a project that can never be rendered at all.
        let (mut project, _, _, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        project.track_mut(bus).unwrap().output = Output::Bus(reverb);
        project.track_mut(reverb).unwrap().output = Output::Bus(bus);

        assert!(project.repair_routing());
        // One of the two edges survives — which one does not matter, only that no track can now
        // reach itself, which is what makes an order to render in exist.
        for track in &project.tracks {
            for target in track.feeds() {
                assert!(
                    !project.routing_would_cycle(track.id, target),
                    "{} still lies on a loop",
                    track.name
                );
            }
        }
        assert!(!project.repair_routing(), "the repair is idempotent");
        // And the bus is still reachable at all: repairing must not detach the whole chain.
        assert!(project.track(bus).unwrap().feeds().count() <= 1);
    }

    #[test]
    fn a_route_to_something_that_is_not_a_bus_is_dropped_on_load() {
        let (mut project, kick, snare, _) = bussed_project();
        // An instrument track is not a mixing point, and neither is a track that is not there.
        project.track_mut(kick).unwrap().output = Output::Bus(snare);
        project
            .track_mut(snare)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(9), TrackId(9_999)));

        assert!(project.repair_routing());
        assert_eq!(project.track(kick).unwrap().output, Output::Master);
        assert!(project.track(snare).unwrap().sends.is_empty());
    }

    #[test]
    fn solo_travels_both_ways_along_the_routing() {
        let (mut project, kick, snare, bus) = bussed_project();

        // Downstream: soloing a track has to leave the bus it feeds open, or its audio has
        // nowhere to go and the solo is silence.
        project.track_mut(kick).unwrap().mixer.solo = true;
        let audible = project.solo_resolution();
        let at = |id: TrackId| audible[project.track_index(id).unwrap()];
        assert!(at(kick));
        assert!(at(bus), "the soloed track's own bus was silenced");
        assert!(!at(snare));

        // Upstream: soloing the bus has to leave its feeders open, because a bus has nothing of
        // its own to play.
        project.track_mut(kick).unwrap().mixer.solo = false;
        project.track_mut(bus).unwrap().mixer.solo = true;
        let audible = project.solo_resolution();
        let at = |id: TrackId| audible[project.track_index(id).unwrap()];
        assert!(at(kick) && at(snare) && at(bus));
    }

    #[test]
    fn nothing_soloed_leaves_everything_audible() {
        let (project, _, _, _) = bussed_project();
        assert_eq!(project.solo_resolution(), vec![true; project.tracks.len()]);
    }

    #[test]
    fn deleting_a_bus_sends_what_fed_it_straight_to_the_master() {
        // The routing goes, not the tracks: what was going through the bus goes where it would
        // have gone had the bus never existed.
        let (mut project, kick, snare, bus) = bussed_project();
        project
            .track_mut(snare)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(77), bus));

        assert!(project.remove_track(bus));
        assert_eq!(project.track(kick).unwrap().output, Output::Master);
        assert!(project.track(snare).unwrap().sends.is_empty());
    }

    #[test]
    fn a_clip_cannot_be_moved_onto_a_bus() {
        // A bus holds no clips. Refusing has to happen before the clip leaves its own track:
        // "not an instrument track" once counted a bus as an audio track, and the clip was taken
        // off one track and then found nowhere to land.
        let mut project = Project::new("Bus", 48_000.0);
        let audio = project.add_audio_track("Sample");
        let source =
            project.add_audio_source("s", AssetPath::inside("Audio/s.wav"), 1_000, 48_000.0, 1);
        let clip = project.add_audio_clip(audio, source, Ticks::ZERO).unwrap();
        let bus = project.add_bus_track("Bus");

        assert!(!project.move_clip_to_track(clip, bus));
        assert_eq!(project.track_of_clip(clip), Some(audio));
    }

    #[test]
    fn a_duplicated_track_gets_send_ids_of_its_own() {
        // Every id in a copy is reissued for the same reason a clip's is: two sends answering to
        // one id would send an edit aimed at the copy to the original.
        let (mut project, kick, _, bus) = bussed_project();
        let send = project.next_send_id();
        project
            .track_mut(kick)
            .unwrap()
            .sends
            .push(AuxSend::new(send, bus));

        let copy = project.duplicate_track(kick).unwrap();
        assert_ne!(project.track(copy).unwrap().sends[0].id, send);
        // The routing itself is copied as it was: the copy feeds the same bus.
        assert_eq!(project.track(copy).unwrap().output, Output::Bus(bus));
        assert_eq!(project.track(copy).unwrap().sends[0].target, bus);
    }

    #[test]
    fn routing_survives_a_round_trip_through_json() {
        let (mut project, kick, _, bus) = bussed_project();
        project.track_mut(kick).unwrap().sends.push(AuxSend {
            id: SendId(42),
            target: bus,
            level_db: -7.5,
            pre_fader: true,
        });

        let json = serde_json::to_string(&project).expect("serialises");
        let back: Project = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, project, "{json}");
    }

    #[test]
    fn importing_the_same_font_twice_returns_the_first_one() {
        // A font is hundreds of megabytes. Noticing the repeat is the difference between one copy
        // in memory and two, and importing the same file twice is an ordinary slip.
        let mut project = Project::new("Fonts", 48_000.0);
        let first = project.add_soundfont("Grand", AssetPath::external("/fonts/grand.sf2"), 64);
        let again =
            project.add_soundfont("Grand Piano", AssetPath::external("/fonts/grand.sf2"), 64);
        assert_eq!(first, again);
        assert_eq!(project.soundfonts.len(), 1);
        // And the first name wins, rather than the entry being rewritten under whoever holds it.
        assert_eq!(project.soundfonts[&first].name, "Grand");

        let other = project.add_soundfont("Strings", AssetPath::external("/fonts/strings.sf2"), 64);
        assert_ne!(first, other);
        assert_eq!(project.soundfonts.len(), 2);
    }

    #[test]
    fn a_font_id_never_collides_with_anything_else() {
        // Every id in the document comes from one counter, and `repair_ids` has to sweep the new
        // map too or a project that is reopened will hand out an id that is already taken.
        let mut project = Project::new("Fonts", 48_000.0);
        let font = project.add_soundfont("Grand", AssetPath::external("/fonts/grand.sf2"), 64);
        let track = project.add_instrument_track("Lead", "x");
        assert_ne!(font.0, track.0);

        let mut reopened = project.clone();
        reopened.next_id = 0;
        reopened.repair_id_counter();
        assert!(reopened.next_id > font.0, "an id could be handed out twice");
    }

    #[test]
    fn a_project_written_before_fonts_existed_still_opens() {
        // The `serde(default)` on the map, stated as a test rather than trusted to a comment.
        let json = r#"{
            "name": "Old",
            "sample_rate": 48000.0,
            "tempo_map": {"points": [{"tick": 0, "bpm": 120.0}]},
            "time_signature": {"numerator": 4, "denominator": 4},
            "grid": 240,
            "tracks": [],
            "master": {"gain_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "effects": []},
            "audio_sources": {},
            "next_id": 1
        }"#;
        let project: Project = serde_json::from_str(json).expect("an older document still parses");
        assert!(project.soundfonts.is_empty());
        // The same document is from before the meter could change, and its lone `time_signature`
        // is a field nothing reads any more. It opens in 4/4 rather than refusing over a key that
        // is not there — every note in it survives, which is the whole of what the default buys.
        assert_eq!(project.signatures, crate::time::SignatureMap::default());
    }

    #[test]
    fn a_version_one_document_reads_its_bare_paths_as_external() {
        // Version 1 stored an asset as a plain string, which meant "somewhere on this machine".
        // Reading those as `External` is the whole migration; anything more would be a guess about
        // files this build has not looked at.
        let json = r#"{
            "format_version": 1,
            "name": "Old",
            "sample_rate": 48000.0,
            "tempo_map": {"points": [{"tick": 0, "bpm": 120.0}]},
            "time_signature": {"numerator": 4, "denominator": 4},
            "grid": 240,
            "tracks": [],
            "master": {"gain_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "effects": []},
            "audio_sources": {
                "1": {
                    "id": 1, "name": "kick", "path": "/music/loops/kick.wav",
                    "frame_count": 480, "sample_rate": 48000.0, "channel_count": 2
                }
            },
            "soundfonts": {
                "2": {"id": 2, "name": "GM", "path": "/libraries/GM.sf2"}
            },
            "next_id": 3
        }"#;
        let project: Project = serde_json::from_str(json).expect("a version 1 document opens");
        assert_eq!(
            project.audio_sources[&SourceId(1)].path,
            AssetPath::external("/music/loops/kick.wav")
        );
        assert_eq!(
            project.soundfonts[&SoundFontId(2)].path,
            AssetPath::external("/libraries/GM.sf2")
        );
        assert_eq!(
            project.soundfonts[&SoundFontId(2)].byte_size,
            0,
            "a size nobody recorded is 0, not a wrong number"
        );
        assert_eq!(
            project.harmony,
            Harmony::default(),
            "a document written before songs had a key opens in C major with no chords"
        );
    }

    #[test]
    fn a_project_carries_its_harmony_through_a_save_and_a_load() {
        use crate::theory::key::Key;
        use crate::theory::numeral::Numeral;

        let mut project = Project::new("Song", 48_000.0);
        project
            .harmony
            .keys
            .set_initial(Key::parse("F# minor").unwrap());
        project
            .harmony
            .keys
            .set_point(Ticks(3840 * 8), Key::parse("A major").unwrap());
        project
            .harmony
            .chords
            .set_point(Ticks::ZERO, Some(Numeral::parse("i").unwrap()));
        project
            .harmony
            .chords
            .set_point(Ticks(3840), Some(Numeral::parse("bVI").unwrap()));

        let json = serde_json::to_string(&project).unwrap();
        let reloaded: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.harmony, project.harmony);
        assert_eq!(
            reloaded.harmony.key_at(Ticks(3840 * 8)).to_text(),
            "A major"
        );
        assert_eq!(
            reloaded.harmony.chord_at(Ticks(3840)).unwrap().to_string(),
            "D",
            "bVI of F# minor"
        );
    }

    #[test]
    fn a_font_named_two_ways_is_still_one_font() {
        // `Audio/GM.sf2` in a collected project and an absolute path to the same bytes are the
        // same font, and only resolving both can see it.
        let folder = Path::new("/songs/first");
        let mut project = Project::new("Fonts", 48_000.0);
        let collected = project.add_soundfont("GM", AssetPath::inside("Audio/GM.sf2"), 64);

        assert_eq!(
            project.soundfont_at(Some(folder), Path::new("/songs/first/Audio/GM.sf2")),
            Some(collected)
        );
        assert_eq!(
            project.soundfont_at(Some(folder), Path::new("/libraries/GM.sf2")),
            None,
            "a different file with the same name is a different font"
        );
    }

    #[test]
    fn ids_are_unique_across_object_kinds() {
        let mut project = Project::new("Demo", 48_000.0);
        let track_a = project.add_instrument_track("A", "x");
        let track_b = project.add_audio_track("B");
        let clip = project
            .add_midi_clip(track_a, "c", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let effect = project.add_effect(Some(track_b), "auris.fx.gain").unwrap();
        assert_ne!(track_a.0, track_b.0);
        assert_ne!(clip.0, effect.0);
        assert_ne!(track_a.0, clip.0);
    }

    #[test]
    fn project_round_trips_through_json() {
        let project = demo_project();
        let json = serde_json::to_string(&project).unwrap();
        let restored: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(project, restored);
    }

    #[test]
    fn repair_id_counter_avoids_collisions_after_load() {
        let project = demo_project();
        let json = serde_json::to_string(&project).unwrap();
        let mut restored: Project = serde_json::from_str(&json).unwrap();
        restored.repair_id_counter();

        let existing: Vec<u64> = restored.tracks.iter().map(|t| t.id.0).collect();
        let fresh = restored.add_audio_track("New");
        assert!(!existing.contains(&fresh.0));
    }

    #[test]
    fn solo_overrides_unsoloed_tracks() {
        let mut project = Project::new("Demo", 48_000.0);
        let a = project.add_instrument_track("A", "x");
        let b = project.add_instrument_track("B", "x");
        assert!(project.track_is_audible(project.track(a).unwrap()));

        project.track_mut(b).unwrap().mixer.solo = true;
        assert!(!project.track_is_audible(project.track(a).unwrap()));
        assert!(project.track_is_audible(project.track(b).unwrap()));
    }

    #[test]
    fn clip_length_grows_to_fit_notes() {
        let mut clip = MidiClip::new(ClipId(1), "c", Ticks::ZERO, Ticks::QUARTER);
        clip.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(3.5)));
        clip.fit_length_to_notes(Ticks(TICKS_PER_QUARTER));
        assert_eq!(clip.length, Ticks::from_beats(4.0));
    }

    #[test]
    fn audio_clip_fades_reach_zero_at_the_edges() {
        let source = AudioSource {
            id: SourceId(1),
            name: "s".into(),
            path: AssetPath::inside("Audio/s.wav"),
            frame_count: 1000,
            sample_rate: 48_000.0,
            channel_count: 2,
        };
        let mut clip = AudioClip::new(ClipId(2), "c", Ticks::ZERO, &source);
        clip.fade_in_frames = 100;
        clip.fade_out_frames = 100;
        assert_eq!(clip.fade_gain_at(0), 0.0);
        assert!((clip.fade_gain_at(50) - 0.5).abs() < 1e-6);
        assert_eq!(clip.fade_gain_at(500), 1.0);
        assert!(clip.fade_gain_at(999) < 0.02);
    }

    #[test]
    fn a_duplicated_track_shares_no_ids_with_its_original() {
        let mut project = demo_project();
        let original = project.tracks[0].id;
        project.add_effect(Some(original), "auris.fx.gain").unwrap();

        let copy = project.duplicate_track(original).unwrap();
        assert_eq!(project.track_index(copy), Some(1), "the copy sits below");
        assert_ne!(copy, original);

        let before = project.track(original).unwrap();
        let after = project.track(copy).unwrap();
        assert_eq!(after.name, format!("{} copy", before.name));

        let ids = |track: &Track| -> Vec<u64> {
            let mut ids: Vec<u64> = track.mixer.effects.iter().map(|slot| slot.id.0).collect();
            if let Some(inner) = track.kind.as_instrument() {
                ids.extend(inner.clips.iter().map(|clip| clip.id.0));
            }
            ids
        };
        let original_ids = ids(before);
        assert!(!original_ids.is_empty(), "the fixture has ids to reissue");
        for id in ids(after) {
            assert!(
                !original_ids.contains(&id),
                "id {id} is shared with the original, so edits would hit the wrong object"
            );
        }
    }

    #[test]
    fn a_duplicated_clip_lands_immediately_after_the_original() {
        let mut project = demo_project();
        let original = project.tracks[0].kind.as_instrument().unwrap().clips[0].clone();

        let copy = project.duplicate_clip(original.id).unwrap();
        let (_, copy) = project.midi_clip(copy).unwrap();
        assert_eq!(copy.start, original.end());
        assert_eq!(copy.length, original.length);
        assert_eq!(copy.notes, original.notes);
        assert_ne!(copy.id, original.id);
    }

    #[test]
    fn splitting_a_midi_clip_divides_its_notes_and_truncates_the_straddler() {
        let mut project = Project::new("Demo", 48_000.0);
        let track = project.add_instrument_track("Lead", "x");
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::QUARTER, Ticks::from_beats(4.0))
            .unwrap();
        let notes = &mut project.midi_clip_mut(clip).unwrap().notes;
        notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER)); // wholly left
        notes.push(Note::new(62, Ticks::QUARTER, Ticks::from_beats(2.0))); // straddles
        notes.push(Note::new(64, Ticks::from_beats(3.0), Ticks::QUARTER)); // wholly right

        // Two beats into a clip that starts one beat in.
        let right = project
            .split_clip(clip, Ticks::QUARTER + Ticks::from_beats(2.0))
            .unwrap();

        let (_, left) = project.midi_clip(clip).unwrap();
        assert_eq!(left.start, Ticks::QUARTER);
        assert_eq!(left.length, Ticks::from_beats(2.0));
        assert_eq!(left.notes.len(), 2);
        assert_eq!(left.notes[1].pitch, 62);
        assert_eq!(
            left.notes[1].end(),
            Ticks::from_beats(2.0),
            "the straddling note is cut at the split, not left hanging past the clip"
        );

        let (_, right) = project.midi_clip(right).unwrap();
        assert_eq!(right.start, Ticks::QUARTER + Ticks::from_beats(2.0));
        assert_eq!(right.length, Ticks::from_beats(2.0));
        assert_eq!(right.notes.len(), 2);
        assert_eq!(right.notes[0].pitch, 62);
        assert_eq!(right.notes[0].start, Ticks::ZERO, "notes are rebased");
        assert_eq!(right.notes[0].length, Ticks::QUARTER);
        assert_eq!(right.notes[1].start, Ticks::QUARTER);
    }

    #[test]
    fn a_split_outside_the_clip_changes_nothing() {
        let mut project = demo_project();
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        let before = project.clone();

        assert!(project.split_clip(clip, Ticks::ZERO).is_none());
        assert!(project.split_clip(clip, Ticks::from_beats(4.0)).is_none());
        assert!(project.split_clip(clip, Ticks::from_beats(99.0)).is_none());
        assert_eq!(project.tracks, before.tracks);
    }

    #[test]
    fn splitting_an_audio_clip_divides_its_frames_without_losing_any() {
        let mut project = Project::new("Demo", 48_000.0);
        let track = project.add_audio_track("Audio");
        let source =
            project.add_audio_source("s", AssetPath::inside("Audio/s.wav"), 96_000, 48_000.0, 2);
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        project.audio_clip_mut(clip).unwrap().fade_in_frames = 480;
        project.audio_clip_mut(clip).unwrap().fade_out_frames = 480;

        // 96 000 frames at 48 kHz is two seconds; at 120 BPM that is one bar.
        let right = project.split_clip(clip, Ticks::from_beats(2.0)).unwrap();

        let left = project.audio_clip_mut(clip).unwrap().clone();
        let right = project.audio_clip_mut(right).unwrap().clone();
        assert_eq!(left.offset_frames, 0);
        assert_eq!(left.length_frames + right.length_frames, 96_000);
        assert_eq!(right.offset_frames, left.length_frames, "no frames skipped");
        assert_eq!(right.start, Ticks::from_beats(2.0));
        assert_eq!(left.fade_in_frames, 480, "the fade-in stays on the left");
        assert_eq!(left.fade_out_frames, 0, "no fade is invented at the cut");
        assert_eq!(right.fade_in_frames, 0);
        assert_eq!(right.fade_out_frames, 480);
    }

    #[test]
    fn removing_a_track_drops_only_that_track() {
        let mut project = demo_project();
        let id = project.tracks[0].id;
        assert!(project.remove_track(id));
        assert!(project.tracks.is_empty());
        assert!(!project.remove_track(id));
    }
}
