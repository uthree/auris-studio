//! What an instrument plays, and layers: instrument tracks that play another track's notes.
//!
//! Every note track has a score and an instrument, and what the instrument plays is whatever
//! arrives on its notes edges — its own score, as a track starts out, another track's, or several
//! at once, merged. [`Project::played_clips`] is the one answer to that question.
//!
//! A **layer** is an instrument playing other tracks' scores and not its own. It owns everything a
//! track owns — its instrument, its strip, its outputs, its automation — except the notes, so
//! editing the part once is heard on every instrument stacked on it, and each instrument keeps
//! its own settings. That is the whole of "one source, several performers": the bus a layer and
//! its source both feed is where a shared filter goes.
//!
//! On the way across, [`Performance`](super::Performance) nodes can bend what is borrowed: any
//! [`NoteTransform`](super::NoteTransform), including the ones made for this — a key range, the
//! top or bottom voice, a velocity or length scale, a delay. They run on the *sounding* notes,
//! after the source clip's own performance stack, so a layer hears exactly what its source plays
//! and changes only what its performances say.
//!
//! A layer's score is never played elsewhere. A path always starts at a track whose notes are its
//! own, so there is no chain of layers to walk and no cycle among them to refuse: a path whose
//! source is missing, is a drum track or is itself a layer simply plays nothing, the way a missing
//! asset leaves its one track silent.

use crate::time::Ticks;

use super::clip::{MidiClip, Note};
use super::edges::Performance;
use super::track::{Track, TrackKind};
use super::transform::NoteTransform;
use super::{ClipId, PerformanceContext, PerformanceId, Project, TrackId, performed_notes};

/// Everything an instrument plays: one part per path into it.
#[derive(Clone, Debug, Default)]
pub struct PlayedClips<'a> {
    /// The parts, in the order their connections were made.
    pub parts: Vec<PlayedPart<'a>>,
}

impl<'a> PlayedClips<'a> {
    /// Every clip played, each with the part it arrives through.
    pub fn clips(&self) -> impl Iterator<Item = (&'a MidiClip, &PlayedPart<'a>)> + '_ {
        self.parts
            .iter()
            .flat_map(|part| part.clips.iter().map(move |clip| (clip, part)))
    }
}

/// One score's clips and the performances they pass through on the way to an instrument.
#[derive(Clone, Debug)]
pub struct PlayedPart<'a> {
    /// The track whose score this is.
    pub source: TrackId,
    /// Its clips.
    pub clips: &'a [MidiClip],
    /// The stacks of the performances on the path, nearest the score first.
    pub stages: Vec<&'a [NoteTransform]>,
}

impl PlayedPart<'_> {
    /// The notes `clip` sounds on this track, positions relative to the clip.
    ///
    /// The one answer the renderer and the MIDI writer share, for the reason
    /// [`MidiClip::sounding_notes_with_meter`] gives. Each performance runs over the whole
    /// clip's sounding notes, every loop pass at once, as one phrase.
    pub fn sounding_notes(
        &self,
        clip: &MidiClip,
        bpm: f64,
        signatures: crate::time::SignatureMap,
    ) -> Vec<Note> {
        let notes: Vec<Note> = clip
            .sounding_notes_with_meter(bpm, signatures.clone())
            .collect();
        self.through_stages(notes, clip, bpm, signatures)
    }

    /// `notes`, positioned in `clip`, through this track's performances alone — not the clip's
    /// own stack. What a display drawing a layer's borrowed clip as written wants.
    pub fn through_stages(
        &self,
        mut notes: Vec<Note>,
        clip: &MidiClip,
        bpm: f64,
        signatures: crate::time::SignatureMap,
    ) -> Vec<Note> {
        for stage in &self.stages {
            if stage.is_empty() {
                continue;
            }
            notes = performed_notes(
                notes,
                stage,
                PerformanceContext {
                    bpm,
                    pass: 0,
                    start: clip.start,
                    length: clip.sounding_length(),
                    signatures: &signatures,
                },
            );
        }
        notes
    }
}

impl Project {
    /// What `track`'s instrument plays, or `None` when it is not a note track.
    ///
    /// One part per path into the instrument: its own score's clips, another track's through the
    /// performances on the way, or nothing at all from a path whose source is missing or cannot
    /// be followed. An instrument nothing feeds plays nothing.
    pub fn played_clips<'a>(&'a self, track: &'a Track) -> Option<PlayedClips<'a>> {
        track.kind.note_clips()?;
        let parts = self
            .routing
            .paths_into(track.id)
            .into_iter()
            .filter(|path| self.may_play_score(track.id, path.source))
            .filter_map(|path| {
                let clips = self.track(path.source)?.kind.note_clips()?;
                let stages = path
                    .performances
                    .iter()
                    .filter_map(|id| self.routing.performance(*id))
                    .map(|performance| performance.transforms.as_slice())
                    .collect();
                Some(PlayedPart {
                    source: path.source,
                    clips: clips.as_slice(),
                    stages,
                })
            })
            .collect();
        Some(PlayedClips { parts })
    }

    /// Feeds `track`'s instrument from `source`'s score as well as whatever already feeds it.
    ///
    /// `false`, changing nothing, when the instrument may not play that score or already plays it
    /// straight.
    pub fn connect_score(&mut self, source: TrackId, track: TrackId) -> bool {
        let edge = super::Edge::notes(
            super::NotesNode::Score(source),
            super::NotesNode::Instrument(track),
        );
        if !self.may_play_score(track, source) || self.routing.edges().contains(&edge) {
            return false;
        }
        self.routing.push(edge);
        true
    }

    /// The track layer `track` plays the notes of, when it is a layer.
    pub fn layer_source(&self, track: TrackId) -> Option<TrackId> {
        self.routing.source_of(track)
    }

    /// The layers following `source`, in track order.
    pub fn layers_of(&self, source: TrackId) -> Vec<TrackId> {
        let layers = self.routing.layers_of(source);
        self.tracks
            .iter()
            .map(|track| track.id)
            .filter(|track| layers.contains(track))
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
        if !self.can_be_followed(source) {
            return None;
        }
        let below = self.layers_of(source).last().copied().unwrap_or(source);
        let at = self.track_index(below)? + 1;
        let id = self.add_instrument_track(name, instrument_id);
        self.routing.set_follows(id, Some(source));
        self.move_track(id, at);
        Some(id)
    }

    /// Stands a new, empty performance on the path into layer `layer`, directly in front of it,
    /// and returns its id. `None` when `layer` is no layer.
    pub fn add_performance(&mut self, layer: TrackId) -> Option<PerformanceId> {
        if !self.routing.is_layer(layer) {
            return None;
        }
        let id = PerformanceId(self.allocate_id());
        self.routing
            .insert_performance(layer, Performance::new(id))
            .then_some(id)
    }

    /// Turns a layer back into a track with notes of its own, its instrument playing its own score.
    ///
    /// Every part it played is copied in. One with no performance on its path copies its source's
    /// clips. One with a performance
    /// freezes the complete performed notes, including each loop pass, before running them
    /// through the performances. Its performance stack and recipe are then removed so neither can
    /// rewrite the frozen part. If flattening changes the clip boundaries, curves retain their
    /// original pass boundaries in separate clips, independently of moved attacks and releases.
    /// `false` when `id` is not a layer. A missing source leaves no clips.
    pub fn detach_layer(&mut self, id: TrackId) -> bool {
        let Some(played) = self.track(id).and_then(|track| self.played_clips(track)) else {
            return false;
        };
        if !self.routing.is_layer(id) {
            return false;
        }
        let parts: Vec<(Vec<Vec<NoteTransform>>, Vec<MidiClip>)> = played
            .parts
            .iter()
            .map(|part| {
                (
                    part.stages.iter().map(|stage| stage.to_vec()).collect(),
                    part.clips.to_vec(),
                )
            })
            .collect();
        let mut copies = Vec::new();
        for (stages, originals) in parts {
            let unchanged = stages.iter().all(Vec::is_empty);
            for clip in originals {
                if unchanged {
                    copies.push(clip);
                } else {
                    copies.extend(detached_performance(
                        clip,
                        &stages,
                        &self.tempo_map,
                        &self.signatures,
                    ));
                }
            }
        }
        for clip in &mut copies {
            clip.id = ClipId(self.allocate_id());
        }
        let Some(TrackKind::Instrument(inner)) = self.track_mut(id).map(|track| &mut track.kind)
        else {
            return false;
        };
        inner.clips = copies;
        self.routing.set_follows(id, None);
        true
    }
}

/// Freeze notes and controls separately: a layer can move notes across a loop boundary, but
/// its channel curves still reset at the source pass's original boundary.
fn detached_performance(
    original: MidiClip,
    stages: &[Vec<NoteTransform>],
    tempo: &crate::TempoMap,
    signatures: &crate::SignatureMap,
) -> Vec<MidiClip> {
    let mut notes = original.clone();
    let played = PlayedPart {
        source: TrackId(0),
        clips: std::slice::from_ref(&original),
        stages: stages.iter().map(Vec::as_slice).collect(),
    };
    notes.notes =
        played.sounding_notes(&original, tempo.bpm_at(original.start), signatures.clone());
    notes.length = notes
        .notes
        .iter()
        .map(Note::end)
        .fold(original.sounding_length(), Ticks::max);
    notes.loop_end = Ticks::ZERO;
    notes.length_is_explicit = true;
    notes.transforms.clear();
    notes.recipe = None;
    notes.bend.clear();
    notes.controllers.clear();
    let curves: Vec<_> = original.performance_curves().collect();
    // The usual one-pass phrase needs only one clip. Separate controls are necessary only
    // when flattening changes a curve's loop boundaries or a shaped note extends its window.
    if !original.is_looped() && notes.length == original.length {
        for which in curves {
            *notes.curve_mut(which) = original.performed_curve_points(
                which,
                tempo,
                signatures,
                0,
                Ticks::ZERO,
                original.length,
            );
        }
        return vec![notes];
    }
    let mut copies = vec![notes];
    if !curves.is_empty() {
        for (pass, (offset, span)) in
            super::loop_passes(original.length, original.loop_end).enumerate()
        {
            let mut controls = MidiClip::new(
                original.id,
                original.name.clone(),
                original.start + offset,
                span,
            );
            controls.muted = original.muted;
            controls.length_is_explicit = true;
            for &which in &curves {
                *controls.curve_mut(which) = original.performed_curve_points(
                    which,
                    tempo,
                    signatures,
                    pass as u64,
                    offset,
                    span,
                );
            }
            copies.push(controls);
        }
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ChordVoice;

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

    /// The lead with a held C major triad under a melody of E5 then G5.
    fn project_with_voiced_lead() -> (Project, TrackId) {
        let (mut project, lead) = project_with_lead();
        let clip = project.track(lead).unwrap().kind.note_clips().unwrap()[0].id;
        project.midi_clip_mut(clip).unwrap().notes = vec![
            note(48, 0, 1920),
            note(52, 0, 1920),
            note(55, 0, 1920),
            note(76, 0, 480),
            note(79, 480, 480),
        ];
        (project, lead)
    }

    /// What `layer` sounds for its source's first clip.
    fn heard(project: &Project, layer: TrackId) -> Vec<Note> {
        let played = project.played_clips(project.track(layer).unwrap()).unwrap();
        let part = &played.parts[0];
        part.sounding_notes(&part.clips[0], 120.0, project.signatures.clone())
    }

    /// A layer on `lead` with one performance holding `transforms`.
    fn performed_layer(
        project: &mut Project,
        lead: TrackId,
        transforms: Vec<NoteTransform>,
    ) -> TrackId {
        let layer = project.add_layer_track(lead, "Layer", "pad").unwrap();
        let performance = project.add_performance(layer).unwrap();
        project
            .routing
            .performance_mut(performance)
            .unwrap()
            .transforms = transforms;
        layer
    }

    #[test]
    fn a_layer_with_no_performance_plays_its_source_note_for_note() {
        let (mut project, lead) = project_with_voiced_lead();
        let layer = project.add_layer_track(lead, "Pad", "pad").unwrap();
        let source = &project.track(lead).unwrap().kind.note_clips().unwrap()[0];
        let expected: Vec<Note> = source.sounding_notes(120.0).collect();
        assert_eq!(heard(&project, layer), expected);
    }

    #[test]
    fn a_key_range_before_a_transpose_chooses_by_the_written_pitch() {
        let (mut project, lead) = project_with_voiced_lead();
        let layer = performed_layer(
            &mut project,
            lead,
            vec![
                NoteTransform::KeyRange {
                    lowest: 0,
                    highest: 59,
                },
                NoteTransform::Transpose { semitones: -12 },
            ],
        );
        assert_eq!(pitches(&heard(&project, layer)), vec![36, 40, 43]);
    }

    #[test]
    fn the_top_and_bottom_voices_are_the_melody_and_the_root() {
        for (voice, expected) in [
            (ChordVoice::Top, vec![76, 79]),
            (ChordVoice::Bottom, vec![48]),
        ] {
            let (mut project, lead) = project_with_voiced_lead();
            let layer = performed_layer(&mut project, lead, vec![NoteTransform::Voice { voice }]);
            assert_eq!(pitches(&heard(&project, layer)), expected, "{voice:?}");
        }
    }

    #[test]
    fn velocity_length_and_delay_are_scaled_and_clamped() {
        let (mut project, lead) = project_with_lead();
        let layer = performed_layer(
            &mut project,
            lead,
            vec![
                NoteTransform::Velocity { scale: 0.5 },
                NoteTransform::Length { factor: 0.25 },
                NoteTransform::Delay { ticks: 240 },
            ],
        );
        let shaped = heard(&project, layer);
        assert_eq!(shaped[0].velocity, 0.4);
        assert_eq!(shaped[0].length, Ticks(120));
        assert_eq!(shaped[0].start, Ticks(240));

        let layer = performed_layer(
            &mut project,
            lead,
            vec![
                NoteTransform::Velocity { scale: 9.0 },
                NoteTransform::Delay { ticks: -100_000 },
            ],
        );
        let shaped = heard(&project, layer);
        assert_eq!(
            shaped[0].velocity, 1.0,
            "the result is clamped to full scale"
        );
        assert_eq!(
            shaped[1].start,
            Ticks::ZERO,
            "an early note is held at the clip start"
        );
    }

    #[test]
    fn a_layer_sits_below_its_source_and_plays_its_clips() {
        let (mut project, lead) = project_with_lead();
        let first = project.add_layer_track(lead, "Pad", "pad").unwrap();
        let second = project.add_layer_track(lead, "Bass", "bass").unwrap();
        let order: Vec<_> = project.tracks.iter().map(|track| track.id).collect();
        assert_eq!(&order[..3], &[lead, first, second]);

        assert!(
            !project.holds_notes(first),
            "a layer takes no clips of its own"
        );
        let layer = project.track(first).unwrap();
        let played = project.played_clips(layer).unwrap();
        assert_eq!(played.clips().count(), 1);
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
        project.routing.set_follows(layer, Some(drums));
        let track = project.track(layer).unwrap();
        assert_eq!(project.played_clips(track).unwrap().clips().count(), 0);
    }

    #[test]
    fn detaching_writes_the_performance_into_copied_clips() {
        let (mut project, lead) = project_with_lead();
        let layer = performed_layer(
            &mut project,
            lead,
            vec![NoteTransform::Transpose { semitones: 7 }],
        );
        assert!(project.detach_layer(layer));

        let source_clip = project.track(lead).unwrap().kind.note_clips().unwrap()[0].id;
        let detached = project.track(layer).unwrap();
        assert!(project.layer_source(layer).is_none());
        assert!(
            project.routing.performances().is_empty(),
            "the performance went with the path it stood on"
        );
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
    fn detaching_an_unchanged_layer_keeps_its_editable_loop_and_performance() {
        let (mut project, lead) = project_with_lead();
        let clip = &mut project
            .track_mut(lead)
            .unwrap()
            .kind
            .note_clips_mut()
            .unwrap()[0];
        clip.loop_end = Ticks(5000);
        clip.transforms.push(NoteTransform::Humanize {
            amount: 0.4,
            seed: 13,
        });
        clip.recipe = Some(super::super::ClipRecipe::new(
            super::super::ClipPreset::Lead,
            3,
        ));
        let mut original = clip.clone();
        let layer = project.add_layer_track(lead, "Layer", "pad").unwrap();
        project.detach_layer(layer);
        let detached = &project.track(layer).unwrap().kind.note_clips().unwrap()[0];
        assert_ne!(detached.id, original.id);
        original.id = detached.id;
        assert_eq!(detached, &original);
    }

    #[test]
    fn detaching_freezes_every_performed_pass_before_performing_it_again() {
        use super::super::{ClipPreset, ClipRecipe, PitchPerformance};

        for looped in [false, true] {
            let (mut project, lead) = project_with_lead();
            let clip = &mut project
                .track_mut(lead)
                .unwrap()
                .kind
                .note_clips_mut()
                .unwrap()[0];
            clip.start = Ticks(137);
            clip.length = Ticks(960);
            clip.loop_end = if looped { Ticks(2417) } else { Ticks::ZERO };
            clip.notes[0].lyric = "la".into();
            clip.notes[0].phonemes = vec!["l".into(), "a".into()];
            clip.notes[0].phoneme_seconds = vec![0.05, 0.2];
            clip.recipe = Some(ClipRecipe::new(ClipPreset::Lead, 42));
            clip.transforms = vec![
                NoteTransform::Transpose { semitones: 12 },
                NoteTransform::Humanize {
                    amount: 0.7,
                    seed: 19,
                },
                NoteTransform::Expression {
                    settings: super::super::Expression {
                        timing: 0.2,
                        velocity: 0.3,
                        shared: 0.4,
                        seed: 27,
                        ..super::super::Expression::default()
                    },
                },
                NoteTransform::Pitch {
                    settings: PitchPerformance {
                        vibrato: 0.2,
                        modulation: 0.3,
                        volume_swell: 0.4,
                        ..PitchPerformance::default()
                    },
                },
            ];
            clip.bend = vec![super::super::CurvePoint {
                at: Ticks::ZERO,
                value: 0.5,
            }];
            clip.controllers.insert(
                11,
                vec![super::super::CurvePoint {
                    at: Ticks(100),
                    value: 0.6,
                }],
            );
            let layer = performed_layer(
                &mut project,
                lead,
                vec![
                    NoteTransform::KeyRange {
                        lowest: 72,
                        highest: 84,
                    },
                    NoteTransform::Velocity { scale: 0.7 },
                    NoteTransform::Length {
                        factor: if looped { 2.0 } else { 0.7 },
                    },
                    NoteTransform::Delay { ticks: -100 },
                ],
            );

            let played = project.played_clips(project.track(layer).unwrap()).unwrap();
            let part = &played.parts[0];
            let expected = part.sounding_notes(&part.clips[0], 120.0, project.signatures.clone());
            let original = part.clips[0].clone();
            let expected_curves: Vec<_> = original
                .performance_curves()
                .map(|which| {
                    (
                        which,
                        original.sounding_performance_curve_events(
                            which,
                            super::super::CURVE_STEP,
                            &project.tempo_map,
                            &project.signatures,
                        ),
                    )
                })
                .collect();
            assert!(!expected.is_empty());
            project.detach_layer(layer);
            let detached = project.track(layer).unwrap().kind.note_clips().unwrap();
            if !looped {
                assert_eq!(detached.len(), 1);
            }
            assert_eq!(detached[0].notes, expected);
            assert_eq!(
                detached[0].sounding_notes(120.0).collect::<Vec<_>>(),
                expected
            );
            assert!(detached.iter().all(|clip| clip.transforms.is_empty()
                && clip.recipe.is_none()
                && clip.loop_end == Ticks::ZERO));
            for (which, expected) in expected_curves {
                let actual: Vec<_> = detached
                    .iter()
                    .flat_map(|clip| {
                        clip.sounding_performance_curve_events(
                            which,
                            super::super::CURVE_STEP,
                            &project.tempo_map,
                            &project.signatures,
                        )
                        .into_iter()
                        .map(|(at, value)| (clip.start - original.start + at, value))
                    })
                    .collect();
                assert_eq!(actual, expected, "curve {which:?} changed during detach");
            }
            assert_eq!(
                project.track(lead).unwrap().kind.note_clips().unwrap()[0],
                original
            );
            let restored: Project =
                serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
            assert_eq!(restored.track(layer), project.track(layer));
        }
    }

    #[test]
    fn a_layer_round_trips_and_one_without_a_performance_stores_none() {
        let (mut project, lead) = project_with_lead();
        let layer = project.add_layer_track(lead, "Pad", "pad").unwrap();
        let stored = serde_json::to_value(&project.routing).unwrap();
        assert!(stored.get("performances").is_none());
        let notes = stored["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["kind"] == "notes" && edge["to"]["instrument"] == layer.0)
            .unwrap();
        assert_eq!(notes["from"]["score"], serde_json::json!(lead.0));
        let back: super::super::Routing = serde_json::from_value(stored).unwrap();
        assert_eq!(back.source_of(layer), Some(lead));
        assert_eq!(back.path_into(layer).unwrap().performances, []);
    }

    #[test]
    fn an_instrument_plays_every_score_that_reaches_it_merged() {
        let (mut project, lead) = project_with_lead();
        let pad = project.add_instrument_track("Pad", "pad");
        let clip = project
            .add_midi_clip(pad, "B", Ticks::ZERO, Ticks(1920))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = vec![note(48, 0, 1920)];
        assert!(project.connect_score(lead, pad));
        assert!(!project.connect_score(lead, pad), "already playing it");
        assert!(project.holds_notes(pad), "it still plays its own score");

        let played = project.played_clips(project.track(pad).unwrap()).unwrap();
        let sources: Vec<TrackId> = played.parts.iter().map(|part| part.source).collect();
        assert_eq!(sources, [pad, lead]);
        assert_eq!(played.clips().count(), 2);

        // The lead's own instrument is untouched.
        let own = project.played_clips(project.track(lead).unwrap()).unwrap();
        assert_eq!(own.parts.len(), 1);
    }

    #[test]
    fn a_drum_score_plays_only_its_own_instrument() {
        let (mut project, lead) = project_with_lead();
        let drums = project.add_drum_track("Drums", "kit");
        assert!(!project.connect_score(drums, lead));
        // A file that says otherwise loses that edge and keeps the rest.
        project.routing.push(super::super::Edge::notes(
            super::super::NotesNode::Score(drums),
            super::super::NotesNode::Instrument(lead),
        ));
        assert!(project.repair_routing());
        let played = project.played_clips(project.track(lead).unwrap()).unwrap();
        assert_eq!(played.parts.len(), 1);
        assert_eq!(played.parts[0].source, lead);
    }
}
