//! 플러그인: `kmttt-plugin.toml` + 실행 파일. 이벤트가 오면 서버가 실행하고, 플러그인은
//! 일반 CLI·소켓 API를 쓴다. 다만 서버가 플러그인 프로세스를 알아보고 선언·동의된 권한만 허용한다.

use crate::Server;
use kmttt_core::CoreEvent;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// 실행할 명령. 첫 단어가 플러그인 폴더 안 파일이면 그 경로로 바꾼다.
    pub command: String,
    /// 구독할 이벤트 종류(agent.state, worktree.created, approval.requested, security.event, "*")
    #[serde(default)]
    pub events: Vec<String>,
    /// 필요한 권한: layout, pane.read, pane.control
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl PluginManifest {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let m: PluginManifest = toml::from_str(text)?;
        anyhow::ensure!(
            !m.name.is_empty()
                && m.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "plugin name must be alphanumeric, '-' or '_'"
        );
        for c in &m.capabilities {
            anyhow::ensure!(
                kmttt_policy::tools::CAPABILITIES
                    .iter()
                    .any(|(n, _)| n == c),
                "unknown capability '{c}'"
            );
        }
        Ok(m)
    }
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    /// 사용자가 설치 때 동의한 권한과 매니페스트 권한의 교집합
    pub capabilities: Vec<String>,
}

/// `plugins/<이름>/kmttt-plugin.toml` 중 동의 파일(`.approved`)이 있는 것만 불러온다.
pub fn load_all(dir: &Path) -> Vec<Plugin> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let d = e.path();
        let Ok(text) = std::fs::read_to_string(d.join("kmttt-plugin.toml")) else {
            continue;
        };
        let Ok(m) = PluginManifest::parse(&text) else {
            tracing::warn!("invalid plugin manifest in {}", d.display());
            continue;
        };
        let Ok(approved) = std::fs::read_to_string(d.join(".approved")) else {
            continue;
        };
        let approved: Vec<String> = approved
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let caps = m
            .capabilities
            .iter()
            .filter(|c| approved.contains(c))
            .cloned()
            .collect();
        out.push(Plugin {
            manifest: m,
            dir: d,
            capabilities: caps,
        });
    }
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    out
}

fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 이벤트를 구독한 플러그인을 실행한다(이벤트 JSON은 표준 입력으로).
pub fn dispatch(server: &Arc<Server>, ev: &CoreEvent) {
    // 플러그인 자신이 일으킨 이벤트로 다시 플러그인을 부르지 않는다(무한 반복 방지).
    if ev.source.starts_with("plugin:") || ev.kind == "pane.output" {
        return;
    }
    let plugins: Vec<Plugin> = match server.plugins.read() {
        Ok(p) => p
            .iter()
            .filter(|p| p.manifest.events.iter().any(|e| e == "*" || e == &ev.kind))
            .cloned()
            .collect(),
        Err(_) => return,
    };
    if plugins.is_empty() {
        return;
    }
    let payload = serde_json::json!({"kind": ev.kind, "ts_ms": ev.ts_ms, "source": ev.source, "data": ev.data}).to_string();
    for p in plugins {
        let server = server.clone();
        let payload = payload.clone();
        let kind = ev.kind.clone();
        tokio::spawn(async move {
            if let Err(e) = run_plugin(&server, &p, &kind, &payload).await {
                tracing::warn!("plugin {}: {e}", p.manifest.name);
            }
        });
    }
}

async fn run_plugin(
    server: &Arc<Server>,
    p: &Plugin,
    kind: &str,
    payload: &str,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut parts = split_command(&p.manifest.command);
    anyhow::ensure!(!parts.is_empty(), "empty command");
    let local = p.dir.join(&parts[0]);
    if local.is_file() {
        parts[0] = local.display().to_string();
    }
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..])
        .current_dir(&p.dir)
        .env("KMTTT_PLUGIN", &p.manifest.name)
        .env("KMTTT_EVENT", kind)
        .env("KMTTT_SESSION", &server.opts.session)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // 가드는 await 전에 블록 안에서 끝나야 한다(MutexGuard는 Send가 아니다).
    let mut child = {
        let mut guard = kmttt_pty::reaper::spawn_lock();
        let child = cmd.spawn()?;
        guard.register(child.id());
        child
    };
    let pid = child.id();
    if let (Some(pid), Ok(mut st)) = (pid, server.st.lock()) {
        st.plugin_pids.insert(pid, p.manifest.name.clone());
    }
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes()).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(60), child.wait()).await;
    let _ = child.kill().await;
    kmttt_pty::reaper::unregister(pid);
    if let (Some(pid), Ok(mut st)) = (pid, server.st.lock()) {
        st.plugin_pids.remove(&pid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_validation() {
        let m = PluginManifest::parse(
            "name='board'\ncommand='./run.sh --x'\nevents=['agent.state']\ncapabilities=['layout']",
        )
        .unwrap();
        assert_eq!(m.name, "board");
        assert!(PluginManifest::parse("name='bad name'\ncommand='x'").is_err());
        assert!(PluginManifest::parse("name='x'\ncommand='x'\ncapabilities=['root']").is_err());
    }

    #[test]
    fn only_approved_plugins_with_intersection() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(
            a.join("kmttt-plugin.toml"),
            "name='a'\ncommand='x'\ncapabilities=['layout','pane.read']",
        )
        .unwrap();
        let b = d.path().join("b");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("kmttt-plugin.toml"), "name='b'\ncommand='x'").unwrap();
        // a만 동의, 그것도 layout만
        std::fs::write(a.join(".approved"), "layout\n").unwrap();
        let ps = load_all(d.path());
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].capabilities, vec!["layout"]);
    }

    #[test]
    fn command_split() {
        assert_eq!(
            split_command("node 'my script.js' --a"),
            vec!["node", "my script.js", "--a"]
        );
    }
}
