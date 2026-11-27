//! config.toml·policy.toml 읽기와 바꾸기.
//!
//! 사용자가 손으로 쓴 주석·순서를 지키려고 toml_edit으로 바뀐 칸만 고친다. 쓰기 전에 서버와 같은
//! 검사(`Config::parse`, `PolicyConfig::parse`)를 통과해야 하고, 원래 파일은 `.bak`으로 남긴다.

use anyhow::{anyhow, bail, Context, Result};
use kmttt_core::config::Config;
use kmttt_core::paths;
use kmttt_policy::PolicyConfig;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table};

/// 화면에서 바꾼 한 칸. `path`는 TOML 키 경로(예: ["sandbox", "default_profile"]).
#[derive(Debug, Deserialize)]
pub struct Change {
    pub path: Vec<String>,
    /// null이면 키를 지운다(기본값으로 돌아감).
    pub value: Value,
}

pub fn file_path(which: &str) -> Result<PathBuf> {
    match which {
        "config" => Ok(paths::config_file()),
        "policy" => Ok(paths::policy_file()),
        _ => bail!("unknown settings file '{which}'"),
    }
}

/// 두 파일의 실제 적용값(기본값 포함)과 위치, 읽기 오류.
pub fn read_all() -> Value {
    let cfg_path = paths::config_file();
    let pol_path = paths::policy_file();
    let (cfg, cfg_err) = match Config::load(&cfg_path) {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(format!("{e:#}"))),
    };
    let (pol, pol_err) = match PolicyConfig::load(&pol_path) {
        Ok(p) => (p, None),
        Err(e) => (PolicyConfig::default(), Some(format!("{e:#}"))),
    };
    json!({
        "config": cfg,
        "policy": pol,
        "config_path": cfg_path.display().to_string(),
        "policy_path": pol_path.display().to_string(),
        "config_error": cfg_err,
        "policy_error": pol_err,
    })
}

/// 바꾼 내용을 적용한 새 파일 내용. 검사를 통과하지 못하면 오류.
pub fn render(which: &str, current: &str, changes: &[Change]) -> Result<String> {
    let mut doc: DocumentMut = current
        .parse()
        .with_context(|| format!("{which}.toml is not valid TOML"))?;
    for ch in changes {
        set_path(doc.as_table_mut(), &ch.path, &ch.value)
            .with_context(|| format!("set {}", ch.path.join(".")))?;
    }
    let out = doc.to_string();
    match which {
        "config" => {
            Config::parse(&out)?;
        }
        "policy" => {
            PolicyConfig::parse(&out)?;
        }
        _ => bail!("unknown settings file '{which}'"),
    }
    Ok(out)
}

/// 파일을 원자적으로 바꾼다. 이전 내용을 돌려준다(없었으면 None).
pub fn write(which: &str, text: &str) -> Result<Option<String>> {
    let path = file_path(which)?;
    let old = std::fs::read_to_string(&path).ok();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(o) = &old {
        let _ = std::fs::write(path.with_extension("toml.bak"), o);
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(old)
}

/// 이전 내용으로 되돌린다(None이면 파일을 지운다).
pub fn restore(which: &str, old: Option<&str>) -> Result<()> {
    let path = file_path(which)?;
    match old {
        Some(t) => std::fs::write(&path, t)?,
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

fn set_path(root: &mut Table, path: &[String], v: &Value) -> Result<()> {
    let Some((last, parents)) = path.split_last() else {
        bail!("empty key path");
    };
    for k in path {
        if k.is_empty() || k.len() > 64 {
            bail!("bad key");
        }
    }
    let mut t = root;
    for k in parents {
        if !t.contains_key(k) {
            let mut nt = Table::new();
            nt.set_implicit(true);
            t.insert(k, Item::Table(nt));
        }
        t = t
            .get_mut(k)
            .and_then(|i| i.as_table_mut())
            .ok_or_else(|| anyhow!("'{k}' is not a table"))?;
    }
    match v {
        Value::Null => {
            t.remove(last);
        }
        Value::Array(a) if !a.is_empty() && a.iter().all(|x| x.is_object()) => {
            let mut aot = ArrayOfTables::new();
            for x in a {
                let mut tb = Table::new();
                if let Value::Object(m) = x {
                    for (k, xv) in m {
                        if !xv.is_null() {
                            tb.insert(k, Item::Value(to_toml(xv)?));
                        }
                    }
                }
                aot.push(tb);
            }
            t.insert(last, Item::ArrayOfTables(aot));
        }
        Value::Array(a) if a.is_empty() && matches!(last.as_str(), "rule" | "tool_rule") => {
            t.remove(last);
        }
        Value::Object(m) => {
            let mut tb = Table::new();
            for (k, xv) in m {
                if !xv.is_null() {
                    tb.insert(k, Item::Value(to_toml(xv)?));
                }
            }
            t.insert(last, Item::Table(tb));
        }
        other => {
            let nv = to_toml(other)?;
            // 있던 키는 제자리에서 값만 바꿔 키 앞의 주석을 지킨다.
            match t.get_mut(last) {
                Some(item) if item.is_value() => *item = Item::Value(nv),
                _ => {
                    t.insert(last, Item::Value(nv));
                }
            }
        }
    }
    Ok(())
}

fn to_toml(v: &Value) -> Result<toml_edit::Value> {
    Ok(match v {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                bail!("number out of range")
            }
        }
        Value::String(s) => s.as_str().into(),
        Value::Array(a) => {
            let mut arr = Array::new();
            for x in a {
                arr.push(to_toml(x)?);
            }
            toml_edit::Value::Array(arr)
        }
        Value::Object(m) => {
            let mut it = InlineTable::new();
            for (k, x) in m {
                if !x.is_null() {
                    it.insert(k, to_toml(x)?);
                }
            }
            toml_edit::Value::InlineTable(it)
        }
        Value::Null => bail!("null inside a value"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(path: &[&str], value: Value) -> Change {
        Change {
            path: path.iter().map(|s| s.to_string()).collect(),
            value,
        }
    }

    #[test]
    fn keeps_comments_and_sets_nested_keys() {
        let cur = "# my notes\n[defaults]\n# keep me\nask_timeout_secs = 60\n";
        let out = render(
            "policy",
            cur,
            &[
                ch(&["defaults", "ask_timeout_secs"], json!(90)),
                ch(&["sandbox", "default_profile"], json!("restricted")),
            ],
        )
        .unwrap();
        assert!(out.contains("# my notes"));
        assert!(out.contains("# keep me"));
        assert!(out.contains("ask_timeout_secs = 90"));
        let p = PolicyConfig::parse(&out).unwrap();
        assert_eq!(p.sandbox.default_profile, "restricted");
        assert_eq!(p.defaults.ask_timeout_secs, 90);
    }

    #[test]
    fn rejects_values_the_server_would_reject() {
        let e = render(
            "policy",
            "",
            &[ch(&["sandbox", "default_profile"], json!("bogus"))],
        );
        assert!(e.is_err());
        let e = render("config", "", &[ch(&["language"], json!(5))]);
        assert!(e.is_err());
    }

    #[test]
    fn rules_become_array_of_tables_and_empty_removes() {
        let rules = json!([{"id": "r1", "action": "prompt", "decision": "deny", "from": {"agent": "codex"}, "to": {}}]);
        let out = render("policy", "", &[ch(&["rule"], rules)]).unwrap();
        assert!(out.contains("[[rule]]"));
        let p = PolicyConfig::parse(&out).unwrap();
        assert_eq!(p.rule.len(), 1);
        let out2 = render("policy", &out, &[ch(&["rule"], json!([]))]).unwrap();
        assert!(!out2.contains("[[rule]]"));
    }

    #[test]
    fn null_removes_key() {
        let out = render(
            "config",
            "language = \"ko\"\n",
            &[ch(&["language"], Value::Null)],
        )
        .unwrap();
        assert!(!out.contains("language"));
    }
}
