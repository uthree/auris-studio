//! Interval diagnostics with pitch-bucketed active-note lookup.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use super::{
    Diagnostic, Event, HARSH_OVERLAP, MAX_PER_RULE, MUDDY_CEILING, MUDDY_OVERLAP, Rule, Severity,
    sorted_pitches,
};

/// Reports sustained semitone and low-register second intervals.
pub(super) fn intervals(events: &[Event]) -> Vec<Diagnostic> {
    let mut harsh = Vec::new();
    let mut muddy = Vec::new();
    let mut seen = HashSet::new();
    let mut buckets: Vec<Vec<usize>> = (0..=u8::MAX).map(|_| Vec::new()).collect();
    let mut candidates = Vec::new();
    let mut expiry = BinaryHeap::new();
    let mut alive = vec![false; events.len()];

    for (original_index, event) in events.iter().enumerate().filter(|(_, event)| !event.drum) {
        if harsh.len() >= MAX_PER_RULE && muddy.len() >= MAX_PER_RULE {
            break;
        }
        while let Some(&Reverse((end, index))) = expiry.peek() {
            if end > event.start.raw() {
                break;
            }
            expiry.pop();
            alive[index] = false;
        }
        candidates.clear();
        for pitch in nearby_pitches(event.pitch) {
            let distance = event.pitch.abs_diff(pitch);
            let can_be_harsh = harsh.len() < MAX_PER_RULE && distance == 1;
            let can_be_muddy = muddy.len() < MAX_PER_RULE
                && (1..=2).contains(&distance)
                && event.pitch.max(pitch) <= MUDDY_CEILING;
            if !can_be_harsh && !can_be_muddy {
                continue;
            }
            let bucket = &mut buckets[pitch as usize];
            bucket.retain(|candidate| alive[*candidate]);
            candidates.extend(bucket.iter().copied());
        }
        // Buckets are pitch-partitioned; sorting restores the original active-note encounter order
        // exactly, which is observable when the per-rule cap or `seen` deduplication is reached.
        candidates.sort_unstable();
        for &other_index in &candidates {
            let other = &events[other_index];
            let distance = event.pitch.abs_diff(other.pitch);
            let overlap = event.end.min(other.end) - event.start;
            let rule = if distance == 1 && overlap >= HARSH_OVERLAP {
                Rule::HarshInterval
            } else if (1..=2).contains(&distance)
                && event.pitch.max(other.pitch) <= MUDDY_CEILING
                && overlap >= MUDDY_OVERLAP
            {
                Rule::MuddyLowInterval
            } else {
                continue;
            };
            if (rule == Rule::HarshInterval && harsh.len() >= MAX_PER_RULE)
                || (rule == Rule::MuddyLowInterval && muddy.len() >= MAX_PER_RULE)
            {
                continue;
            }
            let pair = if other.note < event.note {
                (other.note, event.note)
            } else {
                (event.note, other.note)
            };
            if !seen.insert((rule, pair)) {
                continue;
            }
            let bucket = match rule {
                Rule::HarshInterval => &mut harsh,
                Rule::MuddyLowInterval => &mut muddy,
                _ => unreachable!(),
            };
            let mut diagnostic =
                Diagnostic::new(rule, Severity::Warning, event.start, event.start + overlap);
            diagnostic.track = (event.track == other.track).then_some(event.track);
            diagnostic.notes = vec![pair.0, pair.1];
            diagnostic.pitches = sorted_pitches([event.pitch, other.pitch].into_iter());
            bucket.push(diagnostic);
        }
        buckets[event.pitch as usize].push(original_index);
        alive[original_index] = true;
        expiry.push(Reverse((event.end.raw(), original_index)));
    }
    harsh.extend(muddy);
    harsh
}

fn nearby_pitches(pitch: u8) -> impl Iterator<Item = u8> {
    [
        pitch.checked_sub(2),
        pitch.checked_sub(1),
        pitch.checked_add(1),
        pitch.checked_add(2),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
/// The former active-list implementation, retained as a differential-test reference.
fn reference_intervals(events: &[Event]) -> Vec<Diagnostic> {
    let mut harsh = Vec::new();
    let mut muddy = Vec::new();
    let mut seen = HashSet::new();
    let mut active: Vec<&Event> = Vec::new();
    for event in events.iter().filter(|event| !event.drum) {
        active.retain(|other| other.end > event.start);
        for other in &active {
            let distance = event.pitch.abs_diff(other.pitch);
            let overlap = event.end.min(other.end) - event.start;
            let rule = if distance == 1 && overlap >= HARSH_OVERLAP {
                Rule::HarshInterval
            } else if (1..=2).contains(&distance)
                && event.pitch.max(other.pitch) <= MUDDY_CEILING
                && overlap >= MUDDY_OVERLAP
            {
                Rule::MuddyLowInterval
            } else {
                continue;
            };
            let pair = if other.note < event.note {
                (other.note, event.note)
            } else {
                (event.note, other.note)
            };
            if !seen.insert((rule, pair)) {
                continue;
            }
            let bucket = match rule {
                Rule::HarshInterval => &mut harsh,
                _ => &mut muddy,
            };
            if bucket.len() >= MAX_PER_RULE {
                continue;
            }
            let mut diagnostic =
                Diagnostic::new(rule, Severity::Warning, event.start, event.start + overlap);
            diagnostic.track = (event.track == other.track).then_some(event.track);
            diagnostic.notes = vec![pair.0, pair.1];
            diagnostic.pitches = sorted_pitches([event.pitch, other.pitch].into_iter());
            bucket.push(diagnostic);
        }
        active.push(event);
    }
    harsh.extend(muddy);
    harsh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::NoteRef;
    use auris_core::project::{ClipId, TrackId};
    use auris_core::time::Ticks;

    fn event(index: usize, pitch: u8, start: i64, end: i64) -> Event {
        Event {
            track: TrackId(1),
            note: NoteRef {
                clip: ClipId(1),
                index,
            },
            pitch,
            start: Ticks(start),
            end: Ticks(end),
            drum: false,
        }
    }

    #[test]
    fn sorted_bucketed_intervals_match_the_reference() {
        let events = vec![
            event(0, 40, 0, 10_000),
            event(1, 41, 0, 10_000),
            event(2, 42, 0, 10_000),
            event(3, 70, 20_000, 30_000),
        ];
        assert_eq!(intervals(&events), reference_intervals(&events));
    }

    #[test]
    fn unsorted_prefix_uses_the_legacy_active_order() {
        let events = vec![event(0, 40, 100, 1_000), event(1, 41, 0, 1_000)];
        assert_eq!(intervals(&events), reference_intervals(&events));
    }

    #[test]
    fn dense_caps_preserve_first_diagnostics() {
        let events: Vec<_> = (0..320)
            .map(|index| {
                event(
                    index,
                    if index % 2 == 0 { 40 } else { 41 },
                    index as i64,
                    10_000,
                )
            })
            .collect();
        assert_eq!(intervals(&events), reference_intervals(&events));
        assert!(intervals(&events).len() <= MAX_PER_RULE * 2);
    }

    #[test]
    fn a_full_harsh_cap_still_checks_low_muddy_seconds() {
        let mut events = Vec::new();
        for index in 0..210 {
            events.push(event(
                index * 2,
                80,
                index as i64 * 3,
                index as i64 * 3 + 5_000,
            ));
            events.push(event(
                index * 2 + 1,
                81,
                index as i64 * 3,
                index as i64 * 3 + 5_000,
            ));
        }
        let end = 1_000 + MUDDY_OVERLAP.raw();
        events.push(event(10_000, 40, 1_000, end));
        events.push(event(10_001, 41, 1_000, end));
        let actual = intervals(&events);
        assert_eq!(actual, reference_intervals(&events));
        assert!(
            actual
                .iter()
                .any(|diagnostic| diagnostic.rule == Rule::MuddyLowInterval)
        );
    }

    #[test]
    fn randomized_sorted_and_unsorted_inputs_match_the_reference() {
        let mut seed = 0x9e37_79b9_u64;
        for unsorted in [false, true] {
            let mut events = Vec::with_capacity(512);
            for index in 0..512 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let start = (seed >> 16) as i64 % 20_000;
                seed = seed.rotate_left(13);
                let pitch = 24 + ((seed >> 24) % 88) as u8;
                let length = 200 + ((seed >> 32) % 8_000) as i64;
                events.push(event(index, pitch, start, start + length));
            }
            if !unsorted {
                events.sort_by_key(|event| event.start);
            }
            assert_eq!(intervals(&events), reference_intervals(&events));
        }
    }

    #[test]
    #[ignore = "release-only interval benchmark"]
    fn benchmark_intervals_old_and_bucketed_nine_alternating_trials() {
        use std::hint::black_box;
        use std::time::Instant;

        fn median(values: &mut [u128]) -> u128 {
            values.sort_unstable();
            values[values.len() / 2]
        }
        fn fixture(kind: usize, count: usize) -> Vec<Event> {
            let mut seed = 0x1234_5678_u64 ^ kind as u64;
            (0..count)
                .map(|index| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    let start = index as i64 * 3;
                    let pitch = match kind {
                        0 => 48,
                        1 => 48 + (index % 2) as u8,
                        2 => 36 + (index % 3) as u8,
                        _ => 24 + ((seed >> 24) % 96) as u8,
                    };
                    event(index, pitch, start, start + 20_000)
                })
                .collect()
        }

        for (name, kind, count) in [
            ("small-dissonant", 1, 32),
            ("large-unison", 0, 4_096),
            ("dissonant", 1, 2_048),
            ("bass", 2, 4_096),
            ("random-mixed", 3, 2_048),
        ] {
            let events = fixture(kind, count);
            assert_eq!(intervals(&events), reference_intervals(&events));
            let mut old = Vec::with_capacity(9);
            let mut optimized = Vec::with_capacity(9);
            for trial in 0..9 {
                if trial % 2 == 0 {
                    let started = Instant::now();
                    black_box(reference_intervals(black_box(&events)));
                    old.push(started.elapsed().as_nanos());
                    let started = Instant::now();
                    black_box(intervals(black_box(&events)));
                    optimized.push(started.elapsed().as_nanos());
                } else {
                    let started = Instant::now();
                    black_box(intervals(black_box(&events)));
                    optimized.push(started.elapsed().as_nanos());
                    let started = Instant::now();
                    black_box(reference_intervals(black_box(&events)));
                    old.push(started.elapsed().as_nanos());
                }
            }
            eprintln!(
                "intervals {name}: old={}ns bucketed={}ns",
                median(&mut old),
                median(&mut optimized)
            );
        }
    }
}
