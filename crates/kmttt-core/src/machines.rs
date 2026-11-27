//! 원격 머신 프로필(`machines.toml`). 비밀번호·키는 저장하지 않고 OpenSSH에 맡긴다.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineProfile {
    pub id: String,
    pub label: String,
    /// ssh 대상: "user@host", "ssh://user@host:2222", 또는 ~/.ssh/config의 Host 이름
    pub target: String,
    #[serde(default = "default_session")]
    pub session: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 원격 kmttt 실행 파일 이름/경로
    #[serde(default = "default_bin")]
    pub bin: String,
}

fn default_session() -> String {
    "default".into()
}
fn yes() -> bool {
    true
}
fn default_bin() -> String {
    "kmttt".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Machines {
    #[serde(default)]
    pub machine: Vec<MachineProfile>,
}

impl Machines {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(toml::from_str(&t)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// 다음 ID(m1, m2, …)
    pub fn next_id(&self) -> String {
        let n = self
            .machine
            .iter()
            .filter_map(|m| m.id.strip_prefix('m').and_then(|x| x.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);
        format!("m{}", n + 1)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut MachineProfile> {
        self.machine.iter_mut().find(|m| m.id == id)
    }
}

/// ssh 대상 문자열이 옵션 주입처럼 보이면 거부한다.
pub fn valid_target(t: &str) -> bool {
    !t.is_empty()
        && !t.starts_with('-')
        && !t
            .chars()
            .any(|c| c.is_whitespace() || c == ';' || c == '`' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = std::env::temp_dir().join(format!("kmttt-m-{}", std::process::id()));
        let f = d.join("machines.toml");
        let mut m = Machines::default();
        m.machine.push(MachineProfile {
            id: m.next_id(),
            label: "workbox".into(),
            target: "me@box".into(),
            session: "default".into(),
            enabled: true,
            bin: "kmttt".into(),
        });
        assert_eq!(m.next_id(), "m2");
        m.save(&f).unwrap();
        assert_eq!(Machines::load(&f).unwrap(), m);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn targets() {
        assert!(valid_target("me@box"));
        assert!(valid_target("ssh://me@box:22"));
        assert!(!valid_target("-oProxyCommand=x"));
        assert!(!valid_target("a b"));
    }
}
