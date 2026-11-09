use super::*;

use std::hint::black_box;

use auris_core::project::Project;

fn reference_alone_flags(events: &[&super::super::score::Event]) -> Vec<bool> {
    events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            events.iter().enumerate().all(|(other, candidate)| {
                other == index || candidate.end <= event.start || candidate.start >= event.end
            })
        })
        .collect()
}

fn reference_pitch_notes(
    project: &Project,
    events: &[super::super::score::Event],
    track: auris_core::project::TrackId,
    rate: f64,
) -> Vec<PitchNote> {
    let Some(kind) = project.track(track).map(|track| &track.kind) else {
        return Vec::new();
    };
    let singer = matches!(kind, auris_core::project::TrackKind::Singer(_));
    if !singer && !matches!(kind, auris_core::project::TrackKind::Instrument(_)) {
        return Vec::new();
    }
    let own: Vec<_> = events
        .iter()
        .filter(|event| event.track == track && !event.drum)
        .collect();
    let tempo = &project.tempo_map;
    let mut notes = Vec::new();
    let mut seconds = 0.0;
    for (index, event) in own.iter().enumerate() {
        let alone = own.iter().enumerate().all(|(other, candidate)| {
            other == index || candidate.end <= event.start || candidate.start >= event.end
        });
        if !alone || !(PITCH_NOTES.0..=PITCH_NOTES.1).contains(&event.pitch) {
            continue;
        }
        let from = tempo.ticks_to_seconds(event.start).0;
        let to = tempo.ticks_to_seconds(event.end).0;
        if to - from < PITCH_SHORTEST {
            continue;
        }
        seconds += to - from;
        if notes.len() >= PITCH_MAX_NOTES || seconds > PITCH_MAX_SECONDS {
            break;
        }
        notes.push(PitchNote {
            note: event.note,
            pitch: event.pitch,
            start: event.start,
            end: event.end,
            frames: ((from * rate) as usize, (to * rate) as usize),
            singer,
        });
    }
    notes
}

fn event(
    track: auris_core::project::TrackId,
    index: usize,
    pitch: u8,
    start: i64,
    end: i64,
) -> super::super::score::Event {
    super::super::score::Event {
        track,
        note: super::super::NoteRef {
            clip: auris_core::project::ClipId(1),
            index,
        },
        pitch,
        start: auris_core::time::Ticks(start),
        end: auris_core::time::Ticks(end),
        drum: false,
    }
}

fn reference_checked_tracks(
    project: &Project,
    events: &[super::super::score::Event],
) -> Vec<CheckedTrack> {
    let mut tracks = Vec::new();
    for track in &project.tracks {
        if track.mixer.mute {
            continue;
        }
        if matches!(&track.kind, auris_core::project::TrackKind::Singer(singer) if singer.take.is_none())
        {
            continue;
        }
        let mut spans: Vec<(auris_core::time::Ticks, auris_core::time::Ticks)> = Vec::new();
        for event in events.iter().filter(|event| event.track == track.id) {
            match spans.last_mut() {
                Some(last) if event.start <= last.1 => last.1 = last.1.max(event.end),
                _ => spans.push((event.start, event.end)),
            }
        }
        if !spans.is_empty() {
            tracks.push((track.id, track.mixer.gain_db, spans));
        }
        if tracks.len() >= MAX_TRACK_PASSES {
            break;
        }
    }
    tracks
}

fn assert_pitch_matches_reference(
    project: &Project,
    events: &[super::super::score::Event],
    track: auris_core::project::TrackId,
) {
    let expected = reference_pitch_notes(project, events, track, AUDIO_LINT_RATE);
    let actual = pitch_notes(project, events, track, AUDIO_LINT_RATE);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual.note, expected.note);
        assert_eq!(actual.pitch, expected.pitch);
        assert_eq!(actual.start, expected.start);
        assert_eq!(actual.end, expected.end);
        assert_eq!(actual.frames, expected.frames);
        assert_eq!(actual.singer, expected.singer);
    }
}

#[test]
fn alone_flags_match_pairwise_reference_for_unsorted_and_degenerate_spans() {
    let track = auris_core::project::TrackId(1);
    let events = [
        event(track, 0, 60, 100, 200),
        event(track, 1, 62, 0, 0),
        event(track, 2, 64, 150, 120),
        event(track, 3, 65, -20, 50),
        event(track, 4, 67, 50, 100),
        event(track, 5, 69, 200, 250),
    ];
    let refs: Vec<_> = events.iter().collect();
    assert_eq!(alone_flags(&refs), reference_alone_flags(&refs));
}

#[test]
fn pitch_notes_match_original_pairwise_selection_for_unsorted_events() {
    let mut project = Project::new("pitch selection", AUDIO_LINT_RATE);
    let track = project.add_instrument_track("Lead", "test.instrument");
    let events = vec![
        event(track, 0, 60, 1_920, 3_840),
        event(track, 1, 90, 0, 1_920),
        event(track, 2, 62, 3_840, 5_760),
        event(track, 3, 63, 5_760, 5_760),
        event(track, 4, 64, 7_680, 9_600),
    ];
    assert_pitch_matches_reference(&project, &events, track);
}

#[test]
fn pitch_notes_match_pairwise_reference_for_many_disjoint_events() {
    let mut project = Project::new("pitch selection", AUDIO_LINT_RATE);
    let track = project.add_instrument_track("Lead", "test.instrument");
    let events: Vec<_> = (0..320)
        .map(|index| {
            let start = index as i64 * 960;
            event(track, index, 48 + (index % 24) as u8, start, start + 480)
        })
        .collect();
    assert_pitch_matches_reference(&project, &events, track);
}

#[test]
fn randomized_sorted_unsorted_and_duplicate_events_match_pairwise_reference() {
    let mut project = Project::new("random pitch selection", AUDIO_LINT_RATE);
    let track = project.add_instrument_track("Lead", "test.instrument");
    let mut state = 0x1234_5678_u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let events: Vec<_> = (0..420)
        .map(|index| {
            let start = (next() % 25_000) as i64 - 2_000;
            let span = (next() % 1_601) as i64 - 400;
            let pitch = (next() % 121) as u8;
            event(track, index, pitch, start, start + span)
        })
        .collect();
    let mut sorted = events.clone();
    sorted.sort_unstable_by_key(|event| (event.start, event.pitch));
    let mut unsorted = sorted.clone();
    for index in (1..unsorted.len()).rev() {
        unsorted.swap(index, (next() as usize) % (index + 1));
    }
    for input in [&events, &sorted, &unsorted] {
        let refs: Vec<_> = input.iter().collect();
        assert_eq!(alone_flags(&refs), reference_alone_flags(&refs));
        assert_pitch_matches_reference(&project, input, track);
    }
    let duplicated = vec![events[0], events[0], events[1], events[1], events[2]];
    let refs: Vec<_> = duplicated.iter().collect();
    assert_eq!(alone_flags(&refs), reference_alone_flags(&refs));
    assert_pitch_matches_reference(&project, &duplicated, track);
}

#[test]
fn sorted_positive_spans_match_pairwise_reference_with_long_held_notes() {
    let track = auris_core::project::TrackId(1);
    let events: Vec<_> = (0..800)
        .map(|index| {
            let start = index as i64 * 240;
            let length = if index % 137 == 0 { 10_000 } else { 120 };
            event(track, index, 60, start, start + length)
        })
        .collect();
    let refs: Vec<_> = events.iter().collect();
    assert_eq!(alone_flags(&refs), reference_alone_flags(&refs));
}

#[test]
fn checked_tracks_reuses_supplied_events_without_changing_selection() {
    let mut project = Project::new("checked tracks", AUDIO_LINT_RATE);
    let first = project.add_instrument_track("First", "test.instrument");
    let second = project.add_instrument_track("Second", "test.instrument");
    let events = vec![
        event(first, 0, 60, 0, 960),
        event(second, 1, 64, 960, 1_920),
    ];
    let actual = checked_tracks(&project, &events);
    assert_eq!(actual, reference_checked_tracks(&project, &events));
}

#[derive(Clone, Copy)]
enum BenchmarkShape {
    Monophonic,
    OutOfRangeUntilTheEnd,
    DenseOverlap,
}

fn benchmark_pitch_events(
    track: auris_core::project::TrackId,
    count: usize,
    shape: BenchmarkShape,
) -> Vec<super::super::score::Event> {
    (0..count)
        .map(|index| {
            let (start, end, pitch) = match shape {
                BenchmarkShape::Monophonic => {
                    let start = index as i64 * 960;
                    (start, start + 480, 60 + (index % 12) as u8)
                }
                BenchmarkShape::OutOfRangeUntilTheEnd => {
                    let start = index as i64 * 960;
                    let pitch = if index + 160 < count { 20 } else { 60 };
                    (start, start + 480, pitch)
                }
                BenchmarkShape::DenseOverlap => (0, 960, 60),
            };
            event(track, index, pitch, start, end)
        })
        .collect()
}

fn median_duration(values: &mut [std::time::Duration]) -> std::time::Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn benchmark_pitch_notes(count: usize, label: &str, shape: BenchmarkShape) {
    let mut project = Project::new(label, AUDIO_LINT_RATE);
    let track = project.add_instrument_track("Benchmark", "test.instrument");
    let events = benchmark_pitch_events(track, count, shape);
    let expected = reference_pitch_notes(&project, &events, track, AUDIO_LINT_RATE);
    let actual = pitch_notes(&project, &events, track, AUDIO_LINT_RATE);
    assert_eq!(actual.len(), expected.len());
    assert!(actual.iter().zip(&expected).all(|(actual, expected)| {
        actual.note == expected.note
            && actual.pitch == expected.pitch
            && actual.start == expected.start
            && actual.end == expected.end
            && actual.frames == expected.frames
            && actual.singer == expected.singer
    }));

    let mut reference = Vec::with_capacity(9);
    let mut optimized = Vec::with_capacity(9);
    for trial in 0..9 {
        let first_reference = trial % 2 == 0;
        for reference_first in [first_reference, !first_reference] {
            let start = std::time::Instant::now();
            let result = if reference_first {
                reference_pitch_notes(&project, &events, track, AUDIO_LINT_RATE)
            } else {
                pitch_notes(&project, &events, track, AUDIO_LINT_RATE)
            };
            let elapsed = start.elapsed();
            black_box(result);
            if reference_first {
                reference.push(elapsed);
            } else {
                optimized.push(elapsed);
            }
        }
    }
    eprintln!(
        "audio lint pitch benchmark {label}: reference {:?}, optimized {:?}",
        median_duration(&mut reference),
        median_duration(&mut optimized),
    );
}

#[test]
#[ignore = "release-only performance measurement"]
fn pitch_notes_small_benchmark() {
    benchmark_pitch_notes(160, "small-monophonic", BenchmarkShape::Monophonic);
}

#[test]
#[ignore = "release-only performance measurement"]
fn pitch_notes_large_monophonic_benchmark() {
    benchmark_pitch_notes(10_000, "large-monophonic", BenchmarkShape::Monophonic);
}

#[test]
#[ignore = "release-only performance measurement"]
fn pitch_notes_large_benchmark() {
    benchmark_pitch_notes(
        10_000,
        "large-out-of-range-until-end",
        BenchmarkShape::OutOfRangeUntilTheEnd,
    );
}

#[test]
#[ignore = "release-only performance measurement"]
fn pitch_notes_dense_overlap_benchmark() {
    benchmark_pitch_notes(10_000, "large-dense-overlap", BenchmarkShape::DenseOverlap);
}
