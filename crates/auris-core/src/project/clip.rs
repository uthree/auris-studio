//! Clips: a block of notes, a reference to a decoded file, and the bank the samples live in.
//!
//! Both kinds in one file, deliberately. Every exhaustive match over
//! [`TrackKind`](super::TrackKind) answers for a MIDI clip and an audio clip in the same body —
//! duplicating one, removing one, finding the track one sits on — and [`Project::split_clip`] is
//! a single function with an arm for each. A file per kind would leave every one of those
//! importing the other half back, which is a boundary nothing respects.
//!
//! [`AudioSourceBank`] is the other end of the indirection an [`AudioClip`] is: the clip names a
//! file by [`SourceId`], the bank holds the decoded samples, and that is what keeps a document
//! small enough to clone for undo.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::asset::AssetPath;
use crate::buffer::AudioBuffer;
use crate::time::{TempoMap, Ticks};

use super::curve::{ClipCurve, CurvePoint, curve_at, curve_events};
use super::recipe::ClipRecipe;
use super::track::TrackKind;
use super::{ClipId, Project, SourceId, TrackId};

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
    pub bend: Vec<CurvePoint>,
    /// The modulation written across the clip, in time order.
    ///
    /// Beside the bend and for the same reasons: controller 1 is a message every instrument
    /// answers rather than a parameter of any one of them, and it belongs to the phrase — a clip
    /// dragged four bars later takes its wheel movements with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modulation: Vec<CurvePoint>,
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
            modulation: Vec::new(),
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

    /// One of the clip's two curves.
    pub fn curve(&self, which: ClipCurve) -> &[CurvePoint] {
        match which {
            ClipCurve::Bend => &self.bend,
            ClipCurve::Modulation => &self.modulation,
        }
    }

    /// One of the clip's two curves, to be edited.
    pub fn curve_mut(&mut self, which: ClipCurve) -> &mut Vec<CurvePoint> {
        match which {
            ClipCurve::Bend => &mut self.bend,
            ClipCurve::Modulation => &mut self.modulation,
        }
    }

    /// What a curve reads at `at`, measured from the clip's own start.
    pub fn curve_at(&self, which: ClipCurve, at: Ticks) -> f32 {
        curve_at(self.curve(which), at)
    }

    /// A curve sampled into the events an instrument reads, from the clip's own start.
    pub fn curve_events(&self, which: ClipCurve, step: Ticks) -> Vec<(Ticks, f32)> {
        curve_events(self.curve(which), self.length, step)
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
    /// Size of the file in bytes, or 0 when it was recorded before this field existed.
    ///
    /// Not for reading the file — for recognising it, exactly as
    /// [`SoundFontRef::byte_size`](super::SoundFontRef::byte_size) does for a font. When the
    /// stored path stops being true the file name alone is a weak match, and this is what
    /// separates the sample that moved from a different one someone happened to give the same
    /// name. [`Self::frame_count`] cannot stand in: it counts frames *after* import resampling,
    /// so it describes what was decoded rather than what is on disk.
    #[serde(default)]
    pub byte_size: u64,
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

impl Project {
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

    /// Registers imported file metadata and returns its new id.
    ///
    /// Everything asked for here describes the *decoded* audio, which is what a caller holding a
    /// buffer has. [`AudioSource::byte_size`] describes the file instead, so it is left at 0 —
    /// which reads as "no fingerprint" — for whoever has the file on disk to fill in: importing,
    /// collecting into the project folder and finding a file that moved all write it.
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
                byte_size: 0,
            },
        );
        id
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
}

/// Length of `length_frames` source frames, placed at `start`, measured in ticks.
pub(super) fn audio_length_ticks(
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
    use crate::project::fixtures::demo_project;
    use crate::time::TICKS_PER_QUARTER;

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
            byte_size: 0,
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
}
