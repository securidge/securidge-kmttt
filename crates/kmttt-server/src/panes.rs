//! pane 만들기·닫기, PTY 출력 펌프, 워크스페이스·탭 조작.

use crate::state::*;
use crate::Server;
use anyhow::{anyhow, Result};
use kmttt_core::layout;
use kmttt_proto::*;
use kmttt_pty::{PtyEvent, PtyProcess, SpawnSpec};
use kmttt_sandbox::Profile;
use kmttt_term::{Screen, TextLog};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub struct NewPane {
    pub cwd: String,
    pub command: Option<String>,
    pub profile: Profile,
}

impl Server {
    pub fn shell(&self) -> (String, Vec<String>) {
        let cfg = self.cfg.read().map(|c| c.shell.clone()).unwrap_or_default();
        if cfg.program.is_empty() {
            kmttt_pty::default_shell()
        } else {
            (cfg.program, cfg.args)
        }
    }

    pub fn default_cwd() -> String {
        std::env::current_dir()
            .ok()
            .or_else(dirs_home)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into())
    }

    pub fn default_profile(&self, worktree: bool) -> Profile {
        let p = self.policy.lock().map(|p| {
            if worktree {
                p.cfg.sandbox.worktree_profile.clone()
            } else {
                p.cfg.sandbox.default_profile.clone()
            }
        });
        p.ok()
            .and_then(|s| Profile::parse(&s).ok())
            .unwrap_or(Profile::None)
    }

    pub fn sandbox_settings(&self) -> kmttt_sandbox::SandboxSettings {
        let mut s = kmttt_sandbox::SandboxSettings::default();
        if let Ok(p) = self.policy.lock() {
            let sp = &p.cfg.sandbox;
            s.container_image = sp.container_image.clone();
            s.container_network = sp.container_network.clone();
            s.wsl_distro = sp.wsl_distro.clone();
            s.memory_limit_mb = sp.memory_limit_mb;
            s.process_limit = sp.process_limit;
            s.egress_enforce = sp.egress_enforce;
            s.allow_tcp_ports = sp.allow_tcp_ports.clone();
        }
        s
    }

    /// PTY를 띄우고 pane을 상태에 등록한다. `st`는 잠긴 상태로 받는다.
    pub fn spawn_pane(
        self: &Arc<Self>,
        st: &mut State,
        ws: &str,
        tab: &str,
        id: &str,
        np: &NewPane,
    ) -> Result<()> {
        let (shell, shell_args) = self.shell();
        let cwd = if Path::new(&np.cwd).is_dir() {
            np.cwd.clone()
        } else {
            Self::default_cwd()
        };
        let egress = self
            .egress
            .get()
            .map(|e| e.port)
            .filter(|_| np.profile != Profile::None);
        let wrapped = kmttt_sandbox::wrap(
            np.profile,
            &shell,
            &shell_args,
            Path::new(&cwd),
            &self.sandbox_settings(),
            egress,
            &self.opts.kmttt_exe,
        )?;
        let (cols, rows) = (st.viewport.0.max(2), st.viewport.1.max(1));
        let mut env = wrapped.env.clone();
        // 프록시 주소에 창 표식을 넣어, 막힌 접속이 어느 창에서 왔는지 보안 기록에 남긴다.
        // (클라이언트가 Proxy-Authorization으로 보낸다. 프록시는 이 값을 화면 표시에만 쓴다.)
        let tag = kmttt_sandbox::egress::pane_tag(id);
        for (k, v) in env.iter_mut() {
            if k.to_ascii_lowercase().ends_with("_proxy") && v.starts_with("http://127.0.0.1:") {
                *v = v.replacen("http://", &format!("http://{tag}@"), 1);
            }
        }
        env.push(("KMTTT_PANE".into(), id.into()));
        env.push(("KMTTT_SESSION".into(), self.opts.session.clone()));
        // 창 안에서 부르는 kmttt가 같은 세션(서버)에 붙도록 CLI가 읽는 이름으로도 넣는다.
        env.push(("KMTTT_SESSION_NAME".into(), self.opts.session.clone()));
        // PATH에 없어도 창 안의 에이전트가 kmttt를 찾을 수 있게 실행 파일 경로를 알려 준다.
        env.push((
            "KMTTT_EXE".into(),
            self.opts.kmttt_exe.display().to_string(),
        ));
        let spec = SpawnSpec {
            program: wrapped.program.clone(),
            args: wrapped.args.clone(),
            cwd: Some(PathBuf::from(&cwd)),
            env,
            rows,
            cols,
        };
        let (pty, rx) = PtyProcess::spawn(&spec)?;
        // container는 기본 bridge 네트워크라 허용 목록이 강제되지 않는다 → 관제 화면에 알린다.
        // (상태 잠금을 쥐고 있으므로 이벤트는 잠금이 풀린 뒤 따로 보낸다.)
        if np.profile == Profile::Container && self.sandbox_settings().container_network != "none" {
            let server = self.clone();
            let pane_id = id.to_string();
            tokio::spawn(async move {
                server.security_event(
                    kmttt_proto::types::Severity::Notice,
                    "sandbox.egress_not_enforced",
                    "kmttt",
                    &pane_id,
                    "container pane uses a routable network; the egress allowlist is advisory here. \
                     Set sandbox.container_network = \"none\" for full isolation, or use the restricted profile",
                );
            });
        }
        if let Some(cmd) = &np.command {
            // 셸이 입력을 버퍼에 쌓으므로 바로 써도 된다. 명령이 끝나도 pane은 셸로 남는다.
            pty.write(format!("{cmd}\r").as_bytes())?;
        }
        let job = pty
            .pid()
            .and_then(kmttt_pty::reaper::PaneJob::attach)
            .map(Arc::new);
        let pane = Pane {
            job,
            id: id.into(),
            ws: ws.into(),
            tab: tab.into(),
            cwd: cwd.clone(),
            command: np.command.clone().unwrap_or_else(|| shell.clone()),
            pid: pty.pid(),
            pty: Some(pty),
            screen: Screen::new(rows, cols),
            log: TextLog::new(5000),
            exited: None,
            agent: None,
            alias: None,
            last_output: None,
            profile: np.profile,
            cols,
            rows,
            blocked_calls: 0,
            usage: (0.0, 0, 0),
            hook_at: None,
            fg: None,
            fg_chain: Vec::new(),
        };
        st.panes.insert(id.into(), pane);
        let s = self.clone();
        let pid = id.to_string();
        tokio::spawn(async move { s.pump(pid, rx).await });
        Ok(())
    }

    /// PTY 출력 → 화면 버퍼·텍스트 기록·붙은 클라이언트. 기록용 텍스트는 모아 두었다가 ticker가 내보낸다.
    async fn pump(
        self: Arc<Self>,
        pane: String,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<PtyEvent>,
    ) {
        while let Some(ev) = rx.recv().await {
            match ev {
                PtyEvent::Output(bytes) => {
                    let text = kmttt_redact::strip_ansi(&bytes);
                    let Ok(mut st) = self.st.lock() else { break };
                    let Some(p) = st.panes.get_mut(&pane) else {
                        break;
                    };
                    // 프로그램이 터미널에 묻는 질의(커서 위치 등)에 답한다. Windows ConPTY는 답이
                    // 올 때까지 출력을 멈추므로 꼭 필요하다.
                    let reply = p.screen.feed_and_reply(&bytes);
                    if !reply.is_empty() {
                        if let Some(pty) = &p.pty {
                            let _ = pty.write(&reply);
                        }
                    }
                    p.log.push_text(&text);
                    p.last_output = Some(Instant::now());
                    let acc = st.out_acc.entry(pane.clone()).or_default();
                    acc.0 += bytes.len() as u64;
                    if acc.1.len() < 64 * 1024 {
                        acc.1.push_str(&text);
                    }
                    let note = RpcMessage::notification(
                        "pane.output",
                        json!({"pane": pane, "data": b64_encode(&bytes)}),
                    );
                    for c in st.clients.values().filter(|c| c.attached) {
                        let _ = c.tx.send(note.clone());
                    }
                }
                PtyEvent::Exited(code) => {
                    if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    self.emit("pane.exited", &pane, json!({"code": code}));
                    // 셸이 끝나면 pane을 닫는다(tmux와 같은 동작).
                    self.close_pane(&pane, false);
                    break;
                }
            }
        }
    }

    /// 새 워크스페이스(탭 하나, pane 하나).
    pub fn create_workspace(
        self: &Arc<Self>,
        label: Option<String>,
        cwd: Option<String>,
        command: Option<String>,
        worktree: Option<WorktreeInfo>,
        focus: bool,
        profile: Option<Profile>,
    ) -> Result<(String, String, String)> {
        let profile = profile.unwrap_or_else(|| self.default_profile(worktree.is_some()));
        let (wid, tid, pid) = {
            let mut st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            st.next_ws += 1;
            let wid = format!("w{}", st.next_ws);
            let tid = format!("{wid}:t1");
            let pid = format!("{wid}:p1");
            let cwd = cwd.unwrap_or_else(Self::default_cwd);
            let label = label.unwrap_or_else(|| {
                worktree
                    .as_ref()
                    .map(|w| w.branch.clone())
                    .unwrap_or_else(|| {
                        Path::new(&cwd)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| wid.clone())
                    })
            });
            st.workspaces.push(Workspace {
                id: wid.clone(),
                label,
                cwd: cwd.clone(),
                worktree,
                tabs: vec![Tab {
                    id: tid.clone(),
                    label: "main".into(),
                    layout: LayoutNode::Leaf { pane: pid.clone() },
                    focus: pid.clone(),
                    zoomed: false,
                }],
                focus_tab: tid.clone(),
                next_pane: 1,
                next_tab: 1,
            });
            if focus || st.focus_ws.is_empty() {
                st.focus_ws = wid.clone();
            }
            if let Err(e) = self.spawn_pane(
                &mut st,
                &wid,
                &tid,
                &pid,
                &NewPane {
                    cwd,
                    command,
                    profile,
                },
            ) {
                st.workspaces.retain(|w| w.id != wid);
                if st.focus_ws == wid {
                    st.focus_ws = st
                        .workspaces
                        .first()
                        .map(|w| w.id.clone())
                        .unwrap_or_default();
                }
                return Err(e);
            }
            st.apply_layout();
            (wid, tid, pid)
        };
        self.after_layout_change();
        self.emit(
            "workspace.created",
            "server",
            json!({"workspace": wid, "tab": tid, "pane": pid}),
        );
        Ok((wid, tid, pid))
    }

    pub fn create_tab(
        self: &Arc<Self>,
        ws: &str,
        label: Option<String>,
        cwd: Option<String>,
        focus: bool,
    ) -> Result<(String, String)> {
        let profile;
        let (tid, pid) = {
            let mut st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            let w = st.ws_mut(ws).ok_or_else(|| anyhow!("no workspace {ws}"))?;
            w.next_tab += 1;
            w.next_pane += 1;
            let tid = format!("{ws}:t{}", w.next_tab);
            let pid = format!("{ws}:p{}", w.next_pane);
            let cwd = cwd.unwrap_or_else(|| w.cwd.clone());
            let is_wt = w.worktree.is_some();
            w.tabs.push(Tab {
                id: tid.clone(),
                label: label.unwrap_or_else(|| format!("tab {}", w.tabs.len() + 1)),
                layout: LayoutNode::Leaf { pane: pid.clone() },
                focus: pid.clone(),
                zoomed: false,
            });
            if focus {
                w.focus_tab = tid.clone();
                st.focus_ws = ws.into();
            }
            drop(st);
            profile = self.default_profile(is_wt);
            let mut st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            if let Err(e) = self.spawn_pane(
                &mut st,
                ws,
                &tid,
                &pid,
                &NewPane {
                    cwd,
                    command: None,
                    profile,
                },
            ) {
                if let Some(w) = st.ws_mut(ws) {
                    w.tabs.retain(|t| t.id != tid);
                    w.focus_tab = w.tabs.first().map(|t| t.id.clone()).unwrap_or_default();
                }
                return Err(e);
            }
            st.apply_layout();
            (tid, pid)
        };
        self.after_layout_change();
        Ok((tid, pid))
    }

    /// 대상 pane을 나눠 새 pane을 만든다.
    pub fn split_pane(
        self: &Arc<Self>,
        target: &str,
        dir: SplitDir,
        cwd: Option<String>,
        command: Option<String>,
        profile: Option<Profile>,
        focus: bool,
    ) -> Result<String> {
        let new_id = {
            let mut st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            let (ws, tab, tcwd) = {
                let p = st
                    .panes
                    .get(target)
                    .ok_or_else(|| anyhow!("no pane {target}"))?;
                (p.ws.clone(), p.tab.clone(), p.cwd.clone())
            };
            let is_wt = st.ws(&ws).map(|w| w.worktree.is_some()).unwrap_or(false);
            let w = st.ws_mut(&ws).ok_or_else(|| anyhow!("no workspace"))?;
            w.next_pane += 1;
            let new_id = format!("{ws}:p{}", w.next_pane);
            let t = st.tab_mut(&tab).ok_or_else(|| anyhow!("no tab"))?;
            if !layout::split(&mut t.layout, target, &new_id, dir) {
                return Err(anyhow!("pane {target} not in its tab layout"));
            }
            t.zoomed = false;
            if focus {
                t.focus = new_id.clone();
            }
            drop(st);
            let profile = profile.unwrap_or_else(|| self.default_profile(is_wt));
            let mut st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            let np = NewPane {
                cwd: cwd.unwrap_or(tcwd),
                command,
                profile,
            };
            if let Err(e) = self.spawn_pane(&mut st, &ws, &tab, &new_id, &np) {
                if let Some(t) = st.tab_mut(&tab) {
                    if let Some(l) = layout::remove(t.layout.clone(), &new_id) {
                        t.layout = l;
                    }
                    t.focus = target.into();
                }
                return Err(e);
            }
            st.apply_layout();
            new_id
        };
        self.after_layout_change();
        self.emit(
            "pane.created",
            "server",
            json!({"pane": new_id, "from": target}),
        );
        Ok(new_id)
    }

    /// pane을 닫는다. 탭·워크스페이스가 비면 함께 닫는다.
    pub fn close_pane(self: &Arc<Self>, id: &str, kill: bool) {
        let mut reopen = false;
        {
            let Ok(mut st) = self.st.lock() else { return };
            let Some(mut p) = st.panes.remove(id) else {
                return;
            };
            if kill {
                if let Some(pty) = p.pty.as_mut() {
                    pty.kill();
                }
            }
            st.out_acc.remove(id);
            // 이 창이 주고받던 "세션 동안 허용"은 창과 함께 끝난다.
            st.session_grants
                .retain(|g| g.caller_pane != id && g.target != id);
            let (ws, tab) = (p.ws.clone(), p.tab.clone());
            if let Some(t) = st.tab_mut(&tab) {
                match layout::remove(t.layout.clone(), id) {
                    Some(l) => {
                        t.layout = l;
                        if t.focus == id {
                            t.focus = layout::panes(&t.layout)
                                .first()
                                .cloned()
                                .unwrap_or_default();
                        }
                        t.zoomed = false;
                    }
                    None => {
                        if let Some(w) = st.ws_mut(&ws) {
                            w.tabs.retain(|t| t.id != tab);
                            if w.focus_tab == tab {
                                w.focus_tab =
                                    w.tabs.last().map(|t| t.id.clone()).unwrap_or_default();
                            }
                        }
                    }
                }
            }
            if st.ws(&ws).is_some_and(|w| w.tabs.is_empty()) {
                st.workspaces.retain(|w| w.id != ws);
                if st.focus_ws == ws {
                    st.focus_ws = st
                        .workspaces
                        .last()
                        .map(|w| w.id.clone())
                        .unwrap_or_default();
                }
            }
            // 붙은 클라이언트가 있는데 워크스페이스가 다 사라지면 새로 하나 연다.
            if st.workspaces.is_empty() && st.any_attached() {
                reopen = true;
            }
            st.apply_layout();
        }
        self.emit("pane.closed", "server", json!({"pane": id}));
        if reopen {
            let _ = self.create_workspace(None, None, None, None, true, None);
        }
        self.after_layout_change();
    }

    pub fn close_workspace(self: &Arc<Self>, ws: &str) -> Result<()> {
        let ids: Vec<String> = {
            let st = self.st.lock().map_err(|_| anyhow!("lock"))?;
            st.panes
                .values()
                .filter(|p| p.ws == ws)
                .map(|p| p.id.clone())
                .collect()
        };
        anyhow::ensure!(!ids.is_empty(), "no workspace {ws}");
        for id in ids {
            self.close_pane(&id, true);
        }
        Ok(())
    }

    /// 레이아웃 변경 뒤 공통 처리: 저장 + 스냅숏 전송.
    pub fn after_layout_change(self: &Arc<Self>) {
        if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        crate::persist::save(self);
        self.push_snapshot(false);
    }

    /// pane에 입력을 쓴다.
    pub fn write_pane(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let st = self.st.lock().map_err(|_| anyhow!("lock"))?;
        let p = st.panes.get(id).ok_or_else(|| anyhow!("no pane {id}"))?;
        let pty = p
            .pty
            .as_ref()
            .ok_or_else(|| anyhow!("pane {id} has exited"))?;
        pty.write(bytes)
    }

    /// 새로 붙은 클라이언트가 모든 pane 화면을 재현하도록 초기 바이트를 보낸다.
    pub fn send_bootstrap(&self, client: u64) {
        let Ok(st) = self.st.lock() else { return };
        let Some(c) = st.clients.get(&client) else {
            return;
        };
        for p in st.panes.values() {
            let data = p.screen.bootstrap_bytes();
            let _ = c.tx.send(RpcMessage::notification(
                "pane.bootstrap",
                json!({"pane": p.id, "cols": p.cols, "rows": p.rows, "data": b64_encode(&data)}),
            ));
        }
    }
}

impl Server {
    /// 붙어 있는(attach한) 클라이언트에게만 창 하나의 화면을 다시 보낸다.
    pub fn send_bootstrap_pane(&self, client: u64, pane: &str) {
        let Ok(st) = self.st.lock() else { return };
        let Some(c) = st.clients.get(&client) else {
            return;
        };
        if !c.attached {
            return;
        }
        if let Some(p) = st.panes.get(pane) {
            let data = p.screen.bootstrap_bytes();
            let _ = c.tx.send(RpcMessage::notification(
                "pane.bootstrap",
                json!({"pane": p.id, "cols": p.cols, "rows": p.rows, "data": b64_encode(&data)}),
            ));
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
