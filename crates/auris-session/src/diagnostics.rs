//! What a launch leaves behind on disk for somebody reporting a problem: a log file per launch,
//! and a crash report per panic.
//!
//! The log panel keeps the records of *this* launch in memory, which is exactly what a crash
//! takes away. So the same records are also written to a file, one per launch, and a panic writes
//! a report of its own beside them — version, platform, the panic and where it happened, a
//! backtrace, and the lines logged just before. The next launch can then offer that report for an
//! Issue draft instead of the user having to reconstruct what happened.
//!
//! # Where
//!
//! Where each platform's own tools look for an application's logs, rather than in
//! [`config_dir`](crate::config_dir): that directory is meant to be small, hand-edited and kept
//! under version control, and a growing pile of logs is none of those. On macOS that is
//! `~/Library/Logs/Auris Studio`, which Console.app already lists; on Windows
//! `%LOCALAPPDATA%\Auris Studio\Logs`; elsewhere `$XDG_STATE_HOME/auris-studio/logs`.
//! [`LOG_DIR_VAR`] overrides all three.
//!
//! # Bounds
//!
//! The newest [`KEPT_LOGS`] launch logs and [`KEPT_CRASHES`] crash reports are kept; older ones
//! are removed when a new one is written, so a machine that runs Auris every day does not
//! collect logs forever. One launch's file stops growing at [`LOG_FILE_LIMIT`] bytes, so a loop
//! that logs on every frame cannot fill a disk either.
//!
//! # Threads
//!
//! Lines are written from every thread that logs, under one lock held for one line. The panic
//! hook never waits for that lock — a panic inside the logger would otherwise deadlock on itself
//! — and takes it only if it is free.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable naming the log directory outright.
///
/// For a portable installation, and for anybody who wants the logs somewhere their own tools
/// collect. An empty value counts as unset.
pub const LOG_DIR_VAR: &str = "AURIS_LOG_DIR";

/// How many launch logs are kept, counting the one being written.
pub const KEPT_LOGS: usize = 10;

/// How many crash reports are kept.
pub const KEPT_CRASHES: usize = 20;

/// The size at which one launch's log stops growing.
///
/// Eight mebibytes is days of warnings and a long session at `RUST_LOG=debug`. Past it the file
/// says so once and records nothing more; the in-memory log panel carries on.
pub const LOG_FILE_LIMIT: u64 = 8 * 1024 * 1024;

/// What a launch log's file name starts with.
const LOG_PREFIX: &str = "auris-studio-";
/// What a crash report's file name starts with.
const CRASH_PREFIX: &str = "crash-";
/// The file in the crash directory naming the newest report already offered to the user.
const SEEN_FILE: &str = "last-seen";

/// The directory this application writes its logs to.
///
/// See the [module documentation](self) for where that is on each platform.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(LOG_DIR_VAR).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    platform_log_dir(&home())
}

/// Where crash reports are written: a `crashes` folder inside [`log_dir`].
pub fn crash_dir() -> PathBuf {
    log_dir().join("crashes")
}

/// [`log_dir`] without the override, with the home directory passed in.
///
/// Decided with `cfg!` rather than `#[cfg]` so that every arm compiles, and is tested, on every
/// platform.
fn platform_log_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Logs").join("Auris Studio")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("Auris Studio")
            .join("Logs")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("state"))
            .join("auris-studio")
            .join("logs")
    }
}

/// The user's home directory, `USERPROFILE` first on Windows.
fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// One line identifying this build and platform, for the head of a log or crash report.
///
/// `app` is the application's name and version as its frontend knows them.
pub fn build_line(app: &str) -> String {
    format!(
        "{app} ({}) on {} {}",
        if cfg!(debug_assertions) {
            "development build"
        } else {
            "release build"
        },
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// A time as `2026-11-11T17:41:52Z`, in UTC.
///
/// UTC so that a report read on another continent means the same moment, and formatted here
/// rather than through a date crate because this is the only date the application writes.
pub fn timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// [`timestamp`] with the separators a file name cannot hold on Windows removed:
/// `20261111T174152Z`. Still sorts in time order, which the pruning relies on.
fn file_stamp(time: SystemTime) -> String {
    timestamp(time).replace(['-', ':'], "")
}

/// The proleptic Gregorian date of a day counted from 1970-01-01.
///
/// Howard Hinnant's `civil_from_days`, which is exact over the whole range of an `i64` of days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Removes all but the newest `keep` files in `dir` whose names start with `prefix`.
///
/// Newest by name, which carries the time: a modification time is changed by a copy or a backup
/// restore, a name is not. Failures are ignored — a log that could not be pruned is a log that
/// is still useful.
fn prune(dir: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for name in &names[..excess] {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// The log file of one launch.
#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    sink: Mutex<Sink>,
}

#[derive(Debug)]
struct Sink {
    file: File,
    written: u64,
    full: bool,
}

impl LogFile {
    /// Starts a new launch log in `dir`, removing the oldest so that [`KEPT_LOGS`] remain.
    ///
    /// The file opens with `header` — what built it and on what — and the time it was started.
    pub fn create(dir: &Path, header: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let now = SystemTime::now();
        let name = format!("{LOG_PREFIX}{}-{}.log", file_stamp(now), std::process::id());
        let path = dir.join(name);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let head = format!("{header}\nStarted {}\n\n", timestamp(now));
        file.write_all(head.as_bytes())?;
        // Pruned after the new file exists, so it counts toward what is kept.
        prune(dir, LOG_PREFIX, KEPT_LOGS);
        Ok(Self {
            path,
            sink: Mutex::new(Sink {
                file,
                written: head.len() as u64,
                full: false,
            }),
        })
    }

    /// Where this launch's log is being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record.
    ///
    /// A message spanning several lines keeps its continuation lines indented under the first, so
    /// that the file still reads one record per unindented line.
    pub fn record(&self, level: log::Level, target: &str, message: &str) {
        let line = format_line(SystemTime::now(), level, target, message);
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sink.append(&line);
    }

    /// Appends one line if nothing else is writing, and does nothing otherwise.
    ///
    /// For the panic hook, which may be running on the thread that holds the lock.
    fn try_append(&self, text: &str) {
        let mut sink = match self.sink.try_lock() {
            Ok(sink) => sink,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        sink.append(text);
    }
}

impl Sink {
    fn append(&mut self, text: &str) {
        if self.full {
            return;
        }
        if self.written + text.len() as u64 > LOG_FILE_LIMIT {
            self.full = true;
            let _ = self.file.write_all(
                b"\n(The log reached its size limit; later records are in the log panel only.)\n",
            );
            return;
        }
        // Unbuffered, so that what was logged before a crash is on disk when the process dies.
        if self.file.write_all(text.as_bytes()).is_ok() {
            self.written += text.len() as u64;
        }
    }
}

/// One record as the log file holds it.
fn format_line(time: SystemTime, level: log::Level, target: &str, message: &str) -> String {
    let mut line = format!("{} {level:<5} {target}: ", timestamp(time));
    for (index, part) in message.lines().enumerate() {
        if index > 0 {
            line.push_str("\n    ");
        }
        line.push_str(part);
    }
    line.push('\n');
    line
}

/// What the panic hook needs from the frontend that installs it.
pub struct CrashContext {
    /// [`build_line`] for this application.
    pub build: String,
    /// The lines logged most recently, newest last, as text.
    ///
    /// Called from the panic hook, so it must not wait for a lock — see the module docs.
    pub recent: Box<dyn Fn() -> String + Send + Sync>,
    /// This launch's log file, if one could be opened.
    pub log: Option<Arc<LogFile>>,
}

/// Writes a crash report into [`crash_dir`] for every panic, then runs the hook that was there.
///
/// The previous hook still runs, so a terminal keeps the panic message it always printed. A panic
/// a thread recovers from is still reported: it is a bug whether or not the application survived
/// it, and the report is the evidence.
///
/// Call once, from `main`, after logging is set up.
pub fn install_crash_reports(context: CrashContext) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format_crash_report(
            &context.build,
            SystemTime::now(),
            std::thread::current().name().unwrap_or("unnamed"),
            info,
            &std::backtrace::Backtrace::force_capture().to_string(),
            &(context.recent)(),
            context.log.as_deref().map(LogFile::path),
        );
        let written = write_crash_report(&crash_dir(), &report);
        if let Some(log) = &context.log {
            let location = info
                .location()
                .map(|location| format!(" at {location}"))
                .unwrap_or_default();
            let mut line = format_line(
                SystemTime::now(),
                log::Level::Error,
                "panic",
                &format!("{}{location}", panic_message(info)),
            );
            if let Ok(path) = &written {
                line.push_str(&format!("    crash report: {}\n", path.display()));
            }
            log.try_append(&line);
        }
        previous(info);
    }));
}

/// What a panic said, when it said it with a string.
fn panic_message(info: &PanicHookInfo<'_>) -> String {
    if let Some(text) = info.payload().downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = info.payload().downcast_ref::<String>() {
        text.clone()
    } else {
        "(a panic without a message)".to_owned()
    }
}

/// The text of one crash report.
#[allow(clippy::too_many_arguments)]
fn format_crash_report(
    build: &str,
    time: SystemTime,
    thread: &str,
    info: &PanicHookInfo<'_>,
    backtrace: &str,
    recent: &str,
    log: Option<&Path>,
) -> String {
    let location = info
        .location()
        .map(|location| location.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    crash_text(
        build,
        time,
        thread,
        &panic_message(info),
        &location,
        backtrace,
        recent,
        log,
    )
}

/// [`format_crash_report`] with the panic already taken apart, so it can be tested without one.
#[allow(clippy::too_many_arguments)]
fn crash_text(
    build: &str,
    time: SystemTime,
    thread: &str,
    message: &str,
    location: &str,
    backtrace: &str,
    recent: &str,
    log: Option<&Path>,
) -> String {
    let mut text = format!(
        "Auris Studio crash report\n\n\
         Build: {build}\n\
         Time: {}\n\
         Thread: {thread}\n\
         {SUMMARY}{}\n\
         Location: {location}\n",
        timestamp(time),
        first_line(message),
    );
    if message.lines().nth(1).is_some() {
        text.push_str("\nFull message:\n");
        text.push_str(message);
        text.push('\n');
    }
    if let Some(log) = log {
        text.push_str(&format!("Log file: {}\n", log.display()));
    }
    text.push_str("\nBacktrace:\n");
    text.push_str(backtrace.trim_end());
    text.push_str("\n\nRecent log (newest last):\n");
    if recent.trim().is_empty() {
        text.push_str("(nothing was logged)\n");
    } else {
        text.push_str(recent.trim_end());
        text.push('\n');
    }
    text
}

/// The label in front of the panic message, which [`CrashReport::summary`] reads back.
const SUMMARY: &str = "Panic: ";

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// Writes `report` as a new file in `dir` and prunes the oldest beyond [`KEPT_CRASHES`].
fn write_crash_report(dir: &Path, report: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stamp = file_stamp(SystemTime::now());
    let pid = std::process::id();
    // Two panics in the same second — one on each of two threads — get two files, and the
    // serial is zero-padded so that the second still sorts after the first.
    let mut serial = 0u32;
    let mut path = dir.join(format!("{CRASH_PREFIX}{stamp}-{pid}-{serial:03}.txt"));
    while path.exists() {
        serial += 1;
        path = dir.join(format!("{CRASH_PREFIX}{stamp}-{pid}-{serial:03}.txt"));
    }
    fs::write(&path, report)?;
    prune(dir, CRASH_PREFIX, KEPT_CRASHES);
    Ok(path)
}

/// A crash report found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashReport {
    /// Where it is.
    pub path: PathBuf,
    /// Its file name, which sorts in the order the reports were written.
    pub name: String,
    /// Everything it says.
    pub text: String,
}

impl CrashReport {
    /// What the panic said, on one line.
    pub fn summary(&self) -> &str {
        self.text
            .lines()
            .find_map(|line| line.strip_prefix(SUMMARY))
            .unwrap_or("")
    }
}

/// Crash reports kept in one directory.
#[derive(Clone, Debug)]
pub struct Crashes {
    dir: PathBuf,
}

impl Default for Crashes {
    fn default() -> Self {
        Self::new(crash_dir())
    }
}

impl Crashes {
    /// The reports in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The directory itself.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut names: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(CRASH_PREFIX) && name.ends_with(".txt"))
            .collect();
        names.sort();
        names
    }

    fn read(&self, name: String) -> Option<CrashReport> {
        let path = self.dir.join(&name);
        let text = fs::read_to_string(&path).ok()?;
        Some(CrashReport { path, name, text })
    }

    /// The newest report, whether or not it has been offered before.
    pub fn latest(&self) -> Option<CrashReport> {
        self.names().pop().and_then(|name| self.read(name))
    }

    /// The newest report written since [`Self::mark_seen`] was last given one, if any.
    pub fn unseen(&self) -> Option<CrashReport> {
        let seen = fs::read_to_string(self.dir.join(SEEN_FILE)).unwrap_or_default();
        let seen = seen.trim();
        self.names()
            .pop()
            .filter(|name| name.as_str() > seen)
            .and_then(|name| self.read(name))
    }

    /// Records that `report`, and everything older, has been offered.
    pub fn mark_seen(&self, report: &CrashReport) {
        let _ = fs::write(self.dir.join(SEEN_FILE), &report.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn timestamps_are_utc_calendar_dates() {
        assert_eq!(timestamp(at(0)), "1970-01-01T00:00:00Z");
        // 2026-11-11T17:41:52Z.
        assert_eq!(timestamp(at(1_794_418_912)), "2026-11-11T17:41:52Z");
        // A leap day, and the day after it.
        assert_eq!(timestamp(at(1_709_164_800)), "2024-02-29T00:00:00Z");
        assert_eq!(timestamp(at(1_709_251_199)), "2024-02-29T23:59:59Z");
        assert_eq!(timestamp(at(1_709_251_200)), "2024-03-01T00:00:00Z");
        assert_eq!(file_stamp(at(1_794_418_912)), "20261111T174152Z");
    }

    #[test]
    fn every_platform_has_a_log_folder_of_its_own_under_home() {
        let home = Path::new("/home/someone");
        let dir = platform_log_dir(home);
        if cfg!(target_os = "macos") {
            assert_eq!(dir, home.join("Library/Logs/Auris Studio"));
        } else if cfg!(target_os = "windows") {
            assert!(dir.ends_with(Path::new("Auris Studio").join("Logs")));
        } else {
            assert!(dir.ends_with("auris-studio/logs"));
        }
    }

    #[test]
    fn a_log_file_holds_its_header_and_indented_continuations() {
        let dir = tempfile::tempdir().unwrap();
        let log = LogFile::create(dir.path(), "Auris Studio 9.9.9").unwrap();
        log.record(log::Level::Warn, "auris_session", "no font\nsecond line");
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(text.starts_with("Auris Studio 9.9.9\nStarted "));
        assert!(
            text.contains(" WARN  auris_session: no font\n    second line\n"),
            "{text}"
        );
    }

    #[test]
    fn only_the_newest_launch_logs_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..KEPT_LOGS + 3 {
            fs::write(
                dir.path()
                    .join(format!("{LOG_PREFIX}2020010{index:02}.log")),
                "old",
            )
            .unwrap();
        }
        fs::write(dir.path().join("notes.txt"), "not a log").unwrap();
        let log = LogFile::create(dir.path(), "header").unwrap();
        let mut logs: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with(LOG_PREFIX))
            .collect();
        logs.sort();
        assert_eq!(logs.len(), KEPT_LOGS);
        // The file being written is the newest, so it survives its own pruning.
        assert!(log.path().exists());
        assert!(dir.path().join("notes.txt").exists());
        assert!(
            !dir.path()
                .join(format!("{LOG_PREFIX}202001000.log"))
                .exists()
        );
    }

    #[test]
    fn a_log_file_stops_at_its_limit_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let log = LogFile::create(dir.path(), "header").unwrap();
        let line = "x".repeat(64 * 1024);
        for _ in 0..(LOG_FILE_LIMIT / line.len() as u64 + 4) {
            log.record(log::Level::Info, "loop", &line);
        }
        let size = fs::metadata(log.path()).unwrap().len();
        assert!(size <= LOG_FILE_LIMIT + 200, "{size}");
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(text.ends_with("in the log panel only.)\n"));
    }

    #[test]
    fn a_crash_report_carries_what_a_maintainer_asks_for_first() {
        let text = crash_text(
            "Auris Studio 9.9.9 (release build) on macos aarch64",
            at(1_794_418_912),
            "main",
            "index out of bounds\nmore detail",
            "crates/auris-gpui/src/app.rs:10:5",
            "   0: auris_studio::main\n",
            "0s WARN auris_session: no font",
            Some(Path::new("/logs/auris-studio-1.log")),
        );
        for expected in [
            "Build: Auris Studio 9.9.9 (release build) on macos aarch64\n",
            "Time: 2026-11-11T17:41:52Z\n",
            "Thread: main\n",
            "Panic: index out of bounds\n",
            "Full message:\nindex out of bounds\nmore detail\n",
            "Location: crates/auris-gpui/src/app.rs:10:5\n",
            "Log file: /logs/auris-studio-1.log\n",
            "Backtrace:\n   0: auris_studio::main\n",
            "Recent log (newest last):\n0s WARN auris_session: no font\n",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn a_new_crash_is_offered_once_and_stays_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let crashes = Crashes::new(dir.path().join("crashes"));
        assert_eq!(crashes.unseen(), None);
        assert_eq!(crashes.latest(), None);

        let report = crash_text("b", at(0), "main", "boom", "here", "", "", None);
        let written = write_crash_report(crashes.dir(), &report).unwrap();
        let unseen = crashes.unseen().expect("a new report is offered");
        assert_eq!(unseen.path, written);
        assert_eq!(unseen.summary(), "boom");

        crashes.mark_seen(&unseen);
        assert_eq!(crashes.unseen(), None);
        // Still there to attach from Help → Report a problem.
        assert_eq!(crashes.latest(), Some(unseen.clone()));

        // A second crash in the same second is a second file, and is offered.
        let second = write_crash_report(crashes.dir(), &report).unwrap();
        assert_ne!(second, written);
        assert_eq!(crashes.unseen().map(|report| report.path), Some(second));
    }

    #[test]
    fn old_crash_reports_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..KEPT_CRASHES + 5 {
            fs::write(
                dir.path().join(format!("{CRASH_PREFIX}2020{index:04}.txt")),
                "old",
            )
            .unwrap();
        }
        write_crash_report(dir.path(), "new").unwrap();
        let count = fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(CRASH_PREFIX)
            })
            .count();
        assert_eq!(count, KEPT_CRASHES);
        assert_eq!(
            Crashes::new(dir.path().to_path_buf())
                .latest()
                .map(|report| report.text),
            Some("new".to_owned())
        );
    }
}
