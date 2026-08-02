//! What a swept rectangle covers.
//!
//! The geometry is separated from the views so it can be tested without a window: a rubber band
//! is easy to get subtly wrong — dragging up-and-left has to select the same things as dragging
//! down-and-right, and an item only touched at its edge has to count.

use std::collections::BTreeSet;

use auris_session::prelude::*;
use gpui::{Bounds, Pixels, Point, point, size};

use crate::app::{AurisApp, BandSurface, Drag};

/// The rectangle between two corners.
///
/// Built from a min/max pair rather than from "start plus delta" so a band swept in any
/// direction is the same rectangle.
pub fn band_bounds(anchor: Point<Pixels>, current: Point<Pixels>) -> Bounds<Pixels> {
    let left = anchor.x.min(current.x);
    let top = anchor.y.min(current.y);
    Bounds {
        origin: point(left, top),
        size: size(
            anchor.x.max(current.x) - left,
            anchor.y.max(current.y) - top,
        ),
    }
}

/// Whether something occupying `start..start + length` overlaps `span`.
///
/// Touching counts as overlapping only where there is width to touch: a band swept to zero
/// width selects nothing, which is what makes a click on empty space a deselect.
pub fn spans_overlap(start: Ticks, length: Ticks, span: (Ticks, Ticks)) -> bool {
    let (from, to) = (span.0.min(span.1), span.0.max(span.1));
    from < to && start < to && start + length > from
}

/// Indices of the notes overlapping a rectangle in clip-relative ticks and pitches.
pub fn notes_in_band(notes: &[Note], ticks: (Ticks, Ticks), pitches: (u8, u8)) -> BTreeSet<usize> {
    let (low, high) = (pitches.0.min(pitches.1), pitches.0.max(pitches.1));
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| {
            note.pitch >= low && note.pitch <= high && spans_overlap(note.start, note.length, ticks)
        })
        .map(|(index, _)| index)
        .collect()
}

impl AurisApp {
    /// Begins sweeping a selection rectangle from `anchor`, in window coordinates.
    ///
    /// A shift-drag adds to what was already selected; a plain one replaces it — including
    /// replacing it with nothing, which is how a click on empty space deselects.
    pub(crate) fn begin_rubber_band(
        &mut self,
        surface: BandSurface,
        anchor: Point<Pixels>,
        additive: bool,
    ) {
        let (base_notes, base_clips) = if additive {
            (self.selected_notes.clone(), self.selected_clips.clone())
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        self.begin_drag(Drag::RubberBand {
            surface,
            anchor,
            current: anchor,
            base_notes,
            base_clips,
        });
        self.apply_rubber_band();
    }

    /// The rectangle being swept, in window coordinates.
    pub(crate) fn rubber_band(&self, surface: BandSurface) -> Option<Bounds<Pixels>> {
        match &self.drag {
            Some(Drag::RubberBand {
                surface: active,
                anchor,
                current,
                ..
            }) if *active == surface => Some(band_bounds(*anchor, *current)),
            _ => None,
        }
        .filter(|band| band.size.width > Pixels::ZERO || band.size.height > Pixels::ZERO)
    }

    /// Replaces the selection with whatever the band currently covers.
    pub(crate) fn apply_rubber_band(&mut self) {
        let Some(Drag::RubberBand {
            surface,
            anchor,
            current,
            base_notes,
            base_clips,
        }) = self.drag.clone()
        else {
            return;
        };
        let band = band_bounds(anchor, current);
        match surface {
            BandSurface::Roll => {
                let mut selected = self.notes_under(band);
                selected.extend(base_notes);
                self.selected_notes = selected;
            }
            BandSurface::Lanes => {
                let mut selected = self.clips_under(band);
                selected.extend(base_clips);
                let primary = self.selected_clip;
                self.select_clips(selected, primary);
            }
        }
    }

    /// Notes the band covers, in the piano roll.
    fn notes_under(&self, band: Bounds<Pixels>) -> BTreeSet<usize> {
        let Some(clip) = self.selected_midi_clip() else {
            return BTreeSet::new();
        };
        let origin = self.roll_origin();
        let ticks = (
            self.timeline.x_to_tick(band.origin.x - origin.x) - clip.start,
            self.timeline
                .x_to_tick(band.origin.x + band.size.width - origin.x)
                - clip.start,
        );
        // `y_to_pitch` clamps, so a band dragged off the top selects up to the highest note
        // rather than stopping at the edge of the visible grid.
        let pitches = (
            self.pitch
                .y_to_pitch(band.origin.y + band.size.height - origin.y),
            self.pitch.y_to_pitch(band.origin.y - origin.y),
        );
        notes_in_band(&clip.notes, ticks, pitches)
    }

    /// Clips the band covers, in the arrangement.
    fn clips_under(&self, band: Bounds<Pixels>) -> BTreeSet<ClipId> {
        let origin = self.lanes_origin();
        let span = (
            self.timeline.x_to_tick(band.origin.x - origin.x),
            self.timeline
                .x_to_tick(band.origin.x + band.size.width - origin.x),
        );
        let rows = (
            band.origin.y - origin.y,
            band.origin.y + band.size.height - origin.y,
        );

        let mut selected = BTreeSet::new();
        for track in self.tracks_in_rows(rows.0, rows.1) {
            let Some(track) = self.project().track(track) else {
                continue;
            };
            match &track.kind {
                TrackKind::Instrument(inner) => selected.extend(
                    inner
                        .clips
                        .iter()
                        .filter(|clip| spans_overlap(clip.start, clip.length, span))
                        .map(|clip| clip.id),
                ),
                TrackKind::Audio(inner) => selected.extend(
                    inner
                        .clips
                        .iter()
                        .filter(|clip| {
                            spans_overlap(clip.start, self.audio_clip_length_ticks(clip), span)
                        })
                        .map(|clip| clip.id),
                ),
            }
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn notes() -> Vec<Note> {
        vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(64, Ticks::QUARTER, Ticks::QUARTER),
            Note::new(67, Ticks::from_beats(2.0), Ticks::QUARTER),
        ]
    }

    #[test]
    fn a_band_is_the_same_rectangle_whichever_way_it_is_swept() {
        let a = point(px(10.0), px(20.0));
        let b = point(px(60.0), px(90.0));
        assert_eq!(band_bounds(a, b), band_bounds(b, a));
        assert_eq!(band_bounds(a, b).origin, a);
        assert_eq!(band_bounds(a, b).size, size(px(50.0), px(70.0)));
    }

    #[test]
    fn a_band_selects_everything_it_touches() {
        let notes = notes();
        // Across the first two notes, at pitches that include both.
        let selected = notes_in_band(&notes, (Ticks::ZERO, Ticks::from_beats(2.0)), (60, 64));
        assert_eq!(selected, BTreeSet::from([0, 1]));
    }

    #[test]
    fn a_note_only_clipped_at_its_edge_still_counts() {
        let notes = notes();
        // The band ends one tick into the first note.
        let selected = notes_in_band(&notes, (Ticks(-10), Ticks(1)), (0, 127));
        assert_eq!(selected, BTreeSet::from([0]));
    }

    #[test]
    fn a_band_swept_backwards_selects_the_same_notes() {
        let notes = notes();
        let forwards = notes_in_band(&notes, (Ticks::ZERO, Ticks::from_beats(2.0)), (60, 67));
        let backwards = notes_in_band(&notes, (Ticks::from_beats(2.0), Ticks::ZERO), (67, 60));
        assert_eq!(forwards, backwards);
    }

    #[test]
    fn pitches_outside_the_band_are_left_alone() {
        let notes = notes();
        let selected = notes_in_band(&notes, (Ticks::ZERO, Ticks::from_beats(4.0)), (63, 65));
        assert_eq!(selected, BTreeSet::from([1]), "only the note at 64");
    }

    #[test]
    fn a_band_with_no_width_selects_nothing() {
        let notes = notes();
        let at_a_point = notes_in_band(&notes, (Ticks(100), Ticks(100)), (0, 127));
        assert!(
            at_a_point.is_empty(),
            "a click that never moved must deselect rather than select what it landed on"
        );
    }

    #[test]
    fn clip_spans_overlap_the_way_note_spans_do() {
        let span = (Ticks::from_beats(1.0), Ticks::from_beats(3.0));
        // Straddling the left edge, wholly inside, straddling the right edge.
        assert!(spans_overlap(Ticks::ZERO, Ticks::from_beats(2.0), span));
        assert!(spans_overlap(
            Ticks::from_beats(1.5),
            Ticks::from_beats(0.5),
            span
        ));
        assert!(spans_overlap(
            Ticks::from_beats(2.5),
            Ticks::from_beats(4.0),
            span
        ));
        // Ending exactly where the band starts, and starting exactly where it ends.
        assert!(!spans_overlap(Ticks::ZERO, Ticks::from_beats(1.0), span));
        assert!(!spans_overlap(
            Ticks::from_beats(3.0),
            Ticks::from_beats(1.0),
            span
        ));
    }
}
