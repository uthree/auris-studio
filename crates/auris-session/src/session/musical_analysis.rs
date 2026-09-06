//! Descriptive measurements of stored notes, independent of rendering and sound libraries.

use std::collections::{BTreeMap, BTreeSet};

use auris_core::time::{TICKS_PER_QUARTER, Ticks};
use auris_core::{ClipId, TrackId};

use super::Session;

/// Measurements of one clip's written notes, before playback transforms.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MusicalClipAnalysis {
    /// Owning track.
    pub track: TrackId,
    /// Clip measured.
    pub clip: ClipId,
    /// Number of written notes.
    pub notes: usize,
    /// Onsets per quarter note of clip length.
    pub notes_per_beat: f64,
    /// Lowest and highest MIDI pitches, or no range for an empty clip.
    pub pitch_range: Option<(u8, u8)>,
    /// Number of distinct pitch classes.
    pub pitch_classes: usize,
    /// Bars containing written note onsets.
    pub nonempty_bars: usize,
    /// Nonempty bars repeating an earlier bar's exact pitches, onsets and note lengths.
    pub repeated_bars: usize,
}

impl Session {
    /// Measures range, density and exact bar-pattern repetition in the stored score.
    ///
    /// These describe choices, not musical quality. Velocity is excluded from repetition;
    /// playback transforms, automation, audio clips and the sound of instruments are not read.
    pub fn analyze_music(&self) -> Vec<MusicalClipAnalysis> {
        let mut reports = Vec::new();
        for track in &self.project.tracks {
            for clip in track.kind.note_clips().into_iter().flatten() {
                let mut classes = BTreeSet::new();
                let mut bars: BTreeMap<u32, Vec<(i64, u8, i64)>> = BTreeMap::new();
                for note in &clip.notes {
                    classes.insert(note.pitch % 12);
                    let at = clip.start + note.start;
                    let bar = self.project.signatures.bar_of(at);
                    let offset = at - self.project.signatures.bar_start(bar);
                    bars.entry(bar).or_default().push((
                        offset.raw(),
                        note.pitch,
                        note.length.raw(),
                    ));
                }
                let nonempty_bars = bars.len();
                let mut patterns = BTreeSet::new();
                for pattern in bars.values_mut() {
                    pattern.sort();
                    patterns.insert(pattern.clone());
                }
                let range = clip
                    .notes
                    .iter()
                    .map(|note| note.pitch)
                    .min()
                    .zip(clip.notes.iter().map(|note| note.pitch).max());
                let beats = clip.length.max(Ticks(1)).raw() as f64 / TICKS_PER_QUARTER as f64;
                reports.push(MusicalClipAnalysis {
                    track: track.id,
                    clip: clip.id,
                    notes: clip.notes.len(),
                    notes_per_beat: clip.notes.len() as f64 / beats,
                    pitch_range: range,
                    pitch_classes: classes.len(),
                    nonempty_bars,
                    repeated_bars: nonempty_bars - patterns.len(),
                });
            }
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::Note;

    #[test]
    fn range_density_and_repetition_are_measured_from_the_written_score() {
        let mut session = session();
        let track = session.add_default_instrument_track("Tune").unwrap();
        let clip = session
            .add_midi_clip(track, "Repeated", Ticks::ZERO, Ticks::QUARTER * 8)
            .unwrap();
        for beat in [0, 4] {
            session
                .add_note(clip, Note::new(60, Ticks::QUARTER * beat, Ticks::QUARTER))
                .unwrap();
            session
                .add_note(
                    clip,
                    Note::new(67, Ticks::QUARTER * (beat + 1), Ticks::QUARTER),
                )
                .unwrap();
        }
        let before = session.project().clone();
        let reports = session.analyze_music();
        assert_eq!(reports[0].notes, 4);
        assert_eq!(reports[0].notes_per_beat, 0.5);
        assert_eq!(reports[0].pitch_range, Some((60, 67)));
        assert_eq!(reports[0].pitch_classes, 2);
        assert_eq!(reports[0].nonempty_bars, 2);
        assert_eq!(reports[0].repeated_bars, 1);
        assert_eq!(session.project(), &before);
    }
}
