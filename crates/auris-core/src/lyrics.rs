//! The song's words over the timeline: which line is being sung, and from when until when.
//!
//! The words a singer track sings live on its notes, one mora or one word to each. That is
//! where a voice synthesiser needs them, and it is the wrong place for every other reader. A
//! caption under a video, a lyric sheet, the agent planning verse 2 — they all ask "what is
//! being sung here?" of the *song*, not of whichever clip happens to hold the notes, and a
//! song may have no singer track at all yet, or two, or one whose notes spell the reading
//! rather than the words as written.
//!
//! So the lyric is also a timeline map, beside the harmony and the sections and built to the
//! same shape as [`SectionMap`](crate::structure::SectionMap): a sorted list of points, each
//! line in force until the next, with `None` for the silence between lines. Like the sections
//! and unlike the key, it may be empty — a song nobody has written words for has none, and it
//! would be a lie to invent them.
//!
//! Nothing here is heard. Rewriting the annotation never touches a note, and writing lyrics
//! onto notes never rewrites the annotation behind a person's back — the commands that keep
//! the two in step are named, and live in the session.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// A change of lyric line at a musical position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    /// Where the line begins.
    pub tick: Ticks,
    /// The words from this point onwards, as written — kanji and all — or `None` for the
    /// silence until the next line.
    #[serde(default)]
    pub text: Option<String>,
}

/// One line of the song, as [`LyricMap::spans_in`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricSpan {
    /// Where the visible part of the line begins, clipped to the window it was asked for.
    pub start: Ticks,
    /// Where it ends: the next change, or the window's edge when the line runs past it.
    pub end: Ticks,
    /// Where the change that started this line actually sits, which may be left of the
    /// window. The position an editor acts through.
    pub change: Ticks,
    /// The words.
    pub text: String,
}

/// The song's lyric lines over the timeline, each in force until the next.
///
/// Invariants, upheld by every mutator and by deserialisation: sorted, no two points at the
/// same position, none before tick 0, and no text that is only whitespace — a line of nothing
/// is the silence between lines and is stored as one. Line breaks inside a line become spaces,
/// because a line is what a caption shows at once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "LyricMapRepr")]
pub struct LyricMap {
    points: Vec<LyricLine>,
}

/// On-disk shape of a [`LyricMap`], so a hand-edited file re-enters through the same
/// normalisation every mutator upholds.
#[derive(Deserialize)]
struct LyricMapRepr {
    #[serde(default)]
    points: Vec<LyricLine>,
}

impl From<LyricMapRepr> for LyricMap {
    fn from(repr: LyricMapRepr) -> Self {
        Self::new(repr.points)
    }
}

impl LyricMap {
    /// Builds a timeline from arbitrary points, normalising order, duplicates, negative ticks
    /// and text that is nothing but whitespace. A run of silences collapses to the first.
    pub fn new(mut points: Vec<LyricLine>) -> Self {
        for point in &mut points {
            point.tick = point.tick.max_zero();
            point.text = normalised(point.text.take());
        }
        points.sort_by_key(|point| point.tick);
        points.dedup_by_key(|point| point.tick);
        let mut map = Self { points };
        map.tidy();
        map
    }

    /// The line changes, ordered by position.
    pub fn points(&self) -> &[LyricLine] {
        &self.points
    }

    /// `true` when no words are written anywhere.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Where the last change sits, or [`Ticks::ZERO`] when nothing has been written.
    pub fn end(&self) -> Ticks {
        self.points.last().map_or(Ticks::ZERO, |point| point.tick)
    }

    /// Inserts or replaces a change. `None` — or text that is only whitespace — starts a
    /// silence here.
    pub fn set_line(&mut self, tick: Ticks, text: Option<String>) {
        let tick = tick.max_zero();
        let text = normalised(text);
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => self.points[index].text = text,
            Err(index) => self.points.insert(index, LyricLine { tick, text }),
        }
        self.tidy();
    }

    /// Writes one sung line from `start` to `end`, leaving whatever was in force at `end`
    /// to carry on after it.
    ///
    /// Any change strictly inside the line is removed: the line *is* that stretch. An `end` at
    /// or before `start` writes a line that runs until the next change.
    pub fn write_line(&mut self, start: Ticks, end: Ticks, text: &str) {
        let start = start.max_zero();
        if end <= start {
            self.set_line(start, Some(text.to_owned()));
            return;
        }
        let after = self.text_at(end).map(str::to_owned);
        let starts_at_end = self.points.iter().any(|point| point.tick == end);
        self.points
            .retain(|point| point.tick <= start || point.tick >= end);
        self.set_line(start, Some(text.to_owned()));
        if !starts_at_end {
            self.set_line(end, after);
        }
    }

    /// Removes every change in `from..to` and leaves the stretch silent, restoring whatever
    /// was in force at `to` after it.
    pub fn clear_range(&mut self, from: Ticks, to: Ticks) {
        let (from, to) = (from.max_zero(), to.max_zero());
        if to <= from {
            return;
        }
        let after = self.text_at(to).map(str::to_owned);
        let starts_at_end = self.points.iter().any(|point| point.tick == to);
        self.points
            .retain(|point| point.tick < from || point.tick >= to);
        self.set_line(from, None);
        if !starts_at_end {
            self.set_line(to, after);
        }
    }

    /// Removes the change at `tick`, letting whatever line preceded it run through.
    pub fn remove_line(&mut self, tick: Ticks) {
        if let Ok(index) = self.points.binary_search_by_key(&tick, |point| point.tick) {
            self.points.remove(index);
            self.tidy();
        }
    }

    /// Where the change in force at `tick` sits, if there is one.
    pub fn change_at(&self, tick: Ticks) -> Option<Ticks> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        self.points.get(index).map(|point| point.tick)
    }

    /// Moves the change at `from` to `to`, and says whether there was one to move. Landing on
    /// another change replaces it, as a section or a chord does.
    pub fn move_line(&mut self, from: Ticks, to: Ticks) -> bool {
        let (from, to) = (from.max_zero(), to.max_zero());
        let Ok(index) = self.points.binary_search_by_key(&from, |point| point.tick) else {
            return false;
        };
        if from == to {
            return true;
        }
        let moved = self.points.remove(index);
        self.set_line(to, moved.text);
        true
    }

    /// The words in force at `tick`, or `None` in a silence or before the first line.
    pub fn text_at(&self, tick: Ticks) -> Option<&str> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        self.points
            .get(index)
            .and_then(|point| point.text.as_deref())
    }

    /// Every sung line crossing `from..to`, clipped to that window. Silences produce no span.
    ///
    /// A line with nothing after it runs to `to`; the last line of a song is ended by a
    /// silence, which [`Self::write_line`] always writes.
    pub fn spans_in(&self, from: Ticks, to: Ticks) -> Vec<LyricSpan> {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        let (from, to) = (from.max_zero(), to.max_zero());
        let mut spans = Vec::new();
        for (index, point) in self.points.iter().enumerate() {
            if point.tick >= to {
                break;
            }
            let Some(text) = point.text.as_deref() else {
                continue;
            };
            let end = self
                .points
                .get(index + 1)
                .map_or(to, |next| next.tick.min(to));
            if end <= from {
                continue;
            }
            spans.push(LyricSpan {
                start: point.tick.max(from),
                end,
                change: point.tick,
                text: text.to_string(),
            });
        }
        spans
    }

    /// Drops silences that say nothing: one at the very start of an otherwise empty map, and
    /// one that follows another silence.
    fn tidy(&mut self) {
        let mut previous_silent = true;
        self.points.retain(|point| {
            let silent = point.text.is_none();
            let keep = !(silent && previous_silent);
            if keep {
                previous_silent = silent;
            }
            keep
        });
    }
}

/// Text fit to store: one line, trimmed, and `None` when nothing is left.
fn normalised(text: Option<String>) -> Option<String> {
    let text = text?;
    let joined = text
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let joined: Vec<&str> = joined.collect();
    if joined.is_empty() {
        None
    } else {
        Some(joined.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(n: i64) -> Ticks {
        Ticks(3840 * n)
    }

    #[test]
    fn a_line_runs_until_the_next_change_and_a_silence_ends_it() {
        let mut lyrics = LyricMap::default();
        assert!(lyrics.is_empty());
        lyrics.write_line(bar(1), bar(2), "さくら さいた");
        lyrics.write_line(bar(3), bar(4), "春が来た");
        assert_eq!(lyrics.text_at(bar(0)), None);
        assert_eq!(lyrics.text_at(bar(1)), Some("さくら さいた"));
        assert_eq!(lyrics.text_at(bar(2)), None, "the silence between lines");
        assert_eq!(lyrics.text_at(bar(3) + Ticks(10)), Some("春が来た"));
        assert_eq!(lyrics.text_at(bar(40)), None, "the song ends silent");
        assert_eq!(lyrics.points().len(), 4);

        let spans = lyrics.spans_in(Ticks::ZERO, bar(8));
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[1].start, spans[1].end), (bar(3), bar(4)));
    }

    #[test]
    fn a_line_written_up_to_the_next_one_does_not_silence_it() {
        let mut lyrics = LyricMap::default();
        lyrics.write_line(bar(2), bar(3), "二行目");
        lyrics.write_line(bar(1), bar(2), "一行目");
        assert_eq!(lyrics.text_at(bar(2)), Some("二行目"));
        assert_eq!(lyrics.points().len(), 3);
    }

    #[test]
    fn writing_over_a_stretch_replaces_what_was_inside_and_keeps_what_follows() {
        let mut lyrics = LyricMap::default();
        lyrics.set_line(bar(0), Some("a".into()));
        lyrics.set_line(bar(1), Some("b".into()));
        lyrics.set_line(bar(2), Some("c".into()));
        lyrics.write_line(Ticks(1920), bar(2) + Ticks(1920), "new");
        assert_eq!(lyrics.text_at(bar(0)), Some("a"));
        assert_eq!(lyrics.text_at(bar(1)), Some("new"));
        assert_eq!(
            lyrics.text_at(bar(2) + Ticks(1920)),
            Some("c"),
            "c carries on"
        );

        lyrics.clear_range(bar(0), bar(3));
        assert_eq!(lyrics.text_at(bar(1)), None);
        assert_eq!(lyrics.text_at(bar(3)), Some("c"));
    }

    #[test]
    fn text_is_one_trimmed_line_and_blank_text_is_a_silence() {
        let lyrics = LyricMap::new(vec![
            LyricLine {
                tick: Ticks(-5),
                text: Some("  上を向いて\n歩こう ".into()),
            },
            LyricLine {
                tick: bar(1),
                text: Some("   ".into()),
            },
            LyricLine {
                tick: bar(2),
                text: None,
            },
        ]);
        assert_eq!(lyrics.points()[0].tick, Ticks::ZERO);
        assert_eq!(lyrics.text_at(Ticks::ZERO), Some("上を向いて 歩こう"));
        assert_eq!(lyrics.points().len(), 2, "two silences in a row are one");
    }

    #[test]
    fn a_change_is_found_from_inside_its_line_moved_and_removed() {
        let mut lyrics = LyricMap::default();
        lyrics.write_line(bar(1), bar(2), "x");
        assert_eq!(lyrics.change_at(bar(1) + Ticks(7)), Some(bar(1)));
        assert!(lyrics.move_line(bar(1), Ticks(960)));
        assert_eq!(lyrics.text_at(Ticks(960)), Some("x"));
        assert!(!lyrics.move_line(bar(5), bar(6)));
        lyrics.remove_line(Ticks(960));
        assert!(lyrics.is_empty(), "the trailing silence goes with it");
    }

    #[test]
    fn a_file_round_trips_and_a_hand_edit_is_normalised() {
        let mut lyrics = LyricMap::default();
        lyrics.write_line(bar(1), bar(2), "歌");
        let text = serde_json::to_string(&lyrics).unwrap();
        assert_eq!(serde_json::from_str::<LyricMap>(&text).unwrap(), lyrics);
        let edited: LyricMap = serde_json::from_str(
            r#"{"points":[{"tick":7680,"text":null},{"tick":3840,"text":"歌"},{"tick":3840}]}"#,
        )
        .unwrap();
        assert_eq!(edited, lyrics);
    }
}
