//! 세션 레이아웃 저장·복원. 서버가 다시 뜨면 워크스페이스·탭·pane 배치와 폴더를 되살리고
//! 각 pane에 새 셸을 띄운다(실행 중이던 프로그램은 되살리지 않는다).
//! `experimental.pane_history`가 켜져 있으면 마스킹한 최근 텍스트도 저장해 보여 준다.

use crate::panes::NewPane;
use crate::state::*;
use crate::Server;
use kmttt_core::{layout, paths};
use kmttt_proto::*;
use kmttt_sandbox::Profile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    version: u32,
    focus_ws: String,
    next_ws: u32,
    workspaces: Vec<SavedWs>,
    panes: HashMap<String, SavedPane>,
}

#[derive(Serialize, Deserialize)]
struct SavedWs {
    id: String,
    label: String,
    cwd: String,
    worktree: Option<WorktreeInfo>,
    focus_tab: String,
    next_pane: u32,
    next_tab: u32,
    tabs: Vec<SavedTab>,
}

#[derive(Serialize, Deserialize)]
struct SavedTab {
    id: String,
    label: String,
    layout: LayoutNode,
    focus: String,
}

#[derive(Serialize, Deserialize)]
struct SavedPane {
    cwd: String,
    profile: String,
    alias: Option<String>,
    #[serde(default)]
    history: Vec<String>,
}

fn file(server: &Server) -> std::path::PathBuf {
    paths::sessions_dir().join(format!("{}.json", server.opts.session))
}

pub fn save(server: &Arc<Server>) {
    let history = server
        .cfg
        .read()
        .map(|c| c.experimental.pane_history)
        .unwrap_or(false);
    let saved = {
        let Ok(st) = server.st.lock() else { return };
        Saved {
            version: 1,
            focus_ws: st.focus_ws.clone(),
            next_ws: st.next_ws,
            workspaces: st
                .workspaces
                .iter()
                .map(|w| SavedWs {
                    id: w.id.clone(),
                    label: w.label.clone(),
                    cwd: w.cwd.clone(),
                    worktree: w.worktree.clone(),
                    focus_tab: w.focus_tab.clone(),
                    next_pane: w.next_pane,
                    next_tab: w.next_tab,
                    tabs: w
                        .tabs
                        .iter()
                        .map(|t| SavedTab {
                            id: t.id.clone(),
                            label: t.label.clone(),
                            layout: t.layout.clone(),
                            focus: t.focus.clone(),
                        })
                        .collect(),
                })
                .collect(),
            panes: st
                .panes
                .values()
                .map(|p| {
                    let hist = if history {
                        p.log.recent(200).iter().map(|l| server.redact(l)).collect()
                    } else {
                        Vec::new()
                    };
                    (
                        p.id.clone(),
                        SavedPane {
                            cwd: p.cwd.clone(),
                            profile: p.profile.as_str().into(),
                            alias: p.alias.clone(),
                            history: hist,
                        },
                    )
                })
                .collect(),
        }
    };
    let _ = std::fs::create_dir_all(paths::sessions_dir());
    if let Ok(text) = serde_json::to_string_pretty(&saved) {
        let tmp = file(server).with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file(server));
        }
    }
}

pub fn restore(server: &Arc<Server>) {
    let Ok(text) = std::fs::read_to_string(file(server)) else {
        return;
    };
    let Ok(saved) = serde_json::from_str::<Saved>(&text) else {
        tracing::warn!("ignoring unreadable session file");
        return;
    };
    let Ok(mut st) = server.st.lock() else { return };
    st.next_ws = saved.next_ws;
    st.focus_ws = saved.focus_ws.clone();
    for w in saved.workspaces {
        let mut tabs = Vec::new();
        for t in w.tabs {
            tabs.push(Tab {
                id: t.id,
                label: t.label,
                layout: t.layout,
                focus: t.focus,
                zoomed: false,
            });
        }
        st.workspaces.push(Workspace {
            id: w.id,
            label: w.label,
            cwd: w.cwd,
            worktree: w.worktree,
            tabs,
            focus_tab: w.focus_tab,
            next_pane: w.next_pane,
            next_tab: w.next_tab,
        });
    }
    // 레이아웃에 있는 pane마다 새 셸을 띄운다. 실패한 pane은 레이아웃에서 뺀다.
    let mut failed = Vec::new();
    let placements: Vec<(String, String, String)> = st
        .workspaces
        .iter()
        .flat_map(|w| {
            w.tabs.iter().flat_map(move |t| {
                layout::panes(&t.layout)
                    .into_iter()
                    .map(move |p| (w.id.clone(), t.id.clone(), p))
            })
        })
        .collect();
    for (ws, tab, pid) in placements {
        let sp = saved.panes.get(&pid);
        let np = NewPane {
            cwd: sp
                .map(|s| s.cwd.clone())
                .unwrap_or_else(Server::default_cwd),
            command: None,
            profile: sp
                .and_then(|s| Profile::parse(&s.profile).ok())
                .unwrap_or(Profile::None),
        };
        match server.spawn_pane(&mut st, &ws, &tab, &pid, &np) {
            Ok(()) => {
                if let Some(s) = sp {
                    if let Some(p) = st.panes.get_mut(&pid) {
                        p.alias = s.alias.clone();
                        if !s.history.is_empty() {
                            let mut text = String::from("\x1b[2m── restored (redacted) ──\r\n");
                            for l in &s.history {
                                text.push_str(l);
                                text.push_str("\r\n");
                            }
                            text.push_str("── end ──\x1b[0m\r\n");
                            p.screen.feed(text.as_bytes());
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("restore {pid}: {e}");
                failed.push((tab, pid));
            }
        }
    }
    for (tab, pid) in failed {
        if let Some(t) = st.tab_mut(&tab) {
            if let Some(l) = layout::remove(t.layout.clone(), &pid) {
                t.layout = l;
                if t.focus == pid {
                    t.focus = layout::panes(&t.layout)
                        .first()
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }
    }
    // 살아 있는 pane이 하나도 없는 탭·워크스페이스는 버린다.
    let live: std::collections::HashSet<String> = st.panes.keys().cloned().collect();
    for w in &mut st.workspaces {
        w.tabs
            .retain(|t| layout::panes(&t.layout).iter().all(|p| live.contains(p)));
        if !w.tabs.iter().any(|t| t.id == w.focus_tab) {
            w.focus_tab = w.tabs.first().map(|t| t.id.clone()).unwrap_or_default();
        }
    }
    st.workspaces.retain(|w| !w.tabs.is_empty());
    let focus = st.focus_ws.clone();
    if st.ws(&focus).is_none() {
        st.focus_ws = st
            .workspaces
            .first()
            .map(|w| w.id.clone())
            .unwrap_or_default();
    }
    st.apply_layout();
}
