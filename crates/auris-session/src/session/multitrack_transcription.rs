//! Validate and place external parts as one atomic, reversible edit.

use super::*;
use crate::prelude::{Note, Seconds};
use crate::transcription_draft::MultitrackTranscription;
use serde_json::json;

impl Session {
    /// Preview or accept all external parts, explicitly mapping source seconds to at_tick.
    /// Unknown instruments use reported guide sounds; original labels remain in the document.
    pub fn import_multitrack_transcription(
        &mut self,
        draft: &MultitrackTranscription,
        name: &str,
        at_tick: i64,
        preview: bool,
    ) -> Result<String, String> {
        // Its pitches are MIDI semitones, which are keys only in a twelve-note tuning.
        super::tuning::require_twelve_tone(&self.project).map_err(|error| error.to_string())?;
        draft.validate()?;
        if self.transaction.is_some() || at_tick < 0 || name.trim().is_empty() || name.len() > 256 {
            return Err(
                "Finish the current edit; use nonnegative at_tick and a bounded name".into(),
            );
        }
        let start = Ticks(at_tick);
        let replacement = draft
            .performance
            .as_ref()
            .filter(|p| {
                p.timeline_policy
                    == crate::transcription_draft::performance::TimelinePolicy::ReplaceDestination
            })
            .map(|p| p.maps(draft.source.start_seconds))
            .transpose()?;
        if replacement.is_some() && (at_tick != 0 || draft.source.start_seconds != 0.0) {
            return Err(
                "Replacing global maps requires at_tick=0 and source.start_seconds=0".into(),
            );
        }
        let tempo = replacement
            .as_ref()
            .map(|maps| &maps.0)
            .unwrap_or(&self.project.tempo_map);
        let origin = tempo.ticks_to_seconds(start).0;
        if !origin.is_finite()
            || origin + draft.source.end_seconds - draft.source.start_seconds > 86_400.0
        {
            return Err("Draft placement exceeds the 24-hour timeline".into());
        }
        let mut groups = Vec::new();
        let mut performances = Vec::new();
        for (index, part) in draft.tracks.iter().enumerate() {
            let mut notes = Vec::new();
            for event in &part.notes {
                let from = tempo
                    .seconds_to_ticks(Seconds(origin + event.start - draft.source.start_seconds));
                let to = tempo
                    .seconds_to_ticks(Seconds(origin + event.end - draft.source.start_seconds));
                if to <= from || from < start {
                    return Err(
                        "A note collapses at project tick resolution; no notes were imported"
                            .into(),
                    );
                }
                let mut note = Note::new(event.pitch, from - start, to - from);
                note.velocity = event.strength;
                notes.push(note);
            }
            let performance = draft
                .performance
                .as_ref()
                .map(|p| {
                    p.parts[index].prepare(&mut notes, |seconds| {
                        tempo.seconds_to_ticks(Seconds(
                            origin + seconds - draft.source.start_seconds,
                        )) - start
                    })
                })
                .transpose()?;
            performances.push(performance);
            groups.push(notes);
        }
        let preview_parts:Vec<_> = draft.tracks.iter().zip(&groups).map(|(part,notes)|json!({"name":part.name,
            "instrument":part.instrument,"program":part.program,"drums":part.drums,"notes":notes.len(),
            "sound_id":part.sound_id,"guide_sound":part.sound_id.is_none()})).collect();
        if preview {
            return Ok(json!({"preview":true,"revision":self.agent_document_revision()?,"parts":preview_parts,"at_tick":at_tick,"replaces_global_maps":replacement.is_some(),"provenance_authenticity":"declared_not_verified"}).to_string());
        }
        self.begin_transaction(Edit::AddInstrumentTrack);
        let result = (|| {
            if let Some((tempo, signatures)) = replacement {
                self.project.tempo_map = tempo;
                self.project.signatures = signatures;
            }
            let mut created = Vec::new();
            let mut provenance_clip = None;
            for (index, ((part, notes), performance)) in draft
                .tracks
                .iter()
                .zip(groups)
                .zip(performances)
                .enumerate()
            {
                let track_name = format!("{name}: {}", part.name);
                let track = if performance.as_ref().is_some_and(|p| p.singer) {
                    Ok(self.add_singer_track(&track_name))
                } else if part.drums {
                    self.add_default_drum_track(&track_name)
                } else {
                    self.add_default_instrument_track(&track_name)
                }
                .map_err(|e| e.to_string())?;
                if let Some(sound) = &part.sound_id {
                    self.use_library_sound(track, sound, &[])?;
                }
                let length = notes
                    .iter()
                    .map(Note::end)
                    .max()
                    .ok_or("Empty part")?
                    .max(performance.as_ref().map_or(Ticks::ZERO, |p| p.end));
                let clip = self
                    .add_midi_clip(track, &part.name, start, length)
                    .map_err(|e| e.to_string())?;
                let imported = self.project.midi_clip_mut(clip).ok_or("Missing new clip")?;
                imported.notes = notes;
                if let Some(performance) = performance {
                    imported.bend = performance.bend;
                    imported.controllers = performance.controllers;
                }
                provenance_clip.get_or_insert(clip);
                created.push(json!({"track":track.0,"clip":clip.0,"source_track":index,"sound_id":part.sound_id,"guide_sound":part.sound_id.is_none()}));
            }
            if let Some(clip) = provenance_clip {
                let provenance = json!({"draft":draft,"at_tick":at_tick,"created":created,"authenticity":"declared_not_verified"});
                self.project.transcription_provenance.insert(
                    clip,
                    serde_json::to_string(&provenance).map_err(|e| e.to_string())?,
                );
            }
            Ok(created)
        })();
        match result {
            Ok(created) => {
                self.invalidate_graph();
                self.meter_is_stale = true;
                self.publish_loop();
                self.end_transaction();
                Ok(json!({"created":created,"parts":preview_parts,"notes":draft.tracks.iter().map(|t|t.notes.len()).sum::<usize>(),
                    "revision":self.agent_document_revision()?,"provenance_authenticity":"declared_not_verified","unsaved":self.is_dirty()}).to_string())
            }
            Err(error) => {
                self.revert_transaction();
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_two_maps_control_tails_and_japanese_lyrics_survive_save_and_history() {
        let draft: MultitrackTranscription =
            serde_json::from_str(include_str!("../../tests/fixtures/transcription-v2.json"))
                .unwrap();
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        session
            .import_multitrack_transcription(&draft, "Performance", 0, false)
            .unwrap();
        let mut imported = session.project().clone();
        assert_eq!(imported.tempo_map.points().len(), 2);
        assert_eq!(imported.signatures.points().len(), 2);
        let piano = &imported.tracks[0].kind.note_clips().unwrap()[0];
        assert_eq!(piano.bend[1].value, 1.5);
        assert_eq!(piano.controllers[&64].last().unwrap().at, Ticks(5760));
        assert_eq!(piano.length, Ticks(5761));
        assert!(
            piano.controllers[&64].last().unwrap().at
                > piano.notes.iter().map(Note::end).max().unwrap()
        );
        let singer = &imported.tracks[1].kind.as_singer().unwrap().clips[0];
        // Each note is labelled with the kana it sings; the written text stays in provenance.
        assert_eq!(singer.notes[0].lyric, "さくら");
        assert_eq!(singer.notes[1].lyric, "さく");
        assert_eq!(singer.notes[1].phonemes, ["s", "a", "k", "ɯ"]);
        let provenance: serde_json::Value =
            serde_json::from_str(imported.transcription_provenance.values().next().unwrap())
                .unwrap();
        assert_eq!(
            provenance["draft"]["performance"]["parts"][1]["lyrics"][0]["reading"],
            "さくら、さく"
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Performance.auris");
        session.save(&path).unwrap();
        imported.saved_by = session.project().saved_by.clone();
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        reopened.open(&path).unwrap();
        assert_eq!(reopened.project(), session.project());
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project(), &imported);

        let mut preserved = draft.clone();
        preserved.performance.as_mut().unwrap().timeline_policy =
            crate::transcription_draft::performance::TimelinePolicy::PreserveDestination;
        let mut destination = Session::new(SessionOptions::headless()).unwrap();
        destination.set_bpm(90.0);
        let tempo = destination.project().tempo_map.clone();
        let meter = destination.project().signatures.clone();
        destination
            .import_multitrack_transcription(&preserved, "Preserved", 960, false)
            .unwrap();
        assert_eq!(destination.project().tempo_map, tempo);
        assert_eq!(destination.project().signatures, meter);
    }

    #[test]
    fn version_two_rejects_unsupported_or_invalid_performance_before_editing() {
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/transcription-v2.json"))
                .unwrap();
        for (pointer, value) in [
            ("/schema_version", json!(1)),
            ("/performance/parts/0/channel", json!(16)),
            (
                "/performance/parts/0/controllers/64/points/1/value",
                json!(1.1),
            ),
            ("/performance/parts/0/controllers/64/end_value", json!(1.0)),
            ("/performance/parts/0/bend/range_semitones", json!(13.0)),
            (
                "/performance/parts/0/bend/curve/interpolation",
                json!("cubic"),
            ),
            ("/performance/tempos/1/seconds", json!(2.1)),
            ("/performance/meters/1/tick", json!(3841)),
            ("/performance/parts/1/lyrics/0/notes", json!([0, 0])),
            ("/performance/parts/1/lyrics/0/confidence", json!(-0.1)),
        ] {
            let mut invalid = original.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let before = session.project().clone();
            if let Ok(draft) = serde_json::from_value::<MultitrackTranscription>(invalid) {
                assert!(
                    session
                        .import_multitrack_transcription(&draft, "Invalid", 0, false)
                        .is_err(),
                    "{pointer}"
                );
            }
            assert_eq!(session.project(), &before);
            assert!(!session.can_undo());
        }
    }

    fn draft() -> MultitrackTranscription {
        serde_json::from_value(json!({"schema_version":1,"source":{"audio_sha256":"a".repeat(64),"start_seconds":10.0,"end_seconds":12.0},
            "recognizer":{"name":"external-research","model":"my-model"},"tracks":[
                {"name":"Piano","instrument":"piano","program":0,"drums":false,"notes":[{"pitch":108,"start":10.0,"end":11.5,"strength":0.9,"confidence":0.8},{"pitch":0,"start":10.5,"end":11.0,"strength":0.2}]},
                {"name":"Bass","instrument":null,"role":"bass","drums":false,"notes":[{"pitch":24,"start":10.25,"end":12.0,"strength":0.6}]},
                {"name":"Drums","instrument":"drum kit","drums":true,"notes":[{"pitch":127,"start":11.0,"end":11.1,"strength":0.4}]}],
            "provenance":{"origin":"declared by external model"}})).unwrap()
    }

    #[test]
    fn piano_bass_and_drums_preview_then_import_and_undo_as_one_edit() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        session.add_bus_track("Existing");
        let before = session.project().clone();
        let draft = draft();
        let preview: serde_json::Value = serde_json::from_str(
            &session
                .import_multitrack_transcription(&draft, "Draft", 3840, true)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(session.project(), &before);
        assert_eq!(preview["parts"].as_array().unwrap().len(), 3);
        session
            .import_multitrack_transcription(&draft, "Draft", 3840, false)
            .unwrap();
        let imported = session.project().clone();
        assert_eq!(imported.tracks.len(), 4);
        assert_eq!(imported.transcription_provenance.len(), 1);
        let piano = imported.tracks[1]
            .kind
            .note_clips()
            .unwrap()
            .first()
            .unwrap();
        assert_eq!(piano.notes[0].pitch, 108);
        assert_eq!(piano.notes[1].pitch, 0);
        assert_eq!(piano.notes[0].velocity, 0.9);
        assert_eq!(piano.start, Ticks(3840));
        assert_eq!(piano.notes[1].start, Ticks::QUARTER);
        assert!(imported.tracks[3].kind.is_drum());
        let restored: Project =
            serde_json::from_slice(&serde_json::to_vec(&imported).unwrap()).unwrap();
        assert_eq!(restored, imported);
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project(), &imported);
    }

    #[test]
    fn invalid_events_unknown_curves_and_collapsed_notes_are_rejected_before_edits() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let mut invalid = draft();
        invalid.tracks[2].notes[0].pitch = 255;
        assert!(
            session
                .import_multitrack_transcription(&invalid, "Draft", 0, false)
                .is_err()
        );
        invalid = draft();
        invalid.tracks[2].notes[0].end = invalid.tracks[2].notes[0].start + 1e-10;
        assert!(
            session
                .import_multitrack_transcription(&invalid, "Draft", 0, false)
                .is_err()
        );
        let mut unknown = serde_json::to_value(draft()).unwrap();
        unknown["tracks"][0]["pitch_bends"] = json!([]);
        assert!(serde_json::from_value::<MultitrackTranscription>(unknown).is_err());
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }

    #[test]
    fn a_later_invalid_playback_mapping_rolls_back_the_whole_import() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let mut invalid = draft();
        invalid.tracks[2].sound_id = Some("nonexistent-sound".into());
        assert!(
            session
                .import_multitrack_transcription(&invalid, "Draft", 0, false)
                .is_err()
        );
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }
}
