//! 명령 실행.

use crate::cli::*;
use anyhow::{anyhow, Context, Result};
use kmttt_core::client::RpcClient;
use kmttt_core::{paths, Config};
use kmttt_proto::{codes, RpcError};
use serde_json::{json, Value};

/// 종료 코드: 0 성공, 1 오류, 2 대상 없음·잘못된 입력, 3 정책 거부, 4 시간 초과, 5 기록 검증 실패
fn rpc_exit(e: &RpcError) -> i32 {
    match e.code {
        codes::POLICY_DENIED | codes::APPROVAL_DENIED | codes::RATE_LIMITED => 3,
        codes::TIMEOUT => 4,
        codes::NOT_FOUND | codes::INVALID_PARAMS | codes::METHOD_NOT_FOUND => 2,
        _ => 1,
    }
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

pub async fn run(args: Cli) -> i32 {
    match dispatch(args).await {
        Ok(code) => code,
        Err(e) => {
            if let Some(r) = e.downcast_ref::<RpcError>() {
                eprintln!("error: {}", r.message);
                return rpc_exit(r);
            }
            eprintln!("error: {e:#}");
            1
        }
    }
}

/// 서버에 연결. 없으면 띄우고 기다린다.
pub async fn ensure_server(
    session: &str,
) -> Result<(RpcClient, kmttt_core::client::Notifications)> {
    let exe = std::env::current_exe().context("current exe")?;
    kmttt_core::spawn::ensure_server(&exe, session).await
}

/// `--remote`로 받은 원격 대상. 설정되면 RPC 명령은 모두 SSH 너머 서버로 간다.
struct RemoteTarget {
    target: String,
    session: String,
    bin: String,
}

static REMOTE: std::sync::OnceLock<RemoteTarget> = std::sync::OnceLock::new();

async fn connect_remote() -> Option<Result<(RpcClient, kmttt_core::client::Notifications)>> {
    let r = REMOTE.get()?;
    Some(
        RpcClient::connect_ssh(&r.target, &r.session, &r.bin)
            .await
            .with_context(|| format!("connect to {} over ssh", r.target)),
    )
}

/// `--remote`와 함께 쓸 수 있는 명령인지. 로컬 파일을 다루는 명령은 원격에서 하는 것처럼
/// 보이면서 실제로는 이 컴퓨터를 바꾸게 되므로 거부한다.
fn remote_capable(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Status
            | Cmd::Workspace(_)
            | Cmd::Tab(_)
            | Cmd::Pane(_)
            | Cmd::Agent(_)
            | Cmd::Worktree(_)
            | Cmd::Approval(_)
            | Cmd::Events { .. }
            | Cmd::Audit(AuditCmd::Status)
            | Cmd::Server(ServerCmd::Stop | ServerCmd::ReloadConfig)
            | Cmd::Schema
            | Cmd::Completion { .. }
            | Cmd::Control
    )
}

/// RPC용 연결: `--remote`면 SSH, 아니면 로컬 서버(없으면 띄운다).
async fn connect_any(session: &str) -> Result<(RpcClient, kmttt_core::client::Notifications)> {
    if let Some(r) = connect_remote().await {
        return r;
    }
    ensure_server(session).await
}

async fn client(session: &str) -> Result<RpcClient> {
    Ok(connect_any(session).await?.0)
}

/// 서버가 꼭 떠 있어야 하는 조회(없으면 띄우지 않고 알린다).
async fn existing(session: &str) -> Result<RpcClient> {
    if let Some(r) = connect_remote().await {
        return r.map(|c| c.0);
    }
    RpcClient::connect_local(session)
        .await
        .map(|c| c.0)
        .map_err(|_| {
            anyhow!("no server is running for session '{session}' (start one with `kmttt`)")
        })
}

async fn call(session: &str, method: &str, params: Value) -> Result<Value> {
    let c = client(session).await?;
    c.call(method, params).await.map_err(anyhow::Error::new)
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map(|s| json!(s)).unwrap_or(Value::Null)
}

async fn dispatch(a: Cli) -> Result<i32> {
    anyhow::ensure!(
        paths::valid_session_name(&a.session),
        "invalid session name '{}'",
        a.session
    );
    let s = a.session.clone();
    let Some(cmd) = a.cmd else {
        return attach(
            &a.session,
            a.remote.as_deref(),
            &a.remote_session,
            &a.remote_bin,
            a.control,
        )
        .await;
    };
    if let Some(target) = &a.remote {
        if !remote_capable(&cmd) {
            anyhow::bail!(
                "this command works on local files only and cannot be used with --remote; \
                 run it on the remote machine instead (ssh {target} kmttt …)"
            );
        }
        anyhow::ensure!(
            kmttt_core::machines::valid_target(target),
            "invalid --remote target '{target}'"
        );
        let _ = REMOTE.set(RemoteTarget {
            target: target.clone(),
            session: a.remote_session.clone(),
            bin: a.remote_bin.clone(),
        });
    }
    match cmd {
        Cmd::Status => {
            let client_v = kmttt_core::VERSION;
            let conn = match connect_remote().await {
                Some(r) => r,
                None => RpcClient::connect_local(&s).await,
            };
            match conn {
                Ok((c, _)) => {
                    let st = c.call("server.status", json!({})).await?;
                    let compatible =
                        st["protocol"].as_u64() == Some(kmttt_proto::PROTOCOL_VERSION as u64);
                    print(
                        &json!({"client_version": client_v, "server": st, "compatible": compatible}),
                    );
                }
                Err(_) => print(&json!({"client_version": client_v, "server": null, "session": s})),
            }
        }
        Cmd::Server(sc) => return server_cmd(&s, sc).await,
        Cmd::Session(SessionCmd::Attach { name }) => {
            return attach(&name, None, "default", "kmttt", false).await
        }
        Cmd::Session(SessionCmd::Stop { name }) => {
            let c = existing(&name).await?;
            print(&c.call("server.stop", json!({})).await?);
        }
        Cmd::Workspace(w) => {
            let v = match w {
                WorkspaceCmd::List => call(&s, "workspace.list", json!({})).await?,
                WorkspaceCmd::Create { cwd, label, no_focus, profile } => {
                    call(&s, "workspace.create", json!({"cwd": opt(&cwd), "label": opt(&label), "focus": !no_focus, "profile": opt(&profile)})).await?
                }
                WorkspaceCmd::Rename { id, label } => call(&s, "workspace.rename", json!({"id": id, "label": label})).await?,
                WorkspaceCmd::Focus { id } => call(&s, "workspace.focus", json!({"id": id})).await?,
                WorkspaceCmd::Close { id } => call(&s, "workspace.close", json!({"id": id})).await?,
            };
            print(&v);
        }
        Cmd::Tab(t) => {
            let v = match t {
                TabCmd::List { workspace } => call(&s, "tab.list", json!({"workspace": opt(&workspace)})).await?,
                TabCmd::Create { workspace, label, cwd, no_focus } => {
                    call(&s, "tab.create", json!({"workspace": opt(&workspace), "label": opt(&label), "cwd": opt(&cwd), "focus": !no_focus})).await?
                }
                TabCmd::Rename { id, label } => call(&s, "tab.rename", json!({"id": id, "label": label})).await?,
                TabCmd::Focus { id } => call(&s, "tab.focus", json!({"id": id})).await?,
                TabCmd::Close { id } => call(&s, "tab.close", json!({"id": id})).await?,
            };
            print(&v);
        }
        Cmd::Pane(p) => {
            let v = match p {
                PaneCmd::List => call(&s, "pane.list", json!({})).await?,
                PaneCmd::Split { target, direction, cwd, command, profile, no_focus } => {
                    call(
                        &s,
                        "pane.split",
                        json!({"target": opt(&target), "direction": direction, "cwd": opt(&cwd), "command": opt(&command), "profile": opt(&profile), "focus": !no_focus}),
                    )
                    .await?
                }
                PaneCmd::Run { pane, command } => call(&s, "pane.run", json!({"pane": pane, "command": command})).await?,
                PaneCmd::Send { pane, text, enter } => call(&s, "pane.send", json!({"pane": pane, "text": unescape(&text), "enter": enter})).await?,
                PaneCmd::Read { pane, source, lines } => {
                    let v = call(&s, "pane.read", json!({"pane": pane, "source": source, "lines": lines})).await?;
                    println!("{}", v["text"].as_str().unwrap_or_default());
                    return Ok(0);
                }
                PaneCmd::WaitOutput { pane, regex, timeout } => call(&s, "pane.wait_output", json!({"pane": pane, "regex": regex, "timeout_ms": timeout})).await?,
                PaneCmd::ProcessInfo { pane } => call(&s, "pane.process_info", json!({"pane": opt(&pane)})).await?,
                PaneCmd::Focus { pane } => call(&s, "pane.focus", json!({"pane": pane})).await?,
                PaneCmd::Zoom { pane } => call(&s, "pane.zoom", json!({"pane": opt(&pane)})).await?,
                PaneCmd::Close { pane } => call(&s, "pane.close", json!({"pane": pane})).await?,
            };
            print(&v);
        }
        Cmd::Agent(ag) => {
            let v = match ag {
                AgentCmd::List => call(&s, "agent.list", json!({})).await?,
                AgentCmd::Start { name, kind, pane } => call(&s, "agent.start", json!({"name": name, "kind": kind, "pane": opt(&pane)})).await?,
                AgentCmd::Prompt { target, text, wait, timeout, force } => {
                    call(&s, "agent.prompt", json!({"target": target, "text": unescape(&text), "wait": wait, "timeout_ms": timeout, "force": force})).await?
                }
                AgentCmd::Read { target, lines } => {
                    let v = call(&s, "agent.read", json!({"target": target, "lines": lines})).await?;
                    println!("{}", v["text"].as_str().unwrap_or_default());
                    return Ok(0);
                }
                AgentCmd::Wait { target, until, timeout } => call(&s, "agent.wait", json!({"target": target, "until": until, "timeout_ms": timeout})).await?,
                AgentCmd::Explain { target } => call(&s, "agent.explain", json!({"target": target})).await?,
            };
            print(&v);
        }
        Cmd::Worktree(w) => {
            let v = match w {
                WorktreeCmd::Create {
                    cwd,
                    branch,
                    base,
                    label,
                    path,
                    no_focus,
                } => {
                    let cwd = cwd.or_else(|| {
                        std::env::current_dir()
                            .ok()
                            .map(|p| p.display().to_string())
                    });
                    call(
                        &s,
                        "worktree.create",
                        json!({"cwd": opt(&cwd), "branch": branch, "base": opt(&base), "label": opt(&label), "path": opt(&path), "focus": !no_focus}),
                    )
                    .await?
                }
                WorktreeCmd::Remove { workspace, force } => {
                    call(
                        &s,
                        "worktree.remove",
                        json!({"workspace": workspace, "force": force}),
                    )
                    .await?
                }
            };
            print(&v);
        }
        Cmd::Approval(ap) => {
            let v = match ap {
                ApprovalCmd::List => call(&s, "approval.list", json!({})).await?,
                ApprovalCmd::Allow { id, for_session } => {
                    call(
                        &s,
                        "approval.decide",
                        json!({"id": id, "allow": true, "session": for_session}),
                    )
                    .await?
                }
                ApprovalCmd::Deny { id } => {
                    call(&s, "approval.decide", json!({"id": id, "allow": false})).await?
                }
                ApprovalCmd::Grants => call(&s, "approval.grants", json!({})).await?,
                ApprovalCmd::Revoke { key } => {
                    call(&s, "approval.revoke", json!({"key": key})).await?
                }
            };
            print(&v);
        }
        Cmd::Events { kinds } => {
            let (c, mut notes) = connect_any(&s).await?;
            let kinds: Vec<String> = if kinds.is_empty() {
                vec!["*".into()]
            } else {
                kinds
            };
            c.call("events.subscribe", json!({"kinds": kinds})).await?;
            loop {
                tokio::select! {
                    n = notes.recv() => match n {
                        Some((m, p)) if m == "event" => println!("{}", serde_json::to_string(&p).unwrap_or_default()),
                        Some(_) => {}
                        None => break,
                    },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
        }
        Cmd::Audit(au) => return audit(&s, au).await,
        Cmd::Policy(pc) => return policy(&s, pc).await,
        Cmd::Integration(ic) => return crate::integrations::run(ic),
        Cmd::Statusline => return crate::integrations::statusline(),
        Cmd::Usage => {
            let out = serde_json::json!({
                "claude": kmttt_core::usage::read_claude(),
                "codex": kmttt_core::usage::read_codex(),
                "now_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(0);
        }
        Cmd::Plugin(pc) => return crate::plugin_cmd::run(&s, pc).await,
        Cmd::Machine(mc) => return crate::machines_cmd::run(mc).await,
        Cmd::Console {
            port,
            allow_actions,
            no_open,
        } => return console(&s, port, allow_actions, no_open).await,
        Cmd::Control => {
            return attach(
                &a.session,
                a.remote.as_deref(),
                &a.remote_session,
                &a.remote_bin,
                true,
            )
            .await
        }
        Cmd::Schema => print(&kmttt_proto::schema::openrpc()),
        Cmd::Completion { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(shell, &mut Cli::command(), "kmttt", &mut std::io::stdout());
        }
        Cmd::Update { check } => return crate::update::run(check).await,
        Cmd::Uninstall { yes, purge_ledger } => return uninstall(&s, yes, purge_ledger).await,
        Cmd::Hook {
            agent,
            event,
            payload,
        } => return crate::integrations::hook(&agent, &event, payload).await,
        Cmd::SandboxExec(_) => unreachable!("handled in main"),
        Cmd::DebugPane { command } => return debug_pane(command).await,
    }
    Ok(0)
}

/// `\n`, `\t`, `\e`(ESC), `\r` 이스케이프를 푼다(pane send용).
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('\\') => out.push('\\'),
            Some('x') => {
                // \xNN: 제어 문자 전송용(예: \x03 = Ctrl+C). 잘못된 형식은 글자 그대로 둔다.
                let hex: String = it.clone().take(2).collect();
                match u8::from_str_radix(&hex, 16) {
                    Ok(b) if hex.len() == 2 && b < 0x80 => {
                        out.push(b as char);
                        it.next();
                        it.next();
                    }
                    _ => out.push_str("\\x"),
                }
            }
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

async fn attach(
    session: &str,
    remote: Option<&str>,
    remote_session: &str,
    remote_bin: &str,
    control: bool,
) -> Result<i32> {
    let config = Config::load(&paths::config_file()).context("config.toml")?;
    let machines = kmttt_core::machines::Machines::load(&paths::machines_file())
        .unwrap_or_default()
        .machine;
    let remote = match remote {
        Some(t) => {
            anyhow::ensure!(kmttt_core::machines::valid_target(t), "invalid ssh target");
            Some(kmttt_tui::Remote {
                target: t.into(),
                session: remote_session.into(),
                bin: remote_bin.into(),
            })
        }
        None => {
            ensure_server(session).await?;
            None
        }
    };
    let why = kmttt_tui::run(kmttt_tui::TuiOptions {
        session: session.into(),
        remote,
        start_in_control: control,
        machines,
        config,
    })
    .await?;
    eprintln!("[kmttt: {why}]");
    Ok(0)
}

async fn server_cmd(s: &str, sc: ServerCmd) -> Result<i32> {
    match sc {
        ServerCmd::Run => {
            init_server_log();
            #[cfg(unix)]
            {
                // 터미널이 닫혀도(SIGHUP) 서버는 계속 돈다.
                if let Ok(mut hup) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                {
                    tokio::spawn(async move { while hup.recv().await.is_some() {} });
                }
            }
            let exe = std::env::current_exe().unwrap_or_else(|_| "kmttt".into());
            kmttt_server::run(kmttt_server::ServerOptions {
                session: s.into(),
                kmttt_exe: exe,
            })
            .await?;
        }
        ServerCmd::Stop => {
            let c = existing(s).await?;
            print(&c.call("server.stop", json!({})).await?);
        }
        ServerCmd::ReloadConfig => {
            let c = existing(s).await?;
            print(&c.call("server.reload_config", json!({})).await?);
        }
        ServerCmd::Stdio => {
            // SSH로 들어온 표준 입출력을 로컬 서버 소켓에 그대로 잇는다(프레임 형식이 같다).
            let (c, _) = ensure_server(s).await?;
            drop(c);
            return stdio_bridge(s).await;
        }
        ServerCmd::AgentManifests => {
            let (d, errors) = kmttt_detect::Detector::with_user_dir(&paths::manifests_dir());
            let list: Vec<Value> = d
                .manifests()
                .iter()
                .map(|m| json!({"kind": m.kind, "display": m.display, "process": m.process, "cmdline": m.cmdline, "launch": m.launch, "hook": m.hook, "rules": m.rules.len(), "source": m.source}))
                .collect();
            print(
                &json!({"manifests": list, "errors": errors, "user_dir": paths::manifests_dir()}),
            );
        }
    }
    Ok(0)
}

async fn stdio_bridge(session: &str) -> Result<i32> {
    let addr = paths::socket_path(session);
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(&addr).await?;
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(&addr)?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let a = tokio::io::copy(&mut stdin, &mut wr);
    let b = tokio::io::copy(&mut rd, &mut stdout);
    tokio::select! {
        _ = a => {}
        _ = b => {}
    }
    Ok(0)
}

fn init_server_log() {
    let _ = std::fs::create_dir_all(paths::data_dir());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log_file());
    if let Ok(f) = file {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("KMTTT_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .try_init();
    }
}

async fn audit(s: &str, au: AuditCmd) -> Result<i32> {
    match au {
        AuditCmd::Verify { dir } => {
            let dir = dir.unwrap_or_else(|| paths::ledger_dir(s));
            let r = kmttt_ledger::verify(&dir)?;
            print(&json!({
                "ok": r.ok,
                "dir": dir,
                "segments": r.segments,
                "records": r.records,
                "checkpoints": r.checkpoints,
                "unsigned_tail": r.unsigned_tail,
                "key_fingerprint": r.key_fingerprint,
                "error": r.error.as_ref().map(|e| json!({"file": e.file, "line": e.line, "seq": e.seq, "reason": e.reason})),
            }));
            Ok(if r.ok { 0 } else { 5 })
        }
        AuditCmd::Export {
            dir,
            output,
            decrypt,
        } => {
            let dir = dir.unwrap_or_else(|| paths::ledger_dir(s));
            let v = kmttt_ledger::verify::export(&dir, &kmttt_ledger::ExportOptions { decrypt })?;
            let text = serde_json::to_string_pretty(&v)?;
            match output {
                Some(p) => {
                    std::fs::write(&p, text)?;
                    eprintln!(
                        "exported {} records to {}",
                        v["records"].as_array().map(|a| a.len()).unwrap_or(0),
                        p.display()
                    );
                }
                None => println!("{text}"),
            }
            Ok(if v["verify"]["ok"] == true { 0 } else { 5 })
        }
        AuditCmd::Status => {
            let c = existing(s).await?;
            print(&c.call("audit.status", json!({"verify": true})).await?);
            Ok(0)
        }
        AuditCmd::Path => {
            println!("{}", paths::ledger_dir(s).display());
            Ok(0)
        }
    }
}

pub const POLICY_TEMPLATE: &str = include_str!("../policy.example.toml");

async fn policy(s: &str, pc: PolicyCmd) -> Result<i32> {
    match pc {
        PolicyCmd::Show => {
            let c = existing(s).await?;
            print(&c.call("policy.show", json!({})).await?);
        }
        PolicyCmd::Path => println!("{}", paths::policy_file().display()),
        PolicyCmd::Init => {
            let p = paths::policy_file();
            if p.exists() {
                eprintln!("{} already exists; left unchanged", p.display());
            } else {
                std::fs::create_dir_all(paths::config_dir())?;
                std::fs::write(&p, POLICY_TEMPLATE)?;
                eprintln!("wrote {}", p.display());
            }
        }
        PolicyCmd::Check => {
            let text = std::fs::read_to_string(paths::policy_file()).unwrap_or_default();
            match kmttt_policy::PolicyConfig::parse(&text) {
                Ok(c) => print(
                    &json!({"ok": true, "rules": c.rule.len(), "tool_rules": c.tool_rule.len()}),
                ),
                Err(e) => {
                    eprintln!("policy.toml: {e}");
                    return Ok(2);
                }
            }
        }
    }
    Ok(0)
}

async fn console(s: &str, port: u16, allow_actions: bool, no_open: bool) -> Result<i32> {
    let cfg = Config::load(&paths::config_file()).unwrap_or_default();
    let c = std::sync::Arc::new(client(s).await?);
    let con = kmttt_web::serve(
        c,
        kmttt_web::ConsoleOptions {
            port: if port == 0 { cfg.console.port } else { port },
            allow_actions: allow_actions || cfg.console.allow_actions,
            lang: cfg.language.clone(),
        },
    )
    .await?;
    eprintln!("Securidge KMttt console: {}", con.url);
    eprintln!("(listening on 127.0.0.1 only; press ctrl+c to stop)");
    if !no_open {
        open_browser(&con.url);
    }
    tokio::signal::ctrl_c().await?;
    con.task.abort();
    Ok(0)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
}

async fn uninstall(s: &str, yes: bool, purge_ledger: bool) -> Result<i32> {
    if !yes {
        eprintln!(
            "This stops the server, removes agent hook integrations and deletes {}.",
            paths::config_dir().display()
        );
        eprintln!(
            "The audit ledger in {} is kept unless --purge-ledger is given.",
            paths::ledger_root().display()
        );
        eprintln!("Run again with --yes to continue. The kmttt binary itself is not deleted.");
        return Ok(2);
    }
    if let Ok((c, _)) = RpcClient::connect_local(s).await {
        let _ = c.call("server.stop", json!({})).await;
    }
    for agent in ["claude-usage", "claude", "codex"] {
        let _ = crate::integrations::uninstall(agent);
    }
    let _ = std::fs::remove_dir_all(paths::config_dir());
    let _ = std::fs::remove_dir_all(paths::sessions_dir());
    if purge_ledger {
        let _ = std::fs::remove_dir_all(paths::ledger_root());
    }
    eprintln!(
        "removed. Delete the kmttt executable yourself: {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    Ok(0)
}

/// Phase 1 확인용: 서버 없이 PTY 하나를 지금 터미널에 그대로 잇는다.
async fn debug_pane(command: Vec<String>) -> Result<i32> {
    use kmttt_pty::{PtyEvent, PtyProcess, SpawnSpec};
    use std::io::Write;
    let (prog, args) = if command.is_empty() {
        kmttt_pty::default_shell()
    } else {
        (command[0].clone(), command[1..].to_vec())
    };
    let (cols, rows) = ratatui_size();
    let (pty, mut rx) = PtyProcess::spawn(&SpawnSpec {
        program: prog,
        args,
        cwd: None,
        env: vec![],
        rows,
        cols,
    })?;
    let pty = std::sync::Arc::new(std::sync::Mutex::new(pty));
    raw_mode(true);
    let p2 = pty.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 1024];
        let mut stdin = std::io::stdin();
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0
                || p2
                    .lock()
                    .map(|p| p.write(&buf[..n]).is_err())
                    .unwrap_or(true)
            {
                break;
            }
        }
    });
    let mut out = std::io::stdout();
    let mut code = 0;
    while let Some(ev) = rx.recv().await {
        match ev {
            PtyEvent::Output(b) => {
                let _ = out.write_all(&b);
                let _ = out.flush();
            }
            PtyEvent::Exited(c) => {
                code = c;
                break;
            }
        }
    }
    raw_mode(false);
    Ok(code)
}

fn ratatui_size() -> (u16, u16) {
    kmttt_tui::term_size().unwrap_or((80, 24))
}

fn raw_mode(on: bool) {
    kmttt_tui::raw_mode(on);
}

#[cfg(test)]
mod tests {
    use super::unescape;

    #[test]
    fn unescape_controls() {
        assert_eq!(unescape(r"\x03"), "\x03");
        assert_eq!(unescape(r"a\nb\e[A"), "a\nb\x1b[A");
        assert_eq!(unescape(r"\xZZ"), r"\xZZ");
        assert_eq!(unescape(r"\x0"), r"\x0");
        assert_eq!(unescape(r"\xff"), r"\xff");
    }
}
