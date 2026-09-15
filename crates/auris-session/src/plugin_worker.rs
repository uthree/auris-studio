use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auris_core::plugin::{PluginCategory, PluginKind};
use serde::{Deserialize, Serialize};

use crate::{ClapPluginInfo, InstalledPluginFiles, SessionError, Vst3PluginInfo};

const WORKER_ARG: &str = "--plugin-discovery-worker";
const MAX_MESSAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_WORKER_DURATION: Duration = Duration::from_secs(30);
const MAX_PLUGIN_CLASSES: usize = 4_096;
const MAX_PLUGIN_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_PLUGIN_FIELD_BYTES: usize = 4_096;
const MAX_ERROR_BYTES: usize = 16 * 1024;
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);
static ACTIVE_PLUGIN_PROBES: AtomicUsize = AtomicUsize::new(0);
const MAX_ACTIVE_PLUGIN_PROBES: usize = 4;

struct ProbePermit<'a>(&'a AtomicUsize);

impl<'a> ProbePermit<'a> {
    fn acquire(
        active: &'a AtomicUsize,
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> Result<Self, SessionError> {
        let started = Instant::now();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(failure("plugin discovery cancelled"));
            }
            if active
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    (current < MAX_ACTIVE_PLUGIN_PROBES).then_some(current + 1)
                })
                .is_ok()
            {
                return Ok(ProbePermit(active));
            }
            if started.elapsed() >= timeout {
                return Err(failure(
                    "plugin inspection timed out waiting for an available worker",
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for ProbePermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn failure(error: impl std::fmt::Display) -> SessionError {
    let mut message = error.to_string();
    if message.len() > MAX_ERROR_BYTES {
        let mut end = MAX_ERROR_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    SessionError::PluginDiscovery(message)
}

fn read_message(path: &Path) -> Result<Vec<u8>, SessionError> {
    let file = fs::File::open(path).map_err(failure)?;
    let size = file.metadata().map_err(failure)?.len();
    if size > MAX_MESSAGE_BYTES {
        return Err(failure("plugin worker message exceeds 32 MiB"));
    }
    let capacity = usize::try_from(size)
        .map_err(|_| failure("plugin worker message does not fit in memory"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| failure("not enough memory to read the plugin worker message"))?;
    file.take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(failure(
            "plugin worker message grew beyond 32 MiB while it was read",
        ));
    }
    Ok(bytes)
}

/// Native plugin format inspected by an isolated discovery worker.
#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub enum PluginFormat {
    /// A CLAP plugin file or bundle.
    Clap,
    /// A VST3 plugin file or bundle.
    Vst3,
}

/// A bounded inventory of installed CLAP and VST3 files.
#[derive(Clone, Debug)]
pub struct PluginDiscoveryJob {
    extra_paths: Vec<PathBuf>,
}

impl PluginDiscoveryJob {
    /// Captures the additional plugin roots configured for the current frontend.
    pub fn new(extra_paths: &[PathBuf]) -> Self {
        Self {
            extra_paths: extra_paths.to_vec(),
        }
    }

    /// Runs discovery in a child process, respecting cancellation and a wall-clock limit.
    pub fn run(
        self,
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> Result<InstalledPluginFiles, SessionError> {
        let response = run_worker(
            WorkerRequest::Discover {
                extra_paths: self.extra_paths,
            },
            cancelled,
            timeout,
        )?;
        match response {
            WorkerResponse::Discovery(files) => {
                validate_discovery(&files)?;
                Ok(files)
            }
            _ => Err(failure("plugin worker returned the wrong response kind")),
        }
    }
}

/// Metadata inspection for one third-party plugin binary.
#[derive(Clone, Debug)]
pub struct PluginProbeJob {
    format: PluginFormat,
    path: PathBuf,
}

impl PluginProbeJob {
    /// Captures one plugin file for isolated metadata inspection.
    pub fn new(format: PluginFormat, path: PathBuf) -> Self {
        Self { format, path }
    }

    /// Loads the binary only in a child process and returns its bounded metadata.
    pub fn run(
        self,
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> Result<PluginProbeResult, SessionError> {
        let timeout = timeout
            .max(Duration::from_millis(100))
            .min(MAX_WORKER_DURATION);
        // A saturated probe pool is a queue, not evidence that this plugin is unreadable. Keep the
        // caller in its existing loading state until a bounded slot opens, then give the admitted
        // worker its normal execution timeout.
        let _permit = ProbePermit::acquire(&ACTIVE_PLUGIN_PROBES, cancelled, timeout)?;
        self.run_admitted(cancelled, timeout)
    }

    /// Runs inspection before one shared catalog deadline, including time spent waiting for a
    /// worker slot.
    pub fn run_until(
        self,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<PluginProbeResult, SessionError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| failure("plugin inspection reached the catalog deadline"))?
            .min(MAX_WORKER_DURATION);
        if remaining < Duration::from_millis(100) {
            return Err(failure("plugin inspection reached the catalog deadline"));
        }
        let _permit = ProbePermit::acquire(&ACTIVE_PLUGIN_PROBES, cancelled, remaining)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| failure("plugin inspection reached the catalog deadline"))?
            .min(MAX_WORKER_DURATION);
        if remaining < Duration::from_millis(100) {
            return Err(failure("plugin inspection reached the catalog deadline"));
        }
        self.run_admitted(cancelled, remaining)
    }

    fn run_admitted(
        self,
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> Result<PluginProbeResult, SessionError> {
        let expected = self.format;
        let response = run_worker(
            WorkerRequest::Probe {
                format: self.format,
                path: self.path,
            },
            cancelled,
            timeout,
        )?;
        match (expected, response) {
            (PluginFormat::Clap, WorkerResponse::Clap(plugins)) => {
                validate_clap_plugins(&plugins)?;
                Ok(PluginProbeResult::Clap(
                    plugins.into_iter().map(Into::into).collect(),
                ))
            }
            (PluginFormat::Vst3, WorkerResponse::Vst3(plugins)) => {
                validate_vst3_plugins(&plugins)?;
                Ok(PluginProbeResult::Vst3(
                    plugins.into_iter().map(Into::into).collect(),
                ))
            }
            _ => Err(failure("plugin worker returned the wrong response kind")),
        }
    }
}

/// Metadata returned after isolated inspection of a native plugin file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginProbeResult {
    /// CLAP descriptors exported by the file.
    Clap(Vec<ClapPluginInfo>),
    /// VST3 audio classes exported by the bundle.
    Vst3(Vec<Vst3PluginInfo>),
}

#[derive(Debug, Deserialize, Serialize)]
enum WorkerRequest {
    Discover { extra_paths: Vec<PathBuf> },
    Probe { format: PluginFormat, path: PathBuf },
}

#[derive(Debug, Deserialize, Serialize)]
enum WorkerResponse {
    Discovery(InstalledPluginFiles),
    Clap(Vec<ClapPluginInfoWire>),
    Vst3(Vec<Vst3PluginInfoWire>),
}

#[derive(Debug, Deserialize, Serialize)]
struct ClapPluginInfoWire {
    clap_id: String,
    name: String,
    vendor: String,
    description: String,
    version: String,
    kind: PluginKind,
    category: PluginCategory,
}

impl From<ClapPluginInfo> for ClapPluginInfoWire {
    fn from(info: ClapPluginInfo) -> Self {
        Self {
            clap_id: info.clap_id,
            name: info.name,
            vendor: info.vendor,
            description: info.description,
            version: info.version,
            kind: info.kind,
            category: info.category,
        }
    }
}

impl From<ClapPluginInfoWire> for ClapPluginInfo {
    fn from(info: ClapPluginInfoWire) -> Self {
        Self {
            clap_id: info.clap_id,
            name: info.name,
            vendor: info.vendor,
            description: info.description,
            version: info.version,
            kind: info.kind,
            category: info.category,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Vst3PluginInfoWire {
    class_id: String,
    name: String,
    vendor: String,
    version: String,
    kind: PluginKind,
    category: PluginCategory,
    has_gui: bool,
}

impl From<Vst3PluginInfo> for Vst3PluginInfoWire {
    fn from(info: Vst3PluginInfo) -> Self {
        Self {
            class_id: info.class_id,
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            kind: info.kind,
            category: info.category,
            has_gui: info.has_gui,
        }
    }
}

impl From<Vst3PluginInfoWire> for Vst3PluginInfo {
    fn from(info: Vst3PluginInfoWire) -> Self {
        Self {
            class_id: info.class_id,
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            kind: info.kind,
            category: info.category,
            has_gui: info.has_gui,
        }
    }
}

fn validate_discovery(files: &InstalledPluginFiles) -> Result<(), SessionError> {
    let (count, bytes) = crate::session::plugin_discovery::discovery_result_usage(files)
        .ok_or_else(|| failure("plugin discovery path metadata overflowed"))?;
    if count > crate::session::plugin_discovery::DISCOVERY_FILE_LIMIT
        || bytes > crate::session::plugin_discovery::DISCOVERY_PATH_BYTE_LIMIT
    {
        return Err(failure("plugin discovery result exceeded its safety limit"));
    }
    Ok(())
}

fn validate_fields<'a>(
    count: usize,
    fields: impl IntoIterator<Item = &'a str>,
) -> Result<(), SessionError> {
    if count > MAX_PLUGIN_CLASSES {
        return Err(failure("plugin metadata contains too many classes"));
    }
    let mut total = 0usize;
    for field in fields {
        if field.len() > MAX_PLUGIN_FIELD_BYTES {
            return Err(failure("plugin metadata contains an oversized field"));
        }
        total = total
            .checked_add(field.len())
            .ok_or_else(|| failure("plugin metadata size overflowed"))?;
        if total > MAX_PLUGIN_METADATA_BYTES {
            return Err(failure("plugin metadata exceeded its safety limit"));
        }
    }
    Ok(())
}

fn validate_clap_plugins(plugins: &[ClapPluginInfoWire]) -> Result<(), SessionError> {
    validate_fields(
        plugins.len(),
        plugins.iter().flat_map(|plugin| {
            [
                plugin.clap_id.as_str(),
                plugin.name.as_str(),
                plugin.vendor.as_str(),
                plugin.description.as_str(),
                plugin.version.as_str(),
            ]
        }),
    )
}

fn validate_vst3_plugins(plugins: &[Vst3PluginInfoWire]) -> Result<(), SessionError> {
    validate_fields(
        plugins.len(),
        plugins.iter().flat_map(|plugin| {
            [
                plugin.class_id.as_str(),
                plugin.name.as_str(),
                plugin.vendor.as_str(),
                plugin.version.as_str(),
            ]
        }),
    )
}

fn execute_worker(request: WorkerRequest) -> Result<WorkerResponse, String> {
    match request {
        WorkerRequest::Discover { extra_paths } => {
            let files = crate::session::scan_installed_plugin_files(&extra_paths)?;
            validate_discovery(&files).map_err(|error| error.to_string())?;
            Ok(WorkerResponse::Discovery(files))
        }
        WorkerRequest::Probe {
            format: PluginFormat::Clap,
            path,
        } => {
            // SAFETY: this is the isolated process created specifically to inspect the file the
            // user opened. A malformed plugin can terminate this child, not the application.
            let library = unsafe { auris_clap::ClapLibrary::load(path) }
                .map_err(|error| error.to_string())?;
            let plugins: Vec<_> = library
                .plugins()
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(Into::into)
                .collect();
            validate_clap_plugins(&plugins).map_err(|error| error.to_string())?;
            Ok(WorkerResponse::Clap(plugins))
        }
        WorkerRequest::Probe {
            format: PluginFormat::Vst3,
            path,
        } => {
            let plugins: Vec<_> = auris_vst3::plugins_in(&path)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(Into::into)
                .collect();
            validate_vst3_plugins(&plugins).map_err(|error| error.to_string())?;
            Ok(WorkerResponse::Vst3(plugins))
        }
    }
}

/// Handles the private plugin-worker invocation before a frontend opens its UI or transport.
///
/// `None` means ordinary application startup. Otherwise the executable should exit with the
/// returned status. Third-party native code is loaded only on this process's main thread.
pub fn handle_plugin_discovery_worker() -> Option<i32> {
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
    let result: Result<WorkerResponse, String> = (|| {
        let bytes = read_message(Path::new(&request))?;
        let request: WorkerRequest = serde_json::from_slice(&bytes).map_err(failure)?;
        execute_worker(request).map_err(failure)
    })()
    .map_err(|error: SessionError| error.to_string());
    let written = serde_json::to_vec(&result)
        .map_err(failure)
        .and_then(|bytes| {
            if bytes.len() as u64 > MAX_MESSAGE_BYTES {
                return Err(failure("plugin worker response exceeds 32 MiB"));
            }
            fs::write(&response, bytes).map_err(failure)
        });
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
            "auris-plugin-worker-{}-{stamp}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(failure)?;
        Ok(Self(path))
    }
}

impl Drop for JobDirectory {
    fn drop(&mut self) {
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

fn run_worker(
    request: WorkerRequest,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<WorkerResponse, SessionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(failure("plugin discovery cancelled"));
    }
    let timeout = timeout
        .max(Duration::from_millis(100))
        .min(MAX_WORKER_DURATION);
    let directory = JobDirectory::new()?;
    let input = directory.0.join("request.json");
    let output = directory.0.join("response.json");
    let bytes = serde_json::to_vec(&request).map_err(failure)?;
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(failure("plugin worker request exceeds 32 MiB"));
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
        command.creation_flags(0x08000000);
    }
    let mut worker = Worker(command.spawn().map_err(failure)?);
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(failure("plugin discovery cancelled"));
        }
        if started.elapsed() >= timeout {
            return Err(failure("plugin discovery worker timed out"));
        }
        if let Some(status) = worker.0.try_wait().map_err(failure)? {
            if !status.success() {
                return Err(failure(format!("plugin discovery worker failed: {status}")));
            }
            let bytes = read_message(&output)?;
            let result: Result<WorkerResponse, String> =
                serde_json::from_slice(&bytes).map_err(failure)?;
            return result.map_err(failure);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_validation_rejects_an_oversized_field() {
        let plugin = ClapPluginInfoWire {
            clap_id: "a".repeat(MAX_PLUGIN_FIELD_BYTES + 1),
            name: String::new(),
            vendor: String::new(),
            description: String::new(),
            version: String::new(),
            kind: PluginKind::Effect,
            category: PluginCategory::Utility,
        };

        let error = validate_clap_plugins(&[plugin]).unwrap_err();

        assert!(error.to_string().contains("oversized field"));
    }

    #[test]
    fn worker_rejects_a_response_kind_that_does_not_match_the_request() {
        let files = InstalledPluginFiles::default();

        let error = match (PluginFormat::Clap, WorkerResponse::Discovery(files)) {
            (PluginFormat::Clap, WorkerResponse::Clap(_)) => None,
            _ => Some(failure("plugin worker returned the wrong response kind")),
        }
        .unwrap();

        assert!(error.to_string().contains("wrong response kind"));
    }

    #[test]
    fn discovery_worker_reports_a_missing_configured_root() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing-plugin-folder");

        let error = execute_worker(WorkerRequest::Discover {
            extra_paths: vec![missing.clone()],
        })
        .unwrap_err();

        assert!(error.contains("configured plugin path"));
        assert!(error.contains(&missing.display().to_string()));
    }

    #[test]
    fn a_fifth_probe_waits_for_a_slot_instead_of_becoming_unreadable() {
        let active = AtomicUsize::new(MAX_ACTIVE_PLUGIN_PROBES);
        let cancelled = AtomicBool::new(false);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(25));
                active.fetch_sub(1, Ordering::AcqRel);
            });
            let permit =
                ProbePermit::acquire(&active, &cancelled, Duration::from_millis(250)).unwrap();

            assert_eq!(active.load(Ordering::Acquire), MAX_ACTIVE_PLUGIN_PROBES);
            drop(permit);
        });
        assert_eq!(active.load(Ordering::Acquire), MAX_ACTIVE_PLUGIN_PROBES - 1);
    }

    #[test]
    fn catalog_deadline_expires_before_starting_a_probe_process() {
        let cancelled = AtomicBool::new(false);
        let job = PluginProbeJob::new(PluginFormat::Clap, PathBuf::from("NeverLoad.clap"));

        let error = job.run_until(&cancelled, Instant::now()).unwrap_err();

        assert!(error.to_string().contains("catalog deadline"));
    }
}
