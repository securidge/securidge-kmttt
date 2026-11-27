//! 외부 반출: 각 레코드를 다른 파일과 UDP syslog(RFC 5424)로도 보낸다.
//! 반출 실패가 기록 자체를 막지는 않는다.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

pub struct Sinks {
    mirror: Option<PathBuf>,
    syslog: Option<(UdpSocket, String)>,
    host: String,
}

impl Sinks {
    pub fn new(mirror: Option<&Path>, syslog: Option<&str>) -> Self {
        let syslog = syslog.filter(|s| !s.is_empty()).and_then(|addr| {
            let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
            Some((sock, addr.to_string()))
        });
        Self {
            mirror: mirror.map(Path::to_path_buf),
            syslog,
            host: hostname(),
        }
    }

    pub fn send(&self, line: &str) {
        if let Some(p) = &self.mirror {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if let Some((sock, addr)) = &self.syslog {
            // <110> = facility 13(log audit) * 8 + severity 6(info)
            let ts = rfc3339_millis(kmttt_now_ms());
            let msg = format!(
                "<110>1 {ts} {} kmttt {} audit - {}",
                self.host,
                std::process::id(),
                line.trim_end()
            );
            let _ = sock.send_to(msg.as_bytes(), addr);
        }
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "-".into())
        .chars()
        .filter(|c| c.is_ascii_graphic())
        .take(64)
        .collect::<String>()
}

fn kmttt_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 유닉스 밀리초 → `YYYY-MM-DDThh:mm:ss.mmmZ` (UTC). 날짜 계산은 H. Hinnant의 civil_from_days.
pub fn rfc3339_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_known_points() {
        use super::rfc3339_millis;
        assert_eq!(rfc3339_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_millis(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(
            rfc3339_millis(1_795_679_215_751),
            "2026-11-26T07:46:55.751Z"
        );
        assert_eq!(rfc3339_millis(-1), "1969-12-31T23:59:59.999Z");
    }

    use super::*;

    #[test]
    fn mirror_and_syslog() {
        let d = tempfile::tempdir().unwrap();
        let mirror = d.path().join("mirror.jsonl");
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let addr = rx.local_addr().unwrap().to_string();
        let s = Sinks::new(Some(&mirror), Some(&addr));
        s.send("{\"seq\":1}\n");
        assert_eq!(std::fs::read_to_string(&mirror).unwrap(), "{\"seq\":1}\n");
        let mut buf = [0u8; 1024];
        let n = rx.recv(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        assert!(msg.starts_with("<110>1 "), "{msg}");
        assert!(msg.ends_with("{\"seq\":1}"), "{msg}");
    }
}
