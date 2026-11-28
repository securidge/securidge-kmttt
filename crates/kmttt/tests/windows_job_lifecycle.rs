//! Real temporary servers exercise ConPTY, nested sandbox Jobs and server crashes.
#![cfg(windows)]

use kmttt_core::client::RpcClient;
use serde_json::{json, Value};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE,
};

struct ServerFixture {
    server: Child,
    root: tempfile::TempDir,
    session: String,
}

impl ServerFixture {
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[shell]\nprogram = 'powershell.exe'\nargs = ['-NoLogo', '-NoProfile']\n",
        )
        .unwrap();
        let session = format!(
            "job-test-{}-{}",
            std::process::id(),
            root.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace('.', "")
        );
        let server = Command::new(env!("CARGO_BIN_EXE_kmttt"))
            .args(["-s", &session, "server", "run"])
            .env("KMTTT_CONFIG_DIR", &config)
            .env("KMTTT_DATA_DIR", root.path().join("data"))
            .env("KMTTT_CLAUDE_SETTINGS", root.path().join("settings.json"))
            .env("KMTTT_CODEX_CONFIG", root.path().join("config.toml"))
            .current_dir(root.path())
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            server,
            root,
            session,
        }
    }

    async fn connect(&mut self) -> RpcClient {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok((client, _)) = RpcClient::connect_local(&self.session).await {
                return client;
            }
            assert!(self.server.try_wait().unwrap().is_none(), "server exited");
            assert!(Instant::now() < deadline, "server did not become ready");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for ServerFixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

/// Retain the actual process handle, so cleanup cannot kill a reused PID.
struct ProbeProcess(OwnedHandle);

impl ProbeProcess {
    fn open(pid: u32) -> Self {
        // SAFETY: request only termination/wait rights for the fixture's child PID.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
        assert!(
            !handle.is_null(),
            "open probe child: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: OpenProcess returned a new owned handle.
        Self(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    fn wait(&self, millis: u32) -> u32 {
        // SAFETY: the owned handle is valid and has SYNCHRONIZE access.
        unsafe { WaitForSingleObject(self.0.as_raw_handle(), millis) }
    }
}

impl Drop for ProbeProcess {
    fn drop(&mut self) {
        if self.wait(0) == WAIT_TIMEOUT {
            // SAFETY: terminate only this fixture's retained process handle on failure.
            unsafe { TerminateProcess(self.0.as_raw_handle(), 1) };
            self.wait(5000);
        }
    }
}

fn ps_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

#[derive(Debug, Clone, Copy)]
enum Ending {
    Close,
    ShellExit,
    ServerCrash,
}

async fn check_lifecycle(profile: &str, ending: Ending) {
    eprintln!("checking {profile}/{ending:?}");
    let mut fixture = ServerFixture::start();
    let client = fixture.connect().await;
    let ready = fixture.root.path().join("child.pid");
    let release = fixture.root.path().join("release");
    let finish = match ending {
        Ending::Close => "& $env:KMTTT_EXE -s $env:KMTTT_SESSION pane close $env:KMTTT_PANE",
        Ending::ShellExit => "exit",
        Ending::ServerCrash => "Start-Sleep -Seconds 60",
    };
    let command = format!(
        "& {{ $p = Start-Process ping.exe -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
         [IO.File]::WriteAllText({}, [string]$p.Id); \
         while (!(Test-Path -LiteralPath {})) {{ Start-Sleep -Milliseconds 50 }}; {} }}",
        ps_quote(&ready), ps_quote(&release), finish
    );
    // Default policy permits creating a fixture. Closing is requested from the
    // genuine pane itself, so no policy relaxation or forged identity is needed.
    let created = client
        .call(
            "workspace.create",
            json!({
                "cwd": fixture.root.path(), "command": command, "profile": profile
            }),
        )
        .await
        .unwrap();
    let pane = created["pane"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let child_pid = loop {
        if let Ok(pid) = std::fs::read_to_string(&ready) {
            if let Ok(pid) = pid.trim().parse() {
                break pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "child not ready ({profile}, {ending:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let child = ProbeProcess::open(child_pid);
    assert_eq!(child.wait(0), WAIT_TIMEOUT, "probe must be alive initially");
    if matches!(ending, Ending::ServerCrash) {
        // TerminateProcess skips Rust Drop and orderly server cleanup. Only
        // kernel handle closure can enforce the pane Job's crash lifetime.
        fixture.server.kill().unwrap();
        fixture.server.wait().unwrap();
    } else {
        std::fs::write(&release, "go").unwrap();
        loop {
            let panes: Value = client.call("pane.list", json!({})).await.unwrap();
            if !panes.as_array().unwrap().iter().any(|p| p["id"] == pane) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "pane did not close ({profile}, {ending:?})"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    assert_eq!(
        child.wait(5000),
        WAIT_OBJECT_0,
        "detached child survived {profile}/{ending:?}"
    );
}

#[tokio::test]
async fn closing_pane_terminates_detached_children() {
    for profile in ["none", "restricted"] {
        check_lifecycle(profile, Ending::Close).await;
    }
}

#[tokio::test]
async fn shell_exit_terminates_detached_children() {
    for profile in ["none", "restricted"] {
        check_lifecycle(profile, Ending::ShellExit).await;
    }
}

#[tokio::test]
async fn server_crash_terminates_detached_children() {
    for profile in ["none", "restricted"] {
        check_lifecycle(profile, Ending::ServerCrash).await;
    }
}
