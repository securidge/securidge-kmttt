//! 창(pane)별 토큰 사용량 읽기.
//!
//! - Claude Code: 대화 기록(transcript JSONL)의 assistant 메시지마다 들어 있는 `message.usage`.
//!   스트리밍 중에는 같은 메시지 ID가 여러 줄로 나오므로 ID마다 마지막 값만 센다.
//!   어느 창의 기록인지는 `kmttt statusline`이 창 환경 변수(KMTTT_SESSION·KMTTT_PANE)와
//!   상태 줄 입력의 `transcript_path`를 [`pane_map_dir`]에 적어 둔 것으로 안다.
//! - Codex CLI: 세션 기록(rollout JSONL)의 `token_count` 이벤트에 들어 있는 누적값
//!   `info.total_token_usage`. 연속한 두 값의 차이를 그 응답의 사용량으로 본다.
//!   Codex는 창 정보를 남기지 않으므로 작업 폴더(cwd)가 같은 최근 기록을 창에 짝짓는다.
//!
//! 셈 기준(두 에이전트 공통): 새로 처리한 입력 + 출력 토큰. 캐시에서 다시 읽은 입력은 빼고,
//! 캐시에 새로 쓴 입력은 넣는다. 값은 응답이 끝날 때마다 늘어나므로 실시간이 아니라 응답 단위다.

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// 처음 읽을 때 파일 끝에서부터 읽는 최대 크기. 아주 긴 대화도 메모리를 많이 쓰지 않게 한다.
const FIRST_READ: u64 = 32 * 1024 * 1024;
/// 한 번에 새로 읽는 최대 크기.
const STEP_READ: u64 = 8 * 1024 * 1024;

/// 상태 줄이 적어 두는 창 ↔ 대화 기록 짝.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLink {
    pub session: String,
    pub pane: String,
    pub kind: String,
    pub transcript: String,
    pub updated_ms: i64,
}

/// 창 ↔ 기록 짝을 저장하는 폴더.
pub fn pane_map_dir() -> PathBuf {
    paths::data_dir().join("usage").join("panes")
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 상태 줄 입력에서 창 짝을 만든다. KMttt 창 밖에서 돈 Claude Code면 None.
pub fn claude_link(input: &Value, session: Option<&str>, pane: Option<&str>) -> Option<PaneLink> {
    let session = session.filter(|s| !s.is_empty())?;
    let pane = pane.filter(|s| !s.is_empty())?;
    let transcript = input["transcript_path"]
        .as_str()
        .filter(|s| !s.is_empty())?;
    Some(PaneLink {
        session: session.to_string(),
        pane: pane.to_string(),
        kind: "claude".into(),
        transcript: transcript.to_string(),
        updated_ms: now_ms(),
    })
}

/// 창 짝을 파일에 저장한다(경로와 이름만 저장, 대화 내용은 저장하지 않음).
pub fn record_link(link: &PaneLink) -> std::io::Result<()> {
    let dir = pane_map_dir();
    std::fs::create_dir_all(&dir)?;
    let f = dir.join(format!(
        "{}__{}.json",
        safe(&link.session),
        safe(&link.pane)
    ));
    let tmp = f.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(link)?)?;
    std::fs::rename(&tmp, &f)
}

/// 이 세션의 창 짝들.
pub fn read_links(session: &str) -> Vec<PaneLink> {
    let prefix = format!("{}__", safe(session));
    let Ok(rd) = std::fs::read_dir(pane_map_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        if let Ok(b) = std::fs::read(e.path()) {
            if b.len() < 16 * 1024 {
                if let Ok(l) = serde_json::from_slice::<PaneLink>(&b) {
                    out.push(l);
                }
            }
        }
    }
    out
}

/// `2026-11-27T11:07:03.252Z` 같은 UTC 시각을 유닉스 밀리초로. 다른 시간대 표기도 받는다.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &s[st..i];
        let take: String = frac.chars().chain("000".chars()).take(3).collect();
        ms = take.parse().ok()?;
    }
    let off_min = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(b'+') | Some(b'-') => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            let oh = n(i + 1..i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') {
                n(i + 4..i + 6)?
            } else {
                n(i + 3..i + 5).unwrap_or(0)
            };
            sign * (oh * 60 + om)
        }
        _ => return None,
    };
    // 날짜 → 일 수(Howard Hinnant의 days_from_civil)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 86400 + h * 3600 + mi * 60 + se - off_min * 60) * 1000) + ms)
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Claude 응답 하나의 사용량: 입력 + 캐시에 새로 쓴 입력 + 출력(캐시 읽기는 뺌).
pub fn claude_tokens(usage: &Value) -> u64 {
    u(&usage["input_tokens"])
        + u(&usage["cache_creation_input_tokens"])
        + u(&usage["output_tokens"])
}

/// Codex 누적값: 입력 - 캐시 입력 + 출력.
pub fn codex_tokens(total: &Value) -> u64 {
    let input = u(&total["input_tokens"]);
    let cached = u(&total["cached_input_tokens"]).min(input);
    (input - cached).saturating_add(u(&total["output_tokens"]))
}

/// 응답 하나: (시각, 토큰).
pub type Sample = (i64, u64);

/// 기록 파일 하나를 이어서 읽는 상태. 이미 읽은 부분은 다시 읽지 않는다.
#[derive(Debug, Default)]
pub struct Tracker {
    pub kind: String,
    offset: u64,
    carry: String,
    /// Claude: 메시지 ID → (처음 본 시각, 토큰). 같은 ID가 다시 오면 토큰만 고친다.
    msgs: HashMap<String, (i64, u64)>,
    /// Codex: 응답마다의 증가분
    events: Vec<Sample>,
    codex_prev: u64,
    /// 파일 크기가 줄었으면(새로 쓴 파일) 처음부터 다시 읽는다.
    len: u64,
}

impl Tracker {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    /// 파일에 새로 붙은 줄을 읽는다.
    pub fn refresh(&mut self, path: &Path) {
        let Ok(mut f) = std::fs::File::open(path) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.len {
            *self = Tracker::new(&self.kind.clone());
        }
        self.len = len;
        if self.offset == 0 && len > FIRST_READ {
            self.offset = len - FIRST_READ;
            self.carry.clear();
            // 줄 중간부터 시작하므로 첫 줄은 버린다.
            self.carry.push('\u{0}');
        }
        if len <= self.offset {
            return;
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        let _ = f.take(STEP_READ).read_to_end(&mut buf);
        self.offset += buf.len() as u64;
        let text = String::from_utf8_lossy(&buf);
        let mut all = std::mem::take(&mut self.carry);
        all.push_str(&text);
        let mut lines: Vec<&str> = all.split('\n').collect();
        let rest = lines.pop().unwrap_or("").to_string();
        let skip_first = all.starts_with('\u{0}');
        for (i, line) in lines.iter().enumerate() {
            if i == 0 && skip_first {
                continue;
            }
            self.feed(line);
        }
        self.carry = rest;
    }

    /// 줄 하나를 반영한다.
    pub fn feed(&mut self, line: &str) {
        if self.kind == "codex" {
            if !line.contains("token_count") {
                return;
            }
        } else if !line.contains("\"usage\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return;
        };
        let ts = v["timestamp"].as_str().and_then(parse_iso_ms).unwrap_or(0);
        if self.kind == "codex" {
            let total = &v["payload"]["info"]["total_token_usage"];
            if !total.is_object() {
                return;
            }
            let cum = codex_tokens(total);
            if cum > self.codex_prev {
                self.events.push((ts, cum - self.codex_prev));
            }
            if cum != self.codex_prev {
                self.codex_prev = cum;
            }
        } else {
            let msg = &v["message"];
            let usage = &msg["usage"];
            if !usage.is_object() {
                return;
            }
            let id = msg["id"]
                .as_str()
                .or_else(|| v["uuid"].as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return;
            }
            let tok = claude_tokens(usage);
            self.msgs
                .entry(id)
                .and_modify(|e| e.1 = tok)
                .or_insert((ts, tok));
        }
    }

    /// 지금까지 읽은 전체 합계.
    pub fn total(&self) -> u64 {
        let it: Box<dyn Iterator<Item = u64>> = if self.kind == "codex" {
            Box::new(self.events.iter().map(|e| e.1))
        } else {
            Box::new(self.msgs.values().map(|e| e.1))
        };
        it.fold(0u64, |a, b| a.saturating_add(b))
    }

    /// `since_ms` 이후 응답들(시각 순).
    pub fn samples_since(&self, since_ms: i64) -> Vec<Sample> {
        let mut out: Vec<Sample> = if self.kind == "codex" {
            self.events
                .iter()
                .filter(|e| e.0 >= since_ms)
                .copied()
                .collect()
        } else {
            self.msgs
                .values()
                .filter(|e| e.0 >= since_ms)
                .copied()
                .collect()
        };
        out.sort_by_key(|e| e.0);
        out
    }
}

/// Codex 기록 파일의 첫 줄(session_meta)에서 작업 폴더를 읽는다.
pub fn codex_meta_cwd(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(64 * 1024).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().take(5) {
        if !line.contains("session_meta") {
            continue;
        }
        let v: Value = serde_json::from_str(line).ok()?;
        return v["payload"]["cwd"].as_str().map(|s| s.to_string());
    }
    None
}

/// 경로 비교용: 구분자·끝 슬래시·(Windows) 대소문자를 맞춘다.
pub fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) || s.get(1..2) == Some(":") {
        s.to_lowercase()
    } else {
        s
    }
}

/// 최근 Codex 기록(압축되지 않은 것, 최근 `within_ms` 안에 바뀐 것)을 새것부터: (수정 시각, 경로, cwd).
pub fn recent_codex_files(dir: &Path, within_ms: i64) -> Vec<(i64, PathBuf, String)> {
    let now = now_ms();
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
                if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                    continue;
                }
                let m = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if now - m > within_ms {
                    continue;
                }
                if let Some(cwd) = codex_meta_cwd(&p) {
                    out.push((m, p, cwd));
                }
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    out
}

/// Codex 창과 기록을 짝짓는다: 작업 폴더가 같은 기록 중 가장 최근 것부터, 한 기록은 한 창에만.
/// 같은 폴더에서 Codex 창을 여러 개 쓰면 짝이 바뀔 수 있다(화면에 "추정"으로 표시).
pub fn codex_assign(
    panes: &[(String, String)],
    files: &[(i64, PathBuf, String)],
) -> HashMap<String, PathBuf> {
    let mut used = std::collections::HashSet::new();
    let mut out = HashMap::new();
    for (pane, cwd) in panes {
        let c = norm_path(cwd);
        if let Some((_, p, _)) = files
            .iter()
            .find(|(_, p, fc)| norm_path(fc) == c && !used.contains(p))
        {
            used.insert(p.clone());
            out.insert(pane.clone(), p.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_times() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2026-11-27T11:07:03.252Z"),
            Some(1795777623252)
        );
        assert_eq!(
            parse_iso_ms("2026-11-27T20:07:03.252+09:00"),
            Some(1795777623252)
        );
        assert_eq!(
            parse_iso_ms("2026-11-27T11:07:03.252123Z"),
            Some(1795777623252)
        );
        assert_eq!(parse_iso_ms("not a time"), None);
    }

    #[test]
    fn claude_counts_each_message_once() {
        let mut t = Tracker::new("claude");
        let line = |id: &str, out: u64| {
            json!({"type":"assistant","timestamp":"2026-11-27T11:00:00Z","message":{"id":id,"usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":5000,"output_tokens":out}}}).to_string()
        };
        t.feed(&line("m1", 5));
        t.feed(&line("m1", 50)); // 같은 메시지의 다음 조각: 마지막 값으로
        t.feed(&line("m2", 1));
        t.feed(r#"{"type":"user","message":{"content":"hi"}}"#);
        assert_eq!(t.total(), (10 + 100 + 50) + (10 + 100 + 1));
        assert_eq!(t.samples_since(0).len(), 2);
    }

    #[test]
    fn codex_uses_increments_of_the_running_total() {
        let mut t = Tracker::new("codex");
        let ev = |inp: u64, cached: u64, out: u64, ts: &str| {
            json!({"timestamp":ts,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":inp,"cached_input_tokens":cached,"output_tokens":out}}}}).to_string()
        };
        t.feed(&ev(1000, 0, 100, "2026-11-27T11:00:00Z"));
        t.feed(&ev(1000, 0, 100, "2026-11-27T11:00:01Z")); // 같은 값 반복은 무시
        t.feed(&ev(3000, 1500, 300, "2026-11-27T11:01:00Z"));
        assert_eq!(t.total(), 1500 + 300);
        let s = t.samples_since(0);
        assert_eq!(s, vec![(1795777200000, 1100), (1795777260000, 700)]);
    }

    #[test]
    fn tracker_reads_only_new_lines() {
        let dir = std::env::temp_dir().join(format!("kmttt-tok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.jsonl");
        let line = |id: &str| {
            format!(
                "{}\n",
                json!({"timestamp":"2026-11-27T11:00:00Z","message":{"id":id,"usage":{"input_tokens":1,"output_tokens":1}}})
            )
        };
        std::fs::write(&f, line("a")).unwrap();
        let mut t = Tracker::new("claude");
        t.refresh(&f);
        assert_eq!(t.total(), 2);
        let mut s = line("a");
        s.push_str(&line("b"));
        let half = &line("c")[..10];
        s.push_str(half);
        std::fs::write(&f, &s).unwrap();
        t.refresh(&f);
        assert_eq!(t.total(), 4, "반쯤 쓴 줄은 아직 세지 않는다");
        s.push_str(&line("c")[10..]);
        std::fs::write(&f, &s).unwrap();
        t.refresh(&f);
        assert_eq!(t.total(), 6);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_pairs_by_folder_newest_first() {
        let files = vec![
            (3, PathBuf::from("r3"), "D:\\work\\api".to_string()),
            (2, PathBuf::from("r2"), "d:/work/api/".to_string()),
            (1, PathBuf::from("r1"), "/home/x/web".to_string()),
        ];
        let panes = vec![
            ("p1".to_string(), "D:/Work/API".to_string()),
            ("p2".to_string(), "D:\\work\\api".to_string()),
            ("p3".to_string(), "/elsewhere".to_string()),
        ];
        let m = codex_assign(&panes, &files);
        assert_eq!(m.get("p1"), Some(&PathBuf::from("r3")));
        assert_eq!(m.get("p2"), Some(&PathBuf::from("r2")));
        assert!(!m.contains_key("p3"));
    }

    #[test]
    fn link_needs_kmttt_pane() {
        let input = json!({"transcript_path":"/x/t.jsonl"});
        assert!(claude_link(&input, None, Some("p1")).is_none());
        assert!(claude_link(&input, Some("default"), Some("")).is_none());
        let l = claude_link(&input, Some("default"), Some("p1")).unwrap();
        assert_eq!(l.transcript, "/x/t.jsonl");
        assert_eq!(safe("../a b"), "___a_b");
    }
}
