//! Cooperative cancellation for one synchronous model tool invocation.
//!
//! MCP request handlers run the toolbox on blocking worker threads. Cancelling the async
//! request cannot stop such a thread, so the frontend installs one of these controls around the
//! call. Read-only work checks the flag at useful boundaries; mutating tools acquire the commit
//! gate immediately before their first durable change. Cancellation wins before that gate, while
//! a command that has already begun committing finishes as one truthful operation.

use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;

thread_local! {
    static CURRENT: RefCell<Option<Arc<Cancellation>>> = const { RefCell::new(None) };
}

/// Shared cancellation and commit state for one tool request.
#[derive(Default)]
pub struct Cancellation {
    state: AtomicU8,
    flag: Arc<AtomicBool>,
    external: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl std::fmt::Debug for Cancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Cancellation")
            .field("state", &self.state.load(Ordering::Acquire))
            .field("cancelled", &self.flag.load(Ordering::Acquire))
            .field("has_external_probe", &self.external.is_some())
            .finish()
    }
}

impl Cancellation {
    /// Creates an active request control.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a request control that also observes an external protocol token directly.
    ///
    /// The asynchronous watcher normally mirrors that token into [`Self::flag`], which lets
    /// blockwise jobs stop cheaply. The direct probe closes the scheduler race at the final commit
    /// boundary: a token already cancelled is observed even if its watcher has not run yet.
    pub fn with_probe(probe: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            external: Some(Arc::new(probe)),
            ..Self::default()
        }
    }

    /// Requests cancellation.
    ///
    /// Returns `true` when cancellation won before the durable commit boundary. Once a tool has
    /// begun committing, that publication must finish so the caller is never left with half of
    /// one operation. The interruption flag is still raised in that case: a multi-output job may
    /// stop before starting its next independently atomic file and truthfully report the outputs
    /// it already completed.
    pub fn cancel(&self) -> bool {
        let won = self
            .state
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        self.flag.store(true, Ordering::Release);
        won
    }

    /// Whether cancellation won before the request's commit boundary.
    pub fn is_cancelled(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            CANCELLED => true,
            COMMITTING => false,
            ACTIVE => {
                if self.external.as_ref().is_some_and(|probe| probe()) {
                    self.cancel();
                }
                self.flag.load(Ordering::Acquire)
            }
            _ => unreachable!("cancellation state is internal"),
        }
    }

    /// The interruption flag consumed by renderers and analysers between bounded work blocks.
    ///
    /// It is raised even after one durable commit has started, so a multi-output operation may
    /// finish that atomic publication and stop before beginning another one.
    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    /// A shareable handle for jobs whose cancellation control owns its flag.
    pub fn flag_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }

    /// Acquires the request's durable commit gate.
    ///
    /// Repeated calls are accepted because one tool may publish more than one related file after
    /// crossing its first irreversible boundary.
    pub fn begin_commit(&self) -> Result<(), String> {
        if self.is_cancelled() {
            return Err(cancelled_message());
        }
        match self
            .state
            .compare_exchange(ACTIVE, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                // The checks bracket the state transition. If the protocol token changed between
                // them, no durable code has run yet and cancellation can still take the gate back.
                if self.external.as_ref().is_some_and(|probe| probe())
                    && self
                        .state
                        .compare_exchange(
                            COMMITTING,
                            CANCELLED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                {
                    self.flag.store(true, Ordering::Release);
                    Err(cancelled_message())
                } else {
                    Ok(())
                }
            }
            Err(COMMITTING) => Ok(()),
            Err(CANCELLED) => Err(cancelled_message()),
            Err(_) => unreachable!("cancellation state is internal"),
        }
    }

    /// Ends one independently atomic publication and reopens cancellation before another.
    ///
    /// Related writes that make up one transaction deliberately stay in `COMMITTING` and call
    /// [`Self::begin_commit`] repeatedly. A tool that offers two independent outputs calls this
    /// after the first is complete. Cancellation raised while that output was publishing then
    /// wins before the next gate instead of being hidden by the earlier commit forever.
    pub fn finish_independent_commit(&self) -> Result<(), String> {
        match self
            .state
            .compare_exchange(COMMITTING, ACTIVE, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(ACTIVE) => {}
            Err(CANCELLED) => return Err(cancelled_message()),
            Err(_) => unreachable!("cancellation state is internal"),
        }
        if self.flag.load(Ordering::Acquire) || self.external.as_ref().is_some_and(|probe| probe())
        {
            self.cancel();
            Err(cancelled_message())
        } else {
            Ok(())
        }
    }
}

/// Runs `work` with `control` installed on the current blocking worker thread.
///
/// The previous control is restored even if `work` unwinds, which keeps pooled worker threads
/// from inheriting another request's cancellation state.
pub fn with_cancellation<T>(control: Arc<Cancellation>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<Cancellation>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|current| {
                current.replace(self.0.take());
            });
        }
    }

    let previous = CURRENT.with(|current| current.replace(Some(control)));
    let _restore = Restore(previous);
    work()
}

pub(crate) fn current() -> Arc<Cancellation> {
    CURRENT
        .with(|current| current.borrow().clone())
        .unwrap_or_else(|| Arc::new(Cancellation::new()))
}

pub(crate) fn check() -> Result<(), String> {
    match current().is_cancelled() {
        true => Err(cancelled_message()),
        false => Ok(()),
    }
}

pub(crate) fn begin_commit() -> Result<(), String> {
    current().begin_commit()
}

pub(crate) fn finish_independent_commit() -> Result<(), String> {
    current().finish_independent_commit()
}

fn cancelled_message() -> String {
    "the tool request was cancelled before making durable changes".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_commit_have_one_winner() {
        let cancelled = Cancellation::new();
        assert!(cancelled.cancel());
        assert!(cancelled.is_cancelled());
        assert!(cancelled.begin_commit().is_err());

        let committing = Cancellation::new();
        committing.begin_commit().unwrap();
        assert!(!committing.cancel());
        assert!(!committing.is_cancelled());
        assert!(committing.flag().load(Ordering::Acquire));
        committing.begin_commit().unwrap();
    }

    #[test]
    fn simultaneous_cancellation_and_commit_have_exactly_one_winner() {
        for _ in 0..256 {
            let control = Arc::new(Cancellation::new());
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let cancelled = {
                let control = Arc::clone(&control);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    control.cancel()
                })
            };
            let committed = {
                let control = Arc::clone(&control);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    control.begin_commit().is_ok()
                })
            };
            barrier.wait();
            let cancelled = cancelled.join().unwrap();
            let committed = committed.join().unwrap();
            assert_ne!(cancelled, committed);
        }
    }

    #[test]
    fn cancellation_after_one_independent_commit_blocks_the_next() {
        let control = Cancellation::new();
        control.begin_commit().unwrap();
        assert!(
            !control.cancel(),
            "the publication already in progress finishes"
        );
        assert!(control.finish_independent_commit().is_err());
        assert!(control.is_cancelled());
        assert!(control.begin_commit().is_err());

        let uninterrupted = Cancellation::new();
        uninterrupted.begin_commit().unwrap();
        uninterrupted.finish_independent_commit().unwrap();
        uninterrupted.begin_commit().unwrap();
    }

    #[test]
    fn cancellation_racing_an_independent_boundary_blocks_the_next_commit() {
        for _ in 0..256 {
            let control = Arc::new(Cancellation::new());
            control.begin_commit().unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let cancelled = {
                let control = Arc::clone(&control);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    control.cancel()
                })
            };
            let finished = {
                let control = Arc::clone(&control);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    control.finish_independent_commit()
                })
            };
            barrier.wait();
            let _ = cancelled.join().unwrap();
            let _ = finished.join().unwrap();
            assert!(control.begin_commit().is_err());
        }
    }

    #[test]
    fn commit_gate_observes_an_external_token_without_waiting_for_its_watcher() {
        let token = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&token);
        let cancellation = Cancellation::with_probe(move || observed.load(Ordering::Acquire));

        token.store(true, Ordering::Release);

        assert!(cancellation.begin_commit().is_err());
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn worker_scope_is_restored_after_unwind() {
        let control = Arc::new(Cancellation::new());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let control = Arc::clone(&control);
            move || {
                with_cancellation(control, || {
                    assert!(!current().is_cancelled());
                    panic!("probe");
                });
            }
        }));
        assert!(result.is_err());
        assert!(!Arc::ptr_eq(&current(), &control));
    }

    #[test]
    fn cancelled_edit_never_reaches_the_project_file() {
        use auris_session::{Session, SessionOptions};

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Song.auris");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        session.add_default_instrument_track("Original").unwrap();
        session.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();

        session.add_default_instrument_track("Cancelled").unwrap();
        let control = Arc::new(Cancellation::new());
        assert!(control.cancel());
        let result = with_cancellation(control, || crate::save_checkpointed(&mut session));

        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
