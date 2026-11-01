# Proposal: bridge-test-stability

## Why

Three tracked test-debt defects (bd rc-2t4j8, rc-zbdfh, rc-dya5z) make the XML/CXF bridge
test surface unreliable under the fleet's standard verification environment
(`TMPDIR=/home/shared/tmp`, parallel execution):

1. **Tempdir race (rc-2t4j8, rc-zbdfh)** — `crates/services/camel-bridge/src/tls.rs` rung
   tests mutate the process-wide `TMPDIR` via `EnvGuard` (including a deliberately
   nonexistent `no-such-dir` path) while parallel `process::tests` create
   `tempfile::tempdir()` parents under it. When the rung test's own `TempDir` drops,
   concurrent tests fail with `NotFound` (`fake_bridge tempdir`, `marker tempdir`).
   Observed in `malformed_soap_ready_fails_startup`, `process_dual_ready_reordered_is_accepted`.
2. **Wall-clock budget flake (rc-zbdfh)** —
   `process::tests::partial_ready_is_bounded_and_completes_in_one_timeout` asserts
   real-elapsed bounds (`elapsed < 5000ms`) around a real child that runs shell `sleep`s;
   under machine load it exceeds the budget by ~280ms. Load-sensitive by construction.
3. **Missing bridge binary (rc-dya5z)** — `crates/camel-test/tests/mtls_bridge_test.rs`
   (`start_and_connect_mtls_handshake`, `wrong_ca_handshake_rejected`) panics with a raw
   "binary not found" when `CAMEL_XML_BRIDGE_BINARY_PATH` is unset, instead of skipping
   with a provisioning diagnostic.

No production behavior is intended to change.

## What Changes

- `crates/services/camel-bridge/src/tls.rs` tests: TMPDIR-mutating rung tests move into
  isolated child test processes (existing in-crate precedent in
  `src/process.rs` (`tls_spawn_isolation_inner`): marker env + `--exact
  --test-threads=1` re-exec; the new inner test uses its own marker env, separate from
  `TLS_SPAWN_ISOLATION_CHILD_ENV`). Env is applied
  per-child via `Command::env()`; the parent test process never mutates `TMPDIR`, so no
  parallel test shares a removable tempdir parent. Suite stays fully parallel.
- `crates/services/camel-bridge/src/process.rs`: extract the one-enclosing-budget
  readiness wait (`timeout(budget, read_ready_ports(...))` plus its elapsed→
  `BridgeError::Timeout` mapping) into a same-module helper used by
  `BridgeProcess::start` unchanged; the partial-ready test drives the helper
  through a duplex reader under `tokio` paused time (virtual clock, event ordering) with
  no child process, no sleeps, no wall-clock assertions, no bound ports.
  Adds a dev-only dependency: `tokio` with the `test-util` feature
  (`tokio = { workspace = true, features = ["test-util"] }`).
- `crates/camel-test/tests/support/xml_bridge.rs` + `mtls_bridge_test.rs`: when
  `CAMEL_XML_BRIDGE_BINARY_PATH` is **absent**, the two mTLS tests print an explicit SKIP
  diagnostic naming the env var and `cargo xtask build-xml-bridge` provisioning, then
  pass; when the var is **set but invalid** (including empty), they still fail loudly.
  The new helper sits next to `require_xml_bridge_binary` whose existing callers stay
  unchanged. Env precondition documented in `crates/camel-test/README.md`.

Excluded: serializing the test suite, weakening any assertion that runs when the binary
is present, provisioning the bridge binary in CI, production bridge behavior changes,
new sleeps/timeouts.

## Acceptance criteria

- `TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib` passes 3 consecutive runs
  (parallel, default test threads), including a targeted concurrent mix of the
  previously racing tests.
- `partial_ready_is_bounded_and_completes_in_one_timeout` (redesigned) passes
  deterministically; a negative case proves `BridgeError::Timeout` still fires when
  readiness never arrives.
- `cargo test -p camel-test --features integration-tests --test mtls_bridge_test`:
  without `CAMEL_XML_BRIDGE_BINARY_PATH` → 2/2 pass
  (`start_and_connect_mtls_handshake`, `wrong_ca_handshake_rejected`), each emitting a
  SKIP diagnostic on stderr; with a valid path → both tests run their full assertions
  unchanged. The wider `cargo test -p camel-test` suite is run with and without the
  var.
- `cargo fmt --check --all`, clippy (touched crates, all targets, `-D warnings`), all
  `cargo xtask lint-*` gates in the AGENTS.md QUALITY GATES list, rustdoc
  `-D warnings` on touched crates, and `openspec validate --all --strict` all green.

## Risk budget

Test-only change; the single production-code touch is a behavior-identical extraction
of the readiness wait into a same-module helper (justified as an unavoidable test seam;
no API or semantics change). Risk accepted: none to runtime behavior. Out of bounds:
any change to `BridgeProcess` lifecycle, kill semantics, or readiness protocol parsing.
