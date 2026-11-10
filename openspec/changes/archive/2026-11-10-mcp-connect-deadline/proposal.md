# Proposal: mcp-connect-deadline

## Why

Connect is the last unbounded phase of the MCP Producer. Mission 567
(`rc-kpf3m`, change `mcp-request-deadline`) bounded post-connect requests
(`request_timeout_secs`, default 120 s) and left the startup budgets as
they were: 5 s DNS resolution, 5 s TCP connect (`dns_pin.rs`). But
`RmcpClient::connect` then awaits rmcp's discover lifecycle
(`serve_with_lifecycle` with `ClientLifecycleMode::Discover`) with no
deadline at all. rmcp's own 10 s `auto_discover_timeout` applies only to
Auto-mode probing — Discover mode has no such knob. A remote that accepts
the TCP connection and never answers the `server/discover` POST hangs
`connect` — and with it route start — indefinitely. This is rc-9kpye, the
residual 567 filed explicitly out of its own scope.

## What Changes

- `McpRemoteConfig` gains `connect_timeout_secs` (u64, whole seconds,
  minimum 1, default **15**), a connect-specific option in the exact style
  of `request_timeout_secs` (serde default + custom deserializer; zero and
  non-numeric values rejected at parse time naming the option). It is a
  separate knob, not an extension of `request_timeout_secs`: that option's
  documented identity is the *post-connect* request deadline (default 120 s
  — far too loose to rescue route start), and reusing it would couple two
  phases with different budgets into one value.
- `RmcpClient::connect` wraps the `serve_with_lifecycle` future (the
  discover lifecycle: TCP connect inside rmcp's transport plus the
  `server/discover` POST) in `tokio::time::timeout(connect_timeout_secs)`.
  This bounds **the caller**: `connect` returns the named error at the
  deadline, so route start can no longer hang. The DNS-pinning resolve
  keeps its own 5 s bound ahead of it (not covered by this deadline). No
  rmcp fork, no new dependency.
- Residual, stated up front and filed upstream as rc-s4xod: rmcp 3.9.0
  awaits the bootstrap POST directly (`streamable_http_client.rs:1183`),
  before its worker's cancellation-select loop — so dropping the timed-out
  future cannot reclaim the parked transport worker or its socket in the
  wedge shape; one parked task + one socket per wedged connect attempt
  persists until the remote answers or TCP dies. Producer start has no
  retry loop, so growth is operator-bounded. Route start itself is bounded
  either way; worker/socket reclamation is rmcp's to fix upstream.
- Default 15 s, from evidence: rmcp gives the identical discover handshake
  a 10 s budget in Auto mode (`DEFAULT_AUTO_DISCOVER_TIMEOUT`, covering
  TCP + POST together); 15 s keeps that full budget available even when a
  healthy remote legitimately consumed our 5 s TCP budget first — policy
  margin above a proven upstream number, not a new guess.
- Timeout error names the phase and the bound:
  `connect to remote '{server}' did not complete the discover handshake
  within {n}s`. The 5 s TCP/DNS error texts stay exactly as 567 made them.
- `docs/src/components/mcp.md` timeout section documents the connect bound,
  its relation to the 5 s DNS/TCP budgets, and the reclamation residual.
- Spec delta on `openspec/specs/mcp-component/` (the capability 567
  touched).

Behaviour change, deliberate: connect is bounded by default (15 s) where
today it can hang forever. Worst-case startup addition is DNS (≤5 s) +
connect deadline (≤15 s) ≈ 20 s — previously unbounded.

Out of scope: the 5 s DNS/TCP budgets, the post-connect
`request_timeout_secs` behaviour, and every other component.

## Acceptance criteria

- A fake that accepts TCP and never answers `server/discover` fails
  `connect` with the named error at a short configured bound.
- A healthy remote connects unaffected under the same bound, and a
  post-connect call that outlasts the connect bound still succeeds under
  `request_timeout_secs` — the connect bound must not become a global
  request timeout.
- `connect_timeout_secs` absent → the parsed default is 15; `0` and `""`
  fail config parse naming the option; an explicit value is used.
- `docs/src/components/mcp.md` states the connect bound, its relation to
  the 5 s budgets, and the reclamation residual (rc-s4xod).
- `cargo test -p camel-component-mcp` green; `scripts/gates-local.sh`
  exit 0.

## Risk budget

- 15 s default adds a worst-case ~20 s startup delay (DNS + connect) for a
  wedged remote where today the delay is infinite; a genuinely slow-but-
  healthy remote can raise the option.
- The wedge shape retains one parked worker task + socket per timed-out
  connect until the remote answers (upstream rmcp limitation, rc-s4xod);
  no retry loop amplifies it.
- No wire or protocol change; no new dependency; DNS pinning, the 5 s
  budgets, and post-connect deadlines stay untouched.
- Existing tests construct `McpRemoteConfig` literally; they gain the new
  field (mechanical).

Bd: rc-9kpye (residual rc-s4xod filed upstream-shaped)
