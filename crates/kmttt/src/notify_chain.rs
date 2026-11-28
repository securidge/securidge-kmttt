//! Execute only an explicitly registered native notifier, without a shell.
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub(crate) const ACTIVE_ENV: &str = "KMTTT_NOTIFY_CHAIN_ACTIVE";

/// Why a registered chain cannot run. The hook and `integration status`
/// downcast to this, so both report the same judgment without matching text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainProblem {
    /// Saved record, config or arguments no longer match the registration.
    Mismatch,
    /// The registered executable cannot be opened.
    TargetMissing,
    /// KMttt itself, a shell or shell forwarder, a script or non-native file.
    Rejected,
}

#[derive(Debug)]
pub(crate) struct ChainError {
    pub problem: ChainProblem,
    message: &'static str,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for ChainError {}

pub(crate) fn chain_error(problem: ChainProblem, message: &'static str) -> anyhow::Error {
    ChainError { problem, message }.into()
}

/// The typed problem behind an error from chain checks, if any.
pub(crate) fn problem_of(e: &anyhow::Error) -> Option<ChainProblem> {
    e.downcast_ref::<ChainError>().map(|c| c.problem)
}

pub(crate) fn resolve(argv: &[String]) -> Result<Vec<String>> {
    let program = argv
        .first()
        .filter(|p| !p.is_empty())
        .context("previous notify has no program")?;
    let path = Path::new(program);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        if path.components().count() != 1 || program.contains(['/', '\\']) {
            bail!("chain requires an absolute executable or a bare PATH program name");
        }
        let filename = if cfg!(windows) && path.extension().is_none() {
            format!("{program}.exe")
        } else {
            program.clone()
        };
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .filter(|p| p.is_absolute())
            .map(|p| p.join(&filename))
            .find(|p| p.is_file())
            .context("previous notify executable was not found in absolute PATH directories")?
    };
    let path = path
        .canonicalize()
        .context("resolve previous notify executable")?;
    let mut resolved = argv.to_vec();
    resolved[0] = path.to_str().context("notifier path is not UTF-8")?.into();
    validate(&resolved)?;
    Ok(resolved)
}

const NOT_NATIVE: &str =
    "chain requires a native executable; scripts need an explicit native interpreter";

pub(crate) fn validate(argv: &[String]) -> Result<()> {
    let program = argv.first().context("missing chain program")?;
    use ChainProblem::*;
    if argv.iter().any(|s| s.contains('\0')) {
        return Err(chain_error(Rejected, "notify arguments contain NUL"));
    }
    let path = PathBuf::from(program);
    if !path.is_absolute() {
        return Err(chain_error(Mismatch, "chain executable must be absolute"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| chain_error(TargetMissing, "open registered chain executable"))?;
    if canonical != path {
        return Err(chain_error(
            Mismatch,
            "registered executable path changed; reinstall --chain",
        ));
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    if matches!(
        stem,
        "kmttt"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "sh"
            | "bash"
            | "dash"
            | "zsh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "fish"
            | "nu"
            // Known shell/command forwarding hosts, including remote shells.
            | "wsl"
            | "wslhost"
            | "conhost"
            | "openconsole"
            | "wt"
            | "windowsterminal"
            | "winpty"
            | "winpty-agent"
            | "ssh"
            | "plink"
    ) || std::env::current_exe()?.canonicalize()? == path
    {
        return Err(chain_error(
            Rejected,
            "chain does not execute KMttt itself, shells or shell forwarding programs",
        ));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(chain_error(
            Rejected,
            "chain requires a native .exe, not .cmd/.bat/scripts",
        ));
    }
    let mut magic = [0; 4];
    std::fs::File::open(&path)
        .map_err(|_| chain_error(TargetMissing, "open registered chain executable"))?
        .read_exact(&mut magic)
        .map_err(|_| chain_error(Rejected, NOT_NATIVE))?;
    let native = if cfg!(windows) {
        magic[..2] == *b"MZ"
    } else {
        matches!(
            magic,
            [0x7f, b'E', b'L', b'F']
                | [0xfe, 0xed, 0xfa, 0xce]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
    };
    if !native {
        return Err(chain_error(Rejected, NOT_NATIVE));
    }
    Ok(())
}

pub(crate) async fn run(argv: &[String], raw_payload: &str) -> Result<()> {
    validate(argv)?;
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .arg(raw_payload)
        .env(ACTIVE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = kmttt_pty::bounded::run(command, Duration::from_secs(3)).await?;
    if !status.success() {
        bail!("previous notifier exited with {status}");
    }
    Ok(())
}
