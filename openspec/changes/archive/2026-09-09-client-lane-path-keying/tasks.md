# Tasks: client-lane-path-keying

## camel-integration-test (adapters/http.rs)

### Task 1.1: Composite lane key in ClientLane

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)

**Steps:**
1. Add a private helper `fn lane_map_key(lane_key: &str, target_path: &str) -> String` that joins the two with `'\x1f'`. Document why the separator is collision-free.
2. In `ClientLane::launch`, move the `ParsedTarget::parse` call before the overflow pre-check. Compose `let map_key = lane_map_key(lane_key, &target.target);` after the parse. Run both overflow checks and the map insert against `map_key`. The spawned exchange carries `map_key` for `fail_lane_entry`.
3. Render the overflow error lane key as `format!("{lane_key} {}", redact_wire_path(&target.target, &secret_keys))`. The raw composite never leaves the module. Keep the pre-dial refusal invariant: the first overflow check still precedes the dial.
4. Change `ClientLane::take` to `fn take(&self, lane_key: &str, uri: &str)`. Parse `uri` with an empty secret-key slice. On parse failure return `None`. On success pop from `lane_map_key(lane_key, &parsed.target)`.
5. Update the module docs of `ClientLane` and the doc comments of `launch`/`take` to state the path-aware composite key and the per-key FIFO canon.

**Tests:**
- Unit test (same `mod tests`): two entries booked under `lane_map_key("K", "/a")` and `lane_map_key("K", "/b")`. `take("K", "http://h/a")` returns the `/a` entry, then `take("K", "http://h/b")` returns the `/b` entry.
- Existing unit test `fail_lane_entry_is_conditional`: insert under `lane_map_key("K", "/x")` and call `take("K", "http://h/x")`.

**Acceptance:** `cargo test -p camel-integration-test --lib adapters::http` passes. No other call site of `take` exists in src.

- [x] task-1.1

### Task 1.2: Router passes the interpolated reference to take

**Files:**
- `crates/camel-integration-test/src/adapters.rs` (modified)

**Steps:**
1. In `PartnerRouter::receive`, change the client-lane probe to `self.client_lane.take(&lane_key, interpolated)`. Keep the server-role adapter lookup on the uncomposed `lane_key`.
2. Update the `receive` doc comment: the client-lane key is the registered key joined with the interpolated path, and a probe miss falls through to the server role unchanged.

**Tests:** covered by Task 1.3 and Task 2.1 batteries.

**Acceptance:** `cargo check -p camel-integration-test --features http` clean.

- [x] task-1.2

## camel-integration-test (tests)

### Task 2.1: Overflow rendering redacts the path half

**Files:**
- `crates/camel-integration-test/tests/http_client_lane_test.rs` (modified)

**Steps:**
1. Verify `lane_fifo_overflow_is_apparatus` still passes unchanged (the rendered lane key contains the full declared URI as prefix of the rendered form). If the space-joined render breaks the `contains` assertion, relax it to assert both the declared key and the path separately.
2. Add `lane_fifo_overflow_redacts_secret_query`: a permissive partner, the router's secret set set to `["authPassword"]` through `set_secret_query_keys`. Send 64 times to `http://{bound}/orders?authPassword=sekrit`, then the 65th send fails `LaneFifoOverflow`. Assert the rendered error contains `authPassword=***` and does not contain `sekrit`.

**Tests:** the two tests above.

**Acceptance:** `cargo test -p camel-integration-test --test http_client_lane_test --features http` passes.

- [x] task-2.1

### Task 2.2: Invert the path-blind characterization

**Files:**
- `crates/camel-integration-test/tests/http_partner_scripting_test.rs` (modified)

**Steps:**
1. Rewrite `OLDEST_FIRST_DOC` into `PATH_AWARE_DOC`: same two dynamic-ref sends to `/a` (`a-ok`) and `/b` (`b-ok`), same crossed receives (`/b` first, then `/a`). The validates flip: `lastReceived: 'http://${MOCK}/b'` expects `contains: b-ok`, `lastReceived: 'http://${MOCK}/a'` expects `contains: a-ok`.
2. Rename `standalone_roundtrip_receives_match_oldest_first_path_blind` to `standalone_roundtrip_receives_drain_their_own_path`. Rewrite both doc comments (the `OLDEST_FIRST_DOC` block and the test) to state the path-aware contract: each receive drains its own path's parked roundtrip, crossed order proves no oldest-first cross-match.

**Tests:** the renamed test asserts verdict `Pass`, both sends recorded on the wire (`/a` then `/b`).

**Acceptance:** `cargo test -p camel-integration-test --test http_partner_scripting_test --features http standalone_roundtrip` passes.

- [x] task-2.2

## Docs and spec

### Task 3.1: Prose sweep for path-blind references

**Files:**
- `docs/src/testing/index.md` (modified, around the rc-cr5yf mention at line 348)
- `crates/camel-integration-test/CONTEXT.md` (modified, the "Partner receives resolve the wire role by dispatch state" note)

**Steps:**
1. In `docs/src/testing/index.md`, update the sentence that defers path-aware client-role keying to bd rc-cr5yf. State the new contract: client-role parking keys on registered key plus wire path, per-path FIFO oldest-first.
2. In `crates/camel-integration-test/CONTEXT.md`, update the dispatch-state note: the client role parks per endpoint URI path (bounded FIFO per key+path), no longer "one response in flight per endpoint URI".

**Acceptance:** `rg -n "path-blind|rc-cr5yf" docs/ crates/` leaves no live-prose claim that client-role parking IS path-blind. Descriptive references to the rejected path-blind behavior in test comments and fresh bd rc-cr5yf citations are allowed.

- [x] task-3.1

## Gates

### Task 4.1: Full battery and quality gates

**Steps:**
1. `cargo fmt --all` in the worktree.
2. `cargo clippy -p camel-integration-test --all-targets --all-features -- -D warnings`.
3. `cargo test -p camel-integration-test --all-features` (full crate battery: ps97b per-path receive regression, rc-qogy FIFO canon, expectReply pairing, receive-first-then-take).
4. `cargo xtask lint-unwrap`, `lint-log-levels`, `lint-ignore` over the touched files.

**Acceptance:** all commands pass in the worktree. Zero warnings.

- [x] task-4.1
