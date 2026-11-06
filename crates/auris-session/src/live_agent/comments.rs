//! The agent's door to comments: notes people pin on tracks, inside clips and on mixer nodes.
use super::*;
use crate::mixer_graph::MixerNode;
use crate::session::CommentHost;

/// What edit_comments does.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Pin a new comment. Supply exactly one of track (a moment on the track's lane), clip (a spot inside that note clip, optionally on a pitch row of its piano roll) or mixer_node (a strip or effect in the mixer graph, about the mix rather than a moment; no bar).
    Add {
        /// Stable track ID from inspect_project.
        track: Option<u64>,
        /// Stable note-clip ID from inspect_project; the position must fall inside the clip.
        clip: Option<u64>,
        /// Mixer node ID from mixer_graph read: track:ID, track:master or effect:ID.
        mixer_node: Option<String>,
        /// 1-based song bar (not counted from the clip). Required on a track or clip.
        #[schemars(range(min = 1))]
        bar: Option<u32>,
        /// 1-based beat of bar, fractional allowed.
        #[serde(default = "first_beat")]
        beat: f64,
        /// MIDI pitch row 0..127 inside a clip; omit to pin the moment only. Ignored on a track.
        #[schemars(range(min = 0, max = 127))]
        pitch: Option<u8>,
        /// What the comment says, 1..4096 UTF-8 bytes; line breaks are kept.
        #[schemars(length(min = 1, max = 4096))]
        text: String,
    },
    /// Retype or move an existing comment, addressed by its holder and comment ID. Omitted fields keep their value.
    Edit {
        /// The track holding it, when it is on a track.
        track: Option<u64>,
        /// The clip holding it, when it is inside a clip.
        clip: Option<u64>,
        /// The mixer node holding it, when it is in the mixer graph.
        mixer_node: Option<String>,
        /// Comment ID from inspect_project comments.
        comment: u64,
        /// New text.
        #[schemars(length(min = 1, max = 4096))]
        text: Option<String>,
        /// New 1-based song bar; supply with beat to move.
        #[schemars(range(min = 1))]
        bar: Option<u32>,
        /// New 1-based beat.
        beat: Option<f64>,
        /// New pitch row inside a clip.
        #[schemars(range(min = 0, max = 127))]
        pitch: Option<u8>,
    },
    /// Remove a comment. Only when the user asked; never to tidy up their notes.
    Remove {
        /// The track holding it, when it is on a track.
        track: Option<u64>,
        /// The clip holding it, when it is inside a clip.
        clip: Option<u64>,
        /// The mixer node holding it, when it is in the mixer graph.
        mixer_node: Option<String>,
        /// Comment ID from inspect_project comments.
        comment: u64,
    },
}

fn first_beat() -> f64 {
    1.0
}

impl Operation {
    /// Whether this removes words a person wrote.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }
}

impl Session {
    fn comment_host(
        &self,
        track: Option<u64>,
        clip: Option<u64>,
        mixer_node: Option<&str>,
    ) -> Result<CommentHost, String> {
        match (track, clip, mixer_node) {
            (Some(track), None, None) => Ok(CommentHost::Track(TrackId(track))),
            (None, Some(clip), None) => Ok(CommentHost::Clip(ClipId(clip))),
            (None, None, Some(node)) => Ok(CommentHost::Mixer(self.graph_node(node)?)),
            _ => Err("Supply exactly one of track, clip or mixer_node".into()),
        }
    }
}

fn position(project: &auris_core::Project, bar: u32, beat: f64) -> Result<Ticks, String> {
    if bar == 0 || !beat.is_finite() || beat < 1.0 {
        return Err("bar and beat are 1-based".into());
    }
    Ok(project.signatures.position(bar, beat))
}

impl Session {
    /// Song time to the holder's own time: a clip comment is stored from the clip's start.
    fn comment_local(&self, host: CommentHost, at: Ticks) -> Result<Ticks, String> {
        match host {
            CommentHost::Track(_) | CommentHost::Mixer(_) => Ok(at),
            CommentHost::Clip(id) => {
                let (_, clip) = self.project().midi_clip(id).ok_or_else(|| {
                    format!("clip {} is not a note clip from inspect_project", id.0)
                })?;
                if at < clip.start || at > clip.start + clip.length {
                    let (bar, beat) = musical(self.project(), clip.start);
                    let (end_bar, end_beat) = musical(self.project(), clip.start + clip.length);
                    return Err(format!(
                        "That position is outside clip {}, which spans bar {bar} beat {beat} to bar {end_bar} beat {end_beat}; pin it on the track instead",
                        id.0
                    ));
                }
                Ok(at - clip.start)
            }
        }
    }

    pub(super) fn agent_edit_comments(&mut self, operation: Operation) -> Result<String, String> {
        let touched = match operation {
            Operation::Add {
                track,
                clip,
                mixer_node,
                bar,
                beat,
                pitch,
                text,
            } => {
                let host = self.comment_host(track, clip, mixer_node.as_deref())?;
                let at = match bar {
                    _ if !host.is_timed() => Ticks::ZERO,
                    Some(bar) => {
                        let at = position(self.project(), bar, beat)?;
                        self.comment_local(host, at)?
                    }
                    None => return Err("Supply bar for a comment on a track or clip".into()),
                };
                self.add_comment(host, at, pitch, &text)
                    .map_err(|e| e.to_string())?
            }
            Operation::Edit {
                track,
                clip,
                mixer_node,
                comment,
                text,
                bar,
                beat,
                pitch,
            } => {
                let host = self.comment_host(track, clip, mixer_node.as_deref())?;
                let id = CommentId(comment);
                let current = self
                    .comment(host, id)
                    .cloned()
                    .ok_or_else(|| format!("No comment {comment} there; read inspect_project"))?;
                let at = match bar {
                    Some(bar) => {
                        let at = position(self.project(), bar, beat.unwrap_or(1.0))?;
                        self.comment_local(host, at)?
                    }
                    None if beat.is_some() => return Err("Supply bar with beat".into()),
                    None => current.at,
                };
                if text.as_deref().is_some_and(|text| text.trim().is_empty()) {
                    return Err("text must not be empty; use kind remove".into());
                }
                self.begin_transaction(crate::history::Edit::SetComment);
                let moved = self
                    .move_comment(host, id, at, pitch.or(current.pitch))
                    .and_then(|()| match &text {
                        Some(text) => self.set_comment_text(host, id, text),
                        None => Ok(()),
                    });
                self.end_transaction();
                moved.map_err(|e| e.to_string())?;
                id
            }
            Operation::Remove {
                track,
                clip,
                mixer_node,
                comment,
            } => {
                let host = self.comment_host(track, clip, mixer_node.as_deref())?;
                self.remove_comment(host, CommentId(comment))
                    .map_err(|e| e.to_string())?;
                CommentId(comment)
            }
        };
        Ok(serde_json::json!({
            "comment": touched.0,
            "comments": comments_json(self.project()),
            "unsaved": true,
            "usage": "inspect_project returns the same comments. Comments never change notes or sound."
        })
        .to_string())
    }
}

/// A position as a reader writes it: 1-based bar and fractional 1-based beat.
fn musical(project: &auris_core::Project, tick: Ticks) -> (u32, f64) {
    let (bar, beat, remainder) = project.signatures.bar_beat_at(tick);
    let per_beat = project
        .signatures
        .signature_at(project.signatures.bar_start(bar))
        .ticks_per_beat()
        .raw()
        .max(1) as f64;
    let beat = f64::from(beat) + remainder as f64 / per_beat;
    (bar, (beat * 1000.0).round() / 1000.0)
}

/// Every comment in the document as an agent reads it: the timed ones in song order, then the
/// ones pinned in the mixer graph, strip by strip.
pub(crate) fn comments_json(project: &auris_core::Project) -> serde_json::Value {
    let mut placed = Vec::new();
    for track in &project.tracks {
        for comment in &track.comments {
            placed.push((comment.at, track, None, comment));
        }
        for clip in track.kind.note_clips().into_iter().flatten() {
            for comment in &clip.comments {
                placed.push((clip.start + comment.at, track, Some(clip), comment));
            }
        }
    }
    placed.sort_by_key(|(at, ..)| *at);
    let timed = placed
        .into_iter()
        .map(|(at, track, clip, comment)| {
            let (bar, beat) = musical(project, at);
            serde_json::json!({
                "comment": comment.id.0, "track": track.id.0, "track_name": track.name,
                "clip": clip.map(|clip| clip.id.0), "clip_name": clip.map(|clip| &clip.name),
                "bar": bar, "beat": beat, "pitch": comment.pitch,
                "pitch_name": comment.pitch.map(|pitch| auris_core::theory::pitch::midi_name(i32::from(pitch))),
                "text": comment.text,
            })
        });
    let strips = project
        .tracks
        .iter()
        .map(|track| (Some(track.id), track.name.as_str(), &track.mixer))
        .chain(std::iter::once((None, "master", &project.master)));
    let mut mixer = Vec::new();
    for (owner, name, strip) in strips {
        let row = |node: MixerNode, effect: Option<&str>, comment: &auris_core::Comment| {
            serde_json::json!({
                "comment": comment.id.0, "mixer_node": node.key(),
                "track": owner.map(|id| id.0), "track_name": name,
                "effect": effect, "text": comment.text,
            })
        };
        for comment in &strip.comments {
            mixer.push(row(MixerNode::Strip(owner), None, comment));
        }
        for slot in &strip.effects {
            for comment in &slot.comments {
                mixer.push(row(
                    MixerNode::Effect(owner, slot.id),
                    Some(&slot.effect_id),
                    comment,
                ));
            }
        }
    }
    timed.chain(mixer).collect::<Vec<_>>().into()
}

#[cfg(test)]
mod tests {
    use crate::live_agent::Command;

    fn session() -> crate::Session {
        crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap()
    }

    fn run(session: &mut crate::Session, value: serde_json::Value) -> Result<String, String> {
        session.agent_command(serde_json::from_value::<Command>(value).unwrap())
    }

    #[test]
    fn comments_are_pinned_by_bar_and_read_back_in_song_order() {
        let mut session = session();
        let track = session.add_singer_track("Vocal");
        let clip = session
            .add_midi_clip(
                track,
                "Verse",
                auris_core::time::Ticks(3840),
                auris_core::time::Ticks(3840),
            )
            .unwrap();
        run(
            &mut session,
            serde_json::json!({"action":"edit_comments","operation":{"kind":"add","clip":clip.0,"bar":2,"beat":3,"pitch":64,"text":"hold this note longer"}}),
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(
            &run(
                &mut session,
                serde_json::json!({"action":"edit_comments","operation":{"kind":"add","track":track.0,"bar":1,"text":"intro stays sparse"}}),
            )
            .unwrap(),
        )
        .unwrap();
        let comments = report["comments"].as_array().unwrap();
        assert_eq!(comments[0]["text"], "intro stays sparse");
        assert_eq!(comments[1]["clip"], clip.0);
        assert_eq!(
            (comments[1]["bar"].as_u64(), comments[1]["beat"].as_f64()),
            (Some(2), Some(3.0))
        );
        assert_eq!(comments[1]["pitch_name"], "E4");
        let stored = &session.project().midi_clip(clip).unwrap().1.comments[0];
        assert_eq!(
            stored.at,
            auris_core::time::Ticks(1920),
            "stored from the clip's start"
        );

        let outside = run(
            &mut session,
            serde_json::json!({"action":"edit_comments","operation":{"kind":"add","clip":clip.0,"bar":5,"text":"x"}}),
        );
        assert!(outside.unwrap_err().contains("outside clip"));

        let id = comments[1]["comment"].as_u64().unwrap();
        run(
            &mut session,
            serde_json::json!({"action":"edit_comments","operation":{"kind":"edit","clip":clip.0,"comment":id,"text":"done: held","bar":2,"beat":1}}),
        )
        .unwrap();
        let stored = &session.project().midi_clip(clip).unwrap().1.comments[0];
        assert_eq!(
            (stored.at.raw(), stored.pitch, stored.text.as_str()),
            (0, Some(64), "done: held")
        );
        assert!(session.undo().is_some());
        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.comments[0].text,
            "hold this note longer",
            "an edit is one undo"
        );

        let inspected: serde_json::Value = serde_json::from_str(
            &run(&mut session, serde_json::json!({"action":"inspect"})).unwrap(),
        )
        .unwrap();
        assert_eq!(inspected["comments"].as_array().unwrap().len(), 2);

        let report: serde_json::Value = serde_json::from_str(
            &run(
                &mut session,
                serde_json::json!({"action":"edit_comments","operation":{"kind":"add","mixer_node":"track:master","text":"leave headroom\nfor mastering"}}),
            )
            .unwrap(),
        )
        .unwrap();
        let last = report["comments"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            (last["mixer_node"].as_str(), last["text"].as_str()),
            (Some("track:master"), Some("leave headroom\nfor mastering"))
        );
        assert_eq!(session.project().master.comments.len(), 1);
        let unknown = run(
            &mut session,
            serde_json::json!({"action":"edit_comments","operation":{"kind":"add","mixer_node":"effect:999","text":"x"}}),
        );
        assert!(unknown.unwrap_err().contains("Unknown mixer node"));
        let no_bar = run(
            &mut session,
            serde_json::json!({"action":"edit_comments","operation":{"kind":"add","track":track.0,"text":"x"}}),
        );
        assert!(no_bar.unwrap_err().contains("bar"));
    }
}
