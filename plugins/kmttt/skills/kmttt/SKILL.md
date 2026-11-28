---
name: kmttt
description: Use when you run inside a Securidge KMttt terminal pane (the KMTTT_PANE environment variable is set) and need to message, wait for or read another AI agent's pane, or when a kmttt command is denied, waits for approval, or a network connection is blocked by the KMttt sandbox. Not for ordinary terminal work.
---

# Working inside Securidge KMttt

KMttt runs each AI agent in its own terminal pane and puts a policy gate between panes. A person approves cross-pane actions. Use the `kmttt` command-line tool for everything below, and always go through the normal, approved path.

## 1. Check where you are

- Inside KMttt, `KMTTT_PANE` (your pane id, for example `w2:p1`) and `KMTTT_SESSION` are set. Bash: `echo "$KMTTT_PANE"`. PowerShell: `$env:KMTTT_PANE`. If it is empty, you are not in KMttt and this skill does not apply.
- Find the tool: try `kmttt --version`. If it is not on PATH, use the copy installed with KMttt Desktop:
  - Windows: `%LOCALAPPDATA%\KMttt Desktop\kmttt.exe` (Git Bash: `"$LOCALAPPDATA/KMttt Desktop/kmttt.exe"`)
  - Otherwise: the `kmttt` binary next to the KMttt Desktop executable.
- Always pass your session so you talk to the right server: `kmttt -s "$KMTTT_SESSION" ...` (PowerShell: `-s $env:KMTTT_SESSION`).

The examples below write `K` for `kmttt -s "$KMTTT_SESSION"`.

## 2. Talk to another agent

```
K agent list                                  # names, kinds, panes and states
K agent prompt <name-or-pane> "message"       # types the message and presses Enter once
K agent prompt <name-or-pane> "message" --wait  # ...and waits until the agent stops working
K agent wait <name-or-pane> --until idle,done,blocked --timeout 600000
K agent read <name-or-pane> --lines 80        # last lines of its screen (secrets masked)
```

- Use `agent prompt` for messages. Do not send text with `pane send` and then a separate Enter: that is two actions, so the person has to approve twice, and the Enter can land in the wrong place.
- A typical exchange is `agent prompt ... --wait`, then `agent read ...`. Send one message per step and keep it short. Do not loop sending messages.
- If the target is waiting for an answer (state `blocked`, for example a menu or a yes/no question), `agent prompt` refuses. Read its screen and tell the person what it is asking. Answer with `K pane send <pane> "<keys>"` only when the person asked you to.

## 3. Approvals

Messaging, typing into, running commands in or reading another pane may need a person's approval. Your command then waits (120 seconds by default) while KMttt shows the request.

- Tell the person in your reply what you are waiting for, for example: "Approve in KMttt: Alt+Y, or Alt+Shift+Y to allow this for the session."
- "Allow for this session" covers only the same agent, action and target pane, until a pane closes or the server stops. A different action or target asks again. That is expected.
- Exit codes: `0` ok, `2` target not found or bad input, `3` denied (by the person, the policy or a rate limit), `4` timed out.
- On exit code `3`, stop. Do not retry in a loop. Do not reach the same goal another way (`pane run`, raw keystrokes, scripts, a different pane). Tell the person what was denied and let them decide.

## 4. Network blocks in isolated panes

In an isolated (restricted) pane, outbound connections go through an allow list. A blocked host returns HTTP 403 with "not in the sandbox allow list".

- Tell the person the host name and why you need it. They can add it from the KMttt audit log with [Allow address], or in Settings → Isolation.
- Do not try other hosts, mirrors or proxies to get around the block.
- AI sign-in and model addresses (Anthropic, Claude, OpenAI, ChatGPT) are allowed by default. If sign-in still fails, report the exact host that was blocked.

## 5. Never do these

- Never run `kmttt approval allow`, `approval deny` or `approval revoke` yourself. Only a person can decide approvals. KMttt denies these calls from agents and records the attempt.
- Never edit KMttt's `policy.toml` or `config.toml`, turn off isolation, or unset `KMTTT_PANE` / start detached processes to look like a person. KMttt traces the process lineage and records it.
- Never paste secrets into another pane. KMttt masks them in reads and logs, but the target agent still receives what you type.
