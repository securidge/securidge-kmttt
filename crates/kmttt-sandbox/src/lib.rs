//! F4 pane 격리 실행.
//!
//! | 프로필 | Windows | Linux | macOS |
//! | none | 그대로 | 그대로 | 그대로 |
//! | restricted | Job Object(프로세스 수·메모리 상한) | Landlock(쓰기는 작업 폴더 등만) | sandbox-exec(쓰기 제한) |
//! | container | docker/podman | docker/podman | docker/podman |
//! | wsl | 지정 WSL 배포판 | — | — |
//!
//! restricted는 `kmttt sandbox-exec --root … -- 명령`으로 감싸 실행한다. 그 프로세스가
//! 스스로 제한을 건 뒤 실제 명령으로 교체(exec)되므로, pane 안의 모든 자손에 제한이 이어진다.

pub mod egress;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    None,
    Restricted,
    Container,
    Wsl,
}

impl Profile {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "" | "none" => Profile::None,
            "restricted" => Profile::Restricted,
            "container" => Profile::Container,
            "wsl" => Profile::Wsl,
            other => bail!("unknown sandbox profile '{other}'"),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::None => "none",
            Profile::Restricted => "restricted",
            Profile::Container => "container",
            Profile::Wsl => "wsl",
        }
    }
    /// 관제 화면에 그대로 보여 주는 실제 강도. 과장하지 않는다.
    pub fn strength(self) -> &'static str {
        match self {
            Profile::None => "none",
            Profile::Restricted => {
                if cfg!(windows) {
                    // Windows restricted는 아직 파일 쓰기를 막지 않는다(자원 상한만).
                    "low"
                } else {
                    "medium"
                }
            }
            Profile::Container => "high",
            Profile::Wsl => "medium",
        }
    }
}

/// 격리 실행에 필요한 설정(kmttt-policy의 SandboxPolicy에서 옮겨 옴).
#[derive(Debug, Clone)]
pub struct SandboxSettings {
    pub container_image: String,
    pub container_network: String,
    pub wsl_distro: String,
    pub memory_limit_mb: u64,
    pub process_limit: u32,
    /// 홈 폴더 아래 쓰기를 허용할 곳(에이전트 자체 설정·캐시)
    pub writable_home: Vec<String>,
    /// restricted에서 TCP 연결을 이그레스 프록시 포트(+허용 포트)로만 제한
    pub egress_enforce: bool,
    pub allow_tcp_ports: Vec<u16>,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            container_image: "node:22-bookworm".into(),
            container_network: "bridge".into(),
            wsl_distro: String::new(),
            memory_limit_mb: 8192,
            process_limit: 512,
            writable_home: default_writable_home(),
            egress_enforce: true,
            allow_tcp_ports: Vec::new(),
        }
    }
}

pub fn default_writable_home() -> Vec<String> {
    [
        ".claude",
        ".claude.json",
        ".codex",
        ".gemini",
        ".cache",
        ".npm",
        ".bun",
        ".local/state",
        ".cargo/registry",
        ".cargo/git",
        ".config/opencode",
        ".copilot",
        ".cursor",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 실제로 PTY에서 실행할 명령.
#[derive(Debug, Clone, PartialEq)]
pub struct Wrapped {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// 명령을 프로필에 맞게 감싼다.
pub fn wrap(
    profile: Profile,
    program: &str,
    args: &[String],
    root: &Path,
    settings: &SandboxSettings,
    egress_port: Option<u16>,
    kmttt_exe: &Path,
) -> Result<Wrapped> {
    let mut env = Vec::new();
    if let Some(port) = egress_port {
        let url = format!("http://127.0.0.1:{port}");
        for k in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "http_proxy",
            "https_proxy",
            "ALL_PROXY",
        ] {
            env.push((k.to_string(), url.clone()));
        }
        env.push(("NO_PROXY".into(), "localhost,127.0.0.1".into()));
    }
    env.push(("KMTTT_SANDBOX".into(), profile.as_str().into()));
    Ok(match profile {
        Profile::None => Wrapped {
            program: program.into(),
            args: args.to_vec(),
            env,
        },
        Profile::Restricted => {
            let mut a = vec![
                "sandbox-exec".to_string(),
                "--root".into(),
                root.display().to_string(),
                "--memory-mb".into(),
                settings.memory_limit_mb.to_string(),
                "--processes".into(),
                settings.process_limit.to_string(),
            ];
            for w in &settings.writable_home {
                a.push("--home-writable".into());
                a.push(w.clone());
            }
            if let (Some(port), true) = (egress_port, settings.egress_enforce) {
                a.push("--egress-port".into());
                a.push(port.to_string());
                for p in &settings.allow_tcp_ports {
                    a.push("--allow-tcp-port".into());
                    a.push(p.to_string());
                }
            }
            a.push("--".into());
            a.push(program.into());
            a.extend(args.iter().cloned());
            Wrapped {
                program: kmttt_exe.display().to_string(),
                args: a,
                env,
            }
        }
        Profile::Container => {
            let engine = container_engine().ok_or_else(|| {
                anyhow::anyhow!("container profile needs docker or podman on PATH")
            })?;
            let mut a = vec![
                "run".to_string(),
                "--rm".into(),
                "-it".into(),
                "--init".into(),
                "-v".into(),
                format!("{}:/work", root.display()),
                "-w".into(),
                "/work".into(),
                "-e".into(),
                "TERM=xterm-256color".into(),
                "--network".into(),
                settings.container_network.clone(),
            ];
            if let Some(port) = egress_port {
                // 컨테이너 안에서는 호스트의 프록시로 나간다.
                let url = format!("http://host.docker.internal:{port}");
                for k in ["HTTP_PROXY", "HTTPS_PROXY"] {
                    a.push("-e".into());
                    a.push(format!("{k}={url}"));
                }
                a.push("--add-host".into());
                a.push("host.docker.internal:host-gateway".into());
            }
            a.push(settings.container_image.clone());
            a.push(program.into());
            a.extend(args.iter().cloned());
            Wrapped {
                program: engine,
                args: a,
                env: vec![("KMTTT_SANDBOX".into(), "container".into())],
            }
        }
        Profile::Wsl => {
            if !cfg!(windows) {
                bail!("wsl profile is only available on Windows");
            }
            let mut a = Vec::new();
            if !settings.wsl_distro.is_empty() {
                a.push("-d".to_string());
                a.push(settings.wsl_distro.clone());
            }
            a.push("--cd".into());
            a.push(root.display().to_string());
            a.push("--".into());
            a.push(program.into());
            a.extend(args.iter().cloned());
            Wrapped {
                program: "wsl.exe".into(),
                args: a,
                env,
            }
        }
    })
}

fn container_engine() -> Option<String> {
    ["docker", "podman"]
        .into_iter()
        .find(|e| which(e))
        .map(String::from)
}

fn which(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|d| d.join(name).is_file() || d.join(format!("{name}.exe")).is_file())
}

/// `kmttt sandbox-exec` 인자.
#[derive(Debug, Clone)]
pub struct RestrictedArgs {
    pub root: PathBuf,
    pub memory_mb: u64,
    pub processes: u32,
    pub home_writable: Vec<String>,
    /// 있으면 TCP 연결을 이 포트(이그레스 프록시)와 `allow_tcp_ports`로만 허용한다.
    pub egress_port: Option<u16>,
    pub allow_tcp_ports: Vec<u16>,
    pub program: String,
    pub args: Vec<String>,
}

impl RestrictedArgs {
    /// 연결을 허용할 TCP 포트(강제 차단을 안 하면 None).
    pub fn tcp_ports(&self) -> Option<Vec<u16>> {
        let p = self.egress_port?;
        let mut v = vec![p];
        v.extend(self.allow_tcp_ports.iter().copied());
        v.sort_unstable();
        v.dedup();
        Some(v)
    }
}

/// 쓰기를 허용할 경로 목록(작업 폴더, 임시 폴더, 장치, 에이전트 설정).
pub fn writable_paths(a: &RestrictedArgs) -> Vec<PathBuf> {
    let mut v = vec![a.root.clone(), std::env::temp_dir()];
    if cfg!(unix) {
        v.push(PathBuf::from("/tmp"));
        v.push(PathBuf::from("/dev"));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        for w in &a.home_writable {
            v.push(home.join(w));
        }
    }
    v
}

/// 제한을 건 뒤 명령으로 교체한다. 성공하면 돌아오지 않는다.
pub fn exec_restricted(a: &RestrictedArgs) -> Result<std::convert::Infallible> {
    platform::exec_restricted(a)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use landlock::{
        path_beneath_rules, Access, AccessFs, AccessNet, LandlockStatus, NetPort, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
    };
    use std::os::unix::process::CommandExt;

    pub fn exec_restricted(a: &RestrictedArgs) -> Result<std::convert::Infallible> {
        let ports = a.tcp_ports();
        // 네트워크 규칙은 ABI v4(Linux 6.7+)부터. 파일 규칙은 v3까지만 쓰고, 네트워크가 필요할 때만 v4를 요구한다.
        let fs_abi = ABI::V3;
        let mut rs = Ruleset::default().handle_access(AccessFs::from_all(fs_abi))?;
        if ports.is_some() {
            rs = rs.handle_access(AccessNet::ConnectTcp)?;
        }
        let mut created = rs
            .create()?
            .add_rules(path_beneath_rules(["/"], AccessFs::from_read(fs_abi)))?
            .add_rules(path_beneath_rules(
                writable_paths(a),
                AccessFs::from_all(fs_abi),
            ))?;
        for p in ports.iter().flatten() {
            created = created.add_rule(NetPort::new(*p, AccessNet::ConnectTcp))?;
        }
        let status = created.restrict_self()?;
        let net_ok = match status.landlock {
            LandlockStatus::Available { effective_abi, .. } => effective_abi >= ABI::V4,
            _ => false,
        };
        match status.ruleset {
            RulesetStatus::FullyEnforced => {}
            RulesetStatus::PartiallyEnforced => eprintln!("kmttt: Landlock is only partially enforced on this kernel"),
            RulesetStatus::NotEnforced => eprintln!("kmttt: WARNING Landlock is not available on this kernel; running WITHOUT file restrictions"),
        }
        if ports.is_some() && !net_ok {
            eprintln!("kmttt: WARNING this kernel has no Landlock network rules (needs Linux 6.7+); outbound connections are NOT restricted");
        }
        let err = std::process::Command::new(&a.program).args(&a.args).exec();
        Err(anyhow::anyhow!("exec {}: {err}", a.program))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn quote(p: &Path) -> String {
        format!(
            "\"{}\"",
            p.display()
                .to_string()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        )
    }

    pub fn profile(a: &RestrictedArgs) -> String {
        let mut s = String::from("(version 1)(allow default)(deny file-write*)(allow file-write*");
        for p in writable_paths(a) {
            s.push_str(&format!(" (subpath {})", quote(&p)));
        }
        s.push_str(
            " (subpath \"/private/tmp\") (subpath \"/private/var/folders\") (regex #\"^/dev/\"))",
        );
        if let Some(ports) = a.tcp_ports() {
            // 밖으로 나가는 연결은 로컬 이그레스 프록시(+허용 포트)와 Unix 소켓만.
            s.push_str("(deny network-outbound)(allow network-outbound (remote unix-socket))");
            for p in ports {
                s.push_str(&format!(
                    "(allow network-outbound (remote ip \"localhost:{p}\"))"
                ));
            }
        }
        s
    }

    pub fn exec_restricted(a: &RestrictedArgs) -> Result<std::convert::Infallible> {
        let err = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(profile(a))
            .arg(&a.program)
            .args(&a.args)
            .exec();
        Err(anyhow::anyhow!("exec sandbox-exec: {err}"))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// 이 프로세스를 Job Object에 넣고(자식도 따라 들어감) 명령을 실행한 뒤 종료 코드로 끝낸다.
    pub fn exec_restricted(a: &RestrictedArgs) -> Result<std::convert::Infallible> {
        // SAFETY: 아래 Win32 호출은 모두 이 함수 안에서 만든 핸들과 0으로 초기화한 구조체만 쓴다.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                bail!(
                    "CreateJobObjectW failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.BasicLimitInformation.ActiveProcessLimit = a.processes.max(8);
            info.JobMemoryLimit = (a.memory_mb.max(256) as usize) * 1024 * 1024;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                bail!(
                    "SetInformationJobObject failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                bail!(
                    "AssignProcessToJobObject failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
        eprintln!("kmttt: restricted (Windows): process/memory limits only; file writes are NOT restricted — use the container or wsl profile for stronger isolation");
        let status = std::process::Command::new(&a.program)
            .args(&a.args)
            .current_dir(&a.root)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;
    pub fn exec_restricted(_a: &RestrictedArgs) -> Result<std::convert::Infallible> {
        bail!("restricted profile is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_parse() {
        assert_eq!(Profile::parse("restricted").unwrap(), Profile::Restricted);
        assert_eq!(Profile::parse("").unwrap(), Profile::None);
        assert!(Profile::parse("jail").is_err());
        assert_eq!(Profile::Container.strength(), "high");
        assert_eq!(Profile::None.strength(), "none");
    }

    #[test]
    fn wrap_none_and_restricted() {
        let s = SandboxSettings::default();
        let w = wrap(
            Profile::None,
            "bash",
            &["-l".into()],
            Path::new("/w"),
            &s,
            Some(9000),
            Path::new("/bin/kmttt"),
        )
        .unwrap();
        assert_eq!(w.program, "bash");
        assert!(w
            .env
            .iter()
            .any(|(k, v)| k == "HTTPS_PROXY" && v == "http://127.0.0.1:9000"));
        let w = wrap(
            Profile::Restricted,
            "claude",
            &[],
            Path::new("/w"),
            &s,
            None,
            Path::new("/bin/kmttt"),
        )
        .unwrap();
        assert_eq!(w.program, "/bin/kmttt");
        assert_eq!(w.args[0], "sandbox-exec");
        let dash = w.args.iter().position(|a| a == "--").unwrap();
        assert_eq!(w.args[dash + 1], "claude");
        assert!(w.args.contains(&"--home-writable".to_string()));
    }

    #[cfg(not(windows))]
    #[test]
    fn wsl_refused_off_windows() {
        let s = SandboxSettings::default();
        assert!(wrap(
            Profile::Wsl,
            "bash",
            &[],
            Path::new("/w"),
            &s,
            None,
            Path::new("k")
        )
        .is_err());
    }

    #[test]
    fn writable_includes_root_and_home_bits() {
        let a = RestrictedArgs {
            root: PathBuf::from("/repo"),
            memory_mb: 1,
            processes: 1,
            home_writable: vec![".claude".into()],
            egress_port: None,
            allow_tcp_ports: vec![],
            program: "x".into(),
            args: vec![],
        };
        let w = writable_paths(&a);
        assert!(w.contains(&PathBuf::from("/repo")));
        assert!(w.iter().any(|p| p.ends_with(".claude")));
        assert_eq!(a.tcp_ports(), None);
    }

    #[test]
    fn tcp_ports_only_when_enforced() {
        let a = RestrictedArgs {
            root: PathBuf::from("/repo"),
            memory_mb: 1,
            processes: 1,
            home_writable: vec![],
            egress_port: Some(9000),
            allow_tcp_ports: vec![3000, 9000],
            program: "x".into(),
            args: vec![],
        };
        assert_eq!(a.tcp_ports(), Some(vec![3000, 9000]));
    }

    #[test]
    fn restricted_wrap_passes_egress_port() {
        let s = SandboxSettings {
            allow_tcp_ports: vec![5432],
            ..Default::default()
        };
        let w = wrap(
            Profile::Restricted,
            "sh",
            &[],
            Path::new("/r"),
            &s,
            Some(9000),
            Path::new("kmttt"),
        )
        .unwrap();
        let joined = w.args.join(" ");
        assert!(joined.contains("--egress-port 9000"), "{joined}");
        assert!(joined.contains("--allow-tcp-port 5432"), "{joined}");
        let off = SandboxSettings {
            egress_enforce: false,
            ..Default::default()
        };
        let w = wrap(
            Profile::Restricted,
            "sh",
            &[],
            Path::new("/r"),
            &off,
            Some(9000),
            Path::new("kmttt"),
        )
        .unwrap();
        assert!(!w.args.join(" ").contains("--egress-port"));
    }

    /// macOS: 실제 sandbox-exec로 허용 포트만 연결되는지 확인한다(CI macOS에서 실행).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_blocks_other_tcp_ports() {
        let allowed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ap = allowed.local_addr().unwrap().port();
        let op = other.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("kmttt-sbx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = RestrictedArgs {
            root: dir.clone(),
            memory_mb: 1,
            processes: 1,
            home_writable: vec![],
            egress_port: Some(ap),
            allow_tcp_ports: vec![],
            program: "x".into(),
            args: vec![],
        };
        let script =
            format!("nc -z -G 3 127.0.0.1 {ap}; echo A=$?; nc -z -G 3 127.0.0.1 {op}; echo B=$?");
        let out = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(platform::profile(&a))
            .arg("/bin/sh")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("A=0"),
            "allowed port should connect: {text} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!text.contains("B=0"), "other port must be blocked: {text}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
