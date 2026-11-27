//! 외부 접속 프록시의 요청 머리 읽기(`kmttt-sandbox::egress`). 요청은 격리된 에이전트가 보낸다.
//!
//! 확인하는 것:
//! - 어떤 바이트에도 패닉하지 않는다.
//! - 받아들인 요청의 호스트는 허용된 글자만 쓰고, 포트는 0이 아니며, 요청 줄 길이가 머리 안이다.
//! - 허용 판정은 "목록의 도메인과 같거나 그 하위 도메인"일 때만 참이다(우회 이름이 없음).
#![no_main]

use kmttt_sandbox::egress::{host_allowed, parse_request, valid_host};
use libfuzzer_sys::fuzz_target;

const ALLOW: &[&str] = &["github.com", "*.crates.io", "pypi.org"];

fuzz_target!(|data: &[u8]| {
    let Ok(req) = parse_request(data) else { return };
    assert!(valid_host(&req.host), "accepted host with bad characters: {:?}", req.host);
    assert!(req.port != 0);
    assert!(req.head_len <= data.len());
    if let Some(p) = &req.origin_form {
        assert!(p.starts_with('/'));
    }
    let allow: Vec<String> = ALLOW.iter().map(|s| s.to_string()).collect();
    if host_allowed(&req.host, &allow) {
        let h = req.host.trim_end_matches('.').to_ascii_lowercase();
        let ok = ALLOW.iter().any(|d| {
            let d = d.trim_start_matches("*.");
            h == d || (h.ends_with(d) && h.as_bytes()[h.len() - d.len() - 1] == b'.')
        });
        assert!(ok, "allowed a host outside the list: {:?}", req.host);
    }
});
