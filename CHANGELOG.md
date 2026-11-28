# Changelog

## 1.3.2 — 미출시 / Unreleased

- Codex 연동 상태 수정: Codex를 연동해도 `kmttt integration status`가 `installed: false`로 나오고, 앱 시작 시 Codex용 kmttt 스킬이 새 버전으로 갱신되지 않던 문제를 고쳤습니다 / Fixed `kmttt integration status` reporting Codex as not installed after a successful install, which also kept the Codex kmttt skill from being refreshed on app start

## 1.3.1 — 2026-11-28

공개 버전입니다. Public release.

- 터미널 멀티플렉서: 세션·워크스페이스·탭·창, 분리·재접속, 레이아웃 복원, git worktree, 플러그인, SSH 원격. 창은 제목 줄을 끌어 옮기고 경계선·단축키로 크기를 조절합니다 / Terminal multiplexer: sessions, workspaces, tabs and panes, detach and reattach, layout restore, git worktrees, plugins, SSH remotes; drag panes to move them and resize with dividers or shortcuts
- 에이전트 상태 감지: Claude Code, Codex CLI, OpenCode, Cursor CLI, Copilot CLI, Gemini CLI, Aider / Agent state detection for these agents
- 보안 통제: 정책 관문과 승인, 위변조 탐지 감사 로그, 민감정보 마스킹, 창별 격리 수준, 위험한 도구 호출을 차단하는 훅. 격리된 창에서도 에이전트 로그인·인증은 동작하고, 외부 통신 차단은 묶어서 기록합니다 / Security controls: policy gate and approvals, tamper-evident audit log, secret masking, per-pane isolation, hooks that block dangerous tool calls; agent sign-in works in isolated panes and egress blocks are grouped in the log
- 승인: [이 세션 동안 허용](Alt+Shift+Y)으로 같은 에이전트·작업·대상 창의 반복 요청을 세션 동안 자동 허용, [승인] 탭에서 해제 / Allow for this session: repeat requests with the same agent, action and target pane are allowed until a pane closes or the server stops; revoke from the Approvals tab
- 창 닫기와 트레이: X 버튼에서 [창 닫기](트레이에 남음)·[프로그램 끝내기](서버와 에이전트까지 종료) 선택, 트레이 메뉴 [KMttt 열기]·[프로그램 끝내기], 중복 실행 시 기존 창 표시 / Close button asks Close window (stays in the tray) or Quit (stops the server and agents); tray menu Open KMttt / Quit; a second launch shows the existing window
- 에이전트 스킬: Claude Code·Codex에게 KMttt 창 안에서 일하는 법(승인 한 번으로 다른 에이전트와 대화, 승인·거부·외부 통신 차단 처리)을 알려 주는 스킬. KMttt에서 연동하면 자동 설치·제거, GitHub에서 `/plugin marketplace add securidge/securidge-kmttt`로도 설치 / An agent skill for Claude Code and Codex, installed and removed with the KMttt integration or from GitHub
- 통합 관제: TUI 관제 모드, 로컬 웹 콘솔 / Control center in the TUI and a local web console
- KMttt Desktop: Windows 설치 파일, Linux `.deb`, macOS `.app` — 알림 막대에서 즉시 승인(Alt+Y/Alt+N, 남은 시간 표시), 백그라운드 알림, 토큰 사용량 패널, 잔여 한도 경고, 감사 로그 필터와 CSV 내보내기, 명령 팔레트, 초기 설정 마법사 / Desktop app with banner approvals and countdown, background notifications, a token usage panel, low-usage alerts, audit log filters and CSV export, a command palette and a setup wizard
- AI 사용량: Claude Code·Codex의 잔여 한도와 초기화까지 남은 시간, 창별 토큰 / Remaining Claude Code and Codex usage, reset times and per-pane tokens
- 퍼즈 테스트 여섯 곳(터미널 출력, 소켓 RPC, 민감정보 마스킹, 외부 통신 프록시, 도구 호출 판정, 에이전트 기록) / Fuzzing of six attacker-facing parsers
- 한국어·영어 화면과 문서 / Korean and English interface and documentation
- 서명한 해시 목록(SHA256SUMS.txt)과 `kmttt update` / A signed checksum list and `kmttt update`

자세한 내용은 [릴리스 노트](docs/releases/v1.3.1.md)를 보세요. See the [release notes](docs/releases/v1.3.1.md).
