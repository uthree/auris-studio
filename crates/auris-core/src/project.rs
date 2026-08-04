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
use crate::time::{TempoMap, Ticks, TimeSignature};

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
/// The vocabulary a person chooses from, which is not quite the vocabulary the composer writes
/// in: `Drums` is one choice here and three parts inside the composer, because a kick, a snare and
/// a hat share an instrument and belong in one clip rather than three.
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
}

impl ClipPreset {
    /// Every preset, in the order a picker should offer them.
    pub const ALL: [ClipPreset; 7] = [
        ClipPreset::Lead,
        ClipPreset::Chords,
        ClipPreset::Pad,
        ClipPreset::Arp,
        ClipPreset::Stab,
        ClipPreset::Bass,
        ClipPreset::Drums,
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
            _ => return None,
        })
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
            // A new clip's length is a default, not a decision, so notes written past it still
            // grow it. Dragging its edge is what makes it a decision.
            length_is_explicit: false,
        }
    }

    /// `true` when the clip was written by the composer rather than played.
    pub fn is_generated(&self) -> bool {
        self.recipe.is_some()
    }

    /// Position just past the end of the clip.
    pub fn end(&self) -> Ticks {
        self.start + self.length
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
}

impl TrackKind {
    /// Short label for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            TrackKind::Instrument(_) => "Instrument",
            TrackKind::Audio(_) => "Audio",
        }
    }

    /// `true` when this track holds notes rather than audio.
    ///
    /// The two kinds are the one thing that decides whether a clip may move to a track, so the
    /// question is asked directly rather than through a pattern match at each call site.
    pub fn is_instrument(&self) -> bool {
        matches!(self, TrackKind::Instrument(_))
    }

    /// The instrument track data, when this is one.
    pub fn as_instrument(&self) -> Option<&InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            TrackKind::Audio(_) => None,
        }
    }

    /// The instrument track data mutably, when this is one.
    pub fn as_instrument_mut(&mut self) -> Option<&mut InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            TrackKind::Audio(_) => None,
        }
    }

    /// The audio track data, when this is one.
    pub fn as_audio(&self) -> Option<&AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            TrackKind::Instrument(_) => None,
        }
    }

    /// The audio track data mutably, when this is one.
    pub fn as_audio_mut(&mut self) -> Option<&mut AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            TrackKind::Instrument(_) => None,
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
                    let seconds = clip.length_frames as f64 / sample_rate;
                    let start_seconds = tempo_map.ticks_to_seconds(clip.start).0;
                    tempo_map.seconds_to_ticks(crate::time::Seconds(start_seconds + seconds))
                })
                .max()
                .unwrap_or(Ticks::ZERO),
        }
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
    /// Bar/beat grid.
    pub time_signature: TimeSignature,
    /// The key and the chords, over the timeline.
    ///
    /// Beside the tempo map rather than inside a track, because it is the same kind of thing: it
    /// changes as the song goes on, and at any one moment every track obeys the same one.
    #[serde(default)]
    pub harmony: Harmony,
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
    pub const FORMAT_VERSION: u32 = 3;

    /// An empty project.
    pub fn new(name: impl Into<String>, sample_rate: f64) -> Self {
        Self {
            format_version: Self::FORMAT_VERSION,
            name: name.into(),
            sample_rate,
            tempo_map: TempoMap::constant(120.0),
            time_signature: TimeSignature::default(),
            harmony: Harmony::default(),
            tracks: Vec::new(),
            master: MixerStrip::default(),
            audio_sources: BTreeMap::new(),
            soundfonts: BTreeMap::new(),
            loop_region: None,
            loop_enabled: false,
            grid: default_grid(),
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

    /// Appends an instrument track playing `instrument_id`.
    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: impl Into<String>,
    ) -> TrackId {
        let id = TrackId(self.allocate_id());
        let color = Color::from_palette(self.tracks.len());
        self.tracks.push(Track {
            id,
            name: name.into(),
            color,
            height: default_track_height(),
            kind: TrackKind::Instrument(InstrumentTrack {
                instrument_id: instrument_id.into(),
                instrument_state: PluginState::empty(),
                clips: Vec::new(),
            }),
            mixer: MixerStrip::default(),
        });
        id
    }

    /// Appends an empty audio track.
    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.allocate_id());
        let color = Color::from_palette(self.tracks.len());
        self.tracks.push(Track {
            id,
            name: name.into(),
            color,
            height: default_track_height(),
            kind: TrackKind::Audio(AudioTrack::default()),
            mixer: MixerStrip::default(),
        });
        id
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
        self.tracks.len() != before
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
    pub fn track_is_audible(&self, track: &Track) -> bool {
        !track.mixer.mute && (!self.has_solo() || track.mixer.solo)
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

    /// Repairs a project loaded from disk: makes sure the id counter is past every id in use,
    /// so ids handed out later cannot collide with existing ones.
    pub fn repair_id_counter(&mut self) {
        let mut highest = 0u64;
        for track in &self.tracks {
            highest = highest.max(track.id.0);
            for slot in &track.mixer.effects {
                highest = highest.max(slot.id.0);
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
