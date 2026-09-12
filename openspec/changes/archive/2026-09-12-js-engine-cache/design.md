# Design: js-engine-cache

## Approach

Follows the signed Option A decision (fleet order 67) and the pre-flight
ruling record (`crates/languages/camel-language-js/docs/jstax-design-ruling.md`).
Probe evidence: the per-eval tax is ~99% realm/intrinsics construction; parse
is ~3%; a fresh realm costs a full `Context`. So the worker reuses ONE stable
realm and accepts a declared isolation contract instead of fresh-`Context`
equivalence.

`BoaEngine` lazily spawns one `camel-js-worker` OS thread per
limits-configuration. Jobs flow over a bounded `sync_channel(1024)`:

```rust
enum JsJob {
    Eval { source: Arc<str>, exchange: JsExchange, timeout_ms: u64,
           enqueued: std::time::Instant,
           reply: SyncSender<Result<JsEvalResult, JsLanguageError>> },
    Validate { source: Arc<str>, reply: SyncSender<Result<(), JsLanguageError>> },
}
// worker-thread only, never crosses a thread boundary:
struct WorkerState {
    ctx: Context,                          // persistent shell
    realm: Realm,                          // stable; recycled on drift/panic
    cache: LruCache<Arc<str>, Script>,     // wrapper scripts, cap 256
    limits: ResolvedJsLimits,
    baseline: IntegrityBaseline,           // captured at realm build
}
```

Per-eval sequence: check job deadline (expired → reply timeout, skip) →
cleanup (delete configurable global keys not in baseline) → install fresh
`console`/`camel` → install-verify `camel`, `console`, and pristine `eval`
(any failure → recycle realm, retry once) → cache lookup by full source bytes
(miss: compile wrapper `eval(<escaped source literal>)`, insert) → run wrapper
(inner indirect eval re-parses user source, ~11 µs; fresh declarative
environment per eval call) → convert and extract → reply → integrity check
(drift → recycle) → next job. Cleanup and the integrity check run after
success, error, panic, and limit failure. The eval body is wrapped in
`catch_unwind`; on panic the worker replies `Err(Execution)` and recycles.
Escaping emits a valid JS string literal for arbitrary Unicode source
(backslash, delimiter, LF/CR/LS/PS escaped); `Arc<str>` keying compares full
bytes by Rust semantics. The wrapper's `eval(...)` is the direct call form,
observably equivalent to indirect here: the wrapper is sloppy, has no lexical
bindings of its own, and install-verify guarantees the pristine realm builtin,
so no shadowing, the same `this`, and the same fresh declarative environment.
Strict-prologue sources get strict-eval scoping (`var` local to the eval
environment, top-level `this` undefined) — an instance of the declared
indirect-eval delta. A failed install-verify retry rechecks the job deadline
(recycle costs 0.4–0.7 ms); a second consecutive failure replies
`Err(Execution)` and recycles again before the next job — no infinite retry.

Integrity baseline: own keys (including symbol keys), property descriptors,
prototype identity, and expected value identity for the named root set:
`globalThis` baseline keys, the `eval` function, and the prototypes of
`Object`, `Array`, `Function`, `String`, `Number`, and `Boolean`. The baseline
is captured after the initial `camel`/`console` install; `camel` and `console`
stay in the key set but are excluded from value-identity comparison — their
presence and pristinity are enforced by install-verify, not by the detector.
The detector's cost is benchmarked separately; if it rivals the per-eval
budget, the named set shrinks to `globalThis` keys, `eval`, and the
`Object`/`Array`/`Function` prototypes, documented in code. The full
`String`/`Number`/`Boolean` trio is the shrink reserve.

Realm recycle = create a new realm on the persistent `Context`, enter it,
re-capture the baseline, and drop the LRU (wrapper scripts are realm-bound).
Recycle is also the entire recovery story for drift, panic, and poison — the
worker never attempts restoration.

Timeout: the outer `tokio::time::timeout` stays (caller detaches). The worker
never blocks on a reply send; a send error means the receiver is gone. A
wedged eval stalls later evals; the stall bound is the backlog times the
per-eval bound, where each eval is bounded by the loop/recursion/stack limits
(native-builtin execution keeps today's admitted unbounded residual).
Expired queued jobs are skipped at dequeue, never executed.

## Affected crates

- `camel-language-js`: `engines/boa.rs` sends jobs; new `engines/worker.rs`
  (worker, baseline, recycle); tests (isolation, integrity, expiry, timing);
  crate `CONTEXT.md` sandbox-posture section replaced with the honest contract.

## Architecture boundaries

Languages tier only; internal to one `Language` implementation; invisible to
route lifecycle. Authority: ADR-0006 (synchronous `script:` path), ADR-0032
(exchange-data trust — host data keeps null prototypes). Trust model
unchanged: script source is trusted operator configuration; untrusted code
belongs in `function:` (ADR-0005). No `camel-dsl` change (source-keyed cache
needs no watch hook).

## Declared engine-state deltas (accepted by owner)

Indirect eval changes global var/function declaration and deletion semantics.
Intrinsic state outside the named integrity set, heap state (retained object
graphs, GC timing), promise jobs, weak refs, and RNG state may survive across
evals, exchanges, and routes until realm recycling or process termination.
Route reload and restart do not reset this state. Undeletable globals can
retain unlimited data and poison reserved bindings — recycling is the
mitigation. Single-worker serialization changes concurrency, throughput,
queue latency, and timeout behavior.

## Future path (not implemented)

Multiple stable realms per worker (the mandate's "contexts" realized as realms
on the single worker `Context` — `Script` is realm-bound and worker-confined)
keyed by compiled-expression identity with LRU eviction — removes cross-route
state sharing without thread-per-expression cost. Requires per-realm wrapper
caches; recycle logic already isolates the mechanics.

## Alternatives considered

- Fresh realm per eval (exact semantics): probe shows a fresh realm costs a
  full `Context` (~377 µs vs 390 µs today) — no win.
- rhai-style compiled artifact in the expression struct: impossible; `Script`
  is `!Send` and realm-bound.
- Cached user scripts without the eval wrapper: lexical bindings would need a
  public realm reset that Boa 0.22 does not expose (`pub(crate)`).
- Restoration of drifted state: rejected — arbitrary restoration is
  impossible to make correct; recycle is simple and provably clean.
