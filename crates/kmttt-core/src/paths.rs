//! 설정·데이터·소켓 경로.
//!
//! 환경 변수로 바꿀 수 있다(테스트와 여러 인스턴스용):
//! `KMTTT_CONFIG_DIR`, `KMTTT_DATA_DIR`, `KMTTT_SOCKET_DIR`.

use std::path::PathBuf;

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// 설정 폴더: Windows `%APPDATA%\Securidge\kmttt`, 그 밖 `~/.config/kmttt`.
pub fn config_dir() -> PathBuf {
    if let Some(d) = env_dir("KMTTT_CONFIG_DIR") {
        return d;
    }
    #[cfg(windows)]
    {
        dirs::config_dir()
            .unwrap_or_else(home)
            .join("Securidge")
            .join("kmttt")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("kmttt")
    }
}

/// 데이터 폴더(기록, 세션 저장): Windows `%LOCALAPPDATA%\Securidge\kmttt`, 그 밖 `~/.local/share/kmttt`.
pub fn data_dir() -> PathBuf {
    if let Some(d) = env_dir("KMTTT_DATA_DIR") {
        return d;
    }
    #[cfg(windows)]
    {
        dirs::data_local_dir()
            .unwrap_or_else(home)
            .join("Securidge")
            .join("kmttt")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local").join("share"))
            .join("kmttt")
    }
}

/// 모든 세션 기록의 상위 폴더(지우기·안내용).
pub fn ledger_root() -> PathBuf {
    data_dir().join("ledger")
}

/// 세션별 기록 폴더. 세션마다 서버가 따로라 해시 체인도 따로여야 한다(쓰는 쪽이 하나뿐이어야 함).
pub fn ledger_dir(session: &str) -> PathBuf {
    ledger_root().join(session)
}

/// 같은 세션 서버가 둘 뜨지 않게 잡는 잠금 파일.
pub fn server_lock_file(session: &str) -> PathBuf {
    data_dir().join(format!("server-{session}.lock"))
}

pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

pub fn manifests_dir() -> PathBuf {
    config_dir().join("manifests")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn policy_file() -> PathBuf {
    config_dir().join("policy.toml")
}

pub fn machines_file() -> PathBuf {
    config_dir().join("machines.toml")
}

pub fn log_file() -> PathBuf {
    data_dir().join("kmttt-server.log")
}

/// 세션 이름은 영숫자·`-`·`_`만 허용한다(경로·파이프 이름에 들어가므로).
pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Unix 소켓 폴더. 폴더 권한은 서버가 0700으로 맞춘다.
#[cfg(unix)]
pub fn socket_dir() -> PathBuf {
    if let Some(d) = env_dir("KMTTT_SOCKET_DIR") {
        return d;
    }
    if let Some(r) = env_dir("XDG_RUNTIME_DIR") {
        return r.join("kmttt");
    }
    // SAFETY: getuid는 실패하지 않는 단순 조회다.
    let uid = unsafe { libc_getuid() };
    std::env::temp_dir().join(format!("kmttt-{uid}"))
}

#[cfg(unix)]
extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// 세션의 소켓 주소. Unix는 파일 경로, Windows는 named pipe 이름.
pub fn socket_path(session: &str) -> String {
    #[cfg(unix)]
    {
        socket_dir()
            .join(format!("{session}.sock"))
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let suffix = env_dir("KMTTT_SOCKET_DIR")
            .map(|d| format!("-{:x}", simple_hash(&d.to_string_lossy())))
            .unwrap_or_default();
        format!(r"\\.\pipe\kmttt-{user}-{session}{suffix}")
    }
}

#[cfg(windows)]
fn simple_hash(s: &str) -> u32 {
    s.bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names() {
        assert!(valid_session_name("default"));
        assert!(valid_session_name("work-2_a"));
        assert!(!valid_session_name("../x"));
        assert!(!valid_session_name(""));
        assert!(!valid_session_name("a b"));
    }

    #[test]
    fn socket_contains_session() {
        assert!(socket_path("abc").contains("abc"));
    }
}
