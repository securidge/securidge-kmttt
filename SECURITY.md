# Security Policy

Securidge KMttt is a security tool, so we treat bugs in it as security bugs first.

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

See `docs/DESIGN.md` §6 for the full threat model.
