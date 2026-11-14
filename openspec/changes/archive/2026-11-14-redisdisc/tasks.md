# Tasks: redisdisc

## camel-component-redis — invalidation classifier

### Task 1.1: `invalidates_connection` classifier with subset property test

**Files:**
- `crates/components/camel-redis/src/transport_error.rs` (modified)

**Steps:**
1. Add `pub fn invalidates_connection(err: &CamelError) -> bool` after
   `is_transient_redis_error`, reusing `MAX_SOURCE_HOPS` and
   `unwrap_arc_dyn_error` with the same bounded chain walk:
   - `CamelError::Config(_) | ConfigValidation(_)` → false.
   - `CamelError::Io(_)` → false (transient=true, invalidating=false; its chain
     carries no structural evidence — doc comment must state this).
   - Chain walk: marker `TransportTimeout` → true; markers
     `TransientRetryBudgetExhausted` and `TransientByProse` → false; first
     `redis::RedisError` → delegate to a new private
     `classify_redis_error_for_invalidation`.
   - `classify_redis_error_for_invalidation`: `Server(ReadOnly)` → true;
     `Server(_)` other → false; `ClusterConnectionNotFound` → false; `Io` →
     true iff inner `std::io::Error` kind ∈ {ConnectionRefused,
     ConnectionReset, ConnectionAborted, BrokenPipe} OR RedisError Display
     contains `"connection dropped"`; any other kind → false.
   - No marker, no RedisError → false.
2. Write the doc comment stating the ruled invalidation set and the strict
   subset obligation (ADR-0013 cross-reference).
3. Add the corpus property test + shape tests (below) in the existing
   `mod tests`.

**Tests:** (in `transport_error.rs` `mod tests`)
- `invalidation_shapes_classify_true`: build four errors (marker_camel with
  `TransportTimeout`; `redis_error_to_camel("SET", Server(ReadOnly))`;
  `redis_error_raw(io_redis_error(ConnectionReset))`; `redis_error_raw`
  of an Io-kind RedisError with Display containing "connection dropped") →
  `invalidates_connection` → assert true for each.
- `other_server_replies_never_invalidate`: for NOGROUP/BUSYGROUP/NOPERM
  (parsed via the existing `parsed_server_error` helper), WRONGTYPE, and
  `Server(ResponseError)` text "ERR connection lost while processing" →
  assert `invalidates_connection` false; additionally assert
  `is_transient_redis_error` is true for the fallback-word reply (subset
  one-wayness demonstrated).
- `io_variant_transient_but_not_invalidating`: `CamelError::Io("x".into())` →
  assert `is_transient_redis_error` true AND `invalidates_connection` false.
- `subset_property_holds_over_corpus`: iterate a corpus vec collecting every
  fixture shape the file already builds (all transient fixtures, all fatal
  fixtures, all three markers, config errors, Io variant, plain
  ProcessorError, chained-marker chains at depths 1 and 9, every
  invalidation-positive from the tests above, TLS close-notify text,
  authentication failure) → assert for each:
  `!invalidates_connection(&e) || is_transient_redis_error(&e)`.
- `io_timeout_is_transient_but_not_invalidating`: Io-kind RedisError with
  inner io kind `TimedOut` → transient true, invalidating false (ruled
  exclusion).

**Acceptance:**
- `cargo test -p camel-component-redis --lib transport_error` passes.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- No new `CamelError` variant introduced.

- [x] 1.1

## camel-component-redis — invalidation seam

### Task 2.1: generation-tagged connection cache in `MultiplexedExecutor`

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Change `conn` field to `Arc<Mutex<Option<(u64, MultiplexedConnection)>>>`;
   add shared allocator `next_gen: std::sync::Arc<AtomicU64>` and per-instance
   `last_used_gen: u64` (init 0; `Clone` copies it). Allocator starts so the
   first build stores generation 1 (generation 0 is reserved for unused
   instances): take the gen with `next_gen.fetch_add(1, Ordering::Relaxed)`
   at build time and add 1, or pre-increment — the stored first generation
   MUST be 1. Update the `conn_arc()` test helper's type.
2. Add `pub(crate) async fn checkout(&self) ->
   Result<(u64, MultiplexedConnection), CamelError>`: fast path clones
   `(gen, conn)`; slow path = existing gate → double-check → build flow,
   storing `(gen, new_conn)` with the fresh generation. Keep the connect
   timeout, auth-enrichment, and `Deadline` logic byte-identical.
3. Rewire `pub async fn get_conn` to `self.checkout().await.map(|(_, c)| c)`
   (public signature unchanged).
4. Add `pub(crate) async fn invalidate_if_current(&self, failed_gen: u64)`:
   lock `conn`; `if guard.as_ref().is_some_and(|(g, _)| *g == failed_gen)` →
   `*guard = None`. Never dials.
5. Trait impl `execute_command`: `let (gen, mut conn) = self.checkout().await?;
   self.last_used_gen = gen;` then dispatch; on `Err(err)` →
   `if invalidates_connection(&err) { self.invalidate_if_current(gen).await }
   Err(err)` — rethrow unchanged, no dial, no retry.
6. Trait impl `reconnect`: replace the unconditional clear with the
   generation-aware compare: clear only if the cached generation equals
   `self.last_used_gen`; then `get_conn`. Document the per-instance semantics
   and that production reconnects always follow this instance's failed
   command. `refresh()` keeps its unconditional clear.
7. Update the stale unit test `multiplexed_executor_reconnect_reresolves` to
   the refined contract (see Tests below).
8. Add a comment on `execute_with_retry` noting the invalidation now happens
   inside `execute_command` on first failure regardless of retry policy.

**Tests:** (in `executor.rs` `mod tests`; FIRST extract the
`cached_fast_path_skips_gate` function-local TCP stub into a
`#[cfg(test)] pub(crate) mod test_stub` INSIDE executor.rs so sibling-module
producer tests (task 2.2) import the same harness, with three capabilities:
(a) a per-connection kill handle —
store each accepted stream so the test can drop the socket deterministically
and CONFIRM closure; (b) a reply script — per-connection scripted replies
(OK vs a RESP error reply like `-WRONGTYPE Operation against a key holding
the wrong kind of value`); (c) a command barrier — a notify fired when a
command frame arrives so tests can hold replies until both clones have
checked out. All new tests reuse this shared harness.)
- `killed_connection_invalidated_and_next_command_reconnects`: stub answers
  handshake and one command, then the test drops the stub's socket (kill the
  accepted stream) → `execute_command` #1 returns `Err` (io-kind) → assert
  `conn_arc()` lock is `None` (invalidated) → restart a fresh stub listener on
  a new FakeTopology addr → `execute_command` #2 succeeds and
  `topology.resolve_call_count()` increased (lazy rebuild re-resolved).
- `non_invalidating_error_keeps_cached_connection`: drive
  `execute_command` against a stub scripted to reply `-WRONGTYPE Operation
  against a key holding the wrong kind of value` with a connection already
  cached → assert the error comes back (unchanged text, source preserved)
  AND the cache still holds `Some` with the SAME generation (no invalidation
  on server replies).
- `clone_reconnect_does_not_clobber_fresh_connection`: stub with a command
  barrier holding each reply; create clone A, drive `execute_command` on A
  (gen 1 recorded in A.last_used_gen), create clone B, drive
  `execute_command` on B (B.last_used_gen copied to 1 by the clone and
  re-recorded by its own command). Kill the socket so BOTH in-flight/follow-up
  commands fail with an invalidating io error; await both failures. Drive A's
  `execute_command` again → invalidation cleared (compare-and-clear from A),
  rebuild happens under the gate (gen 2, one new resolve). Drive B's
  `reconnect()` → compare B.last_used_gen (1) vs cached generation (2) → NO
  clear, NO extra resolve. THEN explicitly assert the stale-generation
  negative: `invalidate_if_current(1)` while gen 2 is cached leaves the
  cache `Some` (compare-and-clear never clears a newer generation). Assert:
  exactly ONE new topology resolve since the kill, and B's next
  `execute_command` succeeds against the fresh stub (served the gen-2
  connection).
- `execute_command_then_reconnect_reresolves`: stub OK → `execute_command`
  succeeds (records gen) → `reconnect()` on the SAME instance → resolve
  count incremented (last-used generation matched, cache cleared, rebuilt).
- `blind_reconnect_on_unused_clone_keeps_cache`: create a fresh clone BEFORE
  any command ran on it, with the parent's cache primed by an
  `execute_command` → drive ONLY `reconnect()` on the clone → resolve count
  unchanged (clone's last_used_gen is 0/never-set vs cached generation ≥ 1 →
  mismatch → keeps the cache). This is the updated
  `multiplexed_executor_reconnect_reresolves` (replaces the old
  unconditional-drop expectation).
- `rethrow_original_error_unchanged`: drive `execute_command` against a stub
  that replies with a RESP error reply (`-WRONGTYPE Operation against a key
  holding the wrong kind of value`) → assert the returned error's Display
  equals the wrap-site text and the inner RedisError is preserved as source
  (no reclassification, no extra wrapping).
- Existing tests `refresh_triggers_reresolution_and_failure_is_not_cached`,
  `cached_fast_path_skips_gate` pass unchanged (the latter refactored onto
  the shared harness with identical assertions).
- Coverage note for the MODIFIED producer-failover scenarios the delta
  inherits: the retry+reconnect coupling is pinned by the existing
  `test_retry_succeeds_after_transient_failures`; topology re-resolution by
  `execute_command_then_reconnect_reresolves` above; real sentinel failover
  by `crates/camel-test/tests/redis_sentinel_test.rs`. No new composite fake
  is introduced.

**Acceptance:**
- `cargo test -p camel-component-redis --lib executor` passes.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `pub fn get_conn` and `refresh` signatures unchanged
  (`grep 'pub async fn get_conn' ` shows `&self` → `Result<MultiplexedConnection, CamelError>`).

- [x] 2.1

### Task 2.2: `check_connection` invalidates and preserves the error source

**Files:**
- `crates/components/camel-redis/src/producer.rs` (modified)

**Steps:**
1. Rewrite `check_connection`: `let (gen, mut connection) =
   self.executor.checkout().await?;` (crate-visible seam from task 2.1).
2. On PING failure, build
   `CamelError::ProcessorErrorWithSource(format!("Redis health check PING
   failed for '{}': {}", endpoint, e), Arc::new(e))` — same text as today,
   source chain preserved (this is the classifier-readable flip; note it in a
   comment referencing the e_opus ruling).
3. If `invalidates_connection(&err)` → `self.executor.invalidate_if_current(gen).await`.
   Return `Err(err)`. On success return `Ok(())`.
4. Check `crates/components/camel-redis/src/health.rs` maps the
   `check_connection` error without flattening it further (if it wraps, keep
   the source chain).

**Tests:** (in `producer.rs` `mod tests`; reuse the extended executor TCP
stub harness from task 2.1 including its kill handle)
- `check_connection_invalidates_on_killed_connection`: stub handshake OK →
  first `check_connection` succeeds (cache primed, gen 1) → kill the stub
  socket → second `check_connection` returns `Err` whose Display starts with
  "Redis health check PING failed" and which carries a source (classify
  transient) → assert `executor.conn_arc()` is `None`.
- `check_connection_recovers_after_kill`: same harness, third call after
  pointing the topology at a fresh stub → `Ok(())` and a new resolve.
- Existing `test_check_connection_fails_without_redis` passes unchanged.

**Acceptance:**
- `cargo test -p camel-component-redis --lib producer` passes.
- `cargo fmt --check` clean for the touched files.

- [x] 2.2

## camel-component-redis — retry budget resets

### Task 3.1: queue loop resets on proven pop

**Files:**
- `crates/components/camel-redis/src/queue.rs` (modified)

**Steps:**
1. In `queue_session`'s pop match: `Ok(Some(item))` → reset
   `attempt = 0` before `deliver(item).await`; `Ok(None)` → reset
   `attempt = 0` before `continue`. Comment cites the consecutive-failures
   model (ADR-0013) and the "never reset on connect" rule.
2. Add the two tests below to `queue.rs` `mod tests` (existing
   `FakeQueueIo`/`with_blpop_per_connect` seams).

**Tests:**
- `queue_budget_survives_repeated_recovered_blips`: `FakeQueueIo` with
  `with_blpop_per_connect(vec![vec![Err(io_reset())], vec![item("v1"),
  Err(io_reset())], vec![item("v2"), Err(io_reset())], vec![item("v3")]])`,
  `fast_policy(2)`, bounded by `tokio::time::timeout(Duration::from_secs(5),
  queue_session(...))`, cancel on the third delivery → session Ok, delivered
  `[v1, v2, v3]` in order, `connect_count == 4`.
  Arithmetic: with the reset, each blip's single failure increments to 1
  (`should_retry(1)` true) and the following successful pop resets to 0;
  WITHOUT the reset the second blip's failure reaches attempt 2 →
  `should_retry(2)` false → exhaustion with only `[v1]` delivered. Note the
  batch shape: a success must PRECEDE the error inside a batch —
  `FakeQueueIo::connect` installs the next batch only on successful
  connect, so an `[Err, item]` batch loses the item when the Err forces the
  reconnect.
- `queue_budget_exhausts_on_consecutive_pop_failures`:
  `with_blpop_per_connect(vec![vec![Err(io_reset())], vec![Err(io_reset())],
  vec![Err(io_reset())]])` (one pop failure per connection), `fast_policy(3)`,
  bounded wait → third failure increments to 3 → `should_retry(3)` false →
  session returns `Err` classified by `is_retry_budget_exhausted`,
  `connect_count == 3`. (The pop failures happen on three DIFFERENT
  connections — after each Err the loop reconnects and the next batch
  installs; a "single connect" reading is impossible with per-connect
  batches.)

**Acceptance:**
- `cargo test -p camel-component-redis --lib queue` passes (17 tests incl.
  reviewer-added `queue_blocking_pop_timeout_reset_is_discriminating`,
  commit d3e3bfe4b, which isolates the Ok(None) reset the survival test
  masks).
- Existing queue tests (`queue_recovers_after_blpop_transient_error`,
  `queue_blocking_pop_timeout_keeps_connection`, budget tests) unchanged and
  green.

- [x] 3.1

### Task 3.2: pubsub loop resets on long-lived stream end

**Files:**
- `crates/components/camel-redis/src/pubsub.rs` (modified)

**Steps:**
1. In `pubsub_session`: after `connect_and_subscribe` returns `Ok(())`, stamp
   `let connected_at = tokio::time::Instant::now();` (per session iteration,
   before the deliver loop).
2. In the `None` (stream end) branch: before `attempt += 1`, if
   `connected_at.elapsed() >= policy.max_delay` → `attempt = 0`. Comment: a
   quiet channel delivers nothing, uptime is the only health proof
   (ADR-0013); a short-lived connection must NOT reset.
3. Use the fake's EXISTING per-connect batching API
   `with_messages_per_connect(batches)` (pubsub_tests.rs:102) — no fake
   extension needed. Wrap session calls in
   `tokio::time::timeout(Duration::from_secs(5), ...)` for bounded waiting.

**Tests:**
- `pubsub_budget_survives_long_lived_stream_ends`: policy
  `{ max_attempts: 2, max_delay: Duration::ZERO, initial_delay: 0ms }` (any
  uptime ≥ 0 resets — deterministic),
  `with_messages_per_connect(vec![vec![m1], vec![m2], vec![m3]])` (the API
  takes `Vec<Vec<Msg>>` — messages, not Options; the fake's `next_msg`
  returns the stream-end `None` when the current batch drains while more
  batches remain, tail pends otherwise), bounded by
  `tokio::time::timeout(Duration::from_secs(5), ...)`, cancel on the THIRD
  delivery → session Ok via cancellation, delivered `[m1, m2, m3]`,
  `connects == 3`.
  Arithmetic: each stream end resets (max_delay 0) then counts 1 →
  `should_retry(1)` true → reconnect into the next batch; WITHOUT the reset
  the second stream end reaches attempt 2 → `should_retry(2)` false →
  exhaustion with only `[m1, m2]` delivered. Cancel on delivery three (not
  two) so BOTH stream ends happen — cancelling after the second delivery
  would skip the second stream end and pass without any reset.
- `pubsub_stream_end_before_max_delay_consumes_budget`: policy
  `{ max_attempts: 2, max_delay: Duration::from_secs(3600) }`,
  `with_messages_per_connect(vec![vec![m1], vec![m2], vec![]])` — the EMPTY
  third batch is load-bearing: conn 2 drains with batches still queued, so
  its stream ENDS (None) and the second failure consumes attempt 2 →
  `should_retry(2)` false → session returns `Err` with
  `is_retry_budget_exhausted` true. With only `[[m1], [m2]]` the final tail
  pends instead of ending and the exhaustion never fires.

**Acceptance:**
- `cargo test -p camel-component-redis --lib pubsub` passes.
- Existing pubsub tests (readiness, replay) unchanged and green.

- [x] 3.2

### Task 3.3: streams read task resets at failure-free `BatchDone`

**Files:**
- `crates/components/camel-redis/src/streams.rs` (modified)

**Steps:**
1. In `read_task_loop`: add `let mut attempt_at_read_ok: u32 = u32::MAX;`
   plus a `read_proven: bool` guard. When a read returns
   `Ok(Some(entries))` or `Ok(None)`, record `attempt_at_read_ok = attempt;`
   and `read_proven = true` before sending `Batch`/`NilTick`.
2. In the settle loop's `ReadTaskCmd::BatchDone` arm: before `break`, if
   `read_proven && attempt == attempt_at_read_ok` → `attempt = 0` and reset
   `read_proven = false` (no failure consumed budget since the proven read).
   The initial `BatchDone` before any read must NOT reset (`read_proven`
   false).
3. In `settle_ack`: on a transient ack failure, consume one attempt BEFORE
   reconnecting — mirror `transient_retry_step` semantics: `attempt += 1;`
   then if `!policy.should_retry(attempt)` reply
   `Err(retry_budget_exhausted(policy, "settling stream ack", "stream ack
   failed"))` and return `AckDispatch::Stop`; otherwise proceed to
   `reconnect_after_ack` (whose internal resolve/connect failures consume
   further attempts via the existing `transient_retry_step` calls). This is
   REQUIRED: today an immediately-successful reconnect consumes no attempt,
   so without this the snapshot comparison cannot block the reset and an
   always-failing ack never exhausts (the blessed spec requires both).
   Preserve the no-replay reply behavior byte-identically (reply carries the
   original transient error).
4. Add the four tests below to `stream_session_tests.rs` (drive through
   `stream_session` with `FakeStreamIo`; the read task's counter is
   observable via session outcome and reconnect counts). Wrap every session
   call in `tokio::time::timeout(Duration::from_secs(5), ...)` and end via
   delivery-count-triggered cancellation or `ReadFailed` — no unbounded
   waits.

**Tests:**
- `stream_read_budget_survives_blips_exceeding_max_attempts`: `FakeStreamIo`
  connect outcomes `[Ok, Ok, Ok, Ok]` (4), read queue
  `[batch([e1]), Err(reset), batch([e2]), Err(reset), batch([e3]),
  Err(reset), batch([e4])]`, ack outcomes `[Ok, Ok, Ok, Ok]`, policy
  `max_attempts: 2`. END SEQUENCE: cancel only AFTER the four acks are
  recorded (await the fake's `acks`/`ack_changed` notify until
  `acks.len() == 4`) AND the fifth read has entered (`read_entered` notify —
  the read pends on the drained queue), THEN cancel. Cancelling on the
  fourth DELIVERY would race the fourth ack and leave it unsettled — the
  reset must be observable at a settled `BatchDone`. → session Ok, delivered
  `[e1, e2, e3, e4]`, `acks == 4`, `connects == 4`.
  Arithmetic: read failures N=3 exceed max_attempts=2; with the reset each
  failure lands on a fresh counter (increment to 1, `should_retry(1)` true)
  and every blip reconnects on connect #2/#3/#4; WITHOUT the reset the
  SECOND cumulative failure increments to 2 → `should_retry(2)` false →
  exhausted with only `[e1]` delivered. The literal blessed GIVEN ("WHEN N
  exceeds the configured max_attempts") holds.
- `stream_read_budget_exhausts_on_consecutive_read_failures`: connect
  outcomes `[Ok, Ok, Ok]`, read queue `[Err(reset), Err(reset), Err(reset)]`,
  policy `max_attempts: 3` → session Err, `is_retry_budget_exhausted` true.
- `failed_ack_between_read_and_batch_done_blocks_reset`: connect outcomes
  `[Ok, Ok]`, read queue `[batch([e1]), Err(reset)]`, ack outcomes
  `[Err(reset)]` → settle_ack consumes attempt 1 (new step-3 increment;
  should_retry(1) with max_attempts 2 passes) → `reconnect_after_ack` uses
  connect #2 OK → BatchDone sees `attempt(1) != snapshot(0)` → NO reset →
  next read fails transiently → attempt 2 → `should_retry(2)` false →
  session returns Err via `ReadFailed`. Assert Err bounded (with the reset
  wrongly firing or with no settle_ack increment, the session recovers and
  the test's Ok-expectation twin fails).
- `always_failing_acks_exhaust_budget`: connect outcomes `[Ok]`, read queue
  `[batch([e1]), batch([e2]), batch([e3])]`, ack outcomes all
  `Err(reset)`, policy `max_attempts: 2` → batch 1 ack consumes attempt 1;
  batch 2 ack consumes attempt 2 → `should_retry(2)` false → reply is
  budget-exhausted → session Err with `is_retry_budget_exhausted` true.
  (Before the step-3 change this loop never exhausts — the counter grew
  unchecked.)

**Acceptance:**
- `cargo test -p camel-component-redis --lib streams::tests` passes (the
  tests module is `streams::tests` — a bare `stream_session` filter matches
  zero test names; the four new tests above are all exercised by this
  filter).
- Existing stream session tests (readiness, NOGROUP, cursor persistence)
  unchanged and green (same `streams::tests` filter).

- [x] 3.3

### Task 3.4: streams recovery counter resets after a successful pass

**Files:**
- `crates/components/camel-redis/src/streams.rs` (modified)

**Steps:**
1. In `recovery_pass`'s `Ok(batch)` arm (after `*cursor =
   batch.next_cursor.clone();` and `queue.extend(batch.recovered);`): add
  `*attempt = 0;` before
   `return Ok(RecoveryPass::Completed)`. Comment: a completed `XAUTOCLAIM`
   pass is the proven round trip (claimed entries or not); independent
   counter per ADR-0013.
2. Add the two tests below to `stream_session_tests.rs` (existing
   `FakeRecoveryIo` seam, recover-enabled params).

**Tests:**
- `recovery_budget_survives_recovered_blips`: recover-enabled session,
  recovery outcomes `[Err(reset), Ok(pass, 0 claimed), Err(reset), Ok(pass)]`
  with `recover_min_idle_ms: 1` deadlines, policy `max_attempts: 2`, session
  wrapped in `tokio::time::timeout(Duration::from_secs(5), ...)`. The
  existing recovery handles signal call ENTRY, not completion — extend
  `RecoveryHandles` (or wait on the recorded-pass count) so the test awaits
  the FOURTH recovery call's COMPLETION before cancelling; then cancel and
  join → session survives both blips and ends Ok on cancellation
  (without the reset the second blip exhausts at attempt 2).
- `recovery_budget_exhausts_on_consecutive_pass_failures`: recovery outcomes
  all `Err(reset)`, policy `max_attempts: 2`, bounded wait → session Err
  with `is_retry_budget_exhausted`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib streams::tests` passes (the two
  new recovery tests are exercised by this filter).
- Existing recovery tests (cursor survival, XPENDING summary) unchanged and
  green.

- [x] 3.4

## docs

### Task 4.1: amend ADR-0013 with invalidation rule and budget semantics

**Files:**
- `docs/adr/0013-network-retry-policy-and-migration.md` (modified)

**Steps:**
1. Header metadata: under `Status: Accepted` add
   `Amended: 2026-11 — transport invalidation rule and consecutive-failure
   budget (bd rc-2un5k, rc-n64k3)`.
2. In `## Decision`, append a subsection "Transport invalidation and the
   retry budget" (ASD-STE100 English; two-source rule — this ADR is the
   citation for the code tasks): the invalidation rule (generation-tagged
   cache `Option<(u64, MultiplexedConnection)>`; compare-and-clear inside
   `execute_command`/`check_connection` when `invalidates_connection`
   classifies the failure; never dial in the error path; lazy rebuild under
   the single-flight gate re-resolves the topology; original error rethrown
   unchanged; generation-aware `reconnect` so clones do not clobber fresh
   connections) and the budget model (consecutive failures since the last
   proven round trip, no time window; reset points per loop shape — streams
   `BatchDone` after a failure-free read round trip, recovery after a
   successful `XAUTOCLAIM` pass, queue on pop `Ok(Some)`/`Ok(None)`, pubsub
   on stream end after uptime ≥ `policy.max_delay`; never on connect; failed
   acks block the streams read reset).
3. Note Redis alignment with the Kafka/MQTT/JMS consecutive-failure model.
4. Reference bd `rc-2un5k` / `rc-n64k3` in the Consequences follow-up list.
5. Refresh the ADR index entry in `CONTEXT-MAP.md` (line ~61): extend the
   0013 summary with "amended 2026-11: transport invalidation rule and
   consecutive-failure budget" (match the amendment style used by the 0021
   entry).
6. Add a short note to `crates/components/camel-redis/CONTEXT.md` where the
   retry behavior is described (the transient-retry section around lines
   28-40): the executor invalidates the cached connection on
   `invalidates_connection` failures and the consumer loops measure
   consecutive failures (cite ADR-0013). This crate CONTEXT is the
   citation source for the docs tasks below (two-source rule).

**Tests:**
- None (prose). `nix shell nixpkgs#mdbook -c mdbook build docs` from the
  repo ROOT (book.toml lives at `docs/book.toml`; the path argument is
  root-relative) must exit 0.

**Acceptance:**
- ADR amendment states both rules with no placeholders; mdbook build exits 0.
- Language English; identifiers untouched.

- [x] 4.1

### Task 4.2: supervision docs corrections

**Files:**
- `docs/src/concepts/error-handling.md` (modified)
- `docs/src/configuration/schema.md` (modified)

**Steps:**
1. `error-handling.md` (~line 408): replace "This is Rust API only." with a
   correct statement: supervision is configured from `Camel.toml` via the
   `[supervision]` table (`max_attempts`, `initial_delay_ms`,
   `backoff_multiplier`, `max_delay_ms` — cite
   `configuration/schema.md` and ADR-0013) or programmatically through
   `CamelContext::builder().supervision(config)`. Keep "not a route field" only
   if still accurate — verify against camel-config before keeping the claim;
   cite `crates/camel-config/src/config.rs` (`SupervisionCamelConfig`).
2. `schema.md` `[supervision]` table, `max_attempts` row: extend the
   description — AMENDED DURING APPLY (r_glm verdict on d8dc08d79, code-
   verified): the absent table means supervision OFF (no policy installed —
   `CamelConfig.supervision` is `Option<SupervisionCamelConfig>`, field-level
   serde default → `None`; context builder spawns the supervision task only
   for `Some`); an omitted `max_attempts` with the table declared means
   restart forever (`Option<u32>` → `None`); the old "default 5" text was
   wrong in code. Cite ADR-0007 (the supervision decision ADR), not
   ADR-0013 — the plan's original citation was wrong too.
3. Apply ASD-STE100 rules to both edits (short sentences, active voice).

**Tests:**
- `nix shell nixpkgs#mdbook -c mdbook build docs` (repo root) exits 0.
- `cd docs && mdbook test` exits 0 (doc-guide-test gate; docs changed).

**Acceptance:**
- Both pages state the corrected facts with citations; no "Rust API only"
  claim about supervision remains in `error-handling.md`; mdbook build+test
  exit 0.

- [x] 4.2

## camel-test — integration regression

### Task 5.1: kill-connection regression and health recovery suite

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)

**Steps:**
1. Add a testcontainers-redis test with a DEDICATED container per kill test
   (do NOT use `shared_redis()` — it serves parallel tests and
   `CLIENT KILL TYPE normal` would disconnect their connections too; build
   the container inline like `shared_topology` does, or add a
   `dedicated_redis()` helper). Tests:
   `client_kill_then_next_command_reconnects`:
   - Arrange: start the dedicated container; construct the producer directly
     —
     `RedisProducer::new(RedisEndpointConfig::from_uri("redis://<host>:<port>?command=LPUSH&key=killreg")?,
     Arc::new(NoopRuntimeObservability))` (`new` and `check_connection` are
     `pub`; `camel_component_api::test_support::NoopRuntimeObservability` is
     unconditionally exported — there is no `producer_config` helper in this
     suite). Send one command with a text body ("a") via
     `producer.call(exchange)`, assert Ok; verify `LLEN killreg == 1` over a
     fresh admin connection (cache primed).
   - Act: kill the producer's socket with the two-part form
     `redis::cmd("CLIENT").arg("KILL").arg("TYPE").arg("normal")` over the
     admin connection; await the reply and assert it reports at least one
     client killed. Send the next LPUSH ("b") → it MUST return `Err`
     whose Display carries the transport failure (original error rethrown
     unchanged; "b" never reached the server — the non-idempotent command
     is not retried). Send the following LPUSH ("c").
   - Assert: the second post-kill LPUSH succeeds (`Ok`) and `LLEN killreg`
     is exactly 2 over a fresh admin connection ("a" + "c"; the lazy rebuild
     happened, the killed in-flight "b" was not applied).
2. Add `health_recovers_after_client_kill`: same arrange on its OWN
   dedicated container; after the kill, call `producer.check_connection()` →
   assert first call `Err`, second call `Ok(())`.
3. Do NOT revert or modify the `SET` reclassification tests.

**Tests:**
- `client_kill_then_next_command_reconnects` (integration, Docker):
  arrange/act/assert as in Steps 1.
- `health_recovers_after_client_kill` (integration, Docker): as in Step 2.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_test`
  exits 0 (foreground; Docker available on host).
- The two new tests are the ONLY additions; no existing suite test loosened.

- [x] 5.1

## Final verification

### Task 6.1: crate lib suite and lint gates

**Files:**
- none (verification-only task)

**Steps:**
1. `cargo fmt --check --all` (worktree).
2. `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings`.
3. `cargo test -p camel-component-redis` (all targets — lib AND the crate's
   integration tests `tests/single_flight.rs`, `tests/response_timeout.rs`,
   etc., which pin the single-flight scenarios the cache rewrite touches).
4. `cargo test -p camel-test --features integration-tests --test redis_test
   --test redis_repositories_test` (foreground; Docker available on host;
   record exit codes).
5. `nix shell nixpkgs#mdbook -c mdbook build docs` AND `cd docs && mdbook
   test` (repo root invocation; docs changed; record exit codes).
6. `bash scripts/gates-local.sh` (worktree; record exit code).
7. NEVER run `cargo test --workspace` full (Docker + native bridges; CI owns
   it).

**Tests:**
- The command exits above are the test evidence (record all of them).

**Acceptance:**
- All seven steps exit 0 (or documented pre-existing-failure exemption with
  bd follow-up — none expected).

- [x] 6.1
