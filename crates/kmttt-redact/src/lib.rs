//! F3 비밀정보 마스킹.
//!
//! 원문은 어디에도 남기지 않는다. 탐지 결과는 종류·위치·지문(SHA-256 앞 12자리)만 돌려준다.
//! ANSI 제어 문자가 토큰 중간에 끼어도 잡도록, 바이트 스트림은 먼저 `strip_ansi`로 걷어낸 뒤 검사한다.

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 탐지 한 건.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: String,
    /// 치환 전 텍스트 안의 바이트 위치
    pub start: usize,
    pub end: usize,
    /// 원문 SHA-256의 앞 12자리. 같은 비밀이 여러 곳에 나왔는지 알 수 있게 한다.
    pub fingerprint: String,
}

/// 사용자 설정(`config.toml`의 `[redact]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactConfig {
    /// 추가 규칙
    pub rules: Vec<RuleConfig>,
    /// 이 정규식에 맞는 값은 비밀로 보지 않음(예외)
    pub allow: Vec<String>,
    /// 고엔트로피 문자열 탐지를 끔
    pub disable_entropy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    pub name: String,
    pub pattern: String,
}

struct Rule {
    name: String,
    re: Regex,
}

/// 내장 규칙. 이름이 가려진 자리에 `[REDACTED:이름]`으로 남는다.
/// 이름이 `s`인 캡처 그룹이 있으면 그 부분만 가린다(키 이름은 남기고 값만).
const BUILTIN: &[(&str, &str)] = &[
    (
        "pem-private-key",
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----)?",
    ),
    ("aws-access-key", r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b"),
    (
        "aws-secret-key",
        r"(?i)aws_?secret_?access_?key\s*[=:]\s*['\x22]?(?P<s>[A-Za-z0-9/+=]{40})",
    ),
    ("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,255}\b"),
    ("github-pat", r"\bgithub_pat_[A-Za-z0-9_]{22,255}\b"),
    ("gitlab-token", r"\bglpat-[A-Za-z0-9_\-]{20,}\b"),
    ("anthropic-key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
    (
        "openai-key",
        r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
    ),
    ("slack-token", r"\bxox[abposr]-[A-Za-z0-9\-]{10,}"),
    (
        "slack-webhook",
        r"https://hooks\.slack\.com/services/[A-Za-z0-9/_\-]+",
    ),
    ("google-api-key", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
    (
        "stripe-key",
        r"\b(?:sk|rk)_(?:live|test)_[0-9a-zA-Z]{20,}\b",
    ),
    ("npm-token", r"\bnpm_[A-Za-z0-9]{36}\b"),
    ("huggingface-token", r"\bhf_[A-Za-z0-9]{30,}\b"),
    (
        "sendgrid-key",
        r"\bSG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}\b",
    ),
    (
        "jwt",
        r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
    ),
    (
        "azure-storage-key",
        r"(?i)AccountKey=(?P<s>[A-Za-z0-9+/=]{40,})",
    ),
    (
        "url-password",
        r"\b[a-zA-Z][a-zA-Z0-9+.\-]*://[^\s:/@]+:(?P<s>[^\s:/@]{3,})@",
    ),
    (
        "bearer-token",
        r"(?i)\bauthorization:\s*bearer\s+(?P<s>[A-Za-z0-9._~+/=\-]{16,})",
    ),
    (
        "env-secret",
        r"(?im)\b[A-Z0-9_]*(?:API_?KEY|TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE_KEY)[A-Z0-9_]*\s*[=:]\s*['\x22]?(?P<s>[^\s'\x22`;&|,)]{6,})",
    ),
];

pub struct Redactor {
    rules: Vec<Rule>,
    allow: Vec<Regex>,
    entropy: bool,
    /// 한 번 탐지된 비밀 값. 나중에 문맥 없이 값만 다시 나와도 가린다.
    /// 메모리에만 두고 디스크에는 절대 쓰지 않는다.
    learned: std::sync::Mutex<Learned>,
}

/// 기억한 비밀 값 목록.
///
/// 출력에서 배운 값은 개수 상한이 있어 오래된 것부터 지운다. 서버 환경 변수에서 배운 값은
/// 고정(pinned)해 지우지 않는다. 그러지 않으면 에이전트가 무작위 문자열을 대량으로 출력해
/// 목록을 채우는 것만으로 진짜 토큰을 밀어낼 수 있다.
#[derive(Default)]
struct Learned {
    /// (값, 들어온 순서, 고정 여부). 찾을 때 긴 값부터 보도록 길이 내림차순으로 둔다.
    items: Vec<(String, u64, bool)>,
    seq: u64,
}

impl Learned {
    fn insert(&mut self, v: &str, pinned: bool) {
        if let Some(it) = self.items.iter_mut().find(|x| x.0 == v) {
            it.2 |= pinned;
            return;
        }
        let unpinned = self.items.iter().filter(|x| !x.2).count();
        if !pinned && unpinned >= LEARN_MAX {
            // 출력에서 배운 값 중 가장 오래된 것을 지운다.
            if let Some(pos) = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, x)| !x.2)
                .min_by_key(|(_, x)| x.1)
                .map(|(i, _)| i)
            {
                self.items.remove(pos);
            }
        }
        self.seq += 1;
        self.items.push((v.to_string(), self.seq, pinned));
        self.items.sort_by_key(|x| std::cmp::Reverse(x.0.len()));
    }
}

/// 기억할 비밀 값의 최소 길이(짧은 값은 오탐이 많다)와 최대 개수.
const LEARN_MIN: usize = 8;
const LEARN_MAX: usize = 2000;

impl Default for Redactor {
    fn default() -> Self {
        Self::new(&RedactConfig::default()).expect("builtin rules compile")
    }
}

impl Redactor {
    pub fn new(cfg: &RedactConfig) -> Result<Self, regex::Error> {
        let mut rules = Vec::new();
        for (name, pat) in BUILTIN {
            rules.push(Rule {
                name: (*name).to_string(),
                re: Regex::new(pat)?,
            });
        }
        for r in &cfg.rules {
            rules.push(Rule {
                name: r.name.clone(),
                re: Regex::new(&r.pattern)?,
            });
        }
        let allow = cfg
            .allow
            .iter()
            .map(|a| Regex::new(a))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            rules,
            allow,
            entropy: !cfg.disable_entropy,
            learned: std::sync::Mutex::new(Learned::default()),
        })
    }

    /// 텍스트에서 비밀을 찾아 가린 결과와 탐지 목록을 돌려준다.
    pub fn redact(&self, text: &str) -> (String, Vec<Finding>) {
        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for rule in &self.rules {
            for caps in rule.re.captures_iter(text) {
                let m = caps
                    .name("s")
                    .unwrap_or_else(|| caps.get(0).expect("group 0"));
                if self.is_allowed(m.as_str()) {
                    continue;
                }
                spans.push((m.start(), m.end(), rule.name.clone()));
            }
        }
        if self.entropy {
            for (s, e) in high_entropy_spans(text) {
                if !self.is_allowed(&text[s..e]) {
                    spans.push((s, e, "high-entropy".into()));
                }
            }
        }
        // 이번에 새로 찾은 값은 기억하고, 전에 기억한 값은 문맥 없이 나와도 가린다.
        for (s, e, _) in spans.clone() {
            self.learn(&text[s..e]);
        }
        if let Ok(known) = self.learned.lock() {
            for (k, _, _) in known.items.iter() {
                let mut from = 0;
                while let Some(i) = text[from..].find(k.as_str()) {
                    let st = from + i;
                    spans.push((st, st + k.len(), "known-secret".into()));
                    from = st + k.len();
                }
            }
        }
        if spans.is_empty() {
            return (text.to_string(), Vec::new());
        }
        // 겹치는 구간은 먼저 시작한(같으면 더 긴) 것 하나로 합친다.
        spans.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut merged: Vec<(usize, usize, String)> = Vec::new();
        for s in spans {
            if let Some(last) = merged.last_mut() {
                if s.0 < last.1 {
                    last.1 = last.1.max(s.1);
                    continue;
                }
            }
            merged.push(s);
        }
        let mut out = String::with_capacity(text.len());
        let mut findings = Vec::with_capacity(merged.len());
        let mut cursor = 0;
        for (s, e, kind) in merged {
            out.push_str(&text[cursor..s]);
            out.push_str("[REDACTED:");
            out.push_str(&kind);
            out.push(']');
            findings.push(Finding {
                fingerprint: fingerprint(&text[s..e]),
                kind,
                start: s,
                end: e,
            });
            cursor = e;
        }
        out.push_str(&text[cursor..]);
        (out, findings)
    }

    /// 출력에서 찾은 비밀 값을 기억한다. 개수 상한을 넘으면 오래된 것부터 잊는다.
    pub fn learn(&self, value: &str) {
        self.learn_with(value, false);
    }

    /// 비밀 값을 고정해 기억한다(서버 환경 변수 등). 출력이 아무리 많아도 잊지 않는다.
    pub fn learn_pinned(&self, value: &str) {
        self.learn_with(value, true);
    }

    fn learn_with(&self, value: &str, pinned: bool) {
        let v = value.trim();
        if v.len() < LEARN_MIN || v.starts_with("[REDACTED:") || v.contains('\n') {
            return;
        }
        if let Ok(mut l) = self.learned.lock() {
            l.insert(v, pinned);
        }
    }

    /// 환경 변수 중 이름이 비밀처럼 보이는 것들의 값을 기억한다.
    pub fn learn_env<I: IntoIterator<Item = (String, String)>>(&self, vars: I) -> usize {
        let mut n = 0;
        for (k, v) in vars {
            let up = k.to_ascii_uppercase();
            if ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"]
                .iter()
                .any(|w| up.contains(w))
                && v.len() >= LEARN_MIN
            {
                self.learn_pinned(&v);
                n += 1;
            }
        }
        n
    }

    /// 설정을 다시 읽을 때 이전 Redactor가 기억한 값을 이어받는다.
    pub fn absorb(&self, other: &Redactor) {
        let vals: Vec<(String, bool)> = other
            .learned
            .lock()
            .map(|l| l.items.iter().map(|x| (x.0.clone(), x.2)).collect())
            .unwrap_or_default();
        for (v, pinned) in vals {
            self.learn_with(&v, pinned);
        }
    }

    pub fn learned_count(&self) -> usize {
        self.learned.lock().map(|l| l.items.len()).unwrap_or(0)
    }

    /// 가린 텍스트만 필요할 때.
    pub fn redact_str(&self, text: &str) -> String {
        self.redact(text).0
    }

    /// 탐지 구간만 필요할 때(화면 마스킹용).
    pub fn find(&self, text: &str) -> Vec<Finding> {
        self.redact(text).1
    }

    fn is_allowed(&self, value: &str) -> bool {
        self.allow.iter().any(|a| a.is_match(value)) || value.starts_with("[REDACTED:")
    }
}

/// 비밀 원문의 지문.
pub fn fingerprint(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    hex::encode(&digest[..6])
}

/// 길이 32 이상, 숫자와 글자가 섞이고 섀넌 엔트로피가 높은 토큰.
fn high_entropy_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let is_tok = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-');
    let mut i = 0;
    while i < bytes.len() {
        if !is_tok(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_tok(bytes[i]) {
            i += 1;
        }
        let tok = &text[start..i];
        if tok.len() >= 32 && looks_random(tok) {
            spans.push((start, i));
        }
    }
    spans
}

fn looks_random(tok: &str) -> bool {
    let has_digit = tok.bytes().any(|b| b.is_ascii_digit());
    let has_upper = tok.bytes().any(|b| b.is_ascii_uppercase());
    let has_lower = tok.bytes().any(|b| b.is_ascii_lowercase());
    // 16진수 해시(커밋 ID 등)는 흔하고 비밀이 아닌 경우가 많아 제외한다.
    let all_hex = tok.bytes().all(|b| b.is_ascii_hexdigit());
    // 경로처럼 보이는 것(슬래시가 여러 개)은 제외.
    let slashes = tok.bytes().filter(|&b| b == b'/').count();
    if all_hex || slashes > 2 || !(has_digit && has_upper && has_lower) {
        return false;
    }
    shannon(tok) > 4.3
}

fn shannon(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// ANSI 이스케이프(CSI, OSC, 단일 ESC)와 제어 문자를 걷어낸 텍스트.
/// `\r\n`은 줄바꿈으로, 홀로 있는 `\r`은 지운다.
pub fn strip_ansi(input: &[u8]) -> String {
    let text = String::from_utf8_lossy(input);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: 파라미터·중간 바이트 뒤 0x40..=0x7e 종결 바이트까지
                    for c2 in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c2) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('_') | Some('^') => {
                    chars.next();
                    // OSC/DCS: BEL 또는 ESC \ 까지
                    while let Some(c2) = chars.next() {
                        if c2 == '\x07' {
                            break;
                        }
                        if c2 == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\n' | '\t' => out.push(c),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트용 가짜 비밀 20종. 실제로 쓰이는 값이 아니다(형식만 맞춤).
    fn fakes() -> Vec<(&'static str, String)> {
        let r40 = "Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0Uv1Wx2Yz3a";
        vec![
            ("aws-access-key", "AKIAABCDEFGHIJKLMNOP".to_string()),
            ("aws-secret-key", format!("aws_secret_access_key = {r40}")),
            ("github-token", format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8")),
            ("github-pat", format!("github_pat_{}", "11ABCDEFG0123456789_abcdefghijKLMNOP")),
            ("gitlab-token", "glpat-AbCdEfGhIjKlMnOpQrSt".to_string()),
            ("anthropic-key", "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx".to_string()),
            ("openai-key", "sk-proj-AbCdEfGhIjKlMnOpQrStUvWx0123".to_string()),
            ("slack-token", "xoxb-1234567890-abcdefghij".to_string()),
            ("slack-webhook", "https://hooks.slack.com/services/T000/B000/XXXXXXXX".to_string()),
            ("google-api-key", format!("AIza{}", "SyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q")),
            ("stripe-key", "sk_live_AbCdEfGhIjKlMnOpQrStUvWx".to_string()),
            ("npm-token", format!("npm_{}", "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789")),
            ("huggingface-token", format!("hf_{}", "AbCdEfGhIjKlMnOpQrStUvWxYz01234567")),
            (
                "sendgrid-key",
                format!("SG.{}.{}", "AbCdEfGhIjKlMnOpQrStUv", "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfG"),
            ),
            ("jwt", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U".to_string()),
            ("azure-storage-key", format!("AccountKey={r40}{r40}")),
            ("url-password", "postgres://admin:hunter2pass@db.internal:5432/app".to_string()),
            ("bearer-token", "Authorization: Bearer abcdefghijklmnop0123456789".to_string()),
            ("env-secret", "export DATABASE_PASSWORD=correct-horse-battery".to_string()),
            ("pem-private-key", "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----".to_string()),
        ]
    }

    /// "키 = 값", "사용자:비밀번호@", "Bearer 값" 같은 형식에서 값 부분만 고른다.
    fn secret_core(s: &str) -> &str {
        if let Some(i) = s.find(":hunter2") {
            return &s[i + 1..i + 13];
        }
        let tail = s
            .rsplit(['=', ' ', '\n'])
            .find(|t| t.len() >= 6)
            .unwrap_or(s);
        &tail[tail.len().saturating_sub(8)..]
    }

    #[test]
    fn twenty_fake_secrets_are_all_redacted() {
        let r = Redactor::default();
        let all = fakes();
        assert_eq!(all.len(), 20);
        for (kind, secret) in all {
            let text = format!("before {secret} after");
            let (out, findings) = r.redact(&text);
            assert!(!findings.is_empty(), "{kind} not detected in {text}");
            assert!(out.contains("[REDACTED:"), "{kind}: {out}");
            assert!(out.starts_with("before "), "{kind}: {out}");
            // 비밀 값 부분(구분자 뒤 마지막 덩어리)이 결과에 남아 있으면 안 된다.
            let core = secret_core(&secret);
            assert!(!out.contains(core), "{kind} leaked {core}: {out}");
        }
    }

    #[test]
    fn learned_values_are_redacted_without_context() {
        let r = Redactor::default();
        let (_, f) = r.redact("export DB_PASSWORD=correct-horse-staple-9");
        assert_eq!(f.len(), 1);
        // 나중에 값만 다시 나온다(echo $DB_PASSWORD)
        let (out, f) = r.redact("correct-horse-staple-9\n");
        assert_eq!(out, "[REDACTED:known-secret]\n");
        assert_eq!(f[0].kind, "known-secret");
        // 짧은 값은 기억하지 않는다
        r.learn("abc");
        assert_eq!(r.redact("abc").0, "abc");
    }

    #[test]
    fn learn_from_env_names() {
        let r = Redactor::default();
        let n = r.learn_env(vec![
            ("MY_API_TOKEN".into(), "tok-12345678".into()),
            ("HOME".into(), "/home/someone".into()),
        ]);
        assert_eq!(n, 1);
        assert!(r
            .redact_str("value tok-12345678 here")
            .contains("known-secret"));
        assert_eq!(r.redact_str("/home/someone"), "/home/someone");
    }

    #[test]
    fn keeps_key_names_for_partial_rules() {
        let (out, _) = Redactor::default().redact("API_TOKEN=supersecretvalue123");
        assert!(out.starts_with("API_TOKEN="), "{out}");
        assert!(!out.contains("supersecretvalue123"));
    }

    #[test]
    fn ordinary_text_untouched() {
        let r = Redactor::default();
        for t in [
            "cargo build --release",
            "commit 3f2a9c1e5b7d4e8a9c217d4e5f6a8b90aa11bb22",
            "src/very/long/path/to/some/file_name_here.rs",
            "The quick brown fox jumps over the lazy dog",
        ] {
            let (out, f) = r.redact(t);
            assert_eq!(out, t);
            assert!(f.is_empty(), "{t}: {f:?}");
        }
    }

    #[test]
    fn allow_list_exempts() {
        let r = Redactor::new(&RedactConfig {
            allow: vec!["^AKIAEXAMPLE".into()],
            ..Default::default()
        })
        .unwrap();
        let (out, _) = r.redact("AKIAEXAMPLE000000000");
        assert_eq!(out, "AKIAEXAMPLE000000000");
    }

    #[test]
    fn same_secret_same_fingerprint() {
        let r = Redactor::default();
        let (_, a) = r.redact("one AKIAABCDEFGHIJKLMNOP two AKIAABCDEFGHIJKLMNOP");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].fingerprint, a[1].fingerprint);
    }

    #[test]
    fn ansi_inside_token_is_caught_after_strip() {
        let raw = b"key: \x1b[1mAKIAABCD\x1b[0mEFGHIJKLMNOP\r\n";
        let text = strip_ansi(raw);
        assert_eq!(text, "key: AKIAABCDEFGHIJKLMNOP\n");
        assert!(!Redactor::default().find(&text).is_empty());
    }

    #[test]
    fn strip_osc_title() {
        assert_eq!(strip_ansi(b"\x1b]0;title\x07ok"), "ok");
        assert_eq!(strip_ansi(b"\x1b]0;title\x1b\\ok"), "ok");
    }

    #[test]
    fn flooding_output_cannot_evict_env_secrets() {
        let r = Redactor::default();
        // 형식 규칙·엔트로피 검사에 걸리지 않는 비밀번호. 출력 문자열보다 길어서, 가장 긴 값부터
        // 지우던 이전 방식에서는 밀려나 그대로 노출됐다.
        let token = "correct-horse-battery-staple-orange";
        r.learn_env([("MY_API_TOKEN".to_string(), token.to_string())]);
        // 에이전트가 무작위 고엔트로피 문자열을 상한보다 많이 출력해 목록을 채운다.
        for i in 0..(LEARN_MAX + 500) {
            r.learn(&format!("aQ{i:06}xR9kLm2Vb7"));
        }
        let (out, _) = r.redact(&format!("value: {token}"));
        assert!(!out.contains(token), "{out}");
        // 출력에서 배운 값은 상한을 지킨다.
        assert!(r.learned_count() <= LEARN_MAX + 1);
    }

    #[test]
    fn output_secrets_forget_oldest_first() {
        let r = Redactor::default();
        r.learn("firstsecret1");
        for i in 0..LEARN_MAX {
            r.learn(&format!("filler{i:08}"));
        }
        let (out, _) = r.redact("firstsecret1");
        assert!(
            out.contains("firstsecret1"),
            "oldest output-learned value is forgotten"
        );
        let (out, _) = r.redact(&format!("filler{:08}", LEARN_MAX - 1));
        assert!(!out.contains(&format!("filler{:08}", LEARN_MAX - 1)));
    }
}
