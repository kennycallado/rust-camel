# Tasks: bound-bare-recv-waits

Verification baseline: `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait`
prints the workspace unadjudicated finding count (548 at branch start). Every
task below states the exact count expected AFTER it lands. REVISED totals:
117 sites (100 single-line + 17 multi-line found by the inter-phase-2 review);
final ratchet 548 → 431. A mismatch (either
direction) means stop and report — do not proceed. Tasks are strictly
sequential 1.1 → 4.4: each count is valid only if every prior-numbered task
has already landed (single worker in the worktree at a time).

Common recipe (applies to every task; kind per design.md §Family shapes):

- **A (single receive in test body)** — wrap:
  `timeout(Duration::from_secs(2), rx.recv()).await.expect("<original message> within 2s").expect("<channel> alive");`
  Import `std::time::Duration` and `tokio::time::timeout` at the test-module
  top if not already imported (qualified `tokio::time::timeout` if adding an
  import would conflict). If the site held `let x = rx.recv().await;` followed
  by `assert!`/`match`, keep that logic operating on the unwrapped
  `Ok(Ok(v))` value; `Err(_)` (elapsed) must panic with a message naming the
  wait and deadline. If the site discarded the result (`let _ =`), replace
  with the full double-expect chain (the discard was hiding a hang).
- **B (while-let drain directly in test body)** — convert to
  `loop { match timeout(Duration::from_secs(2), rx.recv()).await { Ok(Ok(v)) => { <original body> }, Ok(Err(_)) => break, Err(_) => panic!("<channel> drain stalled past 2s") } }`.
- **D (receive inside `tokio::spawn(async move { .. })`)** — convert the
  inner drain to a per-iteration deadline loop:
  `loop { match timeout(Duration::from_secs(2), rx.recv()).await { Ok(Ok(v)) => { <original body> }, Ok(Err(_)) => break, Err(_) => break } }`
  (silent `break` on stall — the drainer is infrastructure; see design.md).
  Preserve any pre-existing `expect` chain INSIDE the task by wrapping it:
  `timeout(Duration::from_secs(2), gate.recv()).await.expect("<gate> within 2s").expect("<gate> channel alive");`
- Deadline: `from_secs(2)` default; `from_secs(5)` only when the enclosing
  test already waits multi-second on the same external dependency.
- Convert sites in a file bottom-up (highest line first) so earlier
  inventory line numbers stay valid. Locate sites by signature + enclosing
  test name (inventory.md), not line alone.
- After edits: `cargo fmt`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  the task's test command, and the fleet lint binary for the count check.

## Phase 1: core test idiom establishment

### camel-core

#### Task 1.1: bound crash-notification receive in consumer_management tests
**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)

**Steps:**
1. Site A @1186: `let notification = crash_rx.recv().await.expect("crash notification expected");` — wrap with the A-recipe (message "crash notification expected within 2s", inner "crash channel alive").
2. Run fmt + clippy + tests + lint count.

**Tests:**
- Existing suite: `cargo test -p camel-core --lib lifecycle::adapters::consumer_management` → all pass (timeout path compiles; happy path unchanged).
- `('/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait' output count) == 547`.

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib consumer_management` passes.
- Fleet lint prints count 547.

- [x] 1.1

#### Task 1.2: bound controller-actor command receives
**Files:**
- `crates/camel-core/src/lifecycle/adapters/controller_actor.rs` (modified)

**Steps:**
1. Seven A sites @621 (`command should be received`), @910 (`stop command`), @924 (`exists command`), @938 (`hash command`), @961 (`route_count command`), @975 (`stop command`), @989 (`hash command`) — wrap each with the A-recipe, preserving each original expect message.
2. Bottom-up order: 989, 975, 961, 938, 924, 910, 621.

**Tests:**
- Existing suite: `cargo test -p camel-core --lib lifecycle::adapters::controller_actor` → all pass.
- Fleet lint count == 540.

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib controller_actor` passes.
- Fleet lint prints count 540.

- [x] 1.2

#### Task 1.3: bound drainclaim emission receives
**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs` (modified)

**Steps:**
1. Three A sites @486 (`first emission`), @487 (`second emission`), @560 (`aggregated emission`) — wrap each with the A-recipe. This file already imports `timeout`/`Duration` and uses the idiom elsewhere (line ~266); match that style exactly.
2. Bottom-up: 560, 487, 486.

**Tests:**
- Existing suite: `cargo test -p camel-core --lib lifecycle::adapters::route_controller_drainclaim` → all pass.
- Fleet lint count == 537.

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib route_controller_drainclaim` passes.
- Fleet lint prints count 537.

- [x] 1.3

### camel-processor

#### Task 1.4: bound resequencer capture receives (discard sites)
**Files:**
- `crates/camel-processor/src/resequencer/mod.rs` (modified)

**Steps:**
1. Three A sites @785, @786, @812 — all `let _ = capture_rx.recv().await;` discards. Replace each with the full double-expect chain (A-recipe, message "resequencer capture within 2s", inner "capture channel alive"; keep distinct per-site messages where the surrounding test differs: @785/@786 are consecutive "first/second capture", @812 is the shutdown-path capture).
2. Bottom-up: 812, 786, 785.

**Tests:**
- Existing suite: `cargo test -p camel-processor --lib resequencer` → all pass.
- Fleet lint count == 534.

**Acceptance:**
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib resequencer` passes.
- Fleet lint prints count 534.

- [x] 1.4

### camel-component-api

#### Task 1.5: bound consumer-claim test receives
**Files:**
- `crates/components/camel-component-api/src/consumer_claim_tests.rs` (modified)

**Steps:**
1. Three A sites @37, @51, @92 — `let envelope = rx.recv().await.expect("envelope must arrive");` — wrap each with the A-recipe preserving the message.
2. Bottom-up: 92, 51, 37.

**Tests:**
- Existing suite: `cargo test -p camel-component-api --lib consumer_claim` → all pass.
- Fleet lint count == 531.

**Acceptance:**
- `cargo clippy -p camel-component-api --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-component-api --lib consumer_claim` passes.
- Fleet lint prints count 531.

- [x] 1.5

## Phase 2: grpc cluster (families A/D)

### camel-component-grpc

#### Task 2.1: bound server.rs unit-test receives (8 A + 4 D)
**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified)

**Steps:**
1. A sites (8): @1122 and @1185 are `match reply_rx.recv().await {` in the test body — restructure to `match timeout(Duration::from_secs(2), reply_rx.recv()).await { Ok(Ok(v)) => <original arms>, Ok(Err(_)) => panic!("reply channel closed"), Err(_) => panic!("reply within 2s") }` keeping each original arm's logic. @1409, @1435, @1460, @1485, @1516 are `let envelope = rx.recv().await.unwrap();` in the test body — wrap with A-recipe (`.expect("envelope within 2s").expect("route channel alive")`). @1554 is `let received = rx.recv().await; assert!(received.is_some());` — wrap and keep the is-some assertion on the unwrapped value: `let received = timeout(..).await.expect("stream receive within 2s"); assert!(received.is_some(), "stream channel closed");`.
2. D sites (4): @1734, @1774, @1811, @1910 — `let envelope = rx.recv().await.unwrap();` receives INSIDE spawned pipeline tasks — wrap each with the A-recipe inside the task, preserving the unwrap's failure semantics as `expect("envelope within 2s").expect("route channel alive")`.
3. Bottom-up: 1910, 1811, 1774, 1734, 1554, 1516, 1485, 1460, 1435, 1409, 1185, 1122.

**Tests:**
- Existing suite: `cargo test -p camel-component-grpc --lib server` → all pass.
- Fleet lint count == 519.

**Acceptance:**
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-component-grpc --lib` passes.
- Fleet lint prints count 519.

- [x] 2.1

#### Task 2.2: bound integration.rs pipeline simulators (13 D)
**Files:**
- `crates/components/camel-component-grpc/tests/integration.rs` (modified)

**Steps:**
1. D drain sites (while-let or if-let inside spawned tasks): @260, @432, @496, @570, @604, @693, @764, @849, @922, @1567, @1644, @1746 — apply the D per-iteration recipe. @432, @693, @764 are one-line drainers `tokio::spawn(async move { while let Some(_envelope) = route_rx.recv().await {} });` — convert to the bounded loop inside the same spawn. @260/@496/@849/@922/@1644 are `if let` single receives inside spawned responders: wrap the receive with timeout; on `Err(_)` (elapsed) or `Ok(Err(_))` (closed) skip responding exactly as the absent-message branch does today — the responder task simply ends.
2. @1753 (EXCEPTION, test-alive gate inside the @1746 task): `release_rx.recv().await.expect("test alive");` — wrap with the A-recipe inside the task preserving the expect chain (message "release gate within 2s", inner "gate channel alive"). Note @1746's own drain is fixed in step 1; @1753 is a distinct nested receive.
3. Bottom-up: 1753 (before 1746's loop conversion, since 1753 sits inside it), then 1746, 1644, 1567, 922, 849, 764, 693, 604, 570, 496, 432, 260.

**Tests:**
- Compile + unit-filtered run: `cargo test -p camel-component-grpc --test integration --no-run` compiles; tests that bind localhost listeners run: `cargo test -p camel-component-grpc --test integration` — if any test requires external infra (Docker), report which and defer that test to CI; the conversion itself must be compile-clean.
- Fleet lint count == 506.

**Acceptance:**
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-component-grpc --test integration --no-run` succeeds.
- Fleet lint prints count 506.

- [x] 2.2

#### Task 2.3: bound server_auth_test receives (2 D)
**Files:**
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (modified)

**Steps:**
1. Two sites @262, @308: `let envelope = route_rx.recv().await.expect("exchange reaches route");` inside spawned route tasks — wrap with the A-recipe inside the task (expect chain preserved: "exchange reaches route within 2s" / "route channel alive").
2. Bottom-up: 308, 262.

**Tests:**
- `cargo test -p camel-component-grpc --test server_auth_test` → passes (binds localhost listeners; no Docker).
- Fleet lint count == 504.

**Acceptance:**
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 504.

- [x] 2.3

#### Task 2.4: bound multi-line recv sites missed by the original inventory (scope correction)
**Files:**
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)

**Steps:**
1. server_auth_test.rs D sites (multi-line shape: the receiver expression spans several lines) @398, @451, @510, @649: `let envelope = route_rx` then `.recv().await.expect("<msg>")` chained below, inside spawned route tasks — wrap the receive with the A-recipe in-task, preserving each site's original expect message verbatim, appending " within 2s", adding the inner "<route> channel alive" expect. mpsc recv returns Option: the double-expect unwraps Elapsed then Option.
2. route_controller_tests.rs @4473 (A, multi-line): single receive in a test body — A-recipe preserving the original message.
3. consumer_management.rs @1101 (A, multi-line): single receive in a test body — A-recipe preserving the original message.
4. Bottom-up within each file.

**Tests:**
- `cargo test -p camel-component-grpc --test server_auth_test` → all pass (incl. the denial/streaming regression tests).
- `cargo test -p camel-core --lib route_controller` → all pass. `cargo test -p camel-core --lib consumer_management` → all pass.
- Fleet lint count == 498.

**Acceptance:**
- `cargo clippy -p camel-component-grpc -p camel-core --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 498.

- [x] 2.4

## Phase 3: container-backed I/O components

### camel-sql

#### Task 3.1: bound sql consumer pipeline simulators (14 D)
**Files:**
- `crates/components/camel-sql/src/consumer.rs` (modified)

**Steps:**
1. Fourteen D sites @776, @821, @867, @922, @1007, @1042, @1095, @1141, @1180, @1243, @1283, @1399, @1561 (while-let drains inside `tokio::spawn(async move {..})` simulators) and @1324 (`if let` inside a spawned task) — apply the D per-iteration recipe; @1324's if-let becomes a match with elapsed→skip as in Task 2.2 step 1.
2. These tests use `sqlite::memory:` — 2s deadline.
3. Bottom-up: 1561, 1399, 1324, 1283, 1243, 1180, 1141, 1095, 1042, 1007, 922, 867, 821, 776.

**Tests:**
- Existing suite: `cargo test -p camel-component-sql --lib consumer` → all pass (sqlite in-memory; runs locally).
- Fleet lint count == 484.

**Acceptance:**
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-component-sql --lib` passes.
- Fleet lint prints count 484.

- [x] 3.1

### camel-kafka

#### Task 3.2: bound kafka consumer mock-pipeline receives (6 D)
**Files:**
- `crates/components/camel-kafka/src/consumer.rs` (modified)

**Steps:**
1. Six D sites @1230, @1469, @1529, @1572, @1618, @1738 — while-let drains inside spawned mock downstream tasks — apply the D per-iteration recipe. Unit tests use mock runtimes (no broker); 2s deadline.
2. Bottom-up: 1738, 1618, 1572, 1529, 1469, 1230.

**Tests:**
- Existing suite: `cargo test -p camel-component-kafka --lib consumer` → all pass.
- Fleet lint count == 478.

**Acceptance:**
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 478.

- [x] 3.2

#### Task 3.3: bound manual_commit receives (1 A + 2 D)
**Files:**
- `crates/components/camel-kafka/src/manual_commit.rs` (modified)

**Steps:**
1. A site @139: `let req = rx.recv().await.unwrap();` — wrap with A-recipe ("manual-commit request within 2s" / "commit channel alive").
2. D sites @160, @186: `if let Some(req) = rx.recv().await {` inside spawned tasks — wrap per Task 2.2 step 1 if-let shape.
3. Bottom-up: 186, 160, 139.

**Tests:**
- Existing suite: `cargo test -p camel-component-kafka --lib manual_commit` → all pass.
- Fleet lint count == 475.

**Acceptance:**
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 475.

- [x] 3.3

## Phase 4: remaining components + ratchet closeout

#### Task 4.1: bound seda and direct D sites (14 D)
**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)
- `crates/components/camel-direct/src/direct_tests.rs` (modified)

**Steps:**
1. seda @2240, @2367, @2973 (while-let drains inside spawned tasks — D) — D per-iteration recipe (silent break on stall). @3221, @3278, @3286, @3353, @3360 (single expects INSIDE spawned gate/responder tasks, marked D): wrap with the A-recipe inside the task PRESERVING THE BINDINGS — `let held = timeout(Duration::from_secs(2), route_rx.recv()).await.expect("pipeline receives exchange within 2s").expect("route channel alive");` (analogously `let e1` @3353, subscriber messages @3278/@3286). These bindings are consumed by a later `drop(held)` / `drop(e1)` that releases the in-flight claim; @3360 sits inside an existing `for _ in 0..2` loop — keep the loop bound unchanged and replace only the receive with the double-expect chain. Do not remove any `drop(...)` and do not alter loop iteration counts: the in-flight-claim assertions in the test body depend on the held value living across the release gate.
2. direct_tests @285, @334, @622, @781, @838, @1364 — while-let drains inside spawned route tasks (D) — D per-iteration recipe.
3. Bottom-up within each file.

**Tests:**
- `cargo test -p camel-component-seda --lib` and `cargo test -p camel-component-direct --lib` → all pass.
- Fleet lint count == 461.

**Acceptance:**
- `cargo clippy -p camel-component-seda -p camel-component-direct --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 461.

- [x] 4.1

#### Task 4.2: bound http, ws, timer sites (7 D + 1 B)
**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-ws/src/lib.rs` (modified)
- `crates/components/camel-timer/src/lib.rs` (modified)

**Steps:**
1. http @8318 (while-let drain INSIDE a spawned task — D), @11686/@11757/@11915 (if-let receives inside spawned tasks — D), @11731/@11859 (`while get_rx.recv().await.is_some() {}` drains inside spawned tasks) — apply D recipe (the `is_some` drains become `loop { match timeout(..) { Ok(Ok(_)) => continue, Ok(Err(_)) => break, Err(_) => break } }`).
2. ws @2620: if-let inside spawned route task (D) — Task 2.2 if-let shape.
3. timer @531: the single B site — `while let Some(envelope) = rx.recv().await {` DIRECTLY in test body — apply the B recipe exactly (panic on stall, break only on close).
4. Bottom-up within each file.

**Tests:**
- `cargo test -p camel-component-http --lib`, `cargo test -p camel-component-ws --lib`, `cargo test -p camel-component-timer --lib` → all pass.
- Fleet lint count == 453.

**Acceptance:**
- `cargo clippy -p camel-component-http -p camel-component-ws -p camel-component-timer --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 453.

- [x] 4.2

#### Task 4.3: bound master, mcp, wasm, camel-test, dsl sites (10 A + 12 D)
**Files:**
- `crates/components/camel-master/src/leadership.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs` (modified)
- `crates/components/camel-component-mcp/tests/dsl_e2e_test.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_integration.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (modified)
- `crates/camel-test/tests/mcp_server_auth_test.rs` (modified)
- `crates/camel-test/tests/http_static_test.rs` (modified)
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (modified)

**Steps:**
1. master leadership.rs A sites @498, @539, @574 (unwrap / "envelope must arrive") and @769, @810 ("envelope must arrive") — A-recipe preserving each message. Five sites total (three original + @498/@574 multi-line).
2. mcp server_consumer_test.rs A sites @203 ("route received the exchange"), @726, @760 (multi-line) — A-recipe.
3. mcp server_tool_dispatch_test.rs D sites @197, @372 ("route must receive the invocation") — A-recipe applied inside the spawned task, expect chain preserved.
4. mcp server_consumer_e2e_test.rs D sites @161, @179, @194, @338, @352 — receives inside spawned route tasks; wrap in-task with A-recipe preserving original messages.
5. mcp dsl_e2e_test.rs D sites @202, @220 — same in-task A-recipe.
6. wasm @343 (source_integration) and @537 (source_stream_integration) — D while-let drains inside spawned collector tasks — D per-iteration recipe.
7. camel-test mcp_server_auth @264 (D, "route received the exchange") — A-recipe inside task. camel-test http_static @138 (D while-let inside spawned task) — D recipe.
8. dsl rest_stream_contract_e2e.rs @512 (D while-let) — D recipe.
9. Bottom-up within each file.

**Tests:**
- `cargo test -p camel-master --lib leadership`, `cargo test -p camel-component-mcp --test server_consumer_test --test server_tool_dispatch_test --test server_consumer_e2e_test --test dsl_e2e_test`, `cargo test -p camel-dsl --test rest_stream_contract_e2e` → pass. wasm tests bind localhost listeners — run them; any test requiring external infra: report + defer to CI. camel-test tests are integration-tier (Docker-gated) — compile via `--no-run` if runtime is infra-blocked, and report.
- Fleet lint count == 431.

**Acceptance:**
- `cargo clippy -p camel-master -p camel-component-mcp -p camel-component-wasm -p camel-test -p camel-dsl --all-targets -- -D warnings` exits 0.
- Fleet lint prints count 431.

- [x] 4.3

#### Task 4.4: ratchet closeout
**Files:**
- `scripts/xtask/ratchet-unbounded-wait.max` (modified)

**Steps:**
1. Verify fleet lint count == 431 (all 117 sites from inventory.md bounded). Zero `allow-test-wait` markers are planned: every inventoried site is deadline-bounded, so the delta-spec's marker scenario is not exercised (it governs any site added later; if a conversion proves semantically impossible during implementation, STOP and report rather than marking silently).
2. Rewrite the first non-comment line of `scripts/xtask/ratchet-unbounded-wait.max` from `548` to `431`.
3. Run the fleet lint binary again — must print `lint-unbounded-wait: OK (431 findings = max 431)`.
4. Run `cargo fmt --check --all` and clippy over every affected crate in one command.

**Tests:**
- `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait` → exits 0 with `OK (431 findings = max 431)`.
- `cargo fmt --check --all` exits 0.

**Acceptance:**
- Fleet lint green at 431.
- `cargo fmt --check --all` exits 0.
- No `.recv().await` finding remains for any inventory.md site (fleet lint delta from branch start == 117).

- [x] 4.4
