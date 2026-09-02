//! The serialisable document model.
//!
//! A [`Project`] holds everything the user edits: tempo, tracks, clips, notes and mixer state.
//! It contains no audio samples and no plugin instances — only ids and parameter values — which
//! keeps it cheap to clone for undo and trivially serialisable to JSON.
//!
//! Two indirections make that work:
//!
//! * A track names its instrument by plugin id plus a
//!   [`PluginState`](crate::plugin::PluginState); the engine asks the
//!   [`PluginRegistry`](crate::registry::PluginRegistry) to build the real object.
//! * An audio clip names its samples by [`SourceId`]; the decoded audio lives in a separate
//!   runtime [`AudioSourceBank`], so the project stays small and a file imported once can back
//!   any number of clips.
//!
//! # Where things are
//!
//! Here: the ids everything else spends, the fonts a document names, and [`Project`] itself —
//! the struct, its [`FORMAT_VERSION`](Project::FORMAT_VERSION), and the methods that are about
//! the whole document rather than any one part of it.
//!
//! The rest is a file per thing a document is made of. `clip` holds **both** clip kinds
//! together: every exhaustive match over [`TrackKind`] answers for a block of notes and a
//! reference to a file in the same body, and a boundary between the two would leave each side
//! importing the other back. `track` is what a clip sits on, `routing` is the mixer strip and
//! where what leaves it goes, `recipe` is how a written clip was written, `curve` is the
//! bend and the wheel drawn across one, and `ornament` is the scoop, fall and vibrato a sung
//! note carries. Every one of them is private and re-exported, so
//! `auris_core::project::MidiClip` is the path it always was.
//!
//! A method lives with the type it is about rather than here: `add_midi_clip` is in `clip`,
//! `remove_track` in `track`, `solo_resolution` in `routing`. They are all still `impl Project`,
//! so nothing a caller writes changes.

mod clip;
mod curve;
mod ornament;
mod recipe;
mod routing;
mod track;
mod transform;

#[cfg(test)]
mod fixtures;

pub use clip::{
    AudioClip, AudioSource, AudioSourceBank, FadeCurve, MAX_STRETCH, MIN_STRETCH, MidiClip, Note,
    UNSTRETCHED, default_loop_end, loop_passes, notes_digest, notes_trimmed_from_front,
    quantised_stretch, sounding_length, stretch_key,
};
pub use curve::{
    BEND_LIMIT, CONTROLLER_LIMIT, CURVE_STEP, ClipCurve, CurvePoint, curve_at, curve_events,
};
pub use ornament::{Fall, Scoop, Vibrato};
pub use recipe::{ClipPreset, ClipRecipe, Subdivision};
pub use routing::{AuxSend, EffectSlot, MixerStrip, Output};
pub use track::{
    AudioTrack, Color, ConsonantLevels, ConsonantWidths, InstrumentTrack, SingerTake, SingerTrack,
    SingerVoice, Track, TrackKind, default_frame_hop,
};
pub use transform::{NoteTransform, performed};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::asset::AssetPath;
use crate::harmony::Harmony;
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

/// The whole document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Format version, bumped when the schema changes incompatibly.
    #[serde(default = "current_format_version")]
    pub format_version: u32,
    /// The build that last saved this file, as its version string.
    ///
    /// Informational, never gating: the format version above decides whether a file can be
    /// *read*, and this is what lets the door note that the *performer* has changed — a document
    /// saved under another build regenerates in the current composer's style, not the one it was
    /// written in. Stamped by the save path beside the format version; empty for a file saved
    /// before the field existed, and for a document never saved at all.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub saved_by: String,
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
    /// Punch region: the stretch of the timeline a take is allowed to keep.
    ///
    /// Its own range rather than the loop's, though the two are usually set to the same bars.
    /// They answer different questions — the loop says what is *played again* and the punch says
    /// what is *written down* — and a musician looping four bars while punching the third of them
    /// is doing the ordinary thing, not an exotic one.
    #[serde(default)]
    pub punch_region: Option<(Ticks, Ticks)>,
    /// Whether a take is trimmed to [`Self::punch_region`].
    ///
    /// A property of the document for the same reason the loop is: the bar that needs fixing is a
    /// fact about the song, and it is still the bar that needs fixing tomorrow.
    ///
    /// Not a format bump. A build that has never heard of either field opens the document and
    /// plays every note of it correctly, with punch off.
    #[serde(default)]
    pub punch_enabled: bool,
    /// Whether a click is heard on every beat while the transport rolls.
    ///
    /// A property of the document rather than of the application, for the same reason the loop
    /// region is one: whether a piece wants counting in is a fact about the piece. A song written
    /// in 7/8 is one somebody comes back to wanting the click, and one they exported last week is
    /// one they do not — and neither of those is a preference that should follow them into the
    /// next project they open.
    ///
    /// The click is never rendered offline and never passes through the master strip, so this
    /// changes nothing about what an export contains — the engine's `metronome` module is where
    /// that is arranged, and this crate may not name it.
    ///
    /// Not a bump: a build that has never heard of this field opens the document and plays every
    /// note of it correctly, with the click off.
    #[serde(default)]
    pub metronome: bool,
    /// How many bars are counted in front of a take, or zero for none.
    ///
    /// Bars rather than beats, because that is how a count-in is asked for — "give me two" — and
    /// because how many beats those are is a question the meter answers. A piece in 7/8 counted
    /// in two bars gets fourteen beats without anybody having to work that out.
    ///
    /// A property of the document for the same reason the click is: a piece somebody comes back
    /// to wanting counting in is one they wanted counting in last week, and a preference that
    /// followed them into the next project would be wrong there as often as it was right.
    ///
    /// Not a bump: a build that has never heard of this field opens the document and plays every
    /// note of it correctly, without counting anybody in.
    #[serde(default)]
    pub count_in_bars: u32,
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
    ///
    /// 8 since a clip's bend became a [`CurvePoint`] list shared with its modulation. The field is
    /// spelt `value` where it was `semitones`, so a version 7 document's bends would be read as
    /// zeroes — silently, because the field has a default — and a slide somebody wrote would
    /// simply stop happening.
    ///
    /// 9 since a numeral's slash bass gained an accidental, so `v/b7` is now a symbol a chord can
    /// be stored as. A version 8 build has no reading for the `b`: it falls through to the
    /// secondary-dominant branch, finds no roman numeral there, and rejects the numeral — which
    /// fails the whole document rather than the one chord. That is the honest answer, but the
    /// version is what makes it happen at the door instead of halfway through a harmony lane.
    ///
    /// 10 since a clip gained [`loop_end`](MidiClip::loop_end). Version 5's case again, and the
    /// distinction it drew is the whole of why this one moves and the metronome flag beside it did
    /// not: a field an older build ignores costs nothing when ignoring it plays the same music, and
    /// costs the work when it does not. A version 9 build would open a song whose drum loop runs
    /// thirty-two bars, play the one bar, and write that back on the next save with the other
    /// thirty-one gone and nothing on screen having said so.
    ///
    /// 11 since an automation lane records the stable
    /// [`key`](crate::automation::AutomationLane::key) of the parameter it drives beside the
    /// [`ParamId`](crate::param::ParamId) that addresses it. Version 5's case a third time, and the
    /// sharpest of the three: a version 10 build ignores the key, so it opens the document, plays
    /// every curve against whatever now occupies the position the id names — a Cutoff curve driving
    /// a Reverb Mix after the plugin's author added one parameter — and writes the file back with
    /// the key gone, taking the only record of what the lane was drawn on with it. The version is
    /// what turns that into a sentence about updating instead.
    /// 12 since a clip's one modulation curve became a map of
    /// [`controllers`](MidiClip::controllers). Version 8's case in a different field: the wheel is
    /// spelt as an entry under `1` where it was a list called `modulation`, so a version 11
    /// document's wheel movements read as nothing at all — silently, because the map has a default
    /// — and a swell somebody wrote would simply stop happening. The other direction is worse
    /// still, and is what the number is really for: a version 11 build would open a part shaped by
    /// an expression pedal, play it flat, and write the file back with every controller in it
    /// gone.
    /// 13 since an audio clip can [`follow the tempo`](AudioClip::follows_tempo). Version 5's case
    /// again, and about as bad as it gets: a version 12 build ignores both new fields, so it opens
    /// a piece whose loops were stretched to fit, plays every one of them at its recorded length —
    /// against a bar line they no longer share — and writes the file back with the fact that they
    /// ever followed anything gone.
    ///
    /// 14 since a following clip can be anchored to a tempo other than the one at its own start —
    /// [`tempo_anchor`](AudioClip::tempo_anchor), which is what a split or a front trim writes so
    /// that dividing a clip does not change how it sounds. Version 5's case once more, and quiet:
    /// a version 13 build ignores the field, so every piece cut the far side of a tempo change
    /// plays at whatever speed its new start implies — a seam in the middle of one take — and the
    /// next save writes the anchor away, so the seam is there for good and the file no longer
    /// records that the two halves were ever one thing.
    ///
    /// 15 since an effect slot can be keyed from another track —
    /// [`sidechain`](crate::project::EffectSlot::sidechain). Version 5's case yet again, and this
    /// one is not even quiet about it while the file is open: a version 14 build ignores the
    /// field, so a compressor keyed from the kick drum runs on the bass alone and the duck that
    /// holds the low end apart simply is not there. The save afterwards writes the key away, and
    /// with it the only record of which track the effect was listening to.
    /// 16 since a clip's fades carry a shape — [`FadeCurve`], one for
    /// each edge. Version 5's case one more time, and the one that is easiest to mistake for a bad
    /// edit: a version 15 build ignores both fields, so every crossfade in the piece plays as two
    /// straight ramps crossing and dips about three decibels in the middle of each join. Nothing
    /// looks wrong, nothing is reported, and the save afterwards writes the shapes away — leaving
    /// a piece whose joins all have a hole in them and no record that they ever did not.
    ///
    /// 17 since [`TrackKind`] gained [`Singer`](TrackKind::Singer) and a note gained its
    /// [`lyric`](Note::lyric) and [`phonemes`](Note::phonemes). Version 6's case for the variant:
    /// a stored enum arm an older build has never heard of fails the whole document, and the
    /// version turns that into a sentence at the door. The note fields alone would not have moved
    /// it — a version 16 build reading them as nothing and writing them away only costs words on
    /// notes that could not be sung anyway, on a track that build cannot open.
    ///
    /// 18 since a clip gained its [`transforms`](MidiClip::transforms). A version 17 build would
    /// read the field as nothing and play the text of every clip unperformed — straight where the
    /// piece swings, quantised where it was loosened — and the save afterwards would write the
    /// stack away entirely. Version 5's shape again: nothing looks wrong, and the piece is not
    /// the one that was saved.
    ///
    /// 19 since [`NoteTransform`] gained [`Lean`](NoteTransform::Lean), which is how the
    /// composer's feel now arrives — installed on the clip instead of baked into its notes.
    /// Version 6's case once more: a stored enum arm an older build has never heard of fails
    /// the whole document, and the version turns that into a sentence at the door. The recipe
    /// losing its `humanize` dial in the same change moved nothing — an unknown field is
    /// skipped on the way in and a missing one defaults on the way out.
    pub const FORMAT_VERSION: u32 = 19;

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
            saved_by: String::new(),
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
            punch_region: None,
            punch_enabled: false,
            metronome: false,
            count_in_bars: 0,
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
            for clip in track.kind.note_clips().into_iter().flatten() {
                highest = highest.max(clip.id.0);
            }
            if let Some(inner) = track.kind.as_audio() {
                for clip in &inner.clips {
                    highest = highest.max(clip.id.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::demo_project;

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
}
