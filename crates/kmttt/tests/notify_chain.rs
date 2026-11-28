//! Native child probes exercise argv, opt-in, deadlines and Job cleanup without
//! reading real agent settings or contacting a model/server/account.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const CAPTURE: &str = "KMTTT_TEST_NOTIFY_CAPTURE";
const MODE: &str = "KMTTT_TEST_NOTIFY_MODE";

fn background(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[test]
fn native_probe() {
    let Some(file) = std::env::var_os(CAPTURE) else {
        return;
    };
    let file = PathBuf::from(file);
    let mode = std::env::var(MODE).unwrap_or_default();
    let mut data = json!({
        "args": std::env::args().skip(1).collect::<Vec<_>>(),
        "exe": std::env::current_exe().unwrap().canonicalize().unwrap(),
        "guard": std::env::var_os("KMTTT_NOTIFY_CHAIN_ACTIVE").is_some(),
        "report_seen": file.with_file_name("reported").exists(),
    });
    if mode == "tree" || mode == "background" {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "native_probe", "--nocapture"])
            .env(CAPTURE, file.with_extension("child.json"))
            .env(MODE, "hang")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        background(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        data["child_pid"] = json!(child.id());
        // Best-effort reaping while this probe lives; its successful exit must
        // not join the thread or wait for the intentionally persistent app.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        // Parent tests retain a separate process handle and clean it up.
    }
    std::fs::write(&file, data.to_string()).unwrap();
    if mode == "fail" {
        std::process::exit(7);
    }
    if mode == "hang" || mode == "tree" {
        std::thread::sleep(Duration::from_secs(60));
    }
}

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    probe: PathBuf,
    capture: PathBuf,
    session: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("notify & helper ")
            .tempdir()
            .unwrap();
        let probe = root
            .path()
            .join(if cfg!(windows) { "probe.exe" } else { "probe" });
        std::fs::copy(std::env::current_exe().unwrap(), &probe).unwrap();
        let session = format!(
            "chain-{}-{}",
            std::process::id(),
            root.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        );
        Self {
            config: root.path().join("config.toml"),
            capture: root.path().join("capture.json"),
            probe,
            session,
            root,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kmttt"));
        cmd.args(["-s", &self.session])
            .current_dir(self.root.path())
            .env(
                "KMTTT_CLAUDE_SETTINGS",
                self.root.path().join("settings.json"),
            )
            .env("KMTTT_CODEX_CONFIG", &self.config)
            .env("KMTTT_CONFIG_DIR", self.root.path().join("config"))
            .env("KMTTT_DATA_DIR", self.root.path().join("data"))
            .env("KMTTT_SESSION", &self.session)
            .env("KMTTT_SESSION_NAME", &self.session)
            .env("KMTTT_PANE", "fixture-pane")
            .env("KMTTT_EXE", env!("CARGO_BIN_EXE_kmttt"))
            .env("HOME", self.root.path())
            .env("USERPROFILE", self.root.path())
            .env("CODEX_HOME", self.root.path().join("codex"))
            .env("PATH", self.root.path())
            .env(CAPTURE, &self.capture)
            .env(MODE, "record")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        background(&mut cmd);
        cmd
    }

    fn argv(&self) -> Vec<String> {
        vec![
            self.probe.to_str().unwrap().into(),
            "--exact".into(),
            "native_probe".into(),
            "--nocapture".into(),
            "space argument".into(),
            "quote\" slash\\ & % $(literal)".into(),
        ]
    }

    fn configure(&self, argv: &[String]) -> String {
        let text = format!(
            "# keep this\nnotify = {}\n[profiles.work]\nmodel = 'keep'\n",
            serde_json::to_string(argv).unwrap()
        );
        std::fs::write(&self.config, &text).unwrap();
        text
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn sidecar(&self) -> PathBuf {
        self.config
            .with_file_name("config.toml.kmttt-notify-previous.json")
    }

    fn captured(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.capture).unwrap()).unwrap()
    }
}

const PAYLOAD: &str =
    "{ \"type\":\"agent-turn-complete\", \"notify\":[\"ignored\"], \"text\":\"& echo nope\" }";

#[test]
fn shell_forwarding_executables_are_rejected_before_config_writes() {
    let f = Fixture::new();
    for name in [
        "wsl",
        "wslhost",
        "conhost",
        "OpenConsole",
        "wt",
        "WindowsTerminal",
        "winpty",
        "winpty-agent",
        "ssh",
        "plink",
    ] {
        let path = f.root.path().join(format!("{name}.exe"));
        std::fs::copy(&f.probe, &path).unwrap();
        let mut argv = f.argv();
        argv[0] = path.to_str().unwrap().into();
        let original = f.configure(&argv);
        let out = f.cli(&["integration", "install", "codex", "--chain"]);
        assert!(!out.status.success(), "accepted {name}");
        assert_eq!(std::fs::read_to_string(&f.config).unwrap(), original);
        assert!(!f.sidecar().exists());
    }
}

struct TestServer(std::process::Child);

#[test]
fn previously_registered_shell_forwarder_is_rejected_at_hook_runtime() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let proxy = f.root.path().join("ConHost.exe");
    std::fs::copy(&f.probe, &proxy).unwrap();
    let path = proxy.canonicalize().unwrap().to_str().unwrap().to_owned();
    let mut record: Value = serde_json::from_slice(&std::fs::read(f.sidecar()).unwrap()).unwrap();
    let mut previous: Vec<String> =
        serde_json::from_str(record["previous_notify"].as_str().unwrap()).unwrap();
    previous[0] = path.clone();
    record["previous_notify"] = json!(serde_json::to_string(&previous).unwrap());
    record["chain_argv"][0] = json!(path);
    std::fs::write(f.sidecar(), record.to_string()).unwrap();
    let out = f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("shell forwarding"));
    assert!(!f.capture.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn unresponsive_audit_server_does_not_block_previous_notifier() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(kmttt_core::paths::socket_path(&f.session))
        .unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        pipe.connect().await.unwrap();
        let frame = kmttt_proto::read_frame(&mut pipe).await.unwrap().unwrap();
        let request = kmttt_proto::RpcMessage::from_bytes(&frame).unwrap();
        let response =
            kmttt_proto::RpcMessage::response(request.id.unwrap(), Ok(json!({"decision":"allow"})));
        kmttt_proto::write_frame(&mut pipe, &response.to_bytes())
            .await
            .unwrap();
        pipe.disconnect().unwrap();
        pipe.connect().await.unwrap();
        let frame = kmttt_proto::read_frame(&mut pipe).await.unwrap().unwrap();
        let request = kmttt_proto::RpcMessage::from_bytes(&frame).unwrap();
        assert_eq!(request.method.as_deref(), Some("hook.notify_chain"));
        seen_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    let start = Instant::now();
    let mut command = f.command();
    command.args(["hook", "codex", "notify", PAYLOAD]);
    let out = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    server.abort();
    assert!(start.elapsed() < Duration::from_secs(2));
    seen_rx.await.unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(f.capture.exists());
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn previous_notifier_outcomes_reach_server_audit_without_payload_or_argv() {
    use kmttt_core::client::RpcClient;
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let mut server = TestServer(f.command().args(["server", "run"]).spawn().unwrap());
    let started = Instant::now();
    let (client, _notes) = loop {
        if let Ok(c) = RpcClient::connect_local(&f.session).await {
            break c;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "test server not ready: {:?}",
            server.0.try_wait()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    for (mode, outcome) in [
        ("record", "completed"),
        ("fail", "failed"),
        ("hang", "timed_out"),
        ("broken", "failed"),
    ] {
        if mode == "broken" {
            // Header validation still passes, but OS process creation fails.
            std::fs::write(
                &f.probe,
                if cfg!(windows) {
                    &b"MZ!!"[..]
                } else {
                    &b"\x7fELF"[..]
                },
            )
            .unwrap();
        }
        let mut command = f.command();
        command
            .env(MODE, mode)
            .args(["hook", "codex", "notify", PAYLOAD]);
        let out = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert!(out.status.success());
        let snapshot = client.call("control.snapshot", json!({})).await.unwrap();
        let events: Vec<_> = snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| {
                e["kind"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("integration.notify_chain.")
            })
            .collect();
        assert!(
            events
                .iter()
                .any(|e| e["kind"] == format!("integration.notify_chain.{outcome}")),
            "{snapshot}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["kind"] == "integration.notify_chain.started")
                .count(),
            match mode {
                "record" => 1,
                "fail" => 2,
                "hang" => 3,
                _ => 4,
            }
        );
        for e in events {
            assert!(e["caller"].as_str().is_some_and(|s| !s.is_empty()));
            let text = e.to_string();
            assert!(!text.contains("echo nope"));
            assert!(!text.contains("space argument"));
            assert!(
                !text.contains("fixture-pane"),
                "client environment must not forge attribution"
            );
        }
    }
    assert!(client
        .call(
            "hook.notify_chain",
            json!({"phase":"failed", "caller":"user", "detail":"secret"})
        )
        .await
        .is_err());
    // Verify durable ledger output too, rather than only the in-memory view.
    let ledger = f.root.path().join("data").join("ledger").join(&f.session);
    let records: Vec<Value> = std::fs::read_dir(ledger)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("ledger-")
                && e.path().extension().is_some_and(|e| e == "jsonl")
        })
        .flat_map(|e| {
            std::fs::read_to_string(e.path())
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str::<Value>(s).unwrap())
                .collect::<Vec<_>>()
        })
        .filter(|e| {
            e["kind"] == "security.event"
                && e["data"]["kind"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("integration.notify_chain."))
        })
        .collect();
    assert_eq!(records.len(), 8);
    let text = serde_json::to_string(&records).unwrap();
    assert!(!text.contains("echo nope"));
    assert!(!text.contains("space argument"));
    assert_eq!(
        records
            .iter()
            .filter(|e| e["data"]["kind"] == "integration.notify_chain.failed")
            .count(),
        2
    );
}

fn notify_chain_ledger(f: &Fixture) -> Vec<Value> {
    let ledger = f.root.path().join("data").join("ledger").join(&f.session);
    std::fs::read_dir(ledger)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("ledger-")
                && e.path().extension().is_some_and(|e| e == "jsonl")
        })
        .flat_map(|e| {
            std::fs::read_to_string(e.path())
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str::<Value>(s).unwrap())
                .collect::<Vec<_>>()
        })
        .filter(|e| {
            e["kind"] == "security.event"
                && e["data"]["kind"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("integration.notify_chain."))
        })
        .map(|e| e["data"].clone())
        .collect()
}

#[tokio::test]
async fn repeated_notify_chain_audits_are_throttled_but_failures_are_kept() {
    use kmttt_core::client::RpcClient;
    let f = Fixture::new();
    let config_dir = f.root.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("policy.toml"),
        "[defaults]
rate_per_min = 3
",
    )
    .unwrap();
    let mut server = TestServer(f.command().args(["server", "run"]).spawn().unwrap());
    let started = Instant::now();
    let (client, _notes) = loop {
        if let Ok(c) = RpcClient::connect_local(&f.session).await {
            break c;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "test server not ready: {:?}",
            server.0.try_wait()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let call = |phase: &'static str| {
        let client = &client;
        async move {
            client
                .call("hook.notify_chain", json!({ "phase": phase }))
                .await
                .unwrap()
        }
    };
    for phase in ["started", "completed", "started"] {
        assert_eq!(call(phase).await["recorded"], true, "{phase}");
    }
    for phase in ["completed", "started", "completed", "completed"] {
        let r = call(phase).await;
        assert_eq!(r["recorded"], false, "{phase}");
        assert_eq!(r["suppressed"], true, "{phase}");
    }
    for phase in ["failed", "timed_out"] {
        assert_eq!(call(phase).await["recorded"], true, "{phase}");
    }
    // The first failure is the caller's next accepted record, so the pending
    // summary is written to the ledger just before it.
    let records = notify_chain_ledger(&f);
    let count = |kind: &str| {
        records
            .iter()
            .filter(|e| e["kind"] == format!("integration.notify_chain.{kind}"))
            .count()
    };
    assert_eq!(count("started"), 2, "{records:?}");
    assert_eq!(count("completed"), 1, "{records:?}");
    assert_eq!(count("failed"), 1, "{records:?}");
    assert_eq!(count("timed_out"), 1, "{records:?}");
    assert_eq!(count("suppressed"), 1, "{records:?}");
    let kinds: Vec<_> = records
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    let summary = kinds
        .iter()
        .position(|k| *k == "integration.notify_chain.suppressed")
        .unwrap();
    assert_eq!(kinds[summary + 1], "integration.notify_chain.failed");
    let detail = records[summary]["detail"].as_str().unwrap();
    assert!(
        detail.starts_with(
            "suppressed 4 repeated notify chain audit records over the rate limit \
             (started 1, completed 3, failed 0, timed_out 0) from "
        ),
        "{detail}"
    );
}

#[test]
fn absent_server_silently_skips_audit_and_does_not_start_a_server() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let start = Instant::now();
    let out = f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.capture.exists());
    assert!(!f.root.path().join("data").join("ledger").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn kmttt_report_finishes_before_the_previous_notifier_runs() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    // Unique fixture session, with the same inherited socket namespace as the
    // child. This mock never touches the real KMttt session or its policy.
    let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(kmttt_core::paths::socket_path(&f.session))
        .unwrap();
    let marker = f.root.path().join("reported");
    let server = tokio::spawn(async move {
        pipe.connect().await.unwrap();
        let frame = kmttt_proto::read_frame(&mut pipe).await.unwrap().unwrap();
        let request = kmttt_proto::RpcMessage::from_bytes(&frame).unwrap();
        assert_eq!(request.method.as_deref(), Some("hook.event"));
        std::fs::write(marker, "reported").unwrap();
        let response = kmttt_proto::RpcMessage::response(
            request.id.unwrap(),
            Ok(json!({"decision": "allow"})),
        );
        kmttt_proto::write_frame(&mut pipe, &response.to_bytes())
            .await
            .unwrap();
    });
    let mut command = f.command();
    command.args(["hook", "codex", "notify", PAYLOAD]);
    let out = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    assert!(out.status.success());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.captured()["report_seen"], true);
}

#[test]
fn chain_forwards_literal_argv_and_original_payload_outside_a_pane() {
    let f = Fixture::new();
    let argv = f.argv();
    let original = f.configure(&argv);
    let out = f.ok(&["integration", "install", "codex", "--chain"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("3 second limit"));
    f.ok(&["integration", "install", "codex"]); // ordinary reinstall preserves opt-in
                                                // Isolated fixture only: verify the product's global notify still works outside panes.
    let out = f
        .command()
        .env_remove("KMTTT_PANE")
        .args(["hook", "codex", "notify", PAYLOAD])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut expected = argv[1..].to_vec();
    expected.push(PAYLOAD.into());
    assert_eq!(f.captured()["args"], json!(expected));
    assert_eq!(f.captured()["guard"], true);
    f.ok(&["integration", "uninstall", "codex"]);
    assert_eq!(std::fs::read_to_string(&f.config).unwrap(), original);
}

#[test]
fn replace_backup_runs_only_after_explicit_chain_and_can_be_paused_again() {
    let f = Fixture::new();
    let original = f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--replace"]);
    let before: Value = serde_json::from_slice(&std::fs::read(f.sidecar()).unwrap()).unwrap();
    f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(!f.capture.exists());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let after: Value = serde_json::from_slice(&std::fs::read(f.sidecar()).unwrap()).unwrap();
    assert_eq!(before["previous_notify"], after["previous_notify"]);
    f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(f.capture.exists());
    std::fs::remove_file(&f.capture).unwrap();
    let out = f.ok(&["integration", "install", "codex", "--replace"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Existing Codex notify is disabled"));
    f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(!f.capture.exists());
    f.ok(&["integration", "uninstall", "codex", "--if-this-exe"]);
    assert_eq!(std::fs::read_to_string(&f.config).unwrap(), original);
}

#[test]
fn bare_program_is_resolved_at_install_not_from_runtime_path() {
    let f = Fixture::new();
    let mut argv = f.argv();
    argv[0] = "probe".into();
    f.configure(&argv);
    f.ok(&["integration", "install", "codex", "--chain"]);
    let other = f.root.path().join("untrusted project");
    std::fs::create_dir(&other).unwrap();
    std::fs::copy(&f.probe, other.join(f.probe.file_name().unwrap())).unwrap();
    let out = f
        .command()
        .env("PATH", &other)
        .current_dir(&other)
        .args(["hook", "codex", "notify", PAYLOAD])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(f.captured()["exe"], json!(f.probe.canonicalize().unwrap()));
}

#[test]
fn scripts_self_relative_programs_and_conflicting_flags_are_rejected_before_writes() {
    let f = Fixture::new();
    let script = f.root.path().join("script.cmd");
    std::fs::write(&script, "@echo should never execute").unwrap();
    let shell = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe")
    } else {
        PathBuf::from("/bin/sh")
    };
    for program in [
        script.to_str().unwrap(),
        shell.to_str().unwrap(),
        env!("CARGO_BIN_EXE_kmttt"),
        "./probe",
        "missing-notifier",
    ] {
        let original = f.configure(&[program.into()]);
        assert!(!f
            .cli(&["integration", "install", "codex", "--chain"])
            .status
            .success());
        assert_eq!(std::fs::read_to_string(&f.config).unwrap(), original);
        assert!(!f.sidecar().exists());
    }
    let original = f.configure(&f.argv());
    assert!(!f
        .cli(&["integration", "install", "codex", "--chain", "--replace"])
        .status
        .success());
    assert!(!f
        .cli(&["integration", "install", "claude", "--chain"])
        .status
        .success());
    assert_eq!(std::fs::read_to_string(&f.config).unwrap(), original);
    let empty = Fixture::new();
    std::fs::write(&empty.config, "").unwrap();
    assert!(!empty
        .cli(&["integration", "install", "codex", "--chain"])
        .status
        .success());
    assert!(!empty.sidecar().exists());
    empty.ok(&["integration", "install", "codex"]);
    let saved = std::fs::read(empty.sidecar()).unwrap();
    assert!(!empty
        .cli(&["integration", "install", "codex", "--chain"])
        .status
        .success());
    assert_eq!(std::fs::read(empty.sidecar()).unwrap(), saved);
}

#[test]
fn mismatched_registration_changed_config_and_recursion_never_run_notifier() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let saved = std::fs::read(f.sidecar()).unwrap();
    let valid: Value = serde_json::from_slice(&saved).unwrap();
    for broken in [
        {
            let mut r = valid.clone();
            r["chain_argv"][1] = json!("changed");
            r
        },
        {
            let mut r = valid.clone();
            r["chain_config"] = json!("another config");
            r
        },
        {
            let mut r = valid.clone();
            r["chain_argv"][0] = json!(env!("CARGO_BIN_EXE_kmttt"));
            r
        },
    ] {
        std::fs::write(f.sidecar(), broken.to_string()).unwrap();
        f.ok(&["hook", "codex", "notify", PAYLOAD]);
        assert!(!f.capture.exists());
    }
    std::fs::write(f.sidecar(), &saved).unwrap();
    let out = f
        .command()
        .env("KMTTT_NOTIFY_CHAIN_ACTIVE", "1")
        .args(["hook", "codex", "notify", PAYLOAD])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!f.capture.exists());
    std::fs::write(&f.config, "notify = ['user-changed']\n").unwrap();
    f.ok(&["hook", "codex", "notify", PAYLOAD]);
    assert!(!f.capture.exists());
}

#[test]
fn failed_previous_notifier_does_not_fail_codex_hook() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let out = f
        .command()
        .env(MODE, "fail")
        .args(["hook", "codex", "notify", PAYLOAD])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("previous notifier exited"));
}

#[cfg(windows)]
struct ProbeHandle(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProbeHandle {
    fn open(pid: u32) -> Self {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        // SAFETY: only the PID returned by our own native fixture is opened.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
        assert!(!handle.is_null());
        Self(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) })
    }
    fn wait(&self, ms: u32) -> u32 {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the handle is owned and has wait access.
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.0.as_raw_handle(), ms)
        }
    }
}

#[cfg(windows)]
impl Drop for ProbeHandle {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        if self.wait(0) == windows_sys::Win32::Foundation::WAIT_TIMEOUT {
            // SAFETY: cleanup retains the fixture process handle, never a reused PID.
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(self.0.as_raw_handle(), 1)
            };
            self.wait(5000);
        }
    }
}

fn wait_for_capture(file: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = std::fs::read(file) {
            if let Ok(v) = serde_json::from_slice(&bytes) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "probe did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn hung_notifier_is_bounded_and_its_descendants_are_terminated() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let start = Instant::now();
    let mode = if cfg!(windows) { "tree" } else { "hang" };
    let mut child = f
        .command()
        .env(MODE, mode)
        .args(["hook", "codex", "notify", PAYLOAD])
        .spawn()
        .unwrap();
    let data = wait_for_capture(&f.capture);
    #[cfg(windows)]
    let grandchild = ProbeHandle::open(data["child_pid"].as_u64().unwrap() as u32);
    #[cfg(not(windows))]
    let _ = data;
    assert!(child.wait().unwrap().success());
    assert!(start.elapsed() < Duration::from_secs(8));
    #[cfg(windows)]
    assert_eq!(
        grandchild.wait(3000),
        windows_sys::Win32::Foundation::WAIT_OBJECT_0
    );
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("exceeded 3000 ms"));
}

#[cfg(windows)]
#[test]
fn successful_notifier_can_launch_an_intentional_background_app() {
    let f = Fixture::new();
    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--chain"]);
    let start = Instant::now();
    let mut child = f
        .command()
        .env(MODE, "background")
        .args(["hook", "codex", "notify", PAYLOAD])
        .spawn()
        .unwrap();
    let data = wait_for_capture(&f.capture);
    let grandchild = ProbeHandle::open(data["child_pid"].as_u64().unwrap() as u32);
    // Process completion is distinct from EOF on capture pipes inherited by a
    // persistent application. Check the actual hook exit before cleaning it up.
    assert!(child.wait().unwrap().success());
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(
        grandchild.wait(0),
        windows_sys::Win32::Foundation::WAIT_TIMEOUT
    );
}

fn status(f: &Fixture, agent: &str) -> (String, Value) {
    let out = f.ok(&["integration", "status", agent]);
    let text = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v[0]["agent"], agent);
    assert!(
        v[0]["installed"].is_boolean(),
        "existing field kept: {text}"
    );
    assert!(v[0]["file"].is_string(), "existing field kept: {text}");
    assert!(v[0]["reason_text"].as_str().is_some_and(|t| !t.is_empty()));
    (v[0]["reason"].as_str().unwrap().to_owned(), v[0].clone())
}

/// Reason fields are fixed text: no paths, notify arguments or file contents.
fn assert_fixed(f: &Fixture, entry: &Value) {
    let reason = format!("{} {}", entry["reason"], entry["reason_text"]);
    for leak in [
        f.root.path().to_str().unwrap(),
        "notify & helper",
        "probe",
        "space argument",
        "literal",
        "keep",
        "ConHost",
    ] {
        assert!(!reason.contains(leak), "{leak} leaked: {reason}");
    }
}

fn reason(f: &Fixture, agent: &str) -> String {
    let (code, entry) = status(f, agent);
    assert_fixed(f, &entry);
    code
}

#[test]
fn integration_status_reports_fixed_codex_reasons() {
    let f = Fixture::new();
    assert_eq!(reason(&f, "codex"), "config_missing");
    std::fs::write(&f.config, "notify = [\n").unwrap();
    assert_eq!(reason(&f, "codex"), "config_invalid");
    std::fs::write(&f.config, "notify = 'keep'\n").unwrap();
    assert_eq!(reason(&f, "codex"), "config_invalid");
    std::fs::write(&f.config, "model = 'keep'\n").unwrap();
    assert_eq!(reason(&f, "codex"), "notify_missing");
    f.configure(&f.argv());
    assert_eq!(reason(&f, "codex"), "notify_other_program");
    let other = f
        .root
        .path()
        .join("other")
        .join(if cfg!(windows) { "kmttt.exe" } else { "kmttt" });
    std::fs::write(
        &f.config,
        format!(
            "notify = {}\n",
            serde_json::to_string(&[other.to_str().unwrap(), "hook", "codex", "notify"]).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(reason(&f, "codex"), "notify_other_kmttt");

    f.configure(&f.argv());
    f.ok(&["integration", "install", "codex", "--replace"]);
    assert_eq!(reason(&f, "codex"), "ok", "paused previous notify is fine");
    f.ok(&["integration", "uninstall", "codex"]);
    f.ok(&["integration", "install", "codex", "--chain"]);
    assert_eq!(reason(&f, "codex"), "ok");

    let saved = std::fs::read(f.sidecar()).unwrap();
    let valid: Value = serde_json::from_slice(&saved).unwrap();
    for broken in [
        {
            let mut r = valid.clone();
            r["chain_argv"][1] = json!("changed");
            r
        },
        {
            let mut r = valid.clone();
            r["chain_config"] = json!("another config");
            r
        },
        {
            let mut r = valid.clone();
            r["chain_argv"][0] = json!(env!("CARGO_BIN_EXE_kmttt"));
            r
        },
        {
            let mut r = valid.clone();
            r["installed_notify"] = json!(["changed"]);
            r
        },
    ] {
        std::fs::write(f.sidecar(), broken.to_string()).unwrap();
        assert_eq!(reason(&f, "codex"), "chain_registration_mismatch");
    }

    // Same judgment as the hook: a registered shell forwarder is refused.
    let proxy = f.root.path().join("ConHost.exe");
    std::fs::copy(&f.probe, &proxy).unwrap();
    let path = proxy.canonicalize().unwrap().to_str().unwrap().to_owned();
    let mut record = valid.clone();
    let mut previous: Vec<String> =
        serde_json::from_str(record["previous_notify"].as_str().unwrap()).unwrap();
    previous[0] = path.clone();
    record["previous_notify"] = json!(serde_json::to_string(&previous).unwrap());
    record["chain_argv"][0] = json!(path);
    std::fs::write(f.sidecar(), record.to_string()).unwrap();
    assert_eq!(reason(&f, "codex"), "chain_rejected");

    std::fs::write(f.sidecar(), &saved).unwrap();
    assert_eq!(reason(&f, "codex"), "ok");
    std::fs::remove_file(&f.probe).unwrap();
    assert_eq!(reason(&f, "codex"), "chain_target_missing");
}

#[test]
fn integration_status_reports_fixed_claude_reasons() {
    let f = Fixture::new();
    let settings = f.root.path().join("settings.json");
    for agent in ["claude", "claude-usage"] {
        assert_eq!(reason(&f, agent), "config_missing");
    }
    std::fs::write(&settings, "{ keep").unwrap();
    for agent in ["claude", "claude-usage"] {
        assert_eq!(reason(&f, agent), "config_invalid");
    }
    std::fs::write(&settings, "[]").unwrap();
    assert_eq!(reason(&f, "claude"), "config_invalid");
    std::fs::write(&settings, "{}").unwrap();
    assert_eq!(reason(&f, "claude"), "hooks_missing");
    assert_eq!(reason(&f, "claude-usage"), "statusline_missing");

    f.ok(&["integration", "install", "claude"]);
    assert_eq!(reason(&f, "claude"), "ok");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    v["hooks"].as_object_mut().unwrap().remove("Stop");
    std::fs::write(&settings, v.to_string()).unwrap();
    assert_eq!(reason(&f, "claude"), "hooks_partial");

    v["statusLine"] = json!({"type": "command", "command": "keep-my-statusline"});
    std::fs::write(&settings, v.to_string()).unwrap();
    assert_eq!(reason(&f, "claude-usage"), "statusline_other");
    f.ok(&["integration", "install", "claude-usage", "--replace"]);
    assert_eq!(reason(&f, "claude-usage"), "ok");
}
