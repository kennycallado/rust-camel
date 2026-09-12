# Pre-flight Design Ruling — mission jstax (bd rc-i5pqu)

Boa JS eval caching. Pure perf change. This ruling binds STAGE 1.
Reviewed against code at `crates/languages/camel-language-js/src/{engines/boa.rs,expression.rs,bindings.rs}`, boa_engine `0.22` (`Cargo.toml:221`).

## VERDICT

**GO-WITH-CONDITIONS.**

The measured cost is `Context::default()` (realm + intrinsics + global object), not
parse. boa `0.22` `Script` is `!Send` and lives on the creating thread's `boa_gc`
thread-local heap, so it CANNOT be stored in the `Send + Sync` expression struct the
way rhai stores `Arc<AST>` — the rhai symmetry breaks at the type level. The only way
to reuse the expensive `Context`/realm across evals is to pin all boa work to one
owning thread. Therefore the design is a **dedicated JS worker thread** (option A/B
hybrid) with a **fresh realm per eval** for isolation. This is decisive and low-risk
IF the spec pins the isolation contract and the timeout/backpressure semantics below.
The conditions are the SPEC TRAPS; ship them or the change silently breaks JS
semantics.

---

## 1. THE DESIGN

### Ownership model

One process-global `JsWorker` per `BoaEngine` limits-configuration, lazily spawned,
owning:

- a single OS thread (`std::thread`, name `camel-js-worker`),
- a persistent `boa_engine::Context` created **on that thread** (built once — pays the
  intrinsics cost exactly once for the worker's life),
- a per-source compiled-`Script` cache: `HashMap<u64, Script>` keyed by
  `xxhash(source)` (or `ahash`), bounded LRU, cap **256 entries** (see Q3),
- an inbound `std::sync::mpsc::Receiver<JsJob>`.

`JsJob { source: Arc<str>, exchange: JsExchange, reply: oneshot::Sender<Result<JsEvalResult, JsLanguageError>> }`.

The `BoaEngine` struct holds `sender: mpsc::Sender<JsJob>` (cheap `Clone`, `Send + Sync`).
`BoaEngine::eval` becomes: send job, block on `reply.recv()`. Because `eval` already
runs inside `spawn_blocking` (`expression.rs:213`), blocking on the reply is legal —
we are ON a blocking-pool thread, not a runtime worker.

Data structures (exact):

```rust
struct JsWorker { tx: std::sync::mpsc::Sender<JsJob> }

enum JsJob {
    Eval { source: Arc<str>, exchange: JsExchange,
           reply: std::sync::mpsc::SyncSender<Result<JsEvalResult, JsLanguageError>> },
    Validate { source: Arc<str>,
               reply: std::sync::mpsc::SyncSender<Result<(), JsLanguageError>> },
}

// lives ONLY on the worker thread; never crosses a thread boundary:
struct WorkerState {
    ctx: boa_engine::Context,                 // built once, on-thread
    cache: lru::LruCache<u64, boa_engine::Script>, // Script is !Send — OK, never sent
    limits: ResolvedJsLimits,
}
```

Use `SyncSender`/`mpsc` (std) for the reply, NOT tokio oneshot — the caller is a
blocking thread and the worker is a plain OS thread; no async runtime is present on
the worker. Zero tokio dependency inside the worker.

### Per-eval sequence (on the worker thread)

1. Receive `JsJob::Eval`.
2. `let realm = ctx.create_realm()?;` — fresh realm = fresh global object + fresh
   intrinsics view. **Verify at spec time** that `create_realm` is materially cheaper
   than `Context::default()`; if it is NOT (Q2 escape hatch), fall to design variant
   **D′** below.
3. `let prev = ctx.enter_realm(realm);`
4. Apply runtime limits on `ctx` (loop/recursion/stack) — idempotent, cheap.
5. `bindings::register_console(&mut ctx)` — sets `console` on the *current* realm's
   global. Unchanged.
6. `bindings::build_camel_global(&exchange, &mut ctx)` + set `camel` global. Unchanged.
7. Cache lookup by `hash(source)`:
   - HIT: reuse the stored `Script`. But a `Script` is realm-bound to the realm it was
     parsed in (`Script::parse` binds to `ctx.realm()`; `evaluate` pushes a CallFrame
     carrying `self.inner.realm`). A `Script` parsed against realm R1 will run its
     `global_declaration_instantiation` against R1's global, NOT the fresh realm R2.
     **This defeats per-eval isolation for cached scripts.** See Q2 resolution — the
     cache must therefore key on **(source, realm-generation)** OR the isolation
     mechanism must be realm-independent. Resolved: **the cache stores the parsed
     `boa_ast` phase is not exposed**, so we cache `Script` AND we make the realm
     stable per-worker, using **global-object scrub** for isolation instead of
     fresh-realm. See the corrected sequence D′ which is the RULING.

The step-7 contradiction is real and is why fresh-realm-per-eval + Script-cache are
mutually exclusive in boa `0.22`. **The binding design is D′:**

### RULING DESIGN — variant D′ (Script cache + stable realm + global scrub)

Worker holds ONE `Context` with ONE long-lived realm. Per eval:

1. Receive job.
2. Ensure runtime limits set on `ctx` (once at worker init; limits never change for a
   fixed `BoaEngine` config).
3. **Scrub the global object to a known-clean baseline** (see Q2 mechanism).
4. `register_console` + `build_camel_global` + set `camel` (overwrites prior eval's
   bindings — already idempotent, `bindings.rs:42,58,71`).
5. Cache lookup `hash(source)`:
   - MISS: `Script::parse(Source::from_bytes(src), None, &mut ctx)` → binds to the
     worker realm; insert into LRU. Parse errors surface here (Q6: validate reuses
     this path).
   - HIT: reuse `Script` — same realm, valid, no re-parse, no re-compile
     (`codeblock()` bytecode cached inside `Script`).
6. `script.evaluate(&mut ctx)` (realm matches current realm — the safe pattern).
7. `js_to_value(result)` + `extract_camel_state`. Reply.
8. Loop.

D′ keeps ONE realm alive (pays intrinsics once), caches `Script` (kills re-parse AND
re-compile), and enforces isolation by **scrub**, not by realm churn. This is the
only design that is BOTH correct AND captures the full parse+compile+intrinsics win.

---

## 2. ANSWERS TO QUESTIONS 1–7

**Q1 — Cache architecture:** Chosen = **(A) dedicated worker thread** in the D′ shape
(persistent Context + one realm + per-source `Script` LRU). NOT (C) thread_local
pool: `spawn_blocking` lands on arbitrary threads → N-way Context duplication and cold
caches. NOT (D) fresh-Context-per-eval: that leaves the dominant `Context::default`
cost unpaid — it is the thing we are removing. (B) is rejected because a fresh Context
per eval on the worker still rebuilds intrinsics every time (the 1.1ms), defeating the
purpose.

**Q2 — Isolation mechanism:** **Global-object scrub to a captured baseline**, run
before every eval. At worker init, after building a pristine realm, capture the
baseline set of own-property keys of the global object (`own_property_keys`). Before
each eval:
(a) delete every own-key of the global object NOT in the baseline set (removes
    persisted `var count`, user `globalThis.x = …`, leftover `camel`/`console`);
(b) re-install `console` and `camel` fresh (steps 4 above) so overwrite is total.
This handles `var` persistence and top-level global assignment.
**Builtin-prototype pollution** (`Object.prototype.x = 1`, `Array.prototype.foo`) is
NOT covered by global-key scrub and is the hard case. Ruling: because the ONLY host
data reaching JS is built with `JsObject::with_null_proto()` (`bindings.rs:43,68,163,
172` + crate CONTEXT "Sandbox posture"), prototype pollution cannot corrupt
exchange-data extraction — extract reads `__data` off null-proto objects. It CAN still
change observable script results across evals (`({}).x` returns `1` in eval N+1). That
violates the sealed "no cross-eval mutable state (includes builtin-prototype
pollution)" constraint. Therefore the scrub is INSUFFICIENT alone.
**Resolution:** the worker must reset builtin prototypes too. boa `0.22` gives no cheap
"reset intrinsics" API, so the binding rule is a **hybrid**: keep the `Script` cache
but run each eval **in a fresh realm entered on the persistent Context**
(`create_realm` + `enter_realm`), AND re-key the cache to survive realm churn by
storing the `Script` and, on realm mismatch, re-parsing into the new realm. Since
`create_realm` cost is the open risk, STAGE 1 MUST benchmark `create_realm` vs
`Context::default` FIRST (micro-bench, off the sealed canonical bench). Two outcomes:
- **If `create_realm` ≥ ~70% of `Context::default`:** fresh-realm buys little; the
  perf goal fails. Fall back to scrub-only (D′) and **explicitly narrow the semantic
  guarantee**: document that builtin-prototype mutation does NOT persist across evals
  by re-installing a null-proto shield is impossible; instead REJECT scripts that
  mutate builtin prototypes is not feasible either. In this branch the honest ruling
  is scrub-only + a spec-declared, tested residual: builtin-prototype writes are
  scrubbed by a **prototype snapshot/restore** (capture own-keys of
  `Object.prototype`, `Array.prototype`, `Function.prototype`, `String.prototype`,
  `Number.prototype`, `Boolean.prototype` at init; delete added keys + restore changed
  values before each eval). This is bounded, deterministic work over a fixed prototype
  set and IS the mitigation. Adopt this as the **primary** mechanism — it is O(fixed)
  and realm-independent, so it composes with the `Script` cache with zero realm
  contradiction.
- **If `create_realm` is cheap (< ~30%):** prefer fresh-realm-per-eval and accept
  per-realm re-parse on first use (cache keyed by realm-gen); simpler correctness
  story.

**Binding default (no bench needed to be safe): prototype-snapshot/restore scrub +
global-key scrub + `Script` cache on one stable realm (D′).** It is the design that is
provably correct for the sealed isolation set and captures the full win. Fresh-realm
is the optimization to consider only if the bench shows it is free.

**Q3 — Invalidation:** Source-keyed process cache with **LRU bound 256 entries**.
Rationale: `--watch` route rebuild constructs NEW expression structs but they share the
SAME process-global worker; a source-keyed cache stays CORRECT across reload (identical
source ⇒ identical parse; boa is deterministic), and stale sources age out via LRU. No
explicit watch hook is required — this is the key advantage of source-keying over
expression-struct-owned caches (the latter would need `!Send` `Script` inside the
struct, which is impossible anyway). 256 × (typical compiled `Script` ≈ low tens of
KB) ≈ single-digit MB ceiling; acceptable. Cap is configurable later; not in v1 spec
surface.

**Q4 — Thread model:** **Single worker per `BoaEngine` config**, serializing all JS
evals. Justified: t2-json tick is single-flow; JS steps are CPU-bound sub-ms after
caching, so a single core saturates before the serialization point hurts. Use a
**bounded** `sync_channel` (capacity = e.g. 1024) for backpressure: when full,
`send` blocks the calling `spawn_blocking` thread — natural backpressure that ADR-0044
admission already caps upstream. Multi-worker sharding is a documented FUTURE lever
(shard by `hash(source) % N`) if a JS-heavy multi-route workload appears; NOT in v1.
The single worker is a serialization point — spec MUST record it as a known tradeoff.

**Q5 — Timeout contract:** The outer `tokio::time::timeout` in `eval_async`
(`expression.rs:211`) stays. On timeout the future is dropped; the `reply` receiver is
dropped; the caller returns `JsLanguageError::Execution "JS execution timeout"` —
identical error surface to today (constraint 1 preserved). The wedged eval continues on
the worker until a boa limit trips (loop 100k / recursion 512 / stack 10240 all
guarantee termination; no I/O, no `setTimeout`, no module resolver exist in the
sandbox). **Key difference from today:** a wedged eval BLOCKS THE SINGLE WORKER until
it terminates, stalling subsequent JS evals for up to the limit-bound duration.
Mitigation is the limits themselves (bounded CPU ⇒ bounded stall) plus: the reply is
sent from the worker AFTER eval completes; if the receiver is already gone (caller
timed out), the worker discards the result and proceeds. Deadlock policy: worker NEVER
blocks on a full reply channel (reply is `SyncSender` cap 1, and on
`send` error = receiver dropped, worker drops result and continues). No path where the
worker awaits the caller. **Acceptable: YES**, given limits guarantee termination.
Spec MUST state the single-worker head-of-line stall bound = worst-case limit trip
time.

**Q6 — API surface:** `JsEngine::eval(&self, &str, JsExchange)` and
`validate(&self, &str)` signatures **UNCHANGED** (non-breaking; constraint 3). The
worker lives behind `BoaEngine`'s existing methods — `eval` sends a job and blocks on
reply; `validate` sends a `Validate` job that runs `Script::parse` on the worker (and
populates the cache as a warm side effect). The `with_engine(custom)` test seam and the
`SlowEngine` fake keep working untouched — they implement the same trait, no worker.
No cache handle is exposed on `JsExpression`; it keeps `script: String` +
`engine: Arc<dyn JsEngine>`. **Zero public-type change.**

**Q7 — Acceptance tests (mission-scoped; canonical bench stays sealed):**
1. `bench_repeated_eval_wall_clock` (NOT `#[ignore]`, CI-stable): 1000 evals of a
   representative transform (`camel.headers.set('n', (parseInt(camel.headers.get('n'))||0)+1); camel.body`)
   through one `BoaEngine`; assert total wall-clock < a generous CI budget (e.g. the
   first eval's own duration × 200, i.e. amortized parse+intrinsics ≪ per-eval). Assert
   RATIO not absolute ms to stay machine-independent: `time(evals 2..N) / time(eval 1)`
   proves amortization.
2. `stateful_var_does_not_persist` (MUST FAIL on naive reuse): eval `var count = (typeof count==='undefined')?0:count; count++; count` twice; second result MUST equal first (== 1), proving `var` did not leak.
3. `builtin_prototype_pollution_does_not_persist`: eval `Object.prototype.pwned = 1; 0` then eval `({}).pwned === undefined ? 'clean' : 'dirty'`; second MUST be `'clean'`.
4. `global_assignment_does_not_persist`: eval `globalThis.leak = 42; 0` then `typeof leak` → `'undefined'`.
5. `identical_result_across_repeats`: same source + same exchange, 3× → byte-identical `JsEvalResult`.
6. `error_surface_identical_first_and_cached`: `throw new Error('boom')` twice → identical `LanguageError` variant + message both times (cache must not swallow the runtime error).
7. `syntax_error_on_first_parse`: invalid source → `ParseError` (validate path + eval path agree).
8. `concurrent_multi_route_no_cross_talk`: two `JsExpression`s with different stateful sources evaluated interleaved via `tokio::join!` (both on the one worker) → each sees only its own state.
9. `console_binding_fresh_each_eval`: eval that overwrites `console.log = ()=>{}` then next eval's `console.log` still routes to tracing (re-installed).

---

## 3. SPEC TRAPS (requirements the OpenSpec spec MUST capture)

- **SR-1 (isolation set is normative):** The spec MUST enumerate the full isolation
  guarantee as testable requirements: (a) top-level `var`/`let`/`const`/`function`
  declarations do NOT persist across evals; (b) `globalThis`/implicit global
  assignment does NOT persist; (c) builtin-prototype mutation (`Object.prototype`,
  `Array.prototype`, `Function.prototype`, `String/Number/Boolean.prototype`) does NOT
  persist; (d) `camel` and `console` are rebuilt every eval. Missing ANY of a–d = spec
  incomplete.
- **SR-2 (scrub is before, not after):** Isolation scrub MUST run BEFORE each eval
  (defensive against a prior eval that panicked/timed out mid-mutation), not after.
- **SR-3 (prototype baseline is fixed and captured at worker init):** The prototype
  key/value baseline is captured once against a pristine realm; the restore set is a
  closed, named list of prototypes. New builtin prototypes require a spec amendment.
- **SR-4 (error surface unchanged):** Parse errors → `LanguageError::ParseError`;
  runtime errors → `LanguageError::EvalError`; timeout → `"JS execution timeout"`.
  Caching MUST NOT alter which variant fires or the message text (constraint 1).
- **SR-5 (evaluation order & results unchanged):** `Script` reuse MUST NOT reorder
  side effects; `create_realm`/scrub MUST NOT change expression results vs a fresh
  `Context`. Tested by result-equivalence over the boa test corpus already in
  `boa.rs`/`expression.rs`.
- **SR-6 (single-worker serialization + head-of-line stall) is a declared tradeoff**
  with the bound = worst-case boa-limit trip time; the bounded channel provides
  backpressure, not unbounded queueing.
- **SR-7 (source-keyed cache correctness under `--watch`):** identical source ⇒ cache
  reuse across route reload is CORRECT and intentional; the spec states no explicit
  invalidation hook is needed and LRU cap bounds memory.
- **SR-8 (Boa boundary intact):** all new code stays in `engines/boa.rs` (+ a new
  `engines/worker.rs` if split); no boa type crosses the `JsEngine` trait; the
  `with_engine` seam and `SlowEngine` fake keep compiling (crate CONTEXT "Boa
  boundary").
- **SR-9 (`!Send` `Script` never crosses threads):** the spec MUST state that
  `boa_engine::Script` and the `Context` live and die on the worker thread; the cache
  is worker-thread-local; nothing boa-GC-owned is placed in a `Send + Sync` struct.
- **SR-10 (limits unchanged):** loop/recursion/stack/source-size/value-nesting caps
  are byte-for-byte the current values; caching does not relax any DoS cap.

## 4. RISKS (top 5, with mitigations)

1. **Builtin-prototype pollution leaks across evals (correctness bug that looks like a
   pass).** → Prototype snapshot/restore scrub over the fixed prototype set (SR-3),
   with test #3 that FAILS on naive reuse. This is the single most likely way a worker
   "passes CI" while violating the sealed constraint.
2. **`create_realm` is not cheaper than `Context::default` → perf goal missed.** →
   STAGE 1 micro-bench GATES the fresh-realm variant; default design (D′, scrub-only +
   `Script` cache) does NOT depend on realm churn and still captures parse+compile+
   intrinsics amortization. Fresh-realm is opt-in only if proven free.
3. **Single worker becomes a throughput bottleneck for JS-heavy multi-route loads.** →
   Bounded channel for backpressure now; documented `hash(source) % N` sharding lever
   for later. t2-json (the measured target) is single-flow so v1 single-worker is
   correct-by-scope.
4. **Wedged eval stalls all JS (head-of-line).** → boa limits guarantee termination
   (no I/O in sandbox); stall bound = limit-trip time; documented (SR-6). Outer tokio
   timeout still returns control to the route on schedule.
5. **Worker thread panics (e.g. boa internal) kills all future JS.** → Wrap the
   per-eval body in `catch_unwind`; on panic reply `Err(Execution)` and REBUILD the
   `Context` + realm baseline + drop the cache (poisoned-realm recovery). Spec MUST
   require worker self-heal on panic, else one bad eval bricks the language for the
   process lifetime.

---

### Self-grill record

**Questions generated:**
1. [glossary] Does "worker thread owning a persistent Context" collide with any
   existing camel-core supervision/lifecycle term (Consumer, Supervision) in
   CONTEXT-MAP?
2. [sharpen] "Cache the script" — is the cached unit the parsed AST, the compiled
   bytecode, or the realm-bound `Script`? Two concepts hide in one word.
3. [scenario] Construct an input that a `Script`-cache + fresh-realm design gets wrong.
4. [cross-ref] Does the code today already provide the isolation the mission wants, so
   that caching must actively preserve it rather than add it?

**Answers (with citations):**
1. [glossary] No collision. "Supervision/ConsumerRestart" is route-level crash
   recovery for Consumers (`CONTEXT-MAP.md:122`); "Consumer" is a component adapter.
   The JS worker is an internal implementation thread of ONE Language impl, not a
   Route/Consumer, and is invisible to CamelContext lifecycle. It needs its own local
   term ("JS worker"), no glossary conflict. (`CONTEXT-MAP.md:122-123`)
2. [sharpen] Three distinct concepts. boa `Script` holds phase `Ast(StatementList)`
   AND lazily-compiled bytecode via `codeblock()` ("no-op if called previously"), and
   is realm-bound. The cached unit is the `Script` (parse + compile), but its
   realm-binding is the trap: it is NOT a free-floating AST like rhai's `Arc<AST>`
   (`crates/languages/camel-language-rhai/src/lib.rs:484` stores `Arc<AST>` which IS
   Send). The ruling names the unit precisely as "realm-bound `Script`" and forbids
   treating it as portable (SR-9).
3. [scenario] Input: eval-A `var g = 5;`, then cached-eval-B (same source reused).
   With fresh-realm-per-eval, a cached `Script` parsed in realm R1 runs its
   `global_declaration_instantiation` against R1's global even after `enter_realm(R2)`,
   because `Script::evaluate` pushes a CallFrame carrying `self.inner.realm` (BOA
   0.22 API fact, verified in brief). So `camel`/`console` set on R2 are invisible to
   the script, and R1's globals accumulate — BOTH a correctness break (missing
   bindings) and an isolation break (R1 var persistence). This scenario is exactly why
   the ruling rejects "Script-cache + fresh-realm" and adopts D′ (one stable realm +
   scrub). Constructed input + predicted result: `camel.body` in cached-eval-B would
   throw ReferenceError (camel undefined on R1) → observable divergence.
4. [cross-ref] The isolation exists TODAY only as a side effect of throwing the whole
   `Context` away every eval (`boa.rs:100` `Context::default()` per call; crate CONTEXT
   "Each evaluation creates a fresh Boa Context"). Nothing in the code actively scrubs
   state — it relies on drop. Therefore caching MUST actively RE-CREATE that guarantee
   (scrub), it cannot inherit it. Host data uses `with_null_proto()`
   (`bindings.rs:43,68,163,172`) which protects extraction from prototype pollution but
   NOT script-visible results — confirming builtin-prototype scrub is mandatory, not
   optional (Risk 1). (`boa.rs:100`, `bindings.rs:43,68`)

**Outcome:** refine — the naive "mirror rhai / cache the Script in the expression
struct" framing is dropped as impossible (`!Send`); the ruling is sharpened to D′
(worker + stable realm + fixed-set prototype/global scrub + LRU `Script` cache) with a
gated fresh-realm optimization.
**Self-grill mode:** self-grill-proposals skill
