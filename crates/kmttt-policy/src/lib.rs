//! F1 정책 엔진과 F5 도구 호출 정책.
//!
//! 판정 입력: 호출자(사람 / pane 안 에이전트 / 플러그인), 동작 이름, 대상 pane.
//! 판정 결과: allow / deny / ask(사람 승인 필요).

pub mod injection;
pub mod rate;
pub mod tools;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use injection::detect_injection;
pub use rate::RateLimiter;
pub use tools::{ToolRule, ToolVerdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
            Decision::Ask => "ask",
        }
    }
}

/// 호출한 쪽. 서버가 소켓 상대 PID로 판별한다(스스로 밝힌 값이 아님).
#[derive(Debug, Clone, PartialEq)]
pub enum Caller {
    /// pane 밖 프로세스(사용자 터미널, 웹 콘솔, SSH 브리지)
    User,
    /// pane 안에서 실행된 프로세스
    Pane {
        pane: String,
        workspace: String,
        agent_kind: Option<String>,
        agent_name: Option<String>,
    },
    /// 서버가 실행한 플러그인
    Plugin {
        name: String,
        capabilities: Vec<String>,
    },
}

impl Caller {
    pub fn label(&self) -> String {
        match self {
            Caller::User => "user".into(),
            Caller::Pane {
                pane,
                agent_kind,
                agent_name,
                ..
            } => match (agent_name, agent_kind) {
                (Some(n), _) => format!("{pane}({n})"),
                (None, Some(k)) => format!("{pane}({k})"),
                _ => pane.clone(),
            },
            Caller::Plugin { name, .. } => format!("plugin:{name}"),
        }
    }
    pub fn key(&self) -> String {
        match self {
            Caller::User => "user".into(),
            Caller::Pane { pane, .. } => pane.clone(),
            Caller::Plugin { name, .. } => format!("plugin:{name}"),
        }
    }
}

/// 동작 대상.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Target {
    pub pane: Option<String>,
    pub workspace: Option<String>,
    pub agent_kind: Option<String>,
    pub agent_name: Option<String>,
}

/// 동작 분류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// 목록·상태 조회
    Meta,
    /// 다른 pane의 내용 읽기
    Read,
    /// 다른 pane에 입력·명령 보내기, 닫기
    Control,
    /// pane·탭·워크스페이스 만들기
    Create,
    /// 사람만 할 수 있는 일(승인, 정책 변경, 서버 정지 …)
    Admin,
    /// 자기 pane의 훅 보고
    Hook,
}

pub fn classify_action(action: &str) -> ActionClass {
    match action {
        "pane.read" | "agent.read" | "pane.wait_output" | "events.subscribe" => ActionClass::Read,
        "pane.run" | "pane.send" | "agent.prompt" | "agent.start" | "pane.close"
        | "client.input" | "worktree.remove" | "workspace.close" | "tab.close" => {
            ActionClass::Control
        }
        "pane.split" | "tab.create" | "workspace.create" | "worktree.create" => ActionClass::Create,
        "hook.event" => ActionClass::Hook,
        "approval.decide"
        | "server.stop"
        | "server.reload_config"
        | "policy.reload"
        | "client.attach"
        | "client.viewport"
        | "pane.redraw"
        | "workspace.rename"
        | "tab.rename"
        | "layout.set_ratio" => ActionClass::Admin,
        _ => ActionClass::Meta,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Defaults {
    /// 에이전트가 다른 에이전트에게 prompt/start
    pub agent_to_agent_prompt: Decision,
    /// 에이전트가 다른 pane에 run/send/close
    pub agent_to_agent_run: Decision,
    /// 에이전트가 다른 pane 읽기: "masked"(마스킹 후 허용), "deny"
    pub agent_read: String,
    /// 에이전트가 pane·탭·워크스페이스 만들기
    pub agent_create: Decision,
    /// 승인 대기 제한 시간(초). 지나면 거부.
    pub ask_timeout_secs: u64,
    /// 호출자별 분당 최대 호출 수(Meta 제외)
    pub rate_per_min: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            agent_to_agent_prompt: Decision::Ask,
            agent_to_agent_run: Decision::Ask,
            agent_read: "masked".into(),
            agent_create: Decision::Allow,
            ask_timeout_secs: 120,
            rate_per_min: 120,
        }
    }
}

/// 호출자·대상 조건. 비어 있는 칸은 "아무거나".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Matcher {
    pub agent: Option<String>,
    pub agent_name: Option<String>,
    pub workspace: Option<String>,
    pub pane: Option<String>,
    pub plugin: Option<String>,
    pub user: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub from: Matcher,
    /// "agent.prompt", "pane.*", "*"
    pub action: String,
    #[serde(default)]
    pub to: Matcher,
    pub decision: Decision,
    /// "10/min" — 이 규칙에 맞는 호출의 분당 한도
    #[serde(default)]
    pub rate: Option<String>,
}

/// 격리 기본값(kmttt-sandbox가 해석).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SandboxPolicy {
    /// 새 pane 기본 프로필: none, restricted, container, wsl
    pub default_profile: String,
    /// worktree 워크스페이스의 pane 기본 프로필
    pub worktree_profile: String,
    /// 에이전트 종류별 프로필(에이전트를 `agent start`로 띄울 때)
    pub per_agent: BTreeMap<String, String>,
    /// 이그레스 프록시 허용 도메인(접미사 일치)
    pub egress_allow: Vec<String>,
    pub container_image: String,
    pub container_network: String,
    pub wsl_distro: String,
    /// restricted 프로필 메모리 상한(MB, Windows Job Object)
    pub memory_limit_mb: u64,
    pub process_limit: u32,
    /// restricted에서 TCP 연결을 이그레스 프록시로만 허용(Linux Landlock 네트워크 규칙, macOS sandbox-exec)
    pub egress_enforce: bool,
    /// 강제 차단 중에도 연결을 허용할 TCP 포트(예: 로컬 개발 서버 3000). 포트 번호만 본다(주소 무관).
    pub allow_tcp_ports: Vec<u16>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            default_profile: "none".into(),
            worktree_profile: "none".into(),
            per_agent: BTreeMap::new(),
            egress_allow: vec![
                "api.anthropic.com".into(),
                "api.openai.com".into(),
                "github.com".into(),
                "githubusercontent.com".into(),
                "registry.npmjs.org".into(),
                "crates.io".into(),
                "static.crates.io".into(),
                "pypi.org".into(),
                "files.pythonhosted.org".into(),
            ],
            container_image: "node:22-bookworm".into(),
            container_network: "bridge".into(),
            wsl_distro: String::new(),
            memory_limit_mb: 8192,
            process_limit: 512,
            egress_enforce: true,
            allow_tcp_ports: Vec::new(),
        }
    }
}

/// `policy.toml` 전체.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PolicyConfig {
    pub defaults: Defaults,
    pub rule: Vec<Rule>,
    pub tool_rule: Vec<ToolRule>,
    /// 내장 도구 규칙을 끔(권장하지 않음)
    pub disable_builtin_tool_rules: bool,
    pub sandbox: SandboxPolicy,
    pub identify: IdentifyPolicy,
}

/// 호출자 식별 보정.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct IdentifyPolicy {
    /// Windows: OS 서비스(서비스 관리자·작업 스케줄러·WMI) 아래에서 도는 호출자는 기본적으로
    /// 에이전트(unknown)로 본다. 여기 적은 프로세스 이름(.exe 없이, 대소문자 무시)이 계보에서
    /// 서비스보다 먼저 나오면 사람(자동화)으로 본다. 예: CI 러너 `Runner.Listener`.
    pub trusted_launchers: Vec<String>,
}

impl PolicyConfig {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: PolicyConfig = toml::from_str(text)?;
        for r in &cfg.rule {
            if let Some(rate) = &r.rate {
                rate::parse_rate(rate).ok_or_else(|| anyhow::anyhow!("bad rate '{rate}'"))?;
            }
        }
        for t in &cfg.tool_rule {
            regex::Regex::new(&t.pattern)?;
        }
        for p in [&cfg.sandbox.default_profile, &cfg.sandbox.worktree_profile] {
            anyhow::ensure!(
                matches!(p.as_str(), "none" | "restricted" | "container" | "wsl"),
                "unknown sandbox profile '{p}'"
            );
        }
        Ok(cfg)
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
}

/// 판정 결과와 근거.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub decision: Decision,
    pub rule_id: String,
    pub reason: String,
    /// 읽기 허용 시 마스킹을 꼭 해야 하는지
    pub masked: bool,
}

impl Verdict {
    fn new(decision: Decision, rule_id: &str, reason: impl Into<String>) -> Self {
        Self {
            decision,
            rule_id: rule_id.into(),
            reason: reason.into(),
            masked: true,
        }
    }
}

pub struct PolicyEngine {
    pub cfg: PolicyConfig,
    tool_rules: Vec<tools::CompiledToolRule>,
    rates: RateLimiter,
}

impl PolicyEngine {
    pub fn new(cfg: PolicyConfig) -> Self {
        let tool_rules = tools::compile(&cfg);
        Self {
            cfg,
            tool_rules,
            rates: RateLimiter::default(),
        }
    }

    /// 호출 하나를 판정한다. 호출 수 제한은 여기서 함께 센다.
    pub fn evaluate(&mut self, caller: &Caller, action: &str, target: &Target) -> Verdict {
        let class = classify_action(action);
        // 1. 사람은 모두 허용(그래도 기록은 남는다).
        if *caller == Caller::User {
            return Verdict::new(Decision::Allow, "builtin:user", "pane 밖 사용자");
        }
        // 2. 사람만 할 수 있는 일.
        if class == ActionClass::Admin {
            return Verdict::new(
                Decision::Deny,
                "builtin:admin",
                format!("{action} is for users only"),
            );
        }
        // 3. 플러그인은 선언한 권한 안에서만.
        if let Caller::Plugin { capabilities, name } = caller {
            let need = tools::capability_for(class);
            if let Some(need) = need {
                if !capabilities.iter().any(|c| c == need) {
                    return Verdict::new(
                        Decision::Deny,
                        "builtin:capability",
                        format!("plugin {name} lacks capability '{need}'"),
                    );
                }
            }
        }
        // 4. 호출 수 제한(조회 제외).
        if class != ActionClass::Meta && class != ActionClass::Hook {
            let key = caller.key();
            if !self
                .rates
                .check(&key, "*", self.cfg.defaults.rate_per_min, 60)
            {
                return Verdict::new(
                    Decision::Deny,
                    "builtin:rate",
                    format!("more than {}/min", self.cfg.defaults.rate_per_min),
                );
            }
        }
        // 5. 사용자 규칙(먼저 맞는 것).
        for (i, r) in self.cfg.rule.iter().enumerate() {
            if glob_match(&r.action, action)
                && match_caller(&r.from, caller)
                && match_target(&r.to, target)
            {
                let id = r.id.clone().unwrap_or_else(|| format!("rule#{i}"));
                if let Some(rate) = &r.rate {
                    let (n, secs) = rate::parse_rate(rate).unwrap_or((60, 60));
                    if !self.rates.check(&caller.key(), &id, n, secs) {
                        return Verdict::new(Decision::Deny, &id, format!("rate {rate} exceeded"));
                    }
                }
                return Verdict::new(r.decision, &id, "matched policy rule");
            }
        }
        // 6. 자기 pane에 대한 일은 허용.
        if let (Caller::Pane { pane, .. }, Some(t)) = (caller, &target.pane) {
            if pane == t {
                return Verdict::new(Decision::Allow, "builtin:self", "own pane");
            }
        }
        // 7. 기본값.
        let d = &self.cfg.defaults;
        match class {
            ActionClass::Meta | ActionClass::Hook => {
                Verdict::new(Decision::Allow, "builtin:meta", "read-only metadata")
            }
            ActionClass::Read => {
                if d.agent_read == "deny" {
                    Verdict::new(
                        Decision::Deny,
                        "defaults.agent_read",
                        "reading other panes is denied",
                    )
                } else {
                    Verdict::new(Decision::Allow, "defaults.agent_read", "masked read")
                }
            }
            ActionClass::Create => {
                Verdict::new(d.agent_create, "defaults.agent_create", "create layout")
            }
            ActionClass::Control => {
                let dec = if matches!(action, "agent.prompt" | "agent.start") {
                    d.agent_to_agent_prompt
                } else {
                    d.agent_to_agent_run
                };
                let id = if matches!(action, "agent.prompt" | "agent.start") {
                    "defaults.agent_to_agent_prompt"
                } else {
                    "defaults.agent_to_agent_run"
                };
                Verdict::new(dec, id, "agent acting on another pane")
            }
            ActionClass::Admin => unreachable!("handled above"),
        }
    }

    /// 훅으로 들어온 도구 호출 판정.
    pub fn evaluate_tool(&self, agent: &str, tool: &str, input: &str) -> ToolVerdict {
        tools::evaluate(&self.tool_rules, agent, tool, input)
    }

    /// 도구 입력(JSON)으로 판정한다. 규칙은 JSON 글자([`tools::match_text`])에 맞춰 본다.
    /// `canonical`은 기록·화면에 쓰는 정규 JSON 문자열이다.
    pub fn evaluate_tool_json(
        &self,
        agent: &str,
        tool: &str,
        input: &serde_json::Value,
        canonical: &str,
    ) -> ToolVerdict {
        tools::evaluate(
            &self.tool_rules,
            agent,
            tool,
            &tools::match_text(canonical, input),
        )
    }
}

fn glob_match(pat: &str, action: &str) -> bool {
    if pat == "*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix(".*") {
        return action.starts_with(prefix) && action[prefix.len()..].starts_with('.');
    }
    pat == action
}

fn match_caller(m: &Matcher, c: &Caller) -> bool {
    match c {
        Caller::User => m.user.unwrap_or(false) && m.agent.is_none() && m.plugin.is_none(),
        Caller::Pane {
            pane,
            workspace,
            agent_kind,
            agent_name,
        } => {
            m.user != Some(true)
                && m.plugin.is_none()
                && opt_eq(&m.agent, agent_kind)
                && opt_eq(&m.agent_name, agent_name)
                && opt_eq(&m.workspace, &Some(workspace.clone()))
                && opt_eq(&m.pane, &Some(pane.clone()))
        }
        Caller::Plugin { name, .. } => {
            m.user != Some(true) && m.agent.is_none() && opt_eq(&m.plugin, &Some(name.clone()))
        }
    }
}

fn match_target(m: &Matcher, t: &Target) -> bool {
    opt_eq(&m.agent, &t.agent_kind)
        && opt_eq(&m.agent_name, &t.agent_name)
        && opt_eq(&m.workspace, &t.workspace)
        && opt_eq(&m.pane, &t.pane)
}

/// 조건이 없으면 참, 있으면 값이 같아야 참.
fn opt_eq(cond: &Option<String>, val: &Option<String>) -> bool {
    match cond {
        None => true,
        Some(c) => val.as_deref() == Some(c.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pane: &str, kind: &str) -> Caller {
        Caller::Pane {
            pane: pane.into(),
            workspace: "w1".into(),
            agent_kind: Some(kind.into()),
            agent_name: None,
        }
    }

    fn to(pane: &str) -> Target {
        Target {
            pane: Some(pane.into()),
            workspace: Some("w1".into()),
            agent_kind: Some("codex".into()),
            agent_name: Some("reviewer".into()),
        }
    }

    #[test]
    fn defaults() {
        let mut e = PolicyEngine::new(PolicyConfig::default());
        let a = agent("w1:p1", "claude");
        assert_eq!(
            e.evaluate(&Caller::User, "agent.prompt", &to("w1:p2"))
                .decision,
            Decision::Allow
        );
        assert_eq!(
            e.evaluate(&a, "agent.prompt", &to("w1:p2")).decision,
            Decision::Ask
        );
        assert_eq!(
            e.evaluate(&a, "pane.run", &to("w1:p2")).decision,
            Decision::Ask
        );
        assert_eq!(
            e.evaluate(&a, "pane.read", &to("w1:p2")).decision,
            Decision::Allow
        );
        assert_eq!(
            e.evaluate(&a, "pane.list", &Target::default()).decision,
            Decision::Allow
        );
        assert_eq!(
            e.evaluate(&a, "pane.split", &to("w1:p1")).decision,
            Decision::Allow
        );
        // 자기 pane은 허용
        assert_eq!(
            e.evaluate(&a, "pane.run", &to("w1:p1")).decision,
            Decision::Allow
        );
        // 에이전트는 승인·서버 정지를 할 수 없다
        assert_eq!(
            e.evaluate(&a, "approval.decide", &Target::default())
                .decision,
            Decision::Deny
        );
        assert_eq!(
            e.evaluate(&a, "server.stop", &Target::default()).decision,
            Decision::Deny
        );
    }

    #[test]
    fn only_people_resize_layout() {
        let mut e = PolicyEngine::new(PolicyConfig::default());
        assert_eq!(
            e.evaluate(
                &agent("w1:p1", "claude"),
                "layout.set_ratio",
                &Target::default()
            )
            .decision,
            Decision::Deny
        );
        assert_eq!(
            e.evaluate(&Caller::User, "layout.set_ratio", &Target::default())
                .decision,
            Decision::Allow
        );
    }

    #[test]
    fn user_rule_allows_specific_pair() {
        let cfg = PolicyConfig::parse(
            r#"
            [[rule]]
            id = "claude-to-reviewer"
            from = { agent = "claude", workspace = "w1" }
            action = "agent.prompt"
            to = { agent_name = "reviewer" }
            decision = "allow"
            rate = "2/min"
            "#,
        )
        .unwrap();
        let mut e = PolicyEngine::new(cfg);
        let a = agent("w1:p1", "claude");
        let v = e.evaluate(&a, "agent.prompt", &to("w1:p2"));
        assert_eq!(v.decision, Decision::Allow);
        assert_eq!(v.rule_id, "claude-to-reviewer");
        assert_eq!(
            e.evaluate(&a, "agent.prompt", &to("w1:p2")).decision,
            Decision::Allow
        );
        // 분당 2회 초과
        assert_eq!(
            e.evaluate(&a, "agent.prompt", &to("w1:p2")).decision,
            Decision::Deny
        );
        // 다른 에이전트는 규칙에 안 맞아 기본값(ask)
        assert_eq!(
            e.evaluate(&agent("w1:p3", "codex"), "agent.prompt", &to("w1:p2"))
                .decision,
            Decision::Ask
        );
    }

    #[test]
    fn plugin_capabilities() {
        let mut e = PolicyEngine::new(PolicyConfig::default());
        let p = Caller::Plugin {
            name: "board".into(),
            capabilities: vec!["layout".into()],
        };
        assert_eq!(
            e.evaluate(&p, "pane.read", &to("w1:p2")).decision,
            Decision::Deny
        );
        assert_eq!(
            e.evaluate(&p, "pane.split", &to("w1:p2")).decision,
            Decision::Allow
        );
        assert_eq!(
            e.evaluate(&p, "pane.list", &Target::default()).decision,
            Decision::Allow
        );
    }

    #[test]
    fn global_rate_limit() {
        let mut cfg = PolicyConfig::default();
        cfg.defaults.rate_per_min = 3;
        cfg.defaults.agent_to_agent_prompt = Decision::Allow;
        let mut e = PolicyEngine::new(cfg);
        let a = agent("w1:p1", "claude");
        for _ in 0..3 {
            assert_eq!(
                e.evaluate(&a, "agent.prompt", &to("w1:p2")).decision,
                Decision::Allow
            );
        }
        let v = e.evaluate(&a, "agent.prompt", &to("w1:p2"));
        assert_eq!(v.decision, Decision::Deny);
        assert_eq!(v.rule_id, "builtin:rate");
    }

    #[test]
    fn parse_errors() {
        assert!(PolicyConfig::parse("[[rule]]\naction='x'\ndecision='maybe'").is_err());
        assert!(
            PolicyConfig::parse("[[rule]]\naction='x'\ndecision='allow'\nrate='fast'").is_err()
        );
        assert!(PolicyConfig::parse("[sandbox]\ndefault_profile='jail'").is_err());
        assert!(PolicyConfig::parse("").is_ok());
    }

    #[test]
    fn globs() {
        assert!(glob_match("*", "a.b"));
        assert!(glob_match("pane.*", "pane.read"));
        assert!(!glob_match("pane.*", "panel.read"));
        assert!(glob_match("agent.prompt", "agent.prompt"));
    }
}
