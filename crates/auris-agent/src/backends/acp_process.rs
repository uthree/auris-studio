//! Process-tree lifetime for ACP adapters.
//!
//! ACP agents may create shell, terminal, or helper descendants. Dropping only the direct
//! Tokio child leaves those descendants running, so each adapter process is placed in an
//! owned process group (Unix) or Job Object (Windows).

use std::io;
use tokio::process::{Child, Command};

/// A child process whose descendants share a lifetime with this value.
pub(crate) struct ProcessScope {
    child: Child,
    #[cfg(windows)]
    job: WinHandle,
    #[cfg(unix)]
    group: Option<i32>,
}

impl ProcessScope {
    /// Spawn `command` in a new process group/job and retain ownership of that scope.
    pub(crate) fn spawn(mut command: Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        command.kill_on_drop(true);
        let child = command.spawn()?;
        #[cfg(windows)]
        {
            let job = WinHandle::for_process(
                child
                    .id()
                    .ok_or_else(|| io::Error::other("ACP child did not expose a process ID"))?,
            )?;
            Ok(Self { child, job })
        }
        #[cfg(not(windows))]
        Ok(Self {
            group: child.id().map(|id| id as i32),
            child,
        })
    }

    /// Access the direct child to take its stdio handles or await it.
    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Kill the entire process tree and wait for the direct child.
    pub(crate) async fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        kill_group(self.group.take());
        #[cfg(windows)]
        self.job.terminate();
        self.child.kill().await
    }

    /// Wait for the direct child while retaining the process-tree scope.
    ///
    /// Keeping the scope alive until this completes ensures descendants remain owned even when
    /// the caller only needs the direct exit status.
    pub(crate) async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }
}

#[cfg(unix)]
impl Drop for ProcessScope {
    fn drop(&mut self) {
        // Dropping a Tokio Child kills the direct process (kill_on_drop above), while the
        // negative PID here also terminates any shell/helper descendants in its group.
        kill_group(self.group);
    }
}

#[cfg(unix)]
fn kill_group(id: Option<i32>) {
    if let Some(id) = id {
        // Keep the crate dependency-free: this is the POSIX kill(2) ABI on supported Unix
        // targets. A negative PID addresses the process group created by process_group(0).
        unsafe {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            let _ = kill(-id, 9);
        }
    }
}

#[cfg(windows)]
struct WinHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
// SAFETY: the handle is exclusively owned; Windows job handles can cross threads.
unsafe impl Send for WinHandle {}

#[cfg(windows)]
impl WinHandle {
    fn for_process(pid: u32) -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{JobObjects::*, Threading::*},
        };
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } != 0;
        if !configured {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let assigned = unsafe { AssignProcessToJobObject(job.0, process) } != 0;
        let error = (!assigned).then(io::Error::last_os_error);
        unsafe {
            CloseHandle(process);
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(job)
    }

    fn terminate(&self) {
        unsafe {
            let _ = windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for WinHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::ProcessScope;
    use std::process::Stdio;
    use tokio::process::Command;

    #[tokio::test]
    async fn dropping_scope_kills_shell_descendant_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]).stdin(Stdio::null());
        let mut scope = ProcessScope::spawn(command).expect("spawn shell");
        let child = scope.child_mut().id().expect("shell pid");
        scope.kill().await.expect("kill process group");
        assert!(scope.wait().await.is_ok());
        assert!(child > 0);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    #[tokio::test]
    async fn dropping_scope_terminates_a_live_grandchild() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Milliseconds 500; $acpChild = Start-Process -FilePath ($env:SystemRoot + '\\System32\\ping.exe') -ArgumentList '127.0.0.1','-n','60' -WindowStyle Hidden -PassThru; Write-Output $acpChild.Id; Wait-Process -Id $acpChild.Id"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(0x08000000);
        let mut scope = ProcessScope::spawn(command).unwrap();
        let mut lines = BufReader::new(scope.child_mut().stdout.take().unwrap()).lines();
        let pid = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        // Retain the descendant handle before closing the job, so PID reuse is irrelevant.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        drop(scope);
        let status = unsafe { WaitForSingleObject(handle, 5000) };
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(status, WAIT_OBJECT_0, "descendant survived the ACP scope");
    }
}
