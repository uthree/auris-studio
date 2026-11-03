//! Differential and stress coverage for the missing-chord rule.
//!
//! The reference below deliberately retains the pre-optimization nested scans. It is independent
//! of the implementation under test, so a faster sweep must preserve every diagnostic exactly.

use super::*;
use auris_core::project::TrackId;
use auris_core::time::{Ticks, TimeSignature};
use std::time::Instant;

fn original_missing_chords(project: &Project, events: &[Event]) -> Vec<Diagnostic> {
    let signatures = &project.signatures;
    let chords = &project.harmony.chords;
    let pitched: Vec<&Event> = events.iter().filter(|event| !event.drum).collect();
    let Some(last) = pitched.iter().map(|event| event.end).max() else {
        return Vec::new();
    };
    let last_bar = signatures.bar_of(last - Ticks(1));
    let mut bars: Vec<(u32, Option<Chord>)> = Vec::new();
    let mut first = 0;
    for bar in 1..=last_bar {
        let start = signatures.bar_start(bar);
        let end = signatures.bar_start(bar + 1);
        let written = chords.numeral_at(start).is_some()
            || chords
                .points()
                .iter()
                .any(|point| point.tick > start && point.tick < end && point.chord.is_some());
        while first < pitched.len() && pitched[first].start < start && pitched[first].end <= start {
            first += 1;
        }
        if written {
            continue;
        }
        let inside: Vec<&&Event> = pitched[first..]
            .iter()
            .take_while(|event| event.start < end)
            .filter(|event| event.end > start)
            .collect();
        let chordal = inside.iter().any(|onset| {
            let at = onset.start.max(start);
            let mut classes = [false; 12];
            for event in &inside {
                if event.start <= at && event.end > at {
                    classes[(event.pitch % 12) as usize] = true;
                }
            }
            classes.iter().filter(|sounding| **sounding).count() >= CHORD_TONES
        });
        if !chordal {
            continue;
        }
        let mut weights = [0i64; 12];
        for event in &inside {
            weights[(event.pitch % 12) as usize] +=
                (event.end.min(end) - event.start.max(start)).raw();
        }
        bars.push((bar, obvious_chord(&weights)));
    }
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < bars.len() {
        let obvious = bars[index].1.is_some();
        let mut stop = index + 1;
        while stop < bars.len()
            && bars[stop].0 == bars[stop - 1].0 + 1
            && bars[stop].1.is_some() == obvious
        {
            stop += 1;
        }
        let run = &bars[index..stop];
        let start = signatures.bar_start(run[0].0);
        let end = signatures.bar_start(run[run.len() - 1].0 + 1);
        let mut diagnostic = Diagnostic::new(Rule::MissingChord, Severity::Info, start, end);
        diagnostic.value = Some(run.len() as f32);
        if obvious {
            let harmony = &project.harmony;
            diagnostic.fix = Some(Fix::SetChords {
                chords: run
                    .iter()
                    .filter_map(|(bar, chord)| {
                        let at = signatures.bar_start(*bar);
                        Some((at, Numeral::from_chord_in((*chord)?, harmony.key_at(at))))
                    })
                    .collect(),
                end,
            });
        }
        diagnostics.push(diagnostic);
        index = stop;
    }
    diagnostics
}

fn event(index: usize, pitch: u8, start: i64, end: i64, drum: bool) -> Event {
    Event {
        track: TrackId(if drum { 2 } else { 1 }),
        note: NoteRef {
            clip: auris_core::project::ClipId(1),
            index,
        },
        pitch,
        start: Ticks(start),
        end: Ticks(end),
        drum,
    }
}

fn project() -> Project {
    Project::new("missing chord differential", 48_000.0)
}

fn deterministic_events(count: usize) -> Vec<Event> {
    let mut state = 0x9e37_79b9u32;
    let mut events = Vec::with_capacity(count);
    for index in 0..count {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let start = (state % 24_000) as i64 - 1_000;
        state = state.rotate_left(7);
        let length = (state % 4_000) as i64;
        events.push(event(
            index,
            (state % 96) as u8,
            start,
            start + length,
            false,
        ));
    }
    events
}

fn assert_same(project: &Project, mut events: Vec<Event>) {
    let expected = original_missing_chords(project, &events);
    let actual = missing_chords(project, &events);
    assert_eq!(expected, actual, "event count {}", events.len());
    events.reverse();
    // The production caller supplies sorted events, but this guards the documented helper's
    // behavior against accidental assumptions while the optimized implementation is developed.
    assert_eq!(
        original_missing_chords(project, &events),
        missing_chords(project, &events)
    );
}

#[test]
fn missing_chords_differential_edge_cases() {
    assert_same(&project(), Vec::new());
    let mut drums = project();
    drums
        .signatures
        .set_point(Ticks(3_840 * 2), TimeSignature::new(3, 4));
    assert_same(
        &drums,
        vec![
            event(0, 36, 0, 3_840, true),
            event(1, 38, 0, 3_840, true),
            event(2, 42, 0, 3_840, true),
        ],
    );
    assert_same(
        &project(),
        vec![
            event(0, 60, 0, 3_840, false),
            event(1, 64, 0, 3_840, false),
            event(2, 67, 0, 3_840, false),
        ],
    );
    assert_same(
        &project(),
        vec![
            event(0, 60, 0, 1_920, false),
            event(1, 64, 1_920, 3_840, false),
            event(2, 67, 0, 3_840, false),
        ],
    );
}

#[test]
fn missing_chords_differential_dense_and_limit_cases() {
    let project = project();
    let mut dense = Vec::new();
    for index in 0..1_000 {
        dense.push(event(index, 60 + (index % 3) as u8, 0, 3_840, false));
    }
    assert_same(&project, dense);

    let mut many_bars = Vec::new();
    for bar in 0..128 {
        let start = bar * 3_840;
        for (offset, pitch) in [(0, 60), (1, 64), (2, 67)] {
            many_bars.push(event(many_bars.len(), pitch, start, start + 3_840, false));
            let _ = offset;
        }
    }
    assert_same(&project, many_bars);

    // Include zero-length and negative-start spans, and both the sorted caller order and the
    // unsorted order that can survive the MAX_EVENTS early return in score::events.
    let random = deterministic_events(2_000);
    let mut sorted = random.clone();
    sorted.sort_by_key(|event| (event.start, event.pitch));
    assert_same(&project, sorted);
    assert_same(&project, random);
}

#[test]
fn missing_chords_differential_time_signatures_and_chord_annotations() {
    let mut project = project();
    project
        .signatures
        .set_point(Ticks(3_840 * 2), TimeSignature::new(3, 4));
    project
        .harmony
        .chords
        .set_point(Ticks::ZERO, Some(Numeral::parse("I").expect("numeral")));
    project.harmony.chords.set_point(
        Ticks(3_840 + 120),
        Some(Numeral::parse("IV").expect("numeral")),
    );
    project.harmony.chords.set_point(Ticks(3_840 * 4), None);
    let events = vec![
        event(0, 60, 0, 3_840, false),
        event(1, 64, 0, 3_840, false),
        event(2, 67, 0, 3_840, false),
        event(3, 62, 3_840, 7_680, false),
        event(4, 65, 3_840, 7_680, false),
        event(5, 69, 3_840, 7_680, false),
        event(6, 59, 7_680, 11_520, false),
        event(7, 63, 7_680, 11_520, false),
        event(8, 66, 7_680, 11_520, false),
    ];
    assert_same(&project, events);
}

#[test]
#[ignore = "release performance comparison"]
fn benchmark_missing_chords_reference_and_optimized() {
    fn pair(name: &str, project: &Project, events: &[Event]) {
        assert_eq!(
            original_missing_chords(project, events),
            missing_chords(project, events),
            "reference mismatch in {name}"
        );
        // Warm instruction/data caches before collecting the alternating nine-pair sample.
        std::hint::black_box(original_missing_chords(project, events));
        std::hint::black_box(missing_chords(project, events));
        let mut old = Vec::new();
        let mut new = Vec::new();
        for trial in 0..9 {
            let old_first = trial % 2 == 0;
            for reference in [old_first, !old_first] {
                let start = Instant::now();
                let result = if reference {
                    original_missing_chords(project, events)
                } else {
                    missing_chords(project, events)
                };
                let elapsed = start.elapsed();
                std::hint::black_box(result);
                if reference {
                    old.push(elapsed);
                } else {
                    new.push(elapsed);
                }
            }
        }
        old.sort_unstable();
        new.sort_unstable();
        eprintln!(
            "missing_chords {name}: reference {:?}, optimized {:?}",
            old[old.len() / 2],
            new[new.len() / 2]
        );
    }

    let project = project();
    let small = (0..32)
        .flat_map(|index| {
            let start = index as i64 * 3_840;
            [60, 64, 67]
                .into_iter()
                .enumerate()
                .map(move |(offset, pitch)| {
                    event(index * 3 + offset, pitch, start, start + 3_840, false)
                })
        })
        .collect::<Vec<_>>();
    pair("small-32-triads", &project, &small);

    let dense_chordal = (0..10_000)
        .map(|index| event(index, [60, 64, 67][index % 3], 0, 3_840, false))
        .collect::<Vec<_>>();
    pair("large-10000-chordal", &project, &dense_chordal);

    let dense_unison = (0..10_000)
        .map(|index| event(index, 60, 0, 3_840, false))
        .collect::<Vec<_>>();
    pair("large-10000-unison", &project, &dense_unison);

    let sequential = (0..10_000)
        .map(|index| {
            // Three classes occur across the bar, but only two ever sound at once. This forces
            // boundary processing instead of the one/two-class shortcut or first-onset exit.
            let start = index as i64 / 2;
            event(index, 60 + (index % 3) as u8, start, start + 1, false)
        })
        .collect::<Vec<_>>();
    pair("sequential-arpeggio", &project, &sequential);

    let mut pedal = vec![event(0, 48, 0, 128 * 3_840, false)];
    for bar in 0..128 {
        let start = bar * 3_840;
        let base = pedal.len();
        pedal.extend([
            event(base, 60, start, start + 3_840, false),
            event(base + 1, 64, start, start + 3_840, false),
        ]);
    }
    pair("long-pedal-expired-notes", &project, &pedal);
}
