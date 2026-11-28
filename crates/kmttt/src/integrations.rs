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
use std::io::Read;
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
        IntegrationCmd::Install { agent, replace } => {
            let msg = if agent == "claude-usage" {
                install_claude_usage(replace)?
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
                out.push(json!({"agent": a, "installed": installed(&a)?, "file": file_for(&a)?}));
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
        // install이 쓰는 TOML 배열 형태: notify = ["…kmttt…", "hook", "codex", "notify"]
        return Ok(text.lines().any(|l| {
            l.trim_start().starts_with("notify")
                && l.contains("kmttt")
                && l.contains("\"hook\", \"codex\"")
        }));
    }
    Ok(text.contains(&format!("hook {agent}")) && text.contains("kmttt"))
}

pub fn install(agent: &str) -> Result<String> {
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
        "codex" => {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            if text
                .lines()
                .any(|l| l.trim_start().starts_with("notify") && !l.contains("kmttt"))
            {
                bail!(
                    "{} already has its own `notify` setting; KMttt will not overwrite it",
                    f.display()
                );
            }
            if !text.contains("hook\", \"codex\"") {
                let line = format!(
                    "notify = [{}, \"hook\", \"codex\", \"notify\"]  # added by kmttt",
                    toml_str(&exe())
                );
                // 최상위 키는 첫 [표] 앞에 넣어야 한다.
                let mut lines: Vec<String> = text.lines().map(String::from).collect();
                let at = lines
                    .iter()
                    .position(|l| l.trim_start().starts_with('['))
                    .unwrap_or(lines.len());
                lines.insert(at, line);
                std::fs::write(&f, lines.join("\n") + "\n")?;
            }
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
        "codex" => {
            let kept: Vec<&str> = text
                .lines()
                .filter(|l| {
                    !(l.trim_start().starts_with("notify")
                        && l.contains("kmttt")
                        && (!only_this_exe || mentions_this_exe(l)))
                })
                .collect();
            std::fs::write(&f, kept.join("\n") + "\n")?;
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
