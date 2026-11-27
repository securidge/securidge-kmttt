//! 이그레스 프록시: 격리된 pane의 HTTP(S) 요청을 도메인 허용 목록으로 거른다.
//! HTTPS는 CONNECT 터널(내용은 보지 않음), HTTP는 절대 URI 요청을 지원한다.
//! 환경 변수 프록시라서 프록시를 무시하는 프로그램은 우회할 수 있다(restricted의 한계).

use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq)]
pub struct EgressEvent {
    pub host: String,
    pub port: u16,
    pub allowed: bool,
}

#[derive(Clone)]
pub struct EgressProxy {
    pub port: u16,
    allow: Arc<RwLock<Vec<String>>>,
}

impl EgressProxy {
    /// 127.0.0.1의 빈 포트에서 프록시를 시작한다.
    pub async fn start(
        allow: Vec<String>,
        events: mpsc::UnboundedSender<EgressEvent>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let allow = Arc::new(RwLock::new(allow));
        let allow2 = allow.clone();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    continue;
                };
                let allow = allow2.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let _ = handle(sock, allow, events).await;
                });
            }
        });
        Ok(Self { port, allow })
    }

    pub fn set_allow(&self, list: Vec<String>) {
        if let Ok(mut a) = self.allow.write() {
            *a = list;
        }
    }
}

/// 접미사 일치: "github.com"은 "api.github.com"도 허용한다.
pub fn host_allowed(host: &str, allow: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allow.iter().any(|d| {
        let d = d.trim_start_matches("*.").to_ascii_lowercase();
        host == d || host.ends_with(&format!(".{d}"))
    })
}

/// 프록시가 받은 요청 머리를 읽은 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    pub host: String,
    pub port: u16,
    /// 일반 HTTP면 upstream에 보낼 경로(`/…`), CONNECT면 None
    pub origin_form: Option<String>,
    pub version: String,
    /// 요청 줄의 바이트 길이(그 뒤는 그대로 upstream에 보낸다)
    pub head_len: usize,
}

/// 호스트 이름에 쓸 수 있는 글자만 받는다(영문·숫자·`.`·`-`·`_`, 대괄호 IPv6면 16진수와 `:`).
/// 허용 목록 비교와 실제 접속이 같은 이름을 보도록, 이상한 글자가 든 이름은 처음부터 거절한다.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && (host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
            || (host.contains(':')
                && host
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')))
}

/// 요청 머리(빈 줄까지)를 읽는다. 받아들일 수 없으면 돌려줄 응답을 준다.
pub fn parse_request(head: &[u8]) -> Result<Request, &'static [u8]> {
    const BAD: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\r\nkmttt egress proxy\n";
    // 요청 줄 끝: 첫 줄바꿈(앞의 \r은 빼고). 줄 뒤(\r\n 포함)는 그대로 upstream에 보낸다.
    let lf = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
    let line_end = if lf > 0 && head.get(lf - 1) == Some(&b'\r') {
        lf - 1
    } else {
        lf
    };
    let first = String::from_utf8_lossy(&head[..line_end]).to_string();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    let (host, port, origin_form) = if method.eq_ignore_ascii_case("CONNECT") {
        let (h, p) = split_host_port(&target, 443).ok_or(BAD)?;
        (h, p, None)
    } else if let Some(rest) = target.strip_prefix("http://") {
        let (authority, path) = rest
            .split_once('/')
            .map(|(a, p)| (a, format!("/{p}")))
            .unwrap_or((rest, "/".into()));
        let (h, p) = split_host_port(authority, 80).ok_or(BAD)?;
        (h, p, Some(path))
    } else {
        return Err(BAD);
    };
    if !valid_host(&host) || port == 0 || !version.starts_with("HTTP/") {
        return Err(BAD);
    }
    Ok(Request {
        method,
        host,
        port,
        origin_form,
        version,
        head_len: line_end,
    })
}

fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        return Some((h.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => {
            Some((h.to_string(), p.parse().ok()?))
        }
        _ => Some((s.to_string(), default_port)),
    }
}

async fn handle(
    mut client: TcpStream,
    allow: Arc<RwLock<Vec<String>>>,
    events: mpsc::UnboundedSender<EgressEvent>,
) -> std::io::Result<()> {
    // 요청 머리(빈 줄까지)를 최대 16KB 읽는다.
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = client.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() > 16 * 1024 {
            return client
                .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n")
                .await;
        }
    }
    let req = match parse_request(&head) {
        Ok(r) => r,
        Err(resp) => return client.write_all(resp).await,
    };
    let Request {
        method,
        host,
        port,
        origin_form,
        version,
        head_len,
    } = req;
    let allowed = allow
        .read()
        .map(|a| host_allowed(&host, &a))
        .unwrap_or(false);
    let _ = events.send(EgressEvent {
        host: host.clone(),
        port,
        allowed,
    });
    if !allowed {
        let body = format!("kmttt: egress to {host} is not in the sandbox allow list (policy.toml [sandbox] egress_allow)\n");
        let resp = format!("HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        return client.write_all(resp.as_bytes()).await;
    }
    let mut upstream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(_) => return client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await,
    };
    match origin_form {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?
        }
        Some(path) => {
            // 요청 줄을 origin-form으로 바꿔 보낸다. 머리 뒤에 따라온 본문도 그대로 보낸다.
            let rest = &head[head_len..];
            upstream
                .write_all(format!("{method} {path} {version}").as_bytes())
                .await?;
            upstream.write_all(rest).await?;
        }
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_parsing() {
        let r = parse_request(b"CONNECT api.example.com:8443 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(
            (r.host.as_str(), r.port, r.origin_form.clone()),
            ("api.example.com", 8443, None)
        );
        let r = parse_request(b"GET http://a.example/x?y HTTP/1.0\nHost: a\n\n").unwrap();
        assert_eq!(
            (
                r.host.as_str(),
                r.port,
                r.origin_form.as_deref(),
                r.version.as_str()
            ),
            ("a.example", 80, Some("/x?y"), "HTTP/1.0")
        );
        assert_eq!(r.head_len, b"GET http://a.example/x?y HTTP/1.0".len());
        let r = parse_request(b"CONNECT [::1]:443 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(r.host, "::1");
        // 허용 목록 비교와 실제 접속 이름이 어긋날 수 있는 글자는 거절
        for bad in [
            &b"CONNECT evil.com#.allowed.com:443 HTTP/1.1\r\n\r\n"[..],
            b"GET http://evil.com\x00.allowed.com/ HTTP/1.1\r\n\r\n",
            b"GET http://user@allowed.com/ HTTP/1.1\r\n\r\n",
            b"CONNECT allowed.com:0 HTTP/1.1\r\n\r\n",
            b"CONNECT allowed.com:99999 HTTP/1.1\r\n\r\n",
            b"GET /relative HTTP/1.1\r\n\r\n",
            b"CONNECT  HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                parse_request(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn suffix_matching() {
        let allow = vec!["github.com".to_string(), "*.npmjs.org".to_string()];
        assert!(host_allowed("github.com", &allow));
        assert!(host_allowed("api.github.com", &allow));
        assert!(host_allowed("registry.npmjs.org", &allow));
        assert!(!host_allowed("evilgithub.com", &allow));
        assert!(!host_allowed("example.com", &allow));
    }

    #[test]
    fn host_port() {
        assert_eq!(
            split_host_port("a.com:8443", 443),
            Some(("a.com".into(), 8443))
        );
        assert_eq!(split_host_port("a.com", 443), Some(("a.com".into(), 443)));
        assert_eq!(split_host_port("[::1]:80", 443), Some(("::1".into(), 80)));
    }

    #[tokio::test]
    async fn connect_denied_and_allowed() {
        // 대상 서버
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tport = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = target.accept().await.unwrap();
            s.write_all(b"pong").await.unwrap();
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = EgressProxy::start(vec!["127.0.0.1".into()], tx)
            .await
            .unwrap();

        let mut c = TcpStream::connect(("127.0.0.1", p.port)).await.unwrap();
        c.write_all(b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        c.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
        assert!(!rx.recv().await.unwrap().allowed);

        let mut c = TcpStream::connect(("127.0.0.1", p.port)).await.unwrap();
        c.write_all(format!("CONNECT 127.0.0.1:{tport} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut resp = vec![0u8; 64];
        let mut got = Vec::new();
        while !String::from_utf8_lossy(&got).contains("pong") {
            let n = c.read(&mut resp).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&resp[..n]);
        }
        let s = String::from_utf8_lossy(&got);
        assert!(s.starts_with("HTTP/1.1 200"), "{s}");
        assert!(s.contains("pong"));
        assert!(rx.recv().await.unwrap().allowed);
    }
}
