//! Process isolation for instrument probing, shared by every executable frontend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{DrumKitAnalysis, DrumProbeRequest, SessionError, probe_drum_request};

const WORKER_ARG: &str = "--drum-probe-worker";
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

fn failure(error: impl std::fmt::Display) -> SessionError {
    SessionError::DrumAnalysis(error.to_string())
}

fn read_message(path: &Path) -> Result<Vec<u8>, SessionError> {
    if fs::metadata(path).map_err(failure)?.len() > MAX_MESSAGE_BYTES {
        return Err(failure("drum probe message exceeds 64 MiB"));
    }
    fs::read(path).map_err(failure)
}

/// Handles the private probe-worker invocation before a frontend opens its UI or transport.
///
/// `None` means ordinary application startup. Otherwise the executable should exit with the
/// returned status. The plugin's lifecycle stays on this process's main thread. Plugin output
/// never enters a frontend protocol: the parent redirects it and reads one result file instead.
pub fn handle_drum_probe_worker() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(WORKER_ARG)) {
        return None;
    }
    let (Some(request), Some(response)) = (args.next(), args.next()) else {
        return Some(2);
    };
    if args.next().is_some() {
        return Some(2);
    }
    let result: Result<DrumKitAnalysis, String> = (|| {
        let bytes = read_message(Path::new(&request))?;
        let request: DrumProbeRequest = serde_json::from_slice(&bytes).map_err(failure)?;
        probe_drum_request(&request)
    })()
    .map_err(|error: SessionError| error.to_string());
    let written = serde_json::to_vec(&result)
        .map_err(failure)
        .and_then(|bytes| fs::write(&response, bytes).map_err(failure));
    Some(if written.is_ok() { 0 } else { 1 })
}

struct JobDirectory(PathBuf);

impl JobDirectory {
    fn new() -> Result<Self, SessionError> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(failure)?
            .as_nanos();
        let serial = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "auris-drum-probe-{}-{stamp}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(failure)?;
        Ok(Self(path))
    }
}

impl Drop for JobDirectory {
    fn drop(&mut self) {
        // Only the directory this job successfully created, never a caller-supplied path.
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Probes in a child copy of the current executable, with cancellation and a wall-clock limit.
///
/// Every Auris frontend calls [`handle_drum_probe_worker`] at startup. A native plugin that
/// hangs or crashes is confined to this child. Call from a background task in a GUI; the
/// request already owns its exact sound snapshot and never borrows the live session.
pub fn run_drum_probe_isolated(
    request: &DrumProbeRequest,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<DrumKitAnalysis, SessionError> {
    request.options.validate()?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(failure("drum analysis cancelled"));
    }
    let timeout = timeout.min(Duration::from_secs(
        u64::from(request.options.timeout_seconds) + 10,
    ));
    let directory = JobDirectory::new()?;
    let input = directory.0.join("request.json");
    let output = directory.0.join("response.json");
    let bytes = serde_json::to_vec(request).map_err(failure)?;
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(failure("drum probe state exceeds 64 MiB"));
    }
    fs::write(&input, bytes).map_err(failure)?;
    let mut command = Command::new(std::env::current_exe().map_err(failure)?);
    command
        .arg(WORKER_ARG)
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut worker = Worker(command.spawn().map_err(failure)?);
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(failure("drum analysis cancelled"));
        }
        if started.elapsed() >= timeout {
            return Err(failure("drum analysis timed out"));
        }
        if let Some(status) = worker.0.try_wait().map_err(failure)? {
            if !status.success() {
                return Err(failure(format!("drum probe process failed: {status}")));
            }
            let bytes = read_message(&output)?;
            let result: Result<DrumKitAnalysis, String> =
                serde_json::from_slice(&bytes).map_err(failure)?;
            let report = result.map_err(failure)?;
            report.validate()?;
            if report.track != request.track
                || report.source_fingerprint != request.source_fingerprint
                || report.options != request.options
                || report.sample_rate != request.sample_rate
                || report.bpm != request.bpm
            {
                return Err(failure(
                    "the drum worker answered different measurement conditions",
                ));
            }
            return Ok(report);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
