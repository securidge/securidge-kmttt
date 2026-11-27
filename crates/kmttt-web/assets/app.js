// Securidge KMttt web console. All text goes through textContent (never innerHTML) because
// pane output and agent names are untrusted.
"use strict";

const body = document.body;
const LANG = body.dataset.lang === "ko" ? "ko" : "en";
const ACTIONS = body.dataset.actions === "1";
const TOKEN = new URLSearchParams(location.search).get("t") || "";
// Keep the token out of the address bar and history.
history.replaceState(null, "", "/");

const T = {
  en: { title: "Control Center", agents: "Agents", approvals: "Approvals", events: "Security events", flows: "Agent-to-agent messages", tools: "Tool calls", integrity: "Ledger integrity",
        agent: "agent", pane: "pane", state: "state", for: "for", sandbox: "sandbox", signal: "signal", last: "last line", empty: "nothing here",
        allow: "Allow", deny: "Deny", readonly: "read-only console", pending: "pending", blocked: "blocked today", ok: "ledger OK", fail: "ledger FAIL",
        working: "working", blockedS: "blocked", done: "done", idle: "idle", unknown: "unknown", records: "records", checkpoint: "last checkpoint", key: "key", level: "level", updated: "updated" },
  ko: { title: "통합 관제", agents: "에이전트", approvals: "승인 대기", events: "보안 이벤트", flows: "에이전트 간 메시지", tools: "도구 호출", integrity: "감사 기록 무결성",
        agent: "에이전트", pane: "pane", state: "상태", for: "유지", sandbox: "격리", signal: "신호", last: "마지막 줄", empty: "없음",
        allow: "허용", deny: "거부", readonly: "읽기 전용 콘솔", pending: "승인 대기", blocked: "오늘 차단", ok: "기록 정상", fail: "기록 이상",
        working: "작업 중", blockedS: "입력 대기", done: "완료", idle: "대기", unknown: "알 수 없음", records: "기록", checkpoint: "마지막 체크포인트", key: "키", level: "수준", updated: "갱신" },
}[LANG];

document.querySelectorAll("[data-i18n]").forEach((el) => { const k = el.dataset.i18n; if (T[k]) el.textContent = T[k]; });

const ICON = { working: "●", blocked: "▲", done: "✓", idle: "○", unknown: "?" };
const PRIO = { blocked: 0, done: 1, working: 2, unknown: 3, idle: 4 };
const stateLabel = (s) => (s === "blocked" ? T.blockedS : T[s] || s);

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined && text !== null) e.textContent = String(text);
  return e;
}
function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }
function hhmmss(ms) { const d = new Date(ms); return d.toTimeString().slice(0, 8); }
function ago(ms) { const s = Math.max(0, Math.floor((Date.now() - ms) / 1000)); return s < 60 ? s + "s" : s < 3600 ? Math.floor(s / 60) + "m" : Math.floor(s / 3600) + "h"; }
function mb(b) { return b > 1 << 30 ? (b / (1 << 30)).toFixed(1) + "G" : Math.round(b / (1 << 20)) + "M"; }
function decisionClass(d) { return d.endsWith("deny") ? "d-deny" : d.startsWith("ask") ? "d-ask" : "d-allow"; }

async function api(path, opts) {
  const res = await fetch(path, Object.assign({ headers: { "X-KMttt-Token": TOKEN }, cache: "no-store" }, opts || {}));
  if (!res.ok) throw new Error(res.status + " " + (await res.text()));
  return res.json();
}

function renderSummary(s) {
  const box = document.getElementById("summary");
  clear(box);
  const counts = { blocked: 0, done: 0, working: 0, idle: 0 };
  s.panes.forEach((p) => { if (p.agent && counts[p.agent.state] !== undefined) counts[p.agent.state]++; });
  box.appendChild(el("span", "pill", `${s.machine} · ${s.session}`));
  box.appendChild(el("span", "pill s-blocked", `▲ ${counts.blocked}`));
  box.appendChild(el("span", "pill s-done", `✓ ${counts.done}`));
  box.appendChild(el("span", "pill s-working", `● ${counts.working}`));
  box.appendChild(el("span", "pill", `○ ${counts.idle}`));
  box.appendChild(el("span", "pill" + (s.approvals.length ? " warn" : ""), `⚑ ${s.approvals.length} ${T.pending}`));
  box.appendChild(el("span", "pill" + (s.blocked_today ? " bad" : ""), `⊘ ${s.blocked_today} ${T.blocked}`));
  box.appendChild(el("span", "pill " + (s.integrity.ok ? "ok" : "bad"), s.integrity.ok ? T.ok : T.fail));
}

function renderAgents(s) {
  const tb = document.querySelector("#agents tbody");
  clear(tb);
  const panes = s.panes.slice().sort((a, b) => (a.agent ? PRIO[a.agent.state] : 9) - (b.agent ? PRIO[b.agent.state] : 9));
  if (!panes.length) { const tr = el("tr"); const td = el("td", "empty", T.empty); td.colSpan = 10; tr.appendChild(td); tb.appendChild(tr); return; }
  panes.forEach((p) => {
    const a = p.agent;
    const tr = el("tr");
    const st = a ? a.state : "";
    tr.appendChild(el("td", "s-" + st, a ? ICON[st] : ""));
    tr.appendChild(el("td", "", a ? (a.name || a.kind) : (p.command.split(/[\\/]/).pop())));
    tr.appendChild(el("td", "", p.id));
    tr.appendChild(el("td", "s-" + st, a ? stateLabel(st) : ""));
    tr.appendChild(el("td", "", a ? ago(a.since_ms) : ""));
    tr.appendChild(el("td", p.sandbox === "none" ? "s-idle" : "s-done", `${p.sandbox}/${p.sandbox_strength}`));
    tr.appendChild(el("td", "d-deny", p.blocked_calls ? "⊘" + p.blocked_calls : ""));
    tr.appendChild(el("td", "", `${p.cpu.toFixed(0)}% ${mb(p.mem_bytes)}`));
    tr.appendChild(el("td", "s-idle", a ? (a.hooked ? "hook" : "screen") : ""));
    tr.appendChild(el("td", "last", p.last_line));
    tb.appendChild(tr);
  });
}

function renderApprovals(s) {
  const ul = document.getElementById("approvals");
  clear(ul);
  if (!s.approvals.length) { ul.appendChild(el("li", "empty", T.empty)); return; }
  s.approvals.forEach((a) => {
    const li = el("li", "approval");
    li.appendChild(el("span", "id", "#" + a.id));
    li.appendChild(el("span", "", `${a.caller} → ${a.action} ${a.target}`));
    li.appendChild(el("span", "t", Math.max(0, Math.floor((a.expires_ms - Date.now()) / 1000)) + "s"));
    if (ACTIONS) {
      const ok = el("button", "allow", T.allow);
      const no = el("button", "deny", T.deny);
      ok.addEventListener("click", () => decide(a.id, "allow"));
      no.addEventListener("click", () => decide(a.id, "deny"));
      li.appendChild(ok); li.appendChild(no);
    }
    li.appendChild(el("div", "detail", a.detail));
    ul.appendChild(li);
  });
}

function renderList(id, items, fn) {
  const ul = document.getElementById(id);
  clear(ul);
  if (!items.length) { ul.appendChild(el("li", "empty", T.empty)); return; }
  items.slice(0, 120).forEach((x) => ul.appendChild(fn(x)));
}

function renderIntegrity(s) {
  const box = document.getElementById("integrity");
  clear(box);
  const i = s.integrity;
  box.className = "kv";
  box.appendChild(el("div", i.ok ? "d-allow" : "d-deny", i.ok ? "● " + T.ok : "✗ " + T.fail));
  box.appendChild(el("div", "", `${T.records}: ${i.records}`));
  box.appendChild(el("div", "", `${T.level}: ${i.level}`));
  box.appendChild(el("div", "", `${T.checkpoint}: ${i.last_checkpoint_ms ? hhmmss(i.last_checkpoint_ms) : "-"}`));
  box.appendChild(el("div", "", `${T.key}: ${i.key_fingerprint}`));
  box.appendChild(el("div", i.ok ? "t" : "d-deny", i.detail));
}

async function decide(id, verdict) {
  try { await api(`/api/approval/${id}/${verdict}`, { method: "POST" }); refresh(); }
  catch (e) { document.getElementById("status").textContent = String(e); }
}

async function refresh() {
  const status = document.getElementById("status");
  try {
    const s = await api("/api/snapshot");
    renderSummary(s);
    renderAgents(s);
    renderApprovals(s);
    renderList("events", s.events, (e) => { const li = el("li"); li.appendChild(el("span", "t", hhmmss(e.ts_ms))); li.appendChild(el("span", "sev-" + e.severity, `${e.kind} `)); li.appendChild(el("span", "", `${e.caller} ${e.target} — ${e.detail}`)); return li; });
    renderList("flows", s.flows, (f) => { const li = el("li"); li.appendChild(el("span", "t", hhmmss(f.ts_ms))); li.appendChild(el("span", "", `${f.from} ─${f.action}→ ${f.to} `)); li.appendChild(el("span", decisionClass(f.decision), f.decision)); return li; });
    renderList("tools", s.tools, (t) => { const li = el("li"); li.appendChild(el("span", "t", hhmmss(t.ts_ms))); li.appendChild(el("span", "s-working", `${t.pane} `)); li.appendChild(el("span", "", `${t.tool} ${t.summary} `)); li.appendChild(el("span", decisionClass(t.decision), t.decision)); return li; });
    renderIntegrity(s);
    status.textContent = `${ACTIONS ? "" : T.readonly + " · "}${T.updated} ${hhmmss(Date.now())} · v${body.dataset.version}`;
  } catch (e) {
    status.textContent = String(e);
  }
}

refresh();
setInterval(refresh, 1500);
