//! git worktree를 워크스페이스로 다룬다.

use anyhow::{bail, Context, Result};
use kmttt_proto::WorktreeInfo;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    // 회수기(reaper)가 종료 상태를 가로채지 않게 잠금 안에서 띄우고 등록한다.
    let mut guard = kmttt_pty::reaper::spawn_lock();
    let child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("run git")?;
    let pid = child.id();
    guard.register(Some(pid));
    drop(guard);
    let out = child.wait_with_output();
    kmttt_pty::reaper::unregister(Some(pid));
    let out = out.context("run git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 브랜치 이름으로 쓸 수 있는지(경로 탈출·옵션 주입 방지).
pub fn valid_branch(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with('-')
        && !b.contains("..")
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

/// `git worktree add`로 새 작업 폴더를 만든다.
pub fn create(
    cwd: &Path,
    branch: &str,
    base: Option<&str>,
    path: Option<&Path>,
    root: &Path,
) -> Result<WorktreeInfo> {
    if !valid_branch(branch) {
        bail!("invalid branch name '{branch}'");
    }
    if let Some(b) = base {
        if !valid_branch(b) {
            bail!("invalid base '{b}'");
        }
    }
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?);
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let target = match path {
        Some(p) => p.to_path_buf(),
        None => root.join(&repo_name).join(branch.replace('/', "-")),
    };
    if target.exists() {
        bail!("{} already exists", target.display());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let target_s = target.display().to_string();
    let exists = git(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    if exists {
        git(&repo, &["worktree", "add", &target_s, branch])?;
    } else {
        let mut args = vec!["worktree", "add", "-b", branch, &target_s];
        if let Some(b) = base {
            args.push(b);
        }
        git(&repo, &args)?;
    }
    Ok(WorktreeInfo {
        repo: repo.display().to_string(),
        branch: branch.into(),
        path: target_s,
    })
}

/// 변경사항이 있으면 거부한다(force면 강제). 브랜치는 지우지 않는다.
pub fn remove(info: &WorktreeInfo, force: bool) -> Result<()> {
    let path = Path::new(&info.path);
    if !force {
        let dirty = git(path, &["status", "--porcelain"])?;
        if !dirty.is_empty() {
            bail!(
                "worktree {} has uncommitted changes; use --force to remove anyway",
                info.path
            );
        }
    }
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&info.path);
    git(Path::new(&info.repo), &args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        assert!(valid_branch("feature/login-rate"));
        assert!(!valid_branch("--upload-pack=x"));
        assert!(!valid_branch("../../etc"));
        assert!(!valid_branch("a b"));
    }

    #[test]
    fn create_and_remove_in_temp_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap()
        };
        g(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "init"]);
        let root = d.path().join("wt");
        let info = create(&repo, "feat-x", None, None, &root).unwrap();
        assert!(Path::new(&info.path).join("a.txt").exists());
        std::fs::write(Path::new(&info.path).join("b.txt"), "dirty").unwrap();
        assert!(remove(&info, false).is_err());
        remove(&info, true).unwrap();
        assert!(!Path::new(&info.path).exists());
    }
}
