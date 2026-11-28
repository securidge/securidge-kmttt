# Changelog

## 1.3.4 — 미출시 / Unreleased

- 승인 알림 막대·Windows 알림·승인 상세·감사 로그에서 요청한 창이 `w2:p6(codex)` 같은 날 글자 대신 창 이름으로 보이고, 식별하지 못한 창은 그렇다고 표시합니다. 감사 로그를 창으로 걸러 볼 때 이름이 붙은 에이전트의 기록이 빠지던 문제도 고쳤습니다 / The approval banner, Windows notification, approval details and audit log now name the requesting pane instead of a raw label like `w2:p6(codex)` and say when it could not be identified; filtering the audit log by pane no longer drops records from named agents
- 문서 전수 검수 반영: 메뉴 이름, 제거 시 Codex `notify` 복원 안내, Codex 연동 방법, 영어판 용어·문법과 빠진 내용, 한국어 조사를 바로잡고, SECURITY.md에 지원 버전과 한국어 요약을 넣었습니다 / Docs audit: fixed menu names, the uninstall note on restoring Codex `notify`, how to connect Codex, English terms, grammar and missing lines, and Korean particles; SECURITY.md now lists supported versions and has a Korean summary
- 문서: GitHub에서 깨진 링크로 보이던 버튼·단축키 표기를 고치고, README 라이선스 절에 두 라이선스 파일 링크를 넣었습니다. CI가 공개 문서의 빈 절·깨진 링크·한영 제목 짝을 검사합니다 / Docs: fixed button and shortcut notes that GitHub rendered as broken links, linked both license files from the README, and CI now checks public docs for empty sections, broken links and Korean/English heading parity
- KMttt X 버튼의 [창 닫기]를 [트레이로 숨기기]로 바꿔, 터미널 창을 닫는 [창 닫기]와 헷갈리지 않게 했습니다. 동작은 같습니다 / The KMttt window's "Close window" choice is now "Hide to tray", so it is not confused with closing a pane; behavior is unchanged
- 승인 버튼 [허용]을 [이번만 허용]으로 바꾸고, 승인 상세 창에 [이 세션 동안 허용]이 무엇을 언제까지 허용하는지, 또는 왜 쓸 수 없는지 한 줄로 보여 줍니다 / The approval button now reads "Allow once", and the approval details say exactly what "Allow for this session" covers and until when, or why it is unavailable
- 탭을 닫을 때도 확인 창을 띄웁니다. Windows에서는 서버 종료와 프로그램 끝내기 확인 문구에도, 창에서 시작한 프로그램과 처음 연 앱이 함께 닫힌다고 안내합니다 / Closing a tab now asks for confirmation, and on Windows the stop-server and quit confirmations also say that programs started from panes, including apps first opened from them, close too
- Windows에서 창이나 워크스페이스를 닫을 때, 그 창에서 시작한 프로그램과 처음 연 앱(VS Code, 로그인 브라우저 등)도 함께 닫힌다고 확인 창에 안내합니다 / On Windows, the close confirmation for a pane or workspace now says that programs started from it, including apps first opened from it, close too
- Codex notify 연결은 WSL·콘솔 호스트·원격 셸 실행 파일을 거부하고 실행 결과를 감사 로그에 남깁니다(서버가 없으면 생략). / Codex notify chaining rejects shell forwarding hosts and audits execution outcomes when a server is available.
- Codex notify 연결의 감사 기록은 호출자별로 분당 `rate_per_min`건까지 남기고, 넘는 시작·완료 기록은 건수와 기간을 담은 요약 한 줄로 합칩니다. 실패·시간 초과는 따로 분당 20건까지 개별로 남기고, 넘는 것도 요약에 건수로 셉니다. / Codex notify chain audit records are limited to `rate_per_min` per caller per minute; extra start/completion records are merged into one summary with counts and time range. Failures and timeouts are recorded individually up to a separate 20 per minute, and any beyond that are counted in the summary.

## 1.3.3 — 2026-11-28

- Windows E2E 점검은 KMttt 창 안에서 실행하면 안내 후 중단하며, 로컬 리다이렉트 핸들 누수 검사를 `cmd /c`와 따옴표로 감싼 경로로 수행합니다 / Windows E2E checks refuse to run inside KMttt and use `cmd /c` with quoted paths for the local redirect handle leak check
- Windows E2E 점검을 로컬에서 임시 세션·폴더로 분리하고 종료 시 정리합니다. 로컬 `default` 세션 지정은 `-Force` 없이는 거부합니다 / Local Windows E2E checks use temporary sessions and directories with cleanup; selecting `default` locally requires `-Force`
- Codex 연동의 `--chain`은 명시적으로 등록한 기존 네이티브 notify를 셸 해석 없이 KMttt 보고 뒤에 호출합니다(3초 제한). `--replace`와 연동 해제 시 보관한 설정을 복원하는 동작은 그대로 유지합니다 / Codex integration supports opt-in `--chain` to call the registered native notifier after KMttt without shell interpretation, with a three-second limit; `--replace` and uninstall restoration remain supported
- 이미 결정된 승인 ID를 다시 결정하면 오류로 거부해 세션 허용이 뒤늦게 추가되지 않도록 했습니다 / Repeated decisions for an already decided approval now return an error without creating a session grant
- Codex 연동에 `--replace`를 쓰면 기존 notify를 보관하고 해제 시 복원합니다. 설치 출력에 기존 notify와 관련 기능이 일시 중단됨을 안내합니다 / Codex integration can save an existing notify with `--replace` and restore it on uninstall; installation warns that the previous notifier and related features are temporarily disabled
- 세션 허용이 불가능한 승인 요청에서 세션 허용을 선택하면 실행을 허용하지 않고 대기 상태를 유지해, 한 번 허용이나 거부로 다시 결정할 수 있습니다 / Invalid session approval decisions leave the request pending without allowing execution, so it can be retried as a single allow or deny

자세한 내용은 [릴리스 노트](docs/releases/v1.3.3.md)를 보세요. See the [release notes](docs/releases/v1.3.3.md).

## 1.3.2 — 2026-11-28

- Windows 에이전트 점검 스크립트에서 `.exe`를 우선하고 npm의 `.cmd`로 대체하며, 가이드에 Codex 직접 실행 시 `.ps1` 차단 대처와 창 종료 시 GUI 앱 동반 종료를 안내합니다 / Windows agent checks prefer `.exe` then npm `.cmd`; the guides explain Codex `.ps1` policy errors and GUI apps closing with their terminal pane
- Windows에서 창을 닫거나 서버가 종료되면 분리된 자손 프로세스도 함께 종료됩니다 / Windows pane descendants, including detached processes, now exit when their pane closes or the server exits
- 보안: 식별하지 못한 호출자(unknown)에게는 [이 세션 동안 허용]을 적용하지 않습니다. 이전에는 이런 호출자들이 같은 이름으로 묶여, 한 에이전트에 준 세션 허용이 다른 창 에이전트의 요청까지 자동 허용할 수 있었습니다 / Security: "Allow for this session" no longer applies to unidentified callers, which shared one grant and could let a grant given to one agent approve another pane's requests
- Codex 연동 상태 수정: Codex를 연동해도 `kmttt integration status`가 `installed: false`로 나오고, 앱 시작 시 Codex용 kmttt 스킬이 새 버전으로 갱신되지 않던 문제를 고쳤습니다 / Fixed `kmttt integration status` reporting Codex as not installed after a successful install, which also kept the Codex kmttt skill from being refreshed on app start

자세한 내용은 [릴리스 노트](docs/releases/v1.3.2.md)를 보세요. See the [release notes](docs/releases/v1.3.2.md).

## 1.3.1 — 2026-11-28

공개 버전입니다. Public release.

- 터미널 멀티플렉서: 세션·워크스페이스·탭·창, 분리·재접속, 레이아웃 복원, git worktree, 플러그인, SSH 원격. 창은 제목 줄을 끌어 옮기고 경계선·단축키로 크기를 조절합니다 / Terminal multiplexer: sessions, workspaces, tabs and panes, detach and reattach, layout restore, git worktrees, plugins, SSH remotes; drag panes to move them and resize with dividers or shortcuts
- 에이전트 상태 감지: Claude Code, Codex CLI, OpenCode, Cursor CLI, Copilot CLI, Gemini CLI, Aider / Agent state detection for these agents
- 보안 통제: 정책 관문과 승인, 위변조 탐지 감사 로그, 민감정보 마스킹, 창별 격리 수준, 위험한 도구 호출을 차단하는 훅. 격리된 창에서도 에이전트 로그인·인증은 동작하고, 외부 통신 차단은 묶어서 기록합니다 / Security controls: policy gate and approvals, tamper-evident audit log, secret masking, per-pane isolation, hooks that block dangerous tool calls; agent sign-in works in isolated panes and egress blocks are grouped in the log
- 승인: [이 세션 동안 허용] (Alt+Shift+Y)으로 같은 에이전트·작업·대상 창의 반복 요청을 세션 동안 자동 허용, [승인] 탭에서 해제 / Allow for this session: repeat requests with the same agent, action and target pane are allowed until a pane closes or the server stops; revoke from the Approvals tab
- 창 닫기와 트레이: X 버튼에서 [창 닫기](트레이에 남음)·[프로그램 끝내기](서버와 에이전트까지 종료) 선택, 트레이 메뉴 [KMttt 열기]·[프로그램 끝내기], 중복 실행 시 기존 창 표시 / Close button asks Close window (stays in the tray) or Quit (stops the server and agents); tray menu Open KMttt / Quit; a second launch shows the existing window
- 에이전트 스킬: Claude Code·Codex에게 KMttt 창 안에서 일하는 법(승인 한 번으로 다른 에이전트와 대화, 승인·거부·외부 통신 차단 처리)을 알려 주는 스킬. KMttt에서 연동하면 자동 설치·제거, GitHub에서 `/plugin marketplace add securidge/securidge-kmttt`로도 설치 / An agent skill for Claude Code and Codex, installed and removed with the KMttt integration or from GitHub
- 통합 관제: TUI 관제 모드, 로컬 웹 콘솔 / Control center in the TUI and a local web console
- KMttt Desktop: Windows 설치 파일, Linux `.deb`, macOS `.app` — 알림 막대에서 즉시 승인(Alt+Y/Alt+N, 남은 시간 표시), 백그라운드 알림, 토큰 사용량 패널, 잔여 한도 경고, 감사 로그 필터와 CSV 내보내기, 명령 팔레트, 초기 설정 마법사 / Desktop app with banner approvals and countdown, background notifications, a token usage panel, low-usage alerts, audit log filters and CSV export, a command palette and a setup wizard
- AI 사용량: Claude Code·Codex의 잔여 한도와 초기화까지 남은 시간, 창별 토큰 / Remaining Claude Code and Codex usage, reset times and per-pane tokens
- 퍼즈 테스트 여섯 곳(터미널 출력, 소켓 RPC, 민감정보 마스킹, 외부 통신 프록시, 도구 호출 판정, 에이전트 기록) / Fuzzing of six attacker-facing parsers
- 한국어·영어 화면과 문서 / Korean and English interface and documentation
- 서명한 해시 목록(SHA256SUMS.txt)과 `kmttt update` / A signed checksum list and `kmttt update`

자세한 내용은 [릴리스 노트](docs/releases/v1.3.1.md)를 보세요. See the [release notes](docs/releases/v1.3.1.md).
