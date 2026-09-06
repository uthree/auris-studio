//! The background queue contains sound identities, never edits to the document.

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use auris_session::{DrumKitAnalysis, prelude::TrackId};

pub(super) const DEBOUNCE: Duration = Duration::from_millis(800);
pub(super) const CHECK_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct DrumAnalysisState {
    entries: BTreeMap<TrackId, Entry>,
    active: Option<Job>,
    pub(super) checked_at: Option<Instant>,
}

struct Entry {
    source: u64,
    due: Instant,
    outcome: Outcome,
}

pub(super) enum Outcome {
    Pending,
    Running,
    Ready(Box<DrumKitAnalysis>),
    Failed(String),
    Cancelled,
}

pub(super) struct Job {
    pub(super) track: TrackId,
    pub(super) source: u64,
    pub(super) cancel: Arc<AtomicBool>,
}

impl DrumAnalysisState {
    /// Observes cheap sound keys; repeated observations leave attempts and results intact.
    pub(super) fn observe(&mut self, sources: BTreeMap<TrackId, u64>, now: Instant) {
        self.entries.retain(|track, _| sources.contains_key(track));
        for (track, source) in sources {
            if self
                .entries
                .get(&track)
                .is_none_or(|entry| entry.source != source)
            {
                self.entries.insert(
                    track,
                    Entry {
                        source,
                        due: now + DEBOUNCE,
                        outcome: Outcome::Pending,
                    },
                );
            }
        }
        if let Some(job) = &self.active
            && self
                .entries
                .get(&job.track)
                .is_none_or(|entry| entry.source != job.source)
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.checked_at = Some(now);
    }

    /// A cancelled worker still occupies the slot until its completion kills and reaps it.
    pub(super) fn start_next(&mut self, now: Instant) -> Option<Job> {
        if self.active.is_some() {
            return None;
        }
        let (&track, entry) = self
            .entries
            .iter_mut()
            .find(|(_, entry)| matches!(entry.outcome, Outcome::Pending) && now >= entry.due)?;
        entry.outcome = Outcome::Running;
        let cancel = Arc::new(AtomicBool::new(false));
        self.active = Some(Job {
            track,
            source: entry.source,
            cancel: Arc::clone(&cancel),
        });
        Some(Job {
            track,
            source: entry.source,
            cancel,
        })
    }

    /// Releases exactly this worker, rejecting cancelled, replaced, or deleted sources.
    pub(super) fn take_finished(&mut self, cancel: &Arc<AtomicBool>) -> Option<(TrackId, u64)> {
        if !self
            .active
            .as_ref()
            .is_some_and(|job| Arc::ptr_eq(&job.cancel, cancel))
        {
            return None;
        }
        let job = self.active.take()?;
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        self.entries
            .get(&job.track)
            .filter(|entry| entry.source == job.source && matches!(entry.outcome, Outcome::Running))
            .map(|_| (job.track, job.source))
    }

    pub(super) fn finish(&mut self, track: TrackId, source: u64, outcome: Outcome) {
        if let Some(entry) = self.entries.get_mut(&track)
            && entry.source == source
        {
            entry.outcome = outcome;
        }
    }

    pub(super) fn outcome(&self, track: TrackId) -> Option<&Outcome> {
        self.entries.get(&track).map(|entry| &entry.outcome)
    }

    pub(super) fn report(&self, track: TrackId) -> Option<&DrumKitAnalysis> {
        match self.outcome(track)? {
            Outcome::Ready(report) => Some(report),
            _ => None,
        }
    }

    pub(super) fn retry(&mut self, track: TrackId, source: u64, now: Instant) {
        self.cancel(track);
        self.entries.insert(
            track,
            Entry {
                source,
                due: now,
                outcome: Outcome::Pending,
            },
        );
    }

    pub(super) fn cancel(&mut self, track: TrackId) {
        if let Some(entry) = self.entries.get_mut(&track) {
            entry.outcome = Outcome::Cancelled;
        }
        if let Some(job) = &self.active
            && job.track == track
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn reset(&mut self) {
        if let Some(job) = &self.active {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.entries.clear();
        self.checked_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (TrackId, TrackId) {
        (TrackId(1), TrackId(2))
    }

    #[test]
    fn sources_are_debounced_and_workers_are_serial() {
        let (a, b) = ids();
        let now = Instant::now();
        let sources = BTreeMap::from([(a, 1), (b, 2)]);
        let mut state = DrumAnalysisState::default();
        state.observe(sources.clone(), now);
        assert!(state.start_next(now).is_none());
        state.observe(sources, now + DEBOUNCE);
        let first = state.start_next(now + DEBOUNCE).unwrap();
        assert!(state.start_next(now + DEBOUNCE).is_none());
        let (track, source) = state.take_finished(&first.cancel).unwrap();
        state.finish(track, source, Outcome::Failed("fixture".into()));
        let second = state.start_next(now + DEBOUNCE).unwrap();
        assert_ne!(first.track, second.track);
        assert!(matches!(
            state.outcome(first.track),
            Some(Outcome::Failed(_))
        ));
    }

    #[test]
    fn source_change_cancels_and_waits_for_the_old_worker() {
        let (track, _) = ids();
        let now = Instant::now();
        let mut state = DrumAnalysisState::default();
        state.observe(BTreeMap::from([(track, 1)]), now);
        let old = state.start_next(now + DEBOUNCE).unwrap();
        state.observe(BTreeMap::from([(track, 2)]), now + DEBOUNCE);
        assert!(old.cancel.load(Ordering::Relaxed));
        assert!(state.start_next(now + DEBOUNCE * 2).is_none());
        assert!(state.take_finished(&old.cancel).is_none());
        let new = state.start_next(now + DEBOUNCE * 2).unwrap();
        assert_eq!(new.source, 2);
        assert!(state.take_finished(&old.cancel).is_none());
        assert!(state.start_next(now + DEBOUNCE * 2).is_none());
        assert_eq!(state.take_finished(&new.cancel), Some((track, 2)));
    }

    #[test]
    fn failure_and_cancellation_latch_until_retry_or_source_change() {
        let (a, b) = ids();
        let now = Instant::now();
        let mut state = DrumAnalysisState::default();
        let sources = BTreeMap::from([(a, 1), (b, 2)]);
        state.observe(sources.clone(), now);
        let active = state.start_next(now + DEBOUNCE).unwrap();
        let queued = if active.track == a { b } else { a };
        state.cancel(queued);
        assert!(!active.cancel.load(Ordering::Relaxed));
        let (track, source) = state.take_finished(&active.cancel).unwrap();
        state.finish(track, source, Outcome::Failed("fixture".into()));
        for offset in 2..5 {
            state.observe(sources.clone(), now + DEBOUNCE * offset);
            assert!(state.start_next(now + DEBOUNCE * offset).is_none());
        }
        state.retry(track, source, now);
        let retry = state.start_next(now).unwrap();
        assert_eq!(retry.track, track);
        state.cancel(track);
        assert!(retry.cancel.load(Ordering::Relaxed));
        assert!(state.take_finished(&retry.cancel).is_none());
        state.observe(sources, now + DEBOUNCE * 5);
        assert!(state.start_next(now + DEBOUNCE * 5).is_none());
    }

    #[test]
    fn deletion_and_document_reset_reject_late_results_even_with_reused_ids() {
        let (track, _) = ids();
        let now = Instant::now();
        for reset_document in [false, true] {
            let mut state = DrumAnalysisState::default();
            state.retry(track, 1, now);
            let old = state.start_next(now).unwrap();
            if reset_document {
                state.reset();
            } else {
                state.observe(BTreeMap::new(), now);
            }
            assert!(old.cancel.load(Ordering::Relaxed));
            state.observe(BTreeMap::from([(track, 1)]), now);
            assert!(state.take_finished(&old.cancel).is_none());
            assert!(state.report(track).is_none());
            let next = state.start_next(now + DEBOUNCE).unwrap();
            assert!(state.take_finished(&old.cancel).is_none());
            assert_eq!(state.take_finished(&next.cancel), Some((track, 1)));
        }
    }
}
