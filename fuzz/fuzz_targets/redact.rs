//! 비밀값 가리기(`kmttt-redact`). 가릴 텍스트는 에이전트 출력이라 공격자가 만든다.
//!
//! 확인하는 것:
//! - 어떤 텍스트에도 패닉하지 않고, 탐지 위치가 원문 안의 올바른 글자 경계다.
//! - 한 번 배운 비밀 값은 어떤 문맥에 섞여 나와도 결과에 남지 않는다(유출 없음).
//! - 같은 입력을 다시 가려도 비밀이 되살아나지 않는다.
#![no_main]

use std::sync::OnceLock;

use kmttt_redact::Redactor;
use libfuzzer_sys::fuzz_target;

/// 규칙 컴파일이 비싸므로 한 번만 만든다(서버도 한 번 만들어 계속 쓴다).
fn redactor() -> &'static Redactor {
    static R: OnceLock<Redactor> = OnceLock::new();
    R.get_or_init(Redactor::default)
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // 첫 바이트로 비밀 길이를 정하고, 그 뒤를 비밀·텍스트로 나눈다.
    let n = 8 + (data[0] as usize % 40);
    let rest = &data[1..];
    let (secret_raw, text_raw) = rest.split_at(n.min(rest.len()));
    let text = String::from_utf8_lossy(text_raw).into_owned();

    let r = redactor();
    let (out, findings) = r.redact(&text);
    for f in &findings {
        assert!(f.start <= f.end && f.end <= text.len());
        assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
    }
    let _ = r.redact(&out);

    // 비밀은 영숫자만 쓴다. 그래야 가림 표시("[REDACTED:종류]")와 섞여 우연히 생기는 경우를 뺄 수 있다.
    let secret: String = secret_raw
        .iter()
        .map(|b| {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            A[*b as usize % A.len()] as char
        })
        .collect();
    if secret.len() < 8 {
        return;
    }
    r.learn(&secret);
    let mixed = format!("{text}{secret}{text}");
    let (out, _) = r.redact(&mixed);
    if labels_contain(&out, &secret) {
        return;
    }
    assert!(!out.contains(&secret), "learned secret leaked");
});

/// 결과에 나온 비밀 문자열이 원문이 아니라 가림 표시 글자에서 온 것인지.
fn labels_contain(out: &str, secret: &str) -> bool {
    let mut rest = out;
    while let Some(i) = rest.find("[REDACTED:") {
        let tail = &rest[i..];
        let end = tail.find(']').map(|e| e + 1).unwrap_or(tail.len());
        if tail[..end].contains(secret) {
            return true;
        }
        rest = &tail[end..];
    }
    false
}
