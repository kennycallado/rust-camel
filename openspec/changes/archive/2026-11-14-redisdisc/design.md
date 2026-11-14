# Design: redisdisc

## Approach

Two independent fixes share one crate. Both are ruled by the e_opus review of
2026-11-14 attached to `rc-2un5k` / `rc-n64k3` — that ruling is authoritative
over this document.

### 1. Transport invalidation (rc-2un5k)

**Cache shape.** `MultiplexedExecutor.conn` becomes
`Arc<Mutex<Option<(u64, MultiplexedConnection)>>>`. The generation allocator is
SHARED interior-mutable state: `Arc<AtomicU64>`, incremented with `fetch_add`
on every successful build — in practice always while holding the
`connect_gate` (single-flight already serializes builds), though `fetch_add`
itself guarantees uniqueness regardless. Clones share the allocator, so
generations are never reused across the executor family; the first build gets
generation 1 (0 is reserved for unused instances). A per-instance
`last_used_gen: u64` field (plain field, NOT Arc-shared — each clone tracks
its own) records the generation the executor instance last handed to a
command: written ONLY by `execute_command`, which records `self.last_used_gen
= gen` immediately after its checkout succeeds and BEFORE dispatching (bare
`checkout` cannot record it — it takes `&self` — and `reconnect` must observe
the generation of the command that just failed), read ONLY by `reconnect`
(`&mut self`), initialized to 0 by `new` and copied verbatim by `Clone`.
Generation 0 is RESERVED for unused instances: the allocator's first
`fetch_add` returns 1, so a cached generation is always ≥ 1 and a
`last_used_gen` of 0 unambiguously means "this instance never checked out".
A stale copy after `Clone` can at worst trigger one extra rebuild; it can
never clobber a different generation.

**Checkout seam.** New `pub(crate) async fn checkout(&self) ->
Result<(u64, MultiplexedConnection), CamelError>` (crate-visible: sibling
`producer.rs` calls it): fast path returns `(gen, conn.clone())`; slow path is
today's gate/double-check/build flow, storing `(gen, conn)` with `gen` taken
from the shared allocator. Public `get_conn` stays signature-stable (repository
service depends on it) and delegates: `checkout().map(|(_, c)| c)`.

**Compare-and-clear.** New `pub(crate) async fn
invalidate_if_current(&self, failed_gen: u64)`: lock `conn`; clear only if the
cached generation equals `failed_gen`. Never dials. Called from:

- `MultiplexedExecutor::execute_command` (`&mut self`, trait impl): checkout,
  dispatch, on `Err(e)` → if `invalidates_connection(&e)` then
  `invalidate_if_current(gen)`; rethrow `e` unchanged. On `Ok` no state change.
- `check_connection` (producer.rs): checkout, PING, on `Err` build the error
  WITH its source (`ProcessorErrorWithSource`, same text as today) so the
  classifier can read the chain, invalidate on the classifier, return it.

**Generation-aware reconnect.** `RedisCommandExecutor::reconnect` clears the
cache ONLY if the cached generation equals `self.last_used_gen` (per-instance),
then `get_conn`. This is what makes N clones failing on one socket non-clobbering:
clone A invalidates and rebuilds (gen N+1); clone B's reconnect compares its
own last-used N against the cached N+1 and keeps the fresh connection.
`refresh()` stays an unconditional clear — its contract is pinned by
`refresh_triggers_reresolution_and_failure_is_not_cached` and it is an explicit
operator seam, not an error-path action.

**Ordering note.** `execute_with_retry` keeps its shape: reconnect still runs
before each retry (attempt > 0) and is now generation-aware, so the retry loop
benefits from the same non-clobber guarantee. A production `reconnect` always
follows this executor instance's own failed `execute_command`, so its
`last_used_gen` is the failed generation and the drop fires. A standalone
reconnect on an instance that never checked out (test-only shape) finds a
generation mismatch or an empty cache and keeps whatever is cached. The pinned
unit test `multiplexed_executor_reconnect_reresolves` drives that standalone
shape and is UPDATED to the refined contract: checkout-then-reconnect
re-resolves; a blind reconnect on an unused instance keeps the cached
connection.

### 2. The classifier (rc-2un5k)

`pub fn invalidates_connection(err: &CamelError) -> bool` in
`transport_error.rs`, reusing the bounded source-chain walk
(`MAX_SOURCE_HOPS`, `unwrap_arc_dyn_error`):

1. Config / ConfigValidation → false (same rule 1 as transient).
2. `CamelError::Io(_)` → false. The `Io(String)` variant can carry flattened
   transport text as well as repository serialization messages, but it offers
   no structural evidence — no `redis::RedisError` or typed io kind sits in an
   inspectable chain — so the classifier refuses to act on its text. Pinned
   pair: `CamelError::Io` is transient=true (transient rule 2), invalidating=
   false. Real transport failures at the invalidation call sites preserve the
   `redis::RedisError` in the source chain.
3. Chain walk. Markers: `TransportTimeout` → **true** (half-open TCP surfaces as
   timeouts); `TransientRetryBudgetExhausted` → false; `TransientByProse` →
   false. First `redis::RedisError` decides:
   - `Server(ReadOnly)` → **true** (demoted master). This is the one server
     reply that invalidates; every OTHER server reply stays false.
   - `Server(_)` (any kind other than `ReadOnly`) → false — never on other
     server error replies (rule 5 codes and rule-6 text matches included).
   - `ClusterConnectionNotFound` → false (transient but not in the ruled
     invalidation set).
   - `Io` → true iff the inner `std::io::Error` kind is one of
     `ConnectionRefused | ConnectionReset | ConnectionAborted | BrokenPipe`;
     otherwise true iff the RedisError Display contains `"connection dropped"`
     (redis-rs's own driver-drop phrase); else false. Deliberately NOT
     invalidating: io `TimedOut` (the `TransportTimeout` marker covers the
     crate's own timeout wraps; the driver-deadline io timeout is not proof of
     a dead socket per ruling).
   - Any other kind → false (covers TLS text and redis-rs static details that
     only the transient rule-6 fallback matches).
4. No marker, no RedisError → false.

**Subset property.** For every error the crate can produce at the two
invalidation call sites, `invalidates_connection(e) ⇒ is_transient_redis_error(e)`.
Pinned by a corpus-driven property test in `transport_error.rs` iterating the
full fixture set (every transient fixture, every fatal fixture, markers,
config, io variants, plain ProcessorError, chained wraps) asserting
`!invalidates || is_transient`, plus explicit negatives: every other server
reply shape (NOGROUP/BUSYGROUP/NOPERM/WRONGTYPE, server text with classifier
words) must NOT invalidate — `Server(ReadOnly)` is the only server-reply
positive.

No new `CamelError` variant.

### 3. Retry budget semantics (rc-n64k3)

Model: consecutive failures since the last proven round trip. No time window
(Kafka/MQTT/JMS parity). Each loop keeps its counter; resets are:

- **streams read task** (`read_task_loop`): when a read returns
  `Ok(Some(entries))`/`Ok(None)`, snapshot `attempt`. In the settle loop, on
  `ReadTaskCmd::BatchDone`: if `attempt == snapshot` (no ack-settle or
  reconnect failure consumed budget in between), reset `attempt = 0`. The
  initial `BatchDone` before any read never resets (never reset on connect).
  Ack settle shares this counter, so a failed ack blocks the reset — an ack
  that always fails still exhausts.
- **streams recovery** (`session_core`'s `recovery_attempt`, threaded through
  `recovery_pass`/`drain_recovered`): reset `*attempt = 0` in `recovery_pass`'s
  `Ok(batch)` arm (a completed `XAUTOCLAIM` pass is the proven round trip,
  claimed entries or not). Its own counter, independent of the read task's.
- **queue** (`queue_session`): reset `attempt = 0` on `Ok(Some(item))` and on
  `Ok(None)` (blocking-pop timeout is a proven server round trip) before
  delivery/continue.
- **pubsub** (`pubsub_session`): stamp `connected_at = Instant::now()` when
  `connect_and_subscribe` returns Ok. When the stream ends (`next_msg` →
  `None`): if `connected_at.elapsed() >= policy.max_delay`, reset
  `attempt = 0` before the existing `attempt += 1` (a quiet channel delivers
  nothing, so uptime is the only health proof).

Never reset on connect in any loop.

### 4. ADR-0013 amendment

Append to the Decision section: the invalidation rule (cache invalidate on
transport-failure classification, compare-and-clear on a generation-tagged
cache, lazy rebuild, rethrow unchanged) and the budget semantics (consecutive
failures since the last proven round trip; reset points per loop shape;
never-on-connect rationale). This is the cross-component contract — Redis is
aligned with what Kafka/MQTT/JMS already do.

### 5. Docs fixes

- `docs/src/concepts/error-handling.md`: the supervision section claims
  "Rust API only". Correct it: `[supervision]` exists in `Camel.toml`
  (cite the ADR-0013 amendment and the crate CONTEXT.md).
- `docs/src/configuration/schema.md`: `max_attempts` documents default 5;
  add that an omitted value with the `[supervision]` table declared means
  restart forever (cite ADR-0013 / schema source).

### 6. Tests

- **Headline regression** (integration, `camel-test`): open a producer
  endpoint, verify one command, `CLIENT KILL TYPE normal` (or an equivalent
  unclean socket drop), assert the failing command returned the original
  error, then assert the NEXT command succeeds on a rebuilt connection.
- **Health recovers after a kill**: same kill; `check_connection` fails once,
  then succeeds.
- **Per-loop budget tests** (unit, existing fake seams): a sequence of N
  blip/recover cycles (failure then success) does not exhaust a small budget;
  a run of N consecutive failures does exhaust it. One per loop: queue,
  pubsub (stream ends with long uptime → reset; short uptime → no reset),
  streams read (BatchDone after failure-free read → reset; failed ack
  between read and BatchDone → no reset), recovery (successful pass resets;
  failing passes accumulate).
- **Subset property test** in `transport_error.rs` (corpus-driven; proptest is
  NOT a dependency of this crate — a deterministic corpus loop satisfies the
  proof obligation and keeps the suite hermetic).

## Affected crates

- `camel-component-redis`: executor.rs (cache shape, checkout,
  invalidate_if_current, generation-aware reconnect), producer.rs
  (check_connection), transport_error.rs (classifier + property test),
  pubsub.rs / queue.rs / streams.rs (reset points), unit tests.
- `camel-test`: integration regression tests (kill-connection, health
  recovery) in the redis suites.
- `docs`: ADR-0013 amendment, error-handling.md, schema.md.

## Architecture boundaries

- Runtime/DSL untouched; the change is inside the Redis component's I/O seam
  and its docs. Data/control plane split respected: invalidation is connection
  bookkeeping (control plane); no exchange payload semantics change; the
  failed command's error surfaces byte-identical to today.
- ADRs referenced: ADR-0012 (transient classification, budget-exhaustion
  marker), ADR-0013 (NetworkRetryPolicy boundaries — amended here), ADR-0007
  (route supervision consumes the budget-exhausted error).

## Alternatives considered

- **Dial in the error path** (reconnect immediately on failure). Rejected by
  ruling: the error path must stay dial-free; lazy rebuild under the existing
  connect_gate keeps single-flight storm elimination and sentinel
  re-resolution semantics intact.
- **New trait method** (`invalidate` on `RedisCommandExecutor`). Rejected by
  ruling — invalidation lives inside `execute_command`/`check_connection`; the
  FakeExecutor is untouched.
- **Time-windowed budget** (reset if last failure older than T). Rejected by
  ruling: adds clock configuration; Kafka/MQTT/JMS already use the
  consecutive-failure model.
- **Reset on successful connect**. Rejected: a connect-then-fail-forever cycle
  would flap without ever reaching supervision. Same for resetting right
  after a read alone: an always-failing ack would flap the same way.
- **Include io `TimedOut` / `ClusterConnectionNotFound` in the invalidation
  set.** Rejected: the ruling enumerates the set; both stay transient-only
  (subset direction is one-way by design).

## Phases

Omitted — single-phase change (one coherent slice: classifier → seam → reset
points → docs, in that task order).
