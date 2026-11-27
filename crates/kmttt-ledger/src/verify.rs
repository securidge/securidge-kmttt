//! 기록 검증과 반출. 서버 없이 파일만으로 동작한다(`kmttt audit verify`).

use crate::{canonical, keys, record_hash, GENESIS, SIG_DOMAIN};
use anyhow::Result;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct VerifyError {
    pub file: String,
    /// 1부터 세는 줄 번호
    pub line: usize,
    pub seq: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerifyReport {
    pub ok: bool,
    pub segments: usize,
    pub records: u64,
    pub checkpoints: u64,
    /// 마지막 체크포인트 뒤에 서명되지 않은 레코드 수(오류는 아니지만 아직 보증되지 않음)
    pub unsigned_tail: u64,
    pub last_checkpoint_ms: Option<i64>,
    pub key_fingerprint: String,
    pub error: Option<VerifyError>,
}

/// 세그먼트 파일들(이름순 = 시작 seq 순).
pub fn segments(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with("ledger-") && name.ends_with(".jsonl") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// 최근 기록 가운데 `kind`인 것의 `data`를 오래된 것부터 최대 `max`개 돌려준다.
/// 암호화된 기록(data가 객체가 아닌 것)과 깨진 줄은 건너뛴다. 재시작 후 화면 복원용(검증 아님).
pub fn recent_data(dir: &Path, kind: &str, max: usize) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let Ok(segs) = segments(dir) else {
        return out;
    };
    for seg in segs.iter().rev() {
        let Ok(text) = std::fs::read_to_string(seg) else {
            continue;
        };
        for line in text.lines().rev() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if v.get("kind").and_then(|k| k.as_str()) == Some(kind) {
                if let Some(d) = v.get("data").filter(|d| d.is_object()) {
                    out.push(d.clone());
                    if out.len() >= max {
                        out.reverse();
                        return out;
                    }
                }
            }
        }
    }
    out.reverse();
    out
}

fn load_pub(dir: &Path) -> Result<Option<VerifyingKey>> {
    let p = dir.join("keys").join("sign.pub");
    let Ok(text) = std::fs::read_to_string(p) else {
        return Ok(None);
    };
    let raw = hex::decode(text.trim())?;
    let arr: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("sign.pub has wrong length"))?;
    Ok(Some(VerifyingKey::from_bytes(&arr)?))
}

/// 체인 전체를 검증한다. 첫 문제 위치에서 멈추고 그 자리를 알려준다.
pub fn verify(dir: &Path) -> Result<VerifyReport> {
    let mut rep = VerifyReport::default();
    let pubkey = load_pub(dir)?;
    if let Some(k) = &pubkey {
        rep.key_fingerprint = keys::fingerprint(&k.to_bytes());
    }
    let segs = segments(dir)?;
    rep.segments = segs.len();
    let mut prev = GENESIS.to_string();
    let mut expect_seq = 1u64;
    let mut last_signed = 0u64;
    let fail =
        |rep: &mut VerifyReport, file: &Path, line: usize, seq: Option<u64>, reason: String| {
            rep.ok = false;
            rep.error = Some(VerifyError {
                file: file.display().to_string(),
                line,
                seq,
                reason,
            });
        };
    for seg in &segs {
        let f = std::fs::File::open(seg)?;
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let lineno = i + 1;
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    fail(
                        &mut rep,
                        seg,
                        lineno,
                        None,
                        format!("damaged record (not JSON): {e}"),
                    );
                    return Ok(rep);
                }
            };
            let seq = v["seq"].as_u64();
            if seq != Some(expect_seq) {
                fail(&mut rep, seg, lineno, seq, format!("sequence gap: expected {expect_seq}, found {seq:?} (record removed or reordered)"));
                return Ok(rep);
            }
            if v["prev"].as_str() != Some(prev.as_str()) {
                fail(
                    &mut rep,
                    seg,
                    lineno,
                    seq,
                    "prev hash does not match the previous record (record removed or inserted)"
                        .into(),
                );
                return Ok(rep);
            }
            let stored = v["hash"].as_str().unwrap_or_default().to_string();
            let mut body = v.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("hash");
            }
            let calc = record_hash(&prev, &body);
            if calc != stored {
                fail(
                    &mut rep,
                    seg,
                    lineno,
                    seq,
                    "hash mismatch: record content was modified".into(),
                );
                return Ok(rep);
            }
            if v["kind"] == "checkpoint" {
                let d = &v["data"];
                let upto = d["upto"].as_u64().unwrap_or(0);
                let head = d["head"].as_str().unwrap_or_default();
                if upto != expect_seq - 1 || head != prev {
                    fail(
                        &mut rep,
                        seg,
                        lineno,
                        seq,
                        "checkpoint does not point at the previous record".into(),
                    );
                    return Ok(rep);
                }
                let Some(pk) = &pubkey else {
                    fail(
                        &mut rep,
                        seg,
                        lineno,
                        seq,
                        "public key file keys/sign.pub is missing".into(),
                    );
                    return Ok(rep);
                };
                if d["pub"].as_str() != Some(hex::encode(pk.to_bytes()).as_str()) {
                    fail(
                        &mut rep,
                        seg,
                        lineno,
                        seq,
                        "checkpoint was signed by a different key than keys/sign.pub".into(),
                    );
                    return Ok(rep);
                }
                let sig_ok = hex::decode(d["sig"].as_str().unwrap_or_default())
                    .ok()
                    .and_then(|b| <[u8; 64]>::try_from(b.as_slice()).ok())
                    .map(|b| {
                        pk.verify_strict(
                            format!("{SIG_DOMAIN}|{upto}|{head}").as_bytes(),
                            &Signature::from_bytes(&b),
                        )
                        .is_ok()
                    })
                    .unwrap_or(false);
                if !sig_ok {
                    fail(
                        &mut rep,
                        seg,
                        lineno,
                        seq,
                        "checkpoint signature is invalid".into(),
                    );
                    return Ok(rep);
                }
                rep.checkpoints += 1;
                rep.last_checkpoint_ms = v["ts_ms"].as_i64();
                last_signed = expect_seq;
            }
            prev = stored;
            rep.records = expect_seq;
            expect_seq += 1;
        }
    }
    rep.unsigned_tail = rep.records.saturating_sub(last_signed);
    rep.ok = true;
    Ok(rep)
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// 암호화된 본문을 풀어서 내보냄(enc.key 필요)
    pub decrypt: bool,
}

/// 검증 결과와 모든 레코드를 JSON 하나로. Securidge Trace 등이 읽을 수 있는 형식.
pub fn export(dir: &Path, opts: &ExportOptions) -> Result<Value> {
    let rep = verify(dir)?;
    let enc = if opts.decrypt {
        keys::load_enc(dir).ok()
    } else {
        None
    };
    let mut records = Vec::new();
    for seg in segments(dir)? {
        let f = std::fs::File::open(&seg)?;
        for line in BufReader::new(f).lines() {
            let line = line?;
            let Ok(mut v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let (Some(k), Some(c)) = (&enc, v["data"]["enc"].as_str()) {
                if let Ok(plain) = keys::decrypt(k, c) {
                    v["data"] = serde_json::from_slice(&plain).unwrap_or(Value::Null);
                }
            }
            records.push(v);
        }
    }
    Ok(json!({
        "format": "securidge-kmttt-ledger-export/1",
        "exported_ms": kmttt_proto::now_ms(),
        "verify": {
            "ok": rep.ok,
            "records": rep.records,
            "checkpoints": rep.checkpoints,
            "unsigned_tail": rep.unsigned_tail,
            "key_fingerprint": rep.key_fingerprint,
            "error": rep.error.as_ref().map(|e| json!({"file": e.file, "line": e.line, "seq": e.seq, "reason": e.reason})),
        },
        "records": records,
    }))
}

/// 표준 형식 확인용(테스트·디버그).
pub fn canonical_line(v: &Value) -> String {
    canonical(v)
}
