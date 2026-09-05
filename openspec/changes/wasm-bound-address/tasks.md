# Tasks: wasm-bound-address

## Task WASM-1: staged_listener module + consumer consumption

**Files:**
- `crates/components/camel-component-wasm/src/staged_listener.rs` (new)
- `crates/components/camel-component-wasm/src/lib.rs` (modified — add `pub mod staged_listener;`)
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified — staged take at the bind site)
- `crates/components/camel-component-wasm/tests/common/mod.rs` (new — shared helper for this and later tasks)
- `crates/components/camel-component-wasm/tests/staged_listener_source.rs` (new — integration scenarios)
- `crates/components/camel-component-wasm/tests/source_bind_gate.rs` (modified — one new scenario test, appended; no migration in this task)

**Steps:**
1. Create `staged_listener.rs` with
   `static STAGED: Mutex<HashMap<(String, u16), TcpListener>>` (std
   `Mutex`, never held across an await):
   - `pub fn stage_listener(listener: tokio::net::TcpListener) -> Result<(), CamelError>` — read `local_addr()`, insert under exact `(host_string, port)`; on `local_addr()` failure or poisoned lock return `CamelError::EndpointCreationFailed` with a `format!` message naming the cause; on occupied key return `EndpointCreationFailed` with exactly `listener already staged for {h}:{p}` and keep the first listener in the slot.
   - `pub(crate) fn take(bind_addr: std::net::SocketAddr) -> Result<Option<TcpListener>, CamelError>` — exact-key hit → `Some(listener)` removed from the map; same-port different-host-string entry → `Err(EndpointCreationFailed)` with exactly `staged listener conflict on port {p}: staged under host {h}, requested {r}` (slot untouched, socket untouched); miss → `Ok(None)`. Host strings compare as-is (no DNS or wildcard normalization: `0.0.0.0` and `127.0.0.1` are different keys).
2. Export the module from `lib.rs`.
3. In `source_consumer.rs`, replace the direct `tokio::net::TcpListener::bind(bind_addr)` at the bind site (after the 12b operator/guest agreement and the ADR-0061 exposure gate) with: `match staged_listener::take(bind_addr)? { Some(listener) => listener, None => tokio::net::TcpListener::bind(bind_addr).await …? }` — the `?` unwraps `take`'s `Result`, so the arms are `Some`/`None` on the `Option`; the `Err` case (conflict) propagates via `?` (already `EndpointCreationFailed`). The `tracing::info!` bound/stopped lines keep working off `bind_addr`/listener as today.
4. Create `tests/common/mod.rs` with
   `pub async fn stage_wasm_source_listener(host: &str) -> u16` — bind `tokio::net::TcpListener` on `{host}:0`, call `stage_listener`, panic with the error text on staging failure, return the actual port.
5. Create `tests/staged_listener_source.rs` (declares `mod common;`) with the three integration tests below, mirroring the route-boot and webhook-dial patterns already used in `tests/source_integration.rs` and the refusal-assertion pattern from `tests/source_bind_gate.rs`; append the fourth integration test (`refused_route_preserves_staged_slot`) to `tests/source_bind_gate.rs`.

**Tests:**
- `stage_take_exact_key_roundtrip` (lib unit, in `staged_listener.rs`) — setup: empty map, a listener bound on `127.0.0.1:0`; action: `stage_listener(l)` then `take(local_addr)`; assert: `take` returns `Some` with the same local address; second `take` on the now-empty key returns `Ok(None)`. Command: `cargo test -p camel-component-wasm --lib staged_listener`. Expected: fails before step 1-3 exist (compile error), passes after.
- `duplicate_staging_rejected_first_stays` (lib unit) — setup: `std::net::TcpListener::bind("127.0.0.1:0")`, `try_clone()`, `set_nonblocking(true)` on both, `tokio::net::TcpListener::from_std` on both (the camel-component-ws CLONE-FIXTURE pattern, lib.rs:3027–3069; a staged listener is moved into the map, so the same socket must be re-referenced through a clone); action: `stage_listener(first)` then `stage_listener(clone)`; assert: second call is `Err` whose message contains `listener already staged for 127.0.0.1:{p}`; then `take` with the exact key returns `Some` (the first staged listener stayed in the slot). Command: same. Expected: fails before, passes after.
- `wrong_host_take_conflicts_and_preserves` (lib unit) — setup: listener bound on `0.0.0.0:0` (same try_clone pattern not needed — single stage), staged; action: `take` with `{port}` under host `127.0.0.1`; assert: `Err` message contains `staged listener conflict on port {p}: staged under host 0.0.0.0, requested 127.0.0.1`; then `take` with the exact `0.0.0.0:{p}` key returns `Some` (slot untouched by the conflict). Command: same. Expected: fails before, passes after.
- `staged_listener_served_end_to_end` (`tests/staged_listener_source.rs`) — setup: `let port = common::stage_wasm_source_listener("127.0.0.1").await;`, wasm source route with guest config `("bind", format!("127.0.0.1:{port}"))` and webhook `path`, mirroring the fixture-acquisition pattern of `tests/source_integration.rs` (if that pattern is `#[ignore]`-gated on pre-built wasm there, this binary adopts the same gating); action: start the route, send an HTTP POST to `127.0.0.1:{port}/{path}`; assert: the guest receives the exchange (existing testkit assertion pattern from `source_integration.rs`); a fresh bind would have hit `EADDRINUSE` because the staged socket was held until consumption — a served webhook proves the staged socket was consumed. Command: `cargo test -p camel-component-wasm --test staged_listener_source`. Expected: fails before, passes after.
- `unstaged_bind_starts` (same file) — setup: route guest config `("bind", "127.0.0.1:0")` — port zero straight in the config, NO helper, NO port discovery, NO dropped listener (deliberately exercising the unstaged `None` arm without performing a bind-read-drop probe); action: start the route, then stop/cancel it; assert: start succeeds (no staged entry existed, the consumer bound `127.0.0.1:0` itself through the `None` arm) and shutdown completes cleanly. Command: same. Expected: fails before, passes after.
- `wrong_host_staged_start_fails_deterministically` (same file) — setup: `stage_wasm_source_listener("0.0.0.0")` → port `P`; route with guest bind `127.0.0.1:{P}`; action: start the route; assert: start fails with an error containing `staged listener conflict on port {P}: staged under host 0.0.0.0, requested 127.0.0.1`; the `source_bind_gate.rs` refusal-assertion pattern shows how route-start errors are observed. Command: same. Expected: fails before, passes after.
- `refused_route_preserves_staged_slot` (`tests/source_bind_gate.rs`, new test, `#[ignore = "requires pre-built guest wasm (see module docs)"]` like its siblings, using the PRE-BUILT conflicting-bind guest — the standard webhook guest mirrors the operator bind and can never disagree at 12b; the conflicting-bind guest derives its declared bind ONLY from `conflict_port`, which is what makes the disagreement constructible) — setup: `stage_wasm_source_listener("0.0.0.0")` → port `P`; route A: guest config `[("bind", "127.0.0.1:1"), ("conflict_port", P.to_string())]` (the `conflicting_binds_fail_before_socket` shape) → operator bind `127.0.0.1:1` vs guest-declared `0.0.0.0:{P}`, refused at the 12b agreement before the bind site; action: start route A (assert refusal error naming both binds, per the `conflicting_binds_fail_before_socket` assertion pattern); then set `WasmSourceBindAcks::global()` to `{("0.0.0.0:{P}"): true}` and start route B: guest config `[("conflict_port", P.to_string()), ("path", "/webhook")]`, NO `bind` entry — operator bind absent, guest-declared `0.0.0.0:{P}` wins, the ack passes the exposure gate, and the bind site resolves `("0.0.0.0", P)`; assert: route B's `start()` returns `Ok` — the staged socket was held until consumption, so a missed take would have failed the fresh bind with `EADDRINUSE`; `Ok(start)` is itself the consumption proof — and, if the conflicting-bind guest implements the source-world `accept-http` serving path (the worker checks the fixture source at `tests/fixtures/conflicting-bind-guest/`), also POST a webhook to `127.0.0.1:{P}/webhook` and assert guest receipt, satisfying the spec scenario's serving clause end-to-end; if the fixture proves not to serve, the worker STOPS and reports `test-design-gap: conflicting-bind guest lacks accept-http serving` instead of guessing (fixture extension is a spec-visible fallback, not an improvisation). ACK SERIALIZATION: the ack store is process-global and this test mutates it — add a binary-wide `static ACK_TEST_LOCK: tokio::sync::Mutex<()>` at the top of `source_bind_gate.rs`; this test holds the lock across its ack-mutating region, and the two existing ack-mutating siblings (`guest_only_non_loopback_bind_gated`, `non_loopback_public_gate_with_and_without_ack`) acquire the same lock at their ack-mutating phases — the `non_loopback_public_gate_with_and_without_ack` doc comment already states these phases "must not run in parallel with each other", and that latent race also exists BETWEEN those two siblings today (both call `WasmSourceBindAcks::global().set`); the lock closes the class. Command: `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored refused_route_preserves_staged_slot` (needs the pre-built conflicting-bind wasm per module docs; local run optional — CI-deferred if the fixture is absent). Expected: fails before (no staged machinery), passes after.
- Scenario `no-staged-entry-binds-normally` is exercised explicitly by `unstaged_bind_starts` (port-zero config through the `None` arm); the entire existing crate suite additionally exercises it — acceptance re-runs the full suite and it must stay green with zero test edits in this task.

**Acceptance:**
- `cargo test -p camel-component-wasm --lib` exits 0 (existing + new unit tests).
- `cargo test -p camel-component-wasm --test staged_listener_source` exits 0 (3 new tests, subject to the fixture-gating rule above; the 4th integration test lives in `source_bind_gate.rs`).
- `cargo test -p camel-component-wasm --test source_bind_gate` compiles (the new refused-route test is `#[ignore]`-gated with its siblings; full `-- --ignored` validation is CI-deferred per ADR-0054).
- `cargo test -p camel-component-wasm` exits 0 overall — existing binaries untouched and green (unstaged path behaviorally compatible).
- `cargo clippy -p camel-component-wasm --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0.
- `grep -n 'TcpListener::bind' crates/components/camel-component-wasm/src/source_consumer.rs` shows the bind only inside the `None` arm of the staged match.

- [x] WASM-1

## Task WASM-2: migrate the four source test binaries

**Files:**
- `crates/components/camel-component-wasm/tests/source_integration.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_bind_gate.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_auth_e2e.rs` (modified)

**Steps:**
1. Add `mod common;` to each of the four binaries.
2. Replace 28 of the 29 `free_port().await` calls with
   `common::stage_wasm_source_listener("127.0.0.1").await` (source_integration 6, source_stream_integration 7, source_auth_e2e 9, source_bind_gate 6 of 7). Guest-config lines formatting `127.0.0.1:{port}` stay byte-identical. Readiness wait-loops polling `TcpStream::connect(...).is_ok()` (source_integration:174, source_stream_integration:114, source_auth_e2e:222) keep working: the staged socket accepts as soon as it is bound and `axum::serve` drains the backlog on start.
3. The ONE remaining site — `source_bind_gate.rs:246`, `port_b` in `conflicting_binds_fail_before_socket`, whose port feeds an asserted-unbound address (`TcpStream::connect(("127.0.0.1", port_b)).await.is_err()` at :278; a staged listener is bound by definition, so staging would flip the assertion) — becomes the fixed reserved address `127.0.0.1:1`: `let port_b = 1u16;` with a comment naming the reason (assertion requires an unbound address; port 1 is never bound by tests or CI services). The refusal-error assertions naming `operator_bind` keep holding with `127.0.0.1:1`.
4. Delete the four local `async fn free_port()` definitions (one per binary).
5. Verify no probe remains: `grep -rn 'free_port' crates/components/camel-component-wasm/` returns nothing (no calls, no definitions).

**Tests:**
- Migrated suites are the tests — each former probe site (except the one fixed-address conversion) now acquires its port from a listener the helper holds until the route consumes it:
  - `cargo test -p camel-component-wasm --test source_integration` — 6 migrated sites; expected: green, zero test-logic edits beyond the acquisition call.
  - `cargo test -p camel-component-wasm --test source_stream_integration` — 7 migrated sites; expected: green.
  - `cargo test -p camel-component-wasm --test source_auth_e2e` — 9 migrated sites; expected: green.
  - `cargo test -p camel-component-wasm --test source_bind_gate` — 6 migrated sites + 1 fixed-address conversion; the refusal tests (routes that never reach the bind site) leave their staged slots inertly, assertions unchanged; the whole target is `#[ignore]`-gated (ADR-0054) — local run must compile, `-- --ignored` validation is CI-deferred. Expected: compiles green locally.
- `no_port_probes_remain_wasm` — action: `grep -rn 'free_port' crates/components/camel-component-wasm/`; assert: exit code 1 (zero matches — calls AND definitions). Command as written. Expected: fails (matches found) before step 4, passes after.

**Acceptance:**
- The four commands above each exit 0 (compile-level for the gated target).
- `grep -rn 'free_port' crates/components/camel-component-wasm/` exits 1 (zero matches).
- `grep -rn 'stage_wasm_source_listener(' crates/components/camel-component-wasm/tests/ | wc -l` equals 32 (paren-filtered call/def lines; raw string mentions are 36 — 3 helper panic-message lines + 1 doc-comment self-mention): 28 migration call sites + 1 definition in `tests/common/mod.rs` + 3 WASM-1 test calls (2 in `tests/staged_listener_source.rs` — `unstaged_bind_starts` deliberately uses no helper — and 1 in `tests/source_bind_gate.rs`).
- ERRATUM (worker-discovered, r_glm-endorsed): the blanket `"127.0.0.1"` staging rule had one exception the task overlooked — `non_loopback_public_gate_with_and_without_ack` phase 2 binds `0.0.0.0:{port}` WITH ack and reaches the bind site, so it stages under `"0.0.0.0"` (exact host-string keys per design.md). Recorded for verification.md.
- `grep -n 'port_b = 1u16' crates/components/camel-component-wasm/tests/source_bind_gate.rs` matches exactly once, with the explanatory comment present.
- `cargo clippy -p camel-component-wasm --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0.
- Scenario `wasm-staged-port-survives-to-serve` is exercised end-to-end by the migrated webhook tests above (the same socket the helper bound serves the exchange).

- [x] WASM-2

## Task WASM-3: docs, gates, verification

**Files:**
- `crates/components/camel-component-wasm/CONTEXT.md` (modified)
- `openspec/changes/wasm-bound-address/verification.md` (new)

**Steps:**
1. In `CONTEXT.md`, extend the Inbound request posture section after the bind-exposure-gate paragraph with a staged-listener bullet mirroring the camel-component-ws wording landed in rc-h0aw: `stage_listener` parks a pre-bound listener under its exact `(host, port)` key, one-shot; the consumer consults it only at the bind site after the agreement and exposure gate; same-port different-host staging fails deterministically (`staged listener conflict …`); the map is empty by default and a test-tier concern (bd rc-wgba; capability spec `staged-listener-binding`).
2. Run the full gate battery in the worktree: `cargo fmt --check --all`; clippy trio per AGENTS.md (`--workspace --all-features` with the kafka/cli/security exclusions, `-p camel-component-kafka --all-targets`, `-p camel-cli`), all with `-D warnings`; the ten `cargo xtask lint-*` gates; `cargo xtask schema --check`; `cargo audit`; `cargo build --workspace`; `cargo test --workspace --lib`; `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`.
3. Write `verification.md` capturing: every gate exit code, per-binary wasm test counts (lib + staged_listener_source + the 4 migrated binaries + remaining untouched binaries), both no-probe greps (`camel-test` from rc-h0aw still empty; wasm now empty), and the review trail (task reviews, spec/plan blessing hashes).
4. Commit CONTEXT.md + verification.md.

**Tests:**
- `context_citations_clean_after_doc_edit` — action: `cargo xtask lint-context-citations`; assert: exit 0, `0 violations`. Command as written. Expected: passes (the new bullet names bd rc-wgba and the capability spec, both real citations).
- `gates_all_green` — action: run each gate from step 2 as a separate command; assert: every exit code 0 (`cargo audit` allowed-warnings set unchanged at 5). Expected: all green.

**Acceptance:**
- Every gate from step 2 exits 0 (exit codes recorded in `verification.md`).
- `cargo xtask lint-context-citations` exits 0.
- `verification.md` exists with the full evidence table.
- `grep -rn 'free_port' crates/components/camel-component-wasm/` still exits 1 and `grep -rn find_free_port crates/camel-test/` still exits 1 (both halves of the MODIFIED no-port-probes scenario hold).
- CI-deferred items listed explicitly: `source_bind_gate -- --ignored` suite and any fixture-gated tests in `staged_listener_source.rs` (ADR-0054 pre-built wasm).

- [x] WASM-3
