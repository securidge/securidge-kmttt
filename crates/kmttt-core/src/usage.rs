//! AI 에이전트 요금제 사용량(남은 양 %와 리셋 시각) 읽기.
//!
//! - Claude Code: 공식 상태 줄(statusLine) 입력 JSON의 `rate_limits.five_hour`·`seven_day`
//!   (`used_percentage` 0~100, `resets_at` 유닉스 초). Claude Code가 상태 줄을 그릴 때마다
//!   `kmttt statusline`이 받아 [`claude_usage_file`]에 숫자만 저장한다. 로그인 정보는 읽지 않는다.
//! - Codex CLI: `~/.codex/sessions/**/rollout-*.jsonl(.zst)`의 `token_count` 이벤트에 들어 있는
//!   `rate_limits.primary`·`secondary`(`used_percent`, `window_minutes`, `resets_at`).
//!
//! 두 값 모두 에이전트가 마지막으로 응답을 받은 시점의 값이다. 에이전트를 쓰지 않는 동안에는
//! 갱신되지 않으므로 `updated_ms`를 함께 보여 줘야 한다.

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 한도 창 하나(예: 5시간, 7일).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// 쓴 비율(0~100). 100을 넘을 수 있다.
    pub used_percent: f64,
    /// 리셋 시각(유닉스 초). 모르면 None.
    pub resets_at: Option<i64>,
    /// 창 길이(분). 모르면 None.
    pub window_minutes: Option<u64>,
}

/// 에이전트 하나의 사용량.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentUsage {
    /// "claude" 또는 "codex"
    pub agent: String,
    /// 짧은 창(Claude 5시간, Codex primary)
    pub primary: Option<Window>,
    /// 긴 창(Claude 7일, Codex secondary)
    pub secondary: Option<Window>,
    /// 이 값이 기록된 시각(유닉스 밀리초)
    pub updated_ms: i64,
    /// 어디서 읽었나(사람이 보는 설명)
    pub source: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Claude Code 사용량을 저장하는 파일.
pub fn claude_usage_file() -> PathBuf {
    paths::data_dir().join("usage").join("claude.json")
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn epoch(v: &Value) -> Option<i64> {
    let x = num(v)?;
    // 밀리초로 오는 경우도 받아 준다.
    let s = if x > 1e12 { x / 1000.0 } else { x };
    (s > 0.0 && s < 1e11).then_some(s as i64)
}

fn claude_window(v: &Value, minutes: u64) -> Option<Window> {
    let used = num(&v["used_percentage"])?;
    Some(Window {
        used_percent: used.clamp(0.0, 1000.0),
        resets_at: epoch(&v["resets_at"]),
        window_minutes: Some(minutes),
    })
}

/// Claude Code 상태 줄 입력에서 사용량을 뽑는다. `rate_limits`가 없으면 None
/// (API 키 사용자이거나 세션의 첫 응답 전).
pub fn parse_claude_statusline(input: &Value) -> Option<AgentUsage> {
    let rl = input.get("rate_limits")?;
    let primary = rl.get("five_hour").and_then(|w| claude_window(w, 300));
    let secondary = rl.get("seven_day").and_then(|w| claude_window(w, 10080));
    if primary.is_none() && secondary.is_none() {
        return None;
    }
    Some(AgentUsage {
        agent: "claude".into(),
        primary,
        secondary,
        updated_ms: now_ms(),
        source: "Claude Code status line".into(),
    })
}

/// 상태 줄 입력을 받아 사용량을 파일에 저장한다(숫자만 저장, 다른 내용은 버림).
pub fn record_claude_statusline(input: &Value) -> std::io::Result<Option<AgentUsage>> {
    let Some(u) = parse_claude_statusline(input) else {
        return Ok(None);
    };
    let f = claude_usage_file();
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = f.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&u)?)?;
    std::fs::rename(&tmp, &f)?;
    Ok(Some(u))
}

pub fn read_claude() -> Option<AgentUsage> {
    let text = std::fs::read(claude_usage_file()).ok()?;
    if text.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&text).ok()
}

/// Codex 세션 기록 폴더(`CODEX_HOME`이 있으면 그 아래).
pub fn codex_sessions_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".codex")
        })
        .join("sessions")
}

/// 압축 기록을 풀 때 최대 크기. 세션 기록이 아주 커도 메모리를 많이 쓰지 않게 한다.
const MAX_READ: u64 = 32 * 1024 * 1024;
/// 일반 기록은 끝부분만 읽는다(rate_limits는 응답마다 기록되므로 끝에 있다).
const TAIL_READ: u64 = 4 * 1024 * 1024;

fn rollout_files(dir: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() && depth < 4 {
                stack.push((p, depth + 1));
            } else if ft.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("rollout-")
                    && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
                {
                    if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                        out.push((m, p));
                    }
                }
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    out
}

fn read_rollout(p: &Path) -> Option<String> {
    let f = std::fs::File::open(p).ok()?;
    let mut buf = Vec::new();
    if p.extension().is_some_and(|e| e == "zst") {
        let dec = ruzstd::decoding::StreamingDecoder::new(f).ok()?;
        dec.take(MAX_READ).read_to_end(&mut buf).ok()?;
    } else {
        let len = f.metadata().ok()?.len();
        let mut f = f;
        if len > TAIL_READ {
            use std::io::{Seek, SeekFrom};
            f.seek(SeekFrom::Start(len - TAIL_READ)).ok()?;
        }
        f.take(TAIL_READ).read_to_end(&mut buf).ok()?;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn codex_window(v: &Value, base_secs: i64) -> Option<Window> {
    let used = num(&v["used_percent"])?;
    let resets_at = epoch(&v["resets_at"]).or_else(|| {
        // 옛 형식: 남은 초
        num(&v["resets_in_seconds"]).map(|s| base_secs + s as i64)
    });
    Some(Window {
        used_percent: used.clamp(0.0, 1000.0),
        resets_at,
        window_minutes: v["window_minutes"].as_u64(),
    })
}

/// rollout 기록 한 개의 내용에서 마지막 rate_limits를 찾는다. `file_secs`는 파일 수정 시각.
pub fn parse_codex_rollout(text: &str, file_secs: i64) -> Option<AgentUsage> {
    for line in text.lines().rev() {
        if !line.contains("rate_limits") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let rl = &v["payload"]["rate_limits"];
        if !rl.is_object() {
            continue;
        }
        let primary = codex_window(&rl["primary"], file_secs);
        let secondary = codex_window(&rl["secondary"], file_secs);
        if primary.is_none() && secondary.is_none() {
            continue;
        }
        return Some(AgentUsage {
            agent: "codex".into(),
            primary,
            secondary,
            updated_ms: file_secs * 1000,
            source: "Codex session log".into(),
        });
    }
    None
}

/// 가장 최근 Codex 세션 기록에서 사용량을 읽는다.
pub fn read_codex() -> Option<AgentUsage> {
    read_codex_in(&codex_sessions_dir())
}

pub fn read_codex_in(dir: &Path) -> Option<AgentUsage> {
    for (mtime, p) in rollout_files(dir).into_iter().take(8) {
        let secs = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Some(text) = read_rollout(&p) {
            if let Some(u) = parse_codex_rollout(&text, secs) {
                return Some(u);
            }
        }
    }
    None
}

/// 사람이 읽는 한 줄 요약(상태 줄·CLI용): `5h 23% · 7d 41%`.
pub fn short_line(u: &AgentUsage) -> String {
    let mut parts = Vec::new();
    if let Some(w) = &u.primary {
        parts.push(format!("5h {:.0}%", w.used_percent));
    }
    if let Some(w) = &u.secondary {
        parts.push(format!("7d {:.0}%", w.used_percent));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_statusline_fields() {
        let v = json!({"model": {"display_name": "Opus"}, "rate_limits": {
            "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
            "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}}});
        let u = parse_claude_statusline(&v).unwrap();
        assert_eq!(u.primary.as_ref().unwrap().used_percent, 23.5);
        assert_eq!(u.primary.as_ref().unwrap().resets_at, Some(1738425600));
        assert_eq!(u.secondary.as_ref().unwrap().window_minutes, Some(10080));
        assert_eq!(short_line(&u), "5h 24% · 7d 41%");
    }

    #[test]
    fn claude_without_rate_limits_is_none() {
        assert!(parse_claude_statusline(&json!({"model": {}})).is_none());
        // 한 창만 있어도 된다.
        let v = json!({"rate_limits": {"seven_day": {"used_percentage": 5}}});
        let u = parse_claude_statusline(&v).unwrap();
        assert!(u.primary.is_none());
        assert_eq!(u.secondary.unwrap().resets_at, None);
    }

    #[test]
    fn codex_rollout_takes_last_sample() {
        let text = [
            r#"{"type":"session_meta","payload":{}}"#,
            r#"{"timestamp":"x","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":10.0,"window_minutes":300,"resets_at":1700000000},"secondary":{"used_percent":1.0,"window_minutes":10080,"resets_at":1700600000}}}}"#,
            r#"{"timestamp":"y","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1700000100},"secondary":null}}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"rate_limits are fine"}}"#,
        ]
        .join("\n");
        let u = parse_codex_rollout(&text, 1_699_990_000).unwrap();
        assert_eq!(u.primary.as_ref().unwrap().used_percent, 12.5);
        assert_eq!(u.primary.as_ref().unwrap().resets_at, Some(1700000100));
        assert!(u.secondary.is_none());
    }

    #[test]
    fn codex_old_format_resets_in_seconds() {
        let text = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":5,"window_minutes":300,"resets_in_seconds":600}}}}"#;
        let u = parse_codex_rollout(text, 1000).unwrap();
        assert_eq!(u.primary.unwrap().resets_at, Some(1600));
    }

    #[test]
    fn codex_reads_newest_file_and_zst() {
        let d = std::env::temp_dir().join(format!("kmttt-usage-{}", std::process::id()));
        let day = d.join("2026").join("11").join("26");
        std::fs::create_dir_all(&day).unwrap();
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":33,"window_minutes":300,"resets_at":1800000000}}}}"#;
        std::fs::write(day.join("rollout-a.jsonl"), line).unwrap();
        let u = read_codex_in(&d).unwrap();
        assert_eq!(u.primary.unwrap().used_percent, 33.0);
        // 압축된 기록(zstd 표준 프레임, 압축 안 한 블록 하나)
        let mut zst = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, line.len() as u8];
        let n = line.len() as u32;
        let hdr = (n << 3) | 1; // last block, raw
        zst.extend_from_slice(&hdr.to_le_bytes()[..3]);
        zst.extend_from_slice(line.as_bytes());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let other = line.replace("33", "44");
        let mut zst2 = zst.clone();
        let start = zst2.len() - line.len();
        zst2.truncate(start);
        zst2.extend_from_slice(other.as_bytes());
        std::fs::write(day.join("rollout-b.jsonl.zst"), &zst2).unwrap();
        let u = read_codex_in(&d).unwrap();
        assert_eq!(u.primary.unwrap().used_percent, 44.0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
