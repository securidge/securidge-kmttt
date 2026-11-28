//! 플랫폼별 PTY와 프로세스 트리.
//!
//! PTY는 portable-pty를 쓴다(Windows는 ConPTY, Unix는 openpty).
//! 출력은 전용 스레드에서 읽어 tokio 채널로 넘기고, 프로세스 종료는 다른 스레드에서 기다린다.

pub mod bounded;
pub mod procs;
pub mod reaper;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub use procs::{ProcEntry, ProcTable};

/// 새 pane에서 실행할 명령.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

/// PTY에서 오는 소식.
#[derive(Debug)]
pub enum PtyEvent {
    Output(Vec<u8>),
    /// 자식 프로세스 종료(종료 코드)
    Exited(i32),
}

/// 실행 중인 PTY 하나.
pub struct PtyProcess {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
}

impl PtyProcess {
    /// 명령을 PTY에서 실행하고, 출력·종료 소식을 보낼 채널을 돌려준다.
    pub fn spawn(spec: &SpawnSpec) -> Result<(Self, mpsc::UnboundedReceiver<PtyEvent>)> {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        // 회수기가 이 자식의 종료 상태를 가로채지 않도록 잠금 안에서 띄우고 등록한다.
        let mut guard = reaper::spawn_lock();
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {}", spec.program))?;
        let pid = child.process_id();
        guard.register(pid);
        drop(guard);
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;
        let (tx, rx) = mpsc::unbounded_channel();

        let tx_out = tx.clone();
        // 읽기 스레드가 끝나면 알려 준다: 종료 소식이 마지막 출력보다 먼저 가지 않게.
        let (read_done_tx, read_done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("kmttt-pty-read".into())
            .spawn(move || {
                let _done = read_done_tx;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx_out.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .context("reader thread")?;
        std::thread::Builder::new()
            .name("kmttt-pty-wait".into())
            .spawn(move || {
                let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
                reaper::unregister(pid);
                // 백그라운드 자손이 PTY를 붙잡고 있으면 EOF가 안 오므로 오래 기다리지는 않는다.
                let _ = read_done_rx.recv_timeout(std::time::Duration::from_millis(300));
                let _ = tx.send(PtyEvent::Exited(code));
            })
            .context("wait thread")?;

        Ok((
            Self {
                master: pair.master,
                writer: Arc::new(Mutex::new(writer)),
                killer,
                pid,
            },
            rx,
        ))
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// PTY에 입력을 쓴다.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("writer poisoned"))?;
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize")
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }

    /// Unix에서 터미널의 전면 프로세스 그룹 리더 PID.
    #[cfg(unix)]
    pub fn foreground_pgid(&self) -> Option<u32> {
        self.master.process_group_leader().map(|p| p as u32)
    }

    /// Windows에는 "전면 프로세스 그룹"이 없어 프로세스 트리로 추정한다.
    #[cfg(not(unix))]
    pub fn foreground_pgid(&self) -> Option<u32> {
        None
    }
}

/// 플랫폼 기본 셸.
pub fn default_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        if which("pwsh.exe") {
            return ("pwsh.exe".into(), vec!["-NoLogo".into()]);
        }
        ("powershell.exe".into(), vec!["-NoLogo".into()])
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        (shell, vec!["-l".into()])
    }
}

/// PATH에서 실행 파일을 찾을 수 있는지.
pub fn which(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn runs_command_and_reports_exit() {
        let spec = SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf 'hi-from-pty'; exit 3".into()],
            cwd: None,
            env: vec![],
            rows: 10,
            cols: 40,
        };
        let (p, mut rx) = PtyProcess::spawn(&spec).unwrap();
        assert!(p.pid().is_some());
        let mut out = Vec::new();
        let mut code = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while code.is_none() || !String::from_utf8_lossy(&out).contains("hi-from-pty") {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(PtyEvent::Output(b))) => out.extend(b),
                Ok(Some(PtyEvent::Exited(c))) => code = Some(c),
                Ok(None) | Err(_) => break,
            }
        }
        assert!(String::from_utf8_lossy(&out).contains("hi-from-pty"));
        assert_eq!(code, Some(3));
    }

    #[tokio::test]
    async fn input_echo_and_resize() {
        let spec = SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![],
            cwd: None,
            env: vec![("PS1".into(), "$ ".into())],
            rows: 10,
            cols: 40,
        };
        let (p, mut rx) = PtyProcess::spawn(&spec).unwrap();
        p.resize(20, 100).unwrap();
        p.write(b"stty size; exit\n").unwrap();
        let mut out = String::new();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            match ev {
                PtyEvent::Output(b) => out.push_str(&String::from_utf8_lossy(&b)),
                PtyEvent::Exited(_) => break,
            }
        }
        assert!(out.contains("20 100"), "{out}");
    }
}

/// Windows ConPTY: 출력이 실제로 나오는지(커서 위치 질의에 답해야 나온다).
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn run_until_exit(spec: SpawnSpec, input: Option<&str>) -> String {
        let (pty, mut rx) = PtyProcess::spawn(&spec).expect("spawn");
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut sent = false;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            while std::time::Instant::now() < deadline {
                match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
                    Ok(Some(PtyEvent::Output(b))) => {
                        if b.windows(4).any(|w| w == b"\x1b[6n") {
                            let _ = pty.write(b"\x1b[1;1R");
                        }
                        out.extend_from_slice(&b);
                    }
                    Ok(Some(PtyEvent::Exited(_))) | Ok(None) => break,
                    Err(_) => {}
                }
                if let (Some(text), false) = (input, sent) {
                    if !out.is_empty() {
                        let _ = pty.write(text.as_bytes());
                        sent = true;
                    }
                }
            }
        });
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn conpty_runs_cmd() {
        let out = run_until_exit(
            SpawnSpec {
                program: "cmd.exe".into(),
                args: vec!["/c".into(), "echo PTY-OK".into()],
                cwd: None,
                env: vec![],
                rows: 24,
                cols: 80,
            },
            None,
        );
        assert!(out.contains("PTY-OK"), "output was: {out:?}");
    }

    #[test]
    fn conpty_runs_interactive_powershell() {
        let out = run_until_exit(
            SpawnSpec {
                program: "powershell.exe".into(),
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
                cwd: None,
                env: vec![],
                rows: 24,
                cols: 80,
            },
            Some("Write-Output ('P' + 'S-OK'); exit\r"),
        );
        assert!(out.contains("PS-OK"), "output was: {out:?}");
    }
}
