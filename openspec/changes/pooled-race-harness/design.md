# Design: pooled-race-harness

## Approach

One shared harness, one lint, migration in dependency order. Pre-flight verdict:
GO-WITH-CONDITIONS (e_opus) — the binding conditions are: the harness MUST drain the
request head and body before replying (closing with unread bytes RSTs on Linux —
that only removes flakiness); the harness MUST support TLS through a public
`serve_connection` rather than pulling tokio-rustls into the API; the lint MUST scan
macro token streams (`format!` hides most literals from a plain syn AST walk) and MUST
exclude literals passed to `starts_with`/`ends_with`/`contains` (assertion sites are
not response construction).

**Harness home.** `camel_component_api::test_support::http`, behind the existing
`test-support` feature (`#[cfg(any(test, feature = "test-support"))] pub mod
test_support;` already exists at `src/lib.rs:24`). Both `camel-test` and
`camel-component-http` (dev-deps) already enable `test-support`, so every migration
site reaches the harness without a dependency cycle. Hand-rolled HTTP/1.1 over tokio
(no hyper in a production crate's test feature): read request line, headers until
`\r\n\r\n`, then Content-Length-sized body; drain fully; then write the scripted
response. No new workspace dependencies: tokio and tokio-util (its
  `CancellationToken`) are already camel-component-api dependencies; byte buffers are
  `Vec<u8>` (the crate does not depend on `bytes`, and this change must not add it).

**Harness API.**
- `HttpTestServer::start(responder)` builder: bind `127.0.0.1:0`, spawn accept loop,
  return handle with `local_addr()`/`port()`. **Teardown ownership:** `Drop` NEVER
  blocks (a blocking join cannot make progress on a current-thread executor):
  dropping aborts the listener task and all active connection tasks via a
  cancellation token and detaches them. For tests that must confirm reclamation, the
  handle exposes an explicit `async fn shutdown(self)` with a bounded join budget —
  an incomplete request or an idle keep-alive connection at shutdown time is aborted,
  not awaited. Teardown works identically on current-thread and multi-thread
  runtimes.
- Responder forms: fixed response (status, headers, body), path-routed closure
  (`Fn(&RecordedRequest) -> Response`), recorded-request capture.
- **Request record exposes both layers:** parsed fields (method, headers, body) AND
  the exact wire head — the raw request line and head bytes with the unnormalized
  request-target including query bytes — so existing raw-bytes assertions
  (`http_partner_test.rs`, `http_test.rs`) survive migration unchanged.
- **Framing modes:** default responses are `Content-Length`-framed; a chunked mode
  emits legal `Transfer-Encoding: chunked` framing (no Content-Length); a **staged**
  mode returns the header block immediately followed by ordered chunk futures
  (signaled via channels — no sleeps), preserving the existing
  immediate-headers-then-delayed-chunks behavior of `lib_tests.rs:2708-2715`.
- Every terminal response gets `Connection: close` injected by default; `.keep_alive(
  reason: &'static str)` opt-in serves requests in a loop on one connection until the
  peer closes — reason recorded at the configuration site. Pipelined bytes after a
  completed request are parsed as the next request in keep-alive mode.
- **Malformed-request outcomes (deterministic):** unparseable head, invalid
  Content-Length, premature EOF mid-body, or an unsupported Transfer-Encoding → the
  harness answers `400` with `Connection: close` and closes the socket. No hang paths.
- Raw escape hatch: `Response::raw(Vec<u8>)` for sites that must own their bytes — the
  harness still injects `Connection: close` unless the raw bytes already carry a
  Connection header, and `raw` is a harness constructor, which is exactly what makes
  its literal arguments lint-exempt (structural exemption, below).
- `pub async fn serve_connection<IO: AsyncRead + AsyncWrite>(io: IO, ...)` — the TLS
  site wraps its own `TlsAcceptor` stream and calls this; the harness crate itself
  needs no rustls.

**Lint.** `scripts/xtask/src/lint_no_raw_http_test_server.rs`, mirroring
`lint_no_free_port.rs` (bd rc-d70up, mission 401) structure: syn visitor over
`MEMBER_ROOTS = ["crates", "scripts", "examples", "benchmarks", "fuzz"]`.
- Finding: a `LitStr`/`LitByteStr` (or literal inside a macro token stream) whose
  text starts with an HTTP/1.x status line — after the `HTTP/1.[01] ` prefix, either
  three digits OR a `{` interpolation placeholder starts the status token — and whose
  literal does not contain a **structurally valid** `Connection: close` header: a
  header line whose name is `connection` (case-insensitive) and whose value contains
  the `close` token, positioned before the blank line that ends the header block.
  Substring containment in a body or another header's value does NOT count (negative
  fixtures required). Both interpolation shapes are covered by fixtures:
  `HTTP/1.1 {status_line}\r\n…` and `HTTP/1.1 {} {}\r\n…`.
- Exclusions: (a) literals that are direct arguments of `.starts_with(..)`,
  `.ends_with(..)`, `.contains(..)` receiver calls (response assertions, e.g.
  `cxf_test/tls.rs:403`); (b) **structural harness exemption** — literals passed as
  arguments (directly or nested in constructor/tuple/vec arguments) to
  `test_support::http` constructors (`HttpTestServer::start*`, `Response::new`,
  `Response::raw`, `Response::chunked*`) are harness-supplied and exempt. There are
  deliberately NO file-wide or import-based exemptions.
- Test scope: union of `tests/**` dirs, files named `*_test.rs`/`*_tests.rs`,
  `#[cfg(test)]` modules, and `#[test]`/`#[tokio::test]` fns — reuse the existing
  `scan_state::TestScopeTracker` from lint-test-sleep.
- Escape hatch: `// allow-raw-http: <non-empty reason>` on the line immediately above
  the literal; suppresses that site only. The command output SHALL list every marker
  location and the marker count, so gate reports show adjudicated residuals.
- Policy binding (proposal AC): crate-site migrations are mandatory — markers are
  expected ONLY on the 3 enumerated `benchmarks/` sites at the end of this change.
- Ratchet: `scripts/xtask/ratchet-raw-http-test-server.max`, fail-closed read,
  monotone-decreasing count ceiling, seeded at the measured baseline when the lint
  lands (Phase 2), lowered by each migration phase, **0 at Phase 5 exit**.
- Registration: `Commands::LintNoRawHttpTestServer` in `scripts/xtask/src/main.rs`
  (same enforcement output shape as `LintNoFreePort`), AGENTS.md QUALITY GATES entry,
  CI workflow gate alongside the other xtask lints.

**Non-migrations (classified, pre-flight).** otel `HttpMock` (std-thread for the
blocking reqwest client; already sends close — stays std, out of harness scope),
cxf TLS SOAP server (already close; already parses request heads),
`component_emission_test.rs` (already close), `HttpPartner` in
camel-integration-test (hyper-correct, ships to camel-cli as a normal dep, no
literals), redis RESP server (not HTTP), accept-then-drop peer in
`http_client_lane_test.rs` (no HTTP response by design), `ws_connect` in
compiled_artifact_test.rs (client side; the 101 response belongs to the real ws
server under test), camel-dsl axum servers (real axum, no literals).

## Affected crates

- `camel-component-api`: new `test_support::http` module (test-support feature) —
  the harness + unit tests. No production surface change.
- `camel-component-http`: migrate ssrf (×3), client_cache (×1), lib_tests (×11
  literals incl. the TLS site) to the harness; dev-dep already present.
- `camel-test`: migrate `http_test.rs` sites (5 literals) to the harness; dep already
  present.
- `camel-integration-test`: migrate the raw far-side server in
  `http_partner_test.rs` (the far side must stay a real listener — the test asserts
  literal dialing; it uses the harness as that listener, keeping request-head
  capture).
- `scripts/xtask`: new lint module + ratchet file + command registration.
- `benchmarks/` (loadgen): 3 test-scope sites get `// allow-raw-http` markers; no new
  deps.
- `.github/workflows` + `AGENTS.md`: gate registration.

## Architecture boundaries

Test-support only: everything lands behind `test-support` (camel-component-api) or in
test/dev files, xtask, CI config. No Runtime/DSL/Component/Service production surface
changes. The harness never links hyper; components keep their own client stacks.
camel-component-api must not gain runtime dependencies for this (rcgen already behind
the feature; harness uses tokio + std only).

## Phases

### Phase 1: Harness
- **Goal:** `camel_component_api::test_support::http` with unit tests.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `HttpTestServer`, `Response`,
  `RecordedRequest`, `serve_connection` (all `#[cfg(any(test,
  feature = "test-support"))]`).
- **Deliverable:** module + unit tests (parse request line/headers/Content-Length
  body; drain-before-reply incl. segmented and truncated bodies; close injection
  default; keep-alive opt-in loop incl. pipelined next request; request recording
  with exact wire head; path routing; raw-body escape; staged chunked output;
  malformed-request 400 outcomes; task-cancelling teardown incl. stalled client and
  incomplete request at drop; serve_connection over a duplex pair).
- **Exit-criteria:** `cargo test -p camel-component-api --lib` green; clippy/fmt clean.

### Phase 2: Lint
- **Goal:** `no-raw-http-test-server` lint landed, ratchet seeded at measured
  baseline, registered in CI + AGENTS.
- **Dependencies:** Phase 1 not required (lint is independent), but ordering keeps
  ratchet churn to one direction.
- **Externally-visible types/interfaces:** xtask command `lint-no-raw-http-test-server`,
  ratchet file `ratchet-raw-http-test-server.max`.
- **Deliverable:** lint module with unit fixtures (close/no-close, macro literal,
  starts_with exclusion, dynamic status-line rule, marker, test-scope matrix),
  main.rs dispatch, ratchet file, CI workflow gate, AGENTS.md registry row.
- **Exit-criteria:** lint runs green at seeded baseline; unit tests green; negative
  fixture (request literal) produces no finding.

### Phase 3: camel-component-http migration
- **Goal:** all camel-component-http literal sites on the harness.
- **Dependencies:** Phases 1-2.
- **Externally-visible types/interfaces:** none new (test-only edits).
- **Deliverable:** ssrf ×3, client_cache ×1, lib_tests ×11 sites migrated (TLS site
  via `serve_connection` + existing in-crate TLS harness); ratchet lowered by the
  number resolved.
- **Exit-criteria:** `cargo test -p camel-component-http` green (touched suites
  named in report); lint count reduced accordingly; clippy/fmt clean.

### Phase 4: camel-test + camel-integration-test migration
- **Goal:** remaining `crates/` literal sites on the harness.
- **Dependencies:** Phase 3.
- **Externally-visible types/interfaces:** none new.
- **Deliverable:** http_test.rs ×5 sites; http_partner_test.rs far side migrated to
  the harness (wire-head capture preserved, real listener kept); ratchet lowered to
  the benchmark-only residual.
- **Exit-criteria:** explicit suite runs green with NONZERO executed-test counts
  recorded as evidence: `cargo test -p camel-test --features integration-tests
  --test http_test` (the file is `#![cfg(feature = "integration-tests")]`-gated at
  the top — a run without the feature compiles zero tests and is NOT evidence) and
  `cargo test -p camel-integration-test --lib http_partner --features http` (feature
  name confirmed from its Cargo.toml at implementation time); ratchet shows only
  benchmark residuals.

### Phase 5: benchmarks adjudication + ratchet to 0 + full gates
- **Goal:** class closed.
- **Dependencies:** Phase 4.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** 3 benchmark test-scope sites marked `// allow-raw-http: <reason>`
  (loadgen stays dep-free); ratchet file at 0; full local gate pass recorded.
- **Exit-criteria:** `cargo xtask lint-no-raw-http-test-server` passes at ratchet 0
  with the marker report showing exactly the 3 benchmark sites; fmt, clippy (touched
  crates + workspace shape), `cargo test --workspace --lib` PLUS the explicit
  feature-enabled suite commands from Phase 4 (evidence of execution recorded),
  lint-unbounded-wait (ceiling 0), lint-test-sleep, lint-no-free-port (ratchet
  unchanged), doc gate all green; report table complete.

## Alternatives considered

- **Hyper-based harness in camel-component-api:** rejected — pulls hyper into a
  production crate's feature graph; the hand-rolled parser is ~100 lines and the
  class exists precisely because hand-rolled servers get framing wrong; centralizing
  the hand-rolled part behind tests is the fix.
- **Per-crate harnesses (extend `tls_harness.rs`, copy into camel-test):** rejected —
  reproduces the drift that created this class; the bd mandates ONE shared listener.
- **Adopt `HttpPartner` everywhere:** rejected — it lives in camel-integration-test,
  which camel-test/camel-component-http cannot dev-dep (cycle risk: camel-test deps
  camel-component-http), and it ships as a runtime dep of camel-cli.
- **Migrate otel/cxf sites too:** rejected — already compliant (close present), otel
  is constrained to std::net by the blocking client; touching them risks behavior
  change for zero class value.
- **Lint flags all HTTP/1.1 literals regardless of close:** rejected — the bd wording
  targets literals that "neither set Connection: close nor go through the harness";
  compliant raw servers stay legal, markers are for adjudicated exceptions only.
