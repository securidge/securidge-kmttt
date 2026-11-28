//! 영어·한국어 문구 표. `t("ko", "sidebar.workspaces")`.

const TABLE: &[(&str, &str, &str)] = &[
    ("sidebar.workspaces", "Workspaces", "워크스페이스"),
    ("sidebar.agents", "Agents", "에이전트"),
    ("sidebar.no_agents", "no agents yet", "아직 에이전트 없음"),
    ("status.prefix", "PREFIX", "PREFIX"),
    (
        "status.detach_hint",
        "prefix+q detach · prefix+? help",
        "prefix+q 분리 · prefix+? 도움말",
    ),
    ("status.masked", "MASKED", "마스킹"),
    ("state.working", "working", "작업 중"),
    ("state.blocked", "blocked", "입력 대기"),
    ("state.done", "done", "완료"),
    ("state.idle", "idle", "대기"),
    ("state.unknown", "unknown", "알 수 없음"),
    ("control.title", "Control Center", "통합 관제"),
    ("control.agents", "Agents", "에이전트"),
    ("control.approvals", "Approvals", "승인 대기"),
    ("control.events", "Security events", "보안 이벤트"),
    (
        "control.flows",
        "Agent-to-agent messages",
        "에이전트 간 메시지",
    ),
    ("control.tools", "Tool calls", "도구 호출"),
    ("control.integrity", "Ledger integrity", "감사 기록 무결성"),
    ("control.machines", "Machines", "머신"),
    ("control.resources", "Resources", "자원"),
    ("control.summary_agents", "agents", "에이전트"),
    ("control.summary_pending", "pending approvals", "승인 대기"),
    ("control.summary_blocked", "blocked today", "오늘 차단"),
    (
        "control.help",
        "↑↓ select · a allow · x deny · Tab panel · Esc back",
        "↑↓ 선택 · a 허용 · x 거부 · Tab 패널 · Esc 돌아가기",
    ),
    ("control.empty", "nothing here", "없음"),
    ("control.observe_only", "observe only", "관찰 전용"),
    ("toast.done", "finished", "작업 완료"),
    ("toast.blocked", "needs input", "입력 필요"),
    ("toast.approval", "approval requested", "승인 요청"),
    ("help.title", "Key bindings", "키 바인딩"),
    ("credit", "Kwangmo Kim", "Kwangmo Kim"),
];

/// 문구를 찾는다. 없으면 키를 그대로 돌려준다.
pub fn t(lang: &str, key: &str) -> String {
    TABLE
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, en, ko)| if lang == "ko" { *ko } else { *en })
        .unwrap_or(key)
        .to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lookup() {
        assert_eq!(super::t("ko", "state.blocked"), "입력 대기");
        assert_eq!(super::t("en", "state.blocked"), "blocked");
        assert_eq!(super::t("en", "missing.key"), "missing.key");
    }

    #[test]
    fn every_key_has_both_languages() {
        for (k, en, ko) in super::TABLE {
            assert!(!en.is_empty() && !ko.is_empty(), "{k}");
        }
    }
}
