//! 연결 하나를 맡아 JSON-RPC 요청을 처리한다. 모든 요청은 `authorize`(F1)를 거친다.

use crate::state::{AgentRt, ClientRt};
use crate::Server;
use kmttt_core::layout;
use kmttt_policy::{Caller, Decision};
use kmttt_proto::*;
use kmttt_sandbox::Profile;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

pub struct Conn {
    pub id: u64,
    pub caller: Caller,
    /// 상대 프로세스 PID(승인 결정을 누가 했는지 기록용)
    pub pid: Option<u32>,
}

fn s(p: &Value, k: &str) -> Option<String> {
    p.get(k)
        .and_then(|v| v.as_str())
        .map(String::from)
        .filter(|v| !v.is_empty())
}
fn b(p: &Value, k: &str, d: bool) -> bool {
    p.get(k).and_then(|v| v.as_bool()).unwrap_or(d)
}
fn u(p: &Value, k: &str, d: u64) -> u64 {
    p.get(k).and_then(|v| v.as_u64()).unwrap_or(d)
}
fn need(p: &Value, k: &str) -> Result<String, RpcError> {
    s(p, k).ok_or_else(|| RpcError::invalid_params(format!("missing '{k}'")))
}
fn err(e: anyhow::Error) -> RpcError {
    RpcError::internal(e.to_string())
}
fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

pub async fn serve_conn<R, W>(server: Arc<Server>, mut rd: R, mut wr: W, pid: Option<u32>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let s2 = server.clone();
    let caller = tokio::task::spawn_blocking(move || s2.identify(pid))
        .await
        .unwrap_or(Caller::User);
    let (tx, mut rx) = mpsc::unbounded_channel::<RpcMessage>();
    let id = match server.st.lock() {
        Ok(mut st) => {
            st.next_client += 1;
            let id = st.next_client;
            st.clients.insert(
                id,
                ClientRt {
                    tx: tx.clone(),
                    attached: false,
                    subs: None,
                    label: caller.label(),
                },
            );
            id
        }
        Err(_) => return,
    };
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if write_frame(&mut wr, &m.to_bytes()).await.is_err() {
                break;
            }
        }
    });
    let conn = Arc::new(Conn { id, caller, pid });
    while let Ok(Some(buf)) = read_frame(&mut rd).await {
        let msg = match RpcMessage::from_bytes(&buf) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(RpcMessage::response(0, Err(e)));
                continue;
            }
        };
        let method = msg.method.clone().unwrap_or_default();
        let params = msg.params.clone().unwrap_or(Value::Null);
        if msg.is_notification() {
            // 입력은 응답 없이 빠르게. 사람(pane 밖) 클라이언트만 허용.
            if method == "client.input" && conn.caller == Caller::User {
                if let (Some(pane), Some(data)) = (s(&params, "pane"), s(&params, "data")) {
                    if let Ok(bytes) = b64_decode(&data) {
                        let _ = server.write_pane(&pane, &bytes);
                    }
                }
            }
            continue;
        }
        let Some(req_id) = msg.id else { continue };
        let server = server.clone();
        let conn = conn.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let r = call(&server, &conn, &method, &params).await;
            let _ = tx.send(RpcMessage::response(req_id, r));
        });
    }
    let was_attached = server
        .st
        .lock()
        .ok()
        .and_then(|mut st| st.clients.remove(&id))
        .map(|c| c.attached)
        .unwrap_or(false);
    if was_attached {
        server.emit("client.detach", &conn.caller.key(), json!({"client": id}));
    }
}

/// 요청 하나 처리.
pub async fn call(
    server: &Arc<Server>,
    conn: &Conn,
    method: &str,
    p: &Value,
) -> Result<Value, RpcError> {
    // 대상 pane이 있는 메서드는 정책 판정 전에 대상을 확정한다.
    let target_key = match method {
        "pane.run" | "pane.send" | "pane.read" | "pane.wait_output" | "pane.process_info"
        | "pane.focus" | "pane.zoom" | "pane.close" => s(p, "pane"),
        "pane.split" => s(p, "target"),
        "agent.prompt" | "agent.read" | "agent.wait" | "agent.explain" => s(p, "target"),
        "agent.start" => s(p, "pane"),
        _ => None,
    };
    let needs_pane = target_key.is_some()
        || matches!(
            method,
            "pane.split"
                | "pane.run"
                | "pane.send"
                | "pane.read"
                | "pane.close"
                | "pane.zoom"
                | "pane.focus"
                | "pane.process_info"
                | "pane.wait_output"
        );
    let pane = if needs_pane {
        let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
        Some(st.resolve_pane(target_key.as_deref()).ok_or_else(|| {
            RpcError::not_found(format!(
                "no pane or agent '{}'",
                target_key.clone().unwrap_or_else(|| "focused".into())
            ))
        })?)
    } else {
        None
    };
    let mut target = server.target_of(pane.as_deref());
    if target.workspace.is_none() {
        target.workspace = s(p, "workspace").or_else(|| s(p, "id").filter(|i| !i.contains(':')));
    }
    let detail = match method {
        "agent.prompt" => s(p, "text").unwrap_or_default(),
        "pane.run" => s(p, "command").unwrap_or_default(),
        "pane.send" => s(p, "text").unwrap_or_default(),
        "agent.start" => format!(
            "start {} as {}",
            s(p, "kind").unwrap_or_default(),
            s(p, "name").unwrap_or_default()
        ),
        _ => String::new(),
    };
    // 에이전트가 보내는 문장에 인젝션 의심 패턴이 있으면 경보(차단은 정책이 결정).
    if conn.caller != Caller::User && !detail.is_empty() {
        let hits = kmttt_policy::detect_injection(&detail);
        if !hits.is_empty() {
            server.security_event(
                Severity::Notice,
                "injection.suspect",
                &conn.caller.label(),
                pane.as_deref().unwrap_or(""),
                &format!("{method}: {}", hits.join(", ")),
            );
        }
    }
    server
        .authorize(&conn.caller, method, &target, &detail)
        .await?;
    let pane_id = || {
        pane.clone()
            .ok_or_else(|| RpcError::invalid_params("missing pane"))
    };

    match method {
        "schema" => Ok(kmttt_proto::schema::openrpc()),
        "server.status" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            Ok(to_json(&ServerStatus {
                version: kmttt_core::VERSION.into(),
                session: server.opts.session.clone(),
                pid: std::process::id(),
                uptime_secs: server.started.elapsed().as_secs(),
                panes: st.panes.len(),
                clients: st.clients.len(),
                protocol: PROTOCOL_VERSION,
            }))
        }
        "server.stop" => {
            let s2 = server.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                s2.shutdown.notify_waiters();
                s2.shutdown.notify_one();
            });
            Ok(json!({"stopping": true}))
        }
        "server.reload_config" => server.reload_config().map_err(err),
        "session.snapshot" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            Ok(to_json(&st.snapshot(&server.opts.session, &red)))
        }
        "client.attach" => {
            let cols = u(p, "cols", 120) as u16;
            let rows = u(p, "rows", 40) as u16;
            let empty = {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                if let Some(c) = st.clients.get_mut(&conn.id) {
                    c.attached = true;
                }
                st.viewport = (cols.max(10), rows.max(3));
                st.header_rows = u(p, "header_rows", 0).min(1) as u16;
                st.apply_layout();
                st.workspaces.is_empty()
            };
            if empty {
                server
                    .create_workspace(None, s(p, "cwd"), None, None, true, None)
                    .map_err(err)?;
            }
            server.send_bootstrap(conn.id);
            server.emit(
                "client.attach",
                "user",
                json!({"client": conn.id, "cols": cols, "rows": rows}),
            );
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            let snap = st.snapshot(&server.opts.session, &red);
            Ok(to_json(&snap))
        }
        "client.viewport" => {
            let cols = u(p, "cols", 120) as u16;
            let rows = u(p, "rows", 40) as u16;
            let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            st.viewport = (cols.max(10), rows.max(3));
            st.header_rows = u(p, "header_rows", 0).min(1) as u16;
            st.apply_layout();
            Ok(json!({"ok": true}))
        }
        "pane.redraw" => {
            // 붙어 있는 화면 클라이언트가 창 하나를 서버가 가진 화면으로 다시 그리게 한다
            // (Windows ConPTY가 크기 변경 때 지운 화면을 되살리는 용도).
            let pane = s(p, "pane").ok_or_else(|| RpcError::invalid_params("pane is required"))?;
            server.send_bootstrap_pane(conn.id, &pane);
            Ok(json!({"ok": true}))
        }
        "workspace.list" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            Ok(to_json(&st.snapshot(&server.opts.session, &red).workspaces))
        }
        "workspace.create" => {
            let profile = s(p, "profile")
                .map(|x| Profile::parse(&x))
                .transpose()
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            let (w, t, pn) = server
                .create_workspace(
                    s(p, "label"),
                    s(p, "cwd"),
                    s(p, "command"),
                    None,
                    b(p, "focus", true),
                    profile,
                )
                .map_err(err)?;
            Ok(json!({"workspace": w, "tab": t, "pane": pn}))
        }
        "workspace.rename" => {
            let id = need(p, "id")?;
            let label = need(p, "label")?;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                st.ws_mut(&id)
                    .ok_or_else(|| RpcError::not_found(format!("no workspace {id}")))?
                    .label = label;
            }
            server.after_layout_change();
            Ok(json!({"ok": true}))
        }
        "workspace.focus" => {
            let id = need(p, "id")?;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                if st.ws(&id).is_none() {
                    return Err(RpcError::not_found(format!("no workspace {id}")));
                }
                st.focus_ws = id;
            }
            server.push_snapshot(false);
            Ok(json!({"ok": true}))
        }
        "workspace.close" => {
            server.close_workspace(&need(p, "id")?).map_err(err)?;
            Ok(json!({"ok": true}))
        }
        "tab.list" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            let snap = st.snapshot(&server.opts.session, &red);
            let ws = s(p, "workspace");
            let tabs: Vec<TabInfo> = snap
                .workspaces
                .into_iter()
                .filter(|w| ws.as_ref().is_none_or(|x| x == &w.id))
                .flat_map(|w| w.tabs)
                .collect();
            Ok(to_json(&tabs))
        }
        "tab.create" => {
            let ws = match s(p, "workspace") {
                Some(w) => w,
                None => server
                    .st
                    .lock()
                    .map(|st| st.focus_ws.clone())
                    .map_err(|_| RpcError::internal("lock"))?,
            };
            let (t, pn) = server
                .create_tab(&ws, s(p, "label"), s(p, "cwd"), b(p, "focus", true))
                .map_err(err)?;
            Ok(json!({"tab": t, "pane": pn}))
        }
        "tab.rename" => {
            let id = need(p, "id")?;
            let label = need(p, "label")?;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                st.tab_mut(&id)
                    .ok_or_else(|| RpcError::not_found(format!("no tab {id}")))?
                    .label = label;
            }
            server.after_layout_change();
            Ok(json!({"ok": true}))
        }
        "tab.focus" => {
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let target_tab = if let Some(id) = s(p, "id") {
                    id
                } else {
                    let off = p.get("offset").and_then(|v| v.as_i64()).unwrap_or(1);
                    let ws = st
                        .ws(&st.focus_ws)
                        .ok_or_else(|| RpcError::not_found("no workspace"))?;
                    let i = ws
                        .tabs
                        .iter()
                        .position(|t| t.id == ws.focus_tab)
                        .unwrap_or(0) as i64;
                    let n = ws.tabs.len() as i64;
                    ws.tabs[((i + off).rem_euclid(n)) as usize].id.clone()
                };
                let ws_id = target_tab.split(':').next().unwrap_or_default().to_string();
                let w = st
                    .ws_mut(&ws_id)
                    .ok_or_else(|| RpcError::not_found("no workspace"))?;
                if !w.tabs.iter().any(|t| t.id == target_tab) {
                    return Err(RpcError::not_found(format!("no tab {target_tab}")));
                }
                w.focus_tab = target_tab;
                st.focus_ws = ws_id;
            }
            server.push_snapshot(false);
            Ok(json!({"ok": true}))
        }
        "tab.close" => {
            let id = need(p, "id")?;
            let panes: Vec<String> = {
                let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                st.tab(&id)
                    .map(|t| layout::panes(&t.layout))
                    .ok_or_else(|| RpcError::not_found(format!("no tab {id}")))?
            };
            for pn in panes {
                server.close_pane(&pn, true);
            }
            Ok(json!({"ok": true}))
        }
        "pane.list" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            Ok(to_json(&st.snapshot(&server.opts.session, &red).panes))
        }
        "pane.split" => {
            let dir = match s(p, "direction").as_deref() {
                None | Some("right") | Some("r") => SplitDir::Right,
                Some("down") | Some("d") => SplitDir::Down,
                Some(x) => {
                    return Err(RpcError::invalid_params(format!(
                        "direction must be right or down, got {x}"
                    )))
                }
            };
            let profile = s(p, "profile")
                .map(|x| Profile::parse(&x))
                .transpose()
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            let id = server
                .split_pane(
                    &pane_id()?,
                    dir,
                    s(p, "cwd"),
                    s(p, "command"),
                    profile,
                    b(p, "focus", true),
                )
                .map_err(err)?;
            Ok(json!({"pane": id}))
        }
        "pane.run" => {
            let pn = pane_id()?;
            server
                .write_pane(&pn, format!("{}\r", need(p, "command")?).as_bytes())
                .map_err(err)?;
            Ok(json!({"ok": true}))
        }
        "pane.send" => {
            let pn = pane_id()?;
            let mut text = need(p, "text")?;
            if b(p, "enter", false) {
                text.push('\r');
            }
            server.write_pane(&pn, text.as_bytes()).map_err(err)?;
            Ok(json!({"ok": true}))
        }
        "pane.read" | "agent.read" => {
            let pn = pane_id()?;
            let lines = u(p, "lines", 50).min(5000) as usize;
            let source = s(p, "source").unwrap_or_else(|| "recent".into());
            let text = {
                let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let pane = st
                    .panes
                    .get(&pn)
                    .ok_or_else(|| RpcError::not_found(format!("no pane {pn}")))?;
                match source.as_str() {
                    "screen" | "visible" => pane.screen.visible_lines().join("\n"),
                    _ => pane.log.recent(lines).join("\n"),
                }
            };
            // 읽기 결과는 언제나 마스킹한다(F3).
            Ok(json!({"pane": pn, "source": source, "text": server.redact(&text)}))
        }
        "pane.wait_output" => {
            let pn = pane_id()?;
            let re = regex::Regex::new(&need(p, "regex")?)
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            let deadline = Instant::now() + Duration::from_millis(u(p, "timeout_ms", 30_000));
            loop {
                let found = {
                    let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                    let pane = st
                        .panes
                        .get(&pn)
                        .ok_or_else(|| RpcError::not_found(format!("no pane {pn}")))?;
                    let mut lines = pane.log.recent(300);
                    lines.extend(pane.screen.visible_lines());
                    lines.into_iter().find(|l| re.is_match(l))
                };
                if let Some(l) = found {
                    return Ok(json!({"pane": pn, "line": server.redact(&l)}));
                }
                if Instant::now() >= deadline {
                    return Err(RpcError::new(
                        codes::TIMEOUT,
                        "pattern did not appear in time",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        "pane.process_info" => {
            let pn = pane_id()?;
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let pane = st
                .panes
                .get(&pn)
                .ok_or_else(|| RpcError::not_found(format!("no pane {pn}")))?;
            Ok(json!({
                "pane": pn,
                "root_pid": pane.pid,
                "foreground": pane.fg.as_ref().map(|(pid, name, cmd)| json!({"pid": pid, "name": name, "cmdline": server.redact(cmd)})),
                "chain": pane.fg_chain,
                "sandbox": pane.profile.as_str(),
            }))
        }
        "pane.focus" => {
            let pn = pane_id()?;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let (ws, tab) = st
                    .panes
                    .get(&pn)
                    .map(|x| (x.ws.clone(), x.tab.clone()))
                    .ok_or_else(|| RpcError::not_found("no pane"))?;
                if let Some(t) = st.tab_mut(&tab) {
                    t.focus = pn.clone();
                }
                if let Some(w) = st.ws_mut(&ws) {
                    w.focus_tab = tab;
                }
                st.focus_ws = ws;
            }
            server.push_snapshot(false);
            Ok(json!({"ok": true}))
        }
        "layout.set_ratio" => {
            let path: Vec<bool> = p
                .get("path")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_bool()).collect())
                .unwrap_or_default();
            let ratio = p
                .get("ratio")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| RpcError::invalid_params("missing ratio"))?
                as f32;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let tab = match s(p, "tab") {
                    Some(t) => t,
                    None => st
                        .focused_tab()
                        .map(|t| t.id.clone())
                        .ok_or_else(|| RpcError::not_found("no tab"))?,
                };
                let t = st
                    .tab_mut(&tab)
                    .ok_or_else(|| RpcError::not_found(format!("no tab {tab}")))?;
                if !kmttt_core::layout::set_ratio(&mut t.layout, &path, ratio) {
                    return Err(RpcError::invalid_params("path does not point at a split"));
                }
                st.apply_layout();
            }
            server.after_layout_change();
            Ok(json!({"ok": true}))
        }
        "layout.swap" | "layout.move" => {
            let (x, y) = if method == "layout.swap" {
                (need(p, "a")?, need(p, "b")?)
            } else {
                (need(p, "pane")?, need(p, "target")?)
            };
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let tab_of = |id: &str| st.panes.get(id).map(|x| x.tab.clone());
                let (Some(tx), Some(ty)) = (tab_of(&x), tab_of(&y)) else {
                    return Err(RpcError::not_found("no pane"));
                };
                if tx != ty {
                    return Err(RpcError::invalid_params("panes are in different tabs"));
                }
                let t = st
                    .tab_mut(&tx)
                    .ok_or_else(|| RpcError::not_found("no tab"))?;
                let ok = if method == "layout.swap" {
                    kmttt_core::layout::swap(&mut t.layout, &x, &y)
                } else {
                    let side = kmttt_core::layout::Side::parse(&need(p, "side")?)
                        .ok_or_else(|| RpcError::invalid_params("side: left|right|up|down"))?;
                    kmttt_core::layout::move_to(&mut t.layout, &x, &y, side)
                };
                if !ok {
                    return Err(RpcError::invalid_params("cannot move that pane there"));
                }
                t.zoomed = false;
                t.focus = x.clone();
                st.apply_layout();
            }
            server.after_layout_change();
            Ok(json!({"ok": true}))
        }
        "layout.equalize" => {
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let tab = match s(p, "tab") {
                    Some(t) => t,
                    None => st
                        .focused_tab()
                        .map(|t| t.id.clone())
                        .ok_or_else(|| RpcError::not_found("no tab"))?,
                };
                let t = st
                    .tab_mut(&tab)
                    .ok_or_else(|| RpcError::not_found(format!("no tab {tab}")))?;
                kmttt_core::layout::equalize(&mut t.layout);
                t.zoomed = false;
                st.apply_layout();
            }
            server.after_layout_change();
            Ok(json!({"ok": true}))
        }
        "pane.zoom" => {
            let pn = pane_id()?;
            {
                let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
                let tab = st
                    .panes
                    .get(&pn)
                    .map(|x| x.tab.clone())
                    .ok_or_else(|| RpcError::not_found("no pane"))?;
                if let Some(t) = st.tab_mut(&tab) {
                    t.zoomed = !(t.zoomed && t.focus == pn);
                    t.focus = pn.clone();
                }
                st.apply_layout();
            }
            server.push_snapshot(false);
            Ok(json!({"ok": true}))
        }
        "pane.close" => {
            server.close_pane(&pane_id()?, true);
            Ok(json!({"ok": true}))
        }
        "agent.list" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let red = |x: &str| server.redact(x);
            let agents: Vec<PaneInfo> = st
                .snapshot(&server.opts.session, &red)
                .panes
                .into_iter()
                .filter(|p| p.agent.is_some())
                .collect();
            Ok(to_json(&agents))
        }
        "agent.start" => agent_start(server, &pane_id()?, p).await,
        "agent.prompt" => agent_prompt(server, &pane_id()?, p).await,
        "agent.wait" => {
            let pn = pane_id()?;
            let until: Vec<AgentState> = s(p, "until")
                .unwrap_or_else(|| "blocked,idle,done".into())
                .split(',')
                .filter_map(|x| AgentState::parse(x.trim()))
                .collect();
            let st = wait_state(
                server,
                &pn,
                &until,
                Duration::from_millis(u(p, "timeout_ms", 600_000)),
            )
            .await?;
            Ok(json!({"pane": pn, "state": st.as_str()}))
        }
        "agent.explain" => {
            let pn = pane_id()?;
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let pane = st
                .panes
                .get(&pn)
                .ok_or_else(|| RpcError::not_found("no pane"))?;
            let det = server
                .detector
                .read()
                .map_err(|_| RpcError::internal("lock"))?;
            let manifest = pane.agent.as_ref().and_then(|a| det.manifest(&a.kind)).map(|m| json!({"kind": m.kind, "display": m.display, "source": m.source, "hook": m.hook}));
            Ok(json!({
                "pane": pn,
                "foreground": pane.fg.as_ref().map(|(pid, name, _)| json!({"pid": pid, "name": name})),
                "chain": pane.fg_chain,
                "agent": pane.agent.as_ref().map(|a| json!({"kind": a.kind, "state": a.state.as_str(), "evidence": a.evidence, "hooked": a.hooked})),
                "manifest": manifest,
                "screen_tail": pane.screen.tail_lines(8).iter().map(|l| server.redact(l)).collect::<Vec<_>>(),
            }))
        }
        "worktree.create" => {
            let cwd = s(p, "cwd").unwrap_or_else(Server::default_cwd);
            let branch = need(p, "branch")?;
            let root = server
                .cfg
                .read()
                .map(|c| c.worktree_root())
                .map_err(|_| RpcError::internal("lock"))?;
            let base = s(p, "base");
            let path = s(p, "path");
            let info = tokio::task::spawn_blocking(move || {
                crate::worktree::create(
                    std::path::Path::new(&cwd),
                    &branch,
                    base.as_deref(),
                    path.as_deref().map(std::path::Path::new),
                    &root,
                )
            })
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(err)?;
            let (w, t, pn) = server
                .create_workspace(
                    s(p, "label"),
                    Some(info.path.clone()),
                    None,
                    Some(info.clone()),
                    b(p, "focus", true),
                    None,
                )
                .map_err(err)?;
            server.emit(
                "worktree.created",
                &conn.caller.key(),
                json!({"workspace": w, "worktree": info}),
            );
            Ok(json!({"workspace": w, "tab": t, "pane": pn, "worktree": info}))
        }
        "worktree.remove" => {
            let ws = need(p, "workspace")?;
            let info = server
                .st
                .lock()
                .map_err(|_| RpcError::internal("lock"))?
                .ws(&ws)
                .and_then(|w| w.worktree.clone())
                .ok_or_else(|| RpcError::not_found(format!("{ws} is not a worktree workspace")))?;
            let force = b(p, "force", false);
            let i2 = info.clone();
            tokio::task::spawn_blocking(move || crate::worktree::remove(&i2, force))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
                .map_err(err)?;
            server.close_workspace(&ws).map_err(err)?;
            server.emit(
                "worktree.removed",
                &conn.caller.key(),
                json!({"workspace": ws, "worktree": info}),
            );
            Ok(json!({"ok": true}))
        }
        "events.subscribe" => {
            let kinds: Vec<String> = p
                .get("kinds")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_else(|| vec!["*".into()]);
            let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            if let Some(c) = st.clients.get_mut(&conn.id) {
                c.subs = Some(kinds.clone());
            }
            Ok(json!({"subscribed": kinds}))
        }
        "approval.list" => {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            Ok(to_json(
                &st.approvals
                    .iter()
                    .map(|a| a.info.clone())
                    .collect::<Vec<_>>(),
            ))
        }
        "approval.decide" => {
            let id = p
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| RpcError::invalid_params("missing id"))?;
            let by = server.caller_lineage(conn.pid);
            server.decide(id, b(p, "allow", false), by)?;
            Ok(json!({"ok": true}))
        }
        "control.snapshot" => Ok(to_json(&server.control_snapshot())),
        "audit.status" => {
            if b(p, "verify", false) {
                crate::authz::refresh_integrity(server, false);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Ok(to_json(&server.integrity_now()))
        }
        "policy.show" => {
            let pol = server
                .policy
                .lock()
                .map_err(|_| RpcError::internal("lock"))?;
            Ok(to_json(&pol.cfg))
        }
        "hook.event" => hook_event(server, conn, p).await,
        "plugin.list" => {
            let ps = server
                .plugins
                .read()
                .map_err(|_| RpcError::internal("lock"))?;
            Ok(json!(ps
                .iter()
                .map(|x| json!({"name": x.manifest.name, "version": x.manifest.version, "events": x.manifest.events, "capabilities": x.capabilities, "dir": x.dir}))
                .collect::<Vec<_>>()))
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other}"),
        )),
    }
}

/// Windows에서 npm으로 설치한 에이전트(codex, gemini 등)는 PATH에 `codex.ps1`과 `codex.cmd`가
/// 함께 있다. PowerShell은 `.ps1`을 먼저 고르는데, 기본 실행 정책(Restricted)에서는 막혀
/// "스크립트를 실행할 수 없습니다" 오류가 난다. 실행 파일(.exe)이 없고 `.cmd`가 있으면 `.cmd`를 쓴다.
fn windows_cmd_shim(launch: &str) -> String {
    if !cfg!(windows) {
        return launch.to_string();
    }
    let mut parts = launch.splitn(2, ' ');
    let prog = parts.next().unwrap_or_default();
    let rest = parts.next();
    if prog.is_empty() || prog.contains(['\\', '/', '.']) {
        return launch.to_string();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return launch.to_string();
    };
    for dir in std::env::split_paths(&path) {
        if dir.join(format!("{prog}.exe")).is_file() {
            return launch.to_string();
        }
        if dir.join(format!("{prog}.cmd")).is_file() {
            return match rest {
                Some(r) => format!("{prog}.cmd {r}"),
                None => format!("{prog}.cmd"),
            };
        }
    }
    launch.to_string()
}

/// 셸만 떠 있는 pane에서 에이전트를 시작한다.
async fn agent_start(server: &Arc<Server>, pane: &str, p: &Value) -> Result<Value, RpcError> {
    let name = need(p, "name")?;
    let kind = need(p, "kind")?;
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(RpcError::invalid_params(
            "agent name must be alphanumeric, '-' or '_'",
        ));
    }
    let launch = {
        let det = server
            .detector
            .read()
            .map_err(|_| RpcError::internal("lock"))?;
        det.manifest(&kind)
            .map(|m| m.launch.clone())
            .ok_or_else(|| RpcError::invalid_params(format!("unknown agent kind '{kind}'")))?
    };
    let (pane_profile, cwd, idle) = {
        let mut st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
        // 이름은 지금 에이전트가 돌고 있는 창끼리만 겹치면 안 된다. 에이전트가 끝났거나
        // 서버 재시작 뒤 셸만 남은 창에 남아 있는 이름은 낡은 것이라 떼어 내고 새로 쓴다.
        for (id, x) in st.panes.iter_mut() {
            if x.alias.as_deref() == Some(&name) && id != pane {
                if x.agent.is_some() {
                    return Err(RpcError::invalid_params(format!(
                        "an agent named '{name}' already exists"
                    )));
                }
                x.alias = None;
            }
        }
        let pn = st
            .panes
            .get(pane)
            .ok_or_else(|| RpcError::not_found("no pane"))?;
        let idle = match (&pn.fg, pn.pid) {
            (Some((fg, _, _)), Some(root)) => *fg == root,
            _ => true,
        };
        (pn.profile, pn.cwd.clone(), idle)
    };
    if !idle {
        return Err(RpcError::invalid_params(format!(
            "pane {pane} is busy; agent.start needs an idle shell prompt"
        )));
    }
    // 에이전트별 격리 프로필이 pane 프로필보다 강하면 명령을 감싼다.
    let per_agent = server
        .policy
        .lock()
        .ok()
        .and_then(|pol| pol.cfg.sandbox.per_agent.get(&kind).cloned());
    let launch = windows_cmd_shim(&launch);
    let mut line = launch.clone();
    if let Some(prof) = per_agent.and_then(|x| Profile::parse(&x).ok()) {
        if prof != Profile::None && prof != pane_profile {
            let mut parts = launch.split_whitespace().map(String::from);
            let prog = parts.next().unwrap_or_default();
            let args: Vec<String> = parts.collect();
            let egress = server.egress.get().map(|e| e.port);
            let w = kmttt_sandbox::wrap(
                prof,
                &prog,
                &args,
                std::path::Path::new(&cwd),
                &server.sandbox_settings(),
                egress,
                &server.opts.kmttt_exe,
            )
            .map_err(err)?;
            line = std::iter::once(w.program)
                .chain(w.args)
                .map(|a| shell_quote(&a))
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    server
        .write_pane(pane, format!("{line}\r").as_bytes())
        .map_err(err)?;
    if let Ok(mut st) = server.st.lock() {
        if let Some(pn) = st.panes.get_mut(pane) {
            pn.alias = Some(name.clone());
        }
    }
    server.push_snapshot(true);
    Ok(json!({"pane": pane, "name": name, "kind": kind, "command": line}))
}

fn shell_quote(a: &str) -> String {
    if a.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,".contains(c))
    {
        a.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", a.replace('"', "\\\""))
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

async fn agent_prompt(server: &Arc<Server>, pane: &str, p: &Value) -> Result<Value, RpcError> {
    let text = need(p, "text")?;
    let paste = server
        .st
        .lock()
        .ok()
        .and_then(|st| st.panes.get(pane).map(|x| x.screen.bracketed_paste()))
        .unwrap_or(false);
    let before = server.st.lock().ok().and_then(|st| {
        st.panes
            .get(pane)
            .and_then(|x| x.agent.as_ref().map(|a| a.state))
    });
    // 질문(승인·선택 메뉴)이 떠 있는데 프롬프트를 보내면, 글자와 Enter가 그 질문의 답이 된다
    // (실제로 Codex 업데이트 메뉴에서 "Update now"가 골라졌다). 기본은 거부한다.
    if before == Some(AgentState::Blocked) && !b(p, "force", false) {
        return Err(RpcError::invalid_params(
            "the agent is waiting for an answer (blocked); answer it with `kmttt pane send` \
             or pass --force to send the prompt anyway",
        ));
    }
    if paste {
        server
            .write_pane(pane, format!("\x1b[200~{text}\x1b[201~").as_bytes())
            .map_err(err)?;
    } else {
        server.write_pane(pane, text.as_bytes()).map_err(err)?;
    }
    // 붙여넣기와 Enter 사이를 띄운다. Windows ConPTY는 bracketed paste를 그대로 넘기지 않아서,
    // 빠르게 들어온 글자 뒤의 Enter를 "붙여넣기 속 줄바꿈"으로 보는 TUI(예: Codex)가 있다.
    tokio::time::sleep(Duration::from_millis(400)).await;
    server.write_pane(pane, b"\r").map_err(err)?;
    // 그래도 질문이 입력 칸에 그대로 남아 있고 일을 시작하지 않았으면 Enter를 딱 한 번 더 누른다.
    // (첫 Enter가 먹혔다면 입력 칸이 비어 있으므로 두 번 보내지 않는다.)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let probe: String = text.chars().take(24).collect();
    let stuck = server
        .st
        .lock()
        .ok()
        .and_then(|st| {
            st.panes.get(pane).map(|x| {
                // 승인 질문(blocked)이 떠 있을 때 Enter를 누르면 "Yes"를 고를 수 있으므로 절대 다시 누르지 않는다.
                let busy_or_asking = x
                    .agent
                    .as_ref()
                    .is_some_and(|a| matches!(a.state, AgentState::Working | AgentState::Blocked));
                !busy_or_asking
                    && !probe.trim().is_empty()
                    && x.screen
                        .tail_lines(6)
                        .iter()
                        .any(|l| l.contains(probe.trim()))
            })
        })
        .unwrap_or(false);
    if stuck {
        tracing::info!(
            pane,
            "prompt still in the input box; pressing Enter once more"
        );
        server.write_pane(pane, b"\r").map_err(err)?;
    }
    if !b(p, "wait", false) {
        return Ok(json!({"pane": pane, "sent": true}));
    }
    let timeout = Duration::from_millis(u(p, "timeout_ms", 600_000));
    // 먼저 일을 시작했는지(최대 10초) 보고, 그다음 멈출 때까지 기다린다.
    let _ = wait_state(
        server,
        pane,
        &[AgentState::Working],
        Duration::from_secs(10),
    )
    .await;
    let st = wait_state(
        server,
        pane,
        &[AgentState::Blocked, AgentState::Idle, AgentState::Done],
        timeout,
    )
    .await?;
    Ok(
        json!({"pane": pane, "sent": true, "before": before.map(|s| s.as_str()), "state": st.as_str()}),
    )
}

async fn wait_state(
    server: &Arc<Server>,
    pane: &str,
    until: &[AgentState],
    timeout: Duration,
) -> Result<AgentState, RpcError> {
    let deadline = Instant::now() + timeout;
    loop {
        let cur = {
            let st = server.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let pn = st
                .panes
                .get(pane)
                .ok_or_else(|| RpcError::not_found(format!("pane {pane} is gone")))?;
            pn.agent.as_ref().map(|a| a.state)
        };
        if let Some(c) = cur {
            if until.contains(&c) {
                return Ok(c);
            }
        }
        if Instant::now() >= deadline {
            return Err(RpcError::new(
                codes::TIMEOUT,
                format!("agent did not reach {until:?} in time (now {cur:?})"),
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 에이전트 훅 이벤트(F5). PreToolUse는 정책으로 차단할 수 있다.
async fn hook_event(server: &Arc<Server>, conn: &Conn, p: &Value) -> Result<Value, RpcError> {
    let agent = s(p, "agent").unwrap_or_else(|| "unknown".into());
    let event = s(p, "event").unwrap_or_default();
    let payload = p.get("payload").cloned().unwrap_or(Value::Null);
    // 훅은 자기 pane에 대해서만 보고할 수 있다.
    let pane = match &conn.caller {
        Caller::Pane { pane, .. } if pane != "unknown" => pane.clone(),
        Caller::User => s(p, "pane").ok_or_else(|| {
            RpcError::invalid_params("hook.event from outside a pane needs 'pane'")
        })?,
        _ => {
            return Err(RpcError::new(
                codes::POLICY_DENIED,
                "hook.event must come from inside a pane",
            ))
        }
    };
    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let input_v = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    let input = payload
        .get("tool_input")
        .map(kmttt_ledger::canonical)
        .unwrap_or_default();
    let summary: String = server.redact(&input).chars().take(240).collect();
    let mut decision = "allow".to_string();
    let mut reason = String::new();
    let state = match event.as_str() {
        "PreToolUse" | "pre" => {
            let v = server
                .policy
                .lock()
                .map(|pol| pol.evaluate_tool_json(&agent, &tool, &input_v, &input))
                .map_err(|_| RpcError::internal("lock"))?;
            reason = format!("{} ({})", v.reason, v.rule_id);
            let label = conn.caller.label();
            let final_d = match v.decision {
                Decision::Allow => Decision::Allow,
                Decision::Deny => Decision::Deny,
                Decision::Ask => {
                    let timeout = server
                        .policy
                        .lock()
                        .map(|x| x.cfg.defaults.ask_timeout_secs)
                        .unwrap_or(120);
                    if server
                        .ask(
                            &label,
                            &format!("tool:{tool}"),
                            &pane,
                            &format!("{summary}  — {}", v.reason),
                            timeout,
                        )
                        .await
                    {
                        Decision::Allow
                    } else {
                        Decision::Deny
                    }
                }
            };
            decision = if v.decision == Decision::Ask {
                format!("ask→{}", final_d.as_str())
            } else {
                final_d.as_str().into()
            };
            if final_d == Decision::Deny {
                if let Ok(mut st) = server.st.lock() {
                    st.blocked_today += 1;
                    if let Some(x) = st.panes.get_mut(&pane) {
                        x.blocked_calls += 1;
                    }
                }
                server.security_event(
                    Severity::Critical,
                    "tool.denied",
                    &label,
                    &pane,
                    &format!("{tool}: {summary} — {reason}"),
                );
            }
            if let Ok(mut st) = server.st.lock() {
                st.push_tool(ToolCall {
                    ts_ms: now_ms(),
                    pane: pane.clone(),
                    agent: agent.clone(),
                    tool: tool.clone(),
                    summary: summary.clone(),
                    decision: decision.clone(),
                });
            }
            AgentState::Working
        }
        "PostToolUse" | "post" | "UserPromptSubmit" | "prompt" => AgentState::Working,
        "Notification" | "notification" | "PermissionRequest" => AgentState::Blocked,
        "Stop" | "stop" | "SubagentStop" | "agent-turn-complete" => AgentState::Idle,
        _ => AgentState::Unknown,
    };
    if state != AgentState::Unknown {
        if let Ok(mut st) = server.st.lock() {
            let visible = st.any_attached() && st.visible_panes().contains(&pane);
            if let Some(x) = st.panes.get_mut(&pane) {
                x.hook_at = Some(Instant::now());
                let a = x.agent.get_or_insert_with(|| AgentRt {
                    kind: agent.clone(),
                    state: AgentState::Unknown,
                    since_ms: now_ms(),
                    evidence: String::new(),
                    hooked: true,
                    was_working: false,
                });
                a.hooked = true;
                let next = if state == AgentState::Idle && !visible {
                    AgentState::Done
                } else {
                    state
                };
                if a.state != next {
                    a.state = next;
                    a.since_ms = now_ms();
                }
                a.was_working = next == AgentState::Working;
                a.evidence = format!("hook {event}");
            }
        }
    }
    server.emit(
        "hook.tool",
        &pane,
        json!({"agent": agent, "event": event, "tool": tool, "summary": summary, "decision": decision, "reason": reason}),
    );
    server.push_snapshot(false);
    let allowed = !decision.ends_with("deny");
    Ok(json!({"decision": if allowed { "allow" } else { "deny" }, "reason": reason}))
}
