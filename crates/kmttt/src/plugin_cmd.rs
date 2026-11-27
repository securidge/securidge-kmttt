//! 플러그인 설치·목록·삭제. 설치 때 매니페스트의 권한을 보여 주고 동의를 받는다.
//! 동의한 권한은 `.approved`에 적고, 서버는 그 파일이 있는 플러그인만 불러온다.

use crate::cli::PluginCmd;
use anyhow::{bail, Context, Result};
use kmttt_core::paths;
use kmttt_server::plugins::PluginManifest;
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let t = dst.join(&name);
        let ft = e.file_type()?;
        if ft.is_symlink() {
            // 심볼릭 링크는 따라가지 않는다(폴더 밖 파일 끌어오기 방지).
            continue;
        }
        if ft.is_dir() {
            copy_dir(&p, &t)?;
        } else {
            std::fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

/// "owner/repo/plugin" → GitHub 저장소를 받아 그 안의 plugin 폴더를 쓴다.
fn fetch(source: &str) -> Result<(PathBuf, Option<tempdir::TempDir>)> {
    let p = PathBuf::from(source);
    if p.is_dir() {
        return Ok((p, None));
    }
    let parts: Vec<&str> = source.split('/').collect();
    let (url, sub) = if source.starts_with("https://") {
        (source.to_string(), None)
    } else if parts.len() >= 2
        && parts.iter().all(|x| {
            !x.is_empty()
                && x.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        (
            format!("https://github.com/{}/{}.git", parts[0], parts[1]),
            parts.get(2).map(|s| s.to_string()),
        )
    } else {
        bail!("plugin source must be a folder, an https git URL, or owner/repo[/plugin]");
    };
    let tmp = tempdir::TempDir::new()?;
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", &url])
        .arg(tmp.path().join("src"))
        .status()
        .context("run git clone")?;
    if !status.success() {
        bail!("git clone {url} failed");
    }
    let mut dir = tmp.path().join("src");
    if let Some(s) = sub {
        dir = dir.join(s);
    }
    Ok((dir, Some(tmp)))
}

/// tempfile 없이 쓰는 작은 임시 폴더.
mod tempdir {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> std::io::Result<Self> {
            let p = std::env::temp_dir().join(format!(
                "kmttt-plugin-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p)?;
            Ok(Self(p))
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

pub async fn run(session: &str, pc: PluginCmd) -> Result<i32> {
    match pc {
        PluginCmd::Install { source, yes } => {
            let (dir, _tmp) = fetch(&source)?;
            let text = std::fs::read_to_string(dir.join("kmttt-plugin.toml"))
                .context("kmttt-plugin.toml not found")?;
            let m = PluginManifest::parse(&text)?;
            eprintln!("Plugin '{}' {} — {}", m.name, m.version, m.description);
            eprintln!("  command : {}", m.command);
            eprintln!(
                "  events  : {}",
                if m.events.is_empty() {
                    "(none)".into()
                } else {
                    m.events.join(", ")
                }
            );
            eprintln!(
                "  It runs with YOUR user permissions. It asks for these KMttt capabilities:"
            );
            if m.capabilities.is_empty() {
                eprintln!("    (none)");
            }
            for c in &m.capabilities {
                let why = kmttt_policy::tools::CAPABILITIES
                    .iter()
                    .find(|(n, _)| n == c)
                    .map(|x| x.1)
                    .unwrap_or("");
                eprintln!("    - {c}: {why}");
            }
            if !yes {
                eprint!("Install and grant these capabilities? [y/N] ");
                std::io::stderr().flush()?;
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                if !matches!(line.trim(), "y" | "Y" | "yes") {
                    eprintln!("cancelled");
                    return Ok(2);
                }
            }
            let dst = paths::plugins_dir().join(&m.name);
            if dst.exists() {
                std::fs::remove_dir_all(&dst)?;
            }
            copy_dir(&dir, &dst)?;
            std::fs::write(dst.join(".approved"), m.capabilities.join("\n") + "\n")?;
            if let Ok((c, _)) = kmttt_core::client::RpcClient::connect_local(session).await {
                let _ = c.call("server.reload_config", json!({})).await;
            }
            println!(
                "{}",
                json!({"installed": m.name, "dir": dst, "capabilities": m.capabilities})
            );
        }
        PluginCmd::List => {
            let ps = kmttt_server::plugins::load_all(&paths::plugins_dir());
            let v: Vec<_> = ps
                .iter()
                .map(|p| json!({"name": p.manifest.name, "version": p.manifest.version, "events": p.manifest.events, "capabilities": p.capabilities, "dir": p.dir}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        PluginCmd::Remove { name } => {
            anyhow::ensure!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "bad plugin name"
            );
            let dst = paths::plugins_dir().join(&name);
            anyhow::ensure!(dst.exists(), "plugin {name} is not installed");
            std::fs::remove_dir_all(&dst)?;
            if let Ok((c, _)) = kmttt_core::client::RpcClient::connect_local(session).await {
                let _ = c.call("server.reload_config", json!({})).await;
            }
            println!("{}", json!({"removed": name}));
        }
    }
    Ok(0)
}
