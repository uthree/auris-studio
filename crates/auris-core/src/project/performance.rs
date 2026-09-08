//! Phrase-aware performance, built off the audio thread and shared by playback and export.

use crate::time::{SignatureMap, Ticks};

use super::{Note, NoteTransform, StrokeDirection, performed};

/// Context for one pass through a clip's performance stack.
#[derive(Clone, Copy)]
pub struct PerformanceContext<'a> {
    /// Tempo at the clip's start, for millisecond articulation lengths.
    pub bpm: f64,
    /// Loop pass, used to choose reproducible humanisation.
    pub pass: u64,
    /// Absolute timeline start of this pass, for metrical brush placement.
    pub start: Ticks,
    /// The content window; inserted notes never start outside it.
    pub length: Ticks,
    /// The project's meter, including changes within the clip.
    pub signatures: &'a SignatureMap,
}

/// Applies the stack in order to a private copy of a phrase.
///
/// Each articulation reads the result of the preceding stage. Additions are not fed back
/// into their own stage. Stored notes, lyrics and curves are never mutated; inserted notes
/// carry no copied lyrics or singer ornaments. This function allocates and must run while
/// preparing a graph, never in an instrument's audio callback.
pub fn performed_notes(
    mut notes: Vec<Note>,
    transforms: &[NoteTransform],
    context: PerformanceContext<'_>,
) -> Vec<Note> {
    let written_count = notes.len();
    for transform in transforms {
        match transform {
            NoteTransform::ForDrumVoice { voice, transforms } => {
                let selected = notes
                    .iter()
                    .filter(|note| note.drum_voice == *voice)
                    .cloned()
                    .collect();
                let mut changed = performed_notes(selected, transforms, context).into_iter();
                for note in notes.iter_mut().filter(|note| note.drum_voice == *voice) {
                    if let Some(replacement) = changed.next() {
                        *note = replacement;
                    }
                }
                notes.extend(changed);
            }
            NoteTransform::Stroke {
                spread_ms,
                direction,
            } => {
                stroke(&mut notes, *spread_ms, *direction, context);
            }
            NoteTransform::Mute { amount } => mute(&mut notes, *amount, context),
            NoteTransform::Brush { amount } => brush(&mut notes, *amount, context),
            NoteTransform::Slide { amount } => slide(&mut notes, *amount, context),
            local => {
                notes = notes
                    .into_iter()
                    .map(|note| {
                        performed(note, std::slice::from_ref(local), context.pass, context.bpm)
                    })
                    .collect();
            }
        }
    }
    // A later timing stage can move an ornament into its source or next attack. Give written
    // notes priority so an ornament's NoteOff cannot silence a still-held written note.
    let additions = notes.split_off(written_count);
    for mut addition in additions {
        let mut occupied: Vec<_> = notes
            .iter()
            .filter(|note| note.pitch == addition.pitch)
            .collect();
        occupied.sort_by_key(|note| note.start);
        let mut start = addition.start;
        let mut end = addition.end();
        for note in occupied {
            if note.end() <= start {
                continue;
            }
            if note.start >= end {
                break;
            }
            if note.start <= start {
                start = note.end();
            } else {
                end = note.start;
                break;
            }
        }
        if start < end {
            addition.start = start;
            addition.length = end - start;
            notes.push(addition);
        }
    }
    notes
}

fn milliseconds(value: f32, bpm: f64) -> Ticks {
    Ticks(
        (f64::from(value) * Ticks::QUARTER.raw() as f64 * bpm.max(0.0) / 60_000.0)
            .round()
            .max(1.0) as i64,
    )
}

/// Index groups in time and pitch order, independent of the vector's insertion order.
fn chords(notes: &[Note]) -> Vec<Vec<usize>> {
    let mut indices: Vec<_> = (0..notes.len()).collect();
    indices.sort_by_key(|&i| (notes[i].start, notes[i].pitch));
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        if let Some(last) = groups.last_mut()
            && notes[last[0]].start == notes[index].start
        {
            last.push(index);
        } else {
            groups.push(vec![index]);
        }
    }
    groups
}

fn stroke(
    notes: &mut [Note],
    spread_ms: f32,
    direction: StrokeDirection,
    context: PerformanceContext<'_>,
) {
    if spread_ms <= 0.0 {
        return;
    }
    let groups = chords(notes);
    for (index, group) in groups.iter().enumerate() {
        if group.len() < 2 {
            continue;
        }
        let start = notes[group[0]].start;
        let next = groups
            .get(index + 1)
            .map_or(context.length, |g| notes[g[0]].start);
        let shortest = group
            .iter()
            .map(|&i| notes[i].length.raw())
            .min()
            .unwrap_or(1);
        let spread = milliseconds(spread_ms.clamp(0.0, 100.0), context.bpm)
            .raw()
            .min(shortest - 1)
            .min((next - start).raw() - 1)
            .max(0);
        let reverse = match direction {
            StrokeDirection::LowToHigh => false,
            StrokeDirection::HighToLow => true,
            StrokeDirection::Alternate => index % 2 == 1,
        };
        for (rank, &i) in group.iter().enumerate() {
            let rank = if reverse {
                group.len() - 1 - rank
            } else {
                rank
            };
            let offset = Ticks(spread * rank as i64 / (group.len() - 1) as i64);
            notes[i].start += offset;
            notes[i].length -= offset;
        }
    }
}

fn ornament(source: &Note, pitch: u8, start: Ticks, length: Ticks, gain: f32) -> Note {
    Note {
        velocity: (source.velocity * gain).clamp(0.0, 1.0),
        drum_voice: source.drum_voice.clone(),
        ..Note::new(pitch, start, length)
    }
}

fn overlaps(note: &Note, start: Ticks, end: Ticks) -> bool {
    note.start < end && note.end() > start
}

fn mute(notes: &mut Vec<Note>, amount: f32, context: PerformanceContext<'_>) {
    if amount <= 0.0 {
        return;
    }
    let mut added = Vec::new();
    for note in notes.iter() {
        let start = note.end();
        if start < Ticks::ZERO || start >= context.length {
            continue;
        }
        let length = milliseconds(12.0, context.bpm).min(context.length - start);
        if notes
            .iter()
            .chain(&added)
            .any(|other: &Note| other.pitch == note.pitch && overlaps(other, start, start + length))
        {
            continue;
        }
        added.push(ornament(
            note,
            note.pitch,
            start,
            length,
            0.35 * amount.clamp(0.0, 1.0),
        ));
    }
    notes.extend(added);
}

fn brush(notes: &mut Vec<Note>, amount: f32, context: PerformanceContext<'_>) {
    if amount <= 0.0 || notes.is_empty() {
        return;
    }
    let groups = chords(notes);
    let mut added = Vec::new();
    let mut previous = None;
    let mut next_group = 0;
    let mut absolute = context.signatures.bar_floor(context.start);
    while absolute < context.start + context.length {
        let start = absolute - context.start;
        if start >= Ticks::ZERO {
            while next_group < groups.len() && notes[groups[next_group][0]].start < start {
                previous = Some(next_group);
                next_group += 1;
            }
            let length = milliseconds(20.0, context.bpm).min(context.length - start);
            if let Some(group) = previous
                && !notes
                    .iter()
                    .any(|note| overlaps(note, start, start + length))
            {
                for &index in &groups[group] {
                    let note = &notes[index];
                    if !added
                        .iter()
                        .any(|other: &Note| other.start == start && other.pitch == note.pitch)
                    {
                        added.push(ornament(
                            note,
                            note.pitch,
                            start,
                            length,
                            0.30 * amount.clamp(0.0, 1.0),
                        ));
                    }
                }
            }
        }
        // Compound time follows dotted beats, and every bar re-reads its meter.
        absolute += context.signatures.signature_at(absolute).beat_ticks();
    }
    notes.extend(added);
}

fn slide(notes: &mut Vec<Note>, amount: f32, context: PerformanceContext<'_>) {
    if amount <= 0.0 {
        return;
    }
    let groups = chords(notes);
    let mut added = Vec::new();
    for pair in groups.windows(2) {
        // Polyphonic voice assignment is ambiguous; only connect a clear single-note line.
        if pair[0].len() != 1 || pair[1].len() != 1 {
            continue;
        }
        let from = pair[0][0];
        let to = pair[1][0];
        let source = &notes[from];
        let target = &notes[to];
        let interval = i16::from(target.pitch) - i16::from(source.pitch);
        if interval.abs() < 2
            || source.end() > target.start
            || target.start - source.end() > Ticks::QUARTER
            || source.length.raw() < 4
            || target.start >= context.length
        {
            continue;
        }
        let length = milliseconds(60.0, context.bpm).min(Ticks(source.length.raw() / 4));
        let start = target.start - length;
        if notes
            .iter()
            .enumerate()
            .any(|(i, note)| i != from && overlaps(note, start, target.start))
        {
            continue;
        }
        let pitch = (i16::from(source.pitch) + interval / 2) as u8;
        added.push(ornament(
            source,
            pitch,
            start,
            length,
            0.65 * amount.clamp(0.0, 1.0),
        ));
        if notes[from].end() > start {
            notes[from].length = start - notes[from].start;
        }
    }
    notes.extend(added);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SignaturePoint;
    use crate::{ClipId, MidiClip, TimeSignature};

    fn clip(notes: Vec<Note>, transforms: Vec<NoteTransform>) -> MidiClip {
        MidiClip {
            notes,
            transforms,
            ..MidiClip::new(ClipId(1), "Phrase", Ticks::ZERO, Ticks(3840))
        }
    }

    fn n(pitch: u8, start: i64, length: i64) -> Note {
        Note::new(pitch, Ticks(start), Ticks(length))
    }

    #[test]
    fn strokes_order_chords_by_pitch_and_keep_the_written_releases() {
        let original = vec![n(67, 0, 480), n(60, 0, 480), n(64, 0, 480)];
        for (direction, expected) in [
            (StrokeDirection::LowToHigh, [96, 0, 48]),
            (StrokeDirection::HighToLow, [0, 96, 48]),
        ] {
            let clip = clip(
                original.clone(),
                vec![NoteTransform::Stroke {
                    spread_ms: 50.0,
                    direction,
                }],
            );
            let heard: Vec<_> = clip.sounding_notes(120.0).collect();
            for (note, offset) in heard.iter().zip(expected) {
                assert_eq!(note.start, Ticks(offset));
                assert_eq!(note.end(), Ticks(480));
            }
            assert_eq!(clip.notes, original);
        }
    }

    #[test]
    fn alternating_strokes_do_not_cross_the_next_attack_or_erase_short_notes() {
        let clip = clip(
            vec![n(60, 0, 20), n(67, 0, 20), n(60, 10, 100), n(67, 10, 100)],
            vec![NoteTransform::Stroke {
                spread_ms: 1000.0,
                direction: StrokeDirection::Alternate,
            }],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(heard[0].start, Ticks::ZERO);
        assert_eq!(heard[1].start, Ticks(9));
        assert_eq!(heard[2].start, Ticks(109));
        assert_eq!(heard[3].start, Ticks(10));
        assert!(heard.iter().all(|note| note.length > Ticks::ZERO));
    }

    #[test]
    fn mute_retriggers_releases_quietly_without_releasing_a_following_note() {
        let mut original = n(60, 0, 480);
        original.lyric = "word".into();
        let clip = clip(
            vec![original.clone(), n(64, 0, 480), n(64, 480, 480)],
            vec![NoteTransform::Mute { amount: 1.0 }],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        let mute = &heard[3];
        assert_eq!(
            (mute.pitch, mute.start, mute.length),
            (60, Ticks(480), Ticks(23))
        );
        assert!((mute.velocity - 0.28).abs() < 1e-6);
        assert!(mute.lyric.is_empty() && mute.phonemes.is_empty());
        assert_eq!(
            heard
                .iter()
                .filter(|note| note.pitch == 64 && note.start == Ticks(480))
                .count(),
            1
        );
        assert_eq!(clip.notes[0], original);
    }

    #[test]
    fn brush_uses_only_silent_intervals_and_the_latest_chord() {
        let clip = clip(
            vec![
                n(60, 0, 100),
                n(64, 0, 100),
                n(62, 1900, 40),
                n(65, 1900, 40),
            ],
            vec![NoteTransform::Brush { amount: 1.0 }],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(heard.len(), 8);
        assert_eq!(
            heard[4..]
                .iter()
                .map(|n| (n.pitch, n.start.raw()))
                .collect::<Vec<_>>(),
            vec![(60, 960), (64, 960), (62, 2880), (65, 2880)]
        );
        assert!(
            heard[4..]
                .iter()
                .all(|n| n.length == Ticks(38) && (n.velocity - 0.24).abs() < 1e-6)
        );
    }

    #[test]
    fn brush_tracks_compound_meter_changes_and_offbeat_clip_origins() {
        let signatures = SignatureMap::from_points(vec![
            SignaturePoint {
                tick: Ticks::ZERO,
                signature: TimeSignature::new(6, 8),
            },
            SignaturePoint {
                tick: Ticks(2880),
                signature: TimeSignature::new(3, 4),
            },
        ]);
        let mut clip = clip(
            vec![n(60, 0, 100)],
            vec![NoteTransform::Brush { amount: 1.0 }],
        );
        clip.start = Ticks(240);
        let heard: Vec<_> = clip.sounding_notes_with_meter(120.0, signatures).collect();
        assert_eq!(
            heard[1..].iter().map(|n| n.start.raw()).collect::<Vec<_>>(),
            vec![1200, 2640, 3600]
        );
    }

    #[test]
    fn slides_bridge_ascending_and_descending_legato_without_copying_lyrics() {
        for (from, to, middle) in [(60, 67, 63), (67, 60, 64)] {
            let clip = clip(
                vec![n(from, 0, 960), n(to, 960, 960)],
                vec![NoteTransform::Slide { amount: 1.0 }],
            );
            let heard: Vec<_> = clip.sounding_notes(120.0).collect();
            assert_eq!(heard.len(), 3);
            assert_eq!(heard[2].pitch, middle);
            assert_eq!(heard[2].length, Ticks(115));
            assert_eq!(heard[0].end(), heard[2].start);
            assert_eq!(heard[2].end(), heard[1].start);
            assert_eq!(clip.notes[0].length, Ticks(960));
        }
    }

    #[test]
    fn slides_leave_semitones_chords_overlaps_and_distant_phrases_alone() {
        for notes in [
            vec![n(60, 0, 960), n(61, 960, 960)],
            vec![n(60, 0, 960), n(64, 0, 960), n(67, 960, 960)],
            vec![n(60, 0, 1200), n(67, 960, 960)],
            vec![n(60, 0, 100), n(67, 1920, 960)],
        ] {
            let clip = clip(notes.clone(), vec![NoteTransform::Slide { amount: 1.0 }]);
            assert_eq!(clip.sounding_notes(120.0).collect::<Vec<_>>(), notes);
        }
    }

    #[test]
    fn ornaments_are_clipped_at_partial_loop_ends_and_round_trip_in_the_file() {
        let mut clip = clip(
            vec![n(60, 0, 100), n(64, 0, 100)],
            vec![
                NoteTransform::Brush { amount: 0.5 },
                NoteTransform::Slide { amount: 0.3 },
                NoteTransform::Mute { amount: 0.5 },
                NoteTransform::Stroke {
                    spread_ms: 20.0,
                    direction: StrokeDirection::Alternate,
                },
                NoteTransform::Humanize {
                    amount: 0.5,
                    seed: 42,
                },
            ],
        );
        clip.loop_end = Ticks(4810);
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert!(
            heard.iter().all(|n| n.start >= Ticks::ZERO
                && n.end() <= clip.loop_end
                && n.length > Ticks::ZERO)
        );
        assert!(heard.iter().any(|n| n.start > clip.length));
        let loaded: MidiClip =
            serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(loaded.sounding_notes(120.0).collect::<Vec<_>>(), heard);
        assert_eq!(clip.notes, vec![n(60, 0, 100), n(64, 0, 100)]);
    }

    #[test]
    fn scoped_articulations_keep_other_drum_voices_untouched() {
        let mut first = n(60, 0, 100);
        first.drum_voice = "first".into();
        let mut second = n(62, 0, 200);
        second.drum_voice = "second".into();
        let clip = clip(
            vec![first.clone(), second.clone()],
            vec![NoteTransform::ForDrumVoice {
                voice: "first".into(),
                transforms: vec![NoteTransform::Mute { amount: 0.5 }],
            }],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(&heard[..2], &[first, second]);
        assert_eq!(heard[2].drum_voice, "first");
    }

    #[test]
    fn humanized_ornaments_never_cut_off_written_notes_on_the_same_pitch() {
        for seed in 0..64 {
            let clip = clip(
                vec![n(60, 0, 480), n(60, 960, 480)],
                vec![
                    NoteTransform::Mute { amount: 1.0 },
                    NoteTransform::Brush { amount: 1.0 },
                    NoteTransform::Humanize { amount: 1.0, seed },
                ],
            );
            let heard: Vec<_> = clip.sounding_notes(120.0).collect();
            for addition in &heard[2..] {
                assert!(heard[..2].iter().all(|note| !overlaps(
                    note,
                    addition.start,
                    addition.end()
                )));
            }
        }
    }
}
