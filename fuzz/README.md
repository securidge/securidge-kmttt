# 퍼즈 테스트 (Fuzzing)

공격자가 입력을 만들 수 있는 곳을 [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)(libFuzzer + AddressSanitizer)로 퍼징합니다.
CI(`.github/workflows/fuzz.yml`)가 main에 올릴 때와 PR에서 대상마다 2분, 매주 한 번(그리고 수동 실행 때) 20분씩 돌립니다.

| 대상 | 코드 | 입력을 만드는 쪽 | 실패로 보는 것 |
| --- | --- | --- | --- |
| `term_feed` | `kmttt-term` 터미널 출력 파서 | 창 안 에이전트의 출력 | 패닉, 질의 답에 정해진 모양이 아닌 바이트가 섞임(입력 주입) |
| `rpc_message` | `kmttt-proto` 프레임·JSON-RPC 파서 | 소켓에 연결한 프로세스 | 패닉, 프레임 상한 위반, 다시 직렬화하면 종류가 바뀜 |
| `redact` | `kmttt-redact` 비밀값 가리기 | 에이전트 출력 | 패닉, 잘못된 탐지 위치, 배운 비밀이 결과에 남음 |
| `egress_request` | `kmttt-sandbox` 외부 접속 프록시의 요청 머리 | 격리된 에이전트 | 패닉, 허용되지 않은 글자가 든 호스트를 받아들임, 목록 밖 도메인을 허용 |
| `tool_policy` | `kmttt-policy` 도구 호출 판정(훅) | 에이전트의 도구 이름·입력 | 패닉, 판정이 매번 다름, 내장 거부 명령을 공백(스페이스·탭) 변형·앞뒤 글자·배열로 감쌌을 때 허용 |
| `agent_logs` | `kmttt-core` 사용량·토큰 기록 읽기 | 상태 줄 입력, 대화 기록, Codex 세션 기록 | 패닉, 합계가 응답별 값의 합과 다름(같은 메시지를 두 번 셈), 넘침 |

## 로컬에서 돌리기

```sh
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz run -O term_feed corpus/term_feed seeds/term_feed -- -max_total_time=300
```

나이틀리가 없으면 안정 버전에 `RUSTC_BOOTSTRAP=1`을 붙여도 됩니다. 실패하면 재현 입력이 `artifacts/<대상>/`에 남고,
`cargo fuzz run <대상> <파일>`로 다시 돌려 볼 수 있습니다. 고친 뒤에는 그 입력을 `seeds/<대상>/`에 넣어 회귀 테스트로 남기세요.
