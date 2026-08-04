//! Song structure over the timeline: イントロ, Aメロ, サビ, and where each begins.
//!
//! A section label is a property of the timeline exactly as the key and the chords are: it
//! changes as the song goes on, at any one moment every track is in the same section, and no
//! track owns it. The map is therefore built to the same shape as
//! [`ChordMap`](crate::harmony::ChordMap) — a sorted list of points, each in force until the
//! next, with `None` for a stretch deliberately left unnamed — so that a reader who knows one
//! knows the other.
//!
//! # Instances are counted, not stored
//!
//! The second サビ is サビ 2, and what makes it so is that another サビ came before it — a fact
//! the map can always recompute, where a stored number could silently contradict the timeline
//! after an edit. [`SectionMap::spans_in`] counts occurrences from the start of the song, so a
//! span knows it is the second chorus even when the window shows only it.
//!
//! # A vocabulary, not an enum
//!
//! Labels are text. A fixed set of kinds would make Cメロ, 落ちサビ or anything a genre invents
//! tomorrow unsayable, and nothing here needs the classification: the composer keys its material
//! by the label itself, so two stretches called the same thing get recognisably the same
//! material, whatever the thing is called. Matching is exact — the frontends offer a completion
//! vocabulary precisely so the same section is spelled the same way twice.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// A change of section at a musical position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionPoint {
    /// Where the section begins.
    pub tick: Ticks,
    /// Its name from this point onwards, or `None` for a stretch deliberately left unnamed.
    ///
    /// The `None` is what makes ending a song's structure mean *ended* rather than *the outro
    /// runs on for ever*: the stretch after the last named section is a real thing, not an
    /// absence.
    #[serde(default)]
    pub label: Option<String>,
}

/// One named stretch of the song, as [`SectionMap::spans_in`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionSpan {
    /// Where the visible part of the span begins — clipped to the window it was asked for.
    pub start: Ticks,
    /// Where it ends: the next change, or the window's edge when the section runs past it.
    pub end: Ticks,
    /// Where the change that started this section actually sits, which may be left of the
    /// window. This is the position an editor acts *through* — the thing that gets renamed,
    /// moved or removed.
    pub change: Ticks,
    /// What the section is called.
    pub label: String,
    /// Which occurrence of this label it is, counting from one at the start of the song.
    ///
    /// A label that appears once is instance 1; the display decides whether saying so is
    /// helpful. See [`SectionMap::repeats`].
    pub instance: usize,
}

/// The song's structure over the timeline, each section in force until the next.
///
/// Like [`ChordMap`](crate::harmony::ChordMap) and unlike
/// [`KeyMap`](crate::harmony::KeyMap), this may be empty: a project nobody has structured has
/// no sections, and inventing an イントロ at tick 0 to make lookups total would be a lie the
/// composer would then go and obey. The other invariants match: sorted, no two points at the
/// same position, none before tick 0 — and no label that is only whitespace, because a section
/// called nothing *is* the unnamed stretch and is stored as one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SectionMapRepr")]
pub struct SectionMap {
    points: Vec<SectionPoint>,
}

/// On-disk shape of a [`SectionMap`], so a hand-edited file re-enters through the same
/// normalisation every mutator upholds.
#[derive(Deserialize)]
struct SectionMapRepr {
    #[serde(default)]
    points: Vec<SectionPoint>,
}

impl From<SectionMapRepr> for SectionMap {
    fn from(repr: SectionMapRepr) -> Self {
        Self::new(repr.points)
    }
}

impl SectionMap {
    /// Builds a timeline from arbitrary points, normalising order, duplicates, negative ticks
    /// and labels that are nothing but whitespace.
    pub fn new(mut points: Vec<SectionPoint>) -> Self {
        for point in &mut points {
            point.tick = point.tick.max_zero();
            point.label = normalised(point.label.take());
        }
        points.sort_by_key(|point| point.tick);
        points.dedup_by_key(|point| point.tick);
        Self { points }
    }

    /// The section changes, ordered by position.
    pub fn points(&self) -> &[SectionPoint] {
        &self.points
    }

    /// `true` when no section has been named anywhere.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Where the last change sits, or [`Ticks::ZERO`] when nothing has been written.
    pub fn end(&self) -> Ticks {
        self.points.last().map_or(Ticks::ZERO, |point| point.tick)
    }

    /// Inserts or replaces a change. `None` — or a name that is only whitespace — leaves the
    /// stretch from here unnamed.
    pub fn set_point(&mut self, tick: Ticks, label: Option<String>) {
        let tick = tick.max_zero();
        let label = normalised(label);
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => self.points[index].label = label,
            Err(index) => self.points.insert(index, SectionPoint { tick, label }),
        }
    }

    /// Removes the change at `tick`, letting whatever section preceded it run through.
    pub fn remove_point(&mut self, tick: Ticks) {
        if let Ok(index) = self.points.binary_search_by_key(&tick, |point| point.tick) {
            self.points.remove(index);
        }
    }

    /// Where the change in force at `tick` sits, if there is one.
    ///
    /// This is the position an editor acts *through*: a section occupies everything from where
    /// it starts to the next change, so "the section here" is almost never one that begins
    /// under the pixel the pointer landed on.
    pub fn change_at(&self, tick: Ticks) -> Option<Ticks> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        self.points.get(index).map(|point| point.tick)
    }

    /// Moves the change at `from` to `to`, and says whether there was one to move.
    ///
    /// Landing on another change replaces it, for the reason
    /// [`ChordMap::move_point`](crate::harmony::ChordMap::move_point) gives: refusing the move
    /// leaves the dragged label snapping back for a reason nothing on screen explains.
    pub fn move_point(&mut self, from: Ticks, to: Ticks) -> bool {
        let (from, to) = (from.max_zero(), to.max_zero());
        let Ok(index) = self.points.binary_search_by_key(&from, |point| point.tick) else {
            return false;
        };
        if from == to {
            return true;
        }
        let moved = self.points.remove(index);
        self.set_point(to, moved.label);
        true
    }

    /// The label in force at `tick`.
    ///
    /// `None` when no section has been named, `tick` precedes the first one, or an unnamed
    /// stretch is in force — undistinguished, because nothing needs to.
    pub fn label_at(&self, tick: Ticks) -> Option<&str> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        self.points
            .get(index)
            .and_then(|point| point.label.as_deref())
    }

    /// The section in force at `tick`: its label, and which occurrence of that label it is.
    pub fn section_at(&self, tick: Ticks) -> Option<(&str, usize)> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        let label = self.points.get(index)?.label.as_deref()?;
        Some((label, self.instance_of(index)))
    }

    /// How many times `label` occurs over the whole timeline.
    ///
    /// What a display asks before numbering: a label that appears once reads better bare, and
    /// one that repeats reads better as サビ 1 and サビ 2 than as サビ and サビ 2.
    pub fn repeats(&self, label: &str) -> usize {
        self.points
            .iter()
            .filter(|point| point.label.as_deref() == Some(label))
            .count()
    }

    /// Which occurrence of its own label the point at `index` is, counting from one.
    fn instance_of(&self, index: usize) -> usize {
        let label = self.points[index].label.as_deref();
        self.points[..=index]
            .iter()
            .filter(|point| point.label.as_deref() == label)
            .count()
    }

    /// Every named stretch crossing `from..to`, clipped to that window.
    ///
    /// Unnamed stretches produce no span, so the result tiles only the parts of the window
    /// that have structure. Instances are counted from the start of the song, not the window.
    pub fn spans_in(&self, from: Ticks, to: Ticks) -> Vec<SectionSpan> {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        let (from, to) = (from.max_zero(), to.max_zero());
        if from == to {
            return Vec::new();
        }

        let mut spans = Vec::new();
        for (index, point) in self.points.iter().enumerate() {
            if point.tick >= to {
                break;
            }
            let Some(label) = point.label.as_deref() else {
                continue;
            };
            let end = self
                .points
                .get(index + 1)
                .map_or(to, |next| next.tick.min(to));
            if end <= from {
                continue;
            }
            spans.push(SectionSpan {
                start: point.tick.max(from),
                end,
                change: point.tick,
                label: label.to_string(),
                instance: self.instance_of(index),
            });
        }
        spans
    }
}

/// A label fit to store: trimmed, and `None` when nothing is left.
fn normalised(label: Option<String>) -> Option<String> {
    let label = label?;
    let trimmed = label.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == label.len() {
        Some(label)
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(n: i64) -> Ticks {
        Ticks(3840 * n)
    }

    fn label(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn a_structure_may_be_empty_and_says_so() {
        let mut sections = SectionMap::default();
        assert!(sections.is_empty());
        assert_eq!(sections.label_at(Ticks::ZERO), None);

        sections.set_point(bar(4), label("サビ"));
        assert!(!sections.is_empty());
        assert_eq!(sections.label_at(bar(2)), None, "before the first section");
        assert_eq!(sections.label_at(bar(4)), Some("サビ"));
        assert_eq!(sections.label_at(bar(90)), Some("サビ"), "runs on");

        sections.set_point(bar(12), None);
        assert_eq!(
            sections.label_at(bar(90)),
            None,
            "an unnamed stretch is real"
        );
    }

    #[test]
    fn the_second_chorus_knows_it_is_the_second() {
        let mut sections = SectionMap::default();
        sections.set_point(Ticks::ZERO, label("イントロ"));
        sections.set_point(bar(4), label("Aメロ"));
        sections.set_point(bar(12), label("サビ"));
        sections.set_point(bar(20), label("Aメロ"));
        sections.set_point(bar(28), label("サビ"));

        assert_eq!(sections.section_at(bar(1)), Some(("イントロ", 1)));
        assert_eq!(sections.section_at(bar(13)), Some(("サビ", 1)));
        assert_eq!(sections.section_at(bar(21)), Some(("Aメロ", 2)));
        assert_eq!(sections.section_at(bar(30)), Some(("サビ", 2)));
        assert_eq!(sections.repeats("サビ"), 2);
        assert_eq!(sections.repeats("イントロ"), 1);
        assert_eq!(sections.repeats("Bメロ"), 0);
    }

    #[test]
    fn spans_are_clipped_to_the_window_but_numbered_from_the_song() {
        let mut sections = SectionMap::default();
        sections.set_point(Ticks::ZERO, label("サビ"));
        sections.set_point(bar(8), label("間奏"));
        sections.set_point(bar(12), label("サビ"));

        // A window that starts inside the second chorus still knows it is the second.
        let spans = sections.spans_in(bar(14), bar(20));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].label, "サビ");
        assert_eq!(spans[0].instance, 2);
        assert_eq!(spans[0].start, bar(14), "clipped to the window");
        assert_eq!(spans[0].change, bar(12), "but the change is where it is");
        assert_eq!(spans[0].end, bar(20), "the last section runs to the edge");

        // The whole song tiles with no gaps between named neighbours.
        let all = sections.spans_in(Ticks::ZERO, bar(20));
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].end, all[1].start);
        assert_eq!(all[1].end, all[2].start);
    }

    #[test]
    fn an_unnamed_stretch_leaves_a_hole_in_the_spans() {
        let mut sections = SectionMap::default();
        sections.set_point(Ticks::ZERO, label("Aメロ"));
        sections.set_point(bar(4), None);
        sections.set_point(bar(8), label("サビ"));

        let spans = sections.spans_in(Ticks::ZERO, bar(12));
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].end, bar(4), "the Aメロ ends where the hole begins");
        assert_eq!(spans[1].start, bar(8));
    }

    #[test]
    fn a_section_can_be_moved_and_takes_its_name_with_it() {
        let mut sections = SectionMap::default();
        sections.set_point(bar(1), label("Aメロ"));
        sections.set_point(bar(2), label("サビ"));

        assert!(sections.move_point(bar(2), bar(5)));
        assert_eq!(sections.label_at(bar(5)), Some("サビ"));
        assert_eq!(
            sections.label_at(bar(2)),
            Some("Aメロ"),
            "the section before it now runs through where it was"
        );
        for pair in sections.points().windows(2) {
            assert!(pair[0].tick < pair[1].tick);
        }

        assert!(
            !sections.move_point(bar(9), bar(1)),
            "nothing sits at bar 9"
        );
        // Dropping one onto another leaves one section, not two at the same tick.
        assert!(sections.move_point(bar(5), bar(1)));
        assert_eq!(sections.points().len(), 1);
        assert_eq!(sections.label_at(bar(1)), Some("サビ"));
    }

    #[test]
    fn the_change_in_force_is_found_from_anywhere_inside_it() {
        let mut sections = SectionMap::default();
        assert_eq!(sections.change_at(bar(4)), None, "nothing written yet");

        sections.set_point(bar(1), label("Aメロ"));
        sections.set_point(bar(3), label("サビ"));
        assert_eq!(
            sections.change_at(Ticks::ZERO),
            None,
            "before the first one"
        );
        assert_eq!(sections.change_at(bar(2)), Some(bar(1)), "part-way through");
        assert_eq!(sections.change_at(bar(9)), Some(bar(3)), "the last runs on");
    }

    #[test]
    fn a_name_of_nothing_but_whitespace_is_the_unnamed_stretch() {
        // A section called nothing is not a different thing from no section; storing the
        // difference would make two states the display cannot tell apart.
        let mut sections = SectionMap::default();
        sections.set_point(Ticks::ZERO, label("  "));
        assert_eq!(sections.label_at(Ticks::ZERO), None);
        sections.set_point(Ticks::ZERO, label("  サビ "));
        assert_eq!(
            sections.label_at(Ticks::ZERO),
            Some("サビ"),
            "and names are trimmed"
        );
    }

    #[test]
    fn the_structure_wire_shape_does_not_drift() {
        let mut sections = SectionMap::default();
        sections.set_point(Ticks::ZERO, label("イントロ"));
        sections.set_point(bar(4), label("サビ"));
        sections.set_point(bar(8), None);

        let json = serde_json::to_string(&sections).unwrap();
        assert_eq!(
            json,
            r#"{"points":[{"tick":0,"label":"イントロ"},{"tick":15360,"label":"サビ"},{"tick":30720,"label":null}]}"#
        );
        assert_eq!(serde_json::from_str::<SectionMap>(&json).unwrap(), sections);
    }

    #[test]
    fn a_hand_edited_file_re_enters_through_the_same_normalisation() {
        // Out of order, a negative tick, a duplicate position and a whitespace label: the same
        // invariants every mutator upholds have to come back out of a file.
        let json = r#"{"points":[
            {"tick":7680,"label":"サビ"},
            {"tick":-99,"label":"イントロ"},
            {"tick":7680,"label":"ignored"},
            {"tick":3840,"label":"   "}
        ]}"#;
        let sections: SectionMap = serde_json::from_str(json).unwrap();
        assert_eq!(sections.points().len(), 3);
        assert_eq!(sections.points()[0].tick, Ticks::ZERO);
        assert_eq!(sections.label_at(Ticks::ZERO), Some("イントロ"));
        assert_eq!(
            sections.label_at(bar(1)),
            None,
            "whitespace reads as unnamed"
        );
        for pair in sections.points().windows(2) {
            assert!(pair[0].tick < pair[1].tick);
        }
    }
}
