//! KMttt 백그라운드 서버.
//!
//! 서버가 PTY·자식 프로세스·레이아웃·감사 기록을 소유하고, 클라이언트는 그리기와 입력만 한다.
//! 모든 호출은 API 게이트웨이(호출자 식별 → 정책)를 거치고, 모든 이벤트는 필터 체인
//! (마스킹 → 기록)을 거친 뒤에만 구독자·플러그인으로 나간다.

mod authz;
mod filters;
mod listener;
mod notify_audit;
mod panes;
mod persist;
pub mod plugins;
mod rpc;
pub mod state;
mod ticker;
mod worktree;

use anyhow::Result;
use kmttt_core::{paths, Config, CoreEvent, FilterChain};
use kmttt_detect::Detector;
use kmttt_ledger::{Ledger, LedgerOptions};
use kmttt_policy::{PolicyConfig, PolicyEngine};
use kmttt_proto::*;
use kmttt_redact::{RedactConfig, Redactor};
use kmttt_sandbox::egress::EgressProxy;
use serde_json::{json, Value};
use state::State;

/// 막힌 외부 접속을 보안 기록에 남기는 규칙. 에이전트는 막히면 몇 초 안에 수십 번 다시 시도하므로
/// 하나하나 남기면 기록과 알림이 넘친다(실사용에서 몇 분에 73건).
/// - 같은 창·같은 주소는 10분에 한 번만 남기고, 그 사이 다시 시도한 횟수는 10분 뒤 한 줄로 합친다.
/// - 단순 차단은 '알림'(화면 알림 없음). 한 창이 10분 안에 서로 다른 주소 8곳 이상에 막히면
///   한 번 '경고'로 올린다(주소를 훑는 행동일 수 있으므로).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub mod egress_log {
    use super::Severity;
    use std::collections::HashMap;

    pub const WINDOW_MS: i64 = 10 * 60 * 1000;
    pub const BURST_HOSTS: usize = 8;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Record {
        pub severity: Severity,
        pub kind: &'static str,
        pub caller: String,
        pub target: String,
        pub detail: String,
    }

    #[derive(Default)]
    pub struct Gate {
        /// (창, 주소) → (마지막으로 남긴 시각, 그 뒤 더 막힌 횟수)
        seen: HashMap<(String, String), (i64, u64)>,
        /// 창 → 최근 막힌 (시각, 주소)
        recent: HashMap<String, Vec<(i64, String)>>,
        /// 창 → 마지막 '경고' 시각
        burst_at: HashMap<String, i64>,
    }

    impl Gate {
        pub fn denied(&mut self, caller: &str, target: &str, now: i64) -> Vec<Record> {
            let mut out = Vec::new();
            let key = (caller.to_string(), target.to_string());
            match self.seen.get_mut(&key) {
                Some((last, more)) if now - *last < WINDOW_MS => *more += 1,
                _ => {
                    self.seen.insert(key, (now, 0));
                    out.push(Record {
                        severity: Severity::Info,
                        kind: "sandbox.egress_denied",
                        caller: caller.into(),
                        target: target.into(),
                        detail: "outbound connection blocked by allow list".into(),
                    });
                }
            }
            let host = target
                .rsplit_once(':')
                .map(|(h, _)| h)
                .unwrap_or(target)
                .to_string();
            let r = self.recent.entry(caller.to_string()).or_default();
            r.retain(|(t, _)| now - *t < WINDOW_MS);
            r.push((now, host));
            let mut hosts: Vec<&String> = r.iter().map(|(_, h)| h).collect();
            hosts.sort();
            hosts.dedup();
            let n = hosts.len();
            let recent_burst = self
                .burst_at
                .get(caller)
                .is_some_and(|t| now - *t < WINDOW_MS);
            if n >= BURST_HOSTS && !recent_burst {
                self.burst_at.insert(caller.to_string(), now);
                out.push(Record {
                    severity: Severity::Notice,
                    kind: "sandbox.egress_burst",
                    caller: caller.into(),
                    target: format!("{n} hosts"),
                    detail: format!("blocked connections to {n} different hosts within 10 minutes"),
                });
            }
            out
        }

        /// 10분이 지난 항목의 '더 막힌 횟수'를 한 줄로 남기고, 오래된 항목을 지운다.
        pub fn flush(&mut self, now: i64) -> Vec<Record> {
            let mut out = Vec::new();
            for ((caller, target), (last, more)) in self.seen.iter_mut() {
                if now - *last >= WINDOW_MS && *more > 0 {
                    out.push(Record {
                        severity: Severity::Info,
                        kind: "sandbox.egress_denied",
                        caller: caller.clone(),
                        target: target.clone(),
                        detail: format!("blocked {more} more times in the last 10 minutes"),
                    });
                    *last = now;
                    *more = 0;
                }
            }
            self.seen
                .retain(|_, (last, more)| now - *last < 3 * WINDOW_MS || *more > 0);
            self.recent
                .retain(|_, r| r.iter().any(|(t, _)| now - *t < WINDOW_MS));
            self.burst_at.retain(|_, t| now - *t < WINDOW_MS);
            out.sort_by(|a, b| (&a.caller, &a.target).cmp(&(&b.caller, &b.target)));
            out
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn repeats_are_grouped() {
            let mut g = Gate::default();
            let mut logged = 0;
            for i in 0..73 {
                logged += g
                    .denied("w1:p1", "mcp-proxy.anthropic.com:443", i * 1000)
                    .len();
            }
            assert_eq!(logged, 1, "같은 주소 73번 → 기록 1줄");
            assert!(
                g.flush(5 * 60 * 1000).is_empty(),
                "10분 전에는 합치지 않는다"
            );
            let f = g.flush(WINDOW_MS + 1000);
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].detail, "blocked 72 more times in the last 10 minutes");
            assert_eq!(f[0].severity, Severity::Info);
            // 10분이 지나 다시 막히면 새로 한 줄
            assert_eq!(
                g.denied("w1:p1", "mcp-proxy.anthropic.com:443", 2 * WINDOW_MS + 2000)
                    .len(),
                1
            );
            // 다른 창은 따로 센다
            assert_eq!(
                g.denied("w1:p2", "mcp-proxy.anthropic.com:443", 2000).len(),
                1
            );
        }

        #[test]
        fn many_hosts_escalate_once() {
            let mut g = Gate::default();
            let mut kinds = Vec::new();
            for i in 0..20 {
                for r in g.denied("w1:p1", &format!("h{i}.example:443"), i as i64 * 1000) {
                    kinds.push((r.kind, r.severity));
                }
            }
            let bursts: Vec<_> = kinds
                .iter()
                .filter(|k| k.0 == "sandbox.egress_burst")
                .collect();
            assert_eq!(bursts.len(), 1);
            assert_eq!(bursts[0].1, Severity::Notice);
            assert!(kinds
                .iter()
                .filter(|k| k.0 == "sandbox.egress_denied")
                .all(|k| k.1 == Severity::Info));
        }
    }
}
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

pub use listener::server_running;

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub session: String,
    /// `kmttt` 실행 파일 경로(sandbox-exec 감싸기에 씀)
    pub kmttt_exe: PathBuf,
}

pub struct Server {
    pub opts: ServerOptions,
    pub started: Instant,
    pub machine: String,
    pub cfg: RwLock<Config>,
    pub policy: Mutex<PolicyEngine>,
    pub redactor: Arc<RwLock<Redactor>>,
    pub detector: RwLock<Detector>,
    pub ledger: Arc<Mutex<Option<Ledger>>>,
    pub chain: FilterChain,
    pub st: Mutex<State>,
    pub procs: Mutex<kmttt_pty::ProcTable>,
    pub egress: OnceLock<EgressProxy>,
    pub shutdown: tokio::sync::Notify,
    pub integrity: Mutex<IntegrityStatus>,
    pub plugins: RwLock<Vec<plugins::Plugin>>,
    /// `hook.notify_chain` 감사 기록의 호출자별 반복 억제.
    notify_audit: Mutex<notify_audit::Gate>,
    /// 종료 중이면 pane이 닫혀도 레이아웃을 저장하지 않는다(재시작 복원용 레이아웃 보존).
    pub stopping: std::sync::atomic::AtomicBool,
}

impl Server {
    /// 문자열 하나를 마스킹한다.
    pub fn redact(&self, s: &str) -> String {
        match self.redactor.read() {
            Ok(r) => r.redact_str(s),
            Err(_) => "[REDACTED:unavailable]".into(),
        }
    }

    /// 이벤트를 필터 체인(마스킹 → 기록)에 통과시킨 뒤 구독자와 플러그인에게 보낸다.
    /// `st` 잠금을 쥔 채로 부르면 안 된다.
    pub fn emit(self: &Arc<Self>, kind: &str, source: &str, data: Value) {
        let Some(ev) = self.chain.process(CoreEvent::new(kind, source, data)) else {
            return;
        };
        let msg = EventMsg {
            kind: ev.kind.clone(),
            ts_ms: ev.ts_ms,
            data: json!({"source": ev.source, "data": ev.data}),
        };
        if let Ok(st) = self.st.lock() {
            let note =
                RpcMessage::notification("event", serde_json::to_value(&msg).unwrap_or_default());
            for c in st.clients.values() {
                if let Some(subs) = &c.subs {
                    if subs
                        .iter()
                        .any(|s| (s == "*" && ev.kind != "pane.output") || s == &ev.kind)
                    {
                        let _ = c.tx.send(note.clone());
                    }
                }
            }
        }
        plugins::dispatch(self, &ev);
    }

    /// 붙어 있는 모든 클라이언트에게 알림을 보낸다.
    pub fn broadcast(&self, method: &str, params: Value) {
        if let Ok(st) = self.st.lock() {
            let note = RpcMessage::notification(method, params);
            for c in st.clients.values().filter(|c| c.attached) {
                let _ = c.tx.send(note.clone());
            }
        }
    }

    /// 레이아웃·상태가 바뀌었으면 스냅숏을 붙어 있는 클라이언트에게 보낸다.
    pub fn push_snapshot(&self, force: bool) {
        let Ok(mut st) = self.st.lock() else { return };
        if !st.any_attached() {
            return;
        }
        let red = |s: &str| self.redact(s);
        let snap = st.snapshot(&self.opts.session, &red);
        let key = State::snapshot_key(&snap);
        if !force && key == st.last_snapshot_key {
            return;
        }
        st.last_snapshot_key = key;
        let note =
            RpcMessage::notification("snapshot", serde_json::to_value(&snap).unwrap_or_default());
        for c in st.clients.values().filter(|c| c.attached) {
            let _ = c.tx.send(note.clone());
        }
    }

    pub fn toast(&self, text: &str, level: &str, pane: Option<&str>) {
        self.broadcast("toast", json!({"text": text, "level": level, "pane": pane}));
    }

    /// 보안 이벤트를 남기고(관제 화면 + 기록 + 구독자) 알린다.
    /// 재시작 직후, 기록에서 최근 보안 이벤트를 관제 화면 목록으로 되살린다.
    /// "오늘 차단" 수도 오늘(UTC) 차단 이벤트로 다시 센다.
    fn restore_events(&self) {
        let dir = paths::ledger_dir(&self.opts.session);
        let recent = kmttt_ledger::verify::recent_data(&dir, "security.event", state::RING);
        let day = |ms: i64| ms.div_euclid(86_400_000);
        let today = day(now_ms());
        let Ok(mut st) = self.st.lock() else {
            return;
        };
        for d in recent {
            let Ok(ev) = serde_json::from_value::<SecurityEvent>(d) else {
                continue;
            };
            st.next_event = st.next_event.max(ev.id);
            if day(ev.ts_ms) == today && matches!(ev.kind.as_str(), "policy.deny" | "tool.denied") {
                st.blocked_today += 1;
            }
            st.events.push_back(ev);
        }
        while st.events.len() > state::RING {
            st.events.pop_front();
        }
    }

    /// 프록시 주소의 창 표식으로 창 ID를 찾는다. 모르면 "sandbox".
    pub fn pane_for_tag(&self, tag: Option<&str>) -> String {
        let Some(tag) = tag else {
            return "sandbox".into();
        };
        self.st
            .lock()
            .ok()
            .and_then(|st| {
                st.panes
                    .keys()
                    .find(|id| kmttt_sandbox::egress::pane_tag(id) == tag)
                    .cloned()
            })
            .unwrap_or_else(|| "sandbox".into())
    }

    pub fn security_event(
        self: &Arc<Self>,
        severity: Severity,
        kind: &str,
        caller: &str,
        target: &str,
        detail: &str,
    ) {
        self.security_event_with(severity, kind, caller, target, detail, true);
    }

    /// `toast`가 false면 기록·구독자 전달은 같고 화면 알림만 생략한다.
    pub fn security_event_with(
        self: &Arc<Self>,
        severity: Severity,
        kind: &str,
        caller: &str,
        target: &str,
        detail: &str,
        toast: bool,
    ) {
        let detail = self.redact(detail);
        let ev = match self.st.lock() {
            Ok(mut st) => st.push_event(severity, kind, caller, target, &detail),
            Err(_) => return,
        };
        if toast && severity >= Severity::Notice {
            self.toast(
                &format!("{} {kind}: {detail}", severity.icon()),
                severity.as_str(),
                None,
            );
        }
        self.emit(
            "security.event",
            caller,
            serde_json::to_value(&ev).unwrap_or_default(),
        );
    }

    /// `hook.notify_chain` 한 건을 억제 규칙에 통과시켜 기록한다. 개별 기록했으면 true.
    pub(crate) fn notify_chain_audit(
        self: &Arc<Self>,
        key: &str,
        caller: &str,
        target: &str,
        phase: notify_audit::Phase,
    ) -> bool {
        let limit = self
            .policy
            .lock()
            .map(|p| p.cfg.defaults.rate_per_min)
            .unwrap_or(120);
        let Ok(out) = self
            .notify_audit
            .lock()
            .map(|mut g| g.record(key, caller, target, phase, limit, now_ms()))
        else {
            return false;
        };
        self.write_notify_audit(out.records);
        out.recorded
    }

    /// 억제 기간이 끝난 요약(또는 `all`이면 전부)을 기록한다.
    pub(crate) fn flush_notify_audit(self: &Arc<Self>, all: bool) {
        let out = self
            .notify_audit
            .lock()
            .map(|mut g| g.flush(now_ms(), all))
            .unwrap_or_default();
        self.write_notify_audit(out);
    }

    fn write_notify_audit(self: &Arc<Self>, records: Vec<notify_audit::Record>) {
        for r in records {
            self.security_event_with(r.severity, r.kind, &r.caller, &r.target, &r.detail, r.toast);
        }
    }

    pub fn lang(&self) -> String {
        self.cfg
            .read()
            .map(|c| c.language.clone())
            .unwrap_or_else(|_| "en".into())
    }

    fn ledger_options(cfg: &Config) -> LedgerOptions {
        LedgerOptions {
            level: kmttt_ledger::Level::parse(&cfg.ledger.level),
            encrypt: cfg.ledger.encrypt,
            checkpoint_secs: cfg.ledger.checkpoint_secs.max(5),
            checkpoint_records: cfg.ledger.checkpoint_records.max(10),
            mirror_file: (!cfg.ledger.mirror_file.is_empty())
                .then(|| PathBuf::from(&cfg.ledger.mirror_file)),
            syslog_udp: (!cfg.ledger.syslog_udp.is_empty()).then(|| cfg.ledger.syslog_udp.clone()),
        }
    }

    /// 설정·정책·매니페스트·마스킹 규칙을 다시 읽는다.
    pub fn reload_config(self: &Arc<Self>) -> Result<Value> {
        let cfg = Config::load(&paths::config_file())?;
        let policy_text = std::fs::read_to_string(paths::policy_file()).unwrap_or_default();
        let policy = PolicyConfig::parse(&policy_text)?;
        let redact_cfg = redact_config(&cfg)?;
        let redactor = Redactor::new(&redact_cfg)?;
        redactor.learn_env(std::env::vars());
        if let Ok(old) = self.redactor.read() {
            redactor.absorb(&old);
        }
        let (detector, manifest_errors) = Detector::with_user_dir(&paths::manifests_dir());
        if let Some(e) = self.egress.get() {
            e.set_allow(policy.sandbox.effective_egress_allow());
        }
        *self.cfg.write().map_err(|_| anyhow::anyhow!("lock"))? = cfg;
        *self.policy.lock().map_err(|_| anyhow::anyhow!("lock"))? = PolicyEngine::new(policy);
        *self.redactor.write().map_err(|_| anyhow::anyhow!("lock"))? = redactor;
        *self.detector.write().map_err(|_| anyhow::anyhow!("lock"))? = detector;
        *self.plugins.write().map_err(|_| anyhow::anyhow!("lock"))? =
            plugins::load_all(&paths::plugins_dir());
        let hash = kmttt_redact::fingerprint(&policy_text);
        self.emit(
            "config.reload",
            "user",
            json!({"policy_sha256_prefix": hash, "manifest_errors": manifest_errors}),
        );
        Ok(json!({"ok": true, "policy_hash": hash, "manifest_errors": manifest_errors}))
    }
}

/// config.toml의 [redact] 표를 RedactConfig로.
pub fn redact_config(cfg: &Config) -> Result<RedactConfig> {
    if cfg.redact.is_empty() {
        return Ok(RedactConfig::default());
    }
    let text = toml::to_string(&cfg.redact)?;
    Ok(toml::from_str(&text)?)
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "local".into())
}

/// 서버를 실행한다. `server.stop`이 오거나 오류가 나면 돌아온다.
pub async fn run(opts: ServerOptions) -> Result<()> {
    anyhow::ensure!(
        paths::valid_session_name(&opts.session),
        "invalid session name"
    );
    if listener::server_running(&opts.session).await {
        anyhow::bail!("a server for session '{}' is already running", opts.session);
    }
    std::fs::create_dir_all(paths::data_dir())?;
    // 두 클라이언트가 동시에 서버를 띄우면 둘 다 위 검사를 통과할 수 있다 → OS 파일 잠금으로 하나만.
    // 잠금은 프로세스가 끝나면 OS가 풀어 준다(비정상 종료에도 남지 않는다).
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths::server_lock_file(&opts.session))?;
    if lock_file.try_lock().is_err() {
        anyhow::bail!(
            "a server for session '{}' is already starting or running",
            opts.session
        );
    }
    let _server_lock = lock_file;
    let subreaper = kmttt_pty::reaper::become_subreaper();
    tracing::info!(subreaper, "process tracking");
    let cfg = Config::load(&paths::config_file())?;
    let policy_text = std::fs::read_to_string(paths::policy_file()).unwrap_or_default();
    let policy = PolicyConfig::parse(&policy_text)?;
    let redactor = Redactor::new(&redact_config(&cfg)?)?;
    // 서버 환경에 있는 비밀처럼 보이는 값은 pane에 나오면 바로 가린다.
    redactor.learn_env(std::env::vars());
    let redactor = Arc::new(RwLock::new(redactor));
    let (detector, manifest_errors) = Detector::with_user_dir(&paths::manifests_dir());
    let ledger = Ledger::open(
        &paths::ledger_dir(&opts.session),
        Server::ledger_options(&cfg),
    )?;
    let ledger_warnings = ledger.open_warnings.clone();
    let ledger = Arc::new(Mutex::new(Some(ledger)));

    let (find_tx, mut find_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut chain = FilterChain::new();
    chain.push(Box::new(filters::RedactFilter::new(
        redactor.clone(),
        find_tx,
    )));
    chain.push(Box::new(filters::LedgerFilter::new(ledger.clone())));

    let egress_allow = policy.sandbox.effective_egress_allow();
    let server = Arc::new(Server {
        opts,
        started: Instant::now(),
        machine: hostname(),
        cfg: RwLock::new(cfg),
        policy: Mutex::new(PolicyEngine::new(policy)),
        redactor,
        detector: RwLock::new(detector),
        ledger,
        chain,
        st: Mutex::new(State::new()),
        procs: Mutex::new(kmttt_pty::ProcTable::new()),
        egress: OnceLock::new(),
        shutdown: tokio::sync::Notify::new(),
        integrity: Mutex::new(IntegrityStatus::default()),
        plugins: RwLock::new(plugins::load_all(&paths::plugins_dir())),
        notify_audit: Mutex::new(notify_audit::Gate::default()),
        stopping: std::sync::atomic::AtomicBool::new(false),
    });

    // 이그레스 프록시: 거부된 연결은 보안 이벤트로.
    let (eg_tx, mut eg_rx) = tokio::sync::mpsc::unbounded_channel();
    match EgressProxy::start(egress_allow, eg_tx).await {
        Ok(p) => {
            let _ = server.egress.set(p);
        }
        Err(e) => tracing::warn!("egress proxy unavailable: {e}"),
    }
    {
        let s = server.clone();
        tokio::spawn(async move {
            let mut gate = egress_log::Gate::default();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                let now = now_ms();
                let out = tokio::select! {
                    ev = eg_rx.recv() => {
                        let Some(ev) = ev else { break };
                        if ev.allowed {
                            continue;
                        }
                        let caller = s.pane_for_tag(ev.tag.as_deref());
                        gate.denied(&caller, &format!("{}:{}", ev.host, ev.port), now)
                    }
                    _ = tick.tick() => gate.flush(now),
                };
                for r in out {
                    s.security_event(r.severity, r.kind, &r.caller, &r.target, &r.detail);
                }
            }
        });
    }
    // 마스킹 필터가 찾은 비밀: 지문마다 한 번만 경보.
    {
        let s = server.clone();
        tokio::spawn(async move {
            while let Some((source, f)) = find_rx.recv().await {
                let first =
                    s.st.lock()
                        .map(|mut st| st.seen_fingerprints.insert(f.fingerprint.clone()))
                        .unwrap_or(false);
                if first {
                    s.security_event(
                        Severity::Notice,
                        "secret.detected",
                        &source,
                        &f.kind,
                        &format!("{} (fingerprint {}) was redacted", f.kind, f.fingerprint),
                    );
                }
            }
        });
    }

    persist::restore(&server);
    let policy_hash = kmttt_redact::fingerprint(&policy_text);
    server.emit(
        "server.start",
        "server",
        json!({"version": kmttt_core::VERSION, "session": server.opts.session, "pid": std::process::id(), "policy_sha256_prefix": policy_hash, "manifest_errors": manifest_errors, "ledger_warnings": ledger_warnings}),
    );
    for w in &ledger_warnings {
        server.security_event(Severity::Critical, "ledger.damaged", "ledger", "ledger", w);
    }
    server.restore_events();
    authz::refresh_integrity(&server, true);

    tokio::spawn(ticker::run(server.clone()));
    let result = listener::serve(server.clone()).await;

    // 정리: 레이아웃 저장, 모든 pane 종료, 마지막 체크포인트.
    server.flush_notify_audit(true);
    persist::save(&server);
    server
        .stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    server.emit("server.stop", "server", json!({}));
    if let Ok(mut st) = server.st.lock() {
        for p in st.panes.values_mut() {
            if let Some(job) = &p.job {
                job.terminate();
            }
            if let Some(pty) = p.pty.as_mut() {
                pty.kill();
            }
        }
    }
    if let Ok(mut l) = server.ledger.lock() {
        if let Some(mut led) = l.take() {
            let _ = led.checkpoint();
        }
    }
    result
}
