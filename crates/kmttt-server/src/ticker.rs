//! 0.5초마다: 전면 프로세스 확인 → 에이전트 식별·상태 감지 → 자원 사용량 → 기록 비우기 → 승인 만료.

use crate::state::AgentRt;
use crate::Server;
use kmttt_detect::ProcView;
use kmttt_proto::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const RECENT: Duration = Duration::from_millis(1500);
/// 훅 신호를 받은 뒤 이 시간 동안은 화면 추정으로 덮어쓰지 않는다.
const HOOK_HOLD: Duration = Duration::from_secs(3);

struct FgInfo {
    pane: String,
    fg: Option<(u32, String, String)>,
    chain: Vec<ProcView>,
    usage: (f32, u64, u32),
}

pub async fn run(server: Arc<Server>) {
    let mut tick: u64 = 0;
    let mut iv = tokio::time::interval(Duration::from_millis(500));
    loop {
        iv.tick().await;
        tick += 1;
        detect(&server, tick.is_multiple_of(2));
        if tick.is_multiple_of(4) {
            flush_output(&server);
        }
        expire_approvals(&server);
        // subreaper로 입양된 고아(pane에서 빠져나온 프로세스)의 좀비를 치운다.
        let reaped = kmttt_pty::reaper::reap_adopted();
        if reaped > 0 {
            tracing::debug!(reaped, "reaped adopted orphans");
        }
        if let Ok(mut l) = server.ledger.lock() {
            if let Some(l) = l.as_mut() {
                let _ = l.tick();
            }
        }
        server.push_snapshot(false);
        // 하루가 바뀌면 "오늘 차단" 수를 0으로(대략 UTC 기준).
        if tick.is_multiple_of(7200) {
            if let Ok(mut st) = server.st.lock() {
                st.blocked_today = 0;
            }
        }
    }
}

fn detect(server: &Arc<Server>, refresh_procs: bool) {
    // 1. pane 루트 PID와 전면 프로세스 그룹 힌트
    let roots: Vec<(String, u32, Option<u32>)> = match server.st.lock() {
        Ok(st) => st
            .panes
            .values()
            .filter_map(|p| {
                p.pid.map(|pid| {
                    (
                        p.id.clone(),
                        pid,
                        p.pty.as_ref().and_then(|t| t.foreground_pgid()),
                    )
                })
            })
            .collect(),
        Err(_) => return,
    };
    // 2. 프로세스 표에서 전면 프로세스·체인·사용량
    let infos: Vec<FgInfo> = {
        let Ok(mut procs) = server.procs.lock() else {
            return;
        };
        if refresh_procs || procs.procs.is_empty() {
            procs.refresh();
        }
        roots
            .iter()
            .map(|(pane, root, pg)| {
                let fg = procs.foreground(*root, *pg).cloned();
                let chain = fg
                    .as_ref()
                    .map(|f| {
                        procs
                            .chain(f.pid, *root)
                            .iter()
                            .map(|e| ProcView {
                                name: e.name.clone(),
                                cmdline: e.cmdline.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                FgInfo {
                    pane: pane.clone(),
                    fg: fg.map(|f| (f.pid, f.name, f.cmdline)),
                    chain,
                    usage: procs.usage(*root),
                }
            })
            .collect()
    };
    // 3. 식별·분류
    let mut changes: Vec<(String, Option<String>, AgentState, AgentState, String)> = Vec::new();
    let mut toasts: Vec<(String, String)> = Vec::new();
    {
        let Ok(detector) = server.detector.read() else {
            return;
        };
        let Ok(mut st) = server.st.lock() else { return };
        let visible = st.visible_panes();
        let attached = st.any_attached();
        let lang = server.lang();
        for info in infos {
            let Some(p) = st.panes.get_mut(&info.pane) else {
                continue;
            };
            p.usage = info.usage;
            p.fg_chain = info.chain.iter().map(|c| c.name.clone()).collect();
            p.fg = info.fg;
            let identified = detector
                .identify(&info.chain)
                .map(|(m, why)| (m.kind.clone(), why));
            let Some((kind, why)) = identified else {
                if p.agent.take().is_some() {
                    changes.push((
                        p.id.clone(),
                        None,
                        AgentState::Unknown,
                        AgentState::Unknown,
                        "agent exited".into(),
                    ));
                }
                continue;
            };
            let recent = p.last_output.is_some_and(|t| t.elapsed() < RECENT);
            let tail = p.screen.tail_lines(40);
            let c = detector.classify(&kind, &tail, recent);
            let is_visible = attached && visible.contains(&p.id);
            let hooked_hold = p.hook_at.is_some_and(|t| t.elapsed() < HOOK_HOLD);
            let agent = p.agent.get_or_insert_with(|| AgentRt {
                kind: kind.clone(),
                state: AgentState::Unknown,
                since_ms: now_ms(),
                evidence: String::new(),
                hooked: false,
                was_working: false,
            });
            if agent.kind != kind {
                agent.kind = kind.clone();
                agent.hooked = false;
            }
            if hooked_hold {
                continue;
            }
            let prev = agent.state;
            let mut next = c.state;
            // done: 백그라운드에서 일하다 조용해졌고 아직 보지 않음
            if next == AgentState::Idle
                && (agent.was_working || prev == AgentState::Done)
                && !is_visible
            {
                next = AgentState::Done;
            }
            if prev == AgentState::Done
                && is_visible
                && next != AgentState::Working
                && next != AgentState::Blocked
            {
                next = AgentState::Idle;
            }
            agent.evidence = format!("{why}; {}", c.evidence);
            if next == AgentState::Working {
                agent.was_working = true;
            } else if next == AgentState::Idle {
                agent.was_working = false;
            }
            if next != prev {
                agent.state = next;
                agent.since_ms = now_ms();
                changes.push((
                    p.id.clone(),
                    Some(kind.clone()),
                    prev,
                    next,
                    agent.evidence.clone(),
                ));
                let name = p.alias.clone().unwrap_or_else(|| kind.clone());
                if !is_visible {
                    match next {
                        AgentState::Done => toasts.push((
                            p.id.clone(),
                            format!(
                                "✓ {} {name}: {}",
                                p.id,
                                kmttt_core::i18n::t(&lang, "toast.done")
                            ),
                        )),
                        AgentState::Blocked => toasts.push((
                            p.id.clone(),
                            format!(
                                "▲ {} {name}: {}",
                                p.id,
                                kmttt_core::i18n::t(&lang, "toast.blocked")
                            ),
                        )),
                        _ => {}
                    }
                }
            }
        }
    }
    let delay = server
        .cfg
        .read()
        .map(|c| c.ui.toast.delay_seconds)
        .unwrap_or(0);
    for (pane, kind, prev, next, evidence) in changes {
        server.emit(
            "agent.state",
            &pane,
            json!({"pane": pane, "agent": kind, "from": prev.as_str(), "to": next.as_str(), "evidence": evidence}),
        );
    }
    for (pane, text) in toasts {
        if delay == 0 {
            server.toast(&text, "info", Some(&pane));
        } else {
            let s = server.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                s.toast(&text, "info", Some(&pane));
            });
        }
    }
}

/// 모아 둔 pane 출력을 기록한다. full이면 (마스킹될) 텍스트까지, 아니면 바이트 수만.
fn flush_output(server: &Arc<Server>) {
    let full = server
        .ledger
        .lock()
        .ok()
        .and_then(|l| l.as_ref().map(|l| l.level() == kmttt_ledger::Level::Full))
        .unwrap_or(false);
    let acc: Vec<(String, (u64, String))> = match server.st.lock() {
        Ok(mut st) => st.out_acc.drain().collect(),
        Err(_) => return,
    };
    for (pane, (bytes, text)) in acc {
        if full {
            server.emit("pane.output", &pane, json!({"bytes": bytes, "text": text}));
        } else {
            server.emit(
                "pane.output",
                &pane,
                json!({"bytes": bytes, "lines": text.matches('\n').count()}),
            );
        }
    }
}

fn expire_approvals(server: &Arc<Server>) {
    let now = now_ms();
    let expired: Vec<Approval> = match server.st.lock() {
        Ok(mut st) => {
            let mut out = Vec::new();
            st.approvals.retain_mut(|a| {
                if a.info.expires_ms <= now {
                    if let Some(tx) = a.tx.take() {
                        let _ = tx.send(false);
                    }
                    out.push(a.info.clone());
                    false
                } else {
                    true
                }
            });
            out
        }
        Err(_) => return,
    };
    if !expired.is_empty() {
        for a in expired {
            server.emit(
                "approval.expired",
                &a.caller,
                serde_json::to_value(&a).unwrap_or_default(),
            );
        }
        server.push_snapshot(true);
    }
}
