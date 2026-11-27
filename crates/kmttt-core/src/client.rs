//! JSON-RPC 클라이언트. 로컬 소켓/named pipe 또는 SSH 표준 입출력 위에서 동작한다.

use anyhow::{anyhow, Context, Result};
use kmttt_proto::{read_frame, write_frame, RpcError, RpcMessage};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// 서버와의 연결 하나.
pub struct RpcClient {
    next_id: AtomicU64,
    pending: Pending,
    out: mpsc::UnboundedSender<Vec<u8>>,
    /// 연결이 끊기면 true
    closed: Arc<std::sync::atomic::AtomicBool>,
    _child: Option<tokio::process::Child>,
}

/// 서버에서 온 알림(method, params).
pub type Notifications = mpsc::UnboundedReceiver<(String, Value)>;

impl RpcClient {
    /// 임의의 읽기·쓰기 스트림 위에 클라이언트를 만든다.
    pub fn from_io<R, W>(mut reader: R, mut writer: W) -> (Self, Notifications)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (note_tx, note_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(buf) = out_rx.recv().await {
                if write_frame(&mut writer, &buf).await.is_err() {
                    break;
                }
            }
        });
        let pending2 = pending.clone();
        let closed2 = closed.clone();
        tokio::spawn(async move {
            while let Ok(Some(buf)) = read_frame(&mut reader).await {
                let Ok(msg) = RpcMessage::from_bytes(&buf) else {
                    continue;
                };
                if msg.is_response() {
                    let id = msg.id.unwrap_or(0);
                    let tx = pending2.lock().ok().and_then(|mut p| p.remove(&id));
                    if let Some(tx) = tx {
                        let r = match msg.error {
                            Some(e) => Err(e),
                            None => Ok(msg.result.unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(r);
                    }
                } else if let Some(m) = msg.method {
                    let _ = note_tx.send((m, msg.params.unwrap_or(Value::Null)));
                }
            }
            closed2.store(true, Ordering::SeqCst);
            // 남은 요청들에 연결 끊김을 알린다.
            if let Ok(mut p) = pending2.lock() {
                for (_, tx) in p.drain() {
                    let _ = tx.send(Err(RpcError::internal("connection closed")));
                }
            }
        });
        (
            Self {
                next_id: AtomicU64::new(1),
                pending,
                out: out_tx,
                closed,
                _child: None,
            },
            note_rx,
        )
    }

    /// 로컬 세션 서버에 연결한다.
    pub async fn connect_local(session: &str) -> Result<(Self, Notifications)> {
        let addr = crate::paths::socket_path(session);
        #[cfg(unix)]
        {
            let s = tokio::net::UnixStream::connect(&addr)
                .await
                .with_context(|| format!("connect {addr}"))?;
            let (r, w) = s.into_split();
            Ok(Self::from_io(r, w))
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            let mut tries = 0;
            let pipe = loop {
                match ClientOptions::new().open(&addr) {
                    Ok(p) => break p,
                    // ERROR_PIPE_BUSY(231): 잠시 뒤 다시
                    Err(e) if e.raw_os_error() == Some(231) && tries < 50 => {
                        tries += 1;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(e) => return Err(anyhow!(e).context(format!("connect {addr}"))),
                }
            };
            let (r, w) = tokio::io::split(pipe);
            Ok(Self::from_io(r, w))
        }
    }

    /// `ssh <target> kmttt server stdio --session <s>`로 원격 서버에 연결한다.
    pub async fn connect_ssh(
        target: &str,
        remote_session: &str,
        remote_bin: &str,
    ) -> Result<(Self, Notifications)> {
        let (host, port) = parse_ssh_target(target);
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-T").arg("-o").arg("BatchMode=yes");
        if let Some(p) = port {
            cmd.arg("-p").arg(p.to_string());
        }
        cmd.arg(&host)
            .arg(remote_bin)
            .arg("server")
            .arg("stdio")
            .arg("--session")
            .arg(remote_session)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().context("spawn ssh")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("ssh stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("ssh stdout"))?;
        let (mut c, n) = Self::from_io(stdout, stdin);
        c._child = Some(child);
        Ok((c, n))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 요청을 보내고 응답을 기다린다.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_timeout(method, params, Duration::from_secs(600))
            .await
    }

    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::internal("connection closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, tx);
        }
        let msg = RpcMessage::request(id, method, params);
        self.out
            .send(msg.to_bytes())
            .map_err(|_| RpcError::internal("connection closed"))?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(RpcError::internal("connection closed")),
            Err(_) => {
                if let Ok(mut p) = self.pending.lock() {
                    p.remove(&id);
                }
                Err(RpcError::new(
                    kmttt_proto::codes::TIMEOUT,
                    "request timed out",
                ))
            }
        }
    }

    /// 응답이 필요 없는 알림(입력 전달 등).
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self
            .out
            .send(RpcMessage::notification(method, params).to_bytes());
    }
}

/// "user@host", "ssh://user@host:2222" → ("user@host", Some(2222))
pub fn parse_ssh_target(t: &str) -> (String, Option<u16>) {
    let t = t.strip_prefix("ssh://").unwrap_or(t);
    if let Some((h, p)) = t.rsplit_once(':') {
        if let Ok(port) = p.parse::<u16>() {
            return (h.to_string(), Some(port));
        }
    }
    (t.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ssh_targets() {
        assert_eq!(parse_ssh_target("me@box"), ("me@box".into(), None));
        assert_eq!(
            parse_ssh_target("ssh://me@box:2222"),
            ("me@box".into(), Some(2222))
        );
        assert_eq!(parse_ssh_target("workbox"), ("workbox".into(), None));
    }

    #[tokio::test]
    async fn request_response_and_notification() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client_io);
        let (mut sr, mut sw) = tokio::io::split(server_io);
        let (c, mut notes) = RpcClient::from_io(cr, cw);
        // 가짜 서버: 요청마다 메서드 이름을 돌려주고, 알림 하나를 먼저 보낸다.
        tokio::spawn(async move {
            while let Ok(Some(buf)) = read_frame(&mut sr).await {
                let m = RpcMessage::from_bytes(&buf).unwrap();
                let n = RpcMessage::notification("event", json!({"k": 1}));
                write_frame(&mut sw, &n.to_bytes()).await.unwrap();
                let r = if m.method.as_deref() == Some("bad") {
                    Err(RpcError::not_found("nope"))
                } else {
                    Ok(json!(m.method))
                };
                write_frame(&mut sw, &RpcMessage::response(m.id.unwrap(), r).to_bytes())
                    .await
                    .unwrap();
            }
        });
        assert_eq!(
            c.call("pane.list", json!({})).await.unwrap(),
            json!("pane.list")
        );
        assert_eq!(
            c.call("bad", json!({})).await.unwrap_err().code,
            kmttt_proto::codes::NOT_FOUND
        );
        let (m, p) = notes.recv().await.unwrap();
        assert_eq!(m, "event");
        assert_eq!(p["k"], 1);
    }
}
