//! Layers: stacking another instrument on a part without copying its notes.
//!
//! A layer is an instrument track that plays a source track's clips through a
//! [`LayerShape`]; [`auris_core::NoteSource`] is the account of what it is. These are the
//! commands that make one, reshape one, and turn one back into a track with notes of its own.
//! Removing a source detaches its layers rather than silencing them, which is
//! [`Session::remove_track`]'s business and reuses [`Session::detach_layer`]'s core.

use auris_core::{LayerShape, TrackId};

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
        if !kind.can_be_followed() {
            return Err(SessionError::WrongTrackKind {
                id: source.0,
                actual: if kind.layer().is_some() {
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

    /// The shape a layer plays its source's notes through, or `None` when `track` is no layer.
    pub fn layer_shape(&self, track: TrackId) -> Option<&LayerShape> {
        self.project
            .track(track)
            .and_then(|track| track.kind.layer())
            .map(|source| &source.shape)
    }

    /// The track a layer follows, or `None` when `track` is no layer.
    pub fn layer_source(&self, track: TrackId) -> Option<TrackId> {
        self.project
            .track(track)
            .and_then(|track| track.kind.layer())
            .map(|source| source.track)
    }

    /// Replaces how a layer shapes the notes it borrows.
    pub fn set_layer_shape(
        &mut self,
        track: TrackId,
        shape: LayerShape,
    ) -> Result<(), SessionError> {
        self.require_layer(track)?;
        if self.layer_shape(track) == Some(&shape) {
            return Ok(());
        }
        self.record(Edit::SetLayerShape);
        if let Some(source) = self
            .project
            .track_mut(track)
            .and_then(|track| track.kind.layer_mut())
        {
            source.shape = shape;
        }
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
        let kind = &self.project.tracks[index].kind;
        match kind.layer() {
            Some(_) => Ok(index),
            None => Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: kind.label(),
                expected: "a layer",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use auris_core::time::Ticks;
    use auris_core::{LayerVoice, Note};
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
        let shape = LayerShape {
            lowest: 100,
            ..LayerShape::default()
        };
        session.set_layer_shape(layer, shape.clone()).unwrap();
        assert_eq!(session.layer_shape(layer), Some(&shape));
        assert!(peak(&mut session) < 1e-6);

        session.undo().unwrap();
        assert!(session.layer_shape(layer).unwrap().is_identity());
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
                .set_layer_shape(drums, LayerShape::default())
                .is_err()
        );
    }

    #[test]
    fn detaching_keeps_the_sound_and_one_undo_restores_the_layer() {
        let (mut session, lead, layer) = session_with_layer();
        let shape = LayerShape {
            voice: LayerVoice::Top,
            transpose: 12,
            ..LayerShape::default()
        };
        session.set_layer_shape(layer, shape).unwrap();
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
}
