//! Layers: an instrument track that plays another track's notes.
//!
//! A layer owns everything a track owns — its instrument, its strip, its outputs and sends, its
//! automation — except the notes. Those it borrows from the track it [follows](NoteSource), so
//! editing the part once is heard on every instrument stacked on it, and each instrument keeps its
//! own settings. That is the whole of "one source, several performers": the bus a layer and its
//! source both feed is where a shared filter goes, and nothing about routing had to change for it.
//!
//! On the way across, a [`LayerShape`] can bend what is borrowed: transpose it, scale its
//! velocity, keep only a key range or only the top or bottom voice, move it in time, and lengthen
//! or shorten it. The shape is applied to the *sounding* notes — after the source clip's own
//! performance stack — so a layer hears exactly what its source plays and changes only what it
//! says it changes.
//!
//! A layer is never a source. Following always names a track whose notes are its own, so there is
//! no chain to walk and no cycle to refuse: a layer whose source is missing, is a drum track or is
//! itself a layer simply plays nothing, the way a missing asset leaves its one track silent.

use serde::{Deserialize, Serialize};

use crate::time::{TICKS_PER_QUARTER, Ticks};

use super::clip::{MidiClip, Note};
use super::track::{Track, TrackKind};
use super::{ClipId, Project, TrackId};

/// Furthest a layer may be moved in time either way: one 4/4 bar.
pub const MAX_LAYER_OFFSET: Ticks = Ticks(TICKS_PER_QUARTER * 4);

/// Where a layer's notes come from, and how they are shaped on the way.
///
/// A layer is an instrument track carrying one of these: it owns its instrument, strip, outputs,
/// sends and automation like any track, and borrows only the notes. The source is always a track
/// with notes of its own — never a layer, never a drum track — so a layer pointing anywhere else
/// plays nothing rather than anything surprising.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteSource {
    /// The track whose clips are played. Never a layer itself.
    pub track: TrackId,
    /// What is done to the borrowed notes.
    #[serde(default, skip_serializing_if = "LayerShape::is_identity")]
    pub shape: LayerShape,
}

impl NoteSource {
    /// Follows `track` as it is, unshaped.
    pub fn new(track: TrackId) -> Self {
        Self {
            track,
            shape: LayerShape::default(),
        }
    }
}

/// Which notes of a chord a layer keeps.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerVoice {
    /// Every note.
    #[default]
    All,
    /// Only a note nothing higher is sounding over when it starts — the melody of a voiced part.
    Top,
    /// Only a note nothing lower is sounding under when it starts — the bass of a voiced part.
    Bottom,
}

/// What a layer does to the notes it borrows, in the order the fields are listed.
///
/// The key range and the voice choose notes by their *source* pitch, so a split at C4 means the
/// left hand of the part as written, wherever the transpose then puts it. Every field's default is
/// the identity, and the default shape plays the source note for note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerShape {
    /// Lowest source pitch kept, inclusive.
    pub lowest: u8,
    /// Highest source pitch kept, inclusive.
    pub highest: u8,
    /// Which voice of a chord is kept.
    pub voice: LayerVoice,
    /// Semitones up (positive) or down (negative); clamped at the ends of the MIDI range.
    pub transpose: i32,
    /// Velocity multiplier, read clamped to 0..=2; the result is clamped to 0..=1.
    pub velocity: f32,
    /// Length multiplier, read clamped to 0.05..=4. A note never shrinks below one tick.
    pub length: f32,
    /// Ticks late (positive) or early (negative), read clamped to [`MAX_LAYER_OFFSET`] either
    /// way. In ticks so a delay of an eighth note stays an eighth note at every tempo.
    pub offset: Ticks,
}

impl Default for LayerShape {
    fn default() -> Self {
        Self {
            lowest: 0,
            highest: 127,
            voice: LayerVoice::All,
            transpose: 0,
            velocity: 1.0,
            length: 1.0,
            offset: Ticks::ZERO,
        }
    }
}

impl LayerShape {
    /// `true` when the shape leaves every note as it is.
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// The shaped notes, positions relative to the clip as given.
    ///
    /// A note moved before the clip's start is held at it, the way a lean is.
    pub fn apply(&self, notes: Vec<Note>) -> Vec<Note> {
        if self.is_identity() {
            return notes;
        }
        let (lowest, highest) = (self.lowest.min(self.highest), self.highest.max(self.lowest));
        let in_range: Vec<Note> = notes
            .into_iter()
            .filter(|note| (lowest..=highest).contains(&note.pitch))
            .collect();
        let voiced = match self.voice {
            LayerVoice::All => in_range,
            LayerVoice::Top => outer_voice(in_range, |other, note| other > note),
            LayerVoice::Bottom => outer_voice(in_range, |other, note| other < note),
        };
        let velocity = if self.velocity.is_finite() {
            self.velocity.clamp(0.0, 2.0)
        } else {
            1.0
        };
        let length = if self.length.is_finite() {
            self.length.clamp(0.05, 4.0)
        } else {
            1.0
        };
        let offset = self
            .offset
            .raw()
            .clamp(-MAX_LAYER_OFFSET.raw(), MAX_LAYER_OFFSET.raw());
        voiced
            .into_iter()
            .map(|mut note| {
                note.pitch = (i32::from(note.pitch) + self.transpose).clamp(0, 127) as u8;
                note.velocity = (note.velocity * velocity).clamp(0.0, 1.0);
                note.length = Ticks(((note.length.raw() as f32 * length).round() as i64).max(1));
                note.start = (note.start + Ticks(offset)).max_zero();
                note
            })
            .collect()
    }
}

/// The notes no other note in the set `beats` at the moment they start.
///
/// "Sounding at the moment it starts" rather than "starting together", so a melody over a held
/// chord is still the top voice and a bass note under a sustained pad is still the bottom one.
fn outer_voice(notes: Vec<Note>, beats: impl Fn(u8, u8) -> bool) -> Vec<Note> {
    let keep: Vec<bool> = notes
        .iter()
        .map(|note| {
            !notes.iter().any(|other| {
                other.start <= note.start
                    && note.start < other.end()
                    && beats(other.pitch, note.pitch)
            })
        })
        .collect();
    notes
        .into_iter()
        .zip(keep)
        .filter_map(|(note, keep)| keep.then_some(note))
        .collect()
}

impl TrackKind {
    /// What this track follows, when it is a layer.
    pub fn layer(&self) -> Option<&NoteSource> {
        match self {
            TrackKind::Instrument(track) => track.follows.as_ref(),
            _ => None,
        }
    }

    /// What this track follows, mutably, when it is a layer.
    pub fn layer_mut(&mut self) -> Option<&mut NoteSource> {
        match self {
            TrackKind::Instrument(track) => track.follows.as_mut(),
            _ => None,
        }
    }

    /// `true` when other tracks may follow this one: a melodic or singer track with notes of its
    /// own. Drum tracks are left out, because a note number names a different sound on every kit.
    pub fn can_be_followed(&self) -> bool {
        match self {
            TrackKind::Instrument(track) => track.follows.is_none(),
            TrackKind::Singer(_) => true,
            _ => false,
        }
    }
}

/// The clips a track plays and the shape they pass through on the way.
#[derive(Copy, Clone, Debug)]
pub struct PlayedClips<'a> {
    /// The clips: the track's own, or its source's.
    pub clips: &'a [MidiClip],
    /// The layer's shape; `None` for a track playing its own notes.
    pub shape: Option<&'a LayerShape>,
}

impl PlayedClips<'_> {
    /// The notes `clip` sounds on this track, positions relative to the clip.
    ///
    /// The one answer the renderer and the MIDI writer share, for the reason
    /// [`MidiClip::sounding_notes_with_meter`] gives.
    pub fn sounding_notes(
        &self,
        clip: &MidiClip,
        bpm: f64,
        signatures: crate::time::SignatureMap,
    ) -> Vec<Note> {
        let notes = clip.sounding_notes_with_meter(bpm, signatures).collect();
        match self.shape {
            Some(shape) => shape.apply(notes),
            None => notes,
        }
    }
}

impl Project {
    /// The note clips `track` plays, or `None` when it is not a note track.
    ///
    /// A layer plays its source's clips through its shape, and nothing at all when its source is
    /// missing or cannot be followed. Every other note track plays its own.
    pub fn played_clips<'a>(&'a self, track: &'a Track) -> Option<PlayedClips<'a>> {
        match track.kind.layer() {
            Some(source) => {
                let clips = self
                    .track(source.track)
                    .filter(|source| source.kind.can_be_followed())
                    .and_then(|source| source.kind.note_clips())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                Some(PlayedClips {
                    clips,
                    shape: Some(&source.shape),
                })
            }
            None => track.kind.note_clips().map(|clips| PlayedClips {
                clips: clips.as_slice(),
                shape: None,
            }),
        }
    }

    /// The layers following `source`, in track order.
    pub fn layers_of(&self, source: TrackId) -> Vec<TrackId> {
        self.tracks
            .iter()
            .filter(|track| {
                track
                    .kind
                    .layer()
                    .is_some_and(|layer| layer.track == source)
            })
            .map(|track| track.id)
            .collect()
    }

    /// Adds a layer following `source` and playing `instrument_id`, below the source and the
    /// layers already on it.
    ///
    /// `None` when there is no such track or it cannot be followed.
    pub fn add_layer_track(
        &mut self,
        source: TrackId,
        name: impl Into<String>,
        instrument_id: impl Into<String>,
    ) -> Option<TrackId> {
        if !self.track(source)?.kind.can_be_followed() {
            return None;
        }
        let below = self.layers_of(source).last().copied().unwrap_or(source);
        let at = self.track_index(below)? + 1;
        let id = self.add_instrument_track(name, instrument_id);
        if let Some(TrackKind::Instrument(inner)) = self.track_mut(id).map(|track| &mut track.kind)
        {
            inner.follows = Some(NoteSource::new(source));
        }
        self.move_track(id, at);
        Some(id)
    }

    /// Turns a layer back into a track with notes of its own.
    ///
    /// The source's clips are copied under new ids and the shape is written into the copied
    /// notes, so the track goes on saying what it said; each copied clip keeps the source's
    /// performance stack, which now plays on top of the shaped text. `false` when `id` is not a
    /// layer. A layer whose source is gone is detached with no clips.
    pub fn detach_layer(&mut self, id: TrackId) -> bool {
        let Some(source) = self.track(id).and_then(|track| track.kind.layer()).cloned() else {
            return false;
        };
        let mut copies: Vec<MidiClip> = self
            .track(source.track)
            .filter(|track| track.kind.can_be_followed())
            .and_then(|track| track.kind.note_clips())
            .cloned()
            .unwrap_or_default();
        for clip in &mut copies {
            clip.id = ClipId(self.allocate_id());
            clip.notes = source.shape.apply(std::mem::take(&mut clip.notes));
        }
        let Some(TrackKind::Instrument(inner)) = self.track_mut(id).map(|track| &mut track.kind)
        else {
            return false;
        };
        inner.follows = None;
        inner.clips = copies;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: u8, start: i64, length: i64) -> Note {
        Note {
            velocity: 0.8,
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    fn pitches(notes: &[Note]) -> Vec<u8> {
        let mut pitches: Vec<u8> = notes.iter().map(|note| note.pitch).collect();
        pitches.sort_unstable();
        pitches
    }

    /// A C major triad held for a bar, with a melody of E5 then G5 over it.
    fn voiced_part() -> Vec<Note> {
        vec![
            note(48, 0, 1920),
            note(52, 0, 1920),
            note(55, 0, 1920),
            note(76, 0, 480),
            note(79, 480, 480),
        ]
    }

    #[test]
    fn the_default_shape_is_the_identity() {
        let notes = voiced_part();
        assert_eq!(LayerShape::default().apply(notes.clone()), notes);
    }

    #[test]
    fn the_key_range_keeps_source_pitches_before_the_transpose() {
        let shape = LayerShape {
            highest: 59,
            transpose: -12,
            ..LayerShape::default()
        };
        assert_eq!(pitches(&shape.apply(voiced_part())), vec![36, 40, 43]);
    }

    #[test]
    fn the_top_voice_is_the_melody_over_a_held_chord() {
        let shape = LayerShape {
            voice: LayerVoice::Top,
            ..LayerShape::default()
        };
        assert_eq!(pitches(&shape.apply(voiced_part())), vec![76, 79]);
    }

    #[test]
    fn the_bottom_voice_is_the_root_under_the_melody() {
        let shape = LayerShape {
            voice: LayerVoice::Bottom,
            ..LayerShape::default()
        };
        assert_eq!(pitches(&shape.apply(voiced_part())), vec![48]);
    }

    #[test]
    fn velocity_length_and_offset_are_scaled_and_clamped() {
        let shape = LayerShape {
            velocity: 0.5,
            length: 0.25,
            offset: Ticks(240),
            ..LayerShape::default()
        };
        let shaped = shape.apply(vec![note(60, 0, 480)]);
        assert_eq!(shaped[0].velocity, 0.4);
        assert_eq!(shaped[0].length, Ticks(120));
        assert_eq!(shaped[0].start, Ticks(240));

        let loud = LayerShape {
            velocity: 9.0,
            offset: Ticks(-100_000),
            ..LayerShape::default()
        };
        let shaped = loud.apply(vec![note(60, 960, 480)]);
        assert_eq!(
            shaped[0].velocity, 1.0,
            "the result is clamped to full scale"
        );
        assert_eq!(
            shaped[0].start,
            Ticks::ZERO,
            "an early note is held at the clip start"
        );
    }

    #[test]
    fn transposing_past_the_midi_range_clamps_rather_than_drops() {
        let shape = LayerShape {
            transpose: 24,
            ..LayerShape::default()
        };
        assert_eq!(pitches(&shape.apply(vec![note(120, 0, 10)])), vec![127]);
    }

    fn project_with_lead() -> (Project, TrackId) {
        let mut project = Project::new("Layers", 48_000.0);
        let lead = project.add_instrument_track("Lead", "chiptune");
        let clip = project
            .add_midi_clip(lead, "A", Ticks::ZERO, Ticks(1920))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = vec![note(60, 0, 480), note(64, 480, 480)];
        project.add_bus_track("Bus");
        (project, lead)
    }

    #[test]
    fn a_layer_sits_below_its_source_and_plays_its_clips() {
        let (mut project, lead) = project_with_lead();
        let first = project.add_layer_track(lead, "Pad", "pad").unwrap();
        let second = project.add_layer_track(lead, "Bass", "bass").unwrap();
        let order: Vec<_> = project.tracks.iter().map(|track| track.id).collect();
        assert_eq!(&order[..3], &[lead, first, second]);

        let layer = project.track(first).unwrap();
        assert!(
            !layer.kind.holds_notes(),
            "a layer takes no clips of its own"
        );
        let played = project.played_clips(layer).unwrap();
        assert_eq!(played.clips.len(), 1);
        assert_eq!(project.layers_of(lead), vec![first, second]);
    }

    #[test]
    fn layers_drums_and_missing_tracks_cannot_be_followed() {
        let (mut project, lead) = project_with_lead();
        let layer = project.add_layer_track(lead, "Pad", "pad").unwrap();
        assert!(project.add_layer_track(layer, "Again", "pad").is_none());
        let drums = project.add_drum_track("Drums", "drums");
        assert!(project.add_layer_track(drums, "Kit", "pad").is_none());
        assert!(
            project
                .add_layer_track(TrackId(9_999), "None", "pad")
                .is_none()
        );

        // A document that says otherwise opens and plays nothing on that track.
        project
            .track_mut(layer)
            .unwrap()
            .kind
            .layer_mut()
            .unwrap()
            .track = drums;
        let track = project.track(layer).unwrap();
        assert!(project.played_clips(track).unwrap().clips.is_empty());
    }

    #[test]
    fn detaching_writes_the_shape_into_copied_clips() {
        let (mut project, lead) = project_with_lead();
        let layer = project.add_layer_track(lead, "Fifth", "pad").unwrap();
        project
            .track_mut(layer)
            .unwrap()
            .kind
            .layer_mut()
            .unwrap()
            .shape
            .transpose = 7;
        assert!(project.detach_layer(layer));

        let source_clip = project.track(lead).unwrap().kind.note_clips().unwrap()[0].id;
        let detached = project.track(layer).unwrap();
        assert!(detached.kind.layer().is_none());
        let clips = detached.kind.note_clips().unwrap();
        assert_eq!(clips.len(), 1);
        assert_ne!(
            clips[0].id, source_clip,
            "the copy gets a clip id of its own"
        );
        assert_eq!(pitches(&clips[0].notes), vec![67, 71]);
        assert!(!project.detach_layer(layer), "it is no longer a layer");
    }

    #[test]
    fn a_layer_round_trips_and_an_unshaped_one_stores_no_shape() {
        let (mut project, lead) = project_with_lead();
        let layer = project.add_layer_track(lead, "Pad", "pad").unwrap();
        let stored = serde_json::to_value(project.track(layer).unwrap()).unwrap();
        let follows = &stored["kind"]["follows"];
        assert_eq!(follows["track"], serde_json::json!(lead.0));
        assert!(follows.get("shape").is_none());
        let back: Track = serde_json::from_value(stored).unwrap();
        assert_eq!(back.kind.layer(), Some(&NoteSource::new(lead)));
    }
}
