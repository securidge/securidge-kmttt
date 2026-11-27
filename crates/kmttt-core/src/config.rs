//! `config.toml`. 파일이 없어도 기본값으로 동작한다.

use crate::keys::KeysConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// "en" 또는 "ko"
    pub language: String,
    pub onboarding: bool,
    pub keys: KeysConfig,
    pub ui: UiConfig,
    pub shell: ShellConfig,
    pub worktrees: WorktreesConfig,
    pub experimental: ExperimentalConfig,
    pub ledger: LedgerConfig,
    pub console: ConsoleConfig,
    /// 마스킹 규칙 추가·예외(kmttt-redact의 RedactConfig와 같은 모양)
    pub redact: toml::Table,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en".into(),
            onboarding: true,
            keys: KeysConfig::default(),
            ui: UiConfig::default(),
            shell: ShellConfig::default(),
            worktrees: WorktreesConfig::default(),
            experimental: ExperimentalConfig::default(),
            ledger: LedgerConfig::default(),
            console: ConsoleConfig::default(),
            redact: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub sidebar_width: u16,
    pub redraw_on_focus_gained: bool,
    /// 화면 공유 모드: 화면에 보이는 비밀도 가린다
    pub mask_screen: bool,
    pub toast: ToastConfig,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            sidebar_width: 30,
            redraw_on_focus_gained: true,
            mask_screen: false,
            toast: ToastConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToastConfig {
    /// "kmttt"(앱 안), "terminal"(바깥 터미널 알림), "os"(운영체제 알림), "off"
    pub delivery: String,
    pub delay_seconds: u64,
}

impl Default for ToastConfig {
    fn default() -> Self {
        Self {
            delivery: "kmttt".into(),
            delay_seconds: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ShellConfig {
    /// 비워 두면 플랫폼 기본 셸
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct WorktreesConfig {
    /// 비워 두면 `~/.kmttt/worktrees`
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ExperimentalConfig {
    /// 서버 재시작 후 pane 텍스트 일부를 복원(마스킹 후 저장)
    pub pane_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LedgerConfig {
    /// "full", "metadata", "off"
    pub level: String,
    /// 기록 데이터를 저장할 때 암호화
    pub encrypt: bool,
    /// 체크포인트 간격(초 / 건수)
    pub checkpoint_secs: u64,
    pub checkpoint_records: u64,
    /// 각 기록을 이 파일에도 덧붙임(다른 디스크·공유 폴더로 반출)
    pub mirror_file: String,
    /// "host:port"로 RFC 5424 syslog(UDP) 전송
    pub syslog_udp: String,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            level: "metadata".into(),
            encrypt: false,
            checkpoint_secs: 60,
            checkpoint_records: 1000,
            mirror_file: String::new(),
            syslog_udp: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ConsoleConfig {
    /// 웹 콘솔에서 승인·거부 같은 조작을 허용
    pub allow_actions: bool,
    /// 0이면 임의 포트
    pub port: u16,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.language.as_str(), "en" | "ko"),
            "language must be \"en\" or \"ko\""
        );
        anyhow::ensure!(
            matches!(self.ledger.level.as_str(), "full" | "metadata" | "off"),
            "ledger.level must be full, metadata or off"
        );
        anyhow::ensure!(
            matches!(
                self.ui.toast.delivery.as_str(),
                "kmttt" | "terminal" | "os" | "off"
            ),
            "ui.toast.delivery must be kmttt, terminal, os or off"
        );
        crate::keys::Bindings::from_config(&self.keys).map_err(anyhow::Error::msg)?;
        Ok(())
    }

    pub fn worktree_root(&self) -> std::path::PathBuf {
        if self.worktrees.directory.is_empty() {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".kmttt")
                .join("worktrees")
        } else {
            std::path::PathBuf::from(&self.worktrees.directory)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_sections() {
        let c = Config::parse(
            r#"
            language = "ko"
            [keys]
            prefix = "ctrl+a"
            new_tab = "prefix+t"
            [ui.toast]
            delivery = "os"
            [ledger]
            level = "full"
            [experimental]
            pane_history = true
            "#,
        )
        .unwrap();
        assert_eq!(c.language, "ko");
        assert_eq!(c.keys.prefix, "ctrl+a");
        assert_eq!(c.keys.actions.get("new_tab").unwrap(), "prefix+t");
        assert_eq!(c.ui.toast.delivery, "os");
        assert!(c.experimental.pane_history);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::parse("language = \"fr\"").is_err());
        assert!(Config::parse("[ledger]\nlevel = \"everything\"").is_err());
        assert!(Config::parse("[keys]\nzoom = \"hyper+z\"").is_err());
    }
}
