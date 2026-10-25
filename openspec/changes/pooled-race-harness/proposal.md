# Proposal: pooled-race-harness

## Why

Eleven-plus hand-written TCP servers in tests answer one response then FIN while hyper
keeps the connection pooled and reuses it for the next hop — the pooled-race flake class
(rc-u3aw, ~1 ms race on macOS runners; bd rc-nhoh8, class child of rc-99d5). The
Connection: close fix for rc-u3aw survived adjudication; the class fix is a shared
harness plus a structural lint. Pre-flight (e_opus) widened the measured class: the
lint-precision inventory is 21 response-literal sites in `crates/` plus 3 test-scope
sites under `benchmarks/` — the "11" in the bd counts only the strict
answer-one-then-FIN servers. Reaching the mandated lint target of 0 requires covering
the full literal class.

## What Changes

- Add one shared HTTP/1.1 test-harness listener at
  `camel_component_api::test_support::http` (behind the existing `test-support`
  feature): parses request line + headers, drains request head/body before replying,
  emits well-formed responses, and **mandates `Connection: close` on terminal
  responses**; keep-alive is an explicit `.keep_alive(reason)` opt-in; public
  `serve_connection` lets TLS tests wrap their own acceptor.
- Migrate the raw test servers that lack `Connection: close` onto the harness:
  camel-component-http (ssrf, client_cache, lib_tests incl. the TLS site),
  camel-test (`http_test.rs`), camel-integration-test (`http_partner_test.rs`).
  Sites that already send `Connection: close` (otel `HttpMock`, cxf TLS server,
  `component_emission_test.rs`) stay as-is. `HttpPartner` (hyper-based,
  camel-integration-test) stays as-is.
- Land the `no-raw-http-test-server` xtask lint: flags HTTP/1.1 response string
  literals in test scope that neither set `Connection: close` nor go through the
  harness; `// allow-raw-http: <reason>` marker for adjudicated exceptions; seeded
  monotone ratchet file lowered to 0 by the end of this change; wired into CI and the
  AGENTS gate registry.
- Adjudicate the 3 benchmark-scope sites with markers (loadgen must not depend on
  camel-component-api).
- Excluded: production code changes, tower-oneshot/mock substitution for
  pooling-under-test sites, sleeps to hide races, test behavior changes (same
  requests, assertions, timing), version bump.

## Acceptance criteria

- The shared harness exists in `camel-component-api` under `test-support`, with unit
  tests for request parsing, drain-before-reply (including segmented and truncated
  bodies), close injection, keep-alive opt-in, request recording (parsed fields AND
  exact wire head), staged chunked output, malformed-request outcomes, and
  task-cancelling teardown.
- Every inventoried `crates/` site (design §Affected crates) SHALL be **migrated** to
  the harness — markers are NOT an accepted resolution for them; pooling-under-test
  suites keep a real server (no oneshot substitution). `// allow-raw-http` markers
  exist ONLY in `benchmarks/` at the end of this change, on the 3 enumerated
  test-scope sites, each with a preserved-behavior rationale; the lint output SHALL
  list marker locations and counts so gate reports show them.
- `cargo xtask lint-no-raw-http-test-server` passes with ratchet at 0; the gate is
  registered in CI and AGENTS.md QUALITY GATES.
- Touched suites green (listed in the final report) plus `cargo test --workspace --lib`;
  fmt, clippy for touched crates; `lint-unbounded-wait` at ceiling 0;
  `lint-test-sleep`; `lint-no-free-port` ratchet unchanged.
- Report table: server site -> harness adoption -> residual marker (if any).

## Risk budget

- Acceptable: responses gain `Connection: close` (the adjudicated mitigation); test
  files reorganize locally around the harness.
- Not acceptable: changed test assertions/timing, sleeps as sync, production-code
  behavior changes, hyper added to camel-component-api, loadgen depending on
  camel-component-api.
