//! 화면 샘플 회귀 테스트.
//!
//! 주의: `tests/fixtures`의 화면은 각 에이전트의 공개된 화면 형식을 본떠 손으로 만든 합성 샘플이다.
//! 실제 에이전트 화면을 녹화한 것이 아니므로, 실제 화면 녹화가 생기면 같은 형식으로 추가해야 한다.
//! 첫 줄 형식: `# kind=<종류> expect=<상태> recent=<true|false>`

use kmttt_detect::Detector;
use kmttt_proto::AgentState;
use std::path::Path;

struct Fixture {
    name: String,
    kind: String,
    expect: AgentState,
    recent: bool,
    lines: Vec<String>,
}

fn load() -> Vec<Fixture> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let mut lines = text.lines();
        let header = lines.next().unwrap();
        let get = |k: &str| {
            header
                .split_whitespace()
                .find_map(|t| t.strip_prefix(&format!("{k}=")))
                .unwrap_or_else(|| panic!("{} missing {k}", f.display()))
                .to_string()
        };
        out.push(Fixture {
            name: f.file_name().unwrap().to_string_lossy().into(),
            kind: get("kind"),
            expect: AgentState::parse(&get("expect")).unwrap(),
            recent: get("recent") == "true",
            lines: lines.map(|l| l.trim_end().to_string()).collect(),
        });
    }
    out
}

#[test]
fn fixtures_accuracy_at_least_95_percent() {
    let d = Detector::builtin();
    let fixtures = load();
    assert!(fixtures.len() >= 20, "need at least 20 fixtures");
    let mut ok = 0;
    let mut misses = Vec::new();
    for f in &fixtures {
        // 서버와 같은 방식: 내용이 있는 마지막 줄까지 자른 아래쪽 40줄
        let last = f
            .lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        let tail = &f.lines[last.saturating_sub(40)..last];
        let c = d.classify(&f.kind, tail, f.recent);
        if c.state == f.expect {
            ok += 1;
        } else {
            misses.push(format!(
                "{}: expected {:?}, got {:?} ({})",
                f.name, f.expect, c.state, c.evidence
            ));
        }
    }
    let accuracy = ok as f64 / fixtures.len() as f64;
    assert!(
        accuracy >= 0.95,
        "accuracy {accuracy:.2}\n{}",
        misses.join("\n")
    );
}

/// 실제 Windows 노트북에서 실제 에이전트로 찍은 화면(real-*)은 하나도 틀리면 안 된다.
#[test]
fn real_agent_screens_are_all_correct() {
    let d = Detector::builtin();
    let real: Vec<_> = load()
        .into_iter()
        .filter(|f| f.name.starts_with("real-"))
        .collect();
    assert!(real.len() >= 4, "real fixtures missing");
    for f in &real {
        let last = f
            .lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        let tail = &f.lines[last.saturating_sub(40)..last];
        let c = d.classify(&f.kind, tail, f.recent);
        assert_eq!(c.state, f.expect, "{}: {}", f.name, c.evidence);
    }
}
