# Securidge KMttt — Trusted Terminal Tracker 설계서

세부 결정과 그 이유는 `docs/DECISIONS.md`에 있다.

## 1. 개요

Securidge KMttt(Trusted Terminal Tracker)는 AI 코딩 에이전트용 터미널 멀티플렉서에 보안 통제 5종과
통합 관제 화면을 더한 Rust 프로그램이다. 특장점은 "여러 에이전트가 무엇을 하고, 무엇을 주고받고,
무엇이 막혔는지를 한 화면에서 보고 통제한다"이다.

목표

1. **멀티플렉서**: 세션·워크스페이스·탭·pane, 에이전트 상태 감지, 백그라운드 서버, 소켓 API·CLI,
   worktree, 플러그인, SSH 원격을 제공한다.
2. **보안 강화**: 에이전트 간 메시지 통제, 위변조 방지 기록, 비밀정보 마스킹, pane 격리 실행, 훅 로그 통합을 기본 탑재한다.
3. **통합 관제**: 로컬과 원격 머신의 모든 에이전트 상태, 보안 이벤트, 에이전트 간 메시지 흐름을
   TUI 관제 모드, 로컬 웹 콘솔, 데스크톱 앱으로 한눈에 보여준다.

설계 전제

| 항목 | 결정 |
| --- | --- |
| 언어 | Rust (stable). 서버·CLI·TUI는 단일 바이너리 `kmttt`, 창 화면은 `kmttt-desktop` |
| 플랫폼 | Windows 10/11(ConPTY), Linux, macOS |
| 라이선스 | MIT OR Apache-2.0 |
| 저장소 | github.com/securidge/securidge-kmttt |
| UI 언어 | 한국어·영어 |
| 텔레메트리 | 없음. 네트워크 기능은 사용자가 켠 것(SSH, 웹 콘솔, 업데이트 확인)만 동작 |
| 제작자 표시 | 관제 화면, 웹 콘솔, 데스크톱 앱 왼쪽 아래에 Kwangmo Kim |

범위 밖: 모바일 앱, 클라우드 릴레이.

## 2. 기능

| 영역 | 구현 |
| --- | --- |
| 계층 모델 | 세션 > 워크스페이스 > 탭 > pane. ID는 `w1`, `w1:t1`, `w1:p2` 형식이고 에이전트 이름을 별칭으로 쓴다. pane마다 보안 프로필 속성이 있다 |
| 상태 감지 | working · blocked · done · idle · unknown. 전면 프로세스를 식별한 뒤 화면 하단을 TOML 스크린 매니페스트로 매칭한다. 훅 신호가 있으면 화면 추정보다 우선한다 |
| 감지 설명 | `kmttt agent explain`과 관제 화면의 근거 표시 |
| 백그라운드 서버 | 서버가 PTY·프로세스·레이아웃을 소유하고 클라이언트는 렌더링·입력만 한다. prefix+q로 분리하고, 여러 클라이언트가 동시에 붙는다. 클라이언트 접속·종료도 기록한다 |
| 영속성 | 재시작 시 레이아웃을 복원한다. pane 기록 보존은 선택 기능이며 마스킹 후에만 저장한다 |
| 원격·SSH | 원격 직접 실행, `--remote` thin client, `machine add`. 머신별 보안 이벤트를 관제 화면으로 모은다 |
| 소켓 API·CLI | JSON-RPC 2.0과 JSON을 반환하는 CLI(pane run/read/wait-output, agent start/prompt/read/wait/list, 이벤트 구독). 모든 호출은 정책(F1)과 마스킹(F3)을 거친다 |
| worktree | `worktree create/remove`. 변경사항이 있으면 삭제를 거부한다. worktree별 격리 프로필을 지정할 수 있다 |
| 키바인딩 | prefix ctrl+b. v, -, c, n/p, w, z, q, ?, d(관제 모드), s(보안 이벤트), a(승인 대기열), 마우스 |
| 알림 | 앱 내·터미널·OS 알림. 활성 탭은 억제하되 보안 경보는 억제하지 않는다 |
| 플러그인 | `kmttt-plugin.toml`에 capability를 선언하고, 설치할 때 동의를 받으며, 선언 밖 호출은 차단한다 |
| 설정 | `config.toml` + `policy.toml`, `server reload-config`. 정책 변경은 해시와 함께 기록한다 |
| 화면 구성 | 사이드바(워크스페이스·에이전트·상태), pane 트리, 보안 배지(격리 수준, 차단 건수) |
| 운영 명령 | status, 서명 검증 후 update, 셸 자동완성, `kmttt uninstall` |
| 데스크톱 앱 | 같은 서버에 붙는 창 화면(Tauri 2 + xterm.js). 메뉴·버튼·가이드, 설정 화면, AI 사용량 표시 |

## 3. 보안 강화 기능

모든 pane 데이터는 서버 이벤트 버스의 필터 체인(마스킹 F3 → 정책 F1 → 기록 F2)을 통과한 뒤에만
화면·API·디스크로 나간다.

### F1. 에이전트 간 메시지 통제 (Policy Gate)
- 호출자 식별: 소켓 접속자의 PID를 OS에서 구한다(Unix `SO_PEERCRED`, Windows named pipe 클라이언트 PID).
  그 PID가 어느 pane의 프로세스 트리에 속하는지로 pane을 판별한다.
- 정책 판정: 호출자, 동작(`agent.prompt`, `pane.run`, `pane.read` …), 대상을 보고 allow / deny / ask.
  ask는 관제 화면의 승인 대기열로 간다.
- 기본값: 사람(pane 밖 프로세스)은 허용, 에이전트→에이전트 `prompt`·`run`은 ask, `read`는 마스킹 결과만.
- 부가: 호출자별 호출 수 제한, 프롬프트 인젝션 패턴 경보(차단 아님).

### F2. 위변조 방지 기록 (Ledger)
- pane 입출력, API 호출, 정책 판정, 상태 변화, 훅 이벤트, 클라이언트 접속, 설정·정책 변경을 기록.
- 추가 전용 JSONL, `hash = SHA-256(prev_hash ‖ canonical_json(record))`, 1분 또는 1,000건마다 Ed25519 서명 체크포인트.
- `kmttt audit verify`, `kmttt audit export`, syslog·파일 외부 반출. 기록 수준 full / metadata(기본) / off.

### F3. 비밀정보 마스킹 (Redactor)
- AWS 키, GitHub·GitLab 토큰, Anthropic·OpenAI 키, Slack 토큰, JWT, PEM 개인키, `.env` 형식, 고엔트로피 문자열.
- 적용 지점: 기록 저장 직전(항상), `pane.read` 응답(항상), 화면(선택). 원문은 저장하지 않고 종류·위치·지문만 기록.

### F4. pane 격리 실행 (Sandbox Launcher)

| 프로필 | Windows | Linux | macOS | 강도 |
| --- | --- | --- | --- | --- |
| `none` | 일반 실행 | 일반 실행 | 일반 실행 | 없음 |
| `restricted` | Job Object(프로세스 수·메모리) | Landlock(파일 접근) | `sandbox-exec` | 중간 |
| `container` | Docker/Podman | Docker/Podman | Docker/Podman | 높음 |
| `wsl` | 지정 WSL 배포판 | — | — | 중간 |

worktree만 쓰기 허용이 기본. 네트워크는 내장 이그레스 프록시의 도메인 허용 목록을 쓴다(`restricted`는
Linux 6.7+와 macOS에서 강제, `container`는 `sandbox.container_network` 설정에 따름). 관제 화면에 실제 강도를 그대로 표시한다.

### F5. 훅 로그 통합 (Hook Ingest)
- `kmttt integration install <agent>`가 에이전트 훅 설정에 KMttt 전송 명령을 등록(원래 설정은 백업).
- 도구 호출 이벤트를 공통 스키마로 정규화해 기록·표시. 실행 전 차단을 지원하는 에이전트는 KMttt 정책으로 차단.
- 훅 미지원 에이전트는 "관찰 전용"으로 표시.

### 기본 하드닝
- 소켓 0600 / named pipe DACL(현재 사용자만), TCP 미개방.
- 웹 콘솔: 127.0.0.1, 랜덤 토큰, Host·Origin 검사, 엄격한 CSP, 읽기 전용 기본.
- 플러그인: capability 선언·설치 동의·정책 엔진 경유.
- 릴리스: minisign 서명, `kmttt update`는 검증 후 교체, CI에서 `cargo deny`.

## 4. 통합 관제 대시보드

정렬 순서: 승인 대기 → 보안 경보 → blocked → done → working.
TUI 관제 모드(prefix + d)와 로컬 웹 콘솔(`kmttt console`)이 같은 데이터를 보여준다.

| 패널 | 내용 |
| --- | --- |
| 에이전트 그리드 | 상태, 머신, 워크스페이스, 격리 프로필, 상태 유지 시간, 마지막 출력 한 줄(마스킹) |
| 승인 대기열 | F1 ask, F5 위험 도구 호출 — 허용 / 거부 |
| 보안 이벤트 | 정책 차단, 비밀정보 탐지, 인젝션 의심, 격리 위반, 플러그인 권한 초과 (정보·주의·위험) |
| 메시지 흐름 | 최근 N분 에이전트 간 prompt·run·read, 차단은 빨간색 |
| 도구 호출 | F5로 들어온 도구 호출 타임라인 |
| 자원 | pane별 CPU·메모리, 자식 프로세스 수 |
| 무결성 | 감사 기록 체인 검증 결과, 마지막 체크포인트 |
| 머신 | 로컬 + 원격 연결 상태와 경보 건수 |

## 5. 아키텍처

클라이언트(TUI, 데스크톱 앱, CLI·플러그인, 웹 콘솔, 에이전트 훅) → 로컬 IPC(Unix 소켓 0600 / named pipe) →
서버(API 게이트웨이·호출자 식별 → 정책 엔진, 이벤트 버스 + 필터 체인 → 세션 관리자 / 상태 감지 /
PTY 호스트·격리 / 감사 기록) → 에이전트 pane, 원격 머신(SSH), 외부 반출.

| 크레이트 | 역할 |
| --- | --- |
| `kmttt` | 단일 바이너리, CLI, 서버·클라이언트 진입점 |
| `kmttt-proto` | JSON-RPC 2.0 메서드·이벤트 스키마, 프레임, OpenRPC |
| `kmttt-pty` | 플랫폼별 PTY(ConPTY 포함), 프로세스 트리 |
| `kmttt-term` | VT 파서, 화면 버퍼, 스크롤백 |
| `kmttt-core` | 세션 모델, 레이아웃, 설정, Filter 트레이트 |
| `kmttt-detect` | 에이전트 식별, 스크린 매니페스트 |
| `kmttt-policy` | 정책 판정, 호출 수 제한, 인젝션 경보, 도구 정책 |
| `kmttt-redact` | 비밀정보 탐지·치환 |
| `kmttt-ledger` | 해시 체인 기록, 서명 체크포인트, 검증·반출 |
| `kmttt-sandbox` | 격리 프로필, 이그레스 프록시 |
| `kmttt-server` | 데몬, 소켓 서버, 훅 수신, 플러그인 실행 |
| `kmttt-tui` | ratatui 클라이언트, 사이드바, 관제 모드 |
| `kmttt-web` | axum 로컬 웹 콘솔 |
| `desktop/` (`kmttt-desktop`) | Tauri 2 창 앱, xterm.js 터미널, 설정 화면 |

저장 위치: Windows `%APPDATA%\Securidge\kmttt\`(설정), `%LOCALAPPDATA%\Securidge\kmttt\`(기록);
Linux·macOS `~/.config/kmttt/`, `~/.local/share/kmttt/`.

## 6. 위협 모델

| 위협 | 대응 | 범위 밖 |
| --- | --- | --- |
| 프롬프트 인젝션 전파 | F1 호출자 식별 + ask 기본값 + 패턴 경보 | 사람이 승인 요청을 읽지 않고 누르면 못 막음 |
| 비밀정보 유출 | F3 (기록·read는 항상) | 규칙에 없는 형식은 못 잡음, 에이전트→업체 서버는 통제 밖 |
| 파괴적 명령 | F5 훅 차단 + F4 격리 | 훅 미지원 에이전트는 격리에만 의존 |
| 기록 사후 변조 | F2 해시 체인 + 서명 + 외부 반출 | 외부 반출 없이 통째로 지우는 것은 탐지만 가능 |
| 소켓 탈취 | 0600 / DACL | 같은 사용자 권한 악성코드는 구분 불가 |
| 악성 플러그인 | capability + 동의 + 정책 경유 | 플러그인 프로세스 자체는 사용자 권한 |
| 웹 콘솔 공격 | 로컬 바인딩, 토큰, Host·Origin 검사 | 토큰 URL 공유 시 노출 |
| 변조된 업데이트 | minisign 검증 | 서명 키 유출 시 무력화 |

화면 기반 상태 감지는 매니페스트 규칙을 따르므로 에이전트 UI가 바뀌면 매니페스트를 갱신한다.
KMttt 자체가 고가치 표적이므로 소켓·설정·기록을 현재 사용자만 접근하게 두고, 믿지 못하는 에이전트는 격리 프로필로 돌린다.
