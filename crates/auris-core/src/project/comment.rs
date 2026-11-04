//! Comments: notes pinned to a place in the song that nothing ever plays.
//!
//! The habit borrowed from source code. A line of code carries a comment saying why it is the way
//! it is, and the compiler skips it; a bar of music can now carry one saying "the bass rests here
//! on purpose" or "make this fill busier", and the engine skips it the same way. A comment is for
//! whoever reads the document next — the person who wrote it a month later, or an agent asked to
//! work on the passage.
//!
//! Two places hold them, because there are two things a person points at:
//!
//! * [`Track::comments`](super::Track::comments) — a moment on a track's lane, in song time. A
//!   comment about the part as a whole ("the guitar doubles the vocal from here").
//! * [`MidiClip::comments`](super::MidiClip::comments) — a spot inside a clip, in clip time and
//!   optionally on a pitch row of the piano roll. A comment about these notes, which moves,
//!   copies and splits with them because it belongs to them.
//!
//! Neither reaches anything that sounds: no renderer, transform or digest reads them.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// Identifies a comment among the comments of the one track or clip that holds it.
///
/// Scoped to its holder rather than to the project, so that copying a clip copies its comments
/// as they are and still leaves every id unique where it is looked up.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CommentId(pub u64);

/// One note pinned to a place: where, and what it says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Unique among its holder's comments.
    pub id: CommentId,
    /// Where it is pinned: song time on a track, time from the clip's start inside a clip.
    pub at: Ticks,
    /// The piano-roll row it is pinned to, inside a clip; `None` pins it to the moment alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<u8>,
    /// What it says, as written.
    pub text: String,
}

impl Comment {
    /// The longest comment kept, in UTF-8 bytes: a paragraph, not a document.
    pub const MAX_TEXT: usize = 4096;

    /// The text as it is stored: trimmed, and cut at a character boundary within
    /// [`Self::MAX_TEXT`]. `None` when nothing is left, since an empty comment says nothing.
    pub fn clean_text(text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(Self::MAX_TEXT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(text[..end].trim_end().to_owned())
    }
}

/// The operations every holder of comments shares, over its `Vec<Comment>`.
pub trait Comments {
    /// The comments, as stored.
    fn comment_list(&self) -> &[Comment];
    /// The comments, for editing.
    fn comment_list_mut(&mut self) -> &mut Vec<Comment>;

    /// The comment with `id`.
    fn comment(&self, id: CommentId) -> Option<&Comment> {
        self.comment_list().iter().find(|comment| comment.id == id)
    }

    /// Pins a new comment and returns its id, or `None` when `text` is empty.
    fn add_comment(&mut self, at: Ticks, pitch: Option<u8>, text: &str) -> Option<CommentId> {
        let text = Comment::clean_text(text)?;
        let list = self.comment_list_mut();
        let id = CommentId(list.iter().map(|comment| comment.id.0).max().unwrap_or(0) + 1);
        list.push(Comment {
            id,
            at: at.max_zero(),
            pitch: pitch.map(|pitch| pitch.min(127)),
            text,
        });
        Some(id)
    }

    /// Rewrites a comment's text; empty text removes it. Whether the comment existed.
    fn set_comment_text(&mut self, id: CommentId, text: &str) -> bool {
        match Comment::clean_text(text) {
            Some(text) => match self.comment_list_mut().iter_mut().find(|c| c.id == id) {
                Some(comment) => {
                    comment.text = text;
                    true
                }
                None => false,
            },
            None => self.remove_comment(id),
        }
    }

    /// Moves a comment. Whether the comment existed.
    fn move_comment(&mut self, id: CommentId, at: Ticks, pitch: Option<u8>) -> bool {
        match self.comment_list_mut().iter_mut().find(|c| c.id == id) {
            Some(comment) => {
                comment.at = at.max_zero();
                comment.pitch = pitch.map(|pitch| pitch.min(127));
                true
            }
            None => false,
        }
    }

    /// Removes a comment. Whether the comment existed.
    fn remove_comment(&mut self, id: CommentId) -> bool {
        let list = self.comment_list_mut();
        let before = list.len();
        list.retain(|comment| comment.id != id);
        list.len() != before
    }
}

impl Comments for super::Track {
    fn comment_list(&self) -> &[Comment] {
        &self.comments
    }
    fn comment_list_mut(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }
}

impl Comments for super::MidiClip {
    fn comment_list(&self) -> &[Comment] {
        &self.comments
    }
    fn comment_list_mut(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ClipId, MidiClip};

    #[test]
    fn ids_stay_unique_and_empty_text_says_nothing() {
        let mut clip = MidiClip::new(ClipId(1), "A", Ticks::ZERO, Ticks(3840));
        assert_eq!(clip.add_comment(Ticks(0), None, "   "), None);
        let first = clip
            .add_comment(Ticks(-5), Some(200), "  rest here  ")
            .unwrap();
        let second = clip.add_comment(Ticks(960), None, "busier").unwrap();
        assert_ne!(first, second);
        let stored = clip.comment(first).unwrap();
        assert_eq!(
            (stored.at, stored.pitch, stored.text.as_str()),
            (Ticks::ZERO, Some(127), "rest here")
        );
        assert!(clip.remove_comment(first));
        let third = clip.add_comment(Ticks(0), None, "again").unwrap();
        assert_ne!(
            third, second,
            "a removed id below the highest is never reused for it"
        );
        assert!(clip.set_comment_text(second, ""), "empty text removes");
        assert!(clip.comment(second).is_none());
    }

    #[test]
    fn long_text_is_cut_on_a_character() {
        let text = "あ".repeat(Comment::MAX_TEXT);
        let kept = Comment::clean_text(&text).unwrap();
        assert!(kept.len() <= Comment::MAX_TEXT);
        assert!(kept.chars().all(|c| c == 'あ'));
    }

    #[test]
    fn a_clip_without_comments_saves_without_the_field() {
        let mut clip = MidiClip::new(ClipId(1), "A", Ticks::ZERO, Ticks(3840));
        let saved = serde_json::to_value(&clip).unwrap();
        assert!(saved.get("comments").is_none());
        clip.add_comment(Ticks(480), Some(60), "hold");
        let saved = serde_json::to_string(&clip).unwrap();
        let back: MidiClip = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.comments, clip.comments);
    }
}
