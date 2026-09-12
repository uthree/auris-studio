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
    notes: Vec<Note>,
    transforms: &[NoteTransform],
    context: PerformanceContext<'_>,
) -> Vec<Note> {
    performed_note_slots(notes, transforms, context)
        .into_iter()
        .flatten()
        .collect()
}

/// Like [`performed_notes`], preserving one optional slot per source note before additions.
/// `None` means a skipped string; zero velocity retains its ordinary soft-note semantics.
/// Freezing uses these slots to preserve source mapping and hidden notes.
pub fn performed_note_slots(
    mut notes: Vec<Note>,
    transforms: &[NoteTransform],
    context: PerformanceContext<'_>,
) -> Vec<Option<Note>> {
    let written_count = notes.len();
    let mut suppressed = std::collections::BTreeSet::new();
    for transform in transforms {
        let scope = match transform {
            NoteTransform::ForDrumVoice { voice, transforms } => {
                Some((voice.as_str(), transforms.as_slice()))
            }
            _ => None,
        };
        if scope.is_some() || !suppressed.is_empty() {
            let indices: Vec<_> = (0..notes.len())
                .filter(|i| {
                    !suppressed.contains(i)
                        && scope.is_none_or(|(voice, _)| notes[*i].drum_voice == voice)
                })
                .collect();
            let active = indices.iter().map(|&i| notes[i].clone()).collect();
            let stack = scope.map_or(std::slice::from_ref(transform), |(_, stack)| stack);
            let mut changed = performed_note_slots(active, stack, context).into_iter();
            for i in indices {
                match changed.next().expect("every source has a slot") {
                    Some(note) => notes[i] = note,
                    None => {
                        suppressed.insert(i);
                    }
                }
            }
            notes.extend(changed.flatten());
            continue;
        }
        match transform {
            NoteTransform::Octaves { above, below } => {
                let count = notes.len();
                for i in 0..count {
                    for (interval, amount) in [(12_i16, *above), (-12, *below)] {
                        let pitch = i16::from(notes[i].pitch) + interval;
                        if amount > 0.0 && (0..=127).contains(&pitch) {
                            let mut copy = notes[i].clone();
                            copy.pitch = pitch as u8;
                            copy.velocity *= amount.clamp(0.0, 1.0);
                            notes.push(copy);
                        }
                    }
                }
            }
            NoteTransform::Expression { settings } => {
                super::expression::express(&mut notes, settings, context)
            }
            NoteTransform::Strum { settings } => {
                suppressed.extend(super::strum::strum(&mut notes, settings, context))
            }
            NoteTransform::Ghost { settings } => {
                super::ghost::ghost_notes(&mut notes, settings, context)
            }
            NoteTransform::ForDrumVoice { .. } => unreachable!(),
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
    for (offset, mut addition) in additions.into_iter().enumerate() {
        if suppressed.contains(&(written_count + offset)) {
            continue;
        }
        let mut occupied: Vec<_> = notes
            .iter()
            .enumerate()
            .filter(|(i, note)| {
                (*i >= written_count || !suppressed.contains(i)) && note.pitch == addition.pitch
            })
            .map(|(_, note)| note)
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
        .into_iter()
        .enumerate()
        .map(|(i, note)| (i >= written_count || !suppressed.contains(&i)).then_some(note))
        .collect()
}

pub(super) fn milliseconds(value: f32, bpm: f64) -> Ticks {
    Ticks(
        (f64::from(value) * Ticks::QUARTER.raw() as f64 * bpm.max(0.0) / 60_000.0)
            .round()
            .max(1.0) as i64,
    )
}

/// Index groups in time and pitch order, independent of the vector's insertion order.
pub(super) fn chords(notes: &[Note]) -> Vec<Vec<usize>> {
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
    let _ = super::strum::strum(
        notes,
        &super::Strum {
            spread_ms,
            direction,
            clock: super::StrumClock::Attacks,
            ..super::Strum::default()
        },
        context,
    );
}
pub(super) fn ornament(source: &Note, pitch: u8, start: Ticks, length: Ticks, gain: f32) -> Note {
    Note {
        velocity: (source.velocity * gain).clamp(0.0, 1.0),
        drum_voice: source.drum_voice.clone(),
        ..Note::new(pitch, start, length)
    }
}

pub(super) fn overlaps(note: &Note, start: Ticks, end: Ticks) -> bool {
    note.start < end && note.end() > start
}

fn mute(notes: &mut Vec<Note>, amount: f32, context: PerformanceContext<'_>) {
    if amount <= 0.0 {
        return;
    }
    let mut added = Vec::new();
    for index in 0..notes.len() {
        let note = &notes[index];
        // Read the held length at this stage, after gate or other earlier articulation.
        let end = note.end().min(context.length);
        let held = end - note.start;
        if held < Ticks(Ticks::QUARTER.raw() / 2) {
            continue;
        }
        let length = milliseconds(12.0, context.bpm).min(held - Ticks(1));
        let start = end - length;
        if notes
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, note)| note)
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
        // Replace the performed tail, leaving both the source and the phrase's end intact.
        notes[index].length = start - notes[index].start;
    }
    notes.extend(added);
}

fn brush(notes: &mut Vec<Note>, amount: f32, context: PerformanceContext<'_>) {
    super::ghost::ghost_notes(
        notes,
        &super::GhostNotes {
            density: 1.0,
            velocity: 0.30 * amount.clamp(0.0, 1.0),
            pattern: super::GhostPattern::Sixteenths,
            preserve_rests: false,
            ..super::GhostNotes::default()
        },
        context,
    );
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
    fn mute_replaces_the_tail_without_overlapping_the_source_or_following_attack() {
        let mut original = n(60, 0, 480);
        original.lyric = "word".into();
        let clip = clip(
            vec![original.clone(), n(64, 0, 480), n(64, 480, 480)],
            vec![NoteTransform::Mute { amount: 1.0 }],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(heard.len(), 6);
        let mute = &heard[3];
        assert_eq!(
            (mute.pitch, mute.start, mute.length),
            (60, Ticks(457), Ticks(23))
        );
        assert!((mute.velocity - 0.28).abs() < 1e-6);
        assert!(mute.lyric.is_empty() && mute.phonemes.is_empty());
        for ((source, tail), original) in heard[..3].iter().zip(&heard[3..]).zip(&clip.notes) {
            assert_eq!(source.end(), tail.start);
            assert_eq!(tail.end(), original.end());
        }
        assert_eq!(
            heard[3].end(),
            heard[2].start,
            "different pitches meet without overlap"
        );
        assert_eq!(
            heard[4].end(),
            heard[2].start,
            "repeated pitches meet without overlap"
        );
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
    fn mute_stays_inside_a_clipped_note_and_keeps_the_loop_boundary() {
        let mut clip = clip(
            vec![n(60, 0, 960)],
            vec![NoteTransform::Mute { amount: 1.0 }],
        );
        clip.length = Ticks(600);
        clip.loop_end = Ticks(1200);
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(
            heard
                .iter()
                .map(|n| (n.start.raw(), n.end().raw()))
                .collect::<Vec<_>>(),
            vec![(0, 577), (577, 600), (600, 1177), (1177, 1200)]
        );
        assert_eq!(clip.notes, vec![n(60, 0, 960)]);
        clip.length = Ticks(479);
        clip.loop_end = Ticks(479);
        assert_eq!(
            clip.sounding_notes(120.0).count(),
            1,
            "a clipped short note gets no mute"
        );
    }

    #[test]
    fn a_conflicting_same_pitch_tail_does_not_shorten_the_original() {
        let source = vec![n(60, 0, 960), n(60, 930, 120)];
        let clip = clip(source.clone(), vec![NoteTransform::Mute { amount: 1.0 }]);
        assert_eq!(clip.sounding_notes(120.0).collect::<Vec<_>>(), source);
    }

    #[test]
    fn brush_uses_only_silent_intervals_and_the_latest_chord() {
        let mut clip = clip(
            vec![n(60, 0, 100), n(64, 0, 100), n(62, 730, 40), n(65, 730, 40)],
            vec![NoteTransform::Brush { amount: 1.0 }],
        );
        clip.length = Ticks(1440);
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(heard.len(), 12);
        assert_eq!(
            heard[4..]
                .iter()
                .map(|n| (n.pitch, n.start.raw()))
                .collect::<Vec<_>>(),
            vec![
                (60, 240),
                (64, 240),
                (60, 480),
                (64, 480),
                (62, 960),
                (65, 960),
                (62, 1200),
                (65, 1200)
            ]
        );
        assert!(
            heard[4..]
                .iter()
                .all(|n| n.length == Ticks(38) && (n.velocity - 0.24).abs() < 1e-6)
        );
    }

    #[test]
    fn mute_requires_at_least_an_eighth_note_at_every_tempo() {
        let original = vec![n(60, 0, 479), n(64, 0, 480), n(67, 0, 481)];
        let clip = clip(original.clone(), vec![NoteTransform::Mute { amount: 1.0 }]);
        for (bpm, duration) in [(60.0, 12), (120.0, 23), (240.0, 46)] {
            let heard: Vec<_> = clip.sounding_notes(bpm).collect();
            assert_eq!(heard[0], original[0]);
            assert_eq!(heard[1].end(), Ticks(480 - duration));
            assert_eq!(heard[2].end(), Ticks(481 - duration));
            assert_eq!(heard.len(), 5);
            assert_eq!(
                heard[3..]
                    .iter()
                    .map(|note| (note.pitch, note.start.raw(), note.length.raw()))
                    .collect::<Vec<_>>(),
                vec![
                    (64, 480 - duration, duration),
                    (67, 481 - duration, duration)
                ]
            );
        }
        assert_eq!(clip.notes, original);
    }

    #[test]
    fn mute_measures_the_held_length_after_gate() {
        let clip = clip(
            vec![n(60, 0, 480), n(64, 0, 960)],
            vec![
                NoteTransform::Gate { amount: 0.5 },
                NoteTransform::Mute { amount: 1.0 },
            ],
        );
        let heard: Vec<_> = clip.sounding_notes(120.0).collect();
        assert_eq!(heard.len(), 3);
        assert_eq!((heard[2].pitch, heard[2].start), (64, Ticks(457)));
        assert_eq!(heard[1].end(), heard[2].start);
        assert_eq!(clip.notes[0].length, Ticks(480));
    }

    #[test]
    fn brush_uses_sixteenths_through_meter_changes_and_offgrid_clip_origins() {
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
        clip.start = Ticks(60);
        clip.length = Ticks(3120);
        let heard: Vec<_> = clip.sounding_notes_with_meter(120.0, signatures).collect();
        assert_eq!(
            heard[1..].iter().map(|n| n.start.raw()).collect::<Vec<_>>(),
            vec![
                180, 420, 660, 900, 1140, 1380, 1620, 1860, 2100, 2340, 2580, 2820, 3060
            ]
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
        let mut first = n(60, 0, 480);
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
        assert_eq!(heard[0].end(), Ticks(457));
        assert_eq!(heard[1], second);
        assert_eq!(clip.notes[0], first);
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
