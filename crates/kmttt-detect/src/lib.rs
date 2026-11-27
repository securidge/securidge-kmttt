//! 에이전트 식별과 상태 감지.
//!
//! 1. pane의 전면 프로세스와 그 조상 체인을 보고 어떤 에이전트인지 고른다(`identify`).
//! 2. 화면 아래쪽 텍스트를 스크린 매니페스트 규칙에 맞춰 상태를 정한다(`classify`).
//! 3. 훅 신호가 있으면 화면 추정보다 우선한다(서버가 처리).

use kmttt_proto::AgentState;
use regex::Regex;
use serde::Deserialize;
use std::path::Path;

/// 내장 매니페스트(바이너리에 들어감).
pub const BUILTIN: &str = include_str!("../manifests/builtin.toml");

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    agent: Vec<AgentManifestRaw>,
}

#[derive(Debug, Deserialize)]
struct AgentManifestRaw {
    kind: String,
    display: Option<String>,
    #[serde(default)]
    process: Vec<String>,
    #[serde(default)]
    cmdline: Vec<String>,
    launch: Option<String>,
    hook: Option<String>,
    #[serde(default)]
    state: Vec<StateRuleRaw>,
}

#[derive(Debug, Deserialize)]
struct StateRuleRaw {
    state: String,
    #[serde(default = "default_lines")]
    lines: usize,
    any: Vec<String>,
}

fn default_lines() -> usize {
    10
}

#[derive(Debug)]
pub struct StateRule {
    pub state: AgentState,
    pub lines: usize,
    pub patterns: Vec<Regex>,
}

#[derive(Debug)]
pub struct AgentManifest {
    pub kind: String,
    pub display: String,
    pub process: Vec<String>,
    pub cmdline: Vec<String>,
    pub launch: String,
    pub hook: Option<String>,
    pub rules: Vec<StateRule>,
    /// 어디서 읽었나(builtin 또는 파일 경로)
    pub source: String,
}

/// 감지에 쓰는 프로세스 정보(kmttt-pty의 ProcEntry에서 필요한 것만).
#[derive(Debug, Clone)]
pub struct ProcView {
    pub name: String,
    pub cmdline: String,
}

/// 분류 결과와 근거.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub state: AgentState,
    /// "rule blocked#0 matched 'Do you want to proceed'" 같은 한 줄
    pub evidence: String,
    pub rule: Option<String>,
}

pub struct Detector {
    manifests: Vec<AgentManifest>,
}

impl Detector {
    /// 내장 매니페스트만.
    pub fn builtin() -> Self {
        let mut d = Self {
            manifests: Vec::new(),
        };
        d.load_str(BUILTIN, "builtin")
            .expect("builtin manifest is valid");
        d
    }

    /// 내장 + 사용자 폴더(`manifests/*.toml`). 같은 kind는 사용자 것이 이긴다.
    pub fn with_user_dir(dir: &Path) -> (Self, Vec<String>) {
        let mut d = Self::builtin();
        let mut errors = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut files: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            for f in files {
                match std::fs::read_to_string(&f) {
                    Ok(text) => {
                        if let Err(e) = d.load_str(&text, &f.to_string_lossy()) {
                            errors.push(format!("{}: {e}", f.display()));
                        }
                    }
                    Err(e) => errors.push(format!("{}: {e}", f.display())),
                }
            }
        }
        (d, errors)
    }

    pub fn load_str(&mut self, text: &str, source: &str) -> Result<(), String> {
        let file: ManifestFile = toml::from_str(text).map_err(|e| e.to_string())?;
        for raw in file.agent {
            let mut rules = Vec::new();
            for (i, r) in raw.state.iter().enumerate() {
                let state = AgentState::parse(&r.state)
                    .ok_or_else(|| format!("{}: unknown state {}", raw.kind, r.state))?;
                let patterns = r
                    .any
                    .iter()
                    .map(|p| {
                        Regex::new(&format!("(?m){p}"))
                            .map_err(|e| format!("{} rule {i}: {e}", raw.kind))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                rules.push(StateRule {
                    state,
                    lines: r.lines.max(1),
                    patterns,
                });
            }
            let m = AgentManifest {
                display: raw.display.unwrap_or_else(|| raw.kind.clone()),
                process: raw.process.iter().map(|s| s.to_ascii_lowercase()).collect(),
                cmdline: raw.cmdline,
                launch: raw.launch.unwrap_or_else(|| raw.kind.clone()),
                hook: raw.hook,
                rules,
                source: source.to_string(),
                kind: raw.kind,
            };
            self.manifests.retain(|x| x.kind != m.kind);
            self.manifests.push(m);
        }
        Ok(())
    }

    pub fn manifests(&self) -> &[AgentManifest] {
        &self.manifests
    }

    pub fn manifest(&self, kind: &str) -> Option<&AgentManifest> {
        self.manifests.iter().find(|m| m.kind == kind)
    }

    /// 전면 프로세스부터 조상 순서로 받은 체인에서 에이전트 종류를 고른다.
    pub fn identify(&self, chain: &[ProcView]) -> Option<(&AgentManifest, String)> {
        for p in chain {
            if p.name == "kmttt" {
                continue;
            }
            // Windows 경로는 역슬래시라 매니페스트의 `@scope/pkg` 패턴과 맞추려면 바꿔 비교한다.
            let cmdline = p.cmdline.replace('\\', "/");
            for m in &self.manifests {
                if m.process.iter().any(|n| n == &p.name) {
                    return Some((m, format!("process name '{}'", p.name)));
                }
                if let Some(c) = m.cmdline.iter().find(|c| cmdline.contains(c.as_str())) {
                    return Some((m, format!("command line contains '{c}'")));
                }
            }
        }
        None
    }

    /// 화면 아래쪽 줄들과 최근 출력 여부로 상태를 정한다.
    pub fn classify(&self, kind: &str, tail: &[String], recent_output: bool) -> Classification {
        let Some(m) = self.manifest(kind) else {
            return Classification {
                state: AgentState::Unknown,
                evidence: format!("no manifest for {kind}"),
                rule: None,
            };
        };
        if tail.iter().all(|l| l.trim().is_empty()) {
            return Classification {
                state: AgentState::Unknown,
                evidence: "screen is empty".into(),
                rule: None,
            };
        }
        for (i, rule) in m.rules.iter().enumerate() {
            let start = tail.len().saturating_sub(rule.lines);
            let window = tail[start..].join("\n");
            for p in &rule.patterns {
                if let Some(mt) = p.find(&window) {
                    let snippet: String = mt.as_str().chars().take(60).collect();
                    return Classification {
                        state: rule.state,
                        evidence: format!(
                            "{} rule {}#{i} matched '{}'",
                            m.source,
                            rule.state.as_str(),
                            snippet.replace('\n', " ")
                        ),
                        rule: Some(format!("{}#{i}", rule.state.as_str())),
                    };
                }
            }
        }
        if recent_output {
            Classification {
                state: AgentState::Working,
                evidence: "no rule matched; output in the last 1.5 s".into(),
                rule: None,
            }
        } else {
            Classification {
                state: AgentState::Idle,
                evidence: "no rule matched; screen is quiet".into(),
                rule: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(name: &str, cmd: &str) -> ProcView {
        ProcView {
            name: name.into(),
            cmdline: cmd.into(),
        }
    }

    #[test]
    fn builtin_loads() {
        let d = Detector::builtin();
        assert!(d.manifest("claude").is_some());
        assert!(d.manifest("codex").is_some());
        assert!(d.manifests().len() >= 6);
    }

    #[test]
    fn identify_by_name_and_cmdline() {
        let d = Detector::builtin();
        let (m, why) = d
            .identify(&[pv("claude", "claude"), pv("bash", "bash -l")])
            .unwrap();
        assert_eq!(m.kind, "claude");
        assert!(why.contains("process name"));
        let (m, _) = d
            .identify(&[pv(
                "node",
                "node /usr/lib/node_modules/@openai/codex/bin/codex.js",
            )])
            .unwrap();
        assert_eq!(m.kind, "codex");
        assert!(d
            .identify(&[pv("vim", "vim x"), pv("bash", "bash")])
            .is_none());
        // kmttt 훅 프로세스는 건너뛴다.
        assert!(d
            .identify(&[pv("kmttt", "kmttt hook claude pre")])
            .is_none());
    }

    #[test]
    fn user_manifest_overrides() {
        let mut d = Detector::builtin();
        d.load_str(
            r#"
            [[agent]]
            kind = "claude"
            process = ["myclaude"]
            [[agent.state]]
            state = "blocked"
            any = ["WAIT"]
            "#,
            "user",
        )
        .unwrap();
        assert_eq!(
            d.manifests().iter().filter(|m| m.kind == "claude").count(),
            1
        );
        assert_eq!(d.manifest("claude").unwrap().process, vec!["myclaude"]);
        let c = d.classify("claude", &["WAIT".into()], false);
        assert_eq!(c.state, AgentState::Blocked);
        assert!(c.evidence.contains("user"));
    }

    #[test]
    fn bad_manifest_is_reported() {
        let mut d = Detector::builtin();
        assert!(d
            .load_str(
                "[[agent]]\nkind='x'\n[[agent.state]]\nstate='flying'\nany=['a']",
                "t"
            )
            .is_err());
        assert!(d
            .load_str(
                "[[agent]]\nkind='x'\n[[agent.state]]\nstate='idle'\nany=['(']",
                "t"
            )
            .is_err());
    }

    #[test]
    fn fallback_by_activity() {
        let d = Detector::builtin();
        let tail = vec!["some output line".to_string()];
        assert_eq!(d.classify("claude", &tail, true).state, AgentState::Working);
        assert_eq!(d.classify("claude", &tail, false).state, AgentState::Idle);
        assert_eq!(d.classify("claude", &[], false).state, AgentState::Unknown);
        assert_eq!(d.classify("nope", &tail, false).state, AgentState::Unknown);
    }

    #[test]
    fn identifies_node_agents_on_windows_paths() {
        let d = Detector::builtin();
        let chain = vec![ProcView {
            name: "node".into(),
            cmdline: r"C:\Program Files\nodejs\node.exe C:\Users\km\AppData\Roaming\npm\node_modules\@google\gemini-cli\dist\index.js".into(),
        }];
        let (m, why) = d.identify(&chain).expect("gemini on a Windows path");
        assert_eq!(m.kind, "gemini");
        assert!(why.contains("@google/gemini-cli"), "{why}");
    }
}
