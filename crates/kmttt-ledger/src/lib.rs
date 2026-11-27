//! F2 위변조 방지 감사 기록.
//!
//! - 추가 전용 JSONL 세그먼트(`ledger-<첫 seq>.jsonl`), 8MB마다 새 파일. 체인은 파일을 넘어 이어진다.
//! - 레코드마다 `hash = SHA-256(prev_hash ‖ canonical_json(본문))`.
//! - 일정 시간·건수마다 Ed25519로 서명한 체크포인트 레코드를 체인에 넣는다.
//! - 서명 키는 Windows에서 DPAPI로 보호하고, 그 밖에서는 권한 0600 파일에 둔다.
//! - 선택: 본문(`data`)을 XChaCha20-Poly1305로 암호화해 저장한다. 해시는 저장된 형태 기준.

mod canon;
mod keys;
mod sink;
pub mod verify;

pub use canon::canonical;
pub use verify::{verify, ExportOptions, VerifyError, VerifyReport};

use anyhow::{Context, Result};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 서명 대상 문자열의 형식 버전.
pub const SIG_DOMAIN: &str = "kmttt-ledger-v1";
const SEGMENT_MAX: u64 = 8 * 1024 * 1024;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Full,
    Metadata,
    Off,
}

impl Level {
    pub fn parse(s: &str) -> Self {
        match s {
            "full" => Level::Full,
            "off" => Level::Off,
            _ => Level::Metadata,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Full => "full",
            Level::Metadata => "metadata",
            Level::Off => "off",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LedgerOptions {
    pub level: Level,
    pub encrypt: bool,
    pub checkpoint_secs: u64,
    pub checkpoint_records: u64,
    pub mirror_file: Option<PathBuf>,
    pub syslog_udp: Option<String>,
}

impl Default for LedgerOptions {
    fn default() -> Self {
        Self {
            level: Level::Metadata,
            encrypt: false,
            checkpoint_secs: 60,
            checkpoint_records: 1000,
            mirror_file: None,
            syslog_udp: None,
        }
    }
}

/// 열린 감사 기록.
pub struct Ledger {
    dir: PathBuf,
    opts: LedgerOptions,
    file: Option<File>,
    file_size: u64,
    seq: u64,
    prev: String,
    since_checkpoint: u64,
    last_checkpoint: Instant,
    last_checkpoint_ms: Option<i64>,
    signing: SigningKey,
    enc_key: Option<[u8; 32]>,
    sinks: sink::Sinks,
    /// 열 때 발견한 문제(잘린 마지막 줄 등)
    pub open_warnings: Vec<String>,
    /// 살아 있는 동안 기록 폴더의 단독 쓰기 잠금을 쥔다.
    _writer_lock: File,
}

impl Ledger {
    pub fn open(dir: &Path, opts: LedgerOptions) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        restrict_dir(dir);
        // 쓰는 쪽은 하나뿐이어야 체인이 갈라지지 않는다(검증은 잠금 없이 읽기만 한다).
        let writer_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("writer.lock"))
            .with_context(|| format!("open {}", dir.join("writer.lock").display()))?;
        if writer_lock.try_lock().is_err() {
            anyhow::bail!(
                "ledger {} is already open by another kmttt server",
                dir.display()
            );
        }
        let signing = keys::load_or_create_signing(dir)?;
        let enc_key = if opts.encrypt {
            Some(keys::load_or_create_enc(dir)?)
        } else {
            None
        };
        let sinks = sink::Sinks::new(opts.mirror_file.as_deref(), opts.syslog_udp.as_deref());
        let mut l = Self {
            dir: dir.to_path_buf(),
            opts,
            file: None,
            file_size: 0,
            seq: 0,
            prev: GENESIS.into(),
            since_checkpoint: 0,
            last_checkpoint: Instant::now(),
            last_checkpoint_ms: None,
            signing,
            enc_key,
            sinks,
            open_warnings: Vec::new(),
            _writer_lock: writer_lock,
        };
        l.resume()?;
        Ok(l)
    }

    pub fn level(&self) -> Level {
        self.opts.level
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn records(&self) -> u64 {
        self.seq
    }

    pub fn last_checkpoint_ms(&self) -> Option<i64> {
        self.last_checkpoint_ms
    }

    /// 공개키 지문(SHA-256 앞 16자리). 사용자가 따로 적어 두면 키 바꿔치기를 알아챌 수 있다.
    pub fn key_fingerprint(&self) -> String {
        keys::fingerprint(&self.signing.verifying_key().to_bytes())
    }

    /// 마지막 세그먼트를 읽어 체인을 이어 붙일 자리를 찾는다.
    fn resume(&mut self) -> Result<()> {
        let segs = verify::segments(&self.dir)?;
        let Some(last) = segs.last() else {
            return Ok(());
        };
        let f = File::open(last)?;
        let mut last_good: Option<Value> = None;
        let mut bad_tail = false;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(v) if v.get("hash").is_some() => {
                    if bad_tail {
                        // 깨진 줄 뒤에 다시 정상 줄이 오면 그대로 둔다(verify가 보고함).
                        bad_tail = false;
                    }
                    last_good = Some(v)
                }
                _ => bad_tail = true,
            }
        }
        if let Some(v) = last_good {
            self.seq = v["seq"].as_u64().unwrap_or(0);
            self.prev = v["hash"].as_str().unwrap_or(GENESIS).to_string();
            if v["kind"] == "checkpoint" {
                self.last_checkpoint_ms = v["ts_ms"].as_i64();
            }
        }
        if bad_tail {
            // 잘린 마지막 줄이 있는 파일에는 더 쓰지 않고 새 세그먼트를 연다.
            self.open_warnings.push(format!(
                "{} ends with a damaged record; continuing in a new segment",
                last.display()
            ));
        } else {
            let size = std::fs::metadata(last)?.len();
            if size < SEGMENT_MAX {
                self.file = Some(OpenOptions::new().append(true).open(last)?);
                self.file_size = size;
            }
        }
        Ok(())
    }

    fn ensure_file(&mut self) -> Result<()> {
        if self.file.is_some() && self.file_size < SEGMENT_MAX {
            return Ok(());
        }
        let path = self.dir.join(format!("ledger-{:012}.jsonl", self.seq + 1));
        let f = OpenOptions::new().create(true).append(true).open(&path)?;
        restrict_file(&path);
        self.file = Some(f);
        self.file_size = 0;
        Ok(())
    }

    /// 레코드 하나를 덧붙이고 해시를 돌려준다. 수준이 off면 아무것도 하지 않는다.
    pub fn append(&mut self, kind: &str, source: &str, data: Value) -> Result<Option<String>> {
        if self.opts.level == Level::Off {
            return Ok(None);
        }
        let h = self.write_record(kind, source, data)?;
        self.since_checkpoint += 1;
        if self.since_checkpoint >= self.opts.checkpoint_records {
            self.checkpoint()?;
        }
        Ok(Some(h))
    }

    /// 시간 기준 체크포인트가 필요하면 만든다(서버가 주기적으로 부름).
    pub fn tick(&mut self) -> Result<()> {
        if self.opts.level != Level::Off
            && self.since_checkpoint > 0
            && self.last_checkpoint.elapsed().as_secs() >= self.opts.checkpoint_secs
        {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// 지금까지의 체인 머리를 서명한 체크포인트를 남긴다.
    pub fn checkpoint(&mut self) -> Result<()> {
        if self.opts.level == Level::Off {
            return Ok(());
        }
        let upto = self.seq;
        let head = self.prev.clone();
        let msg = format!("{SIG_DOMAIN}|{upto}|{head}");
        let sig = self.signing.sign(msg.as_bytes());
        let data = json!({
            "upto": upto,
            "head": head,
            "sig": hex::encode(sig.to_bytes()),
            "pub": hex::encode(self.signing.verifying_key().to_bytes()),
        });
        // 체크포인트 본문은 암호화하지 않는다(검증에 필요).
        let enc = self.enc_key.take();
        let r = self.write_record("checkpoint", "ledger", data);
        self.enc_key = enc;
        r?;
        self.since_checkpoint = 0;
        self.last_checkpoint = Instant::now();
        self.last_checkpoint_ms = Some(kmttt_proto::now_ms());
        Ok(())
    }

    fn write_record(&mut self, kind: &str, source: &str, data: Value) -> Result<String> {
        self.ensure_file()?;
        let seq = self.seq + 1;
        let ts_ms = kmttt_proto::now_ms();
        let data = match &self.enc_key {
            Some(k) => json!({"enc": keys::encrypt(k, canonical(&data).as_bytes())?}),
            None => data,
        };
        let body = json!({
            "seq": seq,
            "ts_ms": ts_ms,
            "kind": kind,
            "source": source,
            "data": data,
            "prev": self.prev,
        });
        let hash = record_hash(&self.prev, &body);
        let mut full = body;
        full["hash"] = Value::String(hash.clone());
        let mut line = canonical(&full);
        line.push('\n');
        let f = self.file.as_mut().expect("ensure_file");
        f.write_all(line.as_bytes())?;
        f.flush()?;
        self.file_size += line.len() as u64;
        self.seq = seq;
        self.prev = hash.clone();
        self.sinks.send(&line);
        Ok(hash)
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        if self.since_checkpoint > 0 {
            let _ = self.checkpoint();
        }
    }
}

/// `SHA-256(prev ‖ canonical(body))` — body에는 hash 칸이 없어야 한다.
pub fn record_hash(prev: &str, body: &Value) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(canonical(body).as_bytes());
    hex::encode(h.finalize())
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) {}

#[cfg(unix)]
pub(crate) fn restrict_file(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
pub(crate) fn restrict_file(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> LedgerOptions {
        LedgerOptions {
            level: Level::Full,
            checkpoint_records: 5,
            ..Default::default()
        }
    }

    #[test]
    fn append_verify_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            for i in 0..12 {
                l.append("pane.output", "w1:p1", json!({"i": i, "text": "hello"}))
                    .unwrap();
            }
        } // drop → 마지막 체크포인트
        let r = verify(dir.path()).unwrap();
        assert!(r.ok, "{r:?}");
        // 12건 + 체크포인트 3건(5, 10번째 뒤, 그리고 drop 때)
        assert_eq!(r.records, 15);
        assert_eq!(r.checkpoints, 3);
        assert_eq!(r.unsigned_tail, 0);
        // 다시 열어 이어 쓴다.
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            assert_eq!(l.records(), 15);
            l.append("x", "s", json!({})).unwrap();
        }
        let r = verify(dir.path()).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.records, 17);
    }

    #[test]
    fn tampering_is_located() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            for i in 0..8 {
                l.append("api.call", "user", json!({"i": i})).unwrap();
            }
        }
        let seg = verify::segments(dir.path()).unwrap().remove(0);
        let text = std::fs::read_to_string(&seg).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        lines[3] = lines[3].replace("\"i\":3", "\"i\":33");
        std::fs::write(&seg, lines.join("\n") + "\n").unwrap();
        let r = verify(dir.path()).unwrap();
        assert!(!r.ok);
        let e = r.error.unwrap();
        assert_eq!(e.line, 4);
        assert!(e.reason.contains("hash"), "{}", e.reason);
    }

    #[test]
    fn deleted_line_breaks_chain() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            for i in 0..4 {
                l.append("api.call", "user", json!({"i": i})).unwrap();
            }
        }
        let seg = verify::segments(dir.path()).unwrap().remove(0);
        let text = std::fs::read_to_string(&seg).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 1)
            .map(|(_, l)| *l)
            .collect();
        std::fs::write(&seg, kept.join("\n") + "\n").unwrap();
        let r = verify(dir.path()).unwrap();
        assert!(!r.ok);
        assert_eq!(r.error.unwrap().line, 2);
    }

    #[test]
    fn forged_signature_detected() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            l.append("a", "b", json!({})).unwrap();
        }
        // 다른 키로 서명한 것처럼 공개키 파일을 바꾼다.
        let other = SigningKey::from_bytes(&[7u8; 32]);
        std::fs::write(
            dir.path().join("keys").join("sign.pub"),
            hex::encode(other.verifying_key().to_bytes()),
        )
        .unwrap();
        let r = verify(dir.path()).unwrap();
        assert!(!r.ok);
        assert!(
            r.error.unwrap().reason.contains("key"),
            "public key mismatch should be reported"
        );
    }

    #[test]
    fn encrypted_records_verify_and_export() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(
                dir.path(),
                LedgerOptions {
                    encrypt: true,
                    ..opts()
                },
            )
            .unwrap();
            l.append("pane.output", "w1:p1", json!({"text": "top secret words"}))
                .unwrap();
        }
        let raw = std::fs::read_to_string(&verify::segments(dir.path()).unwrap()[0]).unwrap();
        assert!(!raw.contains("top secret words"));
        assert!(verify(dir.path()).unwrap().ok);
        let out = verify::export(dir.path(), &ExportOptions { decrypt: true }).unwrap();
        assert!(out.to_string().contains("top secret words"));
    }

    #[test]
    fn off_level_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(
                dir.path(),
                LedgerOptions {
                    level: Level::Off,
                    ..opts()
                },
            )
            .unwrap();
            assert!(l.append("a", "b", json!({})).unwrap().is_none());
        }
        assert!(verify::segments(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn truncated_tail_starts_new_segment() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(dir.path(), opts()).unwrap();
            l.append("a", "b", json!({"x": 1})).unwrap();
        }
        let seg = verify::segments(dir.path()).unwrap().remove(0);
        let mut f = OpenOptions::new().append(true).open(&seg).unwrap();
        f.write_all(b"{\"seq\":99,\"half").unwrap();
        drop(f);
        let l = Ledger::open(dir.path(), opts()).unwrap();
        assert_eq!(l.open_warnings.len(), 1);
        drop(l);
        // verify는 잘린 줄을 보고한다.
        let r = verify(dir.path()).unwrap();
        assert!(!r.ok);
    }
}
