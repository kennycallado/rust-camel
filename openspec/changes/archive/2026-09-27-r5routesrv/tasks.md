# Tasks: r5routesrv

## camel-cli battery (compiled_artifact_test.rs)

### Task 1.1: REST listener serve battery + shared listener helpers

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add the test-local helpers beside the existing harness (plain `fn`s
   in the test file, no new dev-dependencies):
   - `fn free_port() -> u16` — bind `std::net::TcpListener` to
     `127.0.0.1:0`, read the assigned port, drop the listener. This is
     the ADR-0070 SUBPROCESS EXCEPTION (the spawned artifact cannot
     receive an in-process staged listener): the same pattern as
     `job_coexistence_test::reserve_two_ports`, whose module header
     documents the exception. The port-toctou window is closed by the
     retry convention below, not by staging.
   - `const BIND_RACE_MARK: &str = "Address already in use"` — the loud
     Linux signature of the ADR-0070 port-probe race (copy the constant
     and its doc comment from `job_coexistence_test.rs`).
   - Every listener test that RELEASES its probed port before the child
     binds (Tasks 1.1–1.6) wraps its flow in the
     `job_coexistence_test` retry convention: run the whole flow
     (port pick → doc write → compile → deploy → spawn → assertions);
     if the captured output contains `BIND_RACE_MARK`, retry the ENTIRE
     flow exactly once with a fresh port (the race window is
     millisecond-scale; a second collision is not observed in
     practice). Task 1.7 does NOT use this wrapper — there the test
     HOLDS the port, so `Address already in use` is evidence of a
     child bind attempt, not a retriable race (retrying would discard
     the very failure the test must detect).
   - `fn http_get(port: u16, path: &str) -> Option<(u16, String)>` —
     open `std::net::TcpStream` to `127.0.0.1:port` with a 5 s read
     timeout, write a minimal `GET <path> HTTP/1.1\r\nHost: 127.0.0.1:<port>\r\nConnection: close\r\n\r\n`,
     read to EOF, parse the status code from the status line and return
     the full body String (`None` on connect/read failure).
   - `fn rest_listener_doc(port: u16) -> String` — returns the
     probe-validated document (base `path: /api` is REQUIRED — the DSL
     rejects an empty rest base path):
     ```yaml
     routes:
       - id: ping-route
         from: direct:ping
         steps:
           - set_body: "pong"
     rest:
       - host: 127.0.0.1
         port: <port>
         path: /api
         operations:
           - method: GET
             path: /ping
             to: direct:ping
     ```
2. Verify the existing `compile()`/`deploy_artifact()`/`spawn_child`/
   `spawn_drained`/`wait_for_marker`/`wait_exit_code`/`send_signal`
   helpers scrub `CAMEL_*` variables from the child environment (the
   fleet dev shell exports `CAMEL_CXF_BRIDGE_BINARY_PATH`,
   `CAMEL_XML_BRIDGE_BINARY_PATH`, `CAMEL_JMS_BRIDGE_BINARY_PATH`, and
   compile v2 fails closed on any `CAMEL_*` presence). If they do not,
   extend the spawn path with `env_remove` for every `CAMEL_*` key the
   parent carries.
3. Write test `route_server_serves_listener_until_sigterm`: compile the
   rest doc, deploy the artifact, spawn it with `--report <tmp>/r.json`;
   wait for the `context started` marker (60 s); `http_get(port,
   "/api/ping")` must return `Some((200, body))` with body containing
   `pong`; `kill -TERM`; exit code must be 0 (30 s); the report file
   must contain exactly
   `{"kind":"route","status":"completed","error":null}`.
4. Extend the same test (or a sibling assertion block inside it) with
   the manifest check: run the deployed artifact with `--manifest`
   (use `std::process::Command`, capture stdout) and assert the JSON
   `listeners` array contains `"127.0.0.1:<port>"` and
   `artifact_kind == "server"`.

**Tests:**
- `route_server_serves_listener_until_sigterm`: rest-listener artifact deployed without its source tree → started with `--report`, listener queried after `context started` → HTTP 200 with body `pong`, SIGTERM → exit 0 (30 s bound), report file is exactly `{"kind":"route","status":"completed","error":null}`; `--manifest` output lists `127.0.0.1:<port>` and `artifact_kind` `server`.
  - command: `cargo test -p camel-cli --test compiled_artifact_test route_server_serves_listener_until_sigterm`
  - expected: PASSES after this task (behavior exists at HEAD; the test pins it — if it runs RED, STOP and report `test-design-gap: <what failed>` instead of changing production code).

**Acceptance:**
- `cargo test -p camel-cli --test compiled_artifact_test route_server_serves_listener_until_sigterm` exits 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli --all-targets -- -D warnings` clean.

- [x] 1.1

### Task 1.2: In-flight drain battery

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add `fn slow_rest_doc(port: u16, delay_ms: u64) -> String` — same
   shape as `rest_listener_doc` but the route is `from: direct:slow`
   with steps `- log: "slow-enter"` then `- delay: <delay_ms>` then
   `- set_body: "slow-pong"`, and the operation path is `/slow`
   targeting `direct:slow`. The `log:` step is the observable
   request-entry marker.
2. Write test `route_server_drains_inflight_request` — this test OWNS
   the full "First signal drains gracefully and exits 0" scenario: run
   it with `--report <tmp>/drain.json` and assert the report at the
   end. Compile with `delay_ms = 3000` (default drain budget is 10 s —
   `default_drain_timeout_ms` in camel-config — so the request fits the
   budget); spawn; wait `context started`; spawn a `std::thread` running
   `http_get(port, "/api/slow")` that sends its result through a
   channel; then wait for the `slow-enter` marker in the child's
   captured output (5 ms tight poll, 20 s bound) — the request is now
   PROVABLY inside the delayed step, not merely queued at the listener;
   `kill -TERM`; bounded-join the thread via
   `recv_timeout(Duration::from_secs(20))` on the channel; assert the
   response arrived and contains `slow-pong`; assert exit code 0 with
   the standard 30 s `wait_exit_code` bound; assert the report file
   contains exactly
   `{"kind":"route","status":"completed","error":null}`.

**Tests:**
- `route_server_drains_inflight_request`: serving artifact with a 3 s listener route, request in flight → SIGTERM at t≈1.5 s → in-flight response `slow-pong` still delivered, process exits 0.
  - command: `cargo test -p camel-cli --test compiled_artifact_test route_server_drains_inflight_request`
  - expected: PASSES at HEAD (probe-verified 2026-09-26); RED means a real drain regression — STOP and report it, do not weaken assertions.

**Acceptance:**
- Test exits 0; fmt + clippy (same commands as Task 1.1) clean.

- [x] 1.2

### Task 1.3: Boot-buffered signal battery

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write test `route_server_boot_signal_is_buffered`: compile
   `rest_listener_doc`; spawn; wait for the EARLIEST artifact boot
   marker `from the embedded virtual store` (the
   `camel-cli: loading N routes from the embedded virtual store` INFO
   line — probe-verified 2026-09-26 to precede `Starting CamelContext`
   and the whole component-cascade stretch; this mirrors
   `run_signal_test`'s use of the earliest CWD-trust marker) with a
   5 ms tight poll; `kill -TERM` immediately; assert the process did
   NOT die to the default disposition: it exits 0 within 90 s and the
   captured output contains `CamelContext started` (boot completed
   past the marker) and `shutting down`.

**Tests:**
- `route_server_boot_signal_is_buffered`: artifact mid-boot at `Starting CamelContext` → SIGTERM → boot completes, graceful shutdown, exit 0 (not a signal death).
  - command: `cargo test -p camel-cli --test compiled_artifact_test route_server_boot_signal_is_buffered`
  - expected: PASSES at HEAD (drive_lifecycle arms streams pre-boot).

**Acceptance:**
- Test exits 0; fmt + clippy clean.

- [x] 1.3

### Task 1.4: Second-signal force-exit battery

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write test `route_server_second_signal_force_exits`: compile
   `rest_listener_doc`; spawn; wait for the EARLIEST artifact boot
   marker `from the embedded virtual store` (see Task 1.3 — earliest
   marker leaves the whole boot stretch as the delivery window,
   mirroring `run_signal_test`'s early-marker discipline) using the
   SAME tight technique as `run_signal_test`: a 5 ms poll loop (mirror
   `wait_for_marker_tight`) — marker staleness must stay well under
   the boot stretch. Then send the pair as ONE shell invocation —
   `sh -c "kill -INT <pid>; kill -TERM <pid>"` — exactly like
   `run_signal_test::second_sigterm_during_teardown_force_exits`
   (two separate `kill` spawns leave a multi-ms exec gap that can push
   the second signal past teardown under load — do NOT use two
   `send_signal` calls here). Both signals buffer during boot; the
   shutdown select consumes the first, the force-exit guard polls the
   already-queued second; assert exit code 1 within 90 s and the
   captured output contains `forcing exit`.

**Tests:**
- `route_server_second_signal_force_exits`: buffered INT+TERM pair delivered around the mid-boot marker → exit 1 with the `forcing exit` WARN, no wait for teardown completion.
  - command: `cargo test -p camel-cli --test compiled_artifact_test route_server_second_signal_force_exits`
  - expected: PASSES at HEAD (mechanism is drive_lifecycle's rc-kz85m guard).

**Acceptance:**
- Test exits 0; fmt + clippy clean.

- [x] 1.4

### Task 1.5: Deployment-equivalence battery (camel run posture)

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write test `route_server_matches_camel_run_deployment_posture`:
   allocate a SEPARATE `free_port()` per leg (camel-http binds without
   `SO_REUSEADDR` and `Connection: close` leaves server-side TIME_WAIT
   on the port — reusing one port across legs risks `EADDRINUSE`
   false-reds; behavioral equivalence does not require byte-identical
   docs). In one fixture dir write `rest.yaml` (leg A) and
   `rest2.yaml` (leg B) from `rest_listener_doc` with their own ports.
   - Leg A (artifact): compile `rest.yaml` + deploy + spawn the
     artifact; wait `context started`; `http_get` → 200/`pong`;
     `kill -TERM`; exit 0.
   - Leg B (camel run): spawn the binary at
     `env!("CARGO_BIN_EXE_camel")` (the canonical, guaranteed-fresh
     path — harness precedent `common::spawn_camel_run`) with args
     `run --routes rest2.yaml --no-watch`, current_dir = fixture dir,
     `CAMEL_*` scrubbed; wait `context started`; `http_get` on leg B's
     port → 200/`pong`; `kill -TERM`; exit 0.
   - Assert both legs observed identical serve + exit behavior.

**Tests:**
- `route_server_matches_camel_run_deployment_posture`: same listener doc via compiled artifact and `camel run --routes <doc> --no-watch` → both serve 200 `pong` before SIGTERM and exit 0 after it.
  - command: `cargo test -p camel-cli --test compiled_artifact_test route_server_matches_camel_run_deployment_posture`
  - expected: PASSES at HEAD (probe-verified both legs 2026-09-26).

**Acceptance:**
- Test exits 0; fmt + clippy clean.

- [x] 1.5

### Task 1.6: Job artifact boundedness battery

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add fixture builder `fn job_doc_with_direct_route()` writing into a
   temp dir: `route.yaml` with `routes: [{id: ping-route, from:
   direct:ping, steps: [{set_body: "pong"}]}]` (job-safety allowlist is
   `{direct, seda, log, mock}` — do NOT use timer or rest here) and
   `job.job.yaml` with `execute: {mode: one-shot, send: {to:
   direct:ping, body: hi}, timeout: 10s}` plus `routeFiles:
   [route.yaml]`.
2. Write test `job_artifact_exits_without_signal`: compile the job doc,
   deploy, spawn with NO signal ever sent; assert the process exits by
   itself within 120 s with code 0 and stdout carries
   `"outcome": "Completed"` (the existing job report JSON).

**Tests:**
- `job_artifact_exits_without_signal`: compiled job artifact, no signal → self-terminating completion, exit 0, outcome `Completed` (never serves).
  - command: `cargo test -p camel-cli --test compiled_artifact_test job_artifact_exits_without_signal`
  - expected: PASSES at HEAD (probe-verified).

**Acceptance:**
- Test exits 0; fmt + clippy clean.

- [x] 1.6

### Task 1.7: Envelope-before-bind battery

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write test `envelope_corruption_binds_no_listener`: using the
   existing `compile_signed(dir, doc, artifact, seed, /*require:*/
   true)` helper (it produces the `.sig` envelope beside the artifact)
   compile `rest_listener_doc(port)`; corrupt ONLY the envelope: flip
   one byte near the middle of the `.sig` file (the artifact trailer
   stays valid so `decode_artifact` succeeds and boot verification is
   the failing step — `verify_envelope_bytes` path); deploy the pair
   with the existing `deploy_signed(artifact)` helper (deploys the
   artifact plus a mutable envelope copy — reuse it, do not
   re-implement). Port witness (the established non-binding proof,
   precedent `job_artifact_binds_no_listeners_with_ports_prebound`):
   BEFORE spawning, bind `TcpListener` on the declared listener port
   in the TEST process and HOLD it across the child's entire execution
   — a child that ever attempted the bind would fail with
   `Address already in use` (a bind-race diagnostic, not the signature
   diagnostic), so "never bound" is distinguished from "bound, then
   closed". Run the deployed artifact via `common::run_binary`
   (envelope-failure precedent: `corrupt_envelope_fails_closed`);
   assert exit code 2, captured stderr contains `compiled artifact
   signature verification failed` and does NOT contain `Address
   already in use`; drop the held listener only after the child exit
   is reaped.

**Tests:**
- `envelope_corruption_binds_no_listener`: required-signature artifact with corrupted envelope bytes, valid trailer → exit 2 naming signature verification, the child never binds the declared port (held-listener witness).
  - command: `cargo test -p camel-cli --test compiled_artifact_test envelope_corruption_binds_no_listener`
  - expected: PASSES at HEAD (verify_for_boot runs before dispatch, hence before any bind).

**Acceptance:**
- Test exits 0; fmt + clippy clean.

- [x] 1.7

## Docs alignment

### Task 2.1: Route-server semantics in compile docs + CONTEXT.md

**Files:**
- `docs/src/cli/compile.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `docs/src/cli/compile.md` under `## Running an artifact`, append
   a short paragraph (docwave 277 style, 4-6 lines): a route artifact
   runs like `camel run --no-watch` — it binds every listener its
   documents/config declare and serves until the first SIGINT/SIGTERM;
   the first signal drains in-flight work inside the configured drain
   budget (`drain_timeout_ms`, default 10 s) then exits 0; a second
   signal during teardown force-exits 1; job artifacts stay bounded and
   never serve. Reference bd rc-zs7au.
2. In `crates/camel-cli/CONTEXT.md` compiled-artifacts paragraph, add
   one sentence after "reuse the existing `camel run` boot, context
   start, signal shutdown, report, and exit handling with watch
   disabled": listener-bearing route documents bind and serve until the
   first stop signal; in-flight listener work drains within
   `drain_timeout_ms`; a second stop signal during teardown force-exits
   1 (rc-zs7au). Do NOT renumber or rewrite existing rows/sentences.

**Tests:**
- Docs build: `cargo doc -p camel-cli --no-deps` not required (camel-cli outside CI doc set) — instead verify the markdown renders no broken intra-doc links by inspection and run `cargo xtask lint-context-citations` (must exit 0).
- `grep -n "rc-zs7au" docs/src/cli/compile.md crates/camel-cli/CONTEXT.md` shows both citations.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo fmt --check --all` unaffected (no Rust changes in this task).

- [x] 2.1
