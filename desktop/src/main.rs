//! KMttt Desktop: KMttt 서버에 붙는 창 GUI.
//!
//! 이 앱은 TUI와 같은 "사람 클라이언트"일 뿐이다. 판단(정책·승인·격리·기록)은 모두 서버가 한다.
//! 웹뷰는 이 파일에 정의된 명령만 부를 수 있고, 그중 서버 RPC는 아래 허용 목록으로 좁힌다.
//! Personal project by Kwangmo Kim.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod settings;

use kmttt_core::client::RpcClient;
use kmttt_core::paths;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

struct Conn {
    client: Arc<RpcClient>,
    session: String,
}

#[derive(Default)]
struct AppState {
    conn: Mutex<Option<Conn>>,
    gen: AtomicU64,
}

/// 웹뷰가 부를 수 있는 서버 메서드. 서버 쪽 정책 검사는 이와 별개로 그대로 적용된다.
const ALLOWED: &[&str] = &[
    "session.snapshot",
    "client.viewport",
    "pane.redraw",
    "server.status",
    "server.reload_config",
    "server.stop",
    "workspace.list",
    "workspace.create",
    "workspace.focus",
    "workspace.rename",
    "workspace.close",
    "tab.list",
    "tab.create",
    "tab.focus",
    "tab.rename",
    "tab.close",
    "pane.split",
    "pane.focus",
    "pane.zoom",
    "pane.close",
    "layout.set_ratio",
    "layout.swap",
    "layout.move",
    "layout.equalize",
    "agent.list",
    "agent.start",
    "agent.prompt",
    "agent.explain",
    "approval.list",
    "approval.decide",
    "approval.grants",
    "approval.revoke",
    "control.snapshot",
    "audit.status",
    "policy.show",
    "plugin.list",
];

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "kmttt.exe"
    } else {
        "kmttt"
    }
}

/// 서버를 띄울 kmttt 실행 파일: 앱과 같은 폴더 → PATH 순서.
fn kmttt_exe() -> Option<PathBuf> {
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let p = dir.join(exe_name());
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe_name()))
        .find(|p| p.is_file())
}

async fn client(state: &State<'_, AppState>) -> Result<Arc<RpcClient>, String> {
    state
        .conn
        .lock()
        .await
        .as_ref()
        .map(|c| c.client.clone())
        .ok_or_else(|| "not connected".to_string())
}

/// 서버에 붙는다. 없고 `start`면 kmttt 서버를 띄운다. 알림은 "kmttt-note" 이벤트로 넘긴다.
#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
    start: bool,
) -> Result<Value, String> {
    if !paths::valid_session_name(&session) {
        return Err(format!("invalid session name '{session}'"));
    }
    let (c, mut notes) = match RpcClient::connect_local(&session).await {
        Ok(c) => c,
        Err(e) => {
            if !start {
                return Err(format!("no server for session '{session}': {e:#}"));
            }
            let exe = kmttt_exe().ok_or_else(|| {
                format!("could not find {} next to the app or on PATH", exe_name())
            })?;
            kmttt_core::spawn::ensure_server(&exe, &session)
                .await
                .map_err(|e| format!("{e:#}"))?
        }
    };
    let c = Arc::new(c);
    let gen = state.gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some((method, params)) = notes.recv().await {
            let _ = app2.emit(
                "kmttt-note",
                json!({"gen": gen, "method": method, "params": params}),
            );
        }
        let _ = app2.emit("kmttt-disconnected", json!({"gen": gen}));
    });
    let snap = c
        .call_timeout(
            "client.attach",
            json!({"cols": cols.max(10), "rows": rows.max(3)}),
            Duration::from_secs(20),
        )
        .await
        .map_err(|e| e.message)?;
    *state.conn.lock().await = Some(Conn {
        client: c,
        session: session.clone(),
    });
    Ok(json!({"gen": gen, "session": session, "snapshot": snap}))
}

/// 허용 목록 안의 서버 메서드를 부른다.
#[tauri::command]
async fn rpc(state: State<'_, AppState>, method: String, params: Value) -> Result<Value, String> {
    if !ALLOWED.contains(&method.as_str()) {
        return Err(format!(
            "method '{method}' is not available in the desktop app"
        ));
    }
    let c = client(&state).await?;
    let t = if method.starts_with("agent.") {
        Duration::from_secs(180)
    } else {
        Duration::from_secs(30)
    };
    c.call_timeout(&method, params, t)
        .await
        .map_err(|e| e.message)
}

/// 키 입력을 pane에 보낸다. `binary`면 각 글자를 바이트 하나로 본다(xterm onBinary).
#[tauri::command]
async fn input(
    state: State<'_, AppState>,
    pane: String,
    data: String,
    binary: Option<bool>,
) -> Result<(), String> {
    let c = client(&state).await?;
    let bytes: Vec<u8> = if binary.unwrap_or(false) {
        data.chars().map(|ch| ch as u32 as u8).collect()
    } else {
        data.into_bytes()
    };
    c.notify(
        "client.input",
        json!({"pane": pane, "data": kmttt_proto::b64_encode(&bytes)}),
    );
    Ok(())
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    *state.conn.lock().await = None;
    Ok(())
}

/// Claude Code·Codex 요금제 사용량(남은 % 계산은 화면에서).
#[tauri::command]
async fn usage_read() -> Value {
    tauri::async_runtime::spawn_blocking(|| {
        json!({
            "claude": kmttt_core::usage::read_claude(),
            "codex": kmttt_core::usage::read_codex(),
            "claude_connected": claude_usage_connected(),
            "claude_hooks": claude_hooks_connected(),
            "now_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        })
    })
    .await
    .unwrap_or(Value::Null)
}

/// 앱 창이 앞에 있지 않을 때만 운영체제 알림을 띄운다(승인 요청 알림용).
/// 알림에는 승인 버튼을 두지 않는다: 알림 창으로는 요청 내용을 다 볼 수 없기 때문이다.
#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> bool {
    use tauri::Manager;
    use tauri_plugin_notification::NotificationExt;
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return false;
    }
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    app.notification()
        .builder()
        .title(cut(&title, 80))
        .body(cut(&body, 300))
        .show()
        .is_ok()
}

/// 창별 토큰 사용량을 이어서 읽는 상태(기록 파일 경로 → 읽은 위치·합계).
static TRACKERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, kmttt_core::tokens::Tracker>>,
> = std::sync::OnceLock::new();

/// 기록 파일로 받아들이는 경로: 보통 파일이고 이름이 .jsonl로 끝나야 한다.
/// (창 짝 파일은 같은 사용자 권한으로 바뀔 수 있으므로 장치 파일·파이프 등은 열지 않는다.)
fn token_file_ok(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| e == "jsonl")
        && std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// 토큰 사용량 패널용: 창마다 누적 토큰과 최근 15분의 응답별 토큰.
/// `panes`는 화면이 가진 창 목록 [{id, kind, cwd}]. 숫자만 돌려주고 대화 내용은 읽어 넘기지 않는다.
#[tauri::command]
async fn tokens_read(session: String, panes: Vec<Value>) -> Value {
    tauri::async_runtime::spawn_blocking(move || {
        use kmttt_core::tokens;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let since = now - 15 * 60 * 1000;
        let mut want: Vec<(String, String, PathBuf, bool)> = Vec::new(); // (창, 종류, 파일, 추정)
        let links = tokens::read_links(&session);
        let mut codex_panes = Vec::new();
        for p in panes.iter().take(64) {
            let id = p["id"].as_str().unwrap_or("").to_string();
            let kind = p["kind"].as_str().unwrap_or("");
            if id.is_empty() {
                continue;
            }
            if kind == "claude" {
                if let Some(l) = links.iter().find(|l| l.pane == id) {
                    want.push((id, "claude".into(), PathBuf::from(&l.transcript), false));
                }
            } else if kind == "codex" {
                codex_panes.push((id, p["cwd"].as_str().unwrap_or("").to_string()));
            }
        }
        if !codex_panes.is_empty() {
            let files = tokens::recent_codex_files(
                &kmttt_core::usage::codex_sessions_dir(),
                24 * 3600 * 1000,
            );
            for (pane, f) in tokens::codex_assign(&codex_panes, &files) {
                want.push((pane, "codex".into(), f, true));
            }
        }
        let lock = TRACKERS.get_or_init(Default::default);
        let mut map = match lock.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        let keep: std::collections::HashSet<PathBuf> = want.iter().map(|w| w.2.clone()).collect();
        map.retain(|k, _| keep.contains(k));
        let mut out = Vec::new();
        for (pane, kind, file, guess) in want {
            if !token_file_ok(&file) {
                continue;
            }
            let tr = map
                .entry(file.clone())
                .or_insert_with(|| tokens::Tracker::new(&kind));
            tr.refresh(&file);
            out.push(json!({
                "pane": pane,
                "kind": kind,
                "total": tr.total(),
                "samples": tr.samples_since(since),
                "guess": guess,
            }));
        }
        json!({ "now_ms": now, "panes": out })
    })
    .await
    .unwrap_or(Value::Null)
}

fn claude_settings_json() -> Value {
    let f = std::env::var_os("KMTTT_CLAUDE_SETTINGS")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".claude").join("settings.json"))
        });
    f.and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or(Value::Null)
}

fn claude_usage_connected() -> bool {
    claude_settings_json()["statusLine"]["command"]
        .as_str()
        .is_some_and(|c| c.contains("kmttt") && c.trim_end().ends_with("statusline"))
}

fn claude_hooks_connected() -> bool {
    claude_settings_json()["hooks"]
        .to_string()
        .contains("hook claude")
}

/// 함께 들어 있는 kmttt로 통합을 설치·제거한다(셸을 거치지 않고 인자를 그대로 넘긴다).
fn run_kmttt(args: &[&str]) -> Result<String, String> {
    let exe = kmttt_exe().ok_or_else(|| format!("could not find {}", exe_name()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text.trim().to_string())
    } else {
        Err(text.trim().to_string())
    }
}

/// Claude 사용량 표시를 켠다(상태 줄). `hooks`면 상태 감지용 훅도 함께.
#[tauri::command]
async fn usage_connect_claude(replace: bool, hooks: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut msgs = Vec::new();
        if hooks && !claude_hooks_connected() {
            msgs.push(run_kmttt(&["integration", "install", "claude"])?);
        }
        let mut a = vec!["integration", "install", "claude-usage"];
        if replace {
            a.push("--replace");
        }
        msgs.push(run_kmttt(&a)?);
        Ok(msgs.join("\n"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn usage_disconnect_claude() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        run_kmttt(&["integration", "uninstall", "claude-usage"])
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn settings_read() -> Value {
    settings::read_all()
}

/// 설정을 바꾸고 서버에 다시 읽힌다. 서버가 거부하면 파일을 되돌린다.
#[tauri::command]
async fn settings_save(
    state: State<'_, AppState>,
    file: String,
    changes: Vec<settings::Change>,
) -> Result<Value, String> {
    let path = settings::file_path(&file).map_err(|e| e.to_string())?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let text = settings::render(&file, &current, &changes).map_err(|e| format!("{e:#}"))?;
    let old = settings::write(&file, &text).map_err(|e| format!("{e:#}"))?;
    let conn = state.conn.lock().await.as_ref().map(|c| c.client.clone());
    if let Some(c) = conn {
        if let Err(e) = c
            .call_timeout("server.reload_config", json!({}), Duration::from_secs(20))
            .await
        {
            let _ = settings::restore(&file, old.as_deref());
            return Err(format!(
                "the server rejected the new settings: {}",
                e.message
            ));
        }
        return Ok(json!({"saved": path.display().to_string(), "reloaded": true}));
    }
    Ok(json!({"saved": path.display().to_string(), "reloaded": false}))
}

/// 설정 파일 원문(고급: 직접 보기).
#[tauri::command]
fn settings_raw(file: String) -> Result<Value, String> {
    let path = settings::file_path(&file).map_err(|e| e.to_string())?;
    Ok(json!({
        "path": path.display().to_string(),
        "text": std::fs::read_to_string(&path).unwrap_or_default(),
    }))
}

/// 원문을 통째로 저장(검사·되돌리기는 settings_save와 같다).
#[tauri::command]
async fn settings_save_raw(
    state: State<'_, AppState>,
    file: String,
    text: String,
) -> Result<Value, String> {
    if text.len() > 1 << 20 {
        return Err("file too large".into());
    }
    settings::render(&file, &text, &[]).map_err(|e| format!("{e:#}"))?;
    let old = settings::write(&file, &text).map_err(|e| format!("{e:#}"))?;
    let conn = state.conn.lock().await.as_ref().map(|c| c.client.clone());
    if let Some(c) = conn {
        if let Err(e) = c
            .call_timeout("server.reload_config", json!({}), Duration::from_secs(20))
            .await
        {
            let _ = settings::restore(&file, old.as_deref());
            return Err(format!(
                "the server rejected the new settings: {}",
                e.message
            ));
        }
    }
    Ok(json!({"saved": true}))
}

/// 에이전트 종류(내장 + 사용자 매니페스트).
#[tauri::command]
fn agent_kinds() -> Value {
    let (det, _) = kmttt_detect::Detector::with_user_dir(&paths::manifests_dir());
    Value::Array(
        det.manifests()
            .iter()
            .map(|m| json!({"kind": m.kind, "display": m.display, "launch": m.launch}))
            .collect(),
    )
}

#[tauri::command]
async fn app_info(state: State<'_, AppState>) -> Result<Value, String> {
    let session = state.conn.lock().await.as_ref().map(|c| c.session.clone());
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "session": session,
        "kmttt": kmttt_exe().map(|p| p.display().to_string()),
        "config_dir": paths::config_dir().display().to_string(),
        "log_file": paths::log_file().display().to_string(),
    }))
}

fn prefs_path() -> PathBuf {
    paths::config_dir().join("desktop.json")
}

/// 앱 전용 설정(글꼴·테마·첫 실행 완료 등). 서버 동작과 무관한 것만 둔다.
#[tauri::command]
fn prefs_get() -> Value {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}))
}

#[tauri::command]
fn prefs_set(prefs: Value) -> Result<(), String> {
    if !prefs.is_object() {
        return Err("prefs must be an object".into());
    }
    let text = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    if text.len() > 64 * 1024 {
        return Err("prefs too large".into());
    }
    let p = prefs_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    rx.await
        .ok()
        .flatten()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.display().to_string())
}

/// http(s) 주소만 기본 브라우저로 연다. 확인은 화면에서 먼저 받는다.
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    let ok_scheme = lower.starts_with("https://") || lower.starts_with("http://");
    if !ok_scheme || url.len() > 2048 || url.chars().any(|c| c.is_control() || c == ' ') {
        return Err("only plain http(s) links can be opened".into());
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_read(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().read_text().map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_write(app: AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// 창을 다시 보이고 앞으로 가져온다(트레이·두 번째 실행에서).
fn show_main(app: &AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 창 닫기 버튼: 화면만 숨긴다(트레이에서 다시 열 수 있음). 에이전트와 서버는 계속 돈다.
#[tauri::command]
fn window_hide(app: AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

/// 프로그램 끝내기. 서버 종료는 화면이 먼저 server.stop으로 처리한다.
#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

fn tray_labels() -> (&'static str, &'static str) {
    let ko = prefs_get()
        .get("lang")
        .and_then(|v| v.as_str())
        .map(|l| l == "ko")
        .unwrap_or(true);
    if ko {
        ("KMttt 열기", "프로그램 끝내기")
    } else {
        ("Open KMttt", "Quit")
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let (open_l, quit_l) = tray_labels();
    let open = MenuItem::with_id(app, "open", open_l, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit_l, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tb = TrayIconBuilder::with_id("main")
        .tooltip("KMttt Desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            // 끝내기는 화면에서 확인(실행 중인 에이전트가 끝난다는 안내) 후 서버까지 멈춘다.
            "quit" => {
                show_main(app);
                let _ = app.emit("app-quit-requested", ());
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tb = tb.icon(icon.clone());
    }
    tb.build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        // 이미 실행 중이면(트레이에 숨어 있어도) 새로 띄우지 않고 그 창을 보인다.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .setup(|app| {
            setup_tray(app)?;
            // 앱을 업데이트했으면 연동된 Claude·Codex의 KMttt 스킬도 이 버전으로 맞춘다(실패해도 무시).
            std::thread::spawn(|| {
                let _ = run_kmttt(&["integration", "sync-skills"]);
            });
            Ok(())
        })
        // 창 닫기(X): 바로 닫지 않고 화면에서 "창만 닫기 / 프로그램 끝내기"를 묻는다.
        .on_window_event(|w, ev| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                api.prevent_close();
                let _ = w.emit("app-close-requested", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            rpc,
            input,
            settings_read,
            settings_save,
            settings_raw,
            settings_save_raw,
            agent_kinds,
            app_info,
            prefs_get,
            prefs_set,
            pick_folder,
            open_url,
            clipboard_read,
            clipboard_write,
            usage_read,
            tokens_read,
            notify,
            usage_connect_claude,
            usage_disconnect_claude,
            window_hide,
            app_quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running KMttt Desktop");
}

#[cfg(test)]
mod tests {
    use super::ALLOWED;

    #[test]
    fn allowlist_has_no_pane_command_execution() {
        // 데스크톱 앱은 사람이 보는 화면이다. 다른 pane에 명령을 몰래 넣는 메서드는 열지 않는다.
        for m in [
            "pane.run",
            "pane.send",
            "pane.read",
            "hook.event",
            "hook.tool",
            "events.subscribe",
        ] {
            assert!(!ALLOWED.contains(&m), "{m} must not be exposed");
        }
    }
}
