# Security Policy

Securidge KMttt is a security tool, so we treat bugs in it as security bugs first.

## Supported versions

Security fixes go into the latest release only. Update with the newest installer, or run `kmttt update` for the command-line tool.

## Reporting a vulnerability

Please **do not open a public issue** for a vulnerability. Use GitHub's
[private vulnerability reporting](https://github.com/securidge/securidge-kmttt/security/advisories/new)
instead. Include the version (`kmttt --version`), your OS, and the steps to reproduce.

## What is in scope

- Bypassing the filter chain (redaction → policy → ledger) so pane data leaves without passing it
- Spoofing the caller identity used by the policy engine (F1)
- Forging, editing or truncating ledger records without `kmttt audit verify` noticing (F2)
- Secrets reaching the ledger, `pane.read` replies or the web console unredacted (F3)
- Escaping a `restricted` or `container` sandbox profile beyond the documented limits (F4)
- Socket / named pipe access from another OS user, web console CSRF or DNS rebinding

## Out of scope (documented limits)

- An attacker who already has administrator/root rights on the same machine
- Data that an agent sends to its own vendor's servers
- Network egress from `restricted` panes on Windows and on Linux kernels older than 6.7 (use `container` or `wsl` there)

See [docs/DESIGN.md §6](docs/DESIGN.md#6-위협-모델) for the full threat model (in Korean).

## 한국어 요약

- **지원 버전**: 보안 수정은 최신 릴리스에만 넣습니다. 새 설치 파일로 업데이트하거나, 명령줄 도구는 `kmttt update`를 실행하세요.
- **신고**: 취약점은 공개 이슈로 올리지 말고 GitHub의 [비공개 취약점 신고](https://github.com/securidge/securidge-kmttt/security/advisories/new)를 써 주세요. 버전(`kmttt --version`), OS, 재현 방법을 적어 주세요.
- **범위**: 위 "What is in scope"·"Out of scope" 목록과 같습니다. 전체 위협 모델은 [설계서 6장](docs/DESIGN.md#6-위협-모델)에 있습니다.
