//! 메서드 목록과 OpenRPC 문서 생성.

use serde_json::{json, Value};

/// (메서드 이름, 한 줄 설명, 주요 매개변수)
pub const METHODS: &[(&str, &str, &str)] = &[
    ("server.status", "Server version, session, pid, uptime", ""),
    ("server.stop", "Stop the server and every pane process", ""),
    (
        "server.reload_config",
        "Reload config.toml, policy.toml and manifests",
        "",
    ),
    (
        "session.snapshot",
        "Workspaces, tabs, layouts and panes",
        "",
    ),
    ("client.attach", "Attach a rendering client", "cols, rows"),
    (
        "client.viewport",
        "Report the pane area size of a client",
        "cols, rows, header_rows(0|1)",
    ),
    (
        "pane.redraw",
        "Send one pane's screen again to this attached client",
        "pane",
    ),
    (
        "client.input",
        "Raw input from a client to a pane",
        "pane, data(base64)",
    ),
    ("workspace.list", "List workspaces", ""),
    (
        "workspace.create",
        "Create a workspace",
        "cwd, label, focus",
    ),
    ("workspace.rename", "Rename a workspace", "id, label"),
    ("workspace.focus", "Focus a workspace", "id"),
    ("workspace.close", "Close a workspace and its panes", "id"),
    ("tab.list", "List tabs", "workspace"),
    ("tab.create", "Create a tab", "workspace, label, focus"),
    ("tab.rename", "Rename a tab", "id, label"),
    ("tab.focus", "Focus a tab", "id | workspace+offset"),
    ("tab.close", "Close a tab", "id"),
    ("pane.list", "List panes", ""),
    (
        "pane.split",
        "Split a pane",
        "target, direction(right|down), cwd, command, profile, focus",
    ),
    (
        "pane.run",
        "Type a command line into a pane and press Enter",
        "pane, command",
    ),
    ("pane.send", "Send raw text/keys to a pane", "pane, text"),
    (
        "pane.read",
        "Read pane text (always redacted)",
        "pane, source(screen|recent), lines",
    ),
    (
        "pane.wait_output",
        "Wait until pane text matches a regex",
        "pane, regex, timeout_ms",
    ),
    ("pane.process_info", "Foreground process of a pane", "pane"),
    ("pane.focus", "Focus a pane", "pane"),
    ("pane.zoom", "Toggle zoom of a pane", "pane"),
    (
        "layout.set_ratio",
        "Set the ratio of one split (mouse border drag); people only",
        "tab?, path (list of bool: false=first, true=second child), ratio",
    ),
    ("pane.close", "Close a pane", "pane"),
    ("agent.list", "List detected agents", ""),
    (
        "agent.start",
        "Start an agent in an idle shell pane",
        "name, kind, pane",
    ),
    (
        "agent.prompt",
        "Send a prompt to an agent",
        "target, text, wait, timeout_ms",
    ),
    (
        "agent.read",
        "Read agent output (redacted)",
        "target, lines",
    ),
    (
        "agent.wait",
        "Wait for an agent state",
        "target, until, timeout_ms",
    ),
    (
        "agent.explain",
        "Explain how an agent state was detected",
        "target",
    ),
    (
        "worktree.create",
        "Create a git worktree as a workspace",
        "cwd, branch, base, label, path, focus",
    ),
    (
        "worktree.remove",
        "Remove a worktree workspace",
        "workspace, force",
    ),
    (
        "events.subscribe",
        "Subscribe to events (notifications named `event`)",
        "kinds",
    ),
    ("approval.list", "Pending approvals", ""),
    (
        "approval.decide",
        "Allow or deny a pending approval (users only)",
        "id, allow",
    ),
    ("control.snapshot", "Everything the control view needs", ""),
    ("audit.status", "Ledger integrity status", ""),
    ("policy.show", "Effective policy", ""),
    (
        "hook.event",
        "Tool-call hook event from an agent",
        "agent, phase, payload",
    ),
    ("plugin.list", "Installed plugins", ""),
    ("schema", "This OpenRPC document", ""),
];

/// OpenRPC 1.2 형식의 문서를 만든다.
pub fn openrpc() -> Value {
    let methods: Vec<Value> = METHODS
        .iter()
        .map(|(name, summary, params)| {
            let params: Vec<Value> = params
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| json!({"name": p, "schema": {}}))
                .collect();
            json!({"name": name, "summary": summary, "params": params, "result": {"name": "result", "schema": {}}})
        })
        .collect();
    json!({
        "openrpc": "1.2.6",
        "info": {"title": "Securidge KMttt", "version": env!("CARGO_PKG_VERSION")},
        "methods": methods,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn method_names_unique() {
        let mut names: Vec<_> = super::METHODS.iter().map(|m| m.0).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(len, names.len());
    }
}
