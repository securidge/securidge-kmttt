//! 서버와 클라이언트가 주고받는 데이터 구조.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 에이전트 생명주기 상태 다섯 가지.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// 일하는 중
    Working,
    /// 입력·승인·결정을 기다림
    Blocked,
    /// 백그라운드에서 끝났고 아직 보지 않음
    Done,
    /// 준비됨, 이미 봤음
    Idle,
    /// 에이전트는 맞지만 상태를 확신할 수 없음
    #[default]
    Unknown,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Idle => "idle",
            AgentState::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            "idle" => AgentState::Idle,
            "unknown" => AgentState::Unknown,
            _ => return None,
        })
    }
    /// 관제 화면 정렬 우선순위(작을수록 위).
    pub fn priority(self) -> u8 {
        match self {
            AgentState::Blocked => 0,
            AgentState::Done => 1,
            AgentState::Working => 2,
            AgentState::Unknown => 3,
            AgentState::Idle => 4,
        }
    }
    /// 색이 없는 터미널에서도 구분되도록 상태마다 기호를 둔다.
    pub fn icon(self) -> &'static str {
        match self {
            AgentState::Working => "●",
            AgentState::Blocked => "▲",
            AgentState::Done => "✓",
            AgentState::Idle => "○",
            AgentState::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentInfo {
    /// 에이전트 종류(claude, codex, opencode …)
    pub kind: String,
    /// 사용자가 붙인 별칭(reviewer 등)
    pub name: Option<String>,
    pub state: AgentState,
    /// 현재 상태가 시작된 시각(UNIX ms)
    pub since_ms: i64,
    /// 상태 판단 근거 한 줄
    pub evidence: String,
    /// 훅으로 직접 신호를 보내는지(아니면 화면 관찰 전용)
    pub hooked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    /// 좌우로 나눔(오른쪽에 새 pane)
    Right,
    /// 위아래로 나눔(아래에 새 pane)
    Down,
}

/// 탭 하나의 pane 배치 트리.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    Leaf {
        pane: String,
    },
    Split {
        dir: SplitDir,
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: String,
    pub workspace: String,
    pub tab: String,
    pub cwd: String,
    pub command: String,
    pub title: String,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
    pub agent: Option<AgentInfo>,
    /// 격리 프로필 이름(none, restricted, container, wsl)
    pub sandbox: String,
    /// 실제 격리 강도(none, medium, high)
    pub sandbox_strength: String,
    pub cpu: f32,
    pub mem_bytes: u64,
    pub procs: u32,
    /// 마지막 출력 한 줄(마스킹 적용)
    pub last_line: String,
    /// 이 pane의 정책 차단 건수
    pub blocked_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabInfo {
    pub id: String,
    pub label: String,
    pub layout: LayoutNode,
    pub focus: String,
    pub zoomed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeInfo {
    pub repo: String,
    pub branch: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub label: String,
    pub cwd: String,
    pub worktree: Option<WorktreeInfo>,
    pub tabs: Vec<TabInfo>,
    pub focus_tab: String,
}

/// 세션 전체(레이아웃 + pane) 스냅숏.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionSnapshot {
    pub session: String,
    pub workspaces: Vec<WorkspaceInfo>,
    pub panes: Vec<PaneInfo>,
    pub focus_workspace: String,
}

impl SessionSnapshot {
    pub fn pane(&self, id: &str) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.id == id)
    }
    pub fn workspace(&self, id: &str) -> Option<&WorkspaceInfo> {
        self.workspaces.iter().find(|w| w.id == id)
    }
    /// 현재 화면에 보여야 할 탭.
    pub fn focused_tab(&self) -> Option<&TabInfo> {
        let ws = self.workspace(&self.focus_workspace)?;
        ws.tabs.iter().find(|t| t.id == ws.focus_tab)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Notice,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Notice => "notice",
            Severity::Critical => "critical",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Severity::Info => "i",
            Severity::Notice => "!",
            Severity::Critical => "‼",
        }
    }
}

/// 관제 화면의 보안 이벤트 한 건.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityEvent {
    pub id: u64,
    pub ts_ms: i64,
    pub severity: Severity,
    /// policy.deny, secret.detected, injection.suspect, sandbox.egress_denied, plugin.capability …
    pub kind: String,
    pub caller: String,
    pub target: String,
    pub detail: String,
}

/// 승인 대기열 항목.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Approval {
    pub id: u64,
    pub ts_ms: i64,
    pub expires_ms: i64,
    pub caller: String,
    pub action: String,
    pub target: String,
    /// 무엇을 하려는지 사람이 읽을 수 있는 설명(마스킹 적용)
    pub detail: String,
    /// "이 세션 동안 허용"을 고를 수 있는 요청인가(창 사이 동작만, 도구 호출은 아님)
    #[serde(default)]
    pub session_ok: bool,
}

/// 에이전트 간 메시지 흐름 한 건.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Flow {
    pub ts_ms: i64,
    pub from: String,
    pub to: String,
    pub action: String,
    /// allow, deny, ask→allow, ask→deny
    pub decision: String,
}

/// 훅으로 들어온 도구 호출 한 건.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub ts_ms: i64,
    pub pane: String,
    pub agent: String,
    pub tool: String,
    pub summary: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct IntegrityStatus {
    pub ok: bool,
    pub level: String,
    pub records: u64,
    pub last_checkpoint_ms: Option<i64>,
    pub key_fingerprint: String,
    pub detail: String,
}

/// 관제 화면 한 장에 필요한 모든 것.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ControlSnapshot {
    pub machine: String,
    pub session: String,
    pub ts_ms: i64,
    pub panes: Vec<PaneInfo>,
    pub approvals: Vec<Approval>,
    pub events: Vec<SecurityEvent>,
    pub flows: Vec<Flow>,
    pub tools: Vec<ToolCall>,
    pub integrity: IntegrityStatus,
    pub blocked_today: u64,
    pub version: String,
}

/// 이벤트 구독으로 오는 알림 한 건.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventMsg {
    pub kind: String,
    pub ts_ms: i64,
    pub data: Value,
}

/// 서버 상태 요약(`kmttt status`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerStatus {
    pub version: String,
    pub session: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub panes: usize,
    pub clients: usize,
    pub protocol: u32,
}

/// 프로토콜 버전. 호환되지 않는 변경 때만 올린다.
pub const PROTOCOL_VERSION: u32 = 1;

/// 현재 시각(UNIX ms).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
