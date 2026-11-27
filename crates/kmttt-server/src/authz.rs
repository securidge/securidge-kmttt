//! API 게이트웨이: 호출자 식별, 정책 판정, 승인 대기열, 흐름·보안 이벤트 기록.

use crate::Server;
use kmttt_policy::{classify_action, ActionClass, Caller, Decision, Target};
use kmttt_proto::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

impl Server {
    /// 소켓 상대 PID로 호출자를 정한다. PID를 모르면 가장 제한적인 쪽(알 수 없는 pane)으로 본다.
    pub fn identify(&self, pid: Option<u32>) -> Caller {
        let Some(pid) = pid else {
            return Caller::Pane {
                pane: "unknown".into(),
                workspace: String::new(),
                agent_kind: None,
                agent_name: None,
            };
        };
        // 방금 뜬 프로세스(예: 에이전트가 실행한 `kmttt agent prompt`)가 표에 있도록 새로 읽는다.
        let (roots, plugin_roots) = match self.st.lock() {
            Ok(st) => (
                st.panes
                    .values()
                    .filter_map(|p| {
                        p.pid.map(|r| {
                            (
                                r,
                                p.id.clone(),
                                p.ws.clone(),
                                p.agent.as_ref().map(|a| a.kind.clone()),
                                p.alias.clone(),
                                p.job.clone(),
                            )
                        })
                    })
                    .collect::<Vec<_>>(),
                st.plugin_pids.clone(),
            ),
            // 잠금이 깨졌으면 판단할 수 없다 → 제한적으로(실패 시 닫힘).
            Err(_) => return Self::unknown_caller(),
        };
        let Ok(mut procs) = self.procs.lock() else {
            return Self::unknown_caller();
        };
        procs.refresh();
        for (root, name) in &plugin_roots {
            if procs.is_within(pid, *root) {
                let caps = self
                    .plugins
                    .read()
                    .ok()
                    .and_then(|ps| {
                        ps.iter()
                            .find(|p| &p.manifest.name == name)
                            .map(|p| p.capabilities.clone())
                    })
                    .unwrap_or_default();
                return Caller::Plugin {
                    name: name.clone(),
                    capabilities: caps,
                };
            }
        }
        let as_pane =
            |pane: &String, ws: &String, kind: &Option<String>, alias: &Option<String>| {
                Caller::Pane {
                    pane: pane.clone(),
                    workspace: ws.clone(),
                    agent_kind: kind.clone(),
                    agent_name: alias.clone(),
                }
            };
        // Windows: pane Job 안의 프로세스는 부모를 끊었어도 그 pane이다.
        for (_, pane, ws, kind, alias, job) in &roots {
            if job.as_ref().is_some_and(|j| j.contains(pid)) {
                return as_pane(pane, ws, kind, alias);
            }
        }
        for (root, pane, ws, kind, alias, _) in &roots {
            if procs.is_within(pid, *root) {
                return Caller::Pane {
                    pane: pane.clone(),
                    workspace: ws.clone(),
                    agent_kind: kind.clone(),
                    agent_name: alias.clone(),
                };
            }
        }
        // 보조 신호: pane에서 시작한 프로세스는 KMTTT_PANE을 물려받는다.
        // 부모를 끊어(이중 fork) 트리에서 빠져나와도 환경 변수는 남는다(일부러 지우면 못 잡는다).
        if let Some(env_pane) = kmttt_pty::reaper::pane_env_of(pid) {
            if let Some((_, pane, ws, kind, alias, _)) = roots.iter().find(|r| r.1 == env_pane) {
                return Caller::Pane {
                    pane: pane.clone(),
                    workspace: ws.clone(),
                    agent_kind: kind.clone(),
                    agent_name: alias.clone(),
                };
            }
            // 이미 닫힌 pane 이름이라도 pane 출신인 것은 분명하다.
            return Self::unknown_caller();
        }
        // Linux: 서버가 subreaper라 pane 자손 고아는 서버 밑으로 온다.
        // 계보가 pane 루트를 거치지 않고 서버에 닿으면 pane에서 빠져나온 프로세스다.
        if procs.is_within(pid, std::process::id()) {
            return Self::unknown_caller();
        }
        // macOS: launchd 밑으로 간 고아 + 제어 터미널 없음 → 사람이 친 명령이 아니다.
        if kmttt_pty::reaper::detached_without_tty(pid) {
            return Self::unknown_caller();
        }
        // Windows: WMI·작업 스케줄러 등 OS 실행 서비스가 띄운 호출자.
        let names: Vec<String> = procs.chain(pid, 0).iter().map(|e| e.name.clone()).collect();
        let trusted = self
            .policy
            .lock()
            .map(|p| p.cfg.identify.trusted_launchers.clone())
            .unwrap_or_default();
        if kmttt_pty::reaper::launched_by_os_service(&names, &trusted) {
            return Self::unknown_caller();
        }
        if procs.get(pid).is_none() {
            // 이미 끝난 프로세스라 계보를 확인할 수 없다 → 제한적으로.
            return Caller::Pane {
                pane: "unknown".into(),
                workspace: String::new(),
                agent_kind: None,
                agent_name: None,
            };
        }
        Caller::User
    }

    /// 호출자 프로세스 계보 요약(기록용): `[{"pid":…, "name":…}, …]`, 자기 자신부터 최대 12단계.
    pub fn caller_lineage(&self, pid: Option<u32>) -> serde_json::Value {
        let Some(pid) = pid else {
            return serde_json::Value::Null;
        };
        let Ok(mut procs) = self.procs.lock() else {
            return serde_json::Value::Null;
        };
        procs.refresh();
        let chain: Vec<serde_json::Value> = procs
            .chain(pid, 0)
            .iter()
            .take(12)
            .map(|e| json!({"pid": e.pid, "name": e.name}))
            .collect();
        json!({"pid": pid, "chain": chain})
    }

    fn unknown_caller() -> Caller {
        Caller::Pane {
            pane: "unknown".into(),
            workspace: String::new(),
            agent_kind: None,
            agent_name: None,
        }
    }

    /// pane 대상 정보를 정책 판정용으로 만든다.
    pub fn target_of(&self, pane: Option<&str>) -> Target {
        let Some(pane) = pane else {
            return Target::default();
        };
        let Ok(st) = self.st.lock() else {
            return Target::default();
        };
        match st.panes.get(pane) {
            Some(p) => Target {
                pane: Some(p.id.clone()),
                workspace: Some(p.ws.clone()),
                agent_kind: p.agent.as_ref().map(|a| a.kind.clone()),
                agent_name: p.alias.clone(),
            },
            None => Target {
                pane: Some(pane.into()),
                ..Default::default()
            },
        }
    }

    /// 호출 하나를 허가한다. ask면 사람이 결정할 때까지(또는 제한 시간까지) 기다린다.
    pub async fn authorize(
        self: &Arc<Self>,
        caller: &Caller,
        action: &str,
        target: &Target,
        detail: &str,
    ) -> Result<(), RpcError> {
        let class = classify_action(action);
        let verdict = match self.policy.lock() {
            Ok(mut p) => p.evaluate(caller, action, target),
            Err(_) => return Err(RpcError::internal("policy unavailable")),
        };
        let label = caller.label();
        let tlabel = target
            .pane
            .clone()
            .or_else(|| target.workspace.clone())
            .unwrap_or_default();
        let is_agent = *caller != Caller::User;
        let mut decision = verdict.decision;
        let mut how = decision.as_str().to_string();

        if decision == Decision::Ask {
            let timeout = self
                .policy
                .lock()
                .map(|p| p.cfg.defaults.ask_timeout_secs)
                .unwrap_or(120);
            let allowed = self.ask(&label, action, &tlabel, detail, timeout).await;
            decision = if allowed {
                Decision::Allow
            } else {
                Decision::Deny
            };
            how = format!("ask→{}", decision.as_str());
        }

        // 에이전트·플러그인이 다른 pane에 한 일은 흐름으로 남긴다.
        if is_agent
            && matches!(
                class,
                ActionClass::Control | ActionClass::Read | ActionClass::Create
            )
        {
            if let Ok(mut st) = self.st.lock() {
                st.push_flow(Flow {
                    ts_ms: now_ms(),
                    from: label.clone(),
                    to: tlabel.clone(),
                    action: action.into(),
                    decision: how.clone(),
                });
            }
        }
        if class != ActionClass::Meta || is_agent {
            self.emit(
                "policy.decision",
                &caller.key(),
                json!({"caller": label, "action": action, "target": tlabel, "decision": how, "rule": verdict.rule_id, "reason": verdict.reason, "detail": detail}),
            );
        }
        if decision == Decision::Deny {
            if let Ok(mut st) = self.st.lock() {
                st.blocked_today += 1;
                if let Caller::Pane { pane, .. } = caller {
                    if let Some(p) = st.panes.get_mut(pane) {
                        p.blocked_calls += 1;
                    }
                }
            }
            let kind = if verdict.rule_id == "builtin:capability" {
                "plugin.capability"
            } else {
                "policy.deny"
            };
            self.security_event(
                Severity::Notice,
                kind,
                &label,
                &tlabel,
                &format!("{action} denied ({}: {})", verdict.rule_id, verdict.reason),
            );
            let code = if verdict.rule_id.contains("rate") || verdict.reason.contains("rate") {
                codes::RATE_LIMITED
            } else if how.starts_with("ask") {
                codes::APPROVAL_DENIED
            } else {
                codes::POLICY_DENIED
            };
            return Err(RpcError::new(
                code,
                format!("{action} denied by policy ({})", verdict.rule_id),
            ));
        }
        Ok(())
    }

    /// 승인 대기열에 올리고 결정을 기다린다.
    pub async fn ask(
        self: &Arc<Self>,
        caller: &str,
        action: &str,
        target: &str,
        detail: &str,
        timeout_secs: u64,
    ) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let info = {
            let Ok(mut st) = self.st.lock() else {
                return false;
            };
            st.next_approval += 1;
            let info = Approval {
                id: st.next_approval,
                ts_ms: now_ms(),
                expires_ms: now_ms() + (timeout_secs as i64) * 1000,
                caller: caller.into(),
                action: action.into(),
                target: target.into(),
                detail: self.redact(detail).chars().take(500).collect(),
            };
            st.approvals.push(crate::state::PendingApproval {
                info: info.clone(),
                tx: Some(tx),
            });
            info
        };
        let lang = self.lang();
        self.toast(
            &format!(
                "⚑ {} #{}: {caller} → {action} {target}",
                kmttt_core::i18n::t(&lang, "toast.approval"),
                info.id
            ),
            "critical",
            None,
        );
        self.emit(
            "approval.requested",
            caller,
            serde_json::to_value(&info).unwrap_or_default(),
        );
        self.push_snapshot(true);
        let allowed = tokio::time::timeout(Duration::from_secs(timeout_secs + 1), rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false);
        if let Ok(mut st) = self.st.lock() {
            st.approvals.retain(|a| a.info.id != info.id);
        }
        self.push_snapshot(true);
        allowed
    }

    /// 사람이 승인 대기 항목을 결정한다.
    pub fn decide(
        self: &Arc<Self>,
        id: u64,
        allow: bool,
        decided_by: serde_json::Value,
    ) -> Result<(), RpcError> {
        let info = {
            let mut st = self.st.lock().map_err(|_| RpcError::internal("lock"))?;
            let a = st
                .approvals
                .iter_mut()
                .find(|a| a.info.id == id)
                .ok_or_else(|| RpcError::not_found(format!("no pending approval #{id}")))?;
            if let Some(tx) = a.tx.take() {
                let _ = tx.send(allow);
            }
            a.info.clone()
        };
        self.emit(
            "approval.decided",
            "user",
            json!({"id": id, "allow": allow, "approval": info, "decided_by": decided_by}),
        );
        Ok(())
    }

    pub fn control_snapshot(&self) -> ControlSnapshot {
        let integrity = self.integrity_now();
        let Ok(st) = self.st.lock() else {
            return ControlSnapshot::default();
        };
        let red = |s: &str| self.redact(s);
        let snap = st.snapshot(&self.opts.session, &red);
        ControlSnapshot {
            machine: self.machine.clone(),
            session: self.opts.session.clone(),
            ts_ms: now_ms(),
            panes: snap.panes,
            approvals: st.approvals.iter().map(|a| a.info.clone()).collect(),
            events: st.events.iter().rev().take(200).cloned().collect(),
            flows: st.flows.iter().rev().take(200).cloned().collect(),
            tools: st.tools.iter().rev().take(200).cloned().collect(),
            integrity,
            blocked_today: st.blocked_today,
            version: kmttt_core::VERSION.into(),
        }
    }

    /// 마지막 전체 검증 결과 + 지금 기록 수.
    pub fn integrity_now(&self) -> IntegrityStatus {
        let mut i = self.integrity.lock().map(|i| i.clone()).unwrap_or_default();
        if let Ok(l) = self.ledger.lock() {
            if let Some(l) = l.as_ref() {
                i.records = l.records();
                i.level = l.level().as_str().into();
                i.key_fingerprint = l.key_fingerprint();
                if l.last_checkpoint_ms().is_some() {
                    i.last_checkpoint_ms = l.last_checkpoint_ms();
                }
            }
        }
        i
    }
}

/// 기록 체인을 처음부터 검증해 무결성 상태를 갱신한다(무거우니 별도 스레드).
pub fn refresh_integrity(server: &Arc<Server>, emit_on_fail: bool) {
    let s = server.clone();
    tokio::task::spawn_blocking(move || {
        let dir = kmttt_core::paths::ledger_dir(&s.opts.session);
        let r = kmttt_ledger::verify(&dir);
        let status = match &r {
            Ok(rep) => IntegrityStatus {
                ok: rep.ok,
                level: String::new(),
                records: rep.records,
                last_checkpoint_ms: rep.last_checkpoint_ms,
                key_fingerprint: rep.key_fingerprint.clone(),
                detail: match &rep.error {
                    Some(e) => format!("{} line {}: {}", e.file, e.line, e.reason),
                    None => format!(
                        "{} records, {} checkpoints, {} unsigned at tail",
                        rep.records, rep.checkpoints, rep.unsigned_tail
                    ),
                },
            },
            Err(e) => IntegrityStatus {
                ok: false,
                detail: e.to_string(),
                ..Default::default()
            },
        };
        let failed = !status.ok;
        let detail = status.detail.clone();
        if let Ok(mut i) = s.integrity.lock() {
            *i = status;
        }
        if failed && emit_on_fail {
            let s2 = s.clone();
            let rt = tokio::runtime::Handle::current();
            rt.spawn(async move {
                s2.security_event(
                    Severity::Critical,
                    "ledger.integrity",
                    "ledger",
                    "ledger",
                    &detail,
                );
            });
        }
    });
}
