# Proposal: rediserr

## Why

`is_transient_redis_error` (`crates/components/camel-redis/src/config.rs:1069`)
classifies retry-transience by lowercase substring matching on
`err.to_string()` (`connection`, `io error`, `timed out`, `broken pipe`,
`eof`, `refused`, `readonly`, ...). This is stringly-typed (bd rc-tielu):

- False positives: any `ProcessorError` whose text happens to contain a
  classifier word (e.g. `eof` inside an unrelated word) retries.
- Silent misses: new redis-rs 1.6 spellings that render differently never
  retry.
- The transience verdict — which errors restart a Route via the bounded
  reconnect loops (ADR-0007, ADR-0012) — is invisible in review: changing a
  log message can silently change retry topology.

Precedent: structural retry classification in 11be1863 (`fix(job)`) and
92e9e27c (`test(support)`).

## What Changes

- Preserve structured errors at every `redis::RedisError` → `CamelError`
  conversion boundary on the retry-classification paths: the component's
  command modules (`commands/*.rs`), `topology.rs` resolve/connect,
  `executor.rs` connect path, and the reconnect loops' wraps — via the
  existing `CamelError::ProcessorErrorWithSource(_, Arc<dyn Error>)` variant.
- Reimplement `is_transient_redis_error` structurally, in precedence order:
  1. `Config`/`ConfigValidation` → false (unchanged, ADR-0012 boundary);
  2. `CamelError::Io(_)` → true (today every `Io` renders `IO error: …`,
     which matches `io error` — verdict preserved at variant level);
  3. typed local markers (`TransientRetryBudgetExhausted` from `retry.rs`,
     `TransportTimeout` for tokio-elapsed connect/response timeouts,
     `TransientByProse` for prose-word sites) → true;
  4. `redis::RedisError` in the source chain → classify on `kind()`:
     `Server(ReadOnly)` → true; `Io` with an `io::Error` source → transient
     io kinds (`ConnectionRefused/Reset/Aborted`, `BrokenPipe`, `TimedOut`)
     → true; `ClusterConnectionNotFound` → true (its Debug rendering
     contains `connection` today);
  5. one narrow, documented substring fallback: a preserved
     `redis::RedisError` matched by no enumerated kind shape (custom io
     text, TLS inner errors, server-controlled messages, redis-rs static
     details like `SSL Handshake error`) falls back to the legacy
     substring test on that error's own Display — identical verdicts by
     construction;
- Wrap sites whose static prose contains a classifier word (legacy
  always-transient, e.g. `failed to build Redis connection info:`) carry
  a `TransientByProse` marker — verdict preserved structurally; identity
  is proven by a per-site static-prose audit table maintained in
  design.md.
- `retry.rs::retry_budget_exhausted` keeps its message text (logs unchanged)
  but carries a typed marker source; the load-bearing-word comment regime
  is retired in favor of the marker.
- Plain `ProcessorError`/`ProcessorErrorWithSource` with no redis error and
  no marker in the chain → false (removes the false-positive class).
- Verdict table behaviorally IDENTICAL for every realizable input (same
  input → same verdict). No verdict-change commit: no reviewed verdict
  proved demonstrably wrong.
- Affected crates: `camel-component-redis`, `camel-redis-repo`. NOT changed:
  `camel-api` (no new `CamelError` variant needed), `CamelError::classify`,
  auth-failure message detection (`is_auth_failure_message` stays
  text-based — different concern, different table).

## Acceptance criteria

- No lowercase substring matching on `err.to_string()` for the structural
  paths; the only matching left is the documented `ErrorKind::Io` fallback
  in step 5.
- Existing verdict-pin tests ported to structured fixtures
  (`redis::RedisError` instances, markers, `CamelError::Io`) and pass.
- `cargo fmt --check`, `clippy -D warnings` on affected crates, affected
  test suites green.

## Risk budget

- Highest risk: a missed conversion boundary flips a live transient error
  to fatal, breaking reconnect loops. Mitigation: exhaustive site inventory
  in design.md; r_glm verifies inventory completeness per task.
- Out of bounds: changing `CamelError::classify`, touching `camel-api`,
  rewording any operator-visible error message.
