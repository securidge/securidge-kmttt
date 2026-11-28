# Changelog

## 1.2.2 — 2026-11-28

공개 버전입니다. Public release.

- 터미널 멀티플렉서: 세션·워크스페이스·탭·창, 분리·재접속, 레이아웃 복원, git worktree, 플러그인, SSH 원격. 창은 제목 줄을 끌어 옮기고 경계선·단축키로 크기를 조절합니다 / Terminal multiplexer: sessions, workspaces, tabs and panes, detach and reattach, layout restore, git worktrees, plugins, SSH remotes; drag panes to move them and resize with dividers or shortcuts
- 에이전트 상태 감지: Claude Code, Codex CLI, OpenCode, Cursor CLI, Copilot CLI, Gemini CLI, Aider / Agent state detection for these agents
- 보안 통제: 정책 관문과 승인, 위변조 탐지 감사 로그, 민감정보 마스킹, 창별 격리 수준, 위험한 도구 호출을 차단하는 훅. 격리된 창에서도 에이전트 로그인·인증은 동작하고, 외부 통신 차단은 묶어서 기록합니다 / Security controls: policy gate and approvals, tamper-evident audit log, secret masking, per-pane isolation, hooks that block dangerous tool calls; agent sign-in works in isolated panes and egress blocks are grouped in the log
- 통합 관제: TUI 관제 모드, 로컬 웹 콘솔 / Control center in the TUI and a local web console
- KMttt Desktop: Windows 설치 파일, Linux `.deb`, macOS `.app` — 알림 막대에서 즉시 승인(Alt+Y/Alt+N, 남은 시간 표시), 백그라운드 알림, 토큰 사용량 패널, 잔여 한도 경고, 감사 로그 필터와 CSV 내보내기, 명령 팔레트, 초기 설정 마법사 / Desktop app with banner approvals and countdown, background notifications, a token usage panel, low-usage alerts, audit log filters and CSV export, a command palette and a setup wizard
- AI 사용량: Claude Code·Codex의 잔여 한도와 초기화까지 남은 시간, 창별 토큰 / Remaining Claude Code and Codex usage, reset times and per-pane tokens
- 퍼즈 테스트 여섯 곳(터미널 출력, 소켓 RPC, 민감정보 마스킹, 외부 통신 프록시, 도구 호출 판정, 에이전트 기록) / Fuzzing of six attacker-facing parsers
- 한국어·영어 화면과 문서 / Korean and English interface and documentation
- 서명한 해시 목록(SHA256SUMS.txt)과 `kmttt update` / A signed checksum list and `kmttt update`

자세한 내용은 [릴리스 노트](docs/releases/v1.2.2.md)를 보세요. See the [release notes](docs/releases/v1.2.2.md).
