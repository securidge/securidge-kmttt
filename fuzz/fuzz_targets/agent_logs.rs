//! 에이전트가 쓰는 기록 읽기(`kmttt-core::usage`·`tokens`). 내용 일부는 에이전트·작업 내용이 정한다.
//!
//! 확인하는 것:
//! - Claude 상태 줄 입력, Claude 대화 기록 줄, Codex 세션 기록 줄 어떤 것에도 패닉하지 않는다.
//! - 토큰 합계는 응답별 값의 합과 같고, 넘치지 않는다(같은 메시지를 여러 번 보내도 한 번만 센다).
//! - 시각 읽기는 넘치지 않는다.
#![no_main]

use kmttt_core::{tokens, usage};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = tokens::parse_iso_ms(&text);
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = usage::parse_claude_statusline(&v);
        let _ = tokens::claude_link(&v, Some("s"), Some("p"));
        let _ = tokens::claude_tokens(&v);
        let _ = tokens::codex_tokens(&v);
    }
    let _ = usage::parse_codex_rollout(&text, 1_700_000_000);
    for kind in ["claude", "codex"] {
        let mut t = tokens::Tracker::new(kind);
        for line in text.lines() {
            t.feed(line);
            t.feed(line); // 같은 줄을 두 번 보내도 Claude는 한 번만 센다
        }
        let sum = t.samples_since(i64::MIN).iter().fold(0u64, |a, s| a.saturating_add(s.1));
        assert_eq!(sum, t.total());
    }
});
