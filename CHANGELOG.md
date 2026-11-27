# Changelog

## 1.2.0 — 2026-11-28

공개 버전입니다. Public release.

- 터미널 멀티플렉서: 세션·워크스페이스·탭·창, 분리·재접속, 레이아웃 복원, git worktree, 플러그인, SSH 원격 / Terminal multiplexer: sessions, workspaces, tabs and panes, detach and reattach, layout restore, git worktrees, plugins, SSH remotes
- 에이전트 상태 감지: Claude Code, Codex CLI, OpenCode, Cursor CLI, Copilot CLI, Gemini CLI, Aider / Agent state detection for these agents
- 보안 통제: 정책 관문과 승인, 위변조 방지 기록, 비밀값 가리기, 창별 보호 수준, 위험한 도구 호출을 막는 훅 / Security controls: policy gate and approvals, tamper-evident log, secret masking, per-pane protection levels, hooks that block dangerous tool calls
- 전체 현황: TUI 관제 모드, 로컬 웹 콘솔 / Control center in the TUI and a local web console
- KMttt Desktop: Windows 설치 파일, Linux `.deb`, macOS `.app` — 노란 띠에서 바로 승인(Alt+Y/Alt+N, 남은 시간 표시), 앱이 뒤에 있을 때 Windows 알림, 창 제목 줄, 현재 토큰 사용량 패널, 남은 사용량 경고, 상태 줄의 작업 상태 개수, 보안 기록 필터와 CSV 내보내기, 시작 안내, 사이드바 접기, 처음 사용 안내·가이드·기능 찾기·설정 화면 / Desktop app with banner approvals and countdown, background notifications, pane title rows, a token usage panel, low-usage alerts, status counts, security log filters and CSV export, getting-started steps, a collapsible sidebar, first-run setup, guide, feature search and settings
- AI 사용량: Claude Code·Codex의 남은 사용량과 리셋까지 남은 시간, 창별 토큰 / Remaining Claude Code and Codex usage, reset times and per-pane tokens
- 퍼즈 테스트 여섯 곳(터미널 출력, 소켓 RPC, 비밀값 가리기, 외부 접속 프록시, 도구 호출 판정, 에이전트 기록) / Fuzzing of six attacker-facing parsers
- 한국어·영어 화면과 문서 / Korean and English interface and documentation
- 서명한 해시 목록(SHA256SUMS.txt)과 `kmttt update` / A signed checksum list and `kmttt update`

자세한 내용은 [릴리스 노트](docs/releases/v1.2.0.md)를 보세요. See the [release notes](docs/releases/v1.2.0.md).
