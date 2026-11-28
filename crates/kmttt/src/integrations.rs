//! F5 훅 통합: 에이전트 설정에 `kmttt hook …`을 등록하고, 훅이 불리면 서버에 보고한다.
//!
//! - Claude Code: `~/.claude/settings.json`의 hooks(PreToolUse는 정책으로 차단 가능).
//! - Codex CLI: `~/.codex/config.toml`의 `notify`(턴 종료 알림, 관찰 전용).
//!
//! - Claude Code 사용량(claude-usage): `~/.claude/settings.json`의 statusLine을 `kmttt statusline`으로.
//!   Claude Code는 요금제 사용량(rate_limits)을 공식 상태 줄 입력으로만 내보내기 때문이다.
//!   주의: 사용자 지정 상태 줄이 있으면 Claude Code는 아래쪽 단축키 안내("esc to interrupt" 등)를
//!   숨긴다. KMttt는 그 문구로도 작업 중을 알아보므로, 이 통합은 훅(claude)과 함께 쓴다.
//!
//! 설치 전에 원래 파일을 `<파일>.kmttt-backup-<시각>`으로 백업한다.

use crate::cli::IntegrationCmd;
use anyhow::{bail, Context, Result};
use kmttt_core::client::RpcClient;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const CLAUDE_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Notification",
    "Stop",
];

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn claude_settings() -> PathBuf {
    std::env::var_os("KMTTT_CLAUDE_SETTINGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude").join("settings.json"))
}

fn codex_config() -> PathBuf {
    std::env::var_os("KMTTT_CODEX_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex").join("config.toml"))
}

/// KMttt 에이전트 스킬. 저장소의 plugins/kmttt/skills/kmttt/SKILL.md를 실행 파일에 넣어 둔다.
const SKILL_MD: &str = include_str!("../../../plugins/kmttt/skills/kmttt/SKILL.md");
/// KMttt가 설치한 스킬 폴더에만 남기는 표식(안에 설치한 kmttt 경로). 이 표식이 없으면 건드리지 않는다.
const SKILL_MARK: &str = ".kmttt-installed";

/// 에이전트가 스킬을 읽는 폴더. Claude Code: ~/.claude/skills, Codex: ~/.agents/skills.
/// (설정 파일 위치를 바꿔 쓰는 경우 그 옆 폴더를 쓴다 — 테스트도 이 경로를 쓴다.)
fn skill_dir(agent: &str) -> Option<PathBuf> {
    match agent {
        "claude" => Some(
            claude_settings()
                .parent()
                .map(|p| p.join("skills").join("kmttt"))
                .unwrap_or_else(|| home().join(".claude").join("skills").join("kmttt")),
        ),
        "codex" => Some(match std::env::var_os("KMTTT_CODEX_CONFIG") {
            Some(c) => PathBuf::from(c)
                .parent()
                .map(|p| p.join(".agents").join("skills").join("kmttt"))
                .unwrap_or_default(),
            None => home().join(".agents").join("skills").join("kmttt"),
        }),
        _ => None,
    }
}

/// 스킬을 설치·갱신한다. 같은 이름의 스킬을 사용자가 직접 만들어 두었으면 그대로 둔다.
pub fn install_skill(agent: &str) -> Result<Option<String>> {
    let Some(d) = skill_dir(agent) else {
        return Ok(None);
    };
    let md = d.join("SKILL.md");
    if md.exists() && !d.join(SKILL_MARK).exists() {
        return Ok(Some(format!(
            "left your own skill at {} unchanged",
            d.display()
        )));
    }
    if std::fs::read_to_string(&md).ok().as_deref() == Some(SKILL_MD) {
        return Ok(None);
    }
    std::fs::create_dir_all(&d)?;
    std::fs::write(&md, SKILL_MD)?;
    std::fs::write(d.join(SKILL_MARK), exe() + "\n")?;
    Ok(Some(format!("installed KMttt skill in {}", d.display())))
}

/// KMttt가 설치한 스킬만 지운다(`only_this_exe`면 이 실행 파일이 설치한 것만).
pub fn uninstall_skill(agent: &str, only_this_exe: bool) -> Result<Option<String>> {
    let Some(d) = skill_dir(agent) else {
        return Ok(None);
    };
    let Ok(mark) = std::fs::read_to_string(d.join(SKILL_MARK)) else {
        return Ok(None);
    };
    if only_this_exe && !mentions_this_exe(&mark) {
        return Ok(None);
    }
    let _ = std::fs::remove_file(d.join("SKILL.md"));
    let _ = std::fs::remove_file(d.join(SKILL_MARK));
    let _ = std::fs::remove_dir(&d); // 사용자가 넣은 다른 파일이 있으면 폴더는 남는다
    Ok(Some(format!("removed KMttt skill from {}", d.display())))
}

/// 연동된 에이전트의 스킬을 현재 버전으로 맞춘다(앱을 업데이트한 뒤 한 번).
pub fn sync_skills() -> Result<Vec<String>> {
    let mut out = Vec::new();
    for a in ["claude", "codex"] {
        if installed(a)? {
            if let Some(m) = install_skill(a)? {
                out.push(m);
            }
        }
    }
    Ok(out)
}

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "kmttt".into())
}

fn backup(p: &std::path::Path) -> Result<Option<PathBuf>> {
    if !p.exists() {
        return Ok(None);
    }
    let ts = chrono_like_ts();
    let b = p.with_file_name(format!(
        "{}.kmttt-backup-{ts}",
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    std::fs::copy(p, &b)?;
    Ok(Some(b))
}

fn chrono_like_ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

fn is_ours(cmd: &str, agent: &str) -> bool {
    cmd.contains("kmttt") && cmd.contains(&format!("hook {agent}"))
}

/// 이 실행 파일을 가리키는 명령인지(설치 위치가 다른 kmttt가 등록한 것은 건드리지 않기 위해).
fn mentions_this_exe(cmd: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    let c = norm(cmd);
    let mut forms = vec![norm(&exe())];
    if let Some(sp) = short_path(&exe()) {
        forms.push(norm(&sp));
    }
    forms
        .iter()
        .any(|f| !f.is_empty() && c.contains(f.as_str()))
}

/// Windows 8.3 짧은 경로(공백 없는 경로). 못 구하면 None.
fn short_path(p: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let wide: Vec<u16> = p.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: 입력은 NUL로 끝나고, 출력 버퍼 크기를 정확히 넘긴다.
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        None
    }
}

/// Claude Code 상태 줄 명령. Windows에서 Claude Code는 Git Bash가 있으면 Git Bash로, 없으면
/// PowerShell로 돌린다. 둘 다에서 통하도록 공백 없는 경로(가능하면 8.3 짧은 경로)를 쓴다.
fn statusline_command() -> String {
    let path = exe();
    let p = if path.contains(' ') {
        short_path(&path).unwrap_or(path)
    } else {
        path
    };
    let p = if cfg!(windows) {
        p.replace('\\', "/")
    } else {
        p
    };
    if p.contains(' ') {
        format!("\"{p}\" statusline")
    } else {
        format!("{p} statusline")
    }
}

fn statusline_is_ours(v: &Value) -> bool {
    v["statusLine"]["command"]
        .as_str()
        .is_some_and(|c| c.contains("kmttt") && c.trim_end().ends_with("statusline"))
}

fn previous_statusline_file() -> PathBuf {
    kmttt_core::paths::data_dir()
        .join("usage")
        .join("claude-statusline-previous.json")
}

/// Claude Code가 상태 줄을 그릴 때 부른다. 사용량 숫자만 저장하고 짧은 한 줄을 출력한다.
/// 상태 줄이 깨지지 않도록 어떤 경우에도 0으로 끝난다.
pub fn statusline() -> Result<i32> {
    let mut buf = String::new();
    let _ = std::io::stdin().take(1024 * 1024).read_to_string(&mut buf);
    let v: Value = serde_json::from_str(buf.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let model = v["model"]["display_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // KMttt 창 안에서 돈 Claude Code면 창 ↔ 대화 기록 짝을 남긴다(창별 토큰 사용량 표시용).
    let (sess, pane) = (
        std::env::var("KMTTT_SESSION").ok(),
        std::env::var("KMTTT_PANE").ok(),
    );
    if let Some(link) = kmttt_core::tokens::claude_link(&v, sess.as_deref(), pane.as_deref()) {
        let _ = kmttt_core::tokens::record_link(&link);
    }
    let line = match kmttt_core::usage::record_claude_statusline(&v) {
        Ok(Some(u)) => format!("KMttt · {}", kmttt_core::usage::short_line(&u)),
        _ => "KMttt".to_string(),
    };
    if model.is_empty() {
        println!("{line}");
    } else {
        println!("{model} · {line}");
    }
    Ok(0)
}

pub fn run(ic: IntegrationCmd) -> Result<i32> {
    match ic {
        IntegrationCmd::Install {
            agent,
            replace,
            chain,
        } => {
            if chain && (agent != "codex" || replace) {
                bail!("--chain is only for codex and cannot be combined with --replace");
            }
            let msg = if chain {
                install_codex(false, true)?
            } else if agent == "claude-usage" {
                install_claude_usage(replace)?
            } else if replace {
                install_with(&agent, true)?
            } else {
                install(&agent)?
            };
            println!("{msg}");
        }
        IntegrationCmd::Uninstall { agent, if_this_exe } => {
            let msg = uninstall_with(&agent, if_this_exe)?;
            println!("{msg}");
        }
        IntegrationCmd::SyncSkills => {
            for m in sync_skills()? {
                println!("{m}");
            }
        }
        IntegrationCmd::Status { agent } => {
            let agents: Vec<String> = agent
                .map(|a| vec![a])
                .unwrap_or_else(|| vec!["claude".into(), "codex".into(), "claude-usage".into()]);
            let mut out = Vec::new();
            for a in agents {
                let reason = diagnose(&a)?;
                out.push(json!({
                    "agent": a,
                    "installed": installed(&a)?,
                    "file": file_for(&a)?,
                    "reason": reason.code(),
                    "reason_text": reason.text(),
                }));
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(0)
}

fn file_for(agent: &str) -> Result<PathBuf> {
    Ok(match agent {
        "claude" | "claude-usage" => claude_settings(),
        "codex" => codex_config(),
        other => bail!("no integration for '{other}' (supported: claude, codex, claude-usage); other agents are observed from the screen only"),
    })
}

pub fn installed(agent: &str) -> Result<bool> {
    let f = file_for(agent)?;
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    if agent == "claude-usage" {
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        return Ok(statusline_is_ours(&v));
    }
    if agent == "codex" {
        // A broken Codex config must not hide other integrations or abort sync.
        return Ok(parse_codex_notify(&text)
            .ok()
            .flatten()
            .is_some_and(|n| codex_notify_is_ours(&n.argv)));
    }
    Ok(text.contains(&format!("hook {agent}")) && text.contains("kmttt"))
}

/// Fixed reason why an integration works or not, for `integration status`.
/// Codes and texts are constants: no paths, commands or file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Ok,
    ConfigMissing,
    ConfigInvalid,
    NotifyMissing,
    NotifyOtherProgram,
    NotifyOtherKmttt,
    ChainRegistrationMismatch,
    ChainTargetMissing,
    ChainRejected,
    HooksMissing,
    HooksPartial,
    StatuslineMissing,
    StatuslineOther,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::ConfigMissing => "config_missing",
            Self::ConfigInvalid => "config_invalid",
            Self::NotifyMissing => "notify_missing",
            Self::NotifyOtherProgram => "notify_other_program",
            Self::NotifyOtherKmttt => "notify_other_kmttt",
            Self::ChainRegistrationMismatch => "chain_registration_mismatch",
            Self::ChainTargetMissing => "chain_target_missing",
            Self::ChainRejected => "chain_rejected",
            Self::HooksMissing => "hooks_missing",
            Self::HooksPartial => "hooks_partial",
            Self::StatuslineMissing => "statusline_missing",
            Self::StatuslineOther => "statusline_other",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Self::Ok => "integration is installed and working",
            Self::ConfigMissing => "agent settings file does not exist",
            Self::ConfigInvalid => "agent settings file cannot be parsed",
            Self::NotifyMissing => "Codex notify is not set",
            Self::NotifyOtherProgram => "Codex notify runs another program",
            Self::NotifyOtherKmttt => "Codex notify uses a different KMttt installation",
            Self::ChainRegistrationMismatch => {
                "chained notify no longer matches its registration; reinstall --chain"
            }
            Self::ChainTargetMissing => "chained notify program cannot be found",
            Self::ChainRejected => {
                "chained notify program is refused (KMttt itself, a shell or a script)"
            }
            Self::HooksMissing => "Claude Code hooks for KMttt are not set",
            Self::HooksPartial => "some Claude Code hook events for KMttt are missing",
            Self::StatuslineMissing => "Claude Code status line is not set",
            Self::StatuslineOther => "Claude Code status line runs another program",
        }
    }
}

/// Diagnose one integration. The first problem that stops it wins.
pub fn diagnose(agent: &str) -> Result<Reason> {
    let f = file_for(agent)?;
    let text = match std::fs::read_to_string(&f) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Reason::ConfigMissing),
        Err(_) => return Ok(Reason::ConfigInvalid),
    };
    if agent == "codex" {
        let Ok(notify) = parse_codex_notify(&text) else {
            return Ok(Reason::ConfigInvalid);
        };
        let Some(notify) = notify else {
            return Ok(Reason::NotifyMissing);
        };
        if !codex_notify_is_ours(&notify.argv) {
            return Ok(Reason::NotifyOtherProgram);
        }
        if !mentions_this_exe(&notify.argv[0]) {
            return Ok(Reason::NotifyOtherKmttt);
        }
        return Ok(match registered_codex_chain() {
            Ok(_) => Reason::Ok,
            Err(e) => match crate::notify_chain::problem_of(&e) {
                Some(crate::notify_chain::ChainProblem::TargetMissing) => {
                    Reason::ChainTargetMissing
                }
                Some(crate::notify_chain::ChainProblem::Rejected) => Reason::ChainRejected,
                _ => Reason::ChainRegistrationMismatch,
            },
        });
    }
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Ok(Reason::ConfigInvalid);
    };
    if !v.is_object() {
        return Ok(Reason::ConfigInvalid);
    }
    if agent == "claude-usage" {
        return Ok(if statusline_is_ours(&v) {
            Reason::Ok
        } else if v["statusLine"].is_null() {
            Reason::StatuslineMissing
        } else {
            Reason::StatuslineOther
        });
    }
    let hooked = CLAUDE_EVENTS
        .iter()
        .filter(|ev| {
            v["hooks"][**ev].as_array().is_some_and(|groups| {
                groups.iter().any(|g| {
                    g["hooks"].as_array().is_some_and(|hs| {
                        hs.iter()
                            .any(|h| h["command"].as_str().is_some_and(|c| is_ours(c, "claude")))
                    })
                })
            })
        })
        .count();
    Ok(match hooked {
        0 => Reason::HooksMissing,
        n if n < CLAUDE_EVENTS.len() => Reason::HooksPartial,
        _ => Reason::Ok,
    })
}

pub fn install(agent: &str) -> Result<String> {
    install_with(agent, false)
}

fn install_with(agent: &str, replace: bool) -> Result<String> {
    if agent == "codex" {
        return install_codex(replace, false);
    }
    let f = file_for(agent)?;
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let bak = backup(&f)?;
    match agent {
        "claude" => {
            let text = std::fs::read_to_string(&f).unwrap_or_else(|_| "{}".into());
            let mut v: Value = serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", f.display()))?;
            if !v.is_object() {
                bail!("{} is not a JSON object", f.display());
            }
            let hooks = v
                .as_object_mut()
                .expect("object")
                .entry("hooks")
                .or_insert_with(|| json!({}));
            for ev in CLAUDE_EVENTS {
                let list = hooks
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("hooks is not an object"))?
                    .entry(*ev)
                    .or_insert_with(|| json!([]));
                let arr = list
                    .as_array_mut()
                    .ok_or_else(|| anyhow::anyhow!("hooks.{ev} is not an array"))?;
                let already = arr.iter().any(|g| {
                    g["hooks"].as_array().is_some_and(|hs| {
                        hs.iter()
                            .any(|h| h["command"].as_str().is_some_and(|c| is_ours(c, "claude")))
                    })
                });
                if already {
                    continue;
                }
                // 승인 대기열을 기다릴 수 있도록 PreToolUse 제한 시간을 넉넉히 둔다.
                let timeout = if *ev == "PreToolUse" { 300 } else { 30 };
                let mut group = json!({"hooks": [{"type": "command", "command": format!("\"{}\" hook claude {ev}", exe()), "timeout": timeout}]});
                if matches!(*ev, "PreToolUse" | "PostToolUse") {
                    group["matcher"] = json!("*");
                }
                arr.push(group);
            }
            std::fs::write(&f, serde_json::to_string_pretty(&v)? + "\n")?;
        }
        _ => unreachable!(),
    }
    let skill = install_skill(agent)?
        .map(|m| format!("\n{m}"))
        .unwrap_or_default();
    Ok(format!(
        "installed {agent} integration in {}{}{skill}",
        f.display(),
        bak.map(|b| format!(" (backup: {})", b.display()))
            .unwrap_or_default()
    ))
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

struct CodexNotify {
    span: std::ops::Range<usize>,
    argv: Vec<String>,
}

fn parse_codex_notify(text: &str) -> Result<Option<CodexNotify>> {
    let mut root: std::collections::BTreeMap<String, toml::Spanned<toml::Value>> =
        toml::from_str(text).context("Codex config is not valid TOML")?;
    let Some(notify) = root.remove("notify") else {
        return Ok(None);
    };
    let argv = notify
        .get_ref()
        .as_array()
        .and_then(|a| a.iter().map(|v| v.as_str().map(String::from)).collect())
        .ok_or_else(|| anyhow::anyhow!("Codex notify must be an array of strings"))?;
    Ok(Some(CodexNotify {
        span: notify.span(),
        argv,
    }))
}

fn codex_notify_is_ours(argv: &[String]) -> bool {
    if argv.len() != 4 || argv[1..] != ["hook", "codex", "notify"] {
        return false;
    }
    let normalized = argv[0].replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or_default();
    argv[0] == exe() || name.eq_ignore_ascii_case("kmttt") || name.eq_ignore_ascii_case("kmttt.exe")
}

/// Recovery for malformed documents: recognize complete, owned single-line
/// notify assignments, rather than trusting a substring in an arbitrary command.
fn codex_notify_lines(text: &str) -> Vec<CodexNotify> {
    let mut offset = 0;
    let mut out = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Ok(Some(mut n)) = parse_codex_notify(line) {
            n.span = n.span.start + offset..n.span.end + offset;
            out.push(n);
        }
        offset += line.len();
    }
    out
}

fn codex_previous_file(f: &std::path::Path) -> PathBuf {
    f.with_file_name(format!(
        "{}.kmttt-notify-previous.json",
        f.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Validate ownership and recovery data before editing either file.
fn read_codex_previous(f: &std::path::Path, argv: &[String]) -> Result<Option<Value>> {
    let bytes = match std::fs::read(codex_previous_file(f)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let record: Value = serde_json::from_slice(&bytes).context("invalid saved Codex notify")?;
    if record["version"] != 1
        || record["installed_notify"] != json!(argv)
        || record["executable"].as_str() != argv.first().map(String::as_str)
        || record.get("previous_notify").is_none()
    {
        bail!("saved Codex notify does not match the current integration; left unchanged");
    }
    match &record["previous_notify"] {
        Value::Null => {}
        Value::String(raw) => {
            let doc = format!("notify = {raw}");
            let parsed = parse_codex_notify(&doc)?.expect("notify key was supplied");
            if &doc[parsed.span] != raw {
                bail!("invalid saved Codex notify value; left unchanged");
            }
        }
        _ => bail!("invalid saved Codex notify value; left unchanged"),
    }
    Ok(Some(record))
}

const CODEX_NOTIFY_PAUSED: &str = "\nExisting Codex notify is disabled while KMttt is installed; related features (such as computer use) may stop until you uninstall the KMttt integration.";
const CODEX_NOTIFY_CHAINED: &str =
    "\nPrevious Codex notify is called after KMttt reporting, with a 3 second limit.";

fn install_codex(replace: bool, chain: bool) -> Result<String> {
    let f = codex_config();
    let text = match std::fs::read_to_string(&f) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let notify = match parse_codex_notify(&text) {
        Ok(n) => n,
        Err(e) if replace || chain => return Err(e),
        Err(_) => {
            let own = codex_notify_lines(&text)
                .into_iter()
                .find(|n| codex_notify_is_ours(&n.argv));
            if own.is_none()
                && text.lines().any(|l| {
                    let l = l.trim_start();
                    l.starts_with("notify")
                        || l.starts_with("\"notify\"")
                        || l.starts_with("'notify'")
                })
            {
                bail!(
                    "{} already has its own `notify` setting; use --replace with valid TOML",
                    f.display()
                );
            }
            own
        }
    };
    let paused;
    let chained;
    if let Some(n) = notify.as_ref().filter(|n| codex_notify_is_ours(&n.argv)) {
        // Legacy installations have no sidecar. Reinstalling must not replace
        // the original notifier saved by a previous --replace installation.
        let mut record = read_codex_previous(&f, &n.argv)?;
        if chain {
            let record = record
                .as_mut()
                .context("no saved previous notify to chain")?;
            enable_codex_chain(&f, record)?;
            update_codex_previous(&f, record)?;
        } else if replace {
            if let Some(record) = record.as_mut().filter(|r| r.get("chain_argv").is_some()) {
                record
                    .as_object_mut()
                    .expect("validated record")
                    .remove("chain_argv");
                record
                    .as_object_mut()
                    .expect("validated record")
                    .remove("chain_config");
                update_codex_previous(&f, record)?;
            }
        }
        chained = record.as_ref().is_some_and(|r| r["chain_argv"].is_array());
        paused = !chained && record.is_some_and(|r| r["previous_notify"].is_string());
    } else {
        if notify.is_some() && !replace && !chain {
            bail!("{} already has its own `notify` setting; run again with --replace to save and replace it (restored on uninstall)", f.display());
        }
        let previous = notify.as_ref().map(|n| text[n.span.clone()].to_string());
        chained = chain;
        paused = previous.is_some() && !chain;
        let argv = vec![exe(), "hook".into(), "codex".into(), "notify".into()];
        let mut record = json!({
            "version": 1, "executable": argv[0], "installed_notify": argv,
            "previous_notify": previous,
        });
        if chain {
            enable_codex_chain(&f, &mut record)?;
        }
        let value = format!(
            "[{}]",
            argv.iter()
                .map(|s| toml_str(s))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut updated = text.clone();
        match notify {
            Some(n) => updated.replace_range(n.span, &value),
            None => {
                let at = if text.starts_with('\u{feff}') {
                    '\u{feff}'.len_utf8()
                } else {
                    0
                };
                let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
                updated.insert_str(at, &format!("notify = {value}  # added by kmttt{newline}"));
            }
        }
        // Check the edited document before creating a backup or changing config.
        if replace || chain {
            parse_codex_notify(&updated)?;
        }
        if let Some(dir) = f.parent() {
            std::fs::create_dir_all(dir)?;
        }
        backup(&f)?;
        let record = serde_json::to_vec_pretty(&record)?;
        // Never clobber an earlier recovery record, including after a failed write.
        let pf = codex_previous_file(&f);
        let mut saved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pf)
            .with_context(|| format!("cannot save previous Codex notify in {}", pf.display()))?;
        if let Err(e) = saved.write_all(&record).and_then(|()| saved.sync_all()) {
            drop(saved);
            let _ = std::fs::remove_file(&pf);
            return Err(e.into());
        }
        drop(saved);
        std::fs::write(&f, updated)?;
    }
    let skill = install_skill("codex")?
        .map(|m| format!("\n{m}"))
        .unwrap_or_default();
    Ok(format!(
        "installed codex integration in {}{skill}{}",
        f.display(),
        if chained {
            CODEX_NOTIFY_CHAINED
        } else if paused {
            CODEX_NOTIFY_PAUSED
        } else {
            ""
        }
    ))
}

fn enable_codex_chain(f: &std::path::Path, record: &mut Value) -> Result<()> {
    let raw = record["previous_notify"]
        .as_str()
        .context("no previous notify to chain")?;
    let previous =
        parse_codex_notify(&format!("notify = {raw}"))?.context("missing previous notify")?;
    let argv = crate::notify_chain::resolve(&previous.argv)?;
    let source = f
        .canonicalize()
        .context("resolve registered Codex config")?;
    record["chain_argv"] = json!(argv);
    record["chain_config"] = json!(source);
    Ok(())
}

/// Mode changes replace the sidecar atomically, preserving its recovery value.
fn update_codex_previous(f: &std::path::Path, record: &Value) -> Result<()> {
    let target = codex_previous_file(f);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let tmp = target.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        created = true;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if result.is_err() && created {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn registered_codex_chain() -> Result<Option<Vec<String>>> {
    let f = codex_config();
    let text = match std::fs::read_to_string(&f) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Some(n) = parse_codex_notify(&text)?
        .filter(|n| codex_notify_is_ours(&n.argv) && mentions_this_exe(&n.argv[0]))
    else {
        return Ok(None);
    };
    use crate::notify_chain::{chain_error, ChainProblem::*};
    let Some(record) = read_codex_previous(&f, &n.argv).map_err(|_| {
        chain_error(
            Mismatch,
            "saved Codex notify does not match the current integration",
        )
    })?
    else {
        return Ok(None);
    };
    let Some(argv) = record.get("chain_argv") else {
        return Ok(None);
    };
    if record["chain_config"] != json!(f.canonicalize()?) {
        return Err(chain_error(
            Mismatch,
            "chain registration belongs to another config",
        ));
    }
    let argv: Vec<String> = argv
        .as_array()
        .and_then(|a| a.iter().map(|v| v.as_str().map(String::from)).collect())
        .ok_or_else(|| chain_error(Mismatch, "invalid chain argv"))?;
    let previous = record["previous_notify"]
        .as_str()
        .and_then(|raw| {
            parse_codex_notify(&format!("notify = {raw}"))
                .ok()
                .flatten()
        })
        .ok_or_else(|| chain_error(Mismatch, "missing previous notify"))?;
    if argv.len() != previous.argv.len() || argv.get(1..) != previous.argv.get(1..) {
        return Err(chain_error(
            Mismatch,
            "chain arguments do not match the saved notify",
        ));
    }
    if let Some(program) = previous
        .argv
        .first()
        .filter(|p| std::path::Path::new(p).is_absolute())
    {
        let program = std::path::Path::new(program)
            .canonicalize()
            .map_err(|_| chain_error(TargetMissing, "open registered chain executable"))?;
        if program != std::path::Path::new(&argv[0]) {
            return Err(chain_error(
                Mismatch,
                "chain executable does not match the saved notify",
            ));
        }
    }
    crate::notify_chain::validate(&argv)?;
    Ok(Some(argv))
}

fn uninstall_codex(f: &std::path::Path, text: &str, only_this_exe: bool) -> Result<String> {
    let candidates = match parse_codex_notify(text) {
        Ok(n) => n.into_iter().collect(),
        Err(_) => codex_notify_lines(text),
    };
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|n| {
            codex_notify_is_ours(&n.argv) && (!only_this_exe || mentions_this_exe(&n.argv[0]))
        })
        .collect();
    let Some(first) = candidates.first() else {
        uninstall_skill("codex", only_this_exe)?;
        return Ok(
            "no matching KMttt notify; current config and any saved previous notify left unchanged"
                .into(),
        );
    };
    let record = read_codex_previous(f, &first.argv)?;
    let mut updated = text.to_string();
    // Work backwards so original byte offsets stay valid. Restore the original
    // once, and remove any duplicate owned lines from an invalid document.
    for (i, n) in candidates.iter().enumerate().rev() {
        if let Some(raw) = record
            .as_ref()
            .and_then(|r| r["previous_notify"].as_str())
            .filter(|_| i == 0)
        {
            updated.replace_range(n.span.clone(), raw);
            continue;
        }
        let start = text[..n.span.start]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or_else(|| {
                if text.starts_with('\u{feff}') {
                    '\u{feff}'.len_utf8()
                } else {
                    0
                }
            });
        let end = text[n.span.end..]
            .find('\n')
            .map(|i| n.span.end + i + 1)
            .unwrap_or(text.len());
        updated.replace_range(start..end, "");
    }
    std::fs::write(f, updated)?;
    // Delete recovery data only after the config has been restored successfully.
    if record.is_some() {
        std::fs::remove_file(codex_previous_file(f))?;
    }
    let skill = uninstall_skill("codex", only_this_exe)?
        .map(|m| format!("\n{m}"))
        .unwrap_or_default();
    Ok(format!(
        "removed codex integration from {}{skill}",
        f.display()
    ))
}

/// Claude Code 상태 줄을 `kmttt statusline`으로 바꾼다. 다른 상태 줄이 있으면 `replace` 없이는 거부한다.
pub fn install_claude_usage(replace: bool) -> Result<String> {
    let f = claude_settings();
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = std::fs::read_to_string(&f).unwrap_or_else(|_| "{}".into());
    let mut v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("{} is not valid JSON", f.display()))?;
    if !v.is_object() {
        bail!("{} is not a JSON object", f.display());
    }
    if statusline_is_ours(&v) {
        v["statusLine"] = json!({"type": "command", "command": statusline_command(), "padding": 0});
        std::fs::write(&f, serde_json::to_string_pretty(&v)? + "\n")?;
        return Ok(format!(
            "Claude usage status line already installed in {}",
            f.display()
        ));
    }
    if let Some(prev) = v.get("statusLine").filter(|x| !x.is_null()) {
        if !replace {
            bail!(
                "Claude Code already has its own status line ({}). Run again with --replace to use KMttt's; the current one is kept and restored when you remove it.",
                prev["command"].as_str().unwrap_or("custom")
            );
        }
        let pf = previous_statusline_file();
        if let Some(d) = pf.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&pf, serde_json::to_vec_pretty(prev)?)?;
    }
    let bak = backup(&f)?;
    v["statusLine"] = json!({"type": "command", "command": statusline_command(), "padding": 0});
    std::fs::write(&f, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(format!(
        "installed Claude usage status line in {}{}",
        f.display(),
        bak.map(|b| format!(" (backup: {})", b.display()))
            .unwrap_or_default()
    ))
}

pub fn uninstall(agent: &str) -> Result<String> {
    uninstall_with(agent, false)
}

/// `only_this_exe`면 이 실행 파일을 가리키는 항목만 지운다.
pub fn uninstall_with(agent: &str, only_this_exe: bool) -> Result<String> {
    let f = file_for(agent)?;
    let Ok(text) = std::fs::read_to_string(&f) else {
        return Ok(format!("{} not found; nothing to remove", f.display()));
    };
    if agent == "codex" {
        return uninstall_codex(&f, &text, only_this_exe);
    }
    let ours = |c: &str, a: &str| is_ours(c, a) && (!only_this_exe || mentions_this_exe(c));
    if agent == "claude-usage" {
        {
            let mut v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
            let cmd = v["statusLine"]["command"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if !statusline_is_ours(&v) || (only_this_exe && !mentions_this_exe(&cmd)) {
                return Ok(format!("no KMttt status line in {}", f.display()));
            }
            let pf = previous_statusline_file();
            match std::fs::read(&pf)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            {
                Some(prev) => v["statusLine"] = prev,
                None => {
                    v.as_object_mut().expect("object").remove("statusLine");
                }
            }
            let _ = std::fs::remove_file(&pf);
            std::fs::write(&f, serde_json::to_string_pretty(&v)? + "\n")?;
            return Ok(format!(
                "removed Claude usage status line from {}",
                f.display()
            ));
        }
    }
    match agent {
        "claude" => {
            let mut v: Value = serde_json::from_str(&text)?;
            if let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                for (_, list) in hooks.iter_mut() {
                    if let Some(arr) = list.as_array_mut() {
                        for g in arr.iter_mut() {
                            if let Some(hs) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                                hs.retain(|h| {
                                    !h["command"].as_str().is_some_and(|c| ours(c, "claude"))
                                });
                            }
                        }
                        arr.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
                    }
                }
                hooks.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
            }
            if v.get("hooks")
                .and_then(|h| h.as_object())
                .is_some_and(|h| h.is_empty())
            {
                v.as_object_mut().expect("object").remove("hooks");
            }
            std::fs::write(&f, serde_json::to_string_pretty(&v)? + "\n")?;
        }
        _ => unreachable!(),
    }
    let skill = uninstall_skill(agent, only_this_exe)?
        .map(|m| format!("\n{m}"))
        .unwrap_or_default();
    Ok(format!(
        "removed {agent} integration from {}{skill}",
        f.display()
    ))
}

/// 에이전트가 부르는 훅. Claude Code: 표준 입력 JSON, 종료 코드 2 = 차단(사유는 stderr).
pub async fn hook(agent: &str, event: &str, arg_payload: Option<String>) -> Result<i32> {
    if agent != "codex" || event != "notify" {
        return hook_report(agent, event, arg_payload).await;
    }
    let raw = match arg_payload {
        Some(raw) => raw,
        None => {
            let mut raw = String::new();
            std::io::stdin()
                .take(4 * 1024 * 1024)
                .read_to_string(&mut raw)?;
            raw
        }
    };
    // Codex 0.172.0 legacy_notify.rs calls spawn() and immediately returns;
    // it does not wait for notify's exit. No worker/queue is necessary here.
    // https://github.com/openai/codex/blob/rust-v0.172.0/codex-rs/hooks/src/legacy_notify.rs
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        hook_report(agent, event, Some(raw.clone())),
    )
    .await;
    if std::env::var_os(crate::notify_chain::ACTIVE_ENV).is_none() {
        let result = async {
            if let Some(argv) = registered_codex_chain()? {
                audit_notify_chain("started").await;
                let result = crate::notify_chain::run(&argv, &raw).await;
                let phase = match &result {
                    Ok(()) => "completed",
                    Err(e) if e.is::<kmttt_pty::bounded::DeadlineExceeded>() => "timed_out",
                    Err(_) => "failed",
                };
                audit_notify_chain(phase).await;
                result?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(e) = result {
            eprintln!("KMttt previous notify skipped or failed: {e}");
        }
    }
    Ok(0)
}

// Observation only: never start a server, log argv/payload, or block notify on
// an unavailable/busy audit connection. Each event has a total 200 ms budget.
async fn audit_notify_chain(phase: &str) {
    let session = std::env::var("KMTTT_SESSION")
        .or_else(|_| std::env::var("KMTTT_SESSION_NAME"))
        .unwrap_or_else(|_| "default".into());
    let _ = tokio::time::timeout(Duration::from_millis(200), async {
        let (client, _notes) = RpcClient::connect_local(&session).await?;
        client
            .call("hook.notify_chain", json!({"phase": phase}))
            .await?;
        Ok::<(), anyhow::Error>(())
    })
    .await;
}

async fn hook_report(agent: &str, event: &str, arg_payload: Option<String>) -> Result<i32> {
    // KMttt pane 밖에서 돈 에이전트는 그냥 통과시킨다.
    let Ok(_pane) = std::env::var("KMTTT_PANE") else {
        return Ok(0);
    };
    // pane이 넣어 주는 KMTTT_SESSION이 우선, 없으면 CLI와 같은 KMTTT_SESSION_NAME.
    let session = std::env::var("KMTTT_SESSION")
        .or_else(|_| std::env::var("KMTTT_SESSION_NAME"))
        .unwrap_or_else(|_| "default".into());
    let payload: Value = match arg_payload {
        Some(p) => serde_json::from_str(p.trim_start_matches('\u{feff}')).unwrap_or(Value::Null),
        None => {
            let mut buf = String::new();
            let _ = std::io::stdin()
                .take(4 * 1024 * 1024)
                .read_to_string(&mut buf);
            // Windows PowerShell 5.1은 파이프로 넘길 때 UTF-8 BOM을 붙이기도 한다.
            serde_json::from_str(buf.trim_start_matches('\u{feff}')).unwrap_or(Value::Null)
        }
    };
    // Codex notify는 type 필드로 종류를 알려 준다.
    let event = if agent == "codex" && event == "notify" {
        payload["type"].as_str().unwrap_or("notify").to_string()
    } else {
        event.to_string()
    };
    let pre = event == "PreToolUse";
    let conn =
        tokio::time::timeout(Duration::from_secs(3), RpcClient::connect_local(&session)).await;
    let client = match conn {
        Ok(Ok((c, _))) => c,
        _ => {
            if pre {
                // KMttt 안에서 돌고 있는데 서버가 없으면 닫힌 쪽으로 실패한다.
                eprintln!("KMttt server for session '{session}' is unreachable; tool call blocked by KMttt policy (fail closed).");
                return Ok(2);
            }
            return Ok(0);
        }
    };
    let r = client
        .call_timeout(
            "hook.event",
            json!({"agent": agent, "event": event, "payload": payload}),
            Duration::from_secs(290),
        )
        .await;
    match r {
        Ok(v) if v["decision"] == "deny" => {
            eprintln!(
                "Blocked by Securidge KMttt policy: {}",
                v["reason"].as_str().unwrap_or("denied")
            );
            Ok(if pre { 2 } else { 0 })
        }
        Ok(_) => Ok(0),
        Err(e) => {
            if pre {
                eprintln!(
                    "KMttt could not evaluate this tool call ({}); blocked (fail closed).",
                    e.message
                );
                Ok(2)
            } else {
                Ok(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const ENV_KEYS: [&str; 3] = [
        "KMTTT_CLAUDE_SETTINGS",
        "KMTTT_CODEX_CONFIG",
        "KMTTT_DATA_DIR",
    ];

    /// Hold the lock until every override is restored, including during unwinding.
    /// Override both agents because sync_skills() checks both integrations.
    struct TestEnv {
        previous: [Option<OsString>; 3],
        _lock: MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn new(root: &std::path::Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let env = Self {
                previous: ENV_KEYS.map(std::env::var_os),
                _lock: lock,
            };
            for (key, path) in ENV_KEYS.into_iter().zip([
                root.join("settings.json"),
                root.join("config.toml"),
                root.join("data"),
            ]) {
                std::env::set_var(key, path);
            }
            env
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (key, value) in ENV_KEYS.into_iter().zip(&self.previous) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn test_env_serializes_and_restores_after_success_and_panic() {
        let d = tempfile::tempdir().unwrap();
        for panic in [false, true] {
            let env = TestEnv::new(d.path());
            let previous = env.previous.clone();
            assert!(matches!(
                ENV_LOCK.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            assert_eq!(claude_settings(), d.path().join("settings.json"));
            assert_eq!(codex_config(), d.path().join("config.toml"));
            assert_eq!(kmttt_core::paths::data_dir(), d.path().join("data"));
            let changed = d.path().join("changed.toml");
            let result = std::panic::catch_unwind(move || {
                let _env = env;
                // Exercise restoration after both removal and replacement.
                std::env::remove_var(ENV_KEYS[0]);
                std::env::set_var(ENV_KEYS[1], changed);
                std::env::remove_var(ENV_KEYS[2]);
                assert!(!panic, "exercise environment restoration on unwind");
            });
            assert_eq!(result.is_err(), panic);
            let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(ENV_KEYS.map(std::env::var_os), previous);
        }
    }

    #[test]
    fn codex_replace_round_trips_original_notify_and_preserves_other_edits() {
        for original in [
            "# 한글 comment\n\"notify\" = [\n 'user-notifier', # inner comment\n 'arg'\n] # trailing\n[profiles.fast]\nnotify = ['nested']\n",
            "\u{feff}notify = ['my-kmttt-notifier'] # comment\r\nmodel = 'x'\r\n",
        ] {
            let d = tempfile::tempdir().unwrap();
            let _env = TestEnv::new(d.path());
            let f = codex_config();
            std::fs::write(&f, original).unwrap();
            assert!(install("codex").is_err());
            assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
            assert!(!codex_previous_file(&f).exists());
            run(IntegrationCmd::Install { agent: "codex".into(), replace: true, chain: false }).unwrap();
            assert!(installed("codex").unwrap());
            let saved = std::fs::read(codex_previous_file(&f)).unwrap();
            let msg = install_with("codex", true).unwrap();
            assert!(msg.contains("Existing Codex notify is disabled"));
            assert!(msg.contains("computer use"));
            install("codex").unwrap();
            assert_eq!(std::fs::read(codex_previous_file(&f)).unwrap(), saved);
            let addition = "\n[added_later]\nkeep = 'new setting'\n";
            let updated = std::fs::read_to_string(&f).unwrap() + addition;
            std::fs::write(&f, updated).unwrap();
            uninstall("codex").unwrap();
            assert_eq!(std::fs::read_to_string(&f).unwrap(), original.to_string() + addition);
            assert!(!installed("codex").unwrap());
            assert!(!codex_previous_file(&f).exists());
        }
    }

    #[test]
    fn codex_without_root_notify_round_trips_nested_notify_and_line_endings() {
        for original in [
            "",
            "\u{feff}# keep\r\n[profiles.fast]\r\nnotify = ['nested']\r\n",
        ] {
            let d = tempfile::tempdir().unwrap();
            let _env = TestEnv::new(d.path());
            let f = codex_config();
            std::fs::write(&f, original).unwrap();
            let msg = install("codex").unwrap();
            assert!(!msg.contains("Existing Codex notify is disabled"));
            assert!(installed("codex").unwrap());
            uninstall("codex").unwrap();
            assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
        }
    }

    #[test]
    fn codex_status_and_sync_keep_working_with_invalid_config() {
        for invalid in [
            "notify = [",
            "notify = 'wrong type'",
            "notify = []\nnotify = []\n",
        ] {
            let d = tempfile::tempdir().unwrap();
            let _env = TestEnv::new(d.path());
            install("claude").unwrap();
            let claude_skill = skill_dir("claude").unwrap().join("SKILL.md");
            std::fs::write(&claude_skill, "old skill").unwrap();
            std::fs::write(codex_config(), invalid).unwrap();
            assert!(!installed("codex").unwrap());
            run(IntegrationCmd::Status { agent: None }).unwrap();
            sync_skills().unwrap();
            assert_eq!(std::fs::read_to_string(claude_skill).unwrap(), SKILL_MD);
            assert_eq!(std::fs::read_to_string(codex_config()).unwrap(), invalid);
        }
    }

    #[test]
    fn codex_plain_install_stays_permissive_but_replace_requires_valid_toml() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = codex_config();
        let original = "broken = [\n";
        std::fs::write(&f, original).unwrap();
        assert!(install_with("codex", true).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
        install("codex").unwrap();
        install("codex").unwrap();
        uninstall("codex").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    }

    #[test]
    fn codex_uninstall_cleans_owned_notify_and_skill_with_invalid_config() {
        for original in ["", "notify = ['original']\n"] {
            for damage in [
                "\nnotify = ['duplicate']\n",
                "\nbroken = [\n",
                "\nnotify = 'user-string'\n",
            ] {
                for only_this_exe in [false, true] {
                    let d = tempfile::tempdir().unwrap();
                    let _env = TestEnv::new(d.path());
                    let f = codex_config();
                    std::fs::write(&f, original).unwrap();
                    install_with("codex", true).unwrap();
                    let current = std::fs::read_to_string(&f).unwrap();
                    std::fs::write(&f, current + damage).unwrap();
                    uninstall_with("codex", only_this_exe).unwrap();
                    assert_eq!(
                        std::fs::read_to_string(&f).unwrap(),
                        original.to_string() + damage
                    );
                    assert!(!skill_dir("codex").unwrap().exists());
                    assert!(!codex_previous_file(&f).exists());
                }
            }
        }
    }

    #[test]
    fn codex_user_modified_notify_and_saved_original_are_left_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = codex_config();
        std::fs::write(&f, "notify = ['original']\n").unwrap();
        install_with("codex", true).unwrap();
        let saved = std::fs::read(codex_previous_file(&f)).unwrap();
        let changed = "notify = ['user-changed']\n";
        std::fs::write(&f, changed).unwrap();
        assert!(uninstall("codex").unwrap().contains("unchanged"));
        assert!(install_with("codex", true).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), changed);
        assert_eq!(std::fs::read(codex_previous_file(&f)).unwrap(), saved);
    }

    #[test]
    fn codex_if_this_exe_does_not_restore_another_installations_notify() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = codex_config();
        let original = "notify = ['original']\n";
        std::fs::write(&f, original).unwrap();
        install_with("codex", true).unwrap();
        let other = d.path().join("other/kmttt.exe").display().to_string();
        let mut record: Value =
            serde_json::from_slice(&std::fs::read(codex_previous_file(&f)).unwrap()).unwrap();
        record["executable"] = json!(other);
        record["installed_notify"][0] = json!(other);
        std::fs::write(codex_previous_file(&f), record.to_string()).unwrap();
        let current = format!(
            "notify = [{}, \"hook\", \"codex\", \"notify\"]\n",
            toml_str(&other)
        );
        std::fs::write(&f, &current).unwrap();
        uninstall_with("codex", true).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), current);
        assert!(codex_previous_file(&f).exists());
        uninstall_with("codex", false).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    }

    #[test]
    fn codex_invalid_config_or_failed_recovery_save_never_changes_config() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = codex_config();
        for invalid in ["notify = [", "notify = 'wrong type'", "notify = [42]"] {
            std::fs::write(&f, invalid).unwrap();
            assert!(install_with("codex", true).is_err());
            assert_eq!(std::fs::read_to_string(&f).unwrap(), invalid);
            assert!(!codex_previous_file(&f).exists());
        }
        let original = "notify = ['original']\n";
        std::fs::write(&f, original).unwrap();
        std::fs::create_dir(codex_previous_file(&f)).unwrap();
        assert!(install_with("codex", true).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    }

    #[test]
    fn codex_corrupt_or_mismatched_recovery_record_never_changes_config() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = codex_config();
        std::fs::write(&f, "notify = ['original']\n").unwrap();
        install_with("codex", true).unwrap();
        let current = std::fs::read_to_string(&f).unwrap();
        let valid: Value =
            serde_json::from_slice(&std::fs::read(codex_previous_file(&f)).unwrap()).unwrap();
        for broken in [
            "not json".to_string(),
            {
                let mut v = valid.clone();
                v["installed_notify"][0] = json!("other");
                v.to_string()
            },
            {
                let mut v = valid.clone();
                v["previous_notify"] = json!("['original']\nmodel = 'injected'");
                v.to_string()
            },
        ] {
            std::fs::write(codex_previous_file(&f), &broken).unwrap();
            assert!(uninstall("codex").is_err());
            assert_eq!(std::fs::read_to_string(&f).unwrap(), current);
            assert_eq!(
                std::fs::read_to_string(codex_previous_file(&f)).unwrap(),
                broken
            );
        }
    }

    #[test]
    fn claude_install_uninstall_preserves_user_hooks() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = d.path().join("settings.json");
        std::fs::write(&f, r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#).unwrap();
        install("claude").unwrap();
        install("claude").unwrap(); // 두 번 해도 중복되지 않는다
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["model"], "x");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(installed("claude").unwrap());
        // 스킬도 함께 설치되고(표식 포함), 지울 때 함께 지워진다
        let skill = d.path().join("skills").join("kmttt");
        assert_eq!(
            std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            SKILL_MD
        );
        assert!(skill.join(SKILL_MARK).exists());
        assert!(sync_skills().unwrap().is_empty()); // 이미 최신
        uninstall("claude").unwrap();
        assert!(!skill.exists());
        // 사용자가 직접 만든 같은 이름의 스킬은 덮어쓰지도 지우지도 않는다
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "mine").unwrap();
        install("claude").unwrap();
        uninstall("claude").unwrap();
        assert_eq!(
            std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "mine"
        );
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert!(v["hooks"].get("PreToolUse").is_none());
        // 백업이 남는다
        assert!(std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("kmttt-backup")));

        // 사용량 상태 줄: 남의 상태 줄은 --replace 없이 바꾸지 않고, 지우면 되돌린다.
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        v["statusLine"] = json!({"type": "command", "command": "mine.sh"});
        std::fs::write(&f, v.to_string()).unwrap();
        assert!(install_claude_usage(false).is_err());
        install_claude_usage(true).unwrap();
        assert!(installed("claude-usage").unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert!(v["statusLine"]["command"]
            .as_str()
            .unwrap()
            .ends_with("statusline"));
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        uninstall_with("claude-usage", true).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "mine.sh");
    }

    #[test]
    fn codex_notify_inserted_before_tables() {
        let d = tempfile::tempdir().unwrap();
        let _env = TestEnv::new(d.path());
        let f = d.path().join("config.toml");
        std::fs::write(&f, "model = \"o4\"\n\n[profiles.fast]\nmodel = \"mini\"\n").unwrap();
        std::env::set_var("KMTTT_CODEX_CONFIG", &f);
        assert!(!installed("codex").unwrap());
        install("codex").unwrap();
        // status와 앱 시작 시 스킬 갱신(sync_skills)이 모두 이 판정을 쓴다
        assert!(
            installed("codex").unwrap(),
            "status must see the notify line install wrote"
        );
        let text = std::fs::read_to_string(&f).unwrap();
        let notify = text.find("notify").unwrap();
        let table = text.find("[profiles.fast]").unwrap();
        assert!(notify < table, "{text}");
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        assert!(parsed["notify"].as_array().is_some());
        let skill = d.path().join(".agents").join("skills").join("kmttt");
        assert!(skill.join("SKILL.md").exists());
        uninstall("codex").unwrap();
        assert!(!installed("codex").unwrap());
        assert!(!skill.exists());
        assert!(!std::fs::read_to_string(&f).unwrap().contains("notify"));
        // 사용자가 직접 둔 notify는 건드리지 않는다
        std::fs::write(&f, "notify = [\"my-notifier\"]\n").unwrap();
        assert!(!installed("codex").unwrap());
        assert!(install("codex").is_err());
        std::env::remove_var("KMTTT_CODEX_CONFIG");
    }
}
