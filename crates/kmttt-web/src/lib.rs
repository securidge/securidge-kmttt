//! 로컬 웹 관제 콘솔(`kmttt console`).
//!
//! - 127.0.0.1에만 바인딩, 실행마다 새 랜덤 토큰. API는 `X-KMttt-Token` 헤더가 맞아야 한다.
//! - `Host`가 127.0.0.1/localhost:포트가 아니면 거부(DNS 리바인딩 방지),
//!   POST는 `Origin`까지 확인(CSRF 방지). 엄격한 CSP, 인라인 스크립트 없음.
//! - 기본은 읽기 전용. `allow_actions`일 때만 승인·거부 버튼이 동작한다.
//! - 콘솔 프로세스는 pane 밖 사용자 프로세스로 서버에 붙으므로, 서버 쪽 정책상 사람으로 취급된다.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use kmttt_core::client::RpcClient;
use rand::RngCore;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

const INDEX: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const APP_CSS: &str = include_str!("../assets/app.css");
const FAVICON: &str = include_str!("../assets/favicon.svg");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[derive(Clone)]
pub struct ConsoleOptions {
    pub port: u16,
    pub allow_actions: bool,
    pub lang: String,
}

#[derive(Clone)]
struct AppState {
    client: Arc<RpcClient>,
    token: String,
    port: u16,
    allow_actions: bool,
    lang: String,
}

/// 실행 중인 콘솔.
pub struct Console {
    pub addr: SocketAddr,
    pub token: String,
    pub url: String,
    pub task: tokio::task::JoinHandle<()>,
}

pub fn new_token() -> String {
    let mut b = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

/// 길이가 같으면 끝까지 비교하는 비교(시간 차로 토큰을 알아내지 못하게).
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub async fn serve(client: Arc<RpcClient>, opts: ConsoleOptions) -> anyhow::Result<Console> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", opts.port)).await?;
    let addr = listener.local_addr()?;
    let token = new_token();
    let state = AppState {
        client,
        token: token.clone(),
        port: addr.port(),
        allow_actions: opts.allow_actions,
        lang: if opts.lang == "ko" {
            "ko".into()
        } else {
            "en".into()
        },
    };
    let app = router(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(Console {
        addr,
        url: format!("http://127.0.0.1:{}/?t={token}", addr.port()),
        token,
        task,
    })
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route(
            "/app.js",
            get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/app.css",
            get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/favicon.svg",
            get(|| async { asset(FAVICON, "image/svg+xml") }),
        )
        .route("/api/snapshot", get(snapshot))
        .route("/api/approval/{id}/{verdict}", post(decide))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn asset(body: &'static str, ctype: &'static str) -> Response {
    ([(header::CONTENT_TYPE, ctype)], body).into_response()
}

/// Host·Origin·토큰 검사와 보안 헤더.
async fn guard(State(s): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let allowed_hosts = [
        format!("127.0.0.1:{}", s.port),
        format!("localhost:{}", s.port),
    ];
    if !allowed_hosts.contains(&host) {
        return (StatusCode::MISDIRECTED_REQUEST, "bad host").into_response();
    }
    let path = req.uri().path().to_string();
    if req.method() == axum::http::Method::POST {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        let ok = allowed_hosts
            .iter()
            .any(|h| origin == format!("http://{h}"));
        if !ok {
            return (StatusCode::FORBIDDEN, "bad origin").into_response();
        }
    }
    let token_ok = if path.starts_with("/api/") {
        req.headers()
            .get("x-kmttt-token")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|t| ct_eq(t, &s.token))
    } else if path == "/" {
        req.uri()
            .query()
            .unwrap_or_default()
            .split('&')
            .any(|kv| kv.strip_prefix("t=").is_some_and(|t| ct_eq(t, &s.token)))
    } else {
        true
    };
    if !token_ok {
        return (StatusCode::UNAUTHORIZED, "missing or wrong token").into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    res
}

async fn index(State(s): State<AppState>) -> Response {
    let html = INDEX
        .replace("{{LANG}}", &s.lang)
        .replace("{{ACTIONS}}", if s.allow_actions { "1" } else { "0" })
        .replace("{{VERSION}}", kmttt_core::VERSION);
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

async fn snapshot(State(s): State<AppState>) -> Response {
    match s.client.call("control.snapshot", json!({})).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

async fn decide(
    State(s): State<AppState>,
    Path((id, verdict)): Path<(u64, String)>,
    _h: HeaderMap,
) -> Response {
    if !s.allow_actions {
        return (
            StatusCode::FORBIDDEN,
            "console is read-only (start with --allow-actions)",
        )
            .into_response();
    }
    let allow = match verdict.as_str() {
        "allow" => true,
        "deny" => false,
        _ => return (StatusCode::BAD_REQUEST, "verdict must be allow or deny").into_response(),
    };
    match s
        .client
        .call("approval.decide", json!({"id": id, "allow": allow}))
        .await
    {
        Ok(_) => axum::Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
    }

    #[test]
    fn token_is_random_hex() {
        let a = new_token();
        assert_eq!(a.len(), 48);
        assert_ne!(a, new_token());
    }

    #[test]
    fn assets_have_no_inline_script() {
        assert!(!INDEX.contains("<script>"));
        assert!(INDEX.contains("src=\"/app.js\""));
        assert!(APP_JS.contains("textContent"));
        assert!(
            !APP_JS.contains("innerHTML ="),
            "use textContent to avoid XSS"
        );
    }
}
