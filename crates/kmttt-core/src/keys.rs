//! 키바인딩 표기 해석: `"prefix+c"`, `"ctrl+b"`, `"prefix+minus"`, `"ctrl+alt+h"`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyName {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    F(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySpec {
    /// prefix를 누른 다음에 누르는 키인지
    pub after_prefix: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: KeyName,
}

impl KeySpec {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut spec = KeySpec {
            after_prefix: false,
            ctrl: false,
            alt: false,
            shift: false,
            key: KeyName::Esc,
        };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
            return Err(format!("bad key: {s}"));
        }
        let (last, mods) = parts.split_last().expect("non-empty");
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "prefix" => spec.after_prefix = true,
                "ctrl" | "control" => spec.ctrl = true,
                "alt" | "meta" | "option" => spec.alt = true,
                "shift" => spec.shift = true,
                other => return Err(format!("unknown modifier {other} in {s}")),
            }
        }
        let lower = last.to_ascii_lowercase();
        spec.key = match lower.as_str() {
            "minus" => KeyName::Char('-'),
            "plus" => KeyName::Char('+'),
            "space" => KeyName::Char(' '),
            "question" => KeyName::Char('?'),
            "enter" | "return" => KeyName::Enter,
            "esc" | "escape" => KeyName::Esc,
            "tab" => KeyName::Tab,
            "backspace" => KeyName::Backspace,
            "left" => KeyName::Left,
            "right" => KeyName::Right,
            "up" => KeyName::Up,
            "down" => KeyName::Down,
            f if f.starts_with('f') && f.len() > 1 && f[1..].parse::<u8>().is_ok() => {
                KeyName::F(f[1..].parse().expect("checked"))
            }
            _ => {
                let mut chars = last.chars();
                let c = chars.next().ok_or_else(|| format!("bad key: {s}"))?;
                if chars.next().is_some() {
                    return Err(format!("unknown key {last} in {s}"));
                }
                if c.is_ascii_uppercase() {
                    spec.shift = true;
                }
                KeyName::Char(c.to_ascii_lowercase())
            }
        };
        Ok(spec)
    }
}

/// 사용자가 바꿀 수 있는 동작들과 기본 키.
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("split_right", "prefix+v"),
    ("split_down", "prefix+minus"),
    ("new_tab", "prefix+c"),
    ("next_tab", "prefix+n"),
    ("prev_tab", "prefix+p"),
    ("workspaces", "prefix+w"),
    ("new_workspace", "prefix+shift+n"),
    ("zoom", "prefix+z"),
    ("detach", "prefix+q"),
    ("help", "prefix+question"),
    ("control", "prefix+d"),
    ("security", "prefix+s"),
    ("approvals", "prefix+a"),
    ("close_pane", "prefix+x"),
    ("focus_left", "prefix+h"),
    ("focus_right", "prefix+l"),
    ("focus_up", "prefix+k"),
    ("focus_down", "prefix+j"),
    ("mask_screen", "prefix+m"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeysConfig {
    pub prefix: String,
    #[serde(flatten)]
    pub actions: BTreeMap<String, String>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            prefix: "ctrl+b".into(),
            actions: BTreeMap::new(),
        }
    }
}

/// 해석된 바인딩 표.
#[derive(Debug, Clone)]
pub struct Bindings {
    pub prefix: KeySpec,
    pub map: Vec<(KeySpec, String)>,
}

impl Bindings {
    pub fn from_config(cfg: &KeysConfig) -> Result<Self, String> {
        let prefix = KeySpec::parse(&cfg.prefix)?;
        let mut map = Vec::new();
        for (action, default) in DEFAULT_BINDINGS {
            let s = cfg
                .actions
                .get(*action)
                .map(String::as_str)
                .unwrap_or(default);
            map.push((KeySpec::parse(s)?, action.to_string()));
        }
        for (action, s) in &cfg.actions {
            if !DEFAULT_BINDINGS.iter().any(|(a, _)| a == action) {
                return Err(format!("unknown action {action}"));
            }
            let _ = s;
        }
        Ok(Self { prefix, map })
    }

    /// 키에 맞는 동작. `after_prefix`는 바로 앞에 prefix가 눌렸는지.
    pub fn lookup(&self, key: &KeySpec) -> Option<&str> {
        self.map
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, a)| a.as_str())
    }

    /// 도움말 표(동작, 키 표기).
    pub fn help(&self, cfg: &KeysConfig) -> Vec<(String, String)> {
        DEFAULT_BINDINGS
            .iter()
            .map(|(a, d)| {
                let k = cfg
                    .actions
                    .get(*a)
                    .cloned()
                    .unwrap_or_else(|| d.to_string());
                (a.to_string(), k.replace("prefix", &cfg.prefix))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        let k = KeySpec::parse("prefix+minus").unwrap();
        assert!(k.after_prefix);
        assert_eq!(k.key, KeyName::Char('-'));
        let k = KeySpec::parse("ctrl+b").unwrap();
        assert!(k.ctrl && !k.after_prefix);
        let k = KeySpec::parse("prefix+shift+n").unwrap();
        assert!(k.shift);
        assert_eq!(KeySpec::parse("prefix+N").unwrap(), k);
        assert!(KeySpec::parse("hyper+x").is_err());
        assert_eq!(KeySpec::parse("f5").unwrap().key, KeyName::F(5));
    }

    #[test]
    fn defaults_and_override() {
        let mut cfg = KeysConfig::default();
        cfg.actions.insert("new_tab".into(), "prefix+t".into());
        let b = Bindings::from_config(&cfg).unwrap();
        assert_eq!(
            b.lookup(&KeySpec::parse("prefix+t").unwrap()),
            Some("new_tab")
        );
        assert_eq!(b.lookup(&KeySpec::parse("prefix+c").unwrap()), None);
        assert_eq!(
            b.lookup(&KeySpec::parse("prefix+v").unwrap()),
            Some("split_right")
        );
        cfg.actions.insert("fly".into(), "prefix+f".into());
        assert!(Bindings::from_config(&cfg).is_err());
    }
}
