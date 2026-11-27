//! 필터 체인의 실제 필터: 마스킹(F3) → 기록(F2).

use kmttt_core::filter::{map_strings, Filter, FilterAction};
use kmttt_core::CoreEvent;
use kmttt_ledger::Ledger;
use kmttt_redact::{Finding, Redactor};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;

pub struct RedactFilter {
    r: Arc<RwLock<Redactor>>,
    findings: mpsc::UnboundedSender<(String, Finding)>,
}

impl RedactFilter {
    pub fn new(
        r: Arc<RwLock<Redactor>>,
        findings: mpsc::UnboundedSender<(String, Finding)>,
    ) -> Self {
        Self { r, findings }
    }
}

impl Filter for RedactFilter {
    fn name(&self) -> &'static str {
        "redact"
    }
    fn process(&self, ev: &mut CoreEvent) -> FilterAction {
        let Ok(r) = self.r.read() else {
            // 마스킹을 못 하면 내보내지 않는다(실패 시 닫힘).
            return FilterAction::Drop;
        };
        let mut n = 0;
        let source = ev.source.clone();
        map_strings(&mut ev.data, &mut |s| {
            let (out, found) = r.redact(s);
            if !found.is_empty() {
                n += found.len();
                for f in found {
                    let _ = self.findings.send((source.clone(), f));
                }
                *s = out;
            }
        });
        if n > 0 {
            ev.tags.push(format!("redacted:{n}"));
        }
        FilterAction::Continue
    }
}

pub struct LedgerFilter {
    l: Arc<Mutex<Option<Ledger>>>,
}

impl LedgerFilter {
    pub fn new(l: Arc<Mutex<Option<Ledger>>>) -> Self {
        Self { l }
    }
}

impl Filter for LedgerFilter {
    fn name(&self) -> &'static str {
        "ledger"
    }
    fn process(&self, ev: &mut CoreEvent) -> FilterAction {
        if let Ok(mut g) = self.l.lock() {
            if let Some(l) = g.as_mut() {
                let mut data = ev.data.clone();
                if !ev.tags.is_empty() {
                    data["_tags"] = serde_json::json!(ev.tags);
                }
                if let Err(e) = l.append(&ev.kind, &ev.source, data) {
                    tracing::error!("ledger append failed: {e}");
                }
            }
        }
        FilterAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmttt_core::FilterChain;
    use serde_json::json;

    #[test]
    fn secrets_never_reach_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Mutex::new(Some(
            Ledger::open(
                dir.path(),
                kmttt_ledger::LedgerOptions {
                    level: kmttt_ledger::Level::Full,
                    ..Default::default()
                },
            )
            .unwrap(),
        )));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut chain = FilterChain::new();
        chain.push(Box::new(RedactFilter::new(
            Arc::new(RwLock::new(Redactor::default())),
            tx,
        )));
        chain.push(Box::new(LedgerFilter::new(ledger.clone())));
        let out = chain
            .process(CoreEvent::new(
                "pane.output",
                "w1:p1",
                json!({"text": "key=AKIAABCDEFGHIJKLMNOP"}),
            ))
            .unwrap();
        assert!(!out.data.to_string().contains("AKIAABCDEFGHIJKLMNOP"));
        assert!(rx.try_recv().is_ok());
        drop(ledger.lock().unwrap().take());
        let all: String = kmttt_ledger::verify::segments(dir.path())
            .unwrap()
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        assert!(all.contains("REDACTED"));
        assert!(!all.contains("AKIAABCDEFGHIJKLMNOP"));
    }
}
