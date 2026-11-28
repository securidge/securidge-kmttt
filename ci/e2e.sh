#!/usr/bin/env bash
# Securidge KMttt 보안 E2E 점검(Linux·macOS). 실제 서버·PTY·소켓으로 핵심 보안 동작을 확인한다.
# 사용: ci/e2e.sh [kmttt 실행 파일]   (기본: target/debug/kmttt)
set -u
K=${1:-$(pwd)/target/debug/kmttt}
K=$(cd "$(dirname "$K")" && pwd)/$(basename "$K")
T=$(mktemp -d)
export KMTTT_CONFIG_DIR=$T/cfg KMTTT_DATA_DIR=$T/data KMTTT_SOCKET_DIR=$T/sock
# 테스트 중 비밀처럼 보이는 환경 변수가 학습돼 출력이 가려지는 것을 막는다.
unset GITHUB_TOKEN ACTIONS_RUNTIME_TOKEN ACTIONS_ID_TOKEN_REQUEST_TOKEN 2>/dev/null || true
OS=$(uname -s)
# pane 셸 명령에 bash 문법(/dev/tcp 등)을 쓰므로 셸을 bash로 고정한다(macOS 기본은 zsh).
export SHELL=/bin/bash
# setsid 명령이 없는 OS(macOS)도 있어 파이썬으로 세션을 끊고 실행한다.
DETACH="python3 -c 'import os,sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])'"
# 이 테스트의 서버 PID(Linux는 환경 변수로 정확히, macOS는 CI가 깨끗하다고 보고 이름으로).
server_pids() {
  if [ "$OS" = Linux ]; then
    pgrep -f "server run" | while read -r p; do tr "\0" " " </proc/$p/environ 2>/dev/null | grep -q "$T" && echo "$p"; done
  else
    pgrep -f "kmttt.*server run"
  fi
}
PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "  ok   $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
cleanup() { "$K" server stop >/dev/null 2>&1; pkill -f "$T" >/dev/null 2>&1; rm -rf "$T"; }
trap cleanup EXIT
wait_for() { # wait_for <seconds> <command...>
  local n=$(($1 * 10)); shift
  while [ $n -gt 0 ]; do eval "$@" && return 0; sleep 0.1; n=$((n - 1)); done
  return 1
}
pending() { "$K" approval list 2>/dev/null | grep -c '"id"'; }
first_id() { "$K" approval list 2>/dev/null | grep '"id"' | head -1 | tr -dc 0-9; }

echo "== server start race"
for i in 1 2 3 4 5 6; do ("$K" workspace list >/dev/null 2>&1 &); done
sleep 3
check "exactly one server after 6 concurrent starts" '[ "$(server_pids | wc -l | tr -d " ")" = 1 ]'

echo "== layout"
"$K" workspace create --label e2e >/dev/null
A=$("$K" pane list | grep '"id"' | head -1 | tr -d ' ",' | cut -d: -f2-)
B=$("$K" pane split "$A" --direction right --no-focus | grep '"pane"' | tr -d ' ",' | cut -d: -f2-)
check "two panes ($A, $B)" '[ -n "$A" ] && [ -n "$B" ]'
sleep 1

echo "== F1 agent-to-agent approval"
"$K" pane run "$A" "$K pane run $B 'echo E2E-ALLOWED'; echo RC1=\$?" >/dev/null
check "pane→pane run is queued" 'wait_for 5 "[ \$(pending) -ge 1 ]"'
"$K" approval allow "$(first_id)" >/dev/null
check "allowed run reaches target" 'wait_for 5 "$K pane read $B --lines 5 | grep -q E2E-ALLOWED"'
"$K" pane run "$A" "$K pane run $B 'echo E2E-DENIED'; echo RC2=\$?" >/dev/null
wait_for 5 "[ \$(pending) -ge 1 ]"
"$K" approval deny "$(first_id)" >/dev/null
check "denied run exits 3" 'wait_for 5 "$K pane read $A --lines 5 | grep -q RC2=3"'
check "denied text never reached target" '! "$K" pane read "$B" --lines 20 | grep -q "^E2E-DENIED"'

echo "== F1 allow for this session"
"$K" pane run "$A" "$K pane run $B 'echo E2E-SESSION-1'; echo RC3=\$?" >/dev/null
wait_for 5 "[ \$(pending) -ge 1 ]"
"$K" approval allow "$(first_id)" --for-session >/dev/null
check "session-allowed run reaches target" 'wait_for 5 "$K pane read $B --lines 5 | grep -q E2E-SESSION-1"'
check "session approval is listed" '"$K" approval grants | grep -q "pane.run"'
"$K" pane run "$A" "$K pane run $B 'echo E2E-SESSION-2'; echo RC4=\$?" >/dev/null
check "same caller, action and target runs without asking" 'wait_for 5 "$K pane read $B --lines 5 | grep -q E2E-SESSION-2"'
check "nothing was queued for the repeat" '[ "$(pending)" = 0 ]'
"$K" pane run "$B" "$K pane run $A 'echo E2E-OTHER-WAY'; echo RC5=\$?" >/dev/null
check "the reverse direction still asks" 'wait_for 5 "[ \$(pending) -ge 1 ]"'
"$K" approval deny "$(first_id)" >/dev/null
"$K" pane run "$A" "( $K approval revoke > $T/selfrevoke.txt 2>&1 )" >/dev/null
check "an agent cannot revoke or widen approvals" 'wait_for 5 "grep -qi \"denied\\|only a person\\|not allowed\" $T/selfrevoke.txt"'
"$K" approval revoke >/dev/null
check "revoked approvals are gone" '! "$K" approval grants | grep -q "pane.run"'
"$K" pane run "$A" "$K pane run $B 'echo E2E-SESSION-3'; echo RC6=\$?" >/dev/null
check "after revoke it asks again" 'wait_for 5 "[ \$(pending) -ge 1 ]"'
"$K" approval deny "$(first_id)" >/dev/null

echo "== F1 escape attempts (orphaned callers)"
"$K" pane run "$A" "( $K pane run $B 'echo ESC1' >/dev/null 2>&1 & )" >/dev/null
check "orphan ( cmd & ) is still treated as the pane" 'wait_for 5 "$K approval list | grep -q \"\\\"caller\\\": \\\"$A\\\"\""'
"$K" pane run "$A" "( $DETACH env -u KMTTT_PANE $K pane run $B 'echo ESC2' >/dev/null 2>&1 & )" >/dev/null
check "setsid + env -u orphan is still treated as an agent" 'wait_for 5 "[ \$($K approval list | grep -c \"\\\"caller\\\"\") -ge 2 ]"'
"$K" pane run "$A" "( $DETACH env -u KMTTT_PANE $K approval allow 1 > $T/selfapprove.txt 2>&1 & )" >/dev/null
sleep 2
check "orphan cannot approve its own request" 'grep -qi "denied\|only a person\|not allowed" $T/selfapprove.txt'
for id in $("$K" approval list | grep '"id"' | tr -dc '0-9\n'); do "$K" approval deny "$id" >/dev/null; done
check "escaped commands never ran" '! "$K" pane read "$B" --lines 30 | grep -qE "^ESC[12]$"'
SP=$(server_pids | head -1)
# 회수기는 0.5초마다 돈다.
check "no zombie children left under the server (pid $SP)" 'wait_for 5 "[ -n \"$SP\" ] && ! ps -A -o ppid=,stat= | awk -v p=$SP \"\\\$1==p && \\\$2 ~ /Z/\" | grep -q ."'

echo "== F3 redaction"
"$K" pane run "$A" "export E2E_API_TOKEN=sk-e2eSECRETvalue1234567890abcd; echo \$E2E_API_TOKEN" >/dev/null
sleep 1.5
check "pane read never shows the secret" '! "$K" pane read "$A" --lines 10 | grep -q sk-e2eSECRET'
check "secret was replaced by a marker" '"$K" pane read "$A" --lines 10 | grep -q "REDACTED"'

echo "== F5 hook"
OUT=$(echo '{"tool_name":"Bash","tool_input":{"command":"rm -rf /"}}' | KMTTT_PANE=$A "$K" hook claude PreToolUse 2>&1; echo "rc=$?")
check "dangerous tool call blocked (exit 2)" 'echo "$OUT" | grep -q "rc=2"'
OUT=$(echo '{"tool_name":"Bash","tool_input":{"command":"cargo test"}}' | KMTTT_PANE=$A "$K" hook claude PreToolUse 2>&1; echo "rc=$?")
check "safe tool call allowed (exit 0)" 'echo "$OUT" | grep -q "rc=0"'

echo "== F4 restricted sandbox (Landlock)"
mkdir -p "$T/proj"
# /tmp는 restricted에서도 쓰기 허용(설계)이므로 바깥 경로는 저장소 안에 둔다.
OUTSIDE=$(pwd)/target/e2e-outside-$$
mkdir -p "$OUTSIDE"
OUT=$("$K" sandbox-exec --root "$T/proj" -- sh -c "echo a > $T/proj/x && echo IN_OK; echo b > $OUTSIDE/x 2>/dev/null && echo OUT_WRITTEN || echo OUT_BLOCKED" 2>&1)
rm -rf "$OUTSIDE"
if echo "$OUT" | grep -q "IN_OK"; then
  check "write outside root is blocked" 'echo "$OUT" | grep -q OUT_BLOCKED'
else
  echo "  skip Landlock not available here: $OUT"
fi

echo "== F4 restricted pane: egress"
python3 -c "import socket,time;s=socket.socket();s.bind(('127.0.0.1',47123));s.listen();time.sleep(20)" &
LPID=$!
"$K" workspace create --label sbx --profile restricted --cwd "$T/proj" --no-focus >/dev/null
sleep 1.5
R=$("$K" pane list | grep '"id"' | tr -d ' ",' | cut -d: -f2- | tail -1)
"$K" pane run "$R" 'echo PX=${HTTPS_PROXY:+set}; (exec 3<>/dev/tcp/127.0.0.1/47123) 2>/dev/null && echo DIRECT_LEAK || echo DIRECT_BLOCKED; echo CODE=$(curl -s -o /dev/null -w "%{http_connect}" https://blocked.example/ 2>/dev/null)' >/dev/null
sleep 3
OUTR=$("$K" pane read "$R" --lines 15)
check "restricted pane gets the egress proxy" 'echo "$OUTR" | grep -q "PX=set"'
if "$K" pane read "$R" --lines 40 | grep -q "no Landlock network rules\|Landlock is not available"; then
  echo "  skip kernel lacks Landlock network rules"
else
  check "direct TCP connection is blocked" 'echo "$OUTR" | grep -q DIRECT_BLOCKED'
fi
check "proxy refuses a host outside the allowlist (403)" 'echo "$OUTR" | grep -q "CODE=403"'
kill $LPID 2>/dev/null

echo "== F2 ledger"
check "ledger verifies" '"$K" audit verify | grep -q "\"ok\": true"'
check "a second ledger writer is refused" '"$K" server run 2>&1 | grep -q "already"'
LEDGER=$("$K" audit path)
check "approval decisions record who decided (process lineage)" 'grep -rq "decided_by" "$LEDGER"'
check "ledger never contains the secret" '! grep -rq sk-e2eSECRET "$LEDGER"'

echo "== web console"
("$K" console --no-open >"$T/console.txt" 2>&1 &)
wait_for 5 "grep -q http $T/console.txt"
U=$(grep -o 'http://[^ ]*' "$T/console.txt")
BASE=${U%%/?t=*}
TOK=${U##*t=}
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }
check "API needs the token" '[ "$(code "$BASE/api/snapshot")" = 401 ]'
check "API works with the token header" '[ "$(code -H "X-KMttt-Token: $TOK" "$BASE/api/snapshot")" = 200 ]'
check "foreign Host is refused (DNS rebinding)" '[ "$(code -H "Host: evil.example" -H "X-KMttt-Token: $TOK" "$BASE/api/snapshot")" != 200 ]'
check "actions are off by default" '[ "$(code -X POST -H "X-KMttt-Token: $TOK" "$BASE/api/approval/1/allow")" = 403 ]'
check "favicon served" '[ "$(code "$BASE/favicon.svg")" = 200 ]'
check "CSP header present" 'curl -sI "$BASE/?t=$TOK" | grep -qi "content-security-policy"'

echo "== persistence"
"$K" server stop >/dev/null
sleep 1
"$K" workspace list >/dev/null
snapshot_events() {
  python3 - "$KMTTT_SOCKET_DIR/default.sock" <<'PY'
import json, socket, struct, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
b = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "control.snapshot", "params": {}}).encode()
s.sendall(struct.pack(">I", len(b)) + b)
while True:
    n = struct.unpack(">I", s.recv(4, socket.MSG_WAITALL))[0]
    d = json.loads(s.recv(n, socket.MSG_WAITALL))
    if d.get("id") == 1:
        r = d["result"]; print(len(r["events"]), r["blocked_today"]); break
PY
}
check "security events survive a restart" 'wait_for 5 "[ \$(snapshot_events | cut -d\" \" -f1) -gt 0 ]"'
check "blocked-today count survives a restart" '[ "$(snapshot_events | cut -d" " -f2)" -gt 0 ]'
check "layout restored after restart" 'wait_for 5 "$K pane list | grep -q \"\\\"$B\\\"\""'

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" = 0 ]
