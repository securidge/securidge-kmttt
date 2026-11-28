//! 고아 프로세스 회수와 호출자 식별 보강.
//!
//! pane 안의 에이전트가 `( kmttt pane run … & )`처럼 부모를 끊으면(이중 fork) 그 프로세스는
//! pane 프로세스 트리에서 빠져, 계보만 보는 호출자 식별을 속일 수 있다.
//!
//! Linux에서는 서버를 child subreaper(`PR_SET_CHILD_SUBREAPER`)로 만들어 pane 자손 고아가
//! init 대신 서버 밑으로 오게 한다. 그러면 "계보가 서버에 닿지만 pane 루트를 거치지 않는"
//! 호출자를 pane 출신으로 볼 수 있다. 대신 서버가 그 고아들의 좀비를 직접 회수해야 한다.
//!
//! 서버가 직접 띄운 자식(pane 루트, git, 플러그인)의 종료 상태를 회수기가 가로채면 안 되므로,
//! 직접 띄울 때는 [`spawn_lock`]을 잡은 채로 띄우고 [`register`]로 PID를 알린다.
//! 회수기도 같은 잠금을 잡고 돌기 때문에 "띄웠지만 아직 등록 전"인 틈이 없다.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn own() -> &'static Mutex<HashSet<u32>> {
    static OWN: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    OWN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 직접 자식을 띄우는 동안 잡는 잠금. 반환된 가드로 [`SpawnGuard::register`]를 부른다.
pub struct SpawnGuard(MutexGuard<'static, HashSet<u32>>);

impl SpawnGuard {
    pub fn register(&mut self, pid: Option<u32>) {
        if let Some(p) = pid {
            self.0.insert(p);
        }
    }
}

pub fn spawn_lock() -> SpawnGuard {
    SpawnGuard(own().lock().unwrap_or_else(|e| e.into_inner()))
}

/// 직접 자식의 wait가 끝난 뒤 부른다(같은 PID가 재사용될 때 회수 누락을 막는다).
pub fn unregister(pid: Option<u32>) {
    if let Some(p) = pid {
        own().lock().unwrap_or_else(|e| e.into_inner()).remove(&p);
    }
}

/// 서버 시작 때 한 번 부른다. subreaper가 켜졌으면 true.
pub fn become_subreaper() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: 인자가 정수뿐인 prctl 호출이다.
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// 등록되지 않은(=입양된) 자식 가운데 끝난 것들을 회수한다. 회수한 개수를 돌려준다.
pub fn reap_adopted() -> usize {
    #[cfg(target_os = "linux")]
    {
        let guard = own().lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for pid in direct_children() {
            if guard.contains(&pid) {
                continue;
            }
            let mut status = 0;
            // SAFETY: 특정 PID에 대한 비차단 waitpid. 아직 살아 있으면 0을 돌려준다.
            let r = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
            if r == pid as libc::pid_t {
                n += 1;
            }
        }
        n
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
fn direct_children() -> Vec<u32> {
    let me = std::process::id();
    let mut out = Vec::new();
    // 빠른 길: /proc/self/task/*/children (CONFIG_PROC_CHILDREN)
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        let mut any = false;
        for t in tasks.flatten() {
            if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
                any = true;
                out.extend(s.split_whitespace().filter_map(|x| x.parse::<u32>().ok()));
            }
        }
        if any {
            return out;
        }
    }
    // 느린 길: /proc/<pid>/stat의 ppid가 나인 것
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                // "pid (comm) state ppid ..." — comm에 공백·괄호가 있을 수 있어 마지막 ')' 뒤를 본다.
                if let Some(rest) = stat.rsplit_once(')').map(|x| x.1) {
                    let ppid = rest
                        .split_whitespace()
                        .nth(1)
                        .and_then(|x| x.parse::<u32>().ok());
                    if ppid == Some(me) {
                        out.push(pid);
                    }
                }
            }
        }
    }
    out
}

/// 호출자 프로세스 환경 변수에서 `KMTTT_PANE` 값을 읽는다(보조 신호 — 에이전트가 지울 수 있다).
pub fn pane_env_of(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        raw.split(|b| *b == 0).find_map(|kv| {
            kv.strip_prefix(b"KMTTT_PANE=")
                .map(|v| String::from_utf8_lossy(v).into_owned())
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        pane_env_sysinfo(pid)
    }
}

/// sysinfo로 환경 변수를 읽는 길(macOS·Windows용; Linux에서도 컴파일·테스트된다).
pub fn pane_env_sysinfo(pid: u32) -> Option<String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new().ok()?;
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[p]),
        false,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    let proc_ = sys.process(p)?;
    proc_.environ().iter().find_map(|kv| {
        kv.to_str()
            .and_then(|s| s.strip_prefix("KMTTT_PANE="))
            .map(str::to_string)
    })
}

/// pane 하나의 프로세스들을 OS 차원에서 묶어 두는 표식.
///
/// Windows: pane 루트를 전용 Job Object에 넣는다. 자식은 부모를 끊어도 Job에서 못 나가므로
/// (breakaway 미허용) `IsProcessInJob`으로 pane 출신인지 확실히 알 수 있다. 다른 OS에서는 빈 표식이다.
pub struct PaneJob {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: Job 핸들은 프로세스 전역 커널 객체 핸들이라 스레드 간에 옮겨도 된다.
#[cfg(windows)]
unsafe impl Send for PaneJob {}
#[cfg(windows)]
unsafe impl Sync for PaneJob {}

impl PaneJob {
    /// 방금 띄운 pane 루트 프로세스를 새 Job에 넣는다. 실패하면 None(식별은 다른 방법으로).
    #[cfg(windows)]
    pub fn attach(pid: u32) -> Option<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        // SAFETY: 이 함수에서 연 핸들만 쓰고, 실패 경로에서 모두 닫는다.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                CloseHandle(job);
                return None;
            }
            let proc_ = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if proc_.is_null() {
                CloseHandle(job);
                return None;
            }
            let ok = AssignProcessToJobObject(job, proc_) != 0;
            CloseHandle(proc_);
            if !ok {
                CloseHandle(job);
                return None;
            }
            Some(Self { job })
        }
    }

    #[cfg(not(windows))]
    pub fn attach(_pid: u32) -> Option<Self> {
        None
    }

    /// `pid`가 이 pane의 Job 안에 있는지.
    #[cfg(windows)]
    pub fn contains(&self, pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: 연 프로세스 핸들은 바로 닫는다.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut r = 0;
            let ok = IsProcessInJob(h, self.job, &mut r) != 0;
            CloseHandle(h);
            ok && r != 0
        }
    }

    #[cfg(not(windows))]
    pub fn contains(&self, _pid: u32) -> bool {
        false
    }

    /// Stop all pane processes now, even if identification still holds an Arc.
    pub fn terminate(&self) {
        #[cfg(windows)]
        {
            // SAFETY: attach owns this live Job handle until Drop.
            if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1) }
                == 0
            {
                tracing::warn!(error = %std::io::Error::last_os_error(), "terminate pane job failed");
            }
        }
    }

    /// Only the bounded notifier runner uses this after a successful exit.
    /// Its intentional background children remain in the enclosing pane/sandbox.
    #[cfg(windows)]
    pub(crate) fn preserve_descendants(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };
        // SAFETY: only this Job's kill-on-close flag is cleared, not any enclosing Job.
        unsafe {
            let limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            if SetInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for PaneJob {
    fn drop(&mut self) {
        // SAFETY: attach에서 만든 핸들을 한 번만 닫는다. 마지막 핸들이 닫히면 자손도 종료된다.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

/// macOS: 부모를 끊고 launchd(PID 1) 밑으로 간 데다 제어 터미널도 없는 프로세스인지.
/// 사람이 터미널에서 실행한 CLI는 살아 있는 셸이 부모이고 제어 터미널이 있다.
/// 다른 OS에서는 false(Linux는 subreaper, Windows는 Job으로 따로 확인한다).
pub fn detached_without_tty(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        const PROC_FLAG_CONTROLT: u32 = 0x2; // <sys/proc_info.h>
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: 크기를 맞춘 구조체 버퍼에 커널이 채운다.
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if n != size {
            return false;
        }
        info.pbi_ppid == 1 && info.pbi_flags & PROC_FLAG_CONTROLT == 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        false
    }
}

/// Windows: 호출자에서 위로 올라가며, 터미널·데스크톱 호스트보다 OS 실행 서비스(WMI, 작업
/// 스케줄러, 서비스 관리자)를 먼저 만나면 사람이 터미널에서 친 명령이 아니라 우회 실행으로 본다.
/// (Windows Terminal 같은 패키지 앱도 svchost가 띄우므로, 서비스가 "있는지"가 아니라 "먼저인지"를 본다.)
/// `chain_names`는 호출자 자신부터 조상 순서다.
pub fn launched_by_os_service(chain_names: &[String], trusted: &[String]) -> bool {
    const SERVICES: &[&str] = &["wmiprvse", "taskeng", "taskhostw", "services", "svchost"];
    const HOSTS: &[&str] = &[
        "windowsterminal",
        "openconsole",
        "conhost",
        "explorer",
        "code",
        "sshd",
        "conemu64",
        "alacritty",
        "wezterm-gui",
        "tabby",
        "hyper",
    ];
    if !cfg!(windows) {
        return false;
    }
    for n in chain_names {
        let n = n.to_ascii_lowercase();
        let n = n.trim_end_matches(".exe");
        if HOSTS.contains(&n)
            || trusted
                .iter()
                .any(|t| t.to_ascii_lowercase().trim_end_matches(".exe") == n)
        {
            return false;
        }
        if SERVICES.contains(&n) {
            return true;
        }
    }
    false
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn detects_detached_process() {
        // 이중 fork + setsid: 손자는 launchd 밑으로 가고 제어 터미널이 없다.
        let out = std::process::Command::new("python3")
            .args([
                "-c",
                "import os,sys,time\nr,w=os.pipe()\nif os.fork()==0:\n  os.setsid()\n  if os.fork()==0:\n    n=os.open(os.devnull,os.O_RDWR)\n    for f in (0,1,2): os.dup2(n,f)\n    os.write(w,str(os.getpid()).encode()); os.close(w); time.sleep(6); os._exit(0)\n  os._exit(0)\nos.close(w); os.wait(); print(os.read(r,32).decode())",
            ])
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            detached_without_tty(pid),
            "orphaned setsid process should be detected"
        );
        // 이 테스트 프로세스는 부모(cargo)가 살아 있다.
        assert!(!detached_without_tty(std::process::id()));
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn job_drop_and_explicit_termination_stop_processes() {
        use std::os::windows::process::CommandExt;
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        for (explicit, nested) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut child = std::process::Command::new("ping.exe")
                .args(["-n", "60", "127.0.0.1"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let job = Arc::new(PaneJob::attach(child.id()).expect("assign test process"));
            assert!(job.contains(child.id()));
            assert!(!job.contains(std::process::id()));
            // A live inner Job must not keep processes alive after the pane ends.
            let nested_job = nested.then(|| PaneJob::attach(child.id()).expect("nested job"));
            if let Some(nested_job) = &nested_job {
                assert!(nested_job.contains(child.id()));
                assert!(job.contains(child.id()));
            }
            // Model identify() retaining a reference while close_pane runs.
            let observer = explicit.then(|| Arc::clone(&job));
            if explicit {
                job.terminate();
            }
            drop(job);
            let deadline = Instant::now() + Duration::from_secs(5);
            let exited = loop {
                if child.try_wait().unwrap().is_some() {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            // Also clean up if a deliberately broken implementation is tested.
            if !exited {
                let _ = child.kill();
                let _ = child.wait();
            }
            assert!(
                exited,
                "pane Job did not end its processes (explicit={explicit}, nested={nested})"
            );
            drop(observer);
            drop(nested_job);
        }
    }

    #[test]
    fn job_tracks_detached_grandchild() {
        // pane 루트 역할의 PowerShell이 1초 뒤 손자(ping)를 띄우고 PID를 알린 뒤 끝난다.
        // 손자는 부모가 사라져도 Job 안에 남아야 한다.
        let root = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Start-Sleep -Milliseconds 1500; $p = Start-Process ping -ArgumentList '-n','8','127.0.0.1' -PassThru -WindowStyle Hidden; $p.Id",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let job = PaneJob::attach(root.id()).expect("assign to job");
        assert!(job.contains(root.id()));
        assert!(!job.contains(std::process::id()));
        let out = root.wait_with_output().unwrap();
        let child: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert!(
            job.contains(child),
            "detached grandchild must stay in the pane job"
        );
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &child.to_string()])
            .output();
    }

    #[test]
    fn service_lineage_is_flagged() {
        let chain = vec!["kmttt.exe".to_string(), "WmiPrvSE.exe".to_string()];
        assert!(launched_by_os_service(&chain, &[]));
        let task = vec![
            "kmttt.exe".to_string(),
            "cmd.exe".to_string(),
            "svchost.exe".to_string(),
        ];
        assert!(launched_by_os_service(&task, &[]));
        let packaged = vec![
            "kmttt.exe".to_string(),
            "pwsh.exe".to_string(),
            "WindowsTerminal.exe".to_string(),
            "svchost.exe".to_string(),
        ];
        assert!(!launched_by_os_service(&packaged, &[]));
        let human = vec![
            "kmttt.exe".to_string(),
            "pwsh.exe".to_string(),
            "WindowsTerminal.exe".to_string(),
            "explorer.exe".to_string(),
        ];
        assert!(!launched_by_os_service(&human, &[]));
        let ci = vec![
            "kmttt.exe".to_string(),
            "pwsh.exe".to_string(),
            "Runner.Worker.exe".to_string(),
            "Runner.Listener.exe".to_string(),
            "svchost.exe".to_string(),
            "services.exe".to_string(),
        ];
        assert!(launched_by_os_service(&ci, &[]));
        assert!(!launched_by_os_service(
            &ci,
            &["Runner.Listener".to_string()]
        ));
        let wmi = vec![
            "kmttt.exe".to_string(),
            "WmiPrvSE.exe".to_string(),
            "svchost.exe".to_string(),
        ];
        assert!(launched_by_os_service(
            &wmi,
            &["Runner.Listener".to_string()]
        ));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn reads_own_env_and_children() {
        let mut g = spawn_lock();
        let mut child = std::process::Command::new("sleep")
            .arg("2")
            .env("KMTTT_PANE", "w9:p9")
            .spawn()
            .unwrap();
        g.register(Some(child.id()));
        drop(g);
        // exec 직후 아주 잠깐은 /proc/<pid>/environ이 비어 있을 수 있다.
        let mut env = None;
        for _ in 0..50 {
            env = pane_env_of(child.id());
            if env.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(env.as_deref(), Some("w9:p9"));
        assert_eq!(pane_env_sysinfo(child.id()).as_deref(), Some("w9:p9"));
        assert!(direct_children().contains(&child.id()));
        // 등록된 자식은 회수기가 건드리지 않는다.
        assert_eq!(reap_adopted(), 0);
        child.kill().unwrap();
        child.wait().unwrap();
        unregister(Some(child.id()));
    }
}
