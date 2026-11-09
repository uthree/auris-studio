//! Earlier versions of a song that an edit may restore from.
//!
//! A workshop candidate continues its parent, but "put the chorus back the way it was two
//! versions ago" needs an older candidate as well, and a composer that can only see its parent
//! has nothing to restore from but a guess. The host loads those versions here, read-only and
//! outside the document: they are not saved, not undone and never played. The agent lists their
//! note clips, sees which differ from the current score, and restores the named clips exactly.
//! Clip ids survive from a parent to its children, so one id names one clip in every version.

use std::path::Path;

use auris_core::{ClipId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Session;
use crate::error::SessionError;

/// The most earlier versions one session holds.
pub const MAX_SOURCES: usize = 4;
const MAX_LISTED_CLIPS: usize = 256;

/// One earlier version, as it was saved.
pub(crate) struct SourceProject {
    id: u64,
    title: String,
    project: Project,
}

/// One clip to restore.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreClip {
    /// Clip ID in the source version, from list.
    pub clip: u64,
    /// Current clip to receive the notes; defaults to the same ID. Name another, for example
    /// one just made with add_clip, when the current score no longer has that clip.
    pub into: Option<u64>,
}

/// What to do with the earlier versions.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Each source's note clips, with whether the current score has each and matches it.
    List,
    /// Copy the named clips' notes — pitch, timing, velocity, lyrics and phonemes — from a
    /// source into the current score as one Undo edit.
    Restore {
        /// Source ID from list.
        source: u64,
        /// 1..64 clips.
        #[schemars(length(min = 1, max = 64))]
        clips: Vec<RestoreClip>,
        /// Revision from the latest read_notes or inspect_project.
        expected_revision: String,
    },
}

impl Operation {
    /// Whether it changes the document.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::Restore { .. })
    }
}

impl Session {
    /// Holds an earlier saved version of this song for the agent to restore from.
    pub fn add_source_project(
        &mut self,
        id: u64,
        title: &str,
        path: &Path,
    ) -> Result<(), SessionError> {
        if self.sources.len() >= MAX_SOURCES {
            return Err(SessionError::InvalidNotes(format!(
                "At most {MAX_SOURCES} earlier versions can be supplied"
            )));
        }
        let project = auris_io::load_project(path)?;
        self.sources.retain(|source| source.id != id);
        self.sources.push(SourceProject {
            id,
            title: title.to_owned(),
            project,
        });
        Ok(())
    }

    /// Runs one source-material operation for the live agent.
    pub fn agent_source_material(&mut self, operation: Operation) -> Result<String, String> {
        match operation {
            Operation::List => Ok(self.source_listing().to_string()),
            Operation::Restore {
                source,
                clips,
                expected_revision,
            } => self.restore_source_clips(source, &clips, &expected_revision),
        }
    }

    fn source_listing(&self) -> Value {
        let current = self.project();
        let sources: Vec<Value> = self
            .sources
            .iter()
            .map(|source| {
                let mut clips = Vec::new();
                let mut more = false;
                for track in &source.project.tracks {
                    let Some(notes) = track.kind.note_clips() else {
                        continue;
                    };
                    for clip in notes {
                        if clips.len() == MAX_LISTED_CLIPS {
                            more = true;
                            break;
                        }
                        let now = current.midi_clip(clip.id).map(|(_, now)| now);
                        clips.push(json!({
                            "clip": clip.id.0, "track": track.id.0, "track_name": track.name,
                            "name": clip.name, "start_tick": clip.start.raw(),
                            "length_ticks": clip.length.raw(), "notes": clip.notes.len(),
                            "in_current": now.is_some(),
                            "matches_current": now.is_some_and(|now| now.notes == clip.notes),
                        }));
                    }
                }
                json!({"source": source.id, "title": source.title, "clips": clips, "more_clips": more})
            })
            .collect();
        json!({
            "sources": sources,
            "note": if self.sources.is_empty() {
                "No earlier versions were supplied for this session."
            } else {
                "Restore copies notes exactly; clip position and length stay as they are now. Re-sing restored singer clips."
            },
        })
    }

    fn restore_source_clips(
        &mut self,
        source: u64,
        clips: &[RestoreClip],
        expected_revision: &str,
    ) -> Result<String, String> {
        if clips.is_empty() || clips.len() > 64 {
            return Err("clips must name 1..64 clips".into());
        }
        if self.agent_document_revision()? != expected_revision {
            return Err("Document changed; call read_notes or inspect_project again first".into());
        }
        let from = self
            .sources
            .iter()
            .find(|candidate| candidate.id == source)
            .ok_or_else(|| {
                format!("No source {source}; source_material list names the supplied versions")
            })?;
        let mut replacements = Vec::with_capacity(clips.len());
        let mut report = Vec::with_capacity(clips.len());
        for request in clips {
            let into = ClipId(request.into.unwrap_or(request.clip));
            let (source_track, original) = from
                .project
                .midi_clip(ClipId(request.clip))
                .ok_or_else(|| format!("Source {source} has no note clip {}", request.clip))?;
            let (track, target) = self.project().midi_clip(into).ok_or_else(|| {
                format!(
                    "The current score has no note clip {}; make one with add_clip and pass it as into",
                    into.0
                )
            })?;
            let singer = |project: &Project, id: auris_core::TrackId| {
                project
                    .tracks
                    .iter()
                    .any(|t| t.id == id && t.kind.as_singer().is_some())
            };
            let singer_target = singer(self.project(), track);
            if singer(&from.project, source_track) != singer_target {
                return Err(format!(
                    "Clip {} and clip {} are on different kinds of track; restore singer notes onto a singer track",
                    request.clip, into.0
                ));
            }
            if replacements.iter().any(|(id, _)| *id == into) {
                return Err(format!("Clip {} is named twice", into.0));
            }
            report.push(json!({
                "clip": request.clip, "into": into.0, "notes": original.notes.len(),
                "same_position": target.start == original.start && target.length == original.length,
                "singer": singer_target,
            }));
            replacements.push((into, original.notes.clone()));
        }
        let changed = self.replace_notes_batch(replacements)?;
        Ok(json!({
            "restored": report, "changed_clips": changed,
            "revision": self.agent_document_revision()?,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use crate::{Session, SessionOptions};

    #[test]
    fn a_restored_clip_takes_the_source_notes_exactly_and_undoes_in_one_step() {
        let folder = tempfile::tempdir().unwrap();
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Lead");
        let clip = session
            .add_midi_clip(track, "Chorus", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(72, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let earlier = session
            .save_as(&folder.path().join("Earlier.auris"))
            .unwrap()
            .document;
        // The later version moved the chorus down.
        let moved = session.midi_clip(clip).unwrap().notes[0].clone();
        session
            .replace_notes_batch(vec![(clip, vec![Note { pitch: 60, ..moved }])])
            .unwrap();

        session
            .add_source_project(3, "High chorus", &earlier)
            .unwrap();
        let listed: serde_json::Value = serde_json::from_str(
            &session
                .agent_source_material(super::Operation::List)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(listed["sources"][0]["clips"][0]["matches_current"], false);

        let revision = session.agent_document_revision().unwrap();
        session
            .agent_source_material(super::Operation::Restore {
                source: 3,
                clips: vec![super::RestoreClip {
                    clip: clip.0,
                    into: None,
                }],
                expected_revision: revision,
            })
            .unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].pitch, 72);
        session.undo();
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].pitch, 60);

        // A stale revision and an unknown source are refused.
        assert!(
            session
                .agent_source_material(super::Operation::Restore {
                    source: 9,
                    clips: vec![super::RestoreClip {
                        clip: clip.0,
                        into: None
                    }],
                    expected_revision: session.agent_document_revision().unwrap(),
                })
                .is_err()
        );
        assert!(
            session
                .agent_source_material(super::Operation::Restore {
                    source: 3,
                    clips: vec![super::RestoreClip {
                        clip: clip.0,
                        into: None
                    }],
                    expected_revision: "stale".into(),
                })
                .is_err()
        );
    }
}
