//! KMttt 터미널 클라이언트. 서버에 붙어 화면을 그리고 입력을 전달한다.
//! 서버에서 pane 출력 바이트를 받아 클라이언트 쪽 vt100 파서로 화면을 재구성한다.

pub mod control;
pub mod input;
mod notify;
pub mod render;

use anyhow::{Context, Result};
use control::{ControlState, MachineView, Panel};
use kmttt_core::client::{Notifications, RpcClient};
use kmttt_core::keys::{Bindings, KeySpec};
use kmttt_core::layout::{self, Rect as LRect};
use kmttt_core::machines::MachineProfile;
use kmttt_core::Config;
use kmttt_proto::*;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::crossterm::{execute, terminal};
use ratatui::Terminal;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Remote {
    pub target: String,
    pub session: String,
    pub bin: String,
}

pub struct TuiOptions {
    pub session: String,
    pub remote: Option<Remote>,
    pub start_in_control: bool,
    pub machines: Vec<MachineProfile>,
    pub config: Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Workspaces(usize),
}

pub struct App {
    pub client: Arc<RpcClient>,
    pub snap: SessionSnapshot,
    pub parsers: HashMap<String, vt100::Parser>,
    pub prefix_pending: bool,
    pub overlay: Overlay,
    pub control: Option<ControlState>,
    pub toast: Option<(String, String, Instant)>,
    pub mask_screen: bool,
    pub redactor: kmttt_redact::Redactor,
    pub lang: String,
    pub sidebar_width: u16,
    pub bindings: Bindings,
    pub help_rows: Vec<(String, String)>,
    pub session_label: String,
    pub pending_approvals: Vec<Approval>,
    pub viewport: (u16, u16),
    cfg: Config,
    machines: Vec<MachineProfile>,
    quit: Option<String>,
    /// 마우스로 끌고 있는 경계선(탭 ID, 잡은 경계선)
    drag: Option<(String, layout::DividerHit)>,
}

fn nz(v: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(v.max(1)).expect("non-zero")
}

impl App {
    fn parser_for(&mut self, pane: &str, rows: u16, cols: u16) -> &mut vt100::Parser {
        self.parsers
            .entry(pane.to_string())
            .or_insert_with(|| vt100::Parser::new(nz(rows), nz(cols), 2000))
    }

    fn focused_pane(&self) -> Option<String> {
        self.snap.focused_tab().map(|t| t.focus.clone())
    }

    fn apply_snapshot(&mut self, snap: SessionSnapshot) {
        // pane 크기가 바뀌었으면 로컬 파서도 맞춘다.
        for p in &snap.panes {
            let parser = self.parser_for(&p.id, p.rows, p.cols);
            let (r, c) = parser.screen().size();
            if r.get() != p.rows || c.get() != p.cols {
                parser.screen_mut().set_size(nz(p.rows), nz(p.cols));
            }
        }
        let live: std::collections::HashSet<&String> = snap.panes.iter().map(|p| &p.id).collect();
        self.parsers.retain(|k, _| live.contains(k));
        self.snap = snap;
    }

    fn handle_note(&mut self, method: &str, p: Value) {
        match method {
            "pane.output" => {
                if let (Some(pane), Some(data)) = (p["pane"].as_str(), p["data"].as_str()) {
                    if let Ok(bytes) = b64_decode(data) {
                        let (rows, cols) = self
                            .snap
                            .pane(pane)
                            .map(|x| (x.rows, x.cols))
                            .unwrap_or((24, 80));
                        self.parser_for(pane, rows, cols).process(&bytes);
                    }
                }
            }
            "pane.bootstrap" => {
                if let (Some(pane), Some(data)) = (p["pane"].as_str(), p["data"].as_str()) {
                    let rows = p["rows"].as_u64().unwrap_or(24) as u16;
                    let cols = p["cols"].as_u64().unwrap_or(80) as u16;
                    let mut parser = vt100::Parser::new(nz(rows), nz(cols), 2000);
                    if let Ok(bytes) = b64_decode(data) {
                        parser.process(&bytes);
                    }
                    self.parsers.insert(pane.to_string(), parser);
                }
            }
            "snapshot" => {
                if let Ok(s) = serde_json::from_value::<SessionSnapshot>(p) {
                    self.apply_snapshot(s);
                }
            }
            "toast" => {
                let text = p["text"].as_str().unwrap_or_default().to_string();
                let level = p["level"].as_str().unwrap_or("info").to_string();
                let delivery = self.cfg.ui.toast.delivery.clone();
                // 보안 경보(critical)는 설정과 상관없이 앱 안에 띄운다.
                if delivery != "off" || level == "critical" {
                    self.toast = Some((text.clone(), level.clone(), Instant::now()));
                }
                notify::deliver(&delivery, &text);
            }
            _ => {}
        }
    }
}

/// 서버(로컬 또는 원격)에 붙어 TUI를 실행한다. 끝난 이유를 돌려준다.
pub async fn run(opts: TuiOptions) -> Result<String> {
    let (client, notes) = match &opts.remote {
        Some(r) => RpcClient::connect_ssh(&r.target, &r.session, &r.bin)
            .await
            .context("connect over ssh")?,
        None => RpcClient::connect_local(&opts.session)
            .await
            .context("connect to server")?,
    };
    let client = Arc::new(client);
    let session_label = match &opts.remote {
        Some(r) => format!("{}@{}", r.session, r.target),
        None => opts.session.clone(),
    };
    let bindings = Bindings::from_config(&opts.config.keys).map_err(anyhow::Error::msg)?;
    let help_rows = bindings.help(&opts.config.keys);
    let redactor = kmttt_redact::Redactor::default();

    // 터미널 준비
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        terminal::EnterAlternateScreen,
        event::EnableMouseCapture,
        event::EnableBracketedPaste,
        event::EnableFocusChange
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let result = run_loop(
        &mut term,
        client,
        notes,
        opts,
        bindings,
        help_rows,
        redactor,
        session_label,
    )
    .await;
    // 무슨 일이 있어도 터미널을 되돌린다.
    let _ = execute!(
        term.backend_mut(),
        event::DisableFocusChange,
        event::DisableBracketedPaste,
        event::DisableMouseCapture,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
    let _ = term.show_cursor();
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: Arc<RpcClient>,
    mut notes: Notifications,
    opts: TuiOptions,
    bindings: Bindings,
    help_rows: Vec<(String, String)>,
    redactor: kmttt_redact::Redactor,
    session_label: String,
) -> Result<String> {
    let size = term.size()?;
    let mut app = App {
        client: client.clone(),
        snap: SessionSnapshot::default(),
        parsers: HashMap::new(),
        prefix_pending: false,
        overlay: Overlay::None,
        control: None,
        toast: None,
        mask_screen: opts.config.ui.mask_screen,
        redactor,
        lang: opts.config.language.clone(),
        sidebar_width: opts.config.ui.sidebar_width,
        bindings,
        help_rows,
        session_label,
        pending_approvals: Vec::new(),
        viewport: (0, 0),
        cfg: opts.config.clone(),
        machines: opts.machines.clone(),
        quit: None,
        drag: None,
    };
    let full = ratatui::layout::Rect::new(0, 0, size.width, size.height);
    let (_, _, main, _) = render::areas(&app, full);
    let snap = client
        .call(
            "client.attach",
            json!({"cols": main.width, "rows": main.height}),
        )
        .await
        .map_err(|e| anyhow::anyhow!("attach: {e}"))?;
    app.viewport = (main.width, main.height);
    if let Ok(s) = serde_json::from_value::<SessionSnapshot>(snap) {
        app.apply_snapshot(s);
    }
    if opts.start_in_control {
        enter_control(&mut app);
    }

    // 키 입력은 별도 스레드에서 읽는다(crossterm의 블로킹 read).
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if ev_tx.send(ev).is_err() {
                break;
            }
        }
    });
    let (rem_tx, mut rem_rx): (_, control::RemoteRx) = mpsc::unbounded_channel();
    let mut remotes_started = false;
    let mut redraw = Duration::from_millis(16);
    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    let mut dirty = true;
    let mut last_draw = Instant::now() - redraw;

    loop {
        if dirty && last_draw.elapsed() >= redraw {
            term.draw(|f| match &app.control {
                Some(cs) => control::draw(f, cs, &app.lang),
                None => render::draw(f, &app),
            })?;
            last_draw = Instant::now();
            dirty = false;
        }
        if let Some(q) = app.quit.take() {
            return Ok(q);
        }
        let wait = if dirty {
            redraw.saturating_sub(last_draw.elapsed())
        } else {
            Duration::from_secs(3600)
        };
        tokio::select! {
            n = notes.recv() => {
                match n {
                    Some((m, p)) => { app.handle_note(&m, p); dirty = true; }
                    None => return Ok("server connection closed".into()),
                }
                // 몰려온 출력은 한 번에 처리하고 한 번만 그린다.
                while let Ok((m, p)) = notes.try_recv() {
                    app.handle_note(&m, p);
                }
            }
            Some(ev) = ev_rx.recv() => {
                handle_event(&mut app, term, ev).await;
                dirty = true;
            }
            Some((idx, r)) = rem_rx.recv() => {
                if let Some(cs) = app.control.as_mut() {
                    if let Some(m) = cs.machines.get_mut(idx) {
                        match r {
                            Ok((s, c)) => { m.snap = Some(s); m.client = Some(c); m.error = None; m.stale = false; }
                            Err(e) => { m.error = Some(e); m.stale = true; }
                        }
                    }
                }
                dirty = true;
            }
            _ = tick.tick() => {
                if let Some((_, _, at)) = &app.toast {
                    if at.elapsed() > Duration::from_secs(6) { app.toast = None; }
                }
                // 승인 대기 수(사이드바 배지)와 관제 화면은 주기적으로 받아 온다.
                if let Ok(v) = app.client.call_timeout("approval.list", json!({}), Duration::from_secs(5)).await {
                    app.pending_approvals = serde_json::from_value(v).unwrap_or_default();
                }
                if app.control.is_some() {
                    refresh_control(&mut app).await;
                    if !remotes_started {
                        remotes_started = true;
                        for (i, m) in app.machines.iter().filter(|m| m.enabled).enumerate() {
                            control::spawn_remote(i + 1, m.clone(), rem_tx.clone());
                        }
                    }
                }
                dirty = true;
            }
            _ = tokio::time::sleep(wait) => {}
        }
        redraw = Duration::from_millis(16);
    }
}

fn enter_control(app: &mut App) {
    let mut machines = vec![MachineView {
        label: if app.session_label.contains('@') {
            app.session_label.clone()
        } else {
            "local".into()
        },
        snap: None,
        error: None,
        client: None,
        stale: false,
    }];
    for m in app.machines.iter().filter(|m| m.enabled) {
        machines.push(MachineView {
            label: m.label.clone(),
            snap: None,
            error: None,
            client: None,
            stale: true,
        });
    }
    app.control = Some(ControlState {
        panel: Panel::Agents,
        sel: 0,
        machines,
        detail: None,
    });
}

async fn refresh_control(app: &mut App) {
    let r = app
        .client
        .call_timeout("control.snapshot", json!({}), Duration::from_secs(5))
        .await;
    if let Some(cs) = app.control.as_mut() {
        match r.map(serde_json::from_value::<ControlSnapshot>) {
            Ok(Ok(s)) => {
                cs.machines[0].snap = Some(s);
                cs.machines[0].error = None;
            }
            Ok(Err(e)) => cs.machines[0].error = Some(e.to_string()),
            Err(e) => cs.machines[0].error = Some(e.to_string()),
        }
        let n = control::panel_len(cs);
        if cs.sel >= n {
            cs.sel = n.saturating_sub(1);
        }
    }
}

async fn call(app: &mut App, method: &str, params: Value) {
    if let Err(e) = app.client.call(method, params).await {
        app.toast = Some((format!("{method}: {e}"), "notice".into(), Instant::now()));
    }
}

async fn handle_event(
    app: &mut App,
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ev: Event,
) {
    match ev {
        Event::Resize(w, h) => {
            let full = ratatui::layout::Rect::new(0, 0, w, h);
            let (_, _, main, _) = render::areas(app, full);
            if (main.width, main.height) != app.viewport {
                app.viewport = (main.width, main.height);
                let _ = app
                    .client
                    .call(
                        "client.viewport",
                        json!({"cols": main.width, "rows": main.height}),
                    )
                    .await;
            }
            let _ = term.clear();
        }
        Event::FocusGained => {
            if app.cfg.ui.redraw_on_focus_gained {
                let _ = term.clear();
            }
        }
        Event::Paste(text) => {
            if app.control.is_none() {
                if let Some(p) = app.focused_pane() {
                    let br = app
                        .parsers
                        .get(&p)
                        .map(|x| x.screen().bracketed_paste())
                        .unwrap_or(false);
                    app.client.notify(
                        "client.input",
                        json!({"pane": p, "data": b64_encode(&input::encode_paste(&text, br))}),
                    );
                }
            }
        }
        Event::Mouse(m) => handle_mouse(app, m).await,
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            if app.control.is_some() {
                handle_control_key(app, k).await;
            } else {
                handle_key(app, k).await;
            }
        }
        _ => {}
    }
}

async fn handle_mouse(app: &mut App, m: event::MouseEvent) {
    if app.control.is_some() || app.overlay != Overlay::None {
        return;
    }
    let full = ratatui::layout::Rect::new(
        0,
        0,
        app.viewport.0 + app.sidebar_width + 1,
        app.viewport.1 + 2,
    );
    let (sidebar, _, main, _) = render::areas(app, full);
    let Some(tab) = app.snap.focused_tab().cloned() else {
        return;
    };
    let la = LRect {
        x: main.x,
        y: main.y,
        w: main.width,
        h: main.height,
    };
    let rects = if tab.zoomed {
        vec![(tab.focus.clone(), la)]
    } else {
        layout::compute(&tab.layout, la)
    };
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if m.column < sidebar.width {
                // 사이드바: 워크스페이스 줄(4번째 줄부터)을 누르면 그 워크스페이스로.
                let row = m.row as usize;
                if row >= 4 && row < 4 + app.snap.workspaces.len() {
                    let id = app.snap.workspaces[row - 4].id.clone();
                    call(app, "workspace.focus", json!({"id": id})).await;
                }
                return;
            }
            // 경계선을 잡으면 끌어서 크기를 바꾼다.
            if !tab.zoomed {
                if let Some(hit) = layout::divider_at(&tab.layout, la, m.column, m.row) {
                    app.drag = Some((tab.id.clone(), hit));
                    return;
                }
            }
            if let Some((id, _)) = rects.iter().find(|(_, r)| r.contains(m.column, m.row)) {
                if Some(id.clone()) != app.focused_pane() {
                    call(app, "pane.focus", json!({"pane": id})).await;
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((tab_id, hit)) = app.drag.clone() {
                let ratio = layout::ratio_for(&hit, m.column, m.row);
                // 알림(notify)은 서버가 입력 외에는 버리므로 일반 호출로 보낸다(정책도 거친다).
                call(
                    app,
                    "layout.set_ratio",
                    json!({"tab": tab_id, "path": hit.path, "ratio": ratio}),
                )
                .await;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.drag = None;
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if let Some((id, r)) = rects.iter().find(|(_, r)| r.contains(m.column, m.row)) {
                let wants = app
                    .parsers
                    .get(id)
                    .map(|p| p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
                    .unwrap_or(false);
                if wants {
                    let bytes = input::encode_wheel(
                        matches!(m.kind, MouseEventKind::ScrollUp),
                        m.column - r.x,
                        m.row - r.y,
                    );
                    app.client.notify(
                        "client.input",
                        json!({"pane": id, "data": b64_encode(&bytes)}),
                    );
                }
            }
        }
        _ => {}
    }
}

async fn handle_key(app: &mut App, k: KeyEvent) {
    // 겹친 창(도움말·워크스페이스 선택)
    match app.overlay {
        Overlay::Help => {
            app.overlay = Overlay::None;
            return;
        }
        Overlay::Workspaces(sel) => {
            use ratatui::crossterm::event::KeyCode;
            let n = app.snap.workspaces.len();
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => app.overlay = Overlay::None,
                KeyCode::Up | KeyCode::Char('k') => {
                    app.overlay = Overlay::Workspaces(sel.saturating_sub(1))
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.overlay = Overlay::Workspaces((sel + 1).min(n.saturating_sub(1)))
                }
                KeyCode::Enter => {
                    if let Some(w) = app.snap.workspaces.get(sel) {
                        let id = w.id.clone();
                        call(app, "workspace.focus", json!({"id": id})).await;
                    }
                    app.overlay = Overlay::None;
                }
                KeyCode::Char('n') => {
                    call(app, "workspace.create", json!({"focus": true})).await;
                    app.overlay = Overlay::None;
                }
                KeyCode::Char('x') => {
                    if let Some(w) = app.snap.workspaces.get(sel) {
                        let id = w.id.clone();
                        call(app, "workspace.close", json!({"id": id})).await;
                    }
                    app.overlay = Overlay::None;
                }
                _ => {}
            }
            return;
        }
        Overlay::None => {}
    }
    if app.prefix_pending {
        app.prefix_pending = false;
        if input::same_key(
            &KeySpec {
                after_prefix: false,
                ..app.bindings.prefix.clone()
            },
            &k,
        ) {
            // prefix 두 번 → prefix 키 자체를 보낸다.
            send_key(app, &k);
            return;
        }
        if let Some(spec) = input::to_spec(&k, true) {
            if let Some(action) = app.bindings.lookup(&spec).map(String::from) {
                do_action(app, &action).await;
            }
        }
        return;
    }
    if input::same_key(
        &KeySpec {
            after_prefix: false,
            ..app.bindings.prefix.clone()
        },
        &k,
    ) {
        app.prefix_pending = true;
        return;
    }
    if let Some(spec) = input::to_spec(&k, false) {
        if let Some(action) = app.bindings.lookup(&spec).map(String::from) {
            do_action(app, &action).await;
            return;
        }
    }
    send_key(app, &k);
}

fn send_key(app: &mut App, k: &KeyEvent) {
    let Some(p) = app.focused_pane() else { return };
    let app_cursor = app
        .parsers
        .get(&p)
        .map(|x| x.screen().application_cursor())
        .unwrap_or(false);
    let bytes = input::encode_key(k, app_cursor);
    if !bytes.is_empty() {
        app.client.notify(
            "client.input",
            json!({"pane": p, "data": b64_encode(&bytes)}),
        );
    }
}

async fn do_action(app: &mut App, action: &str) {
    let focused = app.focused_pane();
    match action {
        "split_right" | "split_down" => {
            if let Some(p) = focused {
                let dir = if action == "split_right" {
                    "right"
                } else {
                    "down"
                };
                call(app, "pane.split", json!({"target": p, "direction": dir})).await;
            }
        }
        "new_tab" => call(app, "tab.create", json!({"focus": true})).await,
        "next_tab" => call(app, "tab.focus", json!({"offset": 1})).await,
        "prev_tab" => call(app, "tab.focus", json!({"offset": -1})).await,
        "workspaces" => {
            let i = app
                .snap
                .workspaces
                .iter()
                .position(|w| w.id == app.snap.focus_workspace)
                .unwrap_or(0);
            app.overlay = Overlay::Workspaces(i);
        }
        "new_workspace" => call(app, "workspace.create", json!({"focus": true})).await,
        "zoom" => {
            if let Some(p) = focused {
                call(app, "pane.zoom", json!({"pane": p})).await;
            }
        }
        "detach" => app.quit = Some("detached".into()),
        "help" => app.overlay = Overlay::Help,
        "control" | "security" | "approvals" => {
            enter_control(app);
            if let Some(cs) = app.control.as_mut() {
                cs.panel = match action {
                    "security" => Panel::Events,
                    "approvals" => Panel::Approvals,
                    _ => Panel::Agents,
                };
            }
            refresh_control(app).await;
        }
        "close_pane" => {
            if let Some(p) = focused {
                call(app, "pane.close", json!({"pane": p})).await;
            }
        }
        "focus_left" | "focus_right" | "focus_up" | "focus_down" => {
            let (dx, dy) = match action {
                "focus_left" => (-1, 0),
                "focus_right" => (1, 0),
                "focus_up" => (0, -1),
                _ => (0, 1),
            };
            if let (Some(tab), Some(p)) = (app.snap.focused_tab().cloned(), focused) {
                let area = LRect {
                    x: 0,
                    y: 0,
                    w: app.viewport.0,
                    h: app.viewport.1,
                };
                if let Some(n) = layout::neighbor(&tab.layout, area, &p, dx, dy) {
                    call(app, "pane.focus", json!({"pane": n})).await;
                }
            }
        }
        "mask_screen" => app.mask_screen = !app.mask_screen,
        _ => {}
    }
}

async fn handle_control_key(app: &mut App, k: KeyEvent) {
    use ratatui::crossterm::event::KeyCode;
    let Some(cs) = app.control.as_mut() else {
        return;
    };
    cs.detail = None;
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.control = None;
        }
        KeyCode::Tab => {
            cs.panel = cs.panel.next();
            cs.sel = 0;
        }
        KeyCode::BackTab => {
            for _ in 0..4 {
                cs.panel = cs.panel.next();
            }
            cs.sel = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => cs.sel = cs.sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            let n = control::panel_len(cs);
            cs.sel = (cs.sel + 1).min(n.saturating_sub(1));
        }
        KeyCode::Char('a') | KeyCode::Char('x') if cs.panel == Panel::Approvals => {
            let allow = k.code == KeyCode::Char('a');
            let pick = control::all_approvals(cs)
                .get(cs.sel)
                .map(|(mi, a)| (*mi, a.id));
            if let Some((mi, id)) = pick {
                let remote = if mi == 0 {
                    None
                } else {
                    control::machine_client(cs, mi)
                };
                let client = remote.unwrap_or_else(|| app.client.clone());
                let r = client
                    .call("approval.decide", json!({"id": id, "allow": allow}))
                    .await;
                if let Some(cs) = app.control.as_mut() {
                    cs.detail = Some(match r {
                        Ok(_) => format!("#{id} {}", if allow { "allowed" } else { "denied" }),
                        Err(e) => format!("#{id}: {e}"),
                    });
                }
                refresh_control(app).await;
            }
        }
        KeyCode::Enter if cs.panel == Panel::Agents => {
            let pick = control::all_panes(cs)
                .get(cs.sel)
                .map(|(mi, p)| (*mi, p.id.clone()));
            if let Some((0, id)) = pick {
                app.control = None;
                call(app, "pane.focus", json!({"pane": id})).await;
            } else if pick.is_some() {
                cs.detail = Some(
                    "remote pane: attach with `kmttt --remote <machine>` to type into it".into(),
                );
            }
        }
        KeyCode::Char('K') if cs.panel == Panel::Agents => {
            let pick = control::all_panes(cs)
                .get(cs.sel)
                .map(|(mi, p)| (*mi, p.id.clone()));
            if let Some((mi, id)) = pick {
                let remote = if mi == 0 {
                    None
                } else {
                    control::machine_client(cs, mi)
                };
                let client = remote.unwrap_or_else(|| app.client.clone());
                let r = client.call("pane.close", json!({"pane": id})).await;
                if let Some(cs) = app.control.as_mut() {
                    cs.detail = Some(match r {
                        Ok(_) => format!("{id} closed"),
                        Err(e) => format!("{id}: {e}"),
                    });
                }
            }
        }
        KeyCode::Enter => {
            cs.detail = Some("selected".into());
        }
        _ => {}
    }
}

/// 터미널 크기(cols, rows).
pub fn term_size() -> Option<(u16, u16)> {
    terminal::size().ok()
}

/// raw 모드 켜기·끄기(개발용 debug-pane).
pub fn raw_mode(on: bool) {
    if on {
        let _ = terminal::enable_raw_mode();
    } else {
        let _ = terminal::disable_raw_mode();
    }
}
