# 보안 점검 요약 — KMttt Desktop

> 프로젝트가 수행한 보안 점검의 결과입니다.
> English summary at the end.

## 범위

- 데스크톱 앱(`desktop/`): Tauri 2 Rust 명령, 웹뷰 화면(`desktop/ui`), 권한 설정(capabilities·CSP), 설치 프로그램 훅(`desktop/windows/hooks.nsh`)
- AI 사용량 읽기(`kmttt-core::usage`), `kmttt statusline`, `claude-usage` 통합, 제거 시 정리
- 서버의 보안 동작(승인·기록·마스킹·격리·훅)은 `ci/e2e.sh`(Linux·macOS), `ci/e2e-windows.ps1`(Windows)가 CI에서 계속 확인합니다.

## 보안 동작

| 영역 | 동작 |
| --- | --- |
| 웹뷰 | 앱에 들어 있는 화면 파일만 읽음(CSP `script-src 'self'`, 원격 스크립트·글꼴 없음, 배포 빌드에서 devtools 끔) |
| 서버 호출 | 허용 목록(`ALLOWED`)에 있는 메서드만. `pane.run`·`pane.send`·`pane.read`·`hook.*`·`events.subscribe`는 열지 않음(단위 테스트로 고정) |
| 화면 출력 | 서버에서 온 이름·제목은 글자로만 넣음(`innerHTML` 금지). 작업 공간 이름·창 제목에 HTML을 넣어도 실행되지 않음 |
| 터미널 입력 | 터미널 질의 응답은 서버만 함. 화면 쪽 응답을 막아, 출력만으로 셸에 글자가 입력되는 경로를 없앰 |
| 연결 | 사람이 직접 띄운 앱만 연결됨. pane 안이나 자동화 도구로 띄운 앱은 서버가 `client.attach`를 거부하고, 앱은 이유를 보여 줌 |
| 승인 | 실제 마우스·키보드 이벤트(`isTrusted`)만 받음. 결정은 기록에 남음 |
| 링크 | 주소 전체를 보여 주고 확인을 받은 뒤 `http`·`https`만 엶 |
| 설정 저장 | 서버와 같은 검사를 통과해야 씀, 원래 파일은 `.bak`, 서버가 거부하면 되돌림, 보안이 약해지는 변경은 다시 확인 |
| 사용량 | 로그인 정보를 읽지 않고 숫자만 저장. 압축 해제 32MB, 일반 기록은 끝 4MB만 읽음 |
| 창별 토큰 | 상태 줄은 창 ↔ 대화 기록 경로만 저장(대화 내용은 저장 안 함). 앱은 `.jsonl`로 끝나는 보통 파일만 열고(장치·파이프 제외), 처음 32MB·한 번에 8MB까지 이어서 읽으며, 화면에는 숫자만 넘김 |
| 화면 다시 그리기 | `pane.redraw`는 이 연결이 붙어 있는(attach한) 화면 클라이언트에게만 창 화면을 다시 보냄. 정책 분류는 `client.viewport`와 같은 관리 동작이라, 에이전트가 호출하면 정책을 따름 |
| 운영체제 알림 | 앱 창이 앞에 있지 않을 때만, Rust 명령으로 띄움(웹뷰에 알림 플러그인 권한 없음). 알림에는 승인 버튼이 없음 |
| 제거 | 이 설치본이 등록한 훅·상태 줄만 지우고 사용자의 원래 상태 줄을 되돌림. 설정·보안 기록은 묻고 지움(기본: 남김) |

## 설계상 알아 둘 점

| 내용 | 이유·대응 |
| --- | --- |
| `none` 프로필에서 같은 OS 사용자로 도는 에이전트가 OS 자동화로 [허용]을 누르는 것은 보호 범위 밖 | 위협 모델(결정 기록 D20). 믿지 못하는 에이전트는 `container`·`wsl`로 |
| Tauri `freezePrototype` 꺼 둠 | 켜면 xterm.js가 시작하지 못함. 웹뷰에 외부 스크립트가 없어 프로토타입 오염 입력 경로가 없음 |
| CSP `style-src`에 `'unsafe-inline'` | xterm.js가 스타일 요소를 만듦. 스크립트는 `'self'`만 |
| 사용량 파일·Codex 기록·창 ↔ 기록 짝 파일은 같은 사용자가 고칠 수 있음 | 표시용이며 보안 판단에 쓰지 않음. 짝 파일이 엉뚱한 경로를 가리켜도 숫자만 읽고 크기 상한이 있음 |
| 웹뷰가 앱 명령으로 클립보드를 읽을 수 있음 | 붙여넣기 기능. 웹뷰에서 도는 코드는 앱에 들어 있는 것뿐 |
| 릴리스 파일 확인 | 모든 릴리스 파일의 해시가 든 `SHA256SUMS.txt`에 minisign 서명이 있어, 서명과 해시로 원본 여부를 확인할 수 있음. `kmttt update`도 같은 검사를 통과해야 설치함. Windows 코드 서명은 [코드 서명 정책](../README.ko.md#코드-서명-정책-code-signing-policy) 참고 |

## 퍼즈 테스트

공격자가 입력을 만들 수 있는 여섯 곳(터미널 출력, 소켓 RPC, 비밀값 가리기, 외부 접속 프록시 요청, 도구 호출 판정, 에이전트 기록 읽기)을 cargo-fuzz(libFuzzer + AddressSanitizer)로 퍼징합니다. 패닉뿐 아니라 보안 속성 위반(질의 답에 섞인 임의 바이트, 결과에 남은 비밀값, 목록 밖 도메인 허용, 위험 명령 허용)도 실패로 봅니다. CI가 코드가 바뀔 때마다 짧게, 매주 한 번 길게 돌리며, 대상과 실행 방법은 [`fuzz/README.md`](../fuzz/README.md)에 있습니다.

퍼즈 테스트 작업으로 찾아 고친 문제:

| 문제 | 고친 방법 |
| --- | --- |
| 출력에서 배운 비밀이 상한(2,000개)에 닿으면 가장 긴 값부터 지워져, 에이전트가 문자열을 대량으로 출력하면 환경 변수의 비밀이 가려지지 않을 수 있었음 | 환경 변수에서 배운 값은 고정(결정 기록 D9a) |
| 도구 호출 규칙을 정규 JSON 글자에만 맞춰 봐서, `rm<탭>-rf /`처럼 공백 대신 탭을 쓴 위험 명령이 내장 거부 규칙을 피함(셸에서는 같은 명령) | 문자열 값을 이스케이프를 푼 그대로 함께 맞춰 봄(결정 기록 D30) |
| 외부 접속 프록시가 호스트 이름의 글자를 검사하지 않아, 허용 목록 비교와 실제 접속이 다른 이름을 볼 여지가 있었음 | 영문·숫자·`.`·`-`·`_`(IPv6는 16진수·`:`)만 받음(결정 기록 D31) |
| 기록에 아주 큰 토큰 값이 오면 합계가 넘쳐 화면 코드가 멈출 수 있었음 | 넘치지 않게 더함 |

## 의존성(`cargo audit`)

서버·CLI는 알려진 취약점 0건입니다. 데스크톱은 취약점 0건, 알림 6건이며 모두 Windows 빌드에는 들어가지 않습니다.

| 크레이트 | 알림 | 들어오는 경로 | Windows 빌드 |
| --- | --- | --- | --- |
| `glib` 0.18.5 | RUSTSEC-2024-0429 (`VariantStrIter` 반복자 unsoundness) | tauri → muda → gtk (Linux GTK3) | 포함 안 됨 |
| `proc-macro-error` 1.0.4 | RUSTSEC-2024-0370 (관리 중단) | glib-macros (빌드 시점 매크로) | 포함 안 됨 |
| `zvariant`·`zbus_names`·`zvariant_derive` | RUSTSEC-2026-0370·0371·0372 (관리 중단) | tauri-plugin-opener → zbus (Linux 전용) | 포함 안 됨 |
| `chrono` 0.4.45 | RUSTSEC-2026-0543 (관리 중단) | serde_with의 선택 기능으로 Cargo.lock에만 있음 | 빌드되지 않음 |

---

## English summary

Security review of KMttt Desktop by the project. The webview loads only bundled files and can call only an allow-listed set of server methods; server-provided names are rendered as text; only the server answers terminal queries; an app started inside a pane or by an automation tool is refused by the server; approvals require real input events; links show the full URL and open only http(s); settings are validated with the server's parsers and rolled back on rejection; uninstall removes only this copy's hooks and status line. Six places an attacker can feed (terminal output, socket RPC, secret masking, egress proxy requests, tool-call policy, agent logs) are fuzzed with cargo-fuzz on every change and weekly. This work found and fixed: flooding output evicting secrets learned from the environment; dangerous commands with tabs instead of spaces slipping past the built-in tool rules; egress host names not being checked for odd characters; and token totals that could overflow. Design notes: same-user OS automation clicking Allow under the `none` profile is outside the threat model (use `container`/`wsl`), `freezePrototype` is off, `style-src 'unsafe-inline'`, usage files are display-only. `cargo audit`: no vulnerabilities; six advisories, all in Linux-only dependencies.
