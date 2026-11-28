//! Run a notification command with a deadline and cleanup of timed-out descendants.
use anyhow::{Context, Result};
use std::process::ExitStatus;
use std::time::Duration;

#[cfg(unix)]
struct ProcessGroup {
    pid: i32,
    keep: bool,
}

#[cfg(unix)]
impl ProcessGroup {
    fn terminate(&self) {
        // SAFETY: this PID is the dedicated process group created for our child.
        unsafe { libc::kill(-self.pid, libc::SIGKILL) };
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if !self.keep {
            self.terminate();
        }
    }
}

pub async fn run(mut command: tokio::process::Command, limit: Duration) -> Result<ExitStatus> {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        // No user code runs until its dedicated Job has been attached.
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let mut child = command.spawn().context("start previous notifier")?;
    #[cfg(windows)]
    let job = {
        let pid = child.id().context("notifier PID missing")?;
        let job = crate::reaper::PaneJob::attach(pid).context("attach notifier cleanup Job")?;
        resume(pid).context("resume notifier after Job assignment")?;
        job
    };
    #[cfg(unix)]
    let mut group = ProcessGroup {
        pid: child.id().context("notifier PID missing")? as i32,
        keep: false,
    };

    match tokio::time::timeout(limit, child.wait()).await {
        Ok(result) => {
            let status = result.context("wait for previous notifier")?;
            if status.success() {
                // Successful notifiers may intentionally launch a persistent app.
                #[cfg(windows)]
                job.preserve_descendants()?;
                #[cfg(unix)]
                {
                    group.keep = true;
                }
            }
            Ok(status)
        }
        Err(_) => {
            #[cfg(windows)]
            job.terminate();
            #[cfg(unix)]
            group.terminate();
            let _ = child.kill().await;
            let _ = child.wait().await;
            anyhow::bail!("previous notifier exceeded {} ms", limit.as_millis());
        }
    }
}

#[cfg(windows)]
fn resume(pid: u32) -> std::io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: handles are owned locally, and only our suspended child's thread is resumed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let snapshot = OwnedHandle::from_raw_handle(snapshot);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry) != 0;
        while found {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let thread = OwnedHandle::from_raw_handle(thread);
                if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                    return Err(std::io::Error::last_os_error());
                }
                return Ok(());
            }
            found = Thread32Next(snapshot.as_raw_handle(), &mut entry) != 0;
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "suspended notifier thread missing",
    ))
}
