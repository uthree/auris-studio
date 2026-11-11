//! Layers: stacking another instrument on a part without copying its notes.
//!
//! A layer is an instrument track that plays a source track's clips through the
//! [`Performance`](auris_core::Performance) nodes on its notes path. These are the commands that
//! make one, shape one, and turn one back into a track with notes of its own. Removing a source
//! detaches its layers rather than silencing them, which is [`Session::remove_track`]'s business
//! and reuses [`Session::detach_layer`]'s core.
//!
//! Shaping goes through [`LayerDials`]: the six transforms a layer is usually shaped with, read
//! from and written into the performance standing directly in front of the layer. The panel and
//! the agent both speak dials; the document holds only the transforms.

use auris_core::{ChordVoice, NoteTransform, PerformanceId, Ticks, TrackId};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// Adds a layer playing `instrument_id` on `source`'s notes, below the source and the layers
    /// already on it.
    ///
    /// Refused when the source is missing, is a drum track, or is itself a layer — following
    /// always names a track whose notes are its own.
    pub fn add_layer_track(
        &mut self,
        source: TrackId,
        name: impl Into<String>,
        instrument_id: &str,
    ) -> Result<TrackId, SessionError> {
        let index = self.require_track(source)?;
        let kind = &self.project.tracks[index].kind;
        if !self.project.can_be_followed(source) {
            return Err(SessionError::WrongTrackKind {
                id: source.0,
                actual: if self.project.routing.is_layer(source) {
                    "a layer"
                } else {
                    kind.label()
                },
                expected: "a melodic or singer track with notes of its own",
            });
        }
        if !self.registry.has_instrument(instrument_id) {
            return Err(SessionError::UnknownPlugin(instrument_id.to_string()));
        }
        self.record(Edit::AddLayerTrack);
        let id = self
            .project
            .add_layer_track(source, name, instrument_id)
            .ok_or(SessionError::UnknownTrack(source.0))?;
        self.invalidate_graph();
        Ok(id)
    }

    /// Adds a layer on `source` playing whatever the registry nominates as its default instrument.
    pub fn add_default_layer_track(
        &mut self,
        source: TrackId,
        name: impl Into<String>,
    ) -> Result<TrackId, SessionError> {
        let instrument = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        self.add_layer_track(source, name, &instrument)
    }

    /// The dials of the performance directly in front of a layer, or `None` when `track` is no
    /// layer. A layer with no performance there reads every dial at its identity.
    pub fn layer_dials(&self, track: TrackId) -> Option<LayerDials> {
        if !self.project.routing.is_layer(track) {
            return None;
        }
        let transforms = self
            .layer_performance(track)
            .and_then(|id| self.project.routing.performance(id))
            .map_or(&[][..], |performance| performance.transforms.as_slice());
        Some(LayerDials::read(transforms))
    }

    /// The performance standing directly in front of a layer, if one does.
    pub fn layer_performance(&self, track: TrackId) -> Option<PerformanceId> {
        self.project.routing.performance_into(track)
    }

    /// The track a layer follows, or `None` when `track` is no layer.
    pub fn layer_source(&self, track: TrackId) -> Option<TrackId> {
        self.project.layer_source(track)
    }

    /// Sets a layer's dials, as one undo step.
    ///
    /// The dials are written into the performance in front of the layer, which is made when there
    /// is none and taken off the path again when writing leaves it empty, so a layer at its
    /// identity has no performance standing in front of it.
    pub fn set_layer_dials(
        &mut self,
        track: TrackId,
        dials: LayerDials,
    ) -> Result<(), SessionError> {
        self.require_layer(track)?;
        if self.layer_dials(track).as_ref() == Some(&dials) {
            return Ok(());
        }
        self.record(Edit::SetLayerShape);
        let performance = match self.layer_performance(track) {
            Some(id) => Some(id),
            None if dials.is_identity() => None,
            None => self.project.add_performance(track),
        };
        if let Some(id) = performance {
            let empty = match self.project.routing.performance_mut(id) {
                Some(performance) => {
                    dials.write_into(&mut performance.transforms);
                    performance.transforms.is_empty()
                }
                None => false,
            };
            if empty {
                self.project.routing.bypass_performance(id);
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Takes a performance off its notes path, joining what fed it straight to what it fed, as
    /// one undo step. Refused when there is no such performance.
    pub fn remove_performance(&mut self, id: PerformanceId) -> Result<(), SessionError> {
        if self.project.routing.performance(id).is_none() {
            return Err(SessionError::UnknownMixerNode(format!(
                "performance:{}",
                id.0
            )));
        }
        self.record(Edit::SetLayerShape);
        self.project.routing.bypass_performance(id);
        self.invalidate_graph();
        Ok(())
    }

    /// Turns a layer back into a track with notes of its own: the source's clips are copied, with
    /// the shape written into their notes, so the track sounds as it did.
    pub fn detach_layer(&mut self, track: TrackId) -> Result<(), SessionError> {
        self.require_layer(track)?;
        self.record(Edit::DetachLayer);
        self.project.detach_layer(track);
        self.invalidate_graph();
        Ok(())
    }

    /// Detaches every layer on `source`, inside whatever step is already recording.
    pub(super) fn detach_layers_of(&mut self, source: TrackId) {
        for layer in self.project.layers_of(source) {
            self.project.detach_layer(layer);
        }
    }

    fn require_layer(&self, track: TrackId) -> Result<usize, SessionError> {
        let index = self.require_track(track)?;
        if self.project.routing.is_layer(track) {
            Ok(index)
        } else {
            Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: self.project.tracks[index].kind.label(),
                expected: "a layer",
            })
        }
    }
}

/// The six dials a layer is usually shaped with, as a view over a performance's transforms.
///
/// Each dial stands for one kind of [`NoteTransform`] — a key range, a voice, a transpose, a
/// velocity scale, a length scale and a delay — and reads the first of that kind in the stack,
/// or its identity when there is none. Writing changes that transform in place, adds one at the
/// end when there was none, and removes it when the dial is set back to its identity; every other
/// transform in the stack is left exactly where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerDials {
    /// Lowest pitch kept, inclusive.
    pub lowest: u8,
    /// Highest pitch kept, inclusive.
    pub highest: u8,
    /// The outer voice kept, or `None` for every note.
    pub voice: Option<ChordVoice>,
    /// Semitones up or down.
    pub transpose: i32,
    /// Velocity multiplier, 0..=2.
    pub velocity: f32,
    /// Length multiplier, 0.05..=4.
    pub length: f32,
    /// Ticks late, or early when negative.
    pub offset: Ticks,
}

impl Default for LayerDials {
    fn default() -> Self {
        Self {
            lowest: 0,
            highest: 127,
            voice: None,
            transpose: 0,
            velocity: 1.0,
            length: 1.0,
            offset: Ticks::ZERO,
        }
    }
}

/// Which dial a transform belongs to, if any.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Dial {
    Range,
    Voice,
    Transpose,
    Velocity,
    Length,
    Offset,
}

fn dial_of(transform: &NoteTransform) -> Option<Dial> {
    match transform {
        NoteTransform::KeyRange { .. } => Some(Dial::Range),
        NoteTransform::Voice { .. } => Some(Dial::Voice),
        NoteTransform::Transpose { .. } => Some(Dial::Transpose),
        NoteTransform::Velocity { .. } => Some(Dial::Velocity),
        NoteTransform::Length { .. } => Some(Dial::Length),
        NoteTransform::Delay { .. } => Some(Dial::Offset),
        _ => None,
    }
}

impl LayerDials {
    /// `true` when every dial is at its identity.
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// The dials `transforms` sets.
    pub fn read(transforms: &[NoteTransform]) -> Self {
        let mut dials = Self::default();
        let mut seen = Vec::new();
        for transform in transforms {
            let Some(dial) = dial_of(transform) else {
                continue;
            };
            if seen.contains(&dial) {
                continue;
            }
            seen.push(dial);
            match *transform {
                NoteTransform::KeyRange { lowest, highest } => {
                    dials.lowest = lowest.min(highest);
                    dials.highest = lowest.max(highest);
                }
                NoteTransform::Voice { voice } => dials.voice = Some(voice),
                NoteTransform::Transpose { semitones } => dials.transpose = semitones,
                NoteTransform::Velocity { scale } => dials.velocity = scale,
                NoteTransform::Length { factor } => dials.length = factor,
                NoteTransform::Delay { ticks } => dials.offset = Ticks(ticks),
                _ => {}
            }
        }
        dials
    }

    /// The transform one dial stands for, or `None` at its identity.
    fn transform(&self, dial: Dial) -> Option<NoteTransform> {
        let identity = Self::default();
        match dial {
            Dial::Range => ((self.lowest, self.highest) != (identity.lowest, identity.highest))
                .then_some(NoteTransform::KeyRange {
                    lowest: self.lowest,
                    highest: self.highest,
                }),
            Dial::Voice => self.voice.map(|voice| NoteTransform::Voice { voice }),
            Dial::Transpose => (self.transpose != 0).then_some(NoteTransform::Transpose {
                semitones: self.transpose,
            }),
            Dial::Velocity => (self.velocity != 1.0).then_some(NoteTransform::Velocity {
                scale: self.velocity,
            }),
            Dial::Length => (self.length != 1.0).then_some(NoteTransform::Length {
                factor: self.length,
            }),
            Dial::Offset => (self.offset != Ticks::ZERO).then_some(NoteTransform::Delay {
                ticks: self.offset.raw(),
            }),
        }
    }

    /// Writes these dials into `transforms`, touching only the transforms they stand for.
    pub fn write_into(&self, transforms: &mut Vec<NoteTransform>) {
        for dial in [
            Dial::Range,
            Dial::Voice,
            Dial::Transpose,
            Dial::Velocity,
            Dial::Length,
            Dial::Offset,
        ] {
            let wanted = self.transform(dial);
            match transforms
                .iter()
                .position(|transform| dial_of(transform) == Some(dial))
            {
                Some(at) => match wanted {
                    Some(transform) => transforms[at] = transform,
                    None => {
                        transforms.remove(at);
                    }
                },
                None => transforms.extend(wanted),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auris_core::Note;
    use auris_engine::{OfflineOptions, RenderProgress};

    use super::super::fixtures::{session, undo_depth};
    use super::*;

    /// A session with a lead playing a C major triad for a bar, and a layer on it.
    fn session_with_layer() -> (Session, TrackId, TrackId) {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(lead, "Chords", Ticks::ZERO, Ticks(1920))
            .unwrap();
        session.project.midi_clip_mut(clip).unwrap().notes = [60, 64, 67]
            .into_iter()
            .map(|pitch| Note {
                velocity: 0.8,
                ..Note::new(pitch, Ticks::ZERO, Ticks(960))
            })
            .collect();
        let instrument = session
            .registry
            .default_instrument_id()
            .unwrap()
            .to_string();
        let layer = session.add_layer_track(lead, "Layer", &instrument).unwrap();
        session.forget_history();
        (session, lead, layer)
    }

    fn peak(session: &mut Session) -> f32 {
        session
            .render_job()
            .render(
                &OfflineOptions::whole_project(),
                &mut RenderProgress::default(),
            )
            .unwrap()
            .peak()
    }

    #[test]
    fn a_layer_sounds_its_source_notes_with_the_source_muted() {
        let (mut session, lead, layer) = session_with_layer();
        session.set_track_mute(lead, true).unwrap();
        assert!(
            peak(&mut session) > 0.01,
            "the layer plays the lead's notes"
        );

        session.set_track_mute(layer, true).unwrap();
        assert!(peak(&mut session) < 1e-6, "and nothing else was playing");
    }

    #[test]
    fn a_shape_that_keeps_nothing_silences_only_the_layer() {
        let (mut session, lead, layer) = session_with_layer();
        session.set_track_mute(lead, true).unwrap();
        let shape = LayerDials {
            lowest: 100,
            ..LayerDials::default()
        };
        session.set_layer_dials(layer, shape.clone()).unwrap();
        assert_eq!(session.layer_dials(layer), Some(shape));
        assert!(
            session.layer_performance(layer).is_some(),
            "the dials live in a performance in front of the layer"
        );
        assert!(peak(&mut session) < 1e-6);

        session.undo().unwrap();
        assert!(session.layer_dials(layer).unwrap().is_identity());
        assert!(session.layer_performance(layer).is_none());
        assert!(peak(&mut session) > 0.01);
    }

    #[test]
    fn layers_cannot_take_clips_or_be_followed() {
        let (mut session, _, layer) = session_with_layer();
        assert!(matches!(
            session.add_midi_clip(layer, "Mine", Ticks::ZERO, Ticks(480)),
            Err(SessionError::WrongTrackKind { .. })
        ));
        let instrument = session
            .registry
            .default_instrument_id()
            .unwrap()
            .to_string();
        assert!(matches!(
            session.add_layer_track(layer, "Again", &instrument),
            Err(SessionError::WrongTrackKind {
                actual: "a layer",
                ..
            })
        ));
        let drums = session.add_default_drum_track("Drums").unwrap();
        assert!(session.add_layer_track(drums, "Kit", &instrument).is_err());
        assert!(
            session
                .set_layer_dials(drums, LayerDials::default())
                .is_err()
        );
    }

    #[test]
    fn detaching_keeps_the_sound_and_one_undo_restores_the_layer() {
        let (mut session, lead, layer) = session_with_layer();
        let shape = LayerDials {
            voice: Some(auris_core::ChordVoice::Top),
            transpose: 12,
            ..LayerDials::default()
        };
        session.set_layer_dials(layer, shape).unwrap();
        session.forget_history();

        session.detach_layer(layer).unwrap();
        assert_eq!(session.layer_source(layer), None);
        let clips = session
            .project
            .track(layer)
            .unwrap()
            .kind
            .note_clips()
            .unwrap();
        let pitches: Vec<u8> = clips[0].notes.iter().map(|note| note.pitch).collect();
        assert_eq!(pitches, vec![79], "only the top voice, an octave up");

        session.undo().unwrap();
        assert_eq!(session.layer_source(layer), Some(lead));
        assert_eq!(undo_depth(&mut session), 0);
    }

    #[test]
    fn removing_a_source_detaches_its_layers_in_the_same_step() {
        let (mut session, lead, layer) = session_with_layer();
        session.remove_track(lead).unwrap();
        assert_eq!(session.layer_source(layer), None);
        let clips = session
            .project
            .track(layer)
            .unwrap()
            .kind
            .note_clips()
            .unwrap();
        assert_eq!(clips[0].notes.len(), 3, "the part survives on the layer");
        assert!(peak(&mut session) > 0.01);

        session.undo().unwrap();
        assert_eq!(session.layer_source(layer), Some(lead));
        assert_eq!(undo_depth(&mut session), 0);
    }

    #[test]
    fn detaching_or_removing_a_performed_source_keeps_the_heard_part_and_undo() {
        for remove_source in [false, true] {
            let (mut session, lead, layer) = session_with_layer();
            let clip = session
                .project()
                .track(lead)
                .unwrap()
                .kind
                .note_clips()
                .unwrap()[0]
                .id;
            session
                .set_clip_transforms(
                    clip,
                    vec![auris_core::NoteTransform::Transpose { semitones: 12 }],
                )
                .unwrap();
            session
                .set_layer_dials(
                    layer,
                    LayerDials {
                        lowest: 72,
                        highest: 84,
                        ..LayerDials::default()
                    },
                )
                .unwrap();
            session.set_track_mute(lead, true).unwrap();
            session.forget_history();
            let before = session.project().clone();
            let heard = session
                .render_job()
                .render(
                    &OfflineOptions::whole_project(),
                    &mut RenderProgress::default(),
                )
                .unwrap();
            assert!(heard.peak() > 0.01);

            if remove_source {
                session.remove_track(lead).unwrap();
            } else {
                session.detach_layer(layer).unwrap();
            }
            let clips = session
                .project()
                .track(layer)
                .unwrap()
                .kind
                .note_clips()
                .unwrap();
            assert_eq!(
                clips[0]
                    .notes
                    .iter()
                    .map(|note| note.pitch)
                    .collect::<Vec<_>>(),
                vec![72, 76, 79]
            );
            let detached = session
                .render_job()
                .render(
                    &OfflineOptions::whole_project(),
                    &mut RenderProgress::default(),
                )
                .unwrap();
            assert_eq!(detached.frame_count(), heard.frame_count());
            for (before, after) in heard.channels().iter().zip(detached.channels()) {
                assert!(
                    before
                        .iter()
                        .zip(after)
                        .all(|(before, after)| (before - after).abs() < 1e-6)
                );
            }
            session.undo().unwrap();
            assert_eq!(session.project(), &before);
            assert_eq!(undo_depth(&mut session), 0);
        }
    }

    #[test]
    fn extreme_layer_transposition_builds_and_renders_at_the_midi_edges() {
        for (transpose, expected) in [(i32::MAX, 127), (i32::MIN, 0)] {
            let (mut session, lead, layer) = session_with_layer();
            session.set_track_mute(lead, true).unwrap();
            session
                .set_layer_dials(
                    layer,
                    LayerDials {
                        transpose,
                        ..LayerDials::default()
                    },
                )
                .unwrap();
            session.detach_layer(layer).unwrap();
            let notes = &session
                .project()
                .track(layer)
                .unwrap()
                .kind
                .note_clips()
                .unwrap()[0]
                .notes;
            assert!(notes.iter().all(|note| note.pitch == expected));
            assert!(peak(&mut session).is_finite());
        }
    }

    #[test]
    fn the_mixer_graph_draws_a_notes_wire_and_cutting_it_detaches() {
        use crate::mixer_graph::{MixerEdgeKind, MixerNode};

        let (mut session, lead, layer) = session_with_layer();
        let wire = session
            .mixer_graph()
            .edges
            .into_iter()
            .find(|edge| edge.kind == MixerEdgeKind::Notes)
            .expect("a layer is drawn as a wire from its source");
        assert_eq!(wire.source, MixerNode::Strip(Some(lead)));
        assert_eq!(wire.target, MixerNode::Strip(Some(layer)));

        assert!(session.disconnect_mixer_edges(&[wire]).unwrap());
        assert_eq!(session.layer_source(layer), None);
        assert!(
            !session
                .mixer_graph()
                .edges
                .iter()
                .any(|edge| edge.kind == MixerEdgeKind::Notes)
        );
        session.undo().unwrap();
        assert_eq!(session.layer_source(layer), Some(lead));
    }

    #[test]
    fn dials_touch_only_the_transforms_they_stand_for() {
        let mut transforms = vec![
            auris_core::NoteTransform::Swing {
                percent: 60,
                subdivision: auris_core::Subdivision::Eighth,
            },
            auris_core::NoteTransform::Transpose { semitones: 5 },
        ];
        let mut dials = LayerDials::read(&transforms);
        assert_eq!(dials.transpose, 5);
        dials.transpose = 0;
        dials.offset = Ticks(240);
        dials.write_into(&mut transforms);
        assert_eq!(
            transforms,
            [
                auris_core::NoteTransform::Swing {
                    percent: 60,
                    subdivision: auris_core::Subdivision::Eighth,
                },
                auris_core::NoteTransform::Delay { ticks: 240 },
            ],
            "the swing stays where it was, the transpose goes, the delay is added"
        );
        assert_eq!(LayerDials::read(&transforms), dials);
    }

    #[test]
    fn a_performance_is_its_own_node_on_the_notes_path() {
        use crate::mixer_graph::{MixerEdgeKind, MixerNode};

        let (mut session, lead, layer) = session_with_layer();
        session
            .set_layer_dials(
                layer,
                LayerDials {
                    transpose: 7,
                    ..LayerDials::default()
                },
            )
            .unwrap();
        let performance = session.layer_performance(layer).unwrap();
        let graph = session.mixer_graph();
        assert!(graph.nodes.contains(&MixerNode::Performance(performance)));
        let notes: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == MixerEdgeKind::Notes)
            .map(|edge| (edge.source, edge.target))
            .collect();
        assert_eq!(
            notes,
            [
                (
                    MixerNode::Strip(Some(lead)),
                    MixerNode::Performance(performance)
                ),
                (
                    MixerNode::Performance(performance),
                    MixerNode::Strip(Some(layer))
                ),
            ]
        );

        // Cutting the wire into the performance detaches the layer it fed.
        let into = *graph
            .edges
            .iter()
            .find(|edge| edge.target == MixerNode::Performance(performance))
            .unwrap();
        assert!(session.disconnect_mixer_edges(&[into]).unwrap());
        assert_eq!(session.layer_source(layer), None);
        assert!(session.project().routing.performances().is_empty());
    }
}
