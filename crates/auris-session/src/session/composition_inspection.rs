//! Current controls and explicit differences from the state first composed.
use super::Session;
use crate::param::ParamTarget;
use crate::prelude::*;
use auris_core::project::{CompositionBaseline, CompositionClipBaseline, CompositionTrackBaseline};
use serde_json::{Value, json};
use std::collections::BTreeSet;

impl Session {
    pub(super) fn composition_snapshot(&mut self) -> (CompositionBaseline, Vec<Value>) {
        let ids = self
            .project
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        let mut tracks = Vec::new();
        let mut instruments = Vec::new();
        for id in ids {
            let singer_id = self
                .project
                .track(id)
                .and_then(|track| track.kind.as_singer())
                .map(|singer| singer.instrument_id.clone());
            let descriptors = match singer_id {
                Some(instrument) => self.param_descriptors(&instrument),
                None => self.instrument_descriptors(id),
            };
            let Some(track) = self.project.track(id) else {
                continue;
            };
            let (instrument_id, instrument_file, mut parameters) =
                if let Some(inner) = track.kind.as_instrument() {
                    (
                        inner.instrument_id.clone(),
                        inner.file.clone(),
                        inner.instrument_state.params.clone(),
                    )
                } else if let Some(inner) = track.kind.as_singer() {
                    (
                        inner.instrument_id.clone(),
                        None,
                        inner.instrument_state.params.clone(),
                    )
                } else {
                    (String::new(), None, Default::default())
                };
            let mut parameters_available = !descriptors.is_empty();
            for descriptor in descriptors.iter() {
                let value = if track.kind.as_instrument().is_some() {
                    self.param_value(
                        ParamTarget::Instrument {
                            track: id,
                            param: descriptor.id,
                        },
                        descriptor,
                    )
                } else {
                    parameters
                        .get(descriptor.key.as_ref())
                        .copied()
                        .unwrap_or(descriptor.default)
                };
                if value.is_finite() {
                    parameters.insert(descriptor.key.to_string(), value);
                } else {
                    parameters_available = false;
                    parameters.remove(descriptor.key.as_ref());
                }
            }
            let controls = parameters.iter().map(|(key,value)| {
                let descriptor = descriptors.iter().find(|descriptor|descriptor.key == key.as_str());
                json!({"param":key,"value":value,"name":descriptor.map(|descriptor|descriptor.name.as_ref()),
                    "min":descriptor.map(|descriptor|descriptor.min),"max":descriptor.map(|descriptor|descriptor.max),
                    "unit":descriptor.map(|descriptor|format!("{:?}",descriptor.unit)),
                    "automated":descriptor.is_some_and(|descriptor|self.is_automated(ParamTarget::Instrument{track:id,param:descriptor.id}))})
            }).collect::<Vec<_>>();
            instruments.push(json!({"track":format!("id:{}",id.0),"id":id.0,"name":track.name,
                "kind":track.kind.label(),"instrument_id":instrument_id,"file":instrument_file,"parameters":controls,
                "parameter_metadata_available":parameters_available,"soundfont_preset":self.track_preset(id),
                "voice":track.kind.as_singer().and_then(|singer|singer.voice.as_ref())}));
            tracks.push(CompositionTrackBaseline {
                id,
                name: track.name.clone(),
                kind: match &track.kind {
                    TrackKind::Instrument(_) => "instrument",
                    TrackKind::Drum(_) => "drum",
                    TrackKind::Singer(_) => "singer",
                    TrackKind::Audio(_) => "audio",
                    TrackKind::Bus => "bus",
                }
                .into(),
                instrument_id,
                instrument_file,
                preset: self.track_preset(id),
                voice: track
                    .kind
                    .as_singer()
                    .and_then(|singer| singer.voice.clone()),
                parameters_available,
                parameters,
                clips: track
                    .kind
                    .note_clips()
                    .into_iter()
                    .flatten()
                    .map(|clip| CompositionClipBaseline {
                        id: clip.id,
                        name: clip.name.clone(),
                        start: clip.start,
                        length: clip.length,
                        notes_digest: auris_core::notes_digest(&clip.notes),
                        recipe: clip.recipe.clone(),
                        transforms: clip.transforms.clone(),
                    })
                    .collect(),
            });
        }
        (
            CompositionBaseline {
                tracks,
                harmony: self.project.harmony.clone(),
                tempo: self.project.tempo_map.clone(),
                meter: self.project.signatures.clone(),
                sections: self.project.sections.clone(),
            },
            instruments,
        )
    }

    /// Reads current instrument controls and differences from the stored composition baseline.
    /// Older projects without a baseline report that comparison is unavailable. No score is
    /// regenerated and the original song specification stays unchanged. Comparison covers
    /// sources, static controls, authored notes, recipes, transforms, clip placement and timeline;
    /// mixer/effect edits, automation lanes, audio takes and opaque plugin state remain in the project.
    pub fn inspect_composition_state(&mut self) -> Value {
        let (current, instruments) = self.composition_snapshot();
        let baseline = self.project.composition_baseline.as_ref();
        let mut changes = Vec::new();
        if let Some(baseline) = baseline {
            let mut timeline = Vec::new();
            if baseline.harmony != current.harmony {
                timeline.push("harmony");
            }
            if baseline.tempo != current.tempo {
                timeline.push("tempo");
            }
            if baseline.meter != current.meter {
                timeline.push("meter");
            }
            if baseline.sections != current.sections {
                timeline.push("sections");
            }
            if !timeline.is_empty() {
                changes.push(json!({"target":"timeline","fields":timeline}));
            }
            for track in &current.tracks {
                let Some(original) = baseline
                    .tracks
                    .iter()
                    .find(|original| original.id == track.id)
                else {
                    changes.push(json!({"track":format!("id:{}",track.id.0),"name":track.name,"status":"added"}));
                    continue;
                };
                let mut fields = Vec::new();
                if original.name != track.name {
                    fields.push("name");
                }
                let source_changed = original.kind != track.kind
                    || original.instrument_id != track.instrument_id
                    || original.instrument_file != track.instrument_file
                    || original.preset != track.preset
                    || original.voice != track.voice;
                if source_changed {
                    fields.push("instrument_source");
                }
                let parameters = if source_changed
                    || !original.parameters_available
                    || !track.parameters_available
                {
                    Vec::new()
                } else {
                    original
                        .parameters
                        .keys()
                        .chain(track.parameters.keys())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .filter_map(|key| {
                            let previous = original.parameters.get(key);
                            let value = track.parameters.get(key);
                            (previous != value)
                                .then(|| json!({"param":key,"previous":previous,"value":value}))
                        })
                        .collect::<Vec<_>>()
                };
                if !parameters.is_empty() {
                    fields.push("instrument_parameters");
                }
                let mut clips = Vec::new();
                for clip in &track.clips {
                    let Some(old) = original.clips.iter().find(|old| old.id == clip.id) else {
                        clips.push(json!({"clip_id":clip.id.0,"name":clip.name,"status":"added"}));
                        continue;
                    };
                    let mut altered = Vec::new();
                    if old.notes_digest != clip.notes_digest {
                        altered.push("authored_notes");
                    }
                    if old.recipe != clip.recipe {
                        altered.push("recipe");
                    }
                    if old.transforms != clip.transforms {
                        altered.push("performance");
                    }
                    if old.start != clip.start || old.length != clip.length {
                        altered.push("placement");
                    }
                    if old.name != clip.name {
                        altered.push("name");
                    }
                    if !altered.is_empty() {
                        clips.push(json!({"clip_id":clip.id.0,"name":clip.name,"status":"modified","fields":altered}));
                    }
                }
                for old in &original.clips {
                    if !track.clips.iter().any(|clip| clip.id == old.id) {
                        clips.push(json!({"clip_id":old.id.0,"name":old.name,"status":"removed"}));
                    }
                }
                if !clips.is_empty() {
                    fields.push("clips");
                }
                if !fields.is_empty() {
                    changes.push(json!({"track":format!("id:{}",track.id.0),"name":track.name,"status":"modified","fields":fields,"parameters":parameters,"clips":clips}));
                }
            }
            for original in &baseline.tracks {
                if !current.tracks.iter().any(|track| track.id == original.id) {
                    changes.push(json!({"track":format!("id:{}",original.id.0),"name":original.name,"status":"removed"}));
                }
            }
        }
        json!({"instruments":instruments,"baseline_available":baseline.is_some(),"changes":changes,
            "comparison_scope":["track_names","instrument_sources","static_instrument_parameters","authored_notes","recipes","performance","clip_placement","timeline"],
            "uncompared_state":["mixer_and_effects","automation_lanes","audio_takes","opaque_plugin_state"],
            "baseline_note":baseline.is_none().then_some("No composition baseline was saved. Current state is available, but changes since generation cannot be reconstructed reliably."),
            "final_state":"The .auris project holds the edited state. original_specification records the generation request; regenerating it can replace later edits."})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::Scratch;
    use crate::{InstrumentParamEdit, SessionOptions};

    #[test]
    fn inspection_distinguishes_source_timeline_and_track_changes() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let spec=auris_compose::SongSpec::parse("ending='none'\nform='verse'\n[section.verse]\nbars=2\n[[part]]\nname='lead'\n[[part]]\nname='bass'").unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();
        let lead = session
            .project
            .tracks
            .iter()
            .find(|track| track.name == "lead")
            .unwrap()
            .id;
        let bass = session
            .project
            .tracks
            .iter()
            .find(|track| track.name == "bass")
            .unwrap()
            .id;
        let old = session
            .project
            .track(lead)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_id
            .clone();
        let replacement = session
            .registry()
            .instruments()
            .find(|descriptor| {
                descriptor.id.as_ref() != old && !auris_sampler::needs_asset(&descriptor.id)
            })
            .unwrap()
            .id
            .to_string();
        session.set_track_instrument(lead, &replacement).unwrap();
        session.rename_track(lead, "Changed lead").unwrap();
        session.remove_track(bass).unwrap();
        let added = session.add_default_instrument_track("New part").unwrap();
        session.set_tempo_at(Ticks::ZERO, 96.0);
        let report = session.inspect_composition_state();
        let changes = report["changes"].as_array().unwrap();
        assert!(
            changes.iter().any(
                |change| change["target"] == "timeline" && change["fields"] == json!(["tempo"])
            )
        );
        let changed = changes
            .iter()
            .find(|change| change["track"] == format!("id:{}", lead.0))
            .unwrap();
        assert_eq!(changed["fields"], json!(["name", "instrument_source"]));
        assert_eq!(changed["parameters"], json!([]));
        assert!(
            changes
                .iter()
                .any(|change| change["track"] == format!("id:{}", bass.0)
                    && change["status"] == "removed")
        );
        assert!(
            changes
                .iter()
                .any(|change| change["track"] == format!("id:{}", added.0)
                    && change["status"] == "added")
        );
    }

    #[test]
    fn inspection_keeps_provenance_and_reports_saved_control_and_note_edits() {
        let scratch = Scratch::new("composition-state");
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let spec = auris_compose::SongSpec::parse(
            "ending='none'\nform='verse'\n[section.verse]\nbars=2\n[[part]]\nname='lead'",
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();
        let original = session.project.song_spec.clone();
        assert_eq!(session.inspect_composition_state()["changes"], json!([]));
        let track = session.project.tracks[0].id;
        let clip = session.project.tracks[0].kind.note_clips().unwrap()[0].id;
        let descriptors = session.instrument_descriptors(track);
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.min < descriptor.max)
            .unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: descriptor.id,
        };
        let previous = session.param_value(target, descriptor);
        let value = if previous != descriptor.min {
            descriptor.min
        } else {
            descriptor.max
        };
        session
            .set_instrument_params(&[InstrumentParamEdit {
                track,
                param: descriptor.key.to_string(),
                value,
            }])
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(61, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        let path = session
            .save_as(&scratch.join("Saved.auris"))
            .unwrap()
            .document;
        session.open(&path).unwrap();
        let before = session.project().clone();
        let report = session.inspect_composition_state();
        assert_eq!(report["baseline_available"], true);
        let changed = report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|change| change["track"] == format!("id:{}", track.0))
            .unwrap();
        assert_eq!(changed["parameters"][0]["previous"], json!(previous));
        assert_eq!(changed["parameters"][0]["value"], json!(value));
        assert_eq!(changed["clips"][0]["fields"], json!(["authored_notes"]));
        assert_eq!(session.project(), &before);
        assert_eq!(session.project.song_spec, original);
        assert!(!session.is_dirty());
        let mut legacy = serde_json::to_value(session.project()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("composition_baseline");
        let legacy: Project = serde_json::from_value(legacy).unwrap();
        assert!(legacy.composition_baseline.is_none());
        session.replace_project(legacy);
        assert_eq!(
            session.inspect_composition_state()["baseline_available"],
            false
        );
    }
}
