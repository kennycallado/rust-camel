# Design: bridge-test-stability

## Approach

Three independent test-only fixes, one per bd ticket. The suite stays fully parallel;
no assertion that runs today is weakened.

### Fix 1 — TMPDIR race: child-process isolation for `tls.rs` rung tests (rc-2t4j8, rc-zbdfh)

Root cause: `crates/services/camel-bridge/src/tls.rs` rung tests
(`tmpdir_rung_wins_when_writable`, `system_temp_rung_when_tmpdir_unset`,
`cache_dir_rung_when_no_tmp_usable`, and the all-rungs-fail error test) mutate the
process-wide `TMPDIR` through `EnvGuard` — one of them to a deliberately nonexistent
`no-such-dir` path — while parallel `process::tests` call `tempfile::tempdir()`, whose
parent then lives inside a `TempDir` another test deletes. `EnvGuard` serializes
env-mutating tests against each other, not against env *readers*.

Fix by construction, using the in-crate precedent in `src/process.rs`
(`tls_spawn_isolation_inner`, marker env, `--exact --test-threads=1` re-exec of the
test binary): each rung test becomes an outer `#[test]` that spawns the test binary as
a child with the scenario env applied via `Command::env()`/`.env_remove()` — per-child,
never touching the parent process env. One inert inner `#[test]` (matched by its full
path `tls::tests::<inner>` for `--exact`, inert unless its OWN marker env is set —
separate from `TLS_SPAWN_ISOLATION_CHILD_ENV`) dispatches on a scenario env var.
Outer-owned `TempDir`s stay alive until the child exits, so no concurrent test can
observe a removable parent. Scenario values (e.g. the `fresh` path, the expected XDG
dir) are passed to the child via env vars; assertions run inside the child; the outer
asserts child exit status.

### Fix 2 — deterministic enclosing-budget test in `process.rs` (rc-zbdfh)

`BridgeProcess::start` today wraps
`timeout(Duration::from_millis(start_timeout_ms), read_ready_ports(&mut reader, expect_soap_ready))`.
Extract that expression into a private same-module helper with the contract:

`read_ready_within(reader, start_timeout_ms, expect_soap_ready, name) ->
Result<(u16, Option<u16>), BridgeError>`

- `Ok(Ok(ports))` → `Ok(ports)`;
- elapsed → `BridgeError::Timeout(msg)` with the same message and `tracing::error!`
  that `start` emits today (the mapping lives INSIDE the helper, so the paused-clock
  test below covers the `Timeout` arm);
- `Ok(Err(StdoutClosed))` → passes `StdoutClosed` through untouched, so `start`
  keeps classifying it with the child (`eof_exit_error`) exactly as today.

`start` calls the helper and keeps only the child-specific `StdoutClosed`
classification and pass-through of the other results. Behavior-identical; this is the
one unavoidable production touch (test seam), justified in the park; no API change
(private fn, same module). Dev-only addition:
`tokio = { workspace = true, features = ["test-util"] }` in
`crates/services/camel-bridge/Cargo.toml` `[dev-dependencies]` (the workspace `tokio`
features do not include `test-util`).

The redesigned `partial_ready_is_bounded_and_completes_in_one_timeout` runs under
`#[tokio::test(start_paused = true)]` with `tokio::io::duplex` feeding JSON readiness
lines to the helper; ordering is driven by virtual-clock sleeps in the feeder task:

- Positive: `soap-ready` at virtual 1_000 ms, `ready` at virtual 3_500 ms,
  budget 4_000 → `Ok`, both ports stored, virtual elapsed ≤ budget.
- Counterexample (enclosing budget, no per-line reset): `soap-ready` at virtual
  1_000 ms, `ready` at virtual 4_500 ms > budget 4_000 → `BridgeError::Timeout` at
  the 4_000 ms deadline (a per-line-reset implementation would still succeed at its
  5_000 ms reset deadline). Virtual ordering replaces the old marker-file proof.
- Injected negative: feeder sends no recognized line and stays alive →
  `BridgeError::Timeout` at the virtual deadline (readiness never arrives).

No child process, no real sleeps, no wall-clock assertions, no bound ports (ports are
parsed JSON fields; nothing listens). The duplex writer is held open in the negative
case so the `StdoutClosed` path cannot preempt the deadline.

### Fix 3 — explicit skip for absent bridge binary in `camel-test` (rc-dya5z)

`tests/support/xml_bridge.rs` gains a helper placed next to
`require_xml_bridge_binary` (whose existing callers stay unchanged), with the
contract: env var `CAMEL_XML_BRIDGE_BINARY_PATH` **absent** → print an explicit SKIP
diagnostic naming the var and `cargo xtask build-xml-bridge` provisioning, return
`None`; **set but not an existing file — including an empty value** → panic with the
same provisioning hint (misconfiguration stays loud). The two mTLS tests
(`start_and_connect_mtls_handshake`, `wrong_ca_handshake_rejected`) early-return on
`None`; `require_xml_bridge_binary` keeps its current behavior for its other callers.
Their full assertions are unchanged when the binary is present. The `xml_bridge.rs`
module doc is updated to describe the skip path, and `crates/camel-test/README.md`
documents the env precondition and skip semantics.

## Affected crates

- `camel-bridge` (services): test rework in `src/tls.rs`, `src/process.rs`; one
  private helper extraction in `src/process.rs`; dev-only feature addition
  (`tokio` `test-util` via `[dev-dependencies]`).
- `camel-test` (test workspace): `tests/support/xml_bridge.rs`,
  `tests/mtls_bridge_test.rs`, `README.md`.

## Architecture boundaries

Services layer only, and only its test surface. No Runtime/DSL/Components/Languages/
Functions change; no bridge lifecycle, kill, or readiness-protocol change; the
readiness protocol semantics (any line order, single enclosing budget, port
validation) are preserved byte-for-byte by the extraction. mTLS assertions against a
real bridge binary are untouched.

## Verification

RED first, per ticket: (1) baseline concurrent mix
(`TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib` filtered to the tls rung
tests plus `process_dual_ready*`/`partial_ready*`/`malformed_soap*`, default threads)
reproduces the `NotFound` race; (2) baseline
`env -u CAMEL_XML_BRIDGE_BINARY_PATH cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
shows the 2 raw failures. GREEN: camel-bridge `--lib` ×3 under `TMPDIR`, deterministic
re-runs of the timing test, mtls 10/10 without the var and full assertions with a
valid var; then fmt, clippy (touched crates, all targets), the 24 `cargo xtask lint-*`
gates, rustdoc `-D warnings` on touched crates, `openspec validate --all --strict`.

Spec decision: test-only and behavior-neutral — no delta specs; `skip_specs: true` in
the change config. Evidence: the sole production touch is a behavior-identical
extraction whose `Timeout` mapping is covered by the new paused-clock test (the old
real-child `Timeout` assertion moves into that test); the real-child tests continue to
cover the success, EOF/`StdoutClosed`, and malformed-line paths unchanged. Everything
else is test code, a dev-only dependency feature, or docs.
