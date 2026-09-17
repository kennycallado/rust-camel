# Tasks: jobcoexist

Single-phase change. All tasks in `crates/camel-cli` plus repo docs.
Spec citations point at `openspec/changes/jobcoexist/specs/`.

## 1. job-projection — job boot projection fn + wiring + structural tests

Implements `cli-jobs` scenario "projection allowlist is exact" and the
projection half of "ambient diagnostics are projected away".

Files:
- `crates/camel-cli/src/commands/job/mod.rs` (modified)

Steps:
1. Add a private function in `crates/camel-cli/src/commands/job/mod.rs`,
   directly above `execute_job`:
   `fn job_effective_config(config: &CamelConfig) -> CamelConfig` that
   clones `config`, sets `runtime_journal = None` and
   `observability = ObservabilityConfig::default()`, and returns the
   clone. It assigns exactly those two fields and nothing else.
2. At the top of `execute_job` (before the `beans_registry` block that
   precedes `configure_context_with_beans`), bind
   `let camel_config = job_effective_config(&camel_config);` so every
   downstream use — beans registry emptiness check,
   `configure_context_with_beans`, security compile context, bind acks,
   component cascade — sees the projected config. All three convergence
   sites (argv `:669`, embedded single-doc `:788`, embedded store `:926`)
   inherit the projection through `execute_job`.
3. Add a `#[cfg(test)]` test module (or extend the existing one) with the
   two tests below. Import `camel_config::config::{CamelConfig,
   JournalConfig, JournalDurability, ObservabilityConfig,
   OtelCamelConfig, PrometheusCamelConfig, HealthCamelConfig}` and
   sibling config types as needed for construction.
4. Run the tests; they pass only after steps 1-2 exist.

Tests:
- name: `job_effective_config_neutralizes_journal_and_observability`
  setup: a `CamelConfig` with `runtime_journal = Some(JournalConfig)`
  (any path, `JournalDurability::Immediate`), `observability` with
  `otel = Some(OtelCamelConfig { enabled: true, .. })`,
  `prometheus = Some(PrometheusCamelConfig { enabled: true, .. })`,
  `health = Some(HealthCamelConfig { enabled: true, .. })`.
  action: call `job_effective_config(&config)`.
  assert: result `runtime_journal` is `None`; `observability.otel`,
  `observability.prometheus`, and `observability.health` are all `None`.
  command: `cargo test -p camel-cli job_effective_config_neutralizes`
  expected: fails before step 1 (function missing), passes after.
 - name: `job_effective_config_preserves_all_other_fields`
   setup: a `CamelConfig` with every non-projected field set away from its
   default where constructible: non-empty `routes`, `watch = true`,
   `idempotent_repo`/`cache_repo`/`supervision` populated with real values
   (assert survival), `log_level = "debug"`, distinct `timeout_ms` /
   `drain_timeout_ms` / `watch_debounce_ms`, a `components.raw` entry,
   `platform`, `stream_caching`, a `beans` entry,
   `languages`, `security`, a `binds` entry, a `datasources` entry, and
   `jobs` with a non-default dir.
  action: call `job_effective_config(&config)`.
  assert: field-by-field, every non-projected field of the result equals
  the input's field (compare directly; types are `Clone`, `PartialEq`
  where available, otherwise assert on the same constructing values).
  command: `cargo test -p camel-cli job_effective_config_preserves`
  expected: fails before step 1, passes after.

Acceptance:
- `cargo test -p camel-cli job_effective_config` passes.
- `cargo test -p camel-cli --lib` (or the crate's unit-test target)
  passes with no regression.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check` clean for the file.
- `git -C <worktree> diff` shows no file outside
  `crates/camel-cli/src/commands/job/mod.rs`.
- Note: the spec scenario's `[observability.otel]` clause is covered at
  unit level by this task (`observability = default()` zeroes tracer,
  metrics, otel, prometheus, and health together); the e2e tasks 3-5
  prove the listener/journal clauses.

- [x] 1-job-projection

## 2. manifest-job-listener-filter — TrailerKind::Job config-listener filter

Implements the manifest half of `cli-compile` "Restrict artifact arguments
and expose manifest" (job artifacts omit suppressed listeners) at unit
level.

Files:
- `crates/camel-cli/src/compile/manifest.rs` (modified)

Steps:
1. In `derive_for_store`, change the config-entry listener merge loop: the
   `(entry_components, entry_listeners)` pairs from `scan_config_entry`
   contribute their listeners to the manifest `listeners` list only when
   the artifact `kind` is `TrailerKind::Route`. When `kind` is
   `TrailerKind::Job`, skip pushing config-entry listeners (the job boot
   projection suppresses them at runtime).
2. Do NOT touch listeners contributed by `walk_document` (document-derived
   REST/MCP declarations) — they stay for both kinds.
3. Do NOT touch the single-document `derive` function's document walk.
4. Add the two tests below to the existing `#[cfg(test)]` module in
   `manifest.rs` (~line 796+), reusing its existing `derive_for_store`
   fixture helpers.

Tests:
- name: `job_artifact_manifest_omits_config_listeners`
  setup: a store fixture whose config entry TOML enables
  `[observability.health]` (port 8081) and `[observability.prometheus]`
  (port 9090) with `enabled = true`, derived with
  `TrailerKind::Job`.
  action: call `derive_for_store` and inspect the returned manifest.
  assert: the `listeners` list contains neither `0.0.0.0:8081` nor
  `0.0.0.0:9090`.
  command: `cargo test -p camel-cli job_artifact_manifest_omits`
  expected: fails before step 1 (listeners present), passes after.
- name: `route_artifact_manifest_keeps_config_listeners`
  setup: the same store fixture and config entry, derived with
  `TrailerKind::Route`.
  action: call `derive_for_store`.
  assert: the `listeners` list contains both `0.0.0.0:8081` and
  `0.0.0.0:9090`.
  command: `cargo test -p camel-cli route_artifact_manifest_keeps`
  expected: passes before AND after step 1 (pins existing behavior).

Acceptance:
- `cargo test -p camel-cli manifest` passes including all pre-existing
  manifest tests.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- No change to `derive` (single-document) listener behavior.

- [x] 2-manifest-job-listener-filter

## 3. coexistence-e2e — live `camel run` + `camel job` real-binary test

Implements `cli-jobs` scenario "job coexists with a live server on shared
ambient config" and the `job` spec scenario "Diagnostic lifecycle removal
preserves the teardown border".

Files:
- `crates/camel-cli/tests/job_coexistence_test.rs` (new)
- `crates/camel-cli/tests/common/mod.rs` (modified only if a shared
  helper — binary path resolution, tempdir fixture — needs extending)

Steps:
1. Create the test binary with a header comment naming it the next
   integration-test binary (update the gate count in the header comment
   of `job_one_shot_test.rs` lineage: the count moves 25 → 26).
2. Fixture helper `write_shared_config(dir, prom_port, health_port)`:
   writes `Camel.toml` with `routes = ["routes/*.yaml"]`,
   `log_level = "off"`, `watch = false`, a `[runtime_journal]` table
   (`path = "journal.db"`, `durability = "Immediate"`),
   `[observability.prometheus]` (`enabled = true`, `host = "127.0.0.1"`,
   `port = <prom_port>`), and `[observability.health]` (`enabled = true`,
   `host = "127.0.0.1"`, `port = <health_port>`,
   `handler_timeout_ms = 6000`). Writes a route file
   `routes/ping.yaml` with one `direct:` consumer route. Writes a job
   document `jobs/ping.job.yaml`: one-shot `direct:ping` target with a
   `log:` sink, `timeout` 30s.
3. Port reservation helper: `std::net::TcpListener::bind("127.0.0.1:0")`
   twice, read the two ports, drop the listeners. ADR-0070 subprocess
   exception: the spawned `camel run` cannot receive a staged socket, so
   this probe is allowed here; the failure mode is loud (the server exits
   on bind error, failing the test visibly) and the test may retry the
   full flow once on that specific failure.
4. Test flow: write fixture with reserved ports; spawn the real `camel`
   binary (`camel run`) with cwd = fixture dir; poll TCP connect to both
   ports until ready (deadline 30s, 100ms interval); assert the server
   process has not exited; then run `camel job ping` (cwd = fixture dir,
   bounded by the shared `run_binary` helper's 90s ceiling); assert exit
   code 0 and stdout parses as JSON with `"outcome": "Completed"`;
   kill the server.
5. Run the test; it must fail with exit 2 (journal lock held) before
   task 1 lands, and pass after.

Tests:
- name: `job_coexists_with_live_server_holding_journal_and_ports`
  setup: fixture dir + live `camel run` holding `journal.db` redb lock
  and both reserved listener ports.
  action: `camel job ping` in the same directory with the same ambient
  config.
  assert: job exit code 0; stdout is a JSON report with outcome
  `Completed`; the server process stayed alive throughout (lock-based
  proof: any journal open or port bind attempt by the job would fail
  boot with exit 2).
  command: `cargo test -p camel-cli --test job_coexistence_test -- --test-threads=1`
  expected: exit-2 failure before task 1; passes after.

Acceptance:
- The test passes; `--test-threads=1` keeps journal/port fixtures
  isolated.
- Re-running the binary 3x consecutively stays green (no port-reuse
  flake).
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 3-coexistence-e2e

## 4. concurrent-jobs-and-malformed — overlap + fail-loud tests

Implements `cli-jobs` scenarios "two concurrent jobs on one pod contend
on nothing projected" and "malformed ambient config still fails loud".

Files:
- `crates/camel-cli/tests/job_coexistence_test.rs` (modified — appended
  tests sharing task 3's fixture helpers)

Steps:
1. Add fixture helper `write_delay_job(dir)`: a job document
   `jobs/slow.job.yaml` targeting `seda:work` whose route holds each
   exchange ~2s (a worker pipeline that delays before a `log:` sink),
   `timeout` 60s, plus the `seda:work` consumer route in
   `routes/slow.yaml`.
2. Append the two tests below.
3. Run; both must fail appropriately before task 1 lands (first: exit 2
   on listener/journal conflict; second: unchanged pass — malformed
   config fails at load regardless of projection) and pass after.

Tests:
- name: `two_concurrent_jobs_share_ambient_config`
  setup: fixture with prometheus+health enabled on reserved ports,
  `[runtime_journal]` present, repositories left at in-memory defaults;
  the delay job above.
  action: spawn two `camel job slow` processes back-to-back (gap well
  under the 2s route delay so executions provably overlap); wait for
  both.
  assert: both exit 0; both stdouts parse as JSON reports with outcome
  `Completed`; neither process opened the journal or bound a listener
  (proved by exit 0 while the config enables them).
  command: `cargo test -p camel-cli --test job_coexistence_test two_concurrent -- --test-threads=1`
  expected: exit-2 failures before task 1; both pass after.
- name: `malformed_ambient_config_fails_loud`
  setup: fixture dir whose `Camel.toml` carries
  `[runtime_journal]` with `durability = "bogus"` (unknown enum variant
  at load).
  action: `camel job ping` in that directory.
  assert: exit code 2, stderr carries the existing configuration
  diagnostic; no boot, no report.
  command: `cargo test -p camel-cli --test job_coexistence_test malformed -- --test-threads=1`
  expected: passes before AND after task 1 (parse/validate happens
  before projection; pins fail-loud ordering).

Acceptance:
- All three tests in the binary pass together:
  `cargo test -p camel-cli --test job_coexistence_test -- --test-threads=1`.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 4-concurrent-jobs-and-malformed

## 5. compiled-artifact-tests — job artifact manifest + runtime proof

Implements `cli-compile` scenarios "Embedded job artifact binds no
diagnostic listeners", "Job artifact manifest omits suppressed
listeners", and "Route artifact manifest keeps config-declared listeners"
end to end.

Files:
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified — append
  tests beside the existing `--manifest` runs, e.g.
  `artifact_manifest_exits_without_boot`; reuse its
  `common::run_binary`/deploy fixtures)

Steps:
1. Reuse the existing compiled-artifact test harness in
   `crates/camel-cli/tests/compiled_artifact_test.rs` (binary invocation,
   tempdir, JSON parsing helpers).
2. Fixture: a job document + `Camel.toml` enabling
   `[observability.prometheus]` and `[observability.health]` on two
   reserved ports with `enabled = true`, plus `[runtime_journal]` with a
   tempdir path.
3. Compile a job artifact (`camel compile <doc.job.yaml> --config
   <config> -o <artifact>`), run `--manifest`, parse JSON.
4. Compile a route artifact from the same config, run `--manifest`.
5. Runtime proof with NO release/re-bind window (ADR-0070): bind both
   listeners (`TcpListener::bind` on `127.0.0.1:0`), read the two ports,
   write them into the fixture config, compile the artifact, and HOLD
   the listeners through the entire artifact execution; exit 0 + report
   proves the artifact never tried to bind.

Tests:
- name: `job_artifact_manifest_omits_suppressed_listeners`
  setup: compiled job artifact with the observability-enabled config.
  action: run `<artifact> --manifest`.
  assert: exit 0; manifest JSON `listeners` contains neither reserved
  `host:port` endpoint.
  command: `cargo test -p camel-cli job_artifact_manifest_omits_suppressed`
  expected: fails before task 2 (listeners listed), passes after.
- name: `job_artifact_binds_no_listeners_with_ports_prebound`
  setup: same artifact; test process pre-binds both configured ports and
  holds the listeners.
  action: execute the artifact (cwd = tempdir, embedded job runs
  one-shot).
  assert: exit 0; stdout/report outcome `Completed`; pre-bound listeners
  still held by the test (never contended).
  command: `cargo test -p camel-cli job_artifact_binds_no_listeners`
  expected: fails before task 1 (projection missing → boot exit 2),
  passes after tasks 1+2.
- name: `route_artifact_manifest_keeps_config_declared_listeners`
  setup: compiled route artifact from the same observability-enabled
  config.
  action: run `<artifact> --manifest`.
  assert: manifest JSON `listeners` contains both endpoints.
  command: `cargo test -p camel-cli route_artifact_manifest_keeps_config`
  expected: passes before and after (pins existing behavior).

Acceptance:
- All three tests pass:
  `cargo test -p camel-cli --test compiled_artifact_test -- --test-threads=1`.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 5-compiled-artifact-tests

## 6. docs-sweep — CONTEXT prose truthfulness

Implements the docs portion of the proposal; verifies the ruling extra
"no doc/test asserts the old exit-2 conflict as expected behavior".

Files:
- `crates/camel-cli/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

Steps:
1. `crates/camel-cli/CONTEXT.md`, the `camel job` command paragraph
   (~line 107): after "It boots the REAL composition root (the
   `camel run` seams: config, security context, bind acks, the
   `camel_bundles` cascade, ambient `${env:}` discovery)" insert the
   projection clause: after the ambient config parses and validates, a
   job boot projection removes the durable runtime journal and replaces
   ambient observability with defaults before context configuration, for
   every job form including compiled artifacts; malformed ambient config
   still fails loud; ambient repositories remain config-driven.
2. `CONTEXT-MAP.md` Key Terms: ADD a new entry "Job boot projection" —
   `camel job` (all forms, including compiled artifacts) parses and
   validates the ambient config, then removes `runtime_journal` and
   replaces `observability` with defaults before context configuration;
   components, repositories, security, platform, beans, supervision,
   timeouts, and log level survive; no flag re-enables the suppressed
   surfaces. Authority: cli-jobs spec (change jobcoexist), bd rc-va2jl
   (e_opus ruling 2026-09-16). Follow the map's entry conventions.
3. Sweep verification: run
   `grep -rn "already open\|Address in use" crates/camel-cli/tests/ crates/camel-cli/src/`
   — the only permitted hit is `BIND_RACE_MARK` ("Address already in use")
   in `tests/job_coexistence_test.rs` (ADR-0070 retry marker from task 3's
   blessed design); any other hit presenting resource-conflict exit 2 as
   expected behavior is stale and must be fixed. Also run
   `grep -rniE "exit.{0,2}2" crates/camel-cli/CONTEXT.md`
   (hits are legitimate exit-taxonomy prose — confirm none presents
   resource-conflict exit 2 as expected job behavior), and
   `grep -n "composition root" crates/camel-cli/CONTEXT.md`. Record the
   judgment in the task's completion note.
4. If any stale claim surfaces, fix it in the same edit.
5. Regression proof (proposal acceptance): run
   `cargo test -p camel-cli --tests` (all integration binaries,
   including `job_one_shot_test`, `job_signal_test`,
   `job_early_failure_test`, `job_coexistence_test`,
   `compiled_artifact_test`) and
   `cargo test -p camel-config --test context_config_test` — both green
   with zero edits under `crates/camel-config/`.

Tests:
- name: `docs-sweep-grep` (manual verification, no test binary)
  setup: tasks 1-5 merged.
  action: the greps in step 3 plus
  `grep -n "composition root" crates/camel-cli/CONTEXT.md`.
  assert: job prose mentions the projection; no text claims jobs inherit
  the journal/observability surfaces or exit 2 on resource conflict.
  command: the greps above.
  expected: clean after steps 1-2.
- name: `regression-family-green` (verification command, not a test fn)
  setup: all code tasks complete.
  action: `cargo test -p camel-cli --tests` and
  `cargo test -p camel-config --test context_config_test`.
  assert: both commands exit 0.
  command: as above.
  expected: green; `camel-config` untouched by the whole change.

Acceptance:
- `cargo xtask lint-context-citations` exits 0 (CONTEXT citations still
  resolve).
- The greps return no stale conflict claims.
- `cargo test -p camel-cli --tests` and
  `cargo test -p camel-config --test context_config_test` both exit 0.

- [x] 6-docs-sweep
