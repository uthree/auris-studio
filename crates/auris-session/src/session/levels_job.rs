//! A balance pass whose expensive measurements can leave the session thread.

use std::sync::Arc;

use auris_core::{PluginRegistry, Project};
use auris_engine::{EngineCommand, OfflineOptions, RenderProgress};
use auris_gpu::GpuContext;

use super::{
    BalanceReport, FADER_RANGE_DB, Session, SessionError, TrackId, TrackLevel, analyze_loudness,
    fader_for, faders_lift_db, integrated_lufs, master_gain_db,
};
use crate::RenderJob;

/// Which measurement of a composed mix is being rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposeBalancePhase {
    /// One named track, heard through its normal routing.
    Track(String),
    /// The whole mix after each part has reached its target.
    Mix,
    /// The whole mix after the shared gain lift, including the limiter's response.
    Verification,
}

/// The position of one render within the complete balance pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeBalanceProgress {
    /// Measurements already completed.
    pub completed: usize,
    /// All track measurements plus two complete mix renders.
    pub total: usize,
    /// What the current measurement is listening to.
    pub phase: ComposeBalancePhase,
}

/// One `Send` render and measurement captured on the session's owning thread.
///
/// Run this on a worker, then pass its result to [`Session::continue_composed_balance`] on
/// the session thread. Hosted plugin factories stay there while their render halves travel here.
pub struct ComposeBalanceJob {
    state: Box<BalanceState>,
    render: Box<RenderJob>,
    gpu: Option<Arc<GpuContext>>,
}

/// An opaque measurement ready for the next session-thread step.
///
/// It contains only detached state. No live faders change until the final result is accepted.
pub struct ComposeBalanceResult {
    state: Box<BalanceState>,
}

/// The next measurement, or the report after all measured gains have been applied.
pub enum ComposeBalanceStep {
    /// Another job to run away from the session thread.
    Pending(ComposeBalanceJob),
    /// The pass finished and its gains were applied without an additional undo step.
    Complete(BalanceReport),
}

struct BalanceState {
    owner: Arc<PluginRegistry>,
    project: Project,
    revision: u64,
    levelled: Vec<TrackId>,
    completed: usize,
    tracks: Vec<TrackLevel>,
    lift_db: f32,
    balanced_lufs: Option<f32>,
    now_lufs: Option<f32>,
}

impl BalanceState {
    fn progress(&self) -> ComposeBalanceProgress {
        let phase = match self.levelled.get(self.completed) {
            Some(id) => ComposeBalancePhase::Track(
                self.project
                    .track(*id)
                    .expect("a track in the detached balance project")
                    .name
                    .clone(),
            ),
            None if self.completed == self.levelled.len() => ComposeBalancePhase::Mix,
            None => ComposeBalancePhase::Verification,
        };
        ComposeBalanceProgress {
            completed: self.completed,
            total: self.levelled.len() + 2,
            phase,
        }
    }

    fn render_project(&self) -> Project {
        let mut project = self.project.clone();
        project.master.gain_db = 0.0;
        if let Some(id) = self.levelled.get(self.completed) {
            for entry in &mut project.tracks {
                entry.mixer.solo = entry.id == *id;
                entry.mixer.mute = false;
            }
        }
        project
    }

    fn apply_measurement(&mut self, lufs: Option<f32>, true_peak_db: f32) {
        if let Some(id) = self.levelled.get(self.completed) {
            let entry = self
                .project
                .track_mut(*id)
                .expect("a track in the detached balance project");
            let was_db = entry.mixer.gain_db;
            let target_lufs = entry.mixer.target_lufs;
            let now_db = match (target_lufs, lufs) {
                (Some(target), Some(measured)) => fader_for(target, measured, was_db),
                _ => was_db,
            };
            entry.mixer.gain_db = now_db;
            self.tracks.push(TrackLevel {
                name: entry.name.clone(),
                target_lufs,
                measured_lufs: lufs,
                was_db,
                now_db,
            });
        } else if self.completed == self.levelled.len() {
            self.balanced_lufs = lufs;
            let headroom = self
                .tracks
                .iter()
                .map(|level| FADER_RANGE_DB.1 - level.now_db)
                .fold(f32::INFINITY, f32::min);
            self.lift_db = lufs.map_or(0.0, |lufs| faders_lift_db(lufs, true_peak_db, headroom));
            for (level, &id) in self.tracks.iter_mut().zip(&self.levelled) {
                level.now_db += self.lift_db;
                self.project
                    .track_mut(id)
                    .expect("a track in the detached balance project")
                    .mixer
                    .gain_db = level.now_db;
            }
        } else {
            let gain = lufs.map_or(0.0, |lufs| master_gain_db(lufs, true_peak_db));
            self.project.master.gain_db = gain;
            self.now_lufs = lufs.map(|lufs| lufs + gain);
        }
        self.completed += 1;
    }
}

impl ComposeBalanceJob {
    /// What this job measures and how many earlier measurements have finished.
    pub fn progress(&self) -> ComposeBalanceProgress {
        self.state.progress()
    }

    /// Renders and measures this step, honoring the caller's progress and cancellation hooks.
    ///
    /// The expensive audio rendering and loudness analysis run entirely on the calling thread.
    /// Reported fractions cover the whole balance pass, including its earlier measurements.
    pub fn run(
        mut self,
        progress: &mut RenderProgress<'_>,
    ) -> Result<ComposeBalanceResult, SessionError> {
        let total = (self.state.levelled.len() + 2) as f32;
        let mix = progress.within(
            self.state.completed as f32 / total,
            1.0 / total,
            |progress| {
                self.render
                    .render(&OfflineOptions::whole_project(), progress)
            },
        )?;
        let lufs = integrated_lufs(&mix);
        // Stem faders use LUFS only. Preserve the existing pass's two true-peak measurements.
        let true_peak_db = if self.state.completed >= self.state.levelled.len() && lufs.is_some() {
            analyze_loudness(self.gpu.as_deref(), &mix).true_peak_db()
        } else {
            f32::NEG_INFINITY
        };
        self.state.apply_measurement(lufs, true_peak_db);
        Ok(ComposeBalanceResult { state: self.state })
    }
}

impl Session {
    /// Starts the optional automatic balance of a piece adopted with `compose_without_balance`.
    ///
    /// Captures hosted render instances on this thread. The job may then run on a worker.
    /// Returns `None` when automatic composition balancing is disabled in this session.
    pub fn begin_composed_balance(&mut self) -> Option<ComposeBalanceJob> {
        self.balance_composed.then(|| self.begin_balance_job())
    }

    pub(super) fn begin_balance_job(&mut self) -> ComposeBalanceJob {
        let state = Box::new(BalanceState {
            owner: Arc::clone(&self.registry),
            project: self.project.clone(),
            revision: self.revision,
            levelled: self
                .project
                .tracks
                .iter()
                .filter(|track| !track.kind.is_bus())
                .map(|track| track.id)
                .collect(),
            completed: 0,
            tracks: Vec::new(),
            lift_db: 0.0,
            balanced_lufs: None,
            now_lufs: None,
        });
        self.balance_job_for(state)
    }

    fn balance_job_for(&mut self, mut state: Box<BalanceState>) -> ComposeBalanceJob {
        let render = Box::new(self.job_for(state.render_project()));
        // Loading or restoring a native instance may queue its initial state notifications.
        // Settle those on the owner thread before taking the revision the worker must preserve.
        self.poll();
        state.revision = self.revision;
        ComposeBalanceJob {
            state,
            render,
            gpu: self.gpu.clone(),
        }
    }

    /// Accepts a worker measurement and prepares the next step, or applies the finished gains.
    ///
    /// Call on the session's owning thread: preparing another hosted instance is a main-thread
    /// operation. A document edit, undo or replacement since the first step rejects the result.
    /// Final gains belong to the composition's existing undo step and add no history entry.
    pub fn continue_composed_balance(
        &mut self,
        result: ComposeBalanceResult,
    ) -> Result<ComposeBalanceStep, SessionError> {
        let state = result.state;
        if !Arc::ptr_eq(&self.registry, &state.owner) {
            return Err(SessionError::StaleBalance);
        }
        // A native editor may have queued a change since the most recent window tick. Observe
        // it before checking staleness so it cannot be folded into the next job's setup phase.
        self.poll();
        if self.revision != state.revision {
            return Err(SessionError::StaleBalance);
        }
        if state.completed < state.levelled.len() + 2 {
            return Ok(ComposeBalanceStep::Pending(self.balance_job_for(state)));
        }
        for (&id, level) in state.levelled.iter().zip(&state.tracks) {
            self.write_fader(id, level.now_db);
        }
        let master_db = state.project.master.gain_db;
        self.project.master.gain_db = master_db;
        self.send(EngineCommand::SetMasterGain(master_db));
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        Ok(ComposeBalanceStep::Complete(BalanceReport {
            tracks: state.tracks,
            lift_db: state.lift_db,
            master_db,
            balanced_lufs: state.balanced_lufs,
            now_lufs: state.now_lufs,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, SessionOptions};

    fn composition() -> auris_compose::Composition {
        let spec = auris_compose::SongSpec::parse(
            r#"
            title = "Measured song"
            tempo = 240
            form = ["verse"]
            seed = 17
            [section.verse]
            bars = 1
            [[part]]
            name = "tune"
            role = "melody"
            [[part]]
            name = "low"
            role = "bass"
            "#,
        )
        .unwrap();
        auris_compose::compose(&spec)
    }

    fn prepared_session() -> Session {
        let mut session = Session::new(SessionOptions::headless().with_balance(true)).unwrap();
        session
            .add_default_instrument_track("Before composing")
            .unwrap();
        session.forget_history();
        session
    }

    #[test]
    fn staged_balance_matches_synchronous_composition_and_keeps_one_undo() {
        let composition = composition();
        let mut synchronous = prepared_session();
        let expected = synchronous.compose(&composition).unwrap();
        let mut session = prepared_session();
        let before = session.project().clone();
        let report = session.compose_without_balance(&composition).unwrap();
        assert!(report.balance.is_none());
        let unbalanced = session.project().clone();
        let mut job = session.begin_composed_balance().unwrap();
        let mut reported = Vec::new();
        let total = job.progress().total;
        let mut completed = 0;
        let actual = loop {
            assert_eq!(job.progress().completed, completed);
            // This also verifies that both the job and the returned continuation are Send.
            let (result, fractions) = std::thread::spawn(move || {
                let mut fractions = Vec::new();
                let result = job.run(&mut RenderProgress::reporting(&mut |f| fractions.push(f)));
                (result, fractions)
            })
            .join()
            .unwrap();
            reported.extend(fractions);
            assert_eq!(
                session.project(),
                &unbalanced,
                "a worker changed live faders"
            );
            completed += 1;
            match session.continue_composed_balance(result.unwrap()).unwrap() {
                ComposeBalanceStep::Pending(next) => job = next,
                ComposeBalanceStep::Complete(report) => break report,
            }
        };
        assert_eq!(completed, total);
        assert_eq!(Some(actual), expected.balance);
        assert_eq!(session.project(), synchronous.project());
        assert!(!reported.is_empty());
        assert!(reported.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(reported.last(), Some(&1.0));
        let balanced = session.project().clone();
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo(), "balancing added another undo step");
        assert_eq!(session.redo(), Some(Edit::Compose));
        assert_eq!(session.project(), &balanced);
    }

    #[test]
    fn a_changed_document_rejects_an_intermediate_balance_result() {
        let mut session = prepared_session();
        session.compose_without_balance(&composition()).unwrap();
        let job = session.begin_composed_balance().unwrap();
        let result = job.run(&mut RenderProgress::default()).unwrap();
        let track = session.project().tracks[0].id;
        session
            .rename_track(track, "Edited while measuring")
            .unwrap();
        let edited = session.project().clone();
        assert!(matches!(
            session.continue_composed_balance(result),
            Err(SessionError::StaleBalance)
        ));
        assert_eq!(session.project(), &edited);
    }

    #[test]
    fn another_session_cannot_accept_a_result_at_the_same_revision() {
        let composition = composition();
        let mut source = prepared_session();
        let mut destination = prepared_session();
        source.compose_without_balance(&composition).unwrap();
        destination.compose_without_balance(&composition).unwrap();
        let result = source
            .begin_composed_balance()
            .unwrap()
            .run(&mut RenderProgress::default())
            .unwrap();
        assert_eq!(source.revision(), destination.revision());
        let before = destination.project().clone();
        assert!(matches!(
            destination.continue_composed_balance(result),
            Err(SessionError::StaleBalance)
        ));
        assert_eq!(destination.project(), &before);
    }

    #[test]
    fn undo_before_the_last_measurement_rejects_all_measured_gains() {
        let mut session = prepared_session();
        let before = session.project().clone();
        session.compose_without_balance(&composition()).unwrap();
        let mut job = session.begin_composed_balance().unwrap();
        while job.progress().completed + 1 < job.progress().total {
            let result = job.run(&mut RenderProgress::default()).unwrap();
            let ComposeBalanceStep::Pending(next) =
                session.continue_composed_balance(result).unwrap()
            else {
                panic!("the verification render should still be pending");
            };
            job = next;
        }
        let result = job.run(&mut RenderProgress::default()).unwrap();
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert!(matches!(
            session.continue_composed_balance(result),
            Err(SessionError::StaleBalance)
        ));
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn a_cancelled_measurement_leaves_written_levels_and_history_intact() {
        let mut session = prepared_session();
        session.compose_without_balance(&composition()).unwrap();
        let written = session.project().clone();
        let job = session.begin_composed_balance().unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let result = job.run(&mut RenderProgress::default().cancelled_by(&cancel));
        assert!(matches!(
            result,
            Err(SessionError::Engine(
                auris_engine::EngineError::RenderCancelled
            ))
        ));
        assert_eq!(session.project(), &written);
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert!(!session.can_undo());
    }
}
