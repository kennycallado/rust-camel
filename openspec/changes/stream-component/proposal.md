# Proposal: stream-component

bd: rc-jp9lp (P2). Binding ruling: `docs/rulings/2026-09-18-stream-stdio-primitive-illumination.md` (e_opus, BUILD-WITH-CONSTRAINTS).

## Why

A route author who wants clean piped data on stdout has no path today. `BuilderStep` offers `To`, `Log`, `Stop`, and transforms — `log` is the only in-route output primitive, and it is leveled, redacted, and tracing-shaped (`crates/components/camel-log/src/lib.rs`). Routes that compute a result for a pipe must abuse `log` or write `file:` to `/dev/stdout`. `camel-cli` reaches for raw `println!` for the same reason. Stdin has no route-facing shape at all: a batch pipe (`echo x | camel run route.yaml`) cannot feed exchanges.

The e_opus ruling resolves the identity question: a `stream:` component (fd 0/1/2 adapter) is ordinary integration vocabulary; the "interactive job" runtime semantic is scope-creep and is rejected. ADR-0060:108 (MCP stdio rejection) is inapplicable — it rejected process supervision, not touching our own file descriptors.

## What Changes

- New crate `crates/components/camel-stream` (package `camel-component-stream`), scheme `stream`, sub-endpoints by path segment:
  - `stream:out` — Producer, body as DATA to fd 1, `\n`-terminated by default (`appendNewline=false` for raw), flush per exchange. No levels, no log formatting, no redaction.
  - `stream:err` — Producer, same semantics to fd 2 (prompts, diagnostics).
  - `stream:in` — Consumer, one line = one exchange (`frame=line` default; `raw`, `fixed&size=N` offered). EOF = route completes gracefully, never an error. Input-PRESENCE detection, never `isatty`.
- Slim (default-on) registration in the `camel-bundles` cascade; shipped in the lean set with `direct`, `log`, `mock`, `seda`, `timer` per ruling Q5.
- Boot-time `warn!` when a `stream:out` route is active AND the tracer stdout sink is enabled (collision posture; no auto-mux).
- `camel job` fail-closed consumer allowlist gains exactly `stream:in` (path-checked); `from(stream:out)`/`from(stream:err)` still rejected at load with exit code 2. Allowlist entry and load-gate tests only — job live-source wait mechanics stay with the job epic (rc-d5dgc coordination note, no dependency); end-to-end pipe parity is verified on `camel run`, which already hosts live consumers.
- ADR (accepted-limitations): un-redacted egress is operator-owned; the four REJECTED items (blocking `read` step, `set-prompt` step, REPL state, isatty control flow) with the ADR-0060:108 distinction.
- Interactive-job composition example (`from(stream:in) → transform → to(stream:out)`, prompts via `to(stream:err)`) on `camel run` — emerges free, no new primitive.

**Explicitly excluded (ruling REJECT list):** mid-route blocking `read` DSL step, `set-prompt` step, REPL/conversation state, prompt-orchestration engine, isatty-driven control flow, producer-side auto-redaction, Windows-console/pty handling, any durable wait-for-human semantics.

## Acceptance criteria

- `to(stream:out)` writes the body verbatim to fd 1 (line and raw modes), flushes per exchange, applies no redaction — conformance-tested.
- `stream:in` emits one exchange per line; EOF completes the route with zero exchanges and exit 0 — the no-TTY/no-input conformance test is load-bearing.
- `stream:in` reads the next line only after the current send completes (sequential backpressure); no unbounded queue.
- Slim boot resolves the `stream` scheme with no feature gate; bridges-exclusion story untouched.
- A job document naming `from(stream:in)` loads; one naming `from(stream:out)` as consumer fails closed at load (exit 2) with an exact error.
- Tracer-stdout + `stream:out` coexistence emits a boot `warn!`; docs state the operator resolution.
- ADR + example (`camel run` composition) + crate CONTEXT.md/README present; all mission gates run — the golden deptree fixture gate is a known mission-order conflict (slim-included ruling vs no-regen mandate) and executes the mission's STOP-and-report instruction at gate time (see design).

## Risk budget

Ruling risk register R1–R10 mitigations are binding design inputs. Accepted: the golden deptree fixture WILL drift by the new crate's lines (slim mandate) — never silently regenerated; STOP-and-report per mission order (see design). Out of bounds: any runtime/core change to `BuilderStep`, any new DSL step, any blocking read, any redaction on the data plane, any job-runner live-source wait change.
