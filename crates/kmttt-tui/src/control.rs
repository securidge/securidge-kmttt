//! 통합 관제 모드(prefix + d). 로컬과 원격 머신의 관제 스냅숏을 모아 한 화면에 보여 준다.

use crate::render::{sev_color, state_color};
use kmttt_core::client::RpcClient;
use kmttt_core::i18n::t;
use kmttt_core::machines::MachineProfile;
use kmttt_proto::*;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Agents,
    Approvals,
    Events,
    Flows,
    Tools,
}

impl Panel {
    pub fn next(self) -> Self {
        match self {
            Panel::Agents => Panel::Approvals,
            Panel::Approvals => Panel::Events,
            Panel::Events => Panel::Flows,
            Panel::Flows => Panel::Tools,
            Panel::Tools => Panel::Agents,
        }
    }
}

/// 머신 하나의 최신 상태.
pub struct MachineView {
    pub label: String,
    pub snap: Option<ControlSnapshot>,
    pub error: Option<String>,
    pub client: Option<Arc<RpcClient>>,
    pub stale: bool,
}

pub struct ControlState {
    pub panel: Panel,
    pub sel: usize,
    pub machines: Vec<MachineView>,
    pub detail: Option<String>,
}

/// 원격 머신마다 SSH 연결을 유지하며 1초마다 스냅숏을 보낸다.
pub fn spawn_remote(idx: usize, profile: MachineProfile, tx: mpsc::UnboundedSender<RemoteMsg>) {
    tokio::spawn(async move {
        loop {
            match RpcClient::connect_ssh(&profile.target, &profile.session, &profile.bin).await {
                Ok((c, _notes)) => {
                    let c = Arc::new(c);
                    loop {
                        match c
                            .call_timeout(
                                "control.snapshot",
                                serde_json::json!({}),
                                Duration::from_secs(10),
                            )
                            .await
                        {
                            Ok(v) => match serde_json::from_value::<ControlSnapshot>(v) {
                                Ok(s) => {
                                    if tx.send((idx, Ok((s, c.clone())))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send((idx, Err(e.to_string())));
                                }
                            },
                            Err(e) => {
                                let _ = tx.send((idx, Err(e.to_string())));
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                Err(e) => {
                    if tx.send((idx, Err(format!("ssh: {e}")))).is_err() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

/// 모든 머신의 pane을 (머신 이름, pane) 쌍으로, 급한 순서로.
pub fn all_panes(cs: &ControlState) -> Vec<(usize, &PaneInfo)> {
    let mut v: Vec<(usize, &PaneInfo)> = cs
        .machines
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            m.snap
                .iter()
                .flat_map(move |s| s.panes.iter().map(move |p| (i, p)))
        })
        .collect();
    v.sort_by_key(|(_, p)| {
        (
            p.agent.as_ref().map(|a| a.state.priority()).unwrap_or(8),
            p.agent.is_none(),
        )
    });
    v
}

pub fn all_approvals(cs: &ControlState) -> Vec<(usize, &Approval)> {
    cs.machines
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            m.snap
                .iter()
                .flat_map(move |s| s.approvals.iter().map(move |a| (i, a)))
        })
        .collect()
}

fn all_events(cs: &ControlState) -> Vec<(usize, &SecurityEvent)> {
    let mut v: Vec<(usize, &SecurityEvent)> = cs
        .machines
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            m.snap
                .iter()
                .flat_map(move |s| s.events.iter().map(move |e| (i, e)))
        })
        .collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.ts_ms));
    v
}

fn all_flows(cs: &ControlState) -> Vec<(usize, &Flow)> {
    let mut v: Vec<(usize, &Flow)> = cs
        .machines
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            m.snap
                .iter()
                .flat_map(move |s| s.flows.iter().map(move |e| (i, e)))
        })
        .collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.ts_ms));
    v
}

fn all_tools(cs: &ControlState) -> Vec<(usize, &ToolCall)> {
    let mut v: Vec<(usize, &ToolCall)> = cs
        .machines
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            m.snap
                .iter()
                .flat_map(move |s| s.tools.iter().map(move |e| (i, e)))
        })
        .collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.ts_ms));
    v
}

pub fn panel_len(cs: &ControlState) -> usize {
    match cs.panel {
        Panel::Agents => all_panes(cs).len(),
        Panel::Approvals => all_approvals(cs).len(),
        Panel::Events => all_events(cs).len(),
        Panel::Flows => all_flows(cs).len(),
        Panel::Tools => all_tools(cs).len(),
    }
}

fn hhmmss(ms: i64) -> String {
    let s = (ms / 1000).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn ago(ms: i64) -> String {
    let d = (now_ms() - ms).max(0) / 1000;
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else {
        format!("{}h", d / 3600)
    }
}

fn bytes(n: u64) -> String {
    if n > 1 << 30 {
        format!("{:.1}G", n as f64 / (1u64 << 30) as f64)
    } else {
        format!("{}M", n >> 20)
    }
}

fn block<'a>(title: String, active: bool) -> Block<'a> {
    let st = if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(st)
        .title(Span::styled(format!(" {title} "), st))
}

fn sel_style(active: bool, i: usize, sel: usize) -> Style {
    if active && i == sel {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

pub fn draw(f: &mut Frame, cs: &ControlState, lang: &str) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(10),
            Constraint::Length(1),
        ])
        .split(area);
    draw_summary(f, cs, lang, rows[0]);
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[1]);
    draw_agents(f, cs, lang, mid[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(mid[1]);
    draw_approvals(f, cs, lang, right[0]);
    draw_events(f, cs, lang, right[1]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(36),
            Constraint::Percentage(36),
            Constraint::Percentage(28),
        ])
        .split(rows[2]);
    draw_flows(f, cs, lang, bottom[0]);
    draw_tools(f, cs, lang, bottom[1]);
    draw_integrity(f, cs, lang, bottom[2]);
    let footer = Line::from(vec![
        Span::styled(
            format!(" {} ", t(lang, "credit")),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            cs.detail.clone().unwrap_or_else(|| t(lang, "control.help")),
            Style::default().fg(if cs.detail.is_some() {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ),
    ]);
    f.render_widget(Paragraph::new(footer), rows[3]);
}

fn draw_summary(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let panes = all_panes(cs);
    let mut counts = [0usize; 5];
    for (_, p) in &panes {
        if let Some(a) = &p.agent {
            counts[a.state.priority() as usize] += 1;
        }
    }
    let pending = all_approvals(cs).len();
    let blocked: u64 = cs
        .machines
        .iter()
        .filter_map(|m| m.snap.as_ref().map(|s| s.blocked_today))
        .sum();
    let ok = cs
        .machines
        .iter()
        .all(|m| m.snap.as_ref().is_none_or(|s| s.integrity.ok));
    let agents: usize = counts.iter().sum();
    let spans = vec![
        Span::styled(
            format!(" {} ", t(lang, "control.title")),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {agents} {} ", t(lang, "control.summary_agents"))),
        Span::styled(
            format!("▲{} ", counts[0]),
            Style::default().fg(state_color(AgentState::Blocked)),
        ),
        Span::styled(
            format!("✓{} ", counts[1]),
            Style::default().fg(state_color(AgentState::Done)),
        ),
        Span::styled(
            format!("●{} ", counts[2]),
            Style::default().fg(state_color(AgentState::Working)),
        ),
        Span::styled(
            format!("○{} ", counts[4]),
            Style::default().fg(state_color(AgentState::Idle)),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("⚑ {pending} {} ", t(lang, "control.summary_pending")),
            if pending > 0 {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            },
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("⊘ {blocked} {} ", t(lang, "control.summary_blocked")),
            Style::default().fg(if blocked > 0 { Color::Red } else { Color::Gray }),
        ),
        Span::raw(" │ "),
        Span::styled(
            if ok {
                "ledger OK".to_string()
            } else {
                "ledger FAIL".to_string()
            },
            Style::default()
                .fg(if ok { Color::Green } else { Color::Red })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " │ {} {}",
            cs.machines.len(),
            t(lang, "control.machines")
        )),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_agents(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let active = cs.panel == Panel::Agents;
    let multi = cs.machines.len() > 1;
    // pane 열 너비: 원격 머신 이름이 붙으면 길어지므로 내용에 맞춘다(7~28칸).
    let pane_w = all_panes(cs)
        .iter()
        .map(|(mi, p)| {
            let m = if multi {
                cs.machines[*mi].label.chars().count() + 1
            } else {
                0
            };
            m + p.id.chars().count()
        })
        .max()
        .unwrap_or(7)
        .clamp(7, 28) as u16;
    let rows: Vec<Row> = all_panes(cs)
        .iter()
        .enumerate()
        .map(|(i, (mi, p))| {
            let (icon, state, name, since, hooked) = match &p.agent {
                Some(a) => (
                    a.state.icon().to_string(),
                    Some(a.state),
                    a.name.clone().unwrap_or_else(|| a.kind.clone()),
                    ago(a.since_ms),
                    if a.hooked { "hook" } else { "screen" },
                ),
                None => (
                    " ".into(),
                    None,
                    p.command
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    String::new(),
                    "",
                ),
            };
            let color = state.map(state_color).unwrap_or(Color::DarkGray);
            let machine = if multi {
                format!("{}:", cs.machines[*mi].label)
            } else {
                String::new()
            };
            Row::new(vec![
                Cell::from(Span::styled(icon, Style::default().fg(color))),
                Cell::from(name),
                Cell::from(format!("{machine}{}", p.id)),
                Cell::from(Span::styled(
                    state
                        .map(|s| t(lang, &format!("state.{}", s.as_str())))
                        .unwrap_or_default(),
                    Style::default().fg(color),
                )),
                Cell::from(since),
                Cell::from(Span::styled(
                    format!("{}/{}", p.sandbox, p.sandbox_strength),
                    Style::default().fg(if p.sandbox == "none" {
                        Color::DarkGray
                    } else {
                        Color::Green
                    }),
                )),
                Cell::from(Span::styled(
                    if p.blocked_calls > 0 {
                        format!("⊘{}", p.blocked_calls)
                    } else {
                        String::new()
                    },
                    Style::default().fg(Color::Red),
                )),
                Cell::from(format!("{:.0}% {}", p.cpu, bytes(p.mem_bytes))),
                Cell::from(Span::styled(hooked, Style::default().fg(Color::DarkGray))),
                Cell::from(Span::styled(
                    p.last_line.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .style(sel_style(active, i, cs.sel))
        })
        .collect();
    let widths = [
        Constraint::Length(2),
        Constraint::Length(10),
        Constraint::Length(pane_w),
        Constraint::Length(9),
        Constraint::Length(4),
        Constraint::Length(17),
        Constraint::Length(4),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Fill(1),
    ];
    let header = Row::new(vec![
        "",
        "agent",
        "pane",
        "state",
        "for",
        "sandbox",
        "⊘",
        "cpu mem",
        "signal",
        "last line",
    ])
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(block(t(lang, "control.agents"), active)),
        area,
    );
}

fn draw_approvals(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let active = cs.panel == Panel::Approvals;
    let items = all_approvals(cs);
    let mut lines: Vec<Line> = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {}", t(lang, "control.empty")),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, (_, a)) in items.iter().enumerate() {
        let left = ((a.expires_ms - now_ms()) / 1000).max(0);
        lines.push(
            Line::from(vec![
                Span::styled(
                    format!(" #{} ", a.id),
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                ),
                Span::raw(format!(" {} → {} {} ", a.caller, a.action, a.target)),
                Span::styled(format!("{left}s"), Style::default().fg(Color::DarkGray)),
            ])
            .style(sel_style(active, i, cs.sel)),
        );
        lines.push(Line::from(Span::styled(
            format!("    {}", a.detail.replace('\n', " ")),
            Style::default().fg(Color::Gray),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).block(block(
            format!("{} (a/x)", t(lang, "control.approvals")),
            active,
        )),
        area,
    );
}

fn draw_events(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let active = cs.panel == Panel::Events;
    let lines: Vec<Line> = all_events(cs)
        .iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(i, (_, e))| {
            Line::from(vec![
                Span::styled(
                    format!(" {} ", e.severity.icon()),
                    Style::default()
                        .fg(sev_color(e.severity))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} ", hhmmss(e.ts_ms)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", e.kind),
                    Style::default().fg(sev_color(e.severity)),
                ),
                Span::raw(format!("{} {} — {}", e.caller, e.target, e.detail)),
            ])
            .style(sel_style(active, i, cs.sel))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(block(t(lang, "control.events"), active)),
        area,
    );
}

fn draw_flows(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let active = cs.panel == Panel::Flows;
    let lines: Vec<Line> = all_flows(cs)
        .iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(i, (_, fl))| {
            let c = if fl.decision.ends_with("deny") {
                Color::Red
            } else if fl.decision.starts_with("ask") {
                Color::Yellow
            } else {
                Color::Green
            };
            Line::from(vec![
                Span::styled(
                    format!(" {} ", hhmmss(fl.ts_ms)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{} ─{}→ {} ", fl.from, fl.action, fl.to)),
                Span::styled(fl.decision.clone(), Style::default().fg(c)),
            ])
            .style(sel_style(active, i, cs.sel))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(block(t(lang, "control.flows"), active)),
        area,
    );
}

fn draw_tools(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let active = cs.panel == Panel::Tools;
    let lines: Vec<Line> = all_tools(cs)
        .iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(i, (_, tc))| {
            let c = if tc.decision.ends_with("deny") {
                Color::Red
            } else if tc.decision.starts_with("ask") {
                Color::Yellow
            } else {
                Color::Gray
            };
            Line::from(vec![
                Span::styled(
                    format!(" {} ", hhmmss(tc.ts_ms)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{} ", tc.pane), Style::default().fg(Color::Cyan)),
                Span::raw(format!("{} {} ", tc.tool, tc.summary)),
                Span::styled(tc.decision.clone(), Style::default().fg(c)),
            ])
            .style(sel_style(active, i, cs.sel))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(block(t(lang, "control.tools"), active)),
        area,
    );
}

fn draw_integrity(f: &mut Frame, cs: &ControlState, lang: &str, area: Rect) {
    let mut lines = Vec::new();
    for m in &cs.machines {
        let (sym, color, text) = match (&m.snap, &m.error) {
            (Some(s), _) => {
                let i = &s.integrity;
                let cp = i.last_checkpoint_ms.map(ago).unwrap_or_else(|| "-".into());
                (
                    if i.ok { "●" } else { "✗" },
                    if i.ok && !m.stale {
                        Color::Green
                    } else if m.stale {
                        Color::DarkGray
                    } else {
                        Color::Red
                    },
                    format!(
                        "{} rec · cp {cp} · {} · key {}",
                        i.records,
                        i.level,
                        i.key_fingerprint.chars().take(8).collect::<String>()
                    ),
                )
            }
            (None, Some(e)) => ("✗", Color::Red, e.clone()),
            (None, None) => ("…", Color::DarkGray, "connecting".into()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {sym} "), Style::default().fg(color)),
            Span::styled(
                format!("{} ", m.label),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("   {text}"),
            Style::default().fg(Color::Gray),
        )));
        if let Some(s) = &m.snap {
            if !s.integrity.ok {
                lines.push(Line::from(Span::styled(
                    format!("   {}", s.integrity.detail),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(block(
            format!(
                "{} · {}",
                t(lang, "control.integrity"),
                t(lang, "control.machines")
            ),
            false,
        )),
        area,
    );
}

/// 원격 승인 결정용으로 머신의 클라이언트를 돌려준다(로컬은 None → 호출자가 자기 연결을 씀).
pub fn machine_client(cs: &ControlState, idx: usize) -> Option<Arc<RpcClient>> {
    cs.machines.get(idx).and_then(|m| m.client.clone())
}

/// 원격 머신 스냅숏 메시지: (머신 순번, 스냅숏+클라이언트 또는 오류).
pub type RemoteMsg = (usize, Result<(ControlSnapshot, Arc<RpcClient>), String>);
pub type RemoteRx = mpsc::UnboundedReceiver<RemoteMsg>;

#[allow(dead_code)]
pub type Shared<T> = Arc<Mutex<T>>;
