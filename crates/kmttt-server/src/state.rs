//! 서버가 들고 있는 세션 상태와 스냅숏 만들기.

use kmttt_core::layout::{self, Rect};
use kmttt_proto::*;
use kmttt_pty::PtyProcess;
use kmttt_sandbox::Profile;
use kmttt_term::{Screen, TextLog};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

pub const RING: usize = 300;

pub struct Workspace {
    pub id: String,
    pub label: String,
    pub cwd: String,
    pub worktree: Option<WorktreeInfo>,
    pub tabs: Vec<Tab>,
    pub focus_tab: String,
    pub next_pane: u32,
    pub next_tab: u32,
}

pub struct Tab {
    pub id: String,
    pub label: String,
    pub layout: LayoutNode,
    pub focus: String,
    pub zoomed: bool,
}

pub struct AgentRt {
    pub kind: String,
    pub state: AgentState,
    pub since_ms: i64,
    pub evidence: String,
    pub hooked: bool,
    /// done 판정을 위해: 마지막으로 working이었는지
    pub was_working: bool,
}

pub struct Pane {
    pub id: String,
    pub ws: String,
    pub tab: String,
    pub cwd: String,
    pub command: String,
    pub pty: Option<PtyProcess>,
    pub screen: Screen,
    pub log: TextLog,
    pub pid: Option<u32>,
    pub exited: Option<i32>,
    pub agent: Option<AgentRt>,
    /// 사용자가 붙인 에이전트 이름(reviewer 등)
    pub alias: Option<String>,
    pub last_output: Option<Instant>,
    pub profile: Profile,
    pub cols: u16,
    pub rows: u16,
    pub blocked_calls: u32,
    pub usage: (f32, u64, u32),
    pub hook_at: Option<Instant>,
    /// 전면 프로세스(pid, 이름, 명령줄)와 조상 체인 요약
    pub fg: Option<(u32, String, String)>,
    pub fg_chain: Vec<String>,
    /// OS 차원의 pane 묶음(Windows Job Object). 부모를 끊은 자손도 여기로 식별한다.
    pub job: Option<std::sync::Arc<kmttt_pty::reaper::PaneJob>>,
}

pub struct ClientRt {
    pub tx: mpsc::UnboundedSender<RpcMessage>,
    pub attached: bool,
    /// 구독한 이벤트 종류(None이면 구독 안 함, "*"는 전부)
    pub subs: Option<Vec<String>>,
    pub label: String,
}

pub struct PendingApproval {
    pub info: Approval,
    pub tx: Option<oneshot::Sender<bool>>,
}

#[derive(Default)]
pub struct State {
    pub workspaces: Vec<Workspace>,
    pub focus_ws: String,
    pub next_ws: u32,
    pub panes: HashMap<String, Pane>,
    pub clients: HashMap<u64, ClientRt>,
    pub next_client: u64,
    /// pane 영역 크기(cols, rows)
    pub viewport: (u16, u16),
    /// 데스크톱 화면이 창마다 위에 제목 줄을 그리면 1. 그만큼 창 높이를 줄여 터미널을 가리지 않는다.
    pub header_rows: u16,
    pub approvals: Vec<PendingApproval>,
    pub next_approval: u64,
    pub events: VecDeque<SecurityEvent>,
    pub next_event: u64,
    pub flows: VecDeque<Flow>,
    pub tools: VecDeque<ToolCall>,
    pub blocked_today: u64,
    /// pane별 아직 기록하지 않은 출력(바이트 수, 텍스트)
    pub out_acc: HashMap<String, (u64, String)>,
    /// 서버가 실행한 플러그인 프로세스 → 플러그인 이름
    pub plugin_pids: HashMap<u32, String>,
    pub seen_fingerprints: HashSet<String>,
    pub last_snapshot_key: String,
}

impl State {
    pub fn new() -> Self {
        Self {
            viewport: (120, 40),
            ..Default::default()
        }
    }

    pub fn ws(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn ws_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn tab_mut(&mut self, tab_id: &str) -> Option<&mut Tab> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| w.tabs.iter_mut())
            .find(|t| t.id == tab_id)
    }

    pub fn tab(&self, tab_id: &str) -> Option<&Tab> {
        self.workspaces
            .iter()
            .flat_map(|w| w.tabs.iter())
            .find(|t| t.id == tab_id)
    }

    /// 지금 화면에 보이는 탭.
    pub fn focused_tab(&self) -> Option<&Tab> {
        let ws = self.ws(&self.focus_ws)?;
        ws.tabs.iter().find(|t| t.id == ws.focus_tab)
    }

    pub fn focused_pane(&self) -> Option<String> {
        self.focused_tab().map(|t| t.focus.clone())
    }

    /// 화면에 보이는 pane들(확대 중이면 그 pane 하나).
    pub fn visible_panes(&self) -> Vec<String> {
        match self.focused_tab() {
            Some(t) if t.zoomed => vec![t.focus.clone()],
            Some(t) => layout::panes(&t.layout),
            None => vec![],
        }
    }

    pub fn any_attached(&self) -> bool {
        self.clients.values().any(|c| c.attached)
    }

    /// pane ID, 에이전트 별칭으로 pane을 찾는다. 없으면 초점 pane.
    pub fn resolve_pane(&self, target: Option<&str>) -> Option<String> {
        match target {
            None | Some("") | Some("focused") => self.focused_pane(),
            Some(t) => {
                if self.panes.contains_key(t) {
                    return Some(t.to_string());
                }
                self.panes
                    .values()
                    .find(|p| p.alias.as_deref() == Some(t))
                    .map(|p| p.id.clone())
            }
        }
    }

    pub fn push_event(
        &mut self,
        severity: Severity,
        kind: &str,
        caller: &str,
        target: &str,
        detail: &str,
    ) -> SecurityEvent {
        self.next_event += 1;
        let ev = SecurityEvent {
            id: self.next_event,
            ts_ms: now_ms(),
            severity,
            kind: kind.into(),
            caller: caller.into(),
            target: target.into(),
            detail: detail.into(),
        };
        self.events.push_back(ev.clone());
        while self.events.len() > RING {
            self.events.pop_front();
        }
        ev
    }

    pub fn push_flow(&mut self, f: Flow) {
        self.flows.push_back(f);
        while self.flows.len() > RING {
            self.flows.pop_front();
        }
    }

    pub fn push_tool(&mut self, t: ToolCall) {
        self.tools.push_back(t);
        while self.tools.len() > RING {
            self.tools.pop_front();
        }
    }

    /// 모든 탭의 레이아웃을 현재 viewport로 계산해 pane 크기를 맞춘다.
    pub fn apply_layout(&mut self) {
        let (cols, rows) = self.viewport;
        let hdr = self.header_rows;
        let area = Rect {
            x: 0,
            y: 0,
            w: cols,
            h: rows,
        };
        let mut sizes: Vec<(String, u16, u16)> = Vec::new();
        for ws in &self.workspaces {
            for t in &ws.tabs {
                for (p, r) in layout::compute(&t.layout, area) {
                    let (w, h) = if t.zoomed && p == t.focus {
                        (cols, rows)
                    } else {
                        (r.w, r.h)
                    };
                    let h = if h > hdr + 1 { h - hdr } else { h };
                    sizes.push((p, w.max(2), h.max(1)));
                }
            }
        }
        for (id, w, h) in sizes {
            if let Some(p) = self.panes.get_mut(&id) {
                if p.cols != w || p.rows != h {
                    p.cols = w;
                    p.rows = h;
                    p.screen.resize(h, w);
                    if let Some(pty) = &p.pty {
                        let _ = pty.resize(h, w);
                    }
                }
            }
        }
    }

    pub fn pane_info(&self, p: &Pane, redact: &dyn Fn(&str) -> String) -> PaneInfo {
        PaneInfo {
            id: p.id.clone(),
            workspace: p.ws.clone(),
            tab: p.tab.clone(),
            cwd: p.cwd.clone(),
            command: p.command.clone(),
            title: redact(p.screen.title()),
            pid: p.pid,
            cols: p.cols,
            rows: p.rows,
            exited: p.exited,
            agent: p.agent.as_ref().map(|a| AgentInfo {
                kind: a.kind.clone(),
                name: p.alias.clone(),
                state: a.state,
                since_ms: a.since_ms,
                evidence: a.evidence.clone(),
                hooked: a.hooked,
            }),
            sandbox: p.profile.as_str().into(),
            sandbox_strength: p.profile.strength().into(),
            cpu: p.usage.0,
            mem_bytes: p.usage.1,
            procs: p.usage.2,
            last_line: redact(&p.screen.last_line()).chars().take(200).collect(),
            blocked_calls: p.blocked_calls,
        }
    }

    pub fn snapshot(&self, session: &str, redact: &dyn Fn(&str) -> String) -> SessionSnapshot {
        let workspaces = self
            .workspaces
            .iter()
            .map(|w| WorkspaceInfo {
                id: w.id.clone(),
                label: w.label.clone(),
                cwd: w.cwd.clone(),
                worktree: w.worktree.clone(),
                tabs: w
                    .tabs
                    .iter()
                    .map(|t| TabInfo {
                        id: t.id.clone(),
                        label: t.label.clone(),
                        layout: t.layout.clone(),
                        focus: t.focus.clone(),
                        zoomed: t.zoomed,
                    })
                    .collect(),
                focus_tab: w.focus_tab.clone(),
            })
            .collect();
        let mut panes: Vec<PaneInfo> = self
            .panes
            .values()
            .map(|p| self.pane_info(p, redact))
            .collect();
        panes.sort_by_key(|p| natural_key(&p.id));
        SessionSnapshot {
            session: session.into(),
            workspaces,
            panes,
            focus_workspace: self.focus_ws.clone(),
        }
    }

    /// 화면 변화 판단용 키(자주 바뀌는 CPU·마지막 줄은 뺀다).
    pub fn snapshot_key(snap: &SessionSnapshot) -> String {
        let mut s = snap.clone();
        for p in &mut s.panes {
            p.cpu = 0.0;
            p.mem_bytes = 0;
            p.procs = 0;
            p.last_line.clear();
            if let Some(a) = &mut p.agent {
                a.evidence.clear();
            }
        }
        serde_json::to_string(&s).unwrap_or_default()
    }
}

/// "w2:p10" → (2, 10) 정렬용
pub fn natural_key(id: &str) -> (u32, u32) {
    let mut nums = id
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(0));
    (nums.next().unwrap_or(0), nums.next().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["w1:p10", "w1:p2", "w2:p1"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, vec!["w1:p2", "w1:p10", "w2:p1"]);
    }
}
