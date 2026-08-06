//! What the application logged, kept where a window can show it.
//!
//! Almost everything that goes wrong in a DAW goes wrong *quietly*, on purpose: a SoundFont whose
//! file has moved costs one track its sound rather than the session, an unknown plugin is
//! substituted, an engine command dropped under load is dropped. All of it is logged, and until
//! now the log went to a terminal — which the release build does not have, and which nobody
//! launching an application from an icon has ever seen.
//!
//! So the records are kept. [`Logbook`] is a bounded ring the log macros write into and the log
//! panel reads out of; [`install`] puts it in front of `env_logger` rather than instead of it, so
//! a developer running from a terminal still gets everything they had.
//!
//! # Threads
//!
//! Written from wherever a `log::warn!` happens — which is every thread the application has,
//! except the audio one, where logging is forbidden for the usual reason — and read from the
//! window. A `Mutex` around a `VecDeque` covers both, and the lock is held for exactly one push or
//! one clone.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use log::{Level, Log, Metadata, Record};

/// How many records are kept.
///
/// Enough to hold everything a start-up and a few minutes of work produce, and small enough that
/// a run left going overnight cannot grow without bound. What falls off the end is the oldest,
/// because a problem being diagnosed is a problem that just happened.
pub const CAPACITY: usize = 500;

/// One line the application logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// How bad it is.
    pub level: Level,
    /// Which module wrote it.
    pub target: String,
    /// What it said.
    pub message: String,
    /// Whole seconds since the application started.
    ///
    /// Relative rather than a clock reading, and coarse: what a log panel is read for is *what
    /// happened just before this*, and a wall-clock timestamp answers a question nobody is
    /// asking while costing a date format in two languages.
    pub seconds: u64,
}

/// The records, newest last.
#[derive(Debug)]
pub struct Logbook {
    entries: Mutex<VecDeque<Entry>>,
    start: Instant,
}

impl Default for Logbook {
    fn default() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            start: Instant::now(),
        }
    }
}

impl Logbook {
    /// An empty book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a record, dropping the oldest when the book is full.
    pub fn push(&self, level: Level, target: &str, message: String) {
        let entry = Entry {
            level,
            target: target.to_string(),
            message,
            seconds: self.start.elapsed().as_secs(),
        };
        let mut entries = self.lock();
        if entries.len() == CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Every record, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.lock().iter().cloned().collect()
    }

    /// How many of the records are warnings or errors.
    ///
    /// What the status bar shows beside the panel's icon: a count of things that went wrong is
    /// worth a badge, and a count of everything that happened is not.
    pub fn problems(&self) -> usize {
        self.lock()
            .iter()
            .filter(|entry| entry.level <= Level::Warn)
            .count()
    }

    /// Forgets everything.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Locks the ring, ignoring poisoning.
    ///
    /// A panic elsewhere must not turn every later `log::warn!` into a second panic — least of
    /// all in the machinery whose whole job is to record what went wrong the first time.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// A logger that keeps every record it forwards.
struct Recorder {
    book: Arc<Logbook>,
    inner: env_logger::Logger,
}

impl Log for Recorder {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        // Through the same filter the terminal uses, so the panel and the terminal never disagree
        // about what was logged. `Logger::log` applies the filter itself, and does not tell us
        // whether it did — hence asking first.
        if !self.inner.matches(record) {
            return;
        }
        self.book
            .push(record.level(), record.target(), record.args().to_string());
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// The one book this process writes to.
///
/// A global rather than a value threaded from `main` into the view, because there is exactly one
/// logger per process and pretending otherwise would mean carrying a handle through every
/// constructor to reach the one place that reads it. `log` itself is a global for the same reason.
static BOOK: OnceLock<Arc<Logbook>> = OnceLock::new();

/// Starts logging, into a terminal and into [`book`].
///
/// In front of `env_logger` rather than instead of it: a developer running from a terminal keeps
/// exactly what they had, and the window gains a copy. `RUST_LOG` still decides what is recorded,
/// and warnings are on without it because a warning here is a thing that silently did not happen.
///
/// Call once, from `main`. A second call changes nothing.
pub fn install() {
    let book = Arc::clone(book());
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).build();
    let level = inner.filter();
    let recorder = Recorder { book, inner };
    if log::set_boxed_logger(Box::new(recorder)).is_ok() {
        log::set_max_level(level);
    }
}

/// The records this process has logged.
///
/// Empty in a build that never called [`install`] — a test, or a frontend that logs to a terminal
/// and nowhere else — which is a book with nothing in it rather than an error.
pub fn book() -> &'static Arc<Logbook> {
    BOOK.get_or_init(|| Arc::new(Logbook::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_record_is_what_falls_off_the_end() {
        // A problem being diagnosed is one that just happened, so the end that is kept is the
        // new one — a book that dropped the newest would be a book that goes blind precisely
        // when something is going wrong.
        let book = Logbook::new();
        for index in 0..CAPACITY + 10 {
            book.push(Level::Info, "test", format!("line {index}"));
        }
        let entries = book.entries();
        assert_eq!(entries.len(), CAPACITY);
        assert_eq!(entries[0].message, "line 10");
        assert_eq!(
            entries[CAPACITY - 1].message,
            format!("line {}", CAPACITY + 9)
        );
    }

    #[test]
    fn only_what_went_wrong_is_counted_as_a_problem() {
        // The badge exists to say "something needs looking at". Counting the ordinary chatter
        // would make it a number that is always large and therefore never read.
        let book = Logbook::new();
        book.push(Level::Info, "test", "opened a project".into());
        book.push(Level::Debug, "test", "rebuilt the graph".into());
        book.push(Level::Trace, "test", "a block".into());
        assert_eq!(book.problems(), 0);
        book.push(Level::Warn, "test", "a font moved".into());
        book.push(Level::Error, "test", "the device disappeared".into());
        assert_eq!(book.problems(), 2);
        assert_eq!(book.entries().len(), 5);
    }

    #[test]
    fn an_empty_book_says_so_and_clears_to_one() {
        let book = Logbook::new();
        assert!(book.entries().is_empty());
        book.push(Level::Warn, "test", "something".into());
        assert!(!book.entries().is_empty());
        book.clear();
        assert!(book.entries().is_empty());
        assert_eq!(book.problems(), 0);
    }

    #[test]
    fn a_record_keeps_what_wrote_it() {
        // The target is half the value of a log line: "no SoundFont file for …" means one thing
        // from the session and another entirely from the importer.
        let book = Logbook::new();
        book.push(Level::Warn, "auris_session::session", "no font".into());
        let entry = &book.entries()[0];
        assert_eq!(entry.target, "auris_session::session");
        assert_eq!(entry.message, "no font");
        assert_eq!(entry.level, Level::Warn);
    }
}
