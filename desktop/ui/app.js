// KMttt Desktop 화면.
// 규칙: 화면에 넣는 글자는 모두 textContent로만 넣는다(innerHTML 금지). 서버에서 온 이름·출력은 믿지 않는다.
"use strict";

(() => {
  const TAURI = window.__TAURI__;
  const invoke = (cmd, args) => TAURI.core.invoke(cmd, args || {});
  const listen = (ev, fn) => TAURI.event.listen(ev, fn);
  const I18N = window.KM_I18N;
  const IS_WIN = navigator.userAgent.includes("Windows");
  const IS_MAC = navigator.userAgent.includes("Mac");

  const S = {
    lang: "ko",
    prefs: {},
    gen: 0,
    connected: false,
    connecting: false,
    everConnected: false,
    connError: "",
    session: "default",
    snap: null,
    approvals: [],
    control: null,
    view: "terminal",
    kinds: [],
    cell: { w: 8, h: 17 },
    vp: { cols: 0, rows: 0 },
    info: {},
    policy: null,
    drag: null,
    modals: 0,
    lastFocus: "",
    usage: null,
    tokOpen: false,
    tokens: null,
    notified: new Set(),
    logFilter: { sev: "", pane: "", range: "1h", q: "" },
  };
  const terms = new Map(); // pane id -> { term, el, badge }

  // ───────────── 작은 도구 ─────────────
  const $ = (sel) => document.querySelector(sel);
  function t(k, vars) {
    let s = (I18N[S.lang] && I18N[S.lang][k]) ?? I18N.en[k] ?? k;
    if (vars) for (const [a, b] of Object.entries(vars)) s = s.split("{" + a + "}").join(String(b));
    return s;
  }
  function h(tag, props, ...kids) {
    const el = document.createElement(tag);
    if (props) {
      for (const [k, v] of Object.entries(props)) {
        if (v == null || v === false) continue;
        if (k === "class") el.className = v;
        else if (k === "text") el.textContent = v;
        else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
        else if (k === "style") Object.assign(el.style, v);
        else if (k === "value") el.value = v;
        else if (k === "checked") el.checked = !!v;
        else el.setAttribute(k, v === true ? "" : String(v));
      }
    }
    for (const c of kids.flat(Infinity)) {
      if (c == null || c === false) continue;
      el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }
  function clear(el) {
    while (el.firstChild) el.removeChild(el.firstChild);
    return el;
  }
  // 선 아이콘(SVG). path의 d 값만 받는다.
  function icon(paths, size) {
    const NS = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(NS, "svg");
    for (const [k, v] of Object.entries({ viewBox: "0 0 24 24", width: size || 16, height: size || 16, fill: "none", stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true", class: "ico" }))
      svg.setAttribute(k, String(v));
    for (const d of paths) {
      const el = document.createElementNS(NS, "path");
      el.setAttribute("d", d);
      svg.append(el);
    }
    return svg;
  }
  const ICON = {
    splitRight: ["M4 5h16v14H4z", "M12 5v14"],
    splitDown: ["M4 5h16v14H4z", "M4 12h16"],
    workspace: ["M4 5h16v14H4z", "M12 9v6", "M9 12h6"],
    tab: ["M4 9h16v10H4z", "M4 9l2-4h6l2 4"],
    chart: ["M4 19h16", "M4 15l4-6 4 3 4-6 4 4"],
    search: ["M11 11m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0", "M20 20l-4.5-4.5"],
    help: ["M12 12m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0", "M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.5v.4", "M12 17h.01"],
    gear: ["M10.3 4.3a1.7 1.7 0 0 1 3.4 0l.2 1.1a7 7 0 0 1 1.7 1l1-.4a1.7 1.7 0 0 1 2.1 2.6l-.8.7a7 7 0 0 1 0 1.9l.8.7a1.7 1.7 0 0 1-2.1 2.6l-1-.4a7 7 0 0 1-1.7 1l-.2 1.1a1.7 1.7 0 0 1-3.4 0l-.2-1.1a7 7 0 0 1-1.7-1l-1 .4a1.7 1.7 0 0 1-2.1-2.6l.8-.7a7 7 0 0 1 0-1.9l-.8-.7a1.7 1.7 0 0 1 2.1-2.6l1 .4a7 7 0 0 1 1.7-1z", "M12 10m-2.5 0a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0"],
    play: ["M7 5l12 7-12 7z"],
    info: ["M12 12m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0", "M12 11v5", "M12 8h.01"],
  };
  function b64ToBytes(s) {
    const bin = atob(s);
    const u = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    return u;
  }
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const rawErr = (e) => (typeof e === "string" ? e : e && e.message ? e.message : JSON.stringify(e));
  // 서버가 돌려주는 영어 오류 중 자주 보는 것은 쉬운 말로 바꾸고, 원문은 괄호로 남긴다.
  const ERR_MAP = [
    [/an agent named '([^']+)' already exists/, (m) => t("err.name_taken", { n: m[1] })],
    [/is busy; agent\.start needs an idle shell prompt/, () => t("err.pane_busy")],
    [/waiting for an answer \(blocked\)/, () => t("err.agent_waiting")],
    [/unknown agent kind '([^']+)'/, (m) => t("err.unknown_kind", { k: m[1] })],
    [/denied by policy/, () => t("err.denied")],
    [/did not reach .* in time|did not appear in time/, () => t("err.timeout")],
  ];
  const errText = (e) => {
    const raw = rawErr(e);
    for (const [re, f] of ERR_MAP) {
      const m = raw.match(re);
      if (m) return f(m) + " (" + raw + ")";
    }
    return raw;
  };

  async function rpc(method, params) {
    return invoke("rpc", { method, params: params || {} });
  }
  async function tryRpc(method, params) {
    try {
      return await rpc(method, params);
    } catch (e) {
      showError(errText(e));
      return null;
    }
  }
  function savePrefs() {
    invoke("prefs_set", { prefs: S.prefs }).catch(() => {});
  }

  // ───────────── 상태 조회 ─────────────
  const snap = () => S.snap || { workspaces: [], panes: [], focus_workspace: "" };
  const paneInfo = (id) => snap().panes.find((p) => p.id === id);
  const focusedWs = () => snap().workspaces.find((w) => w.id === snap().focus_workspace);
  function focusedTab() {
    const ws = focusedWs();
    return ws ? ws.tabs.find((x) => x.id === ws.focus_tab) : null;
  }
  const focusedPane = () => (focusedTab() ? focusedTab().focus : null);
  const agents = () => snap().panes.filter((p) => p.agent);
  // 창 제목이 실행 파일 전체 경로(예: C:\WINDOWS\...\powershell.exe)면 파일 이름만 보인다.
  const baseName = (s) => String(s || "").split(/[\\/]/).filter(Boolean).pop() || String(s || "");
  const agentLabel = (p) => (p.agent ? p.agent.name || p.agent.kind : baseName(p.title || p.command || p.id));
  const stateIcon = { working: "●", blocked: "▲", done: "✓", idle: "○", unknown: "?" };
  const statePri = { blocked: 0, done: 1, working: 2, unknown: 3, idle: 4 };
  // 보호 수준 이름을 쉬운 말로: "restricted" → "기본 보호"
  const profName = (p) => (I18N.en["profile." + p] ? t("profile." + p).split(" (")[0] : p || "none");
  const defaultProfile = () => (S.policy && S.policy.sandbox ? S.policy.sandbox.default_profile : "none");
  function wsPanes(ws) {
    const ids = new Set();
    for (const tb of ws.tabs) collectPanes(tb.layout, ids);
    return snap().panes.filter((p) => ids.has(p.id));
  }
  function collectPanes(node, set) {
    if (!node) return;
    if (node.type === "leaf") set.add(node.pane);
    else {
      collectPanes(node.a, set);
      collectPanes(node.b, set);
    }
  }

  // 승인 요청에 나오는 서버 메서드·창 ID를 사람이 읽는 말로 바꾼다(원래 값은 괄호로 함께 보여 준다).
  function actionName(m) {
    const k = "ap.m." + String(m || "").replace(/\./g, "_");
    return (I18N.en[k] ? t(k) : m) + (I18N.en[k] ? " (" + m + ")" : "");
  }
  // 서버 알림(설정 언어로 만든 짧은 문장)을 앱 언어의 쉬운 문장으로 바꾼다.
  function localToast(text) {
    let m = text.match(/^⚑ (?:approval requested|승인 요청) #(\d+): (\S+) → (\S+) (\S+)$/);
    if (m) return t("toast.approval", { id: m[1], from: describePane(m[2]), act: actionName(m[3]), to: describePane(m[4]) });
    m = text.match(/^✓ (\S+) (\S+): (?:finished|작업 끝남)$/);
    if (m) return t("toast.done", { name: m[2] });
    m = text.match(/^▲ (\S+) (\S+): (?:needs input|입력이 필요함)$/);
    if (m) return t("toast.blocked", { name: m[2] });
    return text;
  }
  function describePane(id) {
    const p = paneInfo(id);
    if (!p) return String(id || "");
    const ws = snap().workspaces.find((w) => w.id === p.workspace);
    return t("ap.pane", { name: agentLabel(p), ws: ws ? ws.label : p.workspace }) + " (" + id + ")";
  }

  // ───────────── 동작 목록(메뉴·단축키·팔레트·안내가 함께 쓴다) ─────────────
  const ACTIONS = [
    { id: "new_workspace", menu: "file", keys: "Ctrl+Shift+N", run: () => dlgNewWorkspace(), kw: "workspace folder project 폴더 프로젝트" },
    { id: "new_tab", menu: "file", keys: "Ctrl+Shift+T", run: () => tryRpc("tab.create", { focus: true }), needConn: true },
    { id: "connect_session", menu: "file", run: () => dlgSession() },
    { id: "reconnect", menu: "file", run: () => connect(true) },
    { id: "stop_server", menu: "file", sep: true, run: () => stopServer(), needConn: true },
    { id: "rename_workspace", menu: "workspace", run: () => renameWorkspace(), needConn: true },
    { id: "next_workspace", menu: "workspace", keys: "Ctrl+Shift+PageDown", run: () => cycleWorkspace(1), needConn: true },
    { id: "prev_workspace", menu: "workspace", keys: "Ctrl+Shift+PageUp", run: () => cycleWorkspace(-1), needConn: true },
    { id: "close_tab", menu: "workspace", run: () => closeTab(), needConn: true },
    { id: "close_workspace", menu: "workspace", sep: true, run: () => closeWorkspace(), needConn: true },
    { id: "split_right", menu: "pane", keys: "Alt+Shift+=", run: () => split("right"), needConn: true, kw: "split vertical 나누기 세로" },
    { id: "split_down", menu: "pane", keys: "Alt+Shift+-", run: () => split("down"), needConn: true, kw: "split horizontal 나누기 가로" },
    { id: "zoom", menu: "pane", keys: "Ctrl+Shift+Z", run: () => zoom(), needConn: true },
    { id: "copy", menu: "pane", keys: "Ctrl+Shift+C", run: () => copySel(), sep: true },
    { id: "paste", menu: "pane", keys: "Ctrl+Shift+V", run: () => paste() },
    { id: "close_pane", menu: "pane", keys: "Ctrl+Shift+W", run: () => closePane(), sep: true, needConn: true },
    { id: "start_agent", menu: "agent", keys: "Ctrl+Shift+A", run: () => dlgStartAgent(), needConn: true, kw: "claude codex gemini copilot 시작" },
    { id: "prompt_agent", menu: "agent", keys: "Ctrl+Shift+I", run: () => dlgPrompt(), needConn: true, kw: "prompt send 지시 보내기" },
    { id: "explain_agent", menu: "agent", run: () => explainAgent(), needConn: true },
    { id: "usage_connect", menu: "agent", sep: true, run: () => dlgUsageConnect(), kw: "claude usage quota limit 사용량 한도 리셋" },
    { id: "approvals", menu: "security", keys: "Ctrl+Shift+Y", run: () => setView("approvals"), kw: "approve allow deny 승인 허용 거부" },
    { id: "control", menu: "security", keys: "Ctrl+Shift+O", run: () => setView("control"), kw: "overview 관제" },
    { id: "log", menu: "security", keys: "Ctrl+Shift+L", run: () => setView("log") },
    { id: "verify", menu: "security", run: () => verifyLedger(), needConn: true, kw: "audit ledger 무결성 감사" },
    { id: "settings_policy", menu: "security", sep: true, run: () => dlgSettings("policy") },
    { id: "terminal", menu: "view", keys: "Ctrl+Shift+E", run: () => setView("terminal") },
    { id: "usage_refresh", menu: "view", run: () => pollUsage(true), kw: "usage quota limit token 사용량 한도 토큰" },
    { id: "tokens", menu: "view", run: () => toggleTokens(), kw: "token usage chart 토큰 사용량 차트" },
    { id: "sidebar", menu: "view", keys: "Ctrl+B", run: () => toggleSidebar(), kw: "sidebar collapse 사이드바 접기" },
    { id: "font_bigger", menu: "view", keys: "Ctrl+=", run: () => setFont(+1), sep: true },
    { id: "font_smaller", menu: "view", keys: "Ctrl+-", run: () => setFont(-1) },
    { id: "theme", menu: "view", run: () => setTheme(S.prefs.theme === "light" ? "dark" : "light") },
    { id: "language", menu: "view", run: () => setLang(S.lang === "ko" ? "en" : "ko") },
    { id: "settings", menu: "settings", keys: "Ctrl+,", run: () => dlgSettings("general"), kw: "options preferences 설정 옵션" },
    { id: "guide", menu: "help", keys: "F1", run: () => dlgGuide("start"), kw: "help howto glossary 도움말 사용법 용어" },
    { id: "onboarding", menu: "help", run: () => dlgOnboarding(), kw: "start onboarding 시작 안내" },
    { id: "wizard", menu: "help", run: () => wizard() },
    { id: "palette", menu: "help", keys: "Ctrl+Shift+P", run: () => palette() },
    { id: "shortcuts", menu: "help", run: () => dlgShortcuts() },
    { id: "uninstall_help", menu: "help", sep: true, run: () => dlgGuide("about"), kw: "uninstall remove 제거 삭제" },
    { id: "about", menu: "help", run: () => dlgAbout() },
  ];
  const actionById = (id) => ACTIONS.find((a) => a.id === id);
  function runAction(id) {
    const a = actionById(id);
    if (!a) return;
    closeMenus();
    if (a.needConn && !S.connected) {
      showToast(t("conn.disconnected"), "warn");
      return;
    }
    Promise.resolve(a.run()).catch((e) => showError(errText(e)));
  }
  const actLabel = (a) => t("act." + a.id);

  // 단축키: e.code로 비교해 한글 자판·IME에서도 같은 키로 동작하게 한다.
  function keyName(e) {
    const c = e.code || "";
    if (c.startsWith("Key")) return c.slice(3);
    if (c.startsWith("Digit")) return c.slice(5);
    const map = { Equal: "=", Minus: "-", Comma: ",", PageUp: "PageUp", PageDown: "PageDown", F1: "F1", NumpadAdd: "=", NumpadSubtract: "-" };
    return map[c] || e.key;
  }
  function comboOf(e) {
    const parts = [];
    if (e.ctrlKey || (IS_MAC && e.metaKey)) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    parts.push(keyName(e));
    return parts.join("+");
  }
  const keyLabel = (k) => (IS_MAC ? k.replace("Ctrl+", "⌘") : k);

  document.addEventListener(
    "keydown",
    (e) => {
      if (e.isComposing) return;
      const combo = comboOf(e);
      if (combo === "Escape" || keyName(e) === "Escape") {
        if (closeMenus()) {
          e.preventDefault();
        } else if (S.tokOpen && S.modals === 0) {
          e.preventDefault();
          toggleTokens(false);
        }
        return;
      }
      // 알림 띠의 첫 승인 요청: Alt+Y 승인, Alt+N 거부. 실제 키 입력만 받는다(e.isTrusted).
      if ((combo === "Alt+Y" || combo === "Alt+N") && S.approvals.length && S.modals === 0 && S.alertIsApproval) {
        e.preventDefault();
        e.stopPropagation();
        decideApproval(e, S.approvals[0], combo === "Alt+Y");
        return;
      }
      // Windows 터미널처럼: Ctrl+V 붙여넣기, 선택이 있을 때만 Ctrl+C 복사
      const inTerm = e.target && e.target.closest && e.target.closest(".pane");
      if (IS_WIN && inTerm && combo === "Ctrl+V") {
        e.preventDefault();
        e.stopPropagation();
        paste();
        return;
      }
      if (inTerm && combo === "Ctrl+C") {
        const tm = currentTerm();
        if (tm && tm.hasSelection()) {
          e.preventDefault();
          e.stopPropagation();
          copySel();
          return;
        }
      }
      // 대화 상자가 열려 있으면 팔레트·설정 외 단축키는 쓰지 않는다.
      const a = ACTIONS.find((x) => x.keys && x.keys === combo);
      if (!a) return;
      if (S.modals > 0 && !["palette"].includes(a.id)) return;
      e.preventDefault();
      e.stopPropagation();
      runAction(a.id);
    },
    true,
  );

  // ───────────── 메뉴바·툴바 ─────────────
  const MENUS = ["file", "workspace", "pane", "agent", "security", "view", "settings", "help"];
  let openMenu = null;
  function closeMenus() {
    const had = !!openMenu || !!document.querySelector(".ctxmenu");
    document.querySelectorAll(".dropdown, .ctxmenu").forEach((x) => x.remove());
    document.querySelectorAll(".menubtn.open").forEach((x) => x.classList.remove("open"));
    openMenu = null;
    return had;
  }
  function renderMenubar() {
    const bar = clear($("#menubar"));
    for (const m of MENUS) {
      const btn = h("button", { class: "menubtn", type: "button", text: t("menu." + m) });
      btn.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (m === "settings") {
          closeMenus();
          runAction("settings");
          return;
        }
        if (openMenu === m) {
          closeMenus();
          return;
        }
        closeMenus();
        openMenu = m;
        btn.classList.add("open");
        const dd = h("div", { class: "dropdown", role: "menu" });
        for (const a of ACTIONS.filter((x) => x.menu === m)) {
          if (a.sep && dd.childElementCount) dd.append(h("div", { class: "sep" }));
          const dis = a.needConn && !S.connected;
          dd.append(
            h(
              "button",
              { class: "item", type: "button", role: "menuitem", disabled: dis, onclick: () => runAction(a.id) },
              h("span", { text: actLabel(a) }),
              h("span", { class: "keys", text: a.keys ? keyLabel(a.keys) : "" }),
            ),
          );
        }
        const r = btn.getBoundingClientRect();
        dd.style.left = r.left + "px";
        dd.style.top = r.bottom + "px";
        document.body.append(dd);
      });
      btn.addEventListener("mouseenter", () => {
        if (openMenu && openMenu !== m && m !== "settings") btn.click();
      });
      bar.append(btn);
    }
  }
  document.addEventListener("click", () => closeMenus());

  function renderToolbar() {
    const tb = clear($("#toolbar"));
    const b = (id, kids, cls, title) =>
      h("button", { class: "tbtn " + (cls || ""), type: "button", title: title || actLabel(actionById(id)), "aria-label": title || actLabel(actionById(id)), onclick: () => runAction(id) }, kids);
    tb.append(
      b("start_agent", [icon(ICON.play), t("act.start_agent")], "primary ic", t("act.start_agent") + " · " + keyLabel("Ctrl+Shift+A")),
      h("span", { class: "tsep" }),
      b("new_workspace", [icon(ICON.workspace), t("sb.workspaces")], "ic", t("act.new_workspace") + " · " + keyLabel("Ctrl+Shift+N")),
      b("new_tab", [icon(ICON.tab), t("tb.tab")], "ic", t("act.new_tab") + " · " + keyLabel("Ctrl+Shift+T")),
      b("split_right", [icon(ICON.splitRight)], "ic only", t("act.split_right.short") + " · Alt+Shift+="),
      b("split_down", [icon(ICON.splitDown)], "ic only", t("act.split_down.short") + " · Alt+Shift+-"),
      h("span", { class: "grow" }),
      b("palette", [icon(ICON.search), t("act.palette"), h("kbd", { text: keyLabel("Ctrl+Shift+P") })], "ghost ic", t("act.palette")),
      b("guide", [icon(ICON.help)], "ghost ic only", t("act.guide") + " · F1"),
      b("settings", [icon(ICON.gear)], "ghost ic only", t("act.settings") + " · " + keyLabel("Ctrl+,")),
    );
  }

  // ───────────── 연결 ─────────────
  async function connect(start) {
    if (S.connecting) return;
    S.connecting = true;
    S.connError = "";
    renderStatus();
    renderPanes();
    measureViewport();
    try {
      const r = await invoke("connect", { session: S.session, cols: S.vp.cols || 120, rows: S.vp.rows || 40, start: !!start });
      S.gen = r.gen;
      S.connected = true;
      S.everConnected = true;
      S.snap = r.snapshot;
      S.info = await invoke("app_info");
      await loadPolicy();
      pollNow();
      sendViewport(true);
    } catch (e) {
      S.connected = false;
      S.connError = errText(e);
    }
    S.connecting = false;
    renderAll();
  }
  async function loadPolicy() {
    try {
      const st = await invoke("settings_read");
      S.policy = st.policy;
      S.config = st.config;
    } catch (_) {
      /* 화면 안내에만 쓰므로 실패해도 넘어간다 */
    }
  }

  listen("kmttt-note", (ev) => {
    const m = ev.payload;
    if (!m || m.gen !== S.gen) return;
    const p = m.params || {};
    switch (m.method) {
      case "snapshot":
        S.snap = p;
        renderAll();
        break;
      case "pane.bootstrap": {
        const e = ensureTerm(p.pane, p.cols || 80, p.rows || 24);
        e.term.reset();
        if (e.term.cols !== p.cols || e.term.rows !== p.rows) e.term.resize(p.cols || 80, p.rows || 24);
        e.term.write(b64ToBytes(p.data || ""));
        break;
      }
      case "pane.output": {
        const pi = paneInfo(p.pane);
        const e = ensureTerm(p.pane, pi ? pi.cols : 80, pi ? pi.rows : 24);
        e.term.write(b64ToBytes(p.data || ""));
        break;
      }
      case "toast":
        if (S.prefs.toast !== "off" || p.level === "critical") showToast(localToast(p.text || ""), p.level || "info");
        break;
      default:
        break;
    }
  });
  listen("kmttt-disconnected", (ev) => {
    if (!ev.payload || ev.payload.gen !== S.gen) return;
    S.connected = false;
    renderAll();
  });

  // 서버가 꺼지면 3초마다 다시 붙어 본다(서버를 새로 띄우지는 않는다: 사람이 끈 것일 수 있다).
  setInterval(() => {
    if (!S.connected && !S.connecting && S.everConnected) connect(false);
  }, 3000);

  // 승인 목록·관제 정보는 주기적으로 받아 온다.
  let pollTick = 0;
  async function pollNow() {
    if (!S.connected) return;
    try {
      S.approvals = (await rpc("approval.list")) || [];
      notifyApprovals();
    } catch (_) {
      /* 연결 끊김은 disconnected 이벤트가 처리 */
    }
    if (S.view === "control" || S.view === "log" || pollTick % 6 === 0) {
      try {
        S.control = await rpc("control.snapshot");
      } catch (_) {}
    }
    pollTick++;
    renderViewTabs();
    renderAlerts();
    renderTip();
    renderSidebar();
    renderStatusCounts();
    if (S.view === "approvals") renderApprovals();
    if (S.view === "control") renderControl();
    if (S.view === "log") renderLog();
  }
  setInterval(pollNow, 1500);

  // ───────────── 터미널 ─────────────
  function termTheme() {
    return S.prefs.theme === "light"
      ? { background: "#fbfbf9", foreground: "#1f2328", cursor: "#1f2328", selectionBackground: "#c8dcf5", black: "#24292f", brightBlack: "#57606a" }
      : { background: "#15171c", foreground: "#d9dee7", cursor: "#e7c66b", selectionBackground: "#36506e" };
  }
  const FONT = '"Cascadia Mono", "D2Coding", Consolas, "Malgun Gothic", Menlo, "Apple SD Gothic Neo", "Noto Sans Mono CJK KR", "DejaVu Sans Mono", monospace';
  const fontSize = () => clamp(S.prefs.fontSize || 14, 9, 28);

  function newTerminal(cols, rows) {
    return new window.Terminal({
      cols: Math.max(2, cols),
      rows: Math.max(1, rows),
      fontFamily: FONT,
      fontSize: fontSize(),
      theme: termTheme(),
      scrollback: 5000,
      cursorBlink: true,
      macOptionIsMeta: true,
      linkHandler: { activate: (_ev, uri) => confirmOpenUrl(uri) },
    });
  }

  // 터미널 질의(커서 위치·장치 속성 등)에는 서버가 이미 답한다(Screen::feed_and_reply).
  // xterm.js까지 답하면 그 답이 키 입력처럼 pane에 들어가 셸에 엉뚱한 글자가 찍힌다
  // (실제 Windows ConPTY + PowerShell에서 "?"가 입력되는 것으로 확인). 그래서 화면 쪽 답은 막는다.
  function muteReports(term) {
    const csi = (id) => term.parser.registerCsiHandler(id, () => true);
    csi({ final: "n" });
    csi({ prefix: "?", final: "n" });
    csi({ final: "c" });
    csi({ prefix: ">", final: "c" });
    csi({ prefix: "=", final: "c" });
    csi({ intermediates: "$", final: "p" });
    csi({ prefix: "?", intermediates: "$", final: "p" });
    csi({ final: "t" });
    term.parser.registerDcsHandler({ intermediates: "$", final: "q" }, () => true);
    for (const n of [4, 10, 11, 12]) term.parser.registerOscHandler(n, (data) => data.includes("?"));
  }

  function ensureTerm(id, cols, rows) {
    let e = terms.get(id);
    if (e) return e;
    const host = h("div", { class: "pane-term" });
    // 창 제목 줄: 서버가 창 높이에서 한 줄을 비워 주므로 터미널 글자를 가리지 않는다.
    const badge = h("div", { class: "pane-head" });
    const el = h("div", { class: "pane", "data-pane": id }, badge, host);
    el.style.display = "none";
    $("#panes").append(el);
    const term = newTerminal(cols, rows);
    term.open(host);
    muteReports(term);
    term.onData((d) => invoke("input", { pane: id, data: d }).catch(() => {}));
    term.onBinary((d) => invoke("input", { pane: id, data: d, binary: true }).catch(() => {}));
    el.addEventListener("mousedown", () => {
      if (focusedPane() !== id && S.connected) {
        S.lastFocus = id;
        rpc("pane.focus", { pane: id }).catch(() => {});
      }
    });
    el.addEventListener("contextmenu", (ev) => paneMenu(ev, id));
    e = { term, el, badge };
    terms.set(id, e);
    return e;
  }
  const currentTerm = () => {
    const e = terms.get(focusedPane());
    return e ? e.term : null;
  };

  async function measureCell() {
    try {
      await document.fonts.ready;
    } catch (_) {}
    const host = h("div", { class: "probe" });
    document.body.append(host);
    const probe = newTerminal(20, 4);
    probe.open(host);
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    let w = 0,
      hh = 0;
    const scr = host.querySelector(".xterm-screen");
    if (scr) {
      const r = scr.getBoundingClientRect();
      w = r.width / 20;
      hh = r.height / 4;
    }
    try {
      const d = probe._core._renderService.dimensions.css.cell;
      if (d && d.width > 0 && d.height > 0) {
        w = d.width;
        hh = d.height;
      }
    } catch (_) {
      /* 내부 API가 없으면 화면 크기로 잰 값 사용 */
    }
    probe.dispose();
    host.remove();
    if (w > 2 && hh > 4) S.cell = { w, h: hh };
  }

  // 화면에 보일 때만 잰다. 다른 화면(관제 등)으로 가서 pane 영역이 숨으면 크기가 0이 되는데,
  // 그 값을 서버에 보내면 모든 터미널이 10x3으로 줄어 버린다.
  function measureViewport() {
    const r = $("#panes").getBoundingClientRect();
    if (r.width < 50 || r.height < 30) return false;
    S.vp = { cols: Math.max(10, Math.floor((r.width - 4) / S.cell.w)), rows: Math.max(3, Math.floor((r.height - 4) / S.cell.h)) };
    return true;
  }
  let vpTimer = 0;
  let sentVp = "";
  function sendViewport(force) {
    if (!measureViewport()) return;
    const key = S.vp.cols + "x" + S.vp.rows;
    if (!S.connected || (!force && key === sentVp)) return false;
    sentVp = key;
    // header_rows: 창마다 제목 줄 한 줄을 비워 달라는 뜻(서버가 그만큼 터미널 높이를 줄인다).
    rpc("client.viewport", { cols: S.vp.cols, rows: S.vp.rows, header_rows: 1 }).catch(() => {});
    return true;
  }
  // Windows ConPTY는 창 크기가 바뀌면 화면을 지운다. 크기 조절이 끝나고 잠시 뒤
  // 서버가 가진 화면으로 보이는 창들을 다시 그린다.
  let redrawTimer = 0;
  function redrawSoon() {
    clearTimeout(redrawTimer);
    redrawTimer = setTimeout(() => {
      if (!S.connected) return;
      for (const [id, e] of terms) if (e.el.style.display !== "none") rpc("pane.redraw", { pane: id }).catch(() => {});
    }, 250);
  }
  new ResizeObserver(() => {
    clearTimeout(vpTimer);
    vpTimer = setTimeout(() => {
      if (sendViewport(false) && IS_WIN) redrawSoon();
      renderPanes();
    }, 120);
  }).observe(document.getElementById("panes"));

  // 서버(kmttt-core layout::compute)와 같은 계산: 나뉜 두 쪽 사이에 1칸 경계선.
  function computeLayout(node, area, out, divs, path) {
    if (!node) return;
    if (node.type === "leaf") {
      out.push([node.pane, area]);
      return;
    }
    const ratio = clamp(node.ratio, 0.1, 0.9);
    if (node.dir === "right") {
      const usable = Math.max(0, area.w - 1);
      const aw = clamp(Math.round(usable * ratio), Math.min(1, usable), Math.max(usable - 1, 1));
      const bw = Math.max(0, usable - aw);
      divs.push({ path: path.slice(), area, dir: "right", x: area.x + aw, y: area.y, w: 1, h: area.h });
      computeLayout(node.a, { x: area.x, y: area.y, w: aw, h: area.h }, out, divs, path.concat([false]));
      computeLayout(node.b, { x: area.x + aw + 1, y: area.y, w: bw, h: area.h }, out, divs, path.concat([true]));
    } else {
      const usable = Math.max(0, area.h - 1);
      const ah = clamp(Math.round(usable * ratio), Math.min(1, usable), Math.max(usable - 1, 1));
      const bh = Math.max(0, usable - ah);
      divs.push({ path: path.slice(), area, dir: "down", x: area.x, y: area.y + ah, w: area.w, h: 1 });
      computeLayout(node.a, { x: area.x, y: area.y, w: area.w, h: ah }, out, divs, path.concat([false]));
      computeLayout(node.b, { x: area.x, y: area.y + ah + 1, w: area.w, h: bh }, out, divs, path.concat([true]));
    }
  }
  function ratioFor(d, col, row) {
    const [pos, start, len] = d.dir === "right" ? [col, d.area.x, d.area.w - 1] : [row, d.area.y, d.area.h - 1];
    if (len <= 0) return 0.5;
    return clamp((Math.max(0, pos - start)) / len, 0.1, 0.9);
  }

  function renderPanes() {
    const box = $("#panes");
    box.querySelectorAll(".divider, .panes-empty").forEach((x) => x.remove());
    const live = new Set(snap().panes.map((p) => p.id));
    for (const [id, e] of terms) {
      if (S.snap && !live.has(id)) {
        e.term.dispose();
        e.el.remove();
        terms.delete(id);
      } else e.el.style.display = "none";
    }
    if (!S.connected) {
      clear($("#panebar"));
      box.append(emptyNoConn());
      return;
    }
    const tab = focusedTab();
    if (!tab) {
      clear($("#panebar"));
      box.append(
        h(
          "div",
          { class: "panes-empty" },
          onboardCard(),
          h("button", { class: "btn", type: "button", onclick: () => runAction("new_workspace") }, "+ " + t("act.new_workspace")),
        ),
      );
      return;
    }
    const area = { x: 0, y: 0, w: S.vp.cols, h: S.vp.rows };
    const out = [];
    const divs = [];
    if (tab.zoomed) out.push([tab.focus, area]);
    else computeLayout(tab.layout, area, out, divs, []);
    const cw = S.cell.w,
      ch = S.cell.h;
    for (const [id, r] of out) {
      const p = paneInfo(id);
      const e = ensureTerm(id, p ? p.cols : r.w, p ? p.rows : r.h);
      Object.assign(e.el.style, { display: "block", left: r.x * cw + "px", top: r.y * ch + "px", width: r.w * cw + "px", height: r.h * ch + "px" });
      // 서버가 제목 줄 자리를 비워 줬을 때만(창 높이가 영역보다 작을 때) 제목 줄을 보인다.
      // 터미널 화면(TUI)이 같은 세션에 붙어 있으면 자리를 비우지 않으므로 숨긴다.
      const head = !!p && p.rows < r.h;
      e.el.classList.toggle("has-head", head);
      e.el.style.setProperty("--head-h", head ? ch + "px" : "0px");
      if (p && (e.term.cols !== p.cols || e.term.rows !== p.rows)) e.term.resize(Math.max(2, p.cols), Math.max(1, p.rows));
      e.el.classList.toggle("focused", id === tab.focus);
      renderBadge(e.badge, p, id);
    }
    for (const d of divs) {
      const el = h("div", { class: "divider " + d.dir, title: t("task.resize.h") });
      Object.assign(el.style, { left: d.x * cw + "px", top: d.y * ch + "px", width: d.w * cw + "px", height: d.h * ch + "px" });
      el.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        S.drag = { tab: tab.id, d };
        document.body.classList.add(d.dir === "right" ? "dragging-col" : "dragging-row");
      });
      box.append(el);
    }
    renderPanebar(tab.focus);
    // 서버가 정한 초점 pane으로 키보드 입력을 옮긴다(바뀌었을 때만).
    if (tab.focus !== S.lastFocusApplied && S.view === "terminal" && S.modals === 0) {
      S.lastFocusApplied = tab.focus;
      const e = terms.get(tab.focus);
      if (e) setTimeout(() => e.term.focus(), 0);
    }
  }
  let dragSent = 0;
  document.addEventListener("mousemove", (ev) => {
    if (!S.drag) return;
    const r = $("#panes").getBoundingClientRect();
    const col = Math.floor((ev.clientX - r.left) / S.cell.w);
    const row = Math.floor((ev.clientY - r.top) / S.cell.h);
    const ratio = ratioFor(S.drag.d, col, row);
    const now = Date.now();
    if (now - dragSent > 60) {
      dragSent = now;
      rpc("layout.set_ratio", { tab: S.drag.tab, path: S.drag.d.path, ratio }).catch(() => {});
    }
    S.drag.last = ratio;
  });
  document.addEventListener("mouseup", () => {
    if (!S.drag) return;
    if (S.drag.last != null) rpc("layout.set_ratio", { tab: S.drag.tab, path: S.drag.d.path, ratio: S.drag.last }).catch(() => {});
    S.drag = null;
    document.body.classList.remove("dragging-col", "dragging-row");
  });

  // 지금 선택한 창의 정보와 버튼(터미널 글자를 가리지 않도록 pane 위 줄에 둔다).
  function renderPanebar(id) {
    const bar = clear($("#panebar"));
    const p = paneInfo(id);
    if (!p) return;
    // 이름·상태·나누기 버튼은 창 제목 줄에 있으므로, 여기에는 선택한 창에 대한 동작만 둔다.
    if (p.agent) bar.append(h("button", { class: "mini primary", type: "button", onclick: () => dlgPrompt(p.agent.name || p.id) }, "✉ " + t("cc.prompt") + " · " + agentLabel(p)));
    else bar.append(h("button", { class: "mini primary", type: "button", title: t("task.agent.h"), onclick: () => dlgStartAgent(id) }, "▶ " + t("act.start_agent")));
    bar.append(
      h(
        "button",
        { class: "chip chip-btn" + ((p.sandbox || "none") === "none" ? "" : " ok"), type: "button", title: t("prot.click"), onclick: () => dlgProtection(p.sandbox || "none") },
        profName(p.sandbox || "none"),
        " ",
        icon(ICON.info, 12),
      ),
    );
  }

  const KIND_CLASS = { claude: "k-claude", codex: "k-codex" };
  function renderBadge(badge, p, id) {
    clear(badge);
    if (!p) return;
    const stop = (fn) => (ev) => {
      ev.stopPropagation();
      fn();
    };
    if (p.agent) {
      const st = p.agent.state;
      const kind = (S.kinds.find((k) => k.kind === p.agent.kind) || {}).display || p.agent.kind;
      badge.append(
        h("span", { class: "kdot " + (KIND_CLASS[p.agent.kind] || "k-other"), "aria-hidden": "true" }),
        h("b", { class: "ph-name", text: agentLabel(p) }),
        h("span", { class: "muted ph-kind", text: kind }),
        h("span", { class: "ph-st st-" + st, text: t("state." + st) }),
      );
    } else {
      badge.append(h("span", { class: "muted ph-name", text: baseName(p.title || p.command || t("cc.shell")).slice(0, 40) }));
      badge.append(h("button", { class: "mini primary", type: "button", title: t("task.agent.h"), onclick: stop(() => dlgStartAgent(id)) }, "▶ " + t("act.start_agent")));
    }
    badge.append(h("span", { class: "grow" }));
    const mini = (sym, title, fn) => h("button", { class: "mini", type: "button", title, "aria-label": title, onclick: stop(fn) }, sym);
    badge.append(
      mini(icon(ICON.splitRight, 13), t("act.split_right.short"), () => split("right", id)),
      mini(icon(ICON.splitDown, 13), t("act.split_down.short"), () => split("down", id)),
      mini("⤢", t("act.zoom"), () => zoom(id)),
      mini("✕", t("act.close_pane"), () => closePane(id)),
    );
  }

  function emptyNoConn() {
    if (S.connecting) return h("div", { class: "panes-empty" }, h("h2", { text: t("conn.connecting") }));
    const denied = /denied by policy/.test(S.connError || "");
    return h(
      "div",
      { class: "panes-empty" },
      h("h2", { text: denied ? t("empty.denied.title") : t("empty.no_conn.title") }),
      h("p", { text: denied ? t("empty.denied.body") : t("empty.no_conn.body") }),
      S.connError ? h("pre", { class: "err", text: S.connError }) : null,
      h(
        "div",
        { class: "row" },
        h("button", { class: "btn primary", type: "button", onclick: () => connect(true) }, t("empty.start_server")),
        h("button", { class: "btn", type: "button", onclick: () => dlgSession() }, t("act.connect_session")),
      ),
    );
  }

  function paneMenu(ev, id) {
    ev.preventDefault();
    closeMenus();
    const e = terms.get(id);
    const hasSel = e && e.term.hasSelection();
    const m = h("div", { class: "ctxmenu" });
    const item = (label, fn, dis) =>
      h(
        "button",
        {
          class: "item",
          type: "button",
          disabled: dis,
          onclick: () => {
            closeMenus();
            fn();
          },
        },
        label,
      );
    m.append(
      item(t("act.copy"), () => copySel(id), !hasSel),
      item(t("act.paste"), () => paste(id)),
      h("div", { class: "sep" }),
      item(t("act.split_right"), () => split("right", id)),
      item(t("act.split_down"), () => split("down", id)),
      item(t("act.zoom"), () => zoom(id)),
      h("div", { class: "sep" }),
      item(t("act.start_agent"), () => dlgStartAgent(id)),
      item(t("act.explain_agent"), () => explainAgent(id), !(paneInfo(id) || {}).agent),
      h("div", { class: "sep" }),
      item(t("act.close_pane"), () => closePane(id)),
    );
    m.style.left = ev.clientX + "px";
    m.style.top = ev.clientY + "px";
    document.body.append(m);
    const r = m.getBoundingClientRect();
    if (r.bottom > window.innerHeight) m.style.top = Math.max(0, ev.clientY - r.height) + "px";
    if (r.right > window.innerWidth) m.style.left = Math.max(0, ev.clientX - r.width) + "px";
  }

  async function copySel(id) {
    const e = terms.get(id || focusedPane());
    if (!e || !e.term.hasSelection()) return;
    await invoke("clipboard_write", { text: e.term.getSelection() }).catch((x) => showError(errText(x)));
    e.term.clearSelection();
  }
  async function paste(id) {
    const e = terms.get(id || focusedPane());
    if (!e) return;
    try {
      const text = await invoke("clipboard_read");
      if (text) e.term.paste(text);
    } catch (x) {
      showError(errText(x));
    }
  }

  // ───────────── 동작 구현 ─────────────
  async function split(dir, target) {
    await tryRpc("pane.split", { target: target || focusedPane(), direction: dir, focus: true });
  }
  async function zoom(id) {
    await tryRpc("pane.zoom", { pane: id || focusedPane() });
  }
  async function closePane(id) {
    const pid = id || focusedPane();
    if (!pid) return;
    if (await confirmDlg(t("act.close_pane"), t("confirm.close_pane"), t("act.close_pane"), true)) await tryRpc("pane.close", { pane: pid });
  }
  async function closeTab() {
    const tb = focusedTab();
    if (tb) await tryRpc("tab.close", { id: tb.id });
  }
  async function closeWorkspace(id) {
    const ws = id ? snap().workspaces.find((w) => w.id === id) : focusedWs();
    if (!ws) return;
    if (await confirmDlg(t("act.close_workspace"), t("confirm.close_workspace", { n: ws.label }), t("act.close_workspace"), true))
      await tryRpc("workspace.close", { id: ws.id });
  }
  async function renameWorkspace(id) {
    const ws = id ? snap().workspaces.find((w) => w.id === id) : focusedWs();
    if (!ws) return;
    const name = await promptDlg(t("act.rename_workspace"), t("rename.prompt"), ws.label);
    if (name && name.trim()) await tryRpc("workspace.rename", { id: ws.id, label: name.trim() });
  }
  async function cycleWorkspace(d) {
    const list = snap().workspaces;
    if (!list.length) return;
    const i = list.findIndex((w) => w.id === snap().focus_workspace);
    const n = list[(i + d + list.length) % list.length];
    await tryRpc("workspace.focus", { id: n.id });
  }
  async function stopServer() {
    if (await confirmDlg(t("act.stop_server"), t("confirm.stop_server"), t("act.stop_server"), true)) {
      S.everConnected = false; // 자동 재연결로 다시 띄우지 않는다
      await tryRpc("server.stop");
    }
  }
  async function verifyLedger() {
    const r = await tryRpc("audit.status", { verify: true });
    if (!r) return;
    if (r.ok) showToast(t("log.verified_ok", { n: r.records }), "ok");
    else showError(t("log.verified_bad", { d: r.detail || "?" }));
    S.control = await rpc("control.snapshot").catch(() => S.control);
    renderAlerts();
    if (S.view === "log") renderLog();
  }
  async function explainAgent(id) {
    const pid = id || focusedPane();
    const r = await tryRpc("agent.explain", { target: pid });
    if (r == null) return;
    await modal({ title: t("act.explain_agent"), body: h("pre", { class: "mono scroll", text: JSON.stringify(r, null, 2) }), buttons: [{ label: t("dlg.close"), value: true, primary: true }] });
  }
  function setView(v) {
    S.view = v;
    document.querySelectorAll(".view").forEach((x) => x.classList.toggle("hidden", x.id !== "view-" + v));
    renderViewTabs();
    if (v === "terminal") {
      S.lastFocusApplied = "";
      renderPanes();
    }
    if (v === "control" || v === "log") pollNow();
    if (v === "approvals") renderApprovals();
    if (v === "control") renderControl();
    if (v === "log") renderLog();
  }
  async function setFont(d) {
    S.prefs.fontSize = clamp(fontSize() + d, 9, 28);
    savePrefs();
    await applyFont();
  }
  async function applyFont() {
    for (const e of terms.values()) e.term.options.fontSize = fontSize();
    await measureCell();
    sendViewport(true);
    renderPanes();
  }
  function setTheme(th) {
    S.prefs.theme = th;
    savePrefs();
    document.documentElement.dataset.theme = th;
    for (const e of terms.values()) e.term.options.theme = termTheme();
  }
  function setLang(l) {
    S.lang = l;
    S.prefs.lang = l;
    savePrefs();
    document.documentElement.lang = l;
    renderAll();
  }

  // ───────────── 사이드바·탭·상태 표시 ─────────────
  const stGroup = (st) => (st === "working" ? "working" : st === "blocked" ? "blocked" : "idle");
  function wsDots(ps) {
    const ags = ps.filter((p) => p.agent).sort((a, b) => statePri[a.agent.state] - statePri[b.agent.state]);
    const dots = h("span", { class: "dots", "aria-hidden": "true" });
    for (const a of ags.slice(0, 8)) dots.append(h("i", { class: "d-" + stGroup(a.agent.state) }));
    return [dots, ags];
  }
  function toggleSidebar() {
    S.prefs.sidebarCollapsed = !S.prefs.sidebarCollapsed;
    savePrefs();
    renderSidebar();
    renderUsage();
  }
  function renderSidebar() {
    const collapsed = !!S.prefs.sidebarCollapsed;
    $("#main").classList.toggle("sb-collapsed", collapsed);
    const rail = clear($("#rail"));
    const sb = clear($("#wslist"));
    if (collapsed) {
      rail.append(h("button", { class: "mini rail-btn", type: "button", title: t("sb.expand"), "aria-label": t("sb.expand"), onclick: toggleSidebar }, "»"));
      for (const ws of snap().workspaces) {
        const [, ags] = wsDots(wsPanes(ws));
        const worst = ags[0] ? stGroup(ags[0].agent.state) : null;
        const init = (ws.label || "?").replace(/[^\p{L}\p{N}]+/gu, " ").trim().split(" ").map((w) => w[0]).join("").slice(0, 2).toUpperCase() || "?";
        rail.append(
          h(
            "button",
            {
              class: "rail-ws" + (ws.id === snap().focus_workspace ? " active" : ""),
              type: "button",
              title: ws.label + (ags.length ? " · " + ags.map((a) => agentLabel(a) + " " + t("state." + a.agent.state)).join(", ") : ""),
              onclick: () => {
                setView("terminal");
                if (ws.id !== snap().focus_workspace) tryRpc("workspace.focus", { id: ws.id });
              },
            },
            init,
            worst ? h("i", { class: "d-" + worst }) : null,
          ),
        );
      }
      return;
    }
    sb.append(h("div", { class: "sb-title sb-title-row" }, h("span", { text: t("sb.workspaces") }), h("button", { class: "mini", type: "button", title: t("sb.collapse"), "aria-label": t("sb.collapse"), onclick: toggleSidebar }, "«")));
    const list = h("div", { class: "sb-list" });
    for (const ws of snap().workspaces) {
      const ps = wsPanes(ws);
      const [dots, ags] = wsDots(ps);
      let summary;
      if (!ags.length) summary = h("span", { class: "muted", text: t("sb.no_agent") + " · " + t("sb.panes", { n: ps.length }) });
      else {
        const b = ags.filter((a) => a.agent.state === "blocked");
        summary = b.length
          ? h("span", { class: "warn-text", text: b.map((a) => agentLabel(a)).slice(0, 2).join(", ") + " · " + t("state.blocked") })
          : h("span", { class: "muted", text: t("sb.panes", { n: ps.length }) + " · " + t("state." + ags[0].agent.state) });
      }
      const item = h(
        "div",
        {
          class: "sb-item" + (ws.id === snap().focus_workspace ? " active" : ""),
          tabindex: 0,
          role: "button",
          onkeydown: (ev) => {
            if (ev.key === "Enter" || ev.key === " ") {
              ev.preventDefault();
              ev.currentTarget.click();
            }
          },
          title: ws.cwd + (ags.length ? "\n" + ags.map((a) => agentLabel(a) + " · " + t("state." + a.agent.state)).join("\n") : ""),
          onclick: () => {
            setView("terminal");
            if (ws.id !== snap().focus_workspace) tryRpc("workspace.focus", { id: ws.id });
          },
          oncontextmenu: (ev) => {
            ev.preventDefault();
            closeMenus();
            const m = h(
              "div",
              { class: "ctxmenu" },
              h("button", { class: "item", type: "button", onclick: () => (closeMenus(), renameWorkspace(ws.id)) }, t("act.rename_workspace")),
              h("button", { class: "item", type: "button", onclick: () => (closeMenus(), closeWorkspace(ws.id)) }, t("act.close_workspace")),
            );
            m.style.left = ev.clientX + "px";
            m.style.top = ev.clientY + "px";
            document.body.append(m);
          },
        },
        h("div", { class: "sb-name", text: ws.label }),
        ws.worktree ? h("div", { class: "sb-agent muted", text: "⎇ " + ws.worktree.branch }) : null,
        h("div", { class: "sb-agent" }, ags.length ? dots : null, summary),
      );
      list.append(item);
    }
    sb.append(list);
    sb.append(h("button", { class: "btn sb-new", type: "button", disabled: !S.connected, onclick: () => runAction("new_workspace") }, "+ " + t("act.new_workspace")));
  }

  function renderViewTabs() {
    const vt = clear($("#viewtabs"));
    const n = S.approvals.length;
    for (const v of ["terminal", "control", "approvals", "log"]) {
      vt.append(
        h(
          "button",
          { class: "vtab" + (S.view === v ? " active" : ""), type: "button", onclick: () => setView(v) },
          t("view." + v),
          v === "approvals" && n ? h("span", { class: "count", text: String(n) }) : null,
        ),
      );
    }
    // 터미널 화면이면 이 작업 공간의 탭들
    const ws = focusedWs();
    const tabs = clear($("#wstabs"));
    if (ws && S.connected) {
      for (const tb of ws.tabs) {
        tabs.append(
          h(
            "div",
            {
              class: "wstab" + (tb.id === ws.focus_tab ? " active" : ""),
              title: tb.label,
              onclick: () => tryRpc("tab.focus", { id: tb.id }),
              ondblclick: async () => {
                const nm = await promptDlg(t("rename.prompt"), t("rename.prompt"), tb.label);
                if (nm && nm.trim()) tryRpc("tab.rename", { id: tb.id, label: nm.trim() });
              },
            },
            h("span", { text: tb.label }),
            ws.tabs.length > 1
              ? h(
                  "button",
                  {
                    class: "x",
                    type: "button",
                    title: t("act.close_tab"),
                    onclick: (ev) => {
                      ev.stopPropagation();
                      tryRpc("tab.close", { id: tb.id });
                    },
                  },
                  "×",
                )
              : null,
          ),
        );
      }
      tabs.append(h("button", { class: "wstab add", type: "button", title: t("act.new_tab") + " · " + keyLabel("Ctrl+Shift+T"), onclick: () => runAction("new_tab") }, "+"));
    }
  }

  // 상태 줄 가운데: 작업 중 · 기다림 · 쉬는 중 개수(누르면 그 상태인 첫 창으로).
  function renderStatusCounts() {
    const bar = clear($("#usagebar"));
    if (!S.connected) return;
    const groups = { working: [], blocked: [], idle: [] };
    for (const p of agents()) groups[stGroup(p.agent.state)].push(p);
    for (const g of ["working", "blocked", "idle"]) {
      const n = groups[g].length;
      bar.append(
        h(
          "button",
          { class: "sbchip" + (g === "blocked" && n ? " warn" : ""), type: "button", title: t("sbar.go"), disabled: !n, onclick: () => n && goPane(groups[g][0].id) },
          h("span", { class: "sdot d-" + g, "aria-hidden": "true" }),
          t("sbar." + g, { n }),
        ),
      );
    }
  }
  function renderStatus() {
    $("#credit").textContent = t("credit");
    const c = clear($("#conn"));
    if (S.connecting) c.append(h("span", { text: t("conn.connecting") }));
    else if (S.connected) {
      c.append(h("span", { class: "dot ok" }), " " + t("conn.connected", { s: S.session }) + " · " + t("profile", { p: profName(defaultProfile()) }));
    } else c.append(h("span", { class: "dot bad" }), " " + t("conn.disconnected"));
    if (S.info && S.info.version) c.append(" · v" + S.info.version);
  }

  // 남은 사용량이 15% 아래로 떨어진 한도들(닫은 알림은 같은 리셋 주기 동안 다시 띄우지 않는다).
  function lowUsage() {
    const u = S.usage || {};
    const out = [];
    for (const [name, x] of [["Claude Code", u.claude], ["Codex", u.codex]]) {
      if (!x) continue;
      for (const [win, w] of [[t("us.five_short"), x.primary], [t("us.week_short"), x.secondary]]) {
        const v = usageWin(w);
        if (!v || v.done || v.left >= 15) continue;
        const key = name + "|" + win + "|" + (w.resets_at || 0);
        if ((S.prefs.lowDismissed || []).includes(key)) continue;
        out.push({ name, win, p: v.left, key });
      }
    }
    return out;
  }

  async function decideApproval(ev, a, allow) {
    if (!ev.isTrusted) {
      showError(t("ap.untrusted"));
      return;
    }
    if (ev.currentTarget && ev.currentTarget.disabled !== undefined) ev.currentTarget.disabled = true;
    const r = await tryRpc("approval.decide", { id: a.id, allow });
    if (r) showToast(t("ap.decided"), "ok");
    pollNow();
  }
  const oneLine = (x, n) => {
    const s1 = String(x || "").replace(/\s+/g, " ").trim();
    return s1.length > n ? s1.slice(0, n - 1) + "…" : s1;
  };

  function renderAlerts() {
    const box = $("#alerts");
    const bands = [];
    if (!S.connected && S.everConnected && !S.connecting) bands.push({ level: "bad", text: t("alert.disconnected"), btns: [[t("act.reconnect"), () => connect(true)]] });
    const integ = S.control && S.control.integrity;
    if (S.connected && integ && integ.ok === false && integ.level !== "off") bands.push({ level: "bad", text: t("alert.integrity", { detail: integ.detail || "?" }), btns: [[t("alert.view"), () => setView("log")]] });
    if (S.approvals.length) {
      const a = S.approvals[0];
      const p = paneInfo(a.caller);
      const who = p ? agentLabel(p) : String(a.caller || "");
      const detail = a.detail ? ": " + oneLine(a.detail, 90) : "";
      bands.push({
        level: "warn",
        approval: a,
        text: t("alert.approval_one", { who, act: actionName(a.action).replace(/ \(.*\)$/, ""), detail }),
        title: describePane(a.caller) + " → " + actionName(a.action) + " → " + describePane(a.target) + (a.detail ? "\n" + a.detail : ""),
      });
    }
    const blocked = agents().filter((p) => p.agent.state === "blocked");
    if (blocked.length) {
      const p = blocked[0];
      bands.push({ level: "warn", text: t("alert.blocked", { name: agentLabel(p) }), btns: [[t("alert.go"), () => goPane(p.id)]] });
    }
    for (const lu of lowUsage()) {
      bands.push({
        level: "bad",
        text: t("alert.low_usage", lu),
        btns: [
          [
            t("alert.dismiss"),
            () => {
              S.prefs.lowDismissed = (S.prefs.lowDismissed || []).concat([lu.key]).slice(-20);
              savePrefs();
              renderAlerts();
            },
          ],
        ],
      });
    }
    // 알림 띠와 '지금 할 일' 줄은 같은 자리에 한 줄만 보인다. 줄 수가 바뀌면 터미널 크기가 바뀌고,
    // Windows ConPTY는 크기가 바뀔 때 화면을 지워 버리기 때문이다. 알림이 여러 개면 가장 급한 것만.
    const b = bands[0];
    S.alertIsApproval = !!(b && b.approval);
    // 내용이 같으면 다시 만들지 않는다(1.5초마다 버튼이 바뀌면 누르는 도중 클릭이 사라진다).
    const sig = b ? [b.level, b.text, bands.length, b.approval ? b.approval.id : "", S.lang].join("|") : "";
    if (sig === S.alertSig && (box.firstChild || !b)) {
      S.hasAlert = !!b;
      renderTip();
      return;
    }
    S.alertSig = sig;
    clear(box);
    if (b) {
      const more = bands.length > 1 ? " " + t("alert.more", { n: bands.length - 1 }) : "";
      const band = h("div", { class: "band " + b.level }, h("span", { class: "band-t", text: b.text + more, title: (b.title || b.text) + more }));
      if (b.approval) {
        const a = b.approval;
        const left = Math.max(0, Math.round((a.expires_ms - Date.now()) / 1000));
        band.append(
          h("b", { class: "band-cd", "data-exp": a.expires_ms, text: "⏱ " + t("alert.approval_left", { s: left }) }),
          h("button", { class: "btn small primary", type: "button", title: t("alert.allow_key"), onclick: (ev) => decideApproval(ev, a, true) }, t("ap.allow"), h("kbd", { text: "Alt+Y" })),
          h("button", { class: "btn small", type: "button", title: t("alert.deny_key"), onclick: (ev) => decideApproval(ev, a, false) }, t("ap.deny"), h("kbd", { text: "Alt+N" })),
          h("button", { class: "btn small ghost", type: "button", onclick: () => setView("approvals") }, t("alert.details")),
        );
      } else {
        for (const [label, fn] of b.btns || []) band.append(h("button", { class: "btn small", type: "button", onclick: fn }, label));
      }
      box.append(band);
    }
    S.hasAlert = !!b;
    renderTip();
  }
  // 승인 대기가 있으면 남은 시간을 1초마다 다시 그린다.
  // 버튼을 다시 만들지 않고 글자만 바꾼다(누르는 도중에 버튼이 바뀌면 클릭이 사라진다).
  setInterval(() => {
    for (const el of document.querySelectorAll("[data-exp]")) {
      const left = Math.max(0, Math.round((Number(el.dataset.exp) - Date.now()) / 1000));
      el.textContent = el.classList.contains("band-cd") ? "⏱ " + t("alert.approval_left", { s: left }) : t("ap.expires", { s: left });
    }
  }, 1000);

  // 앱이 뒤에 있을 때 새 승인 요청이 오면 운영체제 알림(승인 버튼 없음).
  function notifyApprovals() {
    for (const a of S.approvals) {
      if (S.notified.has(a.id)) continue;
      S.notified.add(a.id);
      const p = paneInfo(a.caller);
      const left = Math.max(0, Math.round((a.expires_ms - Date.now()) / 1000));
      invoke("notify", {
        title: t("nt.title", { name: p ? agentLabel(p) : String(a.caller || "") }),
        body: t("nt.body", { act: actionName(a.action).replace(/ \(.*\)$/, "") + (a.detail ? ": " + oneLine(a.detail, 120) : ""), s: left }),
      }).catch(() => {});
    }
  }
  async function goPane(id) {
    const p = paneInfo(id);
    if (!p) return;
    setView("terminal");
    if (p.workspace !== snap().focus_workspace) await tryRpc("workspace.focus", { id: p.workspace });
    const ws = snap().workspaces.find((w) => w.id === p.workspace);
    if (ws && ws.focus_tab !== p.tab) await tryRpc("tab.focus", { id: p.tab });
    await tryRpc("pane.focus", { pane: id });
  }

  // ───────────── 안내 패널 ─────────────
  const TASKS = [
    ["workspace", "new_workspace"],
    ["agent", "start_agent"],
    ["split", "split_right"],
    ["resize", null],
    ["prompt", "prompt_agent"],
    ["approve", "approvals"],
    ["control", "control"],
    ["policy", "settings_policy"],
    ["verify", "verify"],
    ["copy", null],
    ["usage", "usage_connect"],
  ];
  function currentTip() {
    if (!S.connected) return null;
    if (!snap().workspaces.length) return { text: t("tip.no_workspace"), act: "new_workspace" };
    if (S.approvals.length) return { text: t("tip.approvals", { n: S.approvals.length }), act: "approvals", warn: true };
    const blocked = agents().find((p) => p.agent.state === "blocked");
    if (blocked) return { text: t("tip.blocked", { name: agentLabel(blocked) }), fn: () => goPane(blocked.id), label: t("alert.go"), warn: true };
    const ws = focusedWs();
    if (ws && !wsPanes(ws).some((p) => p.agent)) return { text: t("tip.no_agent"), act: "start_agent" };
    if (defaultProfile() === "none") return { text: t("tip.weak_profile"), fn: () => dlgSettings("sandbox"), label: t("act.settings") };
    return { text: t("tip.all_good") };
  }
  // 위쪽 한 줄 안내("지금 할 일"). 같은 안내는 숨기면 다시 보이지 않는다(상황이 바뀌면 새 안내).
  function renderTip() {
    const box = clear($("#tipbar"));
    if (S.hasAlert) return;
    const tip = currentTip();
    if (!tip) return;
    const key = tip.text;
    if ((S.prefs.hiddenTips || []).includes(key) && !tip.warn) return;
    const act = tip.act ? actionById(tip.act) : null;
    box.append(
      h(
        "div",
        { class: "tip" + (tip.warn ? " warn" : "") },
        h("span", { class: "tip-k", text: "💡 " + t("guide.now") }),
        h("span", { class: "tip-t", text: tip.text }),
        act ? h("button", { class: "btn small primary", type: "button", onclick: () => runAction(act.id) }, actLabel(act)) : null,
        tip.fn ? h("button", { class: "btn small primary", type: "button", onclick: tip.fn }, tip.label) : null,
        h("button", { class: "linkbtn", type: "button", onclick: () => dlgGuide("tasks") }, t("guide.tab.tasks") + " →"),
        tip.warn
          ? null
          : h(
              "button",
              {
                class: "mini",
                type: "button",
                title: t("tip.dismiss"),
                onclick: () => {
                  S.prefs.hiddenTips = (S.prefs.hiddenTips || []).concat([key]).slice(-20);
                  savePrefs();
                  renderTip();
                },
              },
              "✕",
            ),
      ),
    );
  }

  // 가이드 창: 시작하기 · 이걸 하려면 · 단축키 · 용어 설명 · 정보·제거
  function dlgGuide(start) {
    const tabs = ["start", "tasks", "keys", "words", "about"];
    const tabsEl = h("div", { class: "set-tabs" });
    const content = h("div", { class: "set-content guide-content" });
    let done = () => {};
    const go = (fn) => {
      done(null);
      setTimeout(fn, 0);
    };
    const pages = {
      start: () =>
        h(
          "div",
          null,
          ["1", "2", "3", "4", "5"].map((i) => h("p", { class: "g-step", text: t("guide.start." + i) })),
          h("div", { class: "row" }, h("button", { class: "btn primary", type: "button", onclick: () => go(() => runAction("new_workspace")) }, "+ " + t("act.new_workspace")), h("button", { class: "btn", type: "button", onclick: () => go(() => wizard()) }, t("act.wizard"))),
        ),
      tasks: () =>
        h(
          "div",
          { class: "g-list" },
          TASKS.map(([k, act]) =>
            h(
              "div",
              { class: "g-item" },
              h("div", null, h("div", { class: "g-t", text: t("task." + k + ".t") }), h("div", { class: "g-h", text: t("task." + k + ".h") })),
              act ? h("button", { class: "btn small", type: "button", onclick: () => go(() => runAction(act)) }, t("guide.run")) : null,
            ),
          ),
        ),
      keys: () => {
        const tb = h("tbody");
        for (const a of ACTIONS.filter((x) => x.keys)) tb.append(h("tr", null, h("td", { text: actLabel(a) }), h("td", null, h("kbd", { text: keyLabel(a.keys) }))));
        if (IS_WIN) tb.append(h("tr", null, h("td", { text: t("act.paste") }), h("td", null, h("kbd", { text: "Ctrl+V" }))));
        return h("table", { class: "keys" }, tb);
      },
      words: () => h("dl", { class: "glossary" }, ["workspace", "pane", "agent", "approval", "protection", "usage", "log", "mask", "session"].map((k) => { const [term, ...rest] = t("guide.words." + k).split(": "); return [h("dt", { text: term }), h("dd", { text: rest.join(": ") })]; })),
      about: () => h("div", null, h("p", { class: "notice", text: t("guide.notice") }), h("h4", { text: t("act.uninstall_help") }), h("p", { text: t("guide.uninstall") })),
    };
    const show = (k) => {
      tabsEl.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b.dataset.k === k));
      clear(content).append(pages[k]());
    };
    for (const k of tabs) tabsEl.append(h("button", { type: "button", "data-k": k, onclick: () => show(k) }, t("guide.tab." + k)));
    show(tabs.includes(start) ? start : "start");
    return modal({ title: t("guide.title"), wide: true, body: h("div", { class: "settings guide" }, tabsEl, content), buttons: [{ label: t("dlg.close"), value: true, primary: true }], onReady: (d) => (done = d) });
  }

  // ───────────── AI 사용량(Claude·Codex 남은 양과 리셋까지 남은 시간) ─────────────
  let usageAt = 0;
  async function pollUsage(force) {
    if (!force && Date.now() - usageAt < 20000) return;
    usageAt = Date.now();
    try {
      S.usage = await invoke("usage_read");
    } catch (_) {
      /* 사용량은 보조 정보 */
    }
    renderUsage();
  }
  setInterval(() => pollUsage(false), 5000);
  setInterval(() => renderUsage(), 30000);

  function fmtDur(sec) {
    if (sec < 60) return null;
    const d = Math.floor(sec / 86400),
      hh = Math.floor((sec % 86400) / 3600),
      m = Math.floor((sec % 3600) / 60);
    const parts = [];
    if (d) parts.push(t("us.d", { n: d }));
    if (hh) parts.push(t("us.h", { n: hh }));
    if (!d && m) parts.push(t("us.m", { n: m }));
    return parts.join(" ");
  }
  // 창 하나를 사람이 읽는 값으로: 남은 %, 리셋 문구, 막대 색
  function usageWin(w) {
    if (!w) return null;
    const now = Date.now() / 1000;
    const reset = w.resets_at ? w.resets_at - now : null;
    if (reset != null && reset <= 0) return { left: 100, text: t("us.reset_done"), level: "ok", done: true };
    const left = Math.max(0, Math.round(100 - w.used_percent));
    const dur = reset != null ? fmtDur(reset) : null;
    const text = reset == null ? t("us.reset_unknown") : dur ? t("us.reset_in", { t: dur }) : t("us.reset_soon");
    return { left, text, level: left < 20 ? "bad" : left < 50 ? "warn" : "ok" };
  }
  function usageAgo(ms) {
    const s = (Date.now() - ms) / 1000;
    const d = fmtDur(s);
    return d ? t("us.updated", { t: d }) : t("us.updated_now");
  }
  function shortReset(w) {
    const d = fmtDur(w.resets_at - Date.now() / 1000);
    return d ? t("us.reset_short", { t: d }) : t("us.reset_soon");
  }
  function usageBlock(name, u, emptyText, action) {
    const resetW = u && (usageWin(u.primary) || usageWin(u.secondary));
    const box = h(
      "div",
      { class: "usc", title: u && u.updated_ms ? usageAgo(u.updated_ms) : "" },
      h("div", { class: "usc-h" }, h("b", { text: name }), resetW && !resetW.done && u.primary && u.primary.resets_at ? h("span", { class: "muted tiny", text: shortReset(u.primary) }) : null),
    );
    if (!u || (!u.primary && !u.secondary)) {
      box.append(h("div", { class: "muted small", text: emptyText }));
      if (action) box.append(action);
      return box;
    }
    const lows = [];
    for (const [label, w] of [[t("us.five_short"), u.primary], [t("us.week_short"), u.secondary]]) {
      const v = usageWin(w);
      if (!v) continue;
      const lvl = v.left < 15 ? "bad" : v.level;
      if (v.left < 15 && !v.done) lows.push(label);
      box.append(
        h(
          "div",
          { class: "usc-r", title: label + " · " + t("us.left", { p: v.left }) + " · " + v.text },
          h("span", { class: "muted", text: label }),
          h("div", { class: "bar" }, h("i", { class: "lvl-" + lvl, style: { width: (v.done ? 100 : v.left) + "%" } })),
          h("b", { class: "lvl-" + lvl, text: v.left + "%" }),
        ),
      );
    }
    for (const l of lows) box.append(h("div", { class: "usc-low", text: t("us.low", { win: l }) }));
    return box;
  }
  function renderUsage() {
    const box = $("#usage");
    if (!box) return;
    clear(box);
    if (S.prefs.sidebarCollapsed) return;
    const u = S.usage || {};
    box.append(h("div", { class: "us-head" }, h("span", { class: "sb-title", text: t("us.title") }), h("button", { class: "mini", type: "button", title: t("act.usage_refresh"), "aria-label": t("act.usage_refresh"), onclick: () => pollUsage(true) }, "↻")));
    const connectBtn = h("button", { class: "btn small", type: "button", onclick: () => dlgUsageConnect() }, t("us.connect"));
    box.append(usageBlock("Claude Code", u.claude, u.claude_connected ? t("us.claude_wait") : t("us.claude_none"), u.claude_connected ? null : connectBtn));
    box.append(usageBlock("Codex", u.codex, t("us.codex_none"), null));
    box.append(
      h(
        "button",
        { id: "tokbtn", class: "tokbtn" + (S.tokOpen ? " on" : ""), type: "button", "aria-expanded": S.tokOpen ? "true" : "false", "aria-controls": "tokpanel", onclick: (ev) => (ev.stopPropagation(), toggleTokens()) },
        icon(ICON.chart),
        h("span", { text: S.tokOpen ? t("tok.close") : t("tok.open") }),
        h("span", { class: "ch", text: S.tokOpen ? "‹" : "›" }),
      ),
    );
    renderAlerts();
  }

  // ───────────── 현재 토큰 사용량 패널 ─────────────
  // 펼쳤을 때만 5초마다 읽고 그린다. 닫으면 타이머를 멈춰 아무 일도 하지 않는다.
  const TOK_COLORS = { dark: { claude: "#338ced", codex: "#e76624" }, light: { claude: "#257de0", codex: "#c8541b" } };
  const tokColor = (k) => TOK_COLORS[S.prefs.theme === "light" ? "light" : "dark"][k];
  let tokTimer = 0;
  function toggleTokens(force) {
    const open = force === undefined ? !S.tokOpen : !!force;
    S.tokOpen = open;
    clearInterval(tokTimer);
    tokTimer = 0;
    if (open) {
      readTokens();
      tokTimer = setInterval(readTokens, 5000);
    }
    renderTokens();
    renderUsage();
    // 키보드로 연 사람도 바로 패널 안에서 움직일 수 있게 닫기 버튼으로 초점을 옮긴다.
    if (open) {
      const c = $("#tokpanel .tp-h button");
      if (c) c.focus();
    }
    if (!open) {
      const e = terms.get(focusedPane());
      if (e && S.view === "terminal") e.term.focus();
    }
  }
  async function readTokens() {
    if (!S.tokOpen) return;
    const panes = agents().map((p) => ({ id: p.id, kind: p.agent.kind, cwd: p.cwd }));
    try {
      S.tokens = await invoke("tokens_read", { session: S.session, panes });
    } catch (_) {
      S.tokens = null;
    }
    renderTokens();
  }
  document.addEventListener("click", (ev) => {
    if (!S.tokOpen || S.modals) return;
    const t0 = ev.target;
    if (t0.closest && (t0.closest("#tokpanel") || t0.closest("#tokbtn"))) return;
    toggleTokens(false);
  });
  const fmtTok = (v) => (v >= 1e6 ? (v / 1e6).toFixed(1) + "M" : v >= 1000 ? (v / 1000).toFixed(1) + "k" : String(Math.round(v)));
  function niceMax(v) {
    if (v <= 0) return 1000;
    const p = Math.pow(10, Math.floor(Math.log10(v)));
    for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
    return 10 * p;
  }

  function renderTokens() {
    const panel = $("#tokpanel");
    if (!S.tokOpen) {
      panel.hidden = true;
      clear(panel);
      return;
    }
    panel.hidden = false;
    const hadFocus = panel.contains(document.activeElement);
    clear(panel);
    queueMicrotask(() => {
      if (hadFocus) {
        const c = panel.querySelector(".tp-h button");
        if (c) c.focus();
      }
    });
    panel.style.top = $("#viewtabs").offsetHeight + "px";
    const data = (S.tokens && S.tokens.panes) || [];
    const now = (S.tokens && S.tokens.now_ms) || Date.now();
    const byPane = new Map(data.map((d) => [d.pane, d]));
    const ags = agents().filter((p) => p.agent.kind === "claude" || p.agent.kind === "codex");
    const win5 = now - 5 * 60 * 1000;
    const rate = (d) => (d ? d.samples.filter((x) => x[0] >= win5).reduce((a, x) => a + x[1], 0) / 5 : 0);
    const kinds = ["claude", "codex"];
    const kindRate = {};
    for (const k of kinds) kindRate[k] = data.filter((d) => d.kind === k).reduce((a, d) => a + rate(d), 0);

    panel.append(
      h("div", { class: "tp-h" }, h("b", { text: t("tok.title") }), h("span", { class: "muted small", text: t("tok.sub") }), h("span", { class: "grow" }), h("button", { class: "mini", type: "button", title: t("dlg.close"), "aria-label": t("dlg.close"), onclick: () => toggleTokens(false) }, "✕")),
    );
    const kpis = h("div", { class: "tp-kpis" });
    for (const k of kinds) {
      const n = ags.filter((p) => p.agent.kind === k).length;
      kpis.append(
        h(
          "div",
          { class: "tp-kpi" },
          h("div", { class: "tp-kl" }, h("span", { class: "sw", style: { background: tokColor(k) } }), k === "claude" ? "Claude Code" : "Codex"),
          h("div", { class: "tp-kv", text: fmtTok(kindRate[k]) }),
          h("div", { class: "tp-ks", text: t("tok.per_min", { n }) }),
        ),
      );
    }
    panel.append(kpis);

    // 15분을 30초 칸 30개로 나눠, 칸마다 분당 토큰(칸 합 × 2)을 그린다.
    const N = 30,
      span = 15 * 60 * 1000,
      start = now - span,
      step = span / N;
    // 응답 단위 값은 들쭉날쭉하므로, 각 점은 그 시점까지 3분 동안의 분당 평균으로 그린다.
    const AVG = 3 * 60 * 1000;
    const series = {};
    for (const k of kinds) {
      const pts = data.filter((x) => x.kind === k).flatMap((d) => d.samples);
      series[k] = Array.from({ length: N }, (_, i) => {
        const end = start + (i + 1) * step;
        return pts.filter((x) => x[0] > end - AVG && x[0] <= end).reduce((a, x) => a + x[1], 0) / 3;
      });
    }
    const max = niceMax(Math.max(1, ...series.claude, ...series.codex));
    const chartBox = h("div", { class: "tp-chart" });
    panel.append(h("div", { class: "tp-sect" }, h("span", { text: t("tok.chart") }), h("span", { text: t("tok.when") })), chartBox);
    drawTokChart(chartBox, series, max, now, step, N);

    const list = h("div", { class: "tp-list" });
    panel.append(h("div", { class: "tp-sect" }, h("span", { text: t("tok.per_pane") }), h("span", { text: t("tok.total") })), list);
    const rows = ags.map((p) => ({ p, d: byPane.get(p.id) }));
    const maxRate = Math.max(1, ...rows.map((r) => rate(r.d)));
    if (!rows.length || !data.length) list.append(h("div", { class: "muted small", text: t("tok.none") }));
    for (const { p, d } of rows) {
      const r = rate(d);
      const waiting = p.agent.state === "blocked";
      list.append(
        h(
          "div",
          { class: "tp-row", title: d && d.guess ? t("tok.guess") : "" },
          h("span", { class: "tp-name" }, h("span", { class: "sw", style: { background: tokColor(p.agent.kind) } }), agentLabel(p), d && d.guess ? h("span", { class: "muted", text: " *" }) : null),
          h("div", { class: "bar" }, h("i", { style: { width: Math.max(r ? 2 : 0, (r / maxRate) * 100) + "%", background: tokColor(p.agent.kind) } })),
          h("span", { class: "num" + (waiting ? " warn-text" : ""), text: waiting && !r ? t("tok.waiting") : d ? fmtTok(r) : "—" }),
          h("span", { class: "num muted", text: d ? fmtTok(d.total) : "—" }),
        ),
      );
    }
    panel.append(h("div", { class: "tp-note muted small", text: t("tok.note") }));
  }

  function drawTokChart(box, series, max, now, step, N) {
    const NS = "http://www.w3.org/2000/svg";
    const W = 420,
      H = 190,
      L = 36,
      R = 8,
      T = 8,
      B = 22;
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svg.setAttribute("role", "img");
    svg.setAttribute("aria-label", t("tok.chart_label"));
    const el = (tag, attrs, text) => {
      const e = document.createElementNS(NS, tag);
      for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
      if (text != null) e.textContent = text;
      svg.append(e);
      return e;
    };
    const X = (i) => L + ((W - L - R) * i) / (N - 1);
    const Y = (v) => T + (H - B - T) * (1 - Math.min(v, max) / max);
    for (const f of [0, 0.5, 1]) {
      el("line", { x1: L, x2: W - R, y1: Y(max * f) + 0.5, y2: Y(max * f) + 0.5, class: f ? "tc-grid" : "tc-base" });
      el("text", { x: L - 6, y: Y(max * f) + 4, "text-anchor": "end", class: "tc-tick" }, fmtTok(max * f));
    }
    [
      [0, t("tok.ago15"), "start"],
      [Math.round((N - 1) / 3), t("tok.ago10"), "middle"],
      [Math.round(((N - 1) * 2) / 3), t("tok.ago5"), "middle"],
      [N - 1, t("tok.now"), "end"],
    ].forEach(([i, txt, a]) => el("text", { x: X(i), y: H - 5, "text-anchor": a, class: "tc-tick" }, txt));
    for (const k of ["codex", "claude"]) {
      const v = series[k];
      el("path", { d: v.map((y, i) => (i ? "L" : "M") + X(i).toFixed(1) + "," + Y(y).toFixed(1)).join(""), fill: "none", stroke: tokColor(k), "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round" });
      el("circle", { cx: X(N - 1), cy: Y(v[N - 1]), r: 4, fill: tokColor(k), class: "tc-dot" });
    }
    const cross = el("line", { x1: 0, x2: 0, y1: T, y2: H - B, class: "tc-cross", visibility: "hidden" });
    const tip = h("div", { class: "tc-tip", hidden: true });
    svg.addEventListener("mousemove", (ev) => {
      const r = svg.getBoundingClientRect();
      const x = ((ev.clientX - r.left) / r.width) * W;
      const i = clamp(Math.round(((x - L) / (W - L - R)) * (N - 1)), 0, N - 1);
      cross.setAttribute("x1", X(i));
      cross.setAttribute("x2", X(i));
      cross.setAttribute("visibility", "visible");
      const ago = Math.round(((N - 1 - i) * step) / 1000);
      tip.hidden = false;
      tip.textContent = t("tok.tip", { t: ago >= 60 ? t("tok.ago_m", { n: Math.round(ago / 60) }) : ago ? t("tok.ago_s", { n: ago }) : t("tok.now"), c: fmtTok(series.claude[i]), x: fmtTok(series.codex[i]) });
      tip.style.left = Math.min(r.width - 170, Math.max(0, (X(i) / W) * r.width + 8)) + "px";
    });
    svg.addEventListener("mouseleave", () => {
      cross.setAttribute("visibility", "hidden");
      tip.hidden = true;
    });
    box.append(svg, tip);
  }

  async function dlgUsageConnect() {
    const hooks = h("input", { type: "checkbox", checked: !(S.usage && S.usage.claude_hooks) });
    const replace = h("input", { type: "checkbox" });
    const err = h("div", { class: "f-err" });
    const connected = S.usage && S.usage.claude_connected;
    const body = h(
      "div",
      null,
      h("p", { text: t("us.conn.body") }),
      S.usage && S.usage.claude_hooks ? null : h("label", { class: "check" }, hooks, t("us.conn.hooks")),
      h("label", { class: "check" }, replace, t("us.conn.replace")),
      err,
    );
    const v = await modal({
      title: t("us.conn.title"),
      wide: true,
      body,
      buttons: [
        connected ? { label: t("us.disconnect"), value: "off", danger: true } : null,
        { label: t("dlg.cancel"), value: null },
        {
          label: t("us.connect"),
          value: "on",
          primary: true,
          check: async () => {
            try {
              await invoke("usage_connect_claude", { replace: replace.checked, hooks: hooks.checked });
              return true;
            } catch (e) {
              err.textContent = errText(e);
              return false;
            }
          },
        },
      ].filter(Boolean),
    });
    if (v === "on") showToast(t("us.conn.ok"), "ok");
    if (v === "off") {
      try {
        await invoke("usage_disconnect_claude");
      } catch (e) {
        showError(errText(e));
      }
    }
    pollUsage(true);
  }

  // ───────────── 관제·승인·기록 화면 ─────────────
  const fmtBytes = (b) => (b > 1 << 30 ? (b / (1 << 30)).toFixed(1) + " GB" : (b / (1 << 20)).toFixed(0) + " MB");
  const fmtTime = (ms) => new Date(ms).toLocaleTimeString();

  function renderControl() {
    const v = clear($("#view-control"));
    const c = S.control;
    const head = h("div", { class: "cc-head" });
    if (c && c.integrity) {
      head.append(h("span", { class: "chip " + (c.integrity.ok ? "ok" : "bad"), text: c.integrity.ok ? t("cc.integrity_ok", { n: c.integrity.records }) : t("cc.integrity_bad") }));
      head.append(h("span", { class: "chip", text: t("cc.blocked_today", { n: c.blocked_today || 0 }) }));
    }
    head.append(h("button", { class: "btn small", type: "button", onclick: () => runAction("verify") }, t("act.verify")));
    v.append(head);
    const ps = snap().panes.slice().sort((a, b) => {
      const pa = a.agent ? statePri[a.agent.state] : 9;
      const pb = b.agent ? statePri[b.agent.state] : 9;
      return pa - pb;
    });
    if (!ps.length) {
      v.append(h("p", { class: "empty", text: t("empty.control") }));
      return;
    }
    const grid = h("div", { class: "cards" });
    for (const p of ps) {
      const st = p.agent ? p.agent.state : null;
      const ws = snap().workspaces.find((w) => w.id === p.workspace);
      grid.append(
        h(
          "div",
          { class: "card" + (st === "blocked" ? " warn" : "") },
          h(
            "div",
            { class: "card-h" },
            st ? h("span", { class: "st st-" + st, text: stateIcon[st] + " " }) : null,
            h("b", { text: agentLabel(p) }),
            h("span", { class: "muted", text: " · " + (st ? t("state." + st) : t("cc.shell")) }),
          ),
          h("div", { class: "muted small", text: (ws ? ws.label : p.workspace) + " · " + p.cwd }),
          h(
            "div",
            { class: "kv" },
            h("span", { text: t("cc.sandbox") + ": " + profName(p.sandbox) + (p.sandbox_strength && p.sandbox_strength !== "none" ? " (" + p.sandbox_strength + ")" : "") }),
            h("span", { text: t("cc.cpu") + " " + (p.cpu || 0).toFixed(0) + "%" }),
            h("span", { text: t("cc.mem") + " " + fmtBytes(p.mem_bytes || 0) }),
            p.blocked_calls ? h("span", { class: "warn-text", text: t("cc.blocked_calls", { n: p.blocked_calls }) }) : null,
          ),
          h("pre", { class: "lastline", text: p.last_line || "" }),
          h(
            "div",
            { class: "row" },
            h("button", { class: "btn small", type: "button", onclick: () => goPane(p.id) }, t("cc.view")),
            p.agent ? h("button", { class: "btn small primary", type: "button", onclick: () => dlgPrompt(p.agent.name || p.id) }, t("cc.prompt")) : null,
          ),
        ),
      );
    }
    v.append(grid);
  }

  function renderApprovals() {
    const v = clear($("#view-approvals"));
    if (!S.approvals.length) {
      v.append(h("p", { class: "empty", text: t("empty.approvals") }));
      return;
    }
    for (const a of S.approvals) {
      const left = Math.max(0, Math.round((a.expires_ms - Date.now()) / 1000));
      const decide = (ev, allow) => decideApproval(ev, a, allow);
      v.append(
        h(
          "div",
          { class: "card approval" },
          h(
            "dl",
            null,
            h("dt", { text: t("ap.who") }),
            h("dd", { text: describePane(a.caller) }),
            h("dt", { text: t("ap.action") }),
            h("dd", { text: actionName(a.action) }),
            h("dt", { text: t("ap.target") }),
            h("dd", { text: describePane(a.target) }),
            h("dt", { text: t("ap.detail") }),
            h("dd", null, h("pre", { class: "mono", text: a.detail || "" })),
          ),
          h(
            "div",
            { class: "row" },
            h("span", { class: "muted", "data-exp": a.expires_ms, text: t("ap.expires", { s: left }) }),
            h("span", { class: "grow" }),
            h("button", { class: "btn danger", type: "button", onclick: (ev) => decide(ev, false) }, t("ap.deny")),
            h("button", { class: "btn primary", type: "button", onclick: (ev) => decide(ev, true) }, t("ap.allow")),
          ),
        ),
      );
    }
  }

  const SEVS = ["info", "notice", "critical"];
  const RANGES = { "15m": 15 * 60e3, "1h": 3600e3, "24h": 86400e3, all: Infinity };
  function logRows() {
    const f = S.logFilter;
    const evs = ((S.control && S.control.events) || []).slice().reverse();
    const since = Date.now() - RANGES[f.range || "1h"];
    const q = (f.q || "").toLowerCase();
    const base = evs.filter((e) => e.ts_ms >= since && (!f.pane || e.caller === f.pane || e.target === f.pane) && (!q || [e.kind, e.caller, e.target, e.detail].join(" ").toLowerCase().includes(q)));
    return { all: evs, base, rows: f.sev ? base.filter((e) => e.severity === f.sev) : base };
  }
  function renderLog() {
    const v = clear($("#view-log"));
    const f = S.logFilter;
    const { all, base, rows } = logRows();
    const seg = h("div", { class: "seg", role: "group" });
    for (const sv of ["", ...SEVS]) {
      const n = sv ? base.filter((e) => e.severity === sv).length : base.length;
      seg.append(
        h(
          "button",
          {
            type: "button",
            class: (f.sev === sv ? "on " : "") + (sv ? "sev-" + sv : ""),
            "aria-pressed": f.sev === sv ? "true" : "false",
            onclick: () => {
              f.sev = sv;
              renderLog();
            },
          },
          sv ? t("log.sev." + sv, { n }) : t("log.all", { n }),
        ),
      );
    }
    const paneSel = h("select", { "aria-label": t("log.any_pane") }, h("option", { value: "", text: t("log.any_pane") }), snap().panes.map((p) => h("option", { value: p.id, text: agentLabel(p) + " (" + p.id + ")" })));
    paneSel.value = f.pane;
    paneSel.addEventListener("change", () => {
      f.pane = paneSel.value;
      renderLog();
    });
    const rangeSel = h("select", { "aria-label": t("log.range.1h") }, Object.keys(RANGES).map((k) => h("option", { value: k, text: t("log.range." + k) })));
    rangeSel.value = f.range;
    rangeSel.addEventListener("change", () => {
      f.range = rangeSel.value;
      renderLog();
    });
    const search = h("input", { type: "text", class: "log-q", placeholder: t("log.search"), "aria-label": t("log.search"), value: f.q });
    search.addEventListener("input", () => {
      f.q = search.value;
      clearTimeout(S.logQTimer);
      S.logQTimer = setTimeout(() => {
        renderLog();
        const el = $("#view-log .log-q");
        if (el) {
          el.focus();
          el.setSelectionRange(el.value.length, el.value.length);
        }
      }, 200);
    });
    const exportBtn = h(
      "button",
      {
        class: "btn small",
        type: "button",
        onclick: async () => {
          const esc = (x) => '"' + String(x == null ? "" : x).replace(/"/g, '""') + '"';
          const csv = [["time", "severity", "kind", "caller", "target", "detail"].join(",")]
            .concat(rows.map((e) => [new Date(e.ts_ms).toISOString(), e.severity, e.kind, e.caller, e.target, e.detail].map(esc).join(",")))
            .join("\r\n");
          try {
            await invoke("clipboard_write", { text: csv });
            showToast(t("log.exported", { n: rows.length }), "ok");
          } catch (e) {
            showError(errText(e));
          }
        },
      },
      t("log.export"),
    );
    v.append(h("div", { class: "filters" }, seg, paneSel, rangeSel, search, h("span", { class: "grow" }), exportBtn, h("button", { class: "btn small", type: "button", onclick: () => runAction("verify") }, t("log.verify"))));
    if (!all.length) {
      v.append(h("p", { class: "empty", text: t("empty.log") }));
      return;
    }
    if (!rows.length) {
      v.append(h("p", { class: "empty", text: t("log.no_match") }));
      return;
    }
    const tb = h("tbody");
    for (const e of rows.slice(0, 500)) {
      tb.append(
        h(
          "tr",
          { class: "sev-" + e.severity },
          h("td", { class: "mono", text: fmtTime(e.ts_ms) }),
          h("td", null, h("span", { class: "sevb sev-" + e.severity, text: I18N.en["sev." + e.severity] ? t("sev." + e.severity) : e.severity })),
          h("td", { text: e.kind }),
          h("td", { text: e.caller }),
          h("td", { text: e.target }),
          h("td", { class: "mono", text: e.detail }),
        ),
      );
    }
    v.append(
      h(
        "table",
        { class: "log" },
        h("thead", null, h("tr", null, ["log.time", "log.level", "log.kind", "log.who", "log.target", "log.detail"].map((k) => h("th", { text: t(k) })))),
        tb,
      ),
    );
  }

  // ───────────── 시작 안내(3단계) ─────────────
  function onboardSteps() {
    const u = S.usage || {};
    return [
      { n: 1, done: agents().length > 0, act: () => runAction("start_agent") },
      { n: 2, done: !!u.claude_connected, act: () => dlgUsageConnect() },
      { n: 3, done: !!S.prefs.approvalsReviewed, act: () => ((S.prefs.approvalsReviewed = true), savePrefs(), dlgSettings("policy")) },
    ];
  }
  function onboardCard(onAct) {
    const steps = onboardSteps();
    const cur = steps.find((x) => !x.done);
    const box = h("div", { class: "onb" }, h("div", null, h("div", { class: "onb-t", text: t("ob.title") }), h("div", { class: "muted", text: t("ob.sub") })));
    for (const st of steps) {
      box.append(
        h(
          "div",
          { class: "step" + (st === cur ? " cur" : "") + (st.done ? " done" : "") },
          h("span", { class: "sn", text: st.done ? "✓" : String(st.n) }),
          h("div", null, h("b", { text: t("ob." + st.n + ".t") }), h("div", { class: "muted small", text: t("ob." + st.n + ".b") })),
          h(
            "button",
            {
              class: "btn" + (st === cur ? " primary" : ""),
              type: "button",
              onclick: () => {
                if (onAct) onAct();
                st.act();
              },
            },
            st.done ? t("ob.done") : t("ob." + st.n + ".btn"),
          ),
        ),
      );
    }
    box.append(h("div", { class: "muted small", text: t("ob.later") }));
    return box;
  }
  async function dlgOnboarding() {
    let closeFn = null;
    await modal({
      title: t("ob.title"),
      wide: true,
      body: onboardCard(() => closeFn && closeFn()),
      buttons: [{ label: t("dlg.close"), value: null, primary: true }],
      onReady: (d) => (closeFn = () => d(null)),
    });
  }

  function renderAll() {
    document.title = t("app.title") + (S.connected ? " — " + S.session : "");
    renderMenubar();
    renderToolbar();
    renderAlerts();
    renderSidebar();
    renderViewTabs();
    renderPanes();
    renderTip();
    renderStatus();
    renderStatusCounts();
    renderUsage();
    renderTokens();
    if (S.view === "control") renderControl();
    if (S.view === "approvals") renderApprovals();
    if (S.view === "log") renderLog();
  }

  // ───────────── 대화 상자 ─────────────
  function modal({ title, body, buttons, wide, onKey, onReady }) {
    return new Promise((resolve) => {
      S.modals++;
      const prevFocus = document.activeElement;
      const done = (v) => {
        S.modals--;
        back.remove();
        document.removeEventListener("keydown", keyh, true);
        if (prevFocus && prevFocus.focus) prevFocus.focus();
        resolve(v);
      };
      const keyh = (e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          done(null);
        } else if (onKey) onKey(e, done);
      };
      document.addEventListener("keydown", keyh, true);
      const foot = h("div", { class: "m-foot" });
      for (const b of buttons || []) {
        foot.append(
          h(
            "button",
            {
              class: "btn " + (b.primary ? "primary " : "") + (b.danger ? "danger" : ""),
              type: "button",
              onclick: async () => {
                if (b.check) {
                  const ok = await b.check();
                  if (!ok) return;
                }
                done(typeof b.value === "function" ? b.value() : b.value);
              },
            },
            b.label,
          ),
        );
      }
      const box = h("div", { class: "modal" + (wide ? " wide" : ""), role: "dialog", "aria-modal": "true" }, h("div", { class: "m-head", text: title }), h("div", { class: "m-body" }, body), foot);
      const back = h("div", { class: "backdrop" }, box);
      back.addEventListener("mousedown", (e) => {
        if (e.target === back) done(null);
      });
      document.body.append(back);
      const first = box.querySelector("input, select, textarea, button.primary");
      if (first) setTimeout(() => first.focus(), 0);
      if (onReady) onReady(done);
    });
  }
  const confirmDlg = (title, text, okLabel, danger) =>
    modal({ title, body: h("p", { text }), buttons: [{ label: t("dlg.cancel"), value: false }, { label: okLabel || t("dlg.ok"), value: true, primary: !danger, danger }] }).then((v) => !!v);
  function promptDlg(title, label, value) {
    const inp = h("input", { type: "text", value: value || "", maxlength: 120 });
    return modal({
      title,
      body: field(label, inp),
      buttons: [{ label: t("dlg.cancel"), value: null }, { label: t("dlg.ok"), value: () => inp.value, primary: true }],
      onKey: (e, done) => {
        if (e.key === "Enter" && document.activeElement === inp) {
          e.preventDefault();
          done(inp.value);
        }
      },
    });
  }
  function field(label, control, help) {
    return h("label", { class: "field" }, h("span", { class: "f-label", text: label }), control, help ? h("span", { class: "f-help", text: help }) : null);
  }
  function showError(msg) {
    showToast(t("err.title") + ": " + msg, "bad");
  }
  function showToast(text, level) {
    const el = h("div", { class: "toast " + (level || "info"), text, onclick: () => el.remove() });
    $("#toasts").append(el);
    setTimeout(() => el.remove(), level === "critical" ? 20000 : level === "bad" ? 9000 : 5000);
  }
  async function confirmOpenUrl(uri) {
    if (!/^https?:\/\//i.test(uri)) return;
    if (await confirmDlg(t("confirm.open_url"), uri, t("dlg.ok"))) invoke("open_url", { url: uri }).catch((e) => showError(errText(e)));
  }

  function profileSelect(current, withDefault) {
    const sel = h("select");
    if (withDefault) sel.append(h("option", { value: "", text: t("ws.profile.default", { p: profName(defaultProfile()) }) }));
    for (const p of ["none", "restricted", "container", "wsl"]) {
      if (p === "wsl" && !IS_WIN) continue;
      sel.append(h("option", { value: p, text: t("profile." + p) }));
    }
    sel.value = current || "";
    return sel;
  }

  async function dlgNewWorkspace() {
    if (!S.connected) return connect(true);
    const folder = h("input", { type: "text", placeholder: IS_WIN ? "C:\\work\\my-project" : "~/work/my-project" });
    const browse = h(
      "button",
      {
        class: "btn",
        type: "button",
        onclick: async () => {
          const p = await invoke("pick_folder");
          if (p) {
            folder.value = p;
            if (!label.value) label.value = p.split(/[\\/]/).filter(Boolean).pop() || "";
          }
        },
      },
      t("dlg.browse"),
    );
    const label = h("input", { type: "text", maxlength: 60 });
    const prof = profileSelect("", true);
    const body = h(
      "div",
      null,
      field(t("ws.folder"), h("div", { class: "row" }, folder, browse), t("ws.folder.help")),
      field(t("ws.label"), label, t("ws.label.help")),
      field(t("ws.profile"), prof, t("profile.help")),
      strengthTable(),
    );
    const ok = await modal({ title: t("ws.title"), body, wide: true, buttons: [{ label: t("dlg.cancel"), value: false }, { label: t("dlg.create"), value: true, primary: true }] });
    if (!ok) return;
    setView("terminal");
    await tryRpc("workspace.create", { cwd: folder.value.trim() || null, label: label.value.trim() || null, profile: prof.value || null, focus: true });
  }

  async function loadKinds() {
    if (!S.kinds.length) S.kinds = await invoke("agent_kinds").catch(() => []);
    return S.kinds;
  }
  // 이름 기본값: cc-1, cc-2, codex-1 … (Claude Code는 짧게 cc)
  const NAME_PREFIX = { claude: "cc" };
  function nextAgentName(kind) {
    const used = new Set(agents().map((p) => p.agent.name));
    const pre = NAME_PREFIX[kind] || kind;
    for (let i = 1; i < 100; i++) {
      const n = pre + "-" + i;
      if (!used.has(n)) return n;
    }
    return pre + "-" + Date.now();
  }
  async function dlgStartAgent(paneId) {
    const kinds = await loadKinds();
    const kind = h("select", null, kinds.map((k) => h("option", { value: k.kind, text: k.display + " (" + k.kind + ")" })));
    // 지난번 고른 종류를 기억한다.
    if (S.prefs.lastKind && kinds.some((k) => k.kind === S.prefs.lastKind)) kind.value = S.prefs.lastKind;
    const name = h("input", { type: "text", maxlength: 40, value: nextAgentName(kind.value || (kinds[0] ? kinds[0].kind : "agent")) });
    let nameTouched = false;
    name.addEventListener("input", () => (nameTouched = true));
    kind.addEventListener("change", () => {
      if (!nameTouched) name.value = nextAgentName(kind.value);
    });
    const target = paneId || focusedPane();
    const tp = target ? paneInfo(target) : null;
    const busy = tp && (tp.agent || (tp.title && !/^(pwsh|powershell|bash|zsh|sh|fish|cmd)/i.test(tp.title)));
    const whereSplit = h("input", { type: "radio", name: "where", value: "split", checked: !!busy || !target });
    const whereHere = h("input", { type: "radio", name: "where", value: "here", checked: !busy && !!target });
    const err = h("div", { class: "f-err" });
    const body = h(
      "div",
      null,
      field(t("ag.kind"), kind, t("ag.kind.help")),
      field(t("ag.name"), name, t("ag.name.help")),
      h("div", { class: "field" }, h("span", { class: "f-label", text: t("ag.where") }), h("label", { class: "radio" }, whereSplit, t("ag.where.split")), h("label", { class: "radio" }, whereHere, t("ag.where.here"))),
      err,
    );
    const ok = await modal({
      title: t("ag.title"),
      body,
      buttons: [
        { label: t("dlg.cancel"), value: false },
        {
          label: t("dlg.start"),
          value: true,
          primary: true,
          check: async () => {
            if (!/^[A-Za-z0-9_-]+$/.test(name.value)) {
              err.textContent = t("ag.bad_name");
              return false;
            }
            return true;
          },
        },
      ],
    });
    if (!ok) return;
    S.prefs.lastKind = kind.value;
    savePrefs();
    setView("terminal");
    let pane = target;
    let made = false;
    if (!pane || whereSplit.checked) {
      const r = await tryRpc("pane.split", { target: target || undefined, direction: "right", focus: true });
      if (!r) return;
      pane = r.pane || focusedPane();
      made = true;
    }
    const started = await tryRpc("agent.start", { kind: kind.value, name: name.value, pane });
    // 시작하지 못했으면 이번에 새로 만든 빈 창은 닫아 되돌린다.
    if (!started && made) await rpc("pane.close", { pane }).catch(() => {});
  }

  async function dlgPrompt(targetName) {
    const ags = agents();
    if (!ags.length) {
      await modal({ title: t("pr.title"), body: h("p", { text: t("pr.none") }), buttons: [{ label: t("dlg.cancel"), value: false }, { label: t("act.start_agent"), value: true, primary: true }] }).then((v) => v && dlgStartAgent());
      return;
    }
    const sel = h("select", null, ags.map((p) => h("option", { value: p.agent.name || p.id, text: agentLabel(p) + " · " + t("state." + p.agent.state) })));
    if (targetName) sel.value = targetName;
    const text = h("textarea", { rows: 5, maxlength: 8000 });
    const force = h("input", { type: "checkbox" });
    const body = h("div", null, field(t("pr.target"), sel), field(t("pr.text"), text, t("pr.text.help")), h("label", { class: "check" }, force, t("pr.force")));
    const ok = await modal({ title: t("pr.title"), body, wide: true, buttons: [{ label: t("dlg.cancel"), value: false }, { label: t("dlg.send"), value: true, primary: true }] });
    if (!ok || !text.value.trim()) return;
    const r = await tryRpc("agent.prompt", { target: sel.value, text: text.value, force: force.checked });
    if (r) showToast(t("ap.decided"), "ok");
  }

  async function dlgSession() {
    const name = await promptDlg(t("act.connect_session"), t("set.cn.session"), S.session);
    if (!name) return;
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(name)) return showError(name);
    S.session = name;
    S.prefs.session = name;
    savePrefs();
    for (const e of terms.values()) {
      e.term.dispose();
      e.el.remove();
    }
    terms.clear();
    S.snap = null;
    S.connected = false;
    await invoke("disconnect");
    await connect(true);
  }

  function dlgShortcuts() {
    const tb = h("tbody");
    for (const a of ACTIONS.filter((x) => x.keys)) tb.append(h("tr", null, h("td", { text: actLabel(a) }), h("td", null, h("kbd", { text: keyLabel(a.keys) }))));
    if (IS_WIN) tb.append(h("tr", null, h("td", { text: t("act.paste") }), h("td", null, h("kbd", { text: "Ctrl+V" }))));
    return modal({ title: t("keys.title"), body: h("table", { class: "keys" }, tb), buttons: [{ label: t("dlg.close"), value: true, primary: true }] });
  }

  async function dlgAbout() {
    const st = S.connected ? await rpc("server.status").catch(() => null) : null;
    const lines = t("about.body", { v: S.info.version || "?" }).split("\n");
    const body = h("div", { class: "about" }, h("img", { src: "icon.svg", alt: "", width: 72, height: 72 }), h("div", null, lines.map((l) => h("p", { text: l })), st ? h("p", { class: "muted", text: t("about.server", { s: `v${st.version} · pid ${st.pid} · ${st.session}` }) }) : null));
    return modal({ title: t("act.about"), body, buttons: [{ label: t("dlg.close"), value: true, primary: true }] });
  }

  // 명령 팔레트: 모든 동작을 이름(한국어·영어)으로 찾는다.
  function palette() {
    const inp = h("input", { type: "text", placeholder: t("pal.placeholder") });
    const list = h("div", { class: "pal-list" });
    let items = [];
    let idx = 0;
    const score = (a, q) => {
      const hay = (I18N.ko["act." + a.id] + " " + I18N.en["act." + a.id] + " " + (a.kw || "") + " " + a.id).toLowerCase();
      return q.split(/\s+/).every((w) => hay.includes(w));
    };
    const draw = () => {
      clear(list);
      const q = inp.value.trim().toLowerCase();
      items = ACTIONS.filter((a) => !q || score(a, q));
      idx = clamp(idx, 0, Math.max(0, items.length - 1));
      if (!items.length) list.append(h("div", { class: "muted", text: t("pal.none") }));
      items.forEach((a, i) =>
        list.append(
          h(
            "div",
            { class: "pal-item" + (i === idx ? " sel" : ""), onmousedown: (e) => (e.preventDefault(), doneRef(a.id)) },
            h("span", { text: actLabel(a) }),
            h("span", { class: "keys", text: a.keys ? keyLabel(a.keys) : "" }),
          ),
        ),
      );
      const s = list.querySelector(".sel");
      if (s) s.scrollIntoView({ block: "nearest" });
    };
    let doneRef = () => {};
    inp.addEventListener("input", () => {
      idx = 0;
      draw();
    });
    draw();
    modal({
      title: t("act.palette"),
      body: h("div", null, inp, list),
      wide: true,
      buttons: [],
      onReady: (done) => (doneRef = done),
      onKey: (e, done) => {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          idx++;
          draw();
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          idx--;
          draw();
        } else if (e.key === "Enter") {
          e.preventDefault();
          if (items[idx]) done(items[idx].id);
        }
      },
    }).then((id) => {
      if (id) setTimeout(() => runAction(id), 0);
    });
  }

  // ───────────── 격리 강도 표 ─────────────
  const STRENGTH = {
    ko: {
      windows: {
        none: "막지 않음",
        restricted: "메모리·프로세스 수만 제한. 파일·네트워크는 막지 못함(강도 low)",
        container: "프로젝트 폴더만 보임, KMttt 소켓 없음. 외부 접속 목록은 권고 수준",
        wsl: "WSL 리눅스 안에서 실행. 그 안의 격리 설정을 따름",
      },
      linux: {
        none: "막지 않음",
        restricted: "프로젝트 밖 쓰기·KMttt 설정·기록 수정 차단, TCP는 허용 목록으로만(커널 6.7+). 로컬 Unix 소켓 서비스는 못 막음",
        container: "프로젝트 폴더만 보임, KMttt 소켓 없음. 외부 접속 목록은 권고 수준",
      },
      macos: {
        none: "막지 않음",
        restricted: "sandbox 프로필로 KMttt 설정·기록 수정과 외부 연결 차단. 로컬 서비스는 못 막음",
        container: "프로젝트 폴더만 보임, KMttt 소켓 없음. 외부 접속 목록은 권고 수준",
      },
    },
    en: {
      windows: {
        none: "Blocks nothing",
        restricted: "Limits memory and process count only. Does not block files or network (strength low)",
        container: "Sees only the project folder, no KMttt socket. Egress list is advisory",
        wsl: "Runs inside WSL Linux and follows its isolation",
      },
      linux: {
        none: "Blocks nothing",
        restricted: "Blocks writes outside the project and edits to KMttt config/log; TCP only via the allow list (kernel 6.7+). Local Unix-socket services are not blocked",
        container: "Sees only the project folder, no KMttt socket. Egress list is advisory",
      },
      macos: {
        none: "Blocks nothing",
        restricted: "sandbox profile blocks KMttt config/log edits and outbound connections. Local services are not blocked",
        container: "Sees only the project folder, no KMttt socket. Egress list is advisory",
      },
    },
  };
  function osKey() {
    return IS_WIN ? "windows" : IS_MAC ? "macos" : "linux";
  }
  function strengthTable() {
    const rows = STRENGTH[S.lang === "ko" ? "ko" : "en"][osKey()];
    return h(
      "div",
      { class: "strength" },
      h("div", { class: "f-label", text: t("set.sb.strength", { os: osKey() }) }),
      h(
        "table",
        null,
        h(
          "tbody",
          null,
          Object.entries(rows).map(([k, v]) => h("tr", null, h("td", { text: t("profile." + k).split(" — ")[0] }), h("td", { text: v }))),
        ),
      ),
    );
  }

  async function dlgProtection(prof) {
    const rows = STRENGTH[S.lang === "ko" ? "ko" : "en"][osKey()];
    const body = h(
      "div",
      null,
      h("p", { text: t("profile." + prof) }),
      h("div", { class: "strength" }, h("table", null, h("tbody", null, Object.entries(rows).map(([k, v]) => h("tr", { class: k === prof ? "cur" : "" }, h("td", { text: t("profile." + k).split(" — ")[0] }), h("td", { text: v })))))),
      h("p", { class: "muted small", text: t("prot.change") }),
    );
    const v = await modal({ title: t("prot.title", { p: profName(prof) }), body, wide: true, buttons: [{ label: t("act.settings"), value: "set" }, { label: t("dlg.close"), value: null, primary: true }] });
    if (v === "set") dlgSettings("sandbox");
  }

  // ───────────── 설정 ─────────────
  const DEC_RANK = { deny: 0, ask: 1, allow: 2 };
  // kmttt-policy Defaults::default()와 같아야 한다.
  const POL_DEFAULTS = { agent_to_agent_prompt: "ask", agent_to_agent_run: "ask", agent_create: "allow" };
  const PROF_RANK = { none: 0, restricted: 1, wsl: 2, container: 2 };
  const LEVEL_RANK = { off: 0, metadata: 1, full: 2 };

  async function dlgSettings(startTab) {
    let st;
    try {
      st = await invoke("settings_read");
    } catch (e) {
      return showError(errText(e));
    }
    const cfg = st.config;
    const pol = st.policy;
    const changes = { config: new Map(), policy: new Map() };
    const prefChanges = {};
    const weaker = [];
    const set = (file, path, value) => changes[file].set(path.join("."), { path, value });

    const sel = (opts, cur, onchange) => {
      const s = h("select", null, opts.map(([v, l]) => h("option", { value: v, text: l })));
      s.value = cur;
      s.addEventListener("change", () => onchange(s.value));
      return s;
    };
    const chk = (cur, label, onchange) => {
      const c = h("input", { type: "checkbox", checked: !!cur });
      c.addEventListener("change", () => onchange(c.checked));
      return h("label", { class: "check" }, c, label);
    };
    const num = (cur, onchange, min, max) => {
      const n = h("input", { type: "number", value: String(cur), min, max });
      n.addEventListener("change", () => {
        const v = parseInt(n.value, 10);
        if (Number.isFinite(v)) onchange(clamp(v, min, max));
      });
      return n;
    };
    const txt = (cur, onchange, ph) => {
      const i = h("input", { type: "text", value: cur || "", placeholder: ph || "" });
      i.addEventListener("change", () => onchange(i.value));
      return i;
    };
    const fileNote = (which) =>
      h(
        "div",
        { class: "filenote" },
        h("span", { class: "muted", text: t("set.file") + ": " + (which === "config" ? st.config_path : st.policy_path) }),
        h("button", { class: "linkbtn", type: "button", onclick: () => dlgRaw(which) }, t("set.open_raw")),
        st[which + "_error"] ? h("div", { class: "f-err", text: st[which + "_error"] }) : null,
      );

    // 일반
    const general = h(
      "div",
      null,
      field(
        t("set.language"),
        sel([["ko", "한국어"], ["en", "English"]], S.lang, (v) => {
          prefChanges.lang = v;
          set("config", ["language"], v);
        }),
      ),
      field(t("set.theme"), sel([["dark", t("set.theme.dark")], ["light", t("set.theme.light")]], S.prefs.theme || "dark", (v) => (prefChanges.theme = v))),
      field(t("set.font_size"), num(fontSize(), (v) => (prefChanges.fontSize = v), 9, 28)),
      field(t("set.shell"), txt(cfg.shell.program, (v) => set("config", ["shell", "program"], v), IS_WIN ? "pwsh.exe" : "/bin/zsh"), t("set.shell.help")),
      chk(cfg.ui.mask_screen, t("set.mask_screen"), (v) => {
        set("config", ["ui", "mask_screen"], v);
        if (!v && cfg.ui.mask_screen) weaker.push(t("set.mask_screen"));
      }),
      chk(cfg.experimental.pane_history, t("set.pane_history"), (v) => set("config", ["experimental", "pane_history"], v)),
      fileNote("config"),
    );

    // 보안 정책
    const decOpts = [["allow", t("dec.allow")], ["ask", t("dec.ask")], ["deny", t("dec.deny")]];
    const decField = (key) =>
      field(
        t("set.d." + key),
        sel(decOpts, pol.defaults[key], (v) => {
          set("policy", ["defaults", key], v);
          if (DEC_RANK[v] > DEC_RANK[pol.defaults[key]]) weaker.push(t("set.d." + key) + ": " + t("dec." + pol.defaults[key]) + " → " + t("dec." + v));
        }),
        t("set.default", { v: t("dec." + POL_DEFAULTS[key]) }),
      );
    let rules = (pol.rule || []).map((r) => JSON.parse(JSON.stringify(r)));
    const rulesBox = h("div", { class: "rules" });
    // 규칙의 action은 서버 메서드 이름(glob)이다.
    const ACTIONS_P = ["agent.prompt", "agent.start", "pane.run", "pane.send", "pane.read", "pane.close", "agent.*", "pane.*", "*"];
    const drawRules = () => {
      clear(rulesBox);
      rules.forEach((r, i) => {
        const from = txt((r.from && r.from.agent) || "", (v) => {
          r.from = r.from || {};
          if (v) r.from.agent = v;
          else delete r.from.agent;
          set("policy", ["rule"], rules);
        }, "codex");
        const act = sel((ACTIONS_P.includes(r.action) ? ACTIONS_P : ACTIONS_P.concat([r.action])).map((a) => [a, a]), r.action || "*", (v) => {
          r.action = v;
          set("policy", ["rule"], rules);
        });
        const dec = sel(decOpts, r.decision || "ask", (v) => {
          if (DEC_RANK[v] > DEC_RANK[r.decision || "ask"]) weaker.push(t("set.rules") + " #" + (i + 1));
          r.decision = v;
          set("policy", ["rule"], rules);
        });
        rulesBox.append(
          h(
            "div",
            { class: "rule" },
            h("span", { class: "muted", text: "#" + (i + 1) }),
            from,
            act,
            dec,
            h(
              "button",
              {
                class: "btn small",
                type: "button",
                onclick: () => {
                  if (r.decision !== "allow") weaker.push(t("set.rules") + " #" + (i + 1) + " " + t("set.rule.remove"));
                  rules.splice(i, 1);
                  set("policy", ["rule"], rules);
                  drawRules();
                },
              },
              t("set.rule.remove"),
            ),
          ),
        );
      });
      rulesBox.append(
        h(
          "button",
          {
            class: "btn small",
            type: "button",
            onclick: () => {
              rules.push({ id: "gui-" + (rules.length + 1), from: {}, action: "agent.prompt", to: {}, decision: "ask" });
              set("policy", ["rule"], rules);
              drawRules();
            },
          },
          t("set.rules.add"),
        ),
      );
    };
    drawRules();
    const policy = h(
      "div",
      null,
      decField("agent_to_agent_prompt"),
      decField("agent_to_agent_run"),
      field(
        t("set.d.agent_read"),
        sel([["masked", t("read.masked")], ["deny", t("read.deny")]], pol.defaults.agent_read, (v) => {
          set("policy", ["defaults", "agent_read"], v);
          if (v === "masked" && pol.defaults.agent_read === "deny") weaker.push(t("set.d.agent_read"));
        }),
      ),
      decField("agent_create"),
      field(t("set.d.ask_timeout_secs"), num(pol.defaults.ask_timeout_secs, (v) => set("policy", ["defaults", "ask_timeout_secs"], v), 5, 3600)),
      field(
        t("set.d.rate_per_min"),
        num(pol.defaults.rate_per_min, (v) => {
          set("policy", ["defaults", "rate_per_min"], v);
          if (v > pol.defaults.rate_per_min) weaker.push(t("set.d.rate_per_min"));
        }, 1, 10000),
      ),
      h("div", { class: "field" }, h("span", { class: "f-label", text: t("set.rules") }), h("span", { class: "f-help", text: t("set.rules.help") }), h("div", { class: "rule head" }, h("span"), h("span", { text: t("set.rule.from") }), h("span", { text: t("set.rule.action") }), h("span", { text: t("set.rule.decision") }), h("span")), rulesBox),
      fileNote("policy"),
    );

    // 격리
    const sb = pol.sandbox;
    const profField = (key) =>
      field(
        t("set.sb." + key),
        (() => {
          const s = profileSelect(sb[key], false);
          s.addEventListener("change", () => {
            set("policy", ["sandbox", key], s.value);
            if ((PROF_RANK[s.value] || 0) < (PROF_RANK[sb[key]] || 0)) weaker.push(t("set.sb." + key) + ": " + sb[key] + " → " + s.value);
          });
          return s;
        })(),
      );
    const egress = h("textarea", { rows: 5 });
    egress.value = (sb.egress_allow || []).join("\n");
    egress.addEventListener("change", () => {
      const list = egress.value.split(/\s+/).map((x) => x.trim()).filter(Boolean);
      set("policy", ["sandbox", "egress_allow"], list);
      if (list.some((d) => !(sb.egress_allow || []).includes(d))) weaker.push(t("set.sb.egress_allow"));
    });
    const ports = txt((sb.allow_tcp_ports || []).join(", "), (v) => {
      const list = v.split(/[\s,]+/).map((x) => parseInt(x, 10)).filter((n) => n > 0 && n < 65536);
      set("policy", ["sandbox", "allow_tcp_ports"], list);
      if (list.some((p) => !(sb.allow_tcp_ports || []).includes(p))) weaker.push(t("set.sb.allow_tcp_ports"));
    });
    const sandbox = h(
      "div",
      null,
      profField("default_profile"),
      profField("worktree_profile"),
      strengthTable(),
      chk(sb.egress_enforce, t("set.sb.egress_enforce"), (v) => {
        set("policy", ["sandbox", "egress_enforce"], v);
        if (!v && sb.egress_enforce) weaker.push(t("set.sb.egress_enforce"));
      }),
      field(t("set.sb.egress_allow"), egress),
      field(t("set.sb.allow_tcp_ports"), ports),
      field(t("set.sb.memory_limit_mb"), num(sb.memory_limit_mb, (v) => set("policy", ["sandbox", "memory_limit_mb"], v), 128, 1048576)),
      field(t("set.sb.process_limit"), num(sb.process_limit, (v) => set("policy", ["sandbox", "process_limit"], v), 8, 100000)),
      fileNote("policy"),
    );

    // 기록·마스킹
    const lg = cfg.ledger;
    const ledger = h(
      "div",
      null,
      field(
        t("set.lg.level"),
        sel([["full", t("set.lg.level.full")], ["metadata", t("set.lg.level.metadata")], ["off", t("set.lg.level.off")]], lg.level, (v) => {
          set("config", ["ledger", "level"], v);
          if (LEVEL_RANK[v] < LEVEL_RANK[lg.level]) weaker.push(t("set.lg.level") + ": " + lg.level + " → " + v);
        }),
      ),
      chk(lg.encrypt, t("set.lg.encrypt"), (v) => {
        set("config", ["ledger", "encrypt"], v);
        if (!v && lg.encrypt) weaker.push(t("set.lg.encrypt"));
      }),
      field(t("set.lg.mirror_file"), txt(lg.mirror_file, (v) => set("config", ["ledger", "mirror_file"], v))),
      field(t("set.lg.syslog_udp"), txt(lg.syslog_udp, (v) => set("config", ["ledger", "syslog_udp"], v), "127.0.0.1:514")),
      h("div", { class: "field" }, h("span", { class: "f-label", text: t("set.lg.redact") }), h("span", { class: "f-help", text: t("set.lg.redact.help") })),
      fileNote("config"),
    );

    // 연결·알림
    const connection = h(
      "div",
      null,
      field(t("set.cn.session"), txt(S.session, (v) => (prefChanges.session = v)), t("set.cn.session.help")),
      field(
        t("set.cn.toast"),
        sel([["kmttt", t("set.cn.toast.kmttt")], ["os", t("set.cn.toast.os")], ["off", t("set.cn.toast.off")]], cfg.ui.toast.delivery === "terminal" ? "kmttt" : cfg.ui.toast.delivery, (v) => {
          set("config", ["ui", "toast", "delivery"], v);
          prefChanges.toast = v;
        }),
      ),
      chk(cfg.console.allow_actions, t("set.cn.console"), (v) => {
        set("config", ["console", "allow_actions"], v);
        if (v && !cfg.console.allow_actions) weaker.push(t("set.cn.console"));
      }),
      h("p", { class: "f-help", text: t("set.cn.machines") }),
      fileNote("config"),
    );

    const panes = { general, policy, sandbox, ledger, connection };
    const tabsEl = h("div", { class: "set-tabs" });
    const content = h("div", { class: "set-content" });
    const show = (k) => {
      tabsEl.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b.dataset.k === k));
      clear(content).append(panes[k]);
    };
    for (const k of Object.keys(panes)) tabsEl.append(h("button", { type: "button", "data-k": k, onclick: () => show(k) }, t("set.tab." + (k === "sandbox" ? "sandbox" : k))));
    show(panes[startTab] ? startTab : "general");

    const result = await modal({
      title: t("set.title"),
      wide: true,
      body: h("div", { class: "settings" }, tabsEl, content),
      buttons: [
        { label: t("dlg.cancel"), value: false },
        {
          label: t("dlg.save"),
          value: true,
          primary: true,
          check: async () => {
            const uniq = [...new Set(weaker)];
            if (!uniq.length) return true;
            return modal({
              title: t("set.weaker.title"),
              body: h("div", null, h("p", { text: t("set.weaker.body") }), h("ul", { class: "weaker" }, uniq.map((w) => h("li", { text: w })))),
              buttons: [{ label: t("dlg.cancel"), value: false }, { label: t("set.weaker.ok"), value: true, danger: true }],
            }).then((v) => !!v);
          },
        },
      ],
    });
    if (!result) return;
    // 앱 전용 설정
    if (prefChanges.theme) setTheme(prefChanges.theme);
    if (prefChanges.fontSize && prefChanges.fontSize !== fontSize()) {
      S.prefs.fontSize = prefChanges.fontSize;
      savePrefs();
      await applyFont();
    }
    if (prefChanges.toast) {
      S.prefs.toast = prefChanges.toast;
      savePrefs();
    }
    if (prefChanges.lang && prefChanges.lang !== S.lang) setLang(prefChanges.lang);
    // 서버 설정 파일
    let okAll = true;
    for (const file of ["config", "policy"]) {
      if (!changes[file].size) continue;
      try {
        const r = await invoke("settings_save", { file, changes: [...changes[file].values()] });
        showToast(r.reloaded ? t("set.saved") : t("set.saved_offline"), "ok");
      } catch (e) {
        okAll = false;
        showError(errText(e));
      }
    }
    if (okAll) await loadPolicy();
    if (prefChanges.session && prefChanges.session !== S.session && /^[A-Za-z0-9_-]{1,64}$/.test(prefChanges.session)) {
      S.prefs.session = prefChanges.session;
      savePrefs();
      S.session = prefChanges.session;
      await invoke("disconnect");
      S.connected = false;
      await connect(true);
    }
    renderAll();
  }

  async function dlgRaw(which) {
    const r = await invoke("settings_raw", { file: which });
    const ta = h("textarea", { class: "mono raw", rows: 22, spellcheck: "false" });
    ta.value = r.text;
    const err = h("div", { class: "f-err" });
    await modal({
      title: t("set.raw.title", { f: which + ".toml" }),
      wide: true,
      body: h("div", null, h("p", { class: "f-help", text: t("set.raw.help") }), h("p", { class: "muted", text: r.path }), ta, err),
      buttons: [
        { label: t("dlg.cancel"), value: false },
        {
          label: t("dlg.save"),
          value: true,
          primary: true,
          check: async () => {
            try {
              await invoke("settings_save_raw", { file: which, text: ta.value });
              showToast(t("set.saved"), "ok");
              return true;
            } catch (e) {
              err.textContent = errText(e);
              return false;
            }
          },
        },
      ],
    });
    await loadPolicy();
    renderAll();
  }

  // ───────────── 처음 사용 마법사 ─────────────
  async function wizard() {
    const steps = 4;
    let i = 1;
    let lang = S.lang;
    let prof = defaultProfile() === "none" && !IS_WIN ? "restricted" : defaultProfile();
    let folder = "";
    let kind = "";
    const kinds = await loadKinds();
    while (i >= 1 && i <= steps) {
      const body = h("div", { class: "wizard" });
      body.append(h("div", { class: "muted", text: t("wz.step", { i, n: steps }) }));
      body.append(h("h3", { text: t("wz." + i + ".t") }), h("p", { text: t("wz." + i + ".b") }));
      if (i === 1) {
        body.append(h("p", { class: "notice", text: t("guide.notice") }));
        for (const [v, l] of [["ko", "한국어"], ["en", "English"]]) {
          const r = h("input", { type: "radio", name: "wl", checked: lang === v });
          r.addEventListener("change", () => {
            lang = v;
          });
          body.append(h("label", { class: "radio big" }, r, l));
        }
      } else if (i === 2) {
        for (const p of ["none", "restricted", "container", "wsl"]) {
          if (p === "wsl" && !IS_WIN) continue;
          const r = h("input", { type: "radio", name: "wp", checked: prof === p });
          r.addEventListener("change", () => (prof = p));
          body.append(h("label", { class: "radio big" }, r, t("profile." + p)));
        }
        body.append(strengthTable());
      } else if (i === 3) {
        const inp = h("input", { type: "text", value: folder });
        inp.addEventListener("input", () => (folder = inp.value));
        body.append(
          h(
            "div",
            { class: "row" },
            inp,
            h(
              "button",
              {
                class: "btn",
                type: "button",
                onclick: async () => {
                  const p = await invoke("pick_folder");
                  if (p) {
                    inp.value = p;
                    folder = p;
                  }
                },
              },
              t("dlg.browse"),
            ),
          ),
        );
      } else if (i === 4) {
        for (const k of kinds) {
          const r = h("input", { type: "radio", name: "wk", checked: kind === k.kind });
          r.addEventListener("change", () => (kind = k.kind));
          body.append(h("label", { class: "radio big" }, r, k.display));
        }
      }
      const v = await modal({
        title: t("wz.title"),
        body,
        buttons: [
          i > 1 ? { label: t("dlg.back"), value: "back" } : { label: t("dlg.skip"), value: "skip" },
          i === 4 ? { label: t("dlg.skip"), value: "skip4" } : null,
          i < steps ? { label: t("dlg.next"), value: "next", primary: true } : { label: t("dlg.done"), value: "done", primary: true },
        ].filter(Boolean),
      });
      if (v == null || v === "skip") break;
      if (v === "back") {
        i--;
        continue;
      }
      // 단계별 적용
      if (i === 1 && lang !== S.lang) {
        setLang(lang);
        invoke("settings_save", { file: "config", changes: [{ path: ["language"], value: lang }] }).catch(() => {});
      }
      if (i === 2 && prof !== defaultProfile()) {
        try {
          await invoke("settings_save", { file: "policy", changes: [{ path: ["sandbox", "default_profile"], value: prof }] });
          await loadPolicy();
        } catch (e) {
          showError(errText(e));
        }
      }
      if (i === 3 && folder.trim() && S.connected) {
        await tryRpc("workspace.create", { cwd: folder.trim(), focus: true });
      }
      if (i === 4) {
        if (v === "done" && kind && S.connected) await tryRpc("agent.start", { kind, name: nextAgentName(kind), pane: focusedPane() });
        break;
      }
      i++;
    }
    S.prefs.wizardDone = true;
    savePrefs();
    showToast(t("wz.done"), "ok");
    renderAll();
  }

  // ───────────── 시작 ─────────────
  // 화면 코드 오류도 사용자에게 보이게 한다(조용히 멈춘 것처럼 보이지 않도록).
  window.addEventListener("error", (e) => showToast("UI error: " + (e.message || e), "bad"));
  window.addEventListener("unhandledrejection", (e) => showToast("UI error: " + errText(e.reason), "bad"));
  async function boot() {
    S.prefs = await invoke("prefs_get").catch(() => ({}));
    S.lang = S.prefs.lang || ((navigator.language || "").toLowerCase().startsWith("ko") ? "ko" : "en");
    S.session = S.prefs.session || "default";
    document.documentElement.lang = S.lang;
    document.documentElement.dataset.theme = S.prefs.theme || "dark";
    S.info = await invoke("app_info").catch(() => ({}));
    renderAll();
    await measureCell();
    loadKinds().then(() => renderPanes()).catch(() => {});
    pollUsage(true);
    await connect(true);
    if (!S.prefs.wizardDone) wizard();
  }
  boot();
})();
