//! Throttle for `hook.notify_chain` audit records.
//!
//! The RPC is allowed for every caller (Hook class) so that Codex's notify can
//! always report. A pane agent could also call it directly in a loop, so each
//! caller gets a sliding 60-second window of `limit` records (policy
//! `defaults.rate_per_min`):
//! - Within the limit every phase is recorded as before.
//! - Over the limit `started`/`completed` are only counted. One summary record
//!   with the counts and the first/last suppressed time is written when the
//!   suppression window ends, before the caller's next accepted record, or when
//!   the server stops.
//! - `failed`/`timed_out` have their own, separate cap of [`FAILURE_LIMIT`]
//!   per caller per window, so the first failures always stand out. Every
//!   failure call consumes that cap, recorded or not, so a flood stays capped.
//!   Over the cap they are not lost: the summary counts them (`failed N,
//!   timed_out M`). The toast is skipped whenever either limit is exceeded.
use kmttt_proto::Severity;
use std::collections::{HashMap, VecDeque};

pub const WINDOW_MS: i64 = 60_000;
/// Individually recorded failures (failed + timed_out) per caller per window.
pub const FAILURE_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Started,
    Completed,
    Failed,
    TimedOut,
}

impl Phase {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "started" => Self::Started,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "timed_out" => Self::TimedOut,
            _ => return None,
        })
    }

    fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::TimedOut)
    }

    // Fixed vocabulary: clients cannot choose the audit kind or prose.
    pub fn record(self) -> (Severity, &'static str, &'static str) {
        match self {
            Self::Started => (
                Severity::Info,
                "integration.notify_chain.started",
                "Codex previous notifier execution started",
            ),
            Self::Completed => (
                Severity::Info,
                "integration.notify_chain.completed",
                "Codex previous notifier completed successfully",
            ),
            Self::Failed => (
                Severity::Notice,
                "integration.notify_chain.failed",
                "Codex previous notifier execution failed",
            ),
            Self::TimedOut => (
                Severity::Notice,
                "integration.notify_chain.timed_out",
                "Codex previous notifier exceeded its execution deadline",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub severity: Severity,
    pub kind: &'static str,
    pub caller: String,
    pub target: String,
    pub detail: String,
    pub toast: bool,
}

#[derive(Debug, Default)]
struct Pending {
    started: u64,
    completed: u64,
    failed: u64,
    timed_out: u64,
    first_ms: i64,
    last_ms: i64,
    caller: String,
    target: String,
}

impl Pending {
    fn total(&self) -> u64 {
        self.started + self.completed + self.failed + self.timed_out
    }

    fn summary(&self) -> Record {
        // Counted failures keep their severity in the summary.
        let severity = if self.failed + self.timed_out > 0 {
            Severity::Notice
        } else {
            Severity::Info
        };
        Record {
            severity,
            kind: "integration.notify_chain.suppressed",
            caller: self.caller.clone(),
            target: self.target.clone(),
            detail: format!(
                "suppressed {} repeated notify chain audit records over the rate limit \
                 (started {}, completed {}, failed {}, timed_out {}) from {} to {} (unix ms, {} ms)",
                self.total(),
                self.started,
                self.completed,
                self.failed,
                self.timed_out,
                self.first_ms,
                self.last_ms,
                self.last_ms - self.first_ms
            ),
            toast: false,
        }
    }
}

#[derive(Default)]
struct CallerState {
    hits: VecDeque<i64>,
    failures: VecDeque<i64>,
    pending: Option<Pending>,
}

impl CallerState {
    fn prune(&mut self, now: i64) {
        for q in [&mut self.hits, &mut self.failures] {
            while q.front().is_some_and(|t| now - *t >= WINDOW_MS) {
                q.pop_front();
            }
        }
    }
}

#[derive(Default)]
pub struct Gate {
    callers: HashMap<String, CallerState>,
}

pub struct Outcome {
    /// Records to write, in order (a pending summary precedes the event).
    pub records: Vec<Record>,
    /// Whether this call's phase was written as its own record.
    pub recorded: bool,
}

impl Gate {
    /// `key` identifies the caller for counting (transport identity);
    /// `caller`/`target` are what the ledger shows.
    pub fn record(
        &mut self,
        key: &str,
        caller: &str,
        target: &str,
        phase: Phase,
        limit: u32,
        now: i64,
    ) -> Outcome {
        let st = self.callers.entry(key.to_string()).or_default();
        st.prune(now);
        let (severity, kind, detail) = phase.record();
        let event = |toast| Record {
            severity,
            kind,
            caller: caller.into(),
            target: target.into(),
            detail: detail.into(),
            toast,
        };
        let mut records = Vec::new();
        let under_limit = (st.hits.len() as u64) < u64::from(limit.max(1));
        let accept = if phase.is_failure() {
            // Consumed by every failure call, so a flood keeps the cap full.
            let room = st.failures.len() < FAILURE_LIMIT;
            st.failures.push_back(now);
            room
        } else {
            under_limit
        };
        if accept {
            if under_limit {
                st.hits.push_back(now);
            }
            if let Some(p) = st.pending.take() {
                records.push(p.summary());
            }
            records.push(event(under_limit));
            return Outcome {
                records,
                recorded: true,
            };
        }
        let p = st.pending.get_or_insert_with(|| Pending {
            first_ms: now,
            caller: caller.into(),
            target: target.into(),
            ..Pending::default()
        });
        match phase {
            Phase::Started => p.started += 1,
            Phase::Completed => p.completed += 1,
            Phase::Failed => p.failed += 1,
            Phase::TimedOut => p.timed_out += 1,
        }
        p.last_ms = now;
        Outcome {
            records,
            recorded: false,
        }
    }

    /// Summaries whose suppression window ended (or all, when `all`), and
    /// cleanup of idle callers.
    pub fn flush(&mut self, now: i64, all: bool) -> Vec<Record> {
        let mut out = Vec::new();
        for st in self.callers.values_mut() {
            if st
                .pending
                .as_ref()
                .is_some_and(|p| all || now - p.first_ms >= WINDOW_MS)
            {
                if let Some(p) = st.pending.take() {
                    out.push(p.summary());
                }
            }
            st.prune(now);
        }
        self.callers
            .retain(|_, st| !st.hits.is_empty() || !st.failures.is_empty() || st.pending.is_some());
        out.sort_by(|a, b| (&a.caller, &a.target).cmp(&(&b.caller, &b.target)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(g: &mut Gate, key: &str, phase: Phase, now: i64) -> Outcome {
        g.record(key, key, "codex", phase, 3, now)
    }

    #[test]
    fn normal_use_is_recorded_individually() {
        let mut g = Gate::default();
        for (i, phase) in [Phase::Started, Phase::Completed].into_iter().enumerate() {
            let o = rec(&mut g, "w1:p1", phase, i as i64);
            assert!(o.recorded);
            assert_eq!(o.records.len(), 1);
            assert!(o.records[0].toast);
        }
        assert!(g.flush(10 * WINDOW_MS, true).is_empty());
    }

    #[test]
    fn repeats_over_limit_are_counted_and_summarized_once() {
        let mut g = Gate::default();
        let mut written = 0;
        for i in 0..10 {
            let phase = if i % 2 == 0 {
                Phase::Started
            } else {
                Phase::Completed
            };
            let o = rec(&mut g, "w1:p1", phase, 1000 + i);
            assert_eq!(o.recorded, i < 3);
            written += o.records.len();
        }
        assert_eq!(written, 3);
        assert!(g.flush(1000 + WINDOW_MS - 1, false).is_empty());
        let f = g.flush(1003 + WINDOW_MS, false);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, "integration.notify_chain.suppressed");
        assert_eq!(f[0].severity, Severity::Info);
        assert!(!f[0].toast);
        // i = 3..9: started at 4, 6, 8; completed at 3, 5, 7, 9
        assert_eq!(
            f[0].detail,
            "suppressed 7 repeated notify chain audit records over the rate limit \
             (started 3, completed 4, failed 0, timed_out 0) from 1003 to 1009 (unix ms, 6 ms)"
        );
        assert!(g.flush(1003 + 3 * WINDOW_MS, false).is_empty());
    }

    #[test]
    fn early_failures_are_recorded_even_over_the_normal_limit_without_toast() {
        let mut g = Gate::default();
        for i in 0..3 {
            rec(&mut g, "w1:p1", Phase::Completed, i);
        }
        for phase in [Phase::Failed, Phase::TimedOut] {
            let o = rec(&mut g, "w1:p1", phase, 10);
            assert!(o.recorded);
            assert_eq!(o.records.len(), 1);
            assert_eq!(o.records[0].severity, Severity::Notice);
            assert!(!o.records[0].toast);
        }
        assert!(g.flush(10, true).is_empty(), "nothing was suppressed");
    }

    // QA T-019 regression: a pane agent sending only `failed` (or `timed_out`)
    // must not get an unbounded number of individual ledger records.
    #[test]
    fn failure_flood_is_capped_and_counted_in_the_summary() {
        for (phase, counts) in [
            (Phase::Failed, "failed 980, timed_out 0"),
            (Phase::TimedOut, "failed 0, timed_out 980"),
        ] {
            let mut g = Gate::default();
            let mut written = 0;
            for i in 0..1000 {
                let o = rec(&mut g, "w1:p1", phase, i);
                assert_eq!(o.recorded, (i as usize) < FAILURE_LIMIT);
                written += o.records.len();
            }
            assert_eq!(written, FAILURE_LIMIT);
            let f = g.flush(20 + WINDOW_MS, false);
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].severity, Severity::Notice);
            assert!(!f[0].toast);
            assert_eq!(
                f[0].detail,
                format!(
                    "suppressed 980 repeated notify chain audit records over the rate limit \
                     (started 0, completed 0, {counts}) from 20 to 999 (unix ms, 979 ms)"
                )
            );
        }
    }

    #[test]
    fn suppressed_failures_keep_consuming_the_failure_cap() {
        let mut g = Gate::default();
        // A steady flood (one call per second) never lets the cap refill.
        let mut recorded = 0;
        for i in 0..180 {
            recorded += usize::from(rec(&mut g, "w1:p1", Phase::Failed, i * 1000).recorded);
        }
        assert_eq!(recorded, FAILURE_LIMIT);
        let f = g.flush(180_000 + WINDOW_MS, true);
        assert!(
            f[0].detail.starts_with("suppressed 160 "),
            "{}",
            f[0].detail
        );
        // The failure cap is separate from the normal one.
        assert!(rec(&mut g, "w1:p2", Phase::Failed, 0).recorded);
        for i in 0..FAILURE_LIMIT as i64 {
            rec(&mut g, "w1:p3", Phase::Failed, i);
        }
        assert!(!rec(&mut g, "w1:p3", Phase::Failed, 30).recorded);
        // Recorded failures also used the normal limit (3) while it had room.
        assert!(!rec(&mut g, "w1:p3", Phase::Started, 31).recorded);
    }

    #[test]
    fn next_accepted_record_writes_the_pending_summary_first() {
        let mut g = Gate::default();
        for i in 0..5 {
            rec(&mut g, "w1:p1", Phase::Started, i);
        }
        let o = rec(&mut g, "w1:p1", Phase::Completed, WINDOW_MS + 1);
        assert!(o.recorded);
        let kinds: Vec<_> = o.records.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                "integration.notify_chain.suppressed",
                "integration.notify_chain.completed"
            ]
        );
        assert!(o.records[0].detail.starts_with("suppressed 2 "));
        assert!(g.flush(10 * WINDOW_MS, true).is_empty());
    }

    #[test]
    fn callers_are_counted_separately_and_stop_flushes_everything() {
        let mut g = Gate::default();
        for i in 0..4 {
            rec(&mut g, "w1:p1", Phase::Started, i);
        }
        assert!(rec(&mut g, "w1:p2", Phase::Started, 5).recorded);
        let f = g.flush(6, true);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].caller, "w1:p1");
        assert!(f[0].detail.starts_with("suppressed 1 "));
    }
}
