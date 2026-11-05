//! Differential and release-only performance coverage for bulk note deletion.

use super::*;
use crate::{Session, SessionOptions};
use auris_core::time::Ticks;
use std::time::{Duration, Instant};

fn old_remove(mut notes: Vec<Note>, doomed: &[usize]) -> Vec<Note> {
    for index in doomed.iter().copied().rev() {
        if index < notes.len() {
            notes.remove(index);
        }
    }
    notes
}

fn optimized_remove(mut notes: Vec<Note>, doomed: &[usize]) -> Vec<Note> {
    remove_note_indices(&mut notes, doomed);
    notes
}

fn notes(count: usize) -> Vec<Note> {
    (0..count)
        .map(|index| Note::new((index % 128) as u8, Ticks(index as i64), Ticks(1)))
        .collect()
}

fn selection(count: usize, step: usize) -> Vec<usize> {
    (0..count).step_by(step).collect()
}

#[test]
fn every_small_selection_preserves_the_exact_survivors() {
    let base = notes(12);
    for bits in 0_u32..1 << base.len() {
        let doomed: Vec<_> = (0..base.len())
            .filter(|index| bits & (1 << index) != 0)
            .collect();
        assert_eq!(
            old_remove(base.clone(), &doomed),
            optimized_remove(base.clone(), &doomed)
        );
    }
}

#[test]
fn fast_delete_matches_reverse_remove_for_unsorted_duplicate_and_invalid_indices() {
    let base = notes(128);
    let mut doomed = vec![127, 2, 2, 0, 128, usize::MAX, 7];
    doomed.sort_unstable();
    doomed.dedup();
    doomed.retain(|index| *index < base.len());
    assert_eq!(
        old_remove(base.clone(), &doomed),
        optimized_remove(base, &doomed)
    );
}

#[test]
fn session_bulk_delete_undo_restores_the_exact_clip() {
    let mut session = Session::new(SessionOptions::headless()).expect("session");
    let track = session
        .add_default_instrument_track("Delete benchmark")
        .expect("track");
    let clip = session
        .add_midi_clip(track, "Notes", Ticks::ZERO, Ticks(10_000))
        .expect("clip");
    for index in 0..32 {
        session
            .add_note(clip, Note::new((index % 128) as u8, Ticks(index), Ticks(1)))
            .expect("note");
    }
    session.forget_history();
    let before = session.midi_clip(clip).expect("clip").notes.clone();
    session
        .remove_notes(clip, &[31, 3, 3, 0, 99, 12])
        .expect("delete");
    let after = old_remove(before.clone(), &[0, 3, 12, 31]);
    assert_eq!(session.midi_clip(clip).expect("clip").notes, after);
    assert!(session.undo().is_some());
    assert_eq!(session.midi_clip(clip).expect("clip").notes, before);
    assert!(session.redo().is_some());
    assert_eq!(session.midi_clip(clip).expect("clip").notes, after);
}

#[test]
fn stale_and_empty_deletions_are_noops() {
    let mut session = Session::new(SessionOptions::headless()).expect("session");
    let track = session
        .add_default_instrument_track("Delete no-op")
        .expect("track");
    let clip = session
        .add_midi_clip(track, "Notes", Ticks::ZERO, Ticks(1_000))
        .expect("clip");
    session
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(1)))
        .expect("note");
    session.forget_history();
    session.remove_notes(clip, &[]).expect("empty selection");
    session
        .remove_notes(clip, &[99, usize::MAX])
        .expect("stale selection");
    assert!(!session.can_undo());
    assert!(session.remove_notes(ClipId(99_999), &[0]).is_err());
    assert!(!session.can_undo());
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

#[test]
#[ignore = "release performance comparison"]
fn benchmark_bulk_delete_reference_and_retain() {
    for (count, step) in [
        (128, 100),
        (50_000, 2),
        (100_000, 100),
        (100_000, 2),
        (100_000, 1),
    ] {
        let base = notes(count);
        let doomed = selection(count, step);
        assert_eq!(
            old_remove(base.clone(), &doomed),
            optimized_remove(base.clone(), &doomed)
        );
        let mut old = Vec::with_capacity(9);
        let mut fast = Vec::with_capacity(9);
        for trial in 0..9 {
            let old_input = base.clone();
            let fast_input = base.clone();
            let old_first = trial % 2 == 0;
            let mut measure = |reference: bool, input: Vec<Note>| {
                let start = Instant::now();
                let result = if reference {
                    old_remove(input, &doomed)
                } else {
                    optimized_remove(input, &doomed)
                };
                let elapsed = start.elapsed();
                std::hint::black_box((result.len(), result.first()));
                if reference {
                    old.push(elapsed);
                } else {
                    fast.push(elapsed);
                }
            };
            if old_first {
                measure(true, old_input);
                measure(false, fast_input);
            } else {
                measure(false, fast_input);
                measure(true, old_input);
            }
        }
        old.sort_unstable();
        fast.sort_unstable();
        eprintln!(
            "bulk delete {count} notes step {step}: reverse {:?}, retain {:?}",
            median(old),
            median(fast)
        );
    }
}
