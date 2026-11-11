//! Comments: notes pinned to a track, inside a clip or on a mixer node, which nothing ever plays.
//!
//! The commands here are the whole of the editing. A comment never changes a note, a sound or a
//! render, so the only thing an edit here touches is the document's text — and the undo history,
//! because a comment somebody typed and lost to a stray click is a comment they will not type
//! twice. See [`Comment`] for where each kind lives and why.

use auris_core::time::Ticks;
use auris_core::{ClipId, Comment, CommentId, Comments, TrackId};

use crate::error::SessionError;
use crate::history::Edit;
use crate::mixer_graph::MixerNode;

use super::Session;

/// What a comment is pinned to.
///
/// A track holds comments in song time along its lane; a note clip holds them in its own time,
/// optionally on a pitch row, so that they travel with the notes. A node of the mixer graph
/// holds them with no position at all: they are about how the part sits in the mix, not about
/// a moment of it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CommentHost {
    /// A track's lane in the arrangement.
    Track(TrackId),
    /// Inside a note clip, as the piano roll shows it.
    Clip(ClipId),
    /// A strip or an insert in the mixer graph.
    Mixer(MixerNode),
}

impl CommentHost {
    /// Whether a comment here has a moment: everywhere but the mixer graph.
    pub fn is_timed(self) -> bool {
        !matches!(self, Self::Mixer(_))
    }
}

impl Session {
    fn comment_holder(&self, host: CommentHost) -> Result<&dyn Comments, SessionError> {
        match host {
            CommentHost::Track(id) => self
                .project
                .track(id)
                .map(|track| track as &dyn Comments)
                .ok_or(SessionError::UnknownTrack(id.0)),
            CommentHost::Clip(id) => match self.project.midi_clip(id) {
                Some((_, clip)) => Ok(clip as &dyn Comments),
                None => Err(self.not_a_note_clip(id)),
            },
            CommentHost::Mixer(node) => {
                let missing = || SessionError::UnknownMixerNode(node.key());
                match node {
                    MixerNode::Strip(owner) => self
                        .strip(owner)
                        .map(|strip| strip as &dyn Comments)
                        .ok_or_else(missing),
                    MixerNode::Effect(owner, slot) => self
                        .strip(owner)
                        .and_then(|strip| strip.effects.iter().find(|effect| effect.id == slot))
                        .map(|effect| effect as &dyn Comments)
                        .ok_or_else(missing),
                    // A performance holds no comments of its own.
                    MixerNode::Performance(_) => Err(missing()),
                }
            }
        }
    }

    fn comment_holder_mut(&mut self, host: CommentHost) -> Result<&mut dyn Comments, SessionError> {
        if let CommentHost::Clip(id) = host
            && self.project.midi_clip(id).is_none()
        {
            return Err(self.not_a_note_clip(id));
        }
        match host {
            CommentHost::Track(id) => self
                .project
                .track_mut(id)
                .map(|track| track as &mut dyn Comments)
                .ok_or(SessionError::UnknownTrack(id.0)),
            CommentHost::Clip(id) => self
                .project
                .midi_clip_mut(id)
                .map(|clip| clip as &mut dyn Comments)
                .ok_or(SessionError::UnknownClip(id.0)),
            CommentHost::Mixer(node) => {
                let missing = SessionError::UnknownMixerNode(node.key());
                match node {
                    MixerNode::Strip(owner) => self
                        .strip_mut(owner)
                        .map(|strip| strip as &mut dyn Comments)
                        .ok_or(missing),
                    MixerNode::Effect(owner, slot) => self
                        .strip_mut(owner)
                        .and_then(|strip| strip.effects.iter_mut().find(|effect| effect.id == slot))
                        .map(|effect| effect as &mut dyn Comments)
                        .ok_or(missing),
                    MixerNode::Performance(_) => Err(missing),
                }
            }
        }
    }

    fn not_a_note_clip(&self, id: ClipId) -> SessionError {
        if self.audio_clip_exists(id) {
            SessionError::NotANoteClip(id.0)
        } else {
            SessionError::UnknownClip(id.0)
        }
    }

    /// The comments pinned to `host`, as stored; empty when it has none or does not exist.
    pub fn comments(&self, host: CommentHost) -> &[Comment] {
        self.comment_holder(host)
            .map(|holder| holder.comment_list())
            .unwrap_or_default()
    }

    /// One comment.
    pub fn comment(&self, host: CommentHost, id: CommentId) -> Option<&Comment> {
        self.comment_holder(host).ok()?.comment(id)
    }

    /// Pins a comment to `host` at `at` — song time on a track, clip time inside a clip, and
    /// ignored on a mixer node — and optionally on a piano-roll row. Text is trimmed and may run
    /// over several lines; empty text is refused.
    pub fn add_comment(
        &mut self,
        host: CommentHost,
        at: Ticks,
        pitch: Option<u8>,
        text: &str,
    ) -> Result<CommentId, SessionError> {
        self.comment_holder(host)?;
        if Comment::clean_text(text).is_none() {
            return Err(SessionError::EmptyComment);
        }
        // A pitch means nothing on a track lane, which has no rows, and neither a pitch nor a
        // moment means anything on a mixer node.
        let pitch = pitch.filter(|_| matches!(host, CommentHost::Clip(_)));
        let at = if host.is_timed() { at } else { Ticks::ZERO };
        self.record(Edit::SetComment);
        self.comment_holder_mut(host)?
            .add_comment(at, pitch, text)
            .ok_or(SessionError::EmptyComment)
    }

    /// Retypes a comment. Empty text takes it off, the way clearing a name field would.
    pub fn set_comment_text(
        &mut self,
        host: CommentHost,
        id: CommentId,
        text: &str,
    ) -> Result<(), SessionError> {
        let current = self
            .comment(host, id)
            .ok_or(SessionError::UnknownComment(id.0))?;
        if Comment::clean_text(text).as_deref() == Some(current.text.as_str()) {
            return Ok(());
        }
        self.record(Edit::SetComment);
        self.comment_holder_mut(host)?.set_comment_text(id, text);
        Ok(())
    }

    /// Moves a comment to `at` and `pitch` within the same holder.
    pub fn move_comment(
        &mut self,
        host: CommentHost,
        id: CommentId,
        at: Ticks,
        pitch: Option<u8>,
    ) -> Result<(), SessionError> {
        let current = self
            .comment(host, id)
            .ok_or(SessionError::UnknownComment(id.0))?;
        let pitch = pitch.filter(|_| matches!(host, CommentHost::Clip(_)));
        let at = if host.is_timed() { at } else { Ticks::ZERO };
        if current.at == at.max_zero() && current.pitch == pitch {
            return Ok(());
        }
        self.record(Edit::MoveComment);
        self.comment_holder_mut(host)?.move_comment(id, at, pitch);
        Ok(())
    }

    /// Takes a comment off.
    pub fn remove_comment(&mut self, host: CommentHost, id: CommentId) -> Result<(), SessionError> {
        self.comment(host, id)
            .ok_or(SessionError::UnknownComment(id.0))?;
        self.record(Edit::SetComment);
        self.comment_holder_mut(host)?.remove_comment(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::Note;

    #[test]
    fn comments_are_text_only_and_each_edit_is_one_undo() {
        let mut session = session();
        let track = session.add_singer_track("Vocal");
        let clip = session
            .add_midi_clip(track, "Verse", Ticks(3840), Ticks(3840))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(960)))
            .unwrap();
        let notes = session.project().midi_clip(clip).unwrap().1.notes.clone();

        let on_track = session
            .add_comment(
                CommentHost::Track(track),
                Ticks(1920),
                Some(60),
                "intro is sparse",
            )
            .unwrap();
        assert_eq!(
            session
                .comment(CommentHost::Track(track), on_track)
                .unwrap()
                .pitch,
            None,
            "a lane has no rows"
        );
        let in_clip = session
            .add_comment(
                CommentHost::Clip(clip),
                Ticks(480),
                Some(64),
                "make this busier",
            )
            .unwrap();
        assert!(matches!(
            session.add_comment(CommentHost::Clip(clip), Ticks::ZERO, None, "  "),
            Err(SessionError::EmptyComment)
        ));
        session
            .move_comment(CommentHost::Clip(clip), in_clip, Ticks(960), Some(67))
            .unwrap();
        session
            .set_comment_text(CommentHost::Clip(clip), in_clip, "fill here")
            .unwrap();
        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes,
            notes,
            "no comment edit touches a note"
        );
        let stored = session.comment(CommentHost::Clip(clip), in_clip).unwrap();
        assert_eq!((stored.at, stored.pitch), (Ticks(960), Some(67)));

        session.undo();
        assert_eq!(
            session
                .comment(CommentHost::Clip(clip), in_clip)
                .unwrap()
                .text,
            "make this busier"
        );
        session.undo();
        assert_eq!(
            session
                .comment(CommentHost::Clip(clip), in_clip)
                .unwrap()
                .at,
            Ticks(480)
        );
        session
            .remove_comment(CommentHost::Track(track), on_track)
            .unwrap();
        assert!(session.comments(CommentHost::Track(track)).is_empty());
        assert!(matches!(
            session.remove_comment(CommentHost::Track(track), on_track),
            Err(SessionError::UnknownComment(_))
        ));
    }

    #[test]
    fn mixer_comments_sit_on_their_node_and_leave_with_it() {
        let mut session = session();
        let track = session.add_singer_track("Vocal");
        let slot = session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        let strip = CommentHost::Mixer(MixerNode::Strip(Some(track)));
        let effect = CommentHost::Mixer(MixerNode::Effect(Some(track), slot));
        let master = CommentHost::Mixer(MixerNode::Strip(None));

        let on_strip = session
            .add_comment(
                strip,
                Ticks(1920),
                Some(60),
                "keep it under the vocal\nexcept the chorus",
            )
            .unwrap();
        let stored = session.comment(strip, on_strip).unwrap();
        assert_eq!(
            (stored.at, stored.pitch),
            (Ticks::ZERO, None),
            "a node has no moment"
        );
        assert_eq!(stored.text, "keep it under the vocal\nexcept the chorus");
        session
            .add_comment(effect, Ticks::ZERO, None, "short tail")
            .unwrap();
        session
            .add_comment(master, Ticks::ZERO, None, "-14 LUFS")
            .unwrap();
        assert_eq!(session.project().master.comments.len(), 1);
        assert!(
            session.project().track(track).unwrap().comments.is_empty(),
            "not on the lane"
        );

        session.remove_effect(slot);
        assert!(matches!(
            session.add_comment(effect, Ticks::ZERO, None, "again"),
            Err(SessionError::UnknownMixerNode(_))
        ));
        assert!(session.comments(effect).is_empty(), "gone with its effect");
        session.undo();
        assert_eq!(
            session.comments(effect)[0].text,
            "short tail",
            "and back with it"
        );
    }

    #[test]
    fn clip_comments_travel_with_the_clip_through_copies_and_splits() {
        let mut session = session();
        let track = session.add_singer_track("Vocal");
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(3840))
            .unwrap();
        session
            .add_comment(CommentHost::Clip(clip), Ticks(480), None, "first half")
            .unwrap();
        session
            .add_comment(CommentHost::Clip(clip), Ticks(2400), None, "second half")
            .unwrap();
        let copy = session.duplicate_clip(clip).unwrap();
        assert_eq!(session.comments(CommentHost::Clip(copy)).len(), 2);

        let right = session.split_clip(clip, Ticks(1920)).unwrap();
        let left = session.comments(CommentHost::Clip(clip));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].text, "first half");
        let right = session.comments(CommentHost::Clip(right));
        assert_eq!(right.len(), 1);
        assert_eq!(
            (right[0].text.as_str(), right[0].at),
            ("second half", Ticks(480))
        );
    }
}
