# js-engine-cache Specification

## Purpose
The JavaScript engine worker model. Boa realms are reused across evaluations
under a documented isolation contract: fresh bindings and lexical environment
per evaluation, an install-verify step, integrity-set recycling on drift,
bounded caches, and self-heal on panic.

## Requirements
### Requirement: JS engine cost amortization

The `js` language SHALL amortize Boa realm construction across evaluations by
reusing a worker-owned `Context` with one stable realm, and SHALL key the
wrapper-script cache by full source bytes.

#### Scenario: repeated eval amortization ratio

- **GIVEN** one `BoaEngine` and a representative transform script
- **WHEN** the script is evaluated 1000 times and wall-clock time is measured,
  where eval 1 is the first job submitted to a freshly constructed engine
  (cold worker: includes realm build and wrapper compile) and evals 2..N are
  cache hits on the same source
- **THEN** the mean per-eval time of evals 2..N is under 25% of eval 1's time
  (a ≥ 4x amortization), asserted as a machine-independent ratio, and the
  local steady-state number is measured and documented. Measured (dev box):
  ~93–100 µs/eval release (~4x faster than the pre-change ~390 µs),
  ~350 µs/eval dev, ratio 13.3–14.5x release / 5.5–6.2x dev; CI asserts the
  dev-profile ratio with margin. The original ≥ 20x / ≤ 25 µs estimate
  predated the per-eval integrity verification work (~65 µs mandated by the
  protected-invariants and integrity-set requirements) and was recalibrated
  by spec amendment rather than by weakening the test.

#### Scenario: reuse across watch reloads

- **GIVEN** a route whose JS step source is unchanged across a watch reload
- **WHEN** the route is rebuilt and the step evaluated again
- **THEN** the cache is reused without any explicit invalidation hook, because
  identical source bytes imply an identical wrapper

### Requirement: Honest isolation contract

The `js` language SHALL provide this contract, and the crate documentation
SHALL state it: each evaluation receives fresh `camel` and `console` bindings
and a fresh declarative environment for lexical declarations. Configurable
global additions are removed, and named intrinsic roots are verified between
evaluations. JavaScript evaluations do not receive realm isolation: global
properties, intrinsic state outside the named integrity set, heap state, and
engine-internal state may survive across exchanges and routes until realm
recycling or process termination. Route reload and route restart do not reset
this state. Script source must be trusted operator configuration; untrusted
or mutually distrustful code must use `function:`.

#### Scenario: lexical declarations are fresh per eval

- **GIVEN** a script source with a top-level lexical declaration
  (`let count = 0; count++; count` is evaluated through one engine twice)
- **WHEN** both evaluations complete
- **THEN** both return 1 with no redeclaration error, because the indirect-eval
  wrapper gives each evaluation a fresh declarative environment (re-evaluating
  the same source on a persistent context without the wrapper fails with a
  duplicate-declaration error)

#### Scenario: camel and console are fresh per eval

- **GIVEN** an engine whose previous eval overwrote `console.log` or mutated
  the `camel` object
- **WHEN** the next evaluation runs
- **THEN** re-installed `camel` and `console` bindings behave as fresh, and
  `console` still routes to tracing

#### Scenario: configurable global additions are removed

- **GIVEN** an engine that evaluated `globalThis.leak = 42; 0`
- **WHEN** `typeof leak` is evaluated next
- **THEN** the result is `'undefined'`

### Requirement: Protected invariants with install-verify

The worker SHALL install fresh `camel` and `console` bindings and SHALL verify
them and a pristine `eval` callable before every evaluation. On any
install-verify failure, the worker SHALL discard the whole realm before the
next job. Cleanup SHALL run after success, error, panic, and limit failure.

#### Scenario: poisoned eval triggers recovery

- **GIVEN** an engine that evaluated `eval = 1; 0`
- **WHEN** a subsequent script evaluates `typeof eval`
- **THEN** the result is `'function'`, because identity drift on `eval` was
  detected and the realm recycled

#### Scenario: residue from a failed eval is cleaned

- **GIVEN** a script that sets `globalThis.x = 1` and then throws
- **WHEN** a later script evaluates `typeof x`
- **THEN** the result is `'undefined'` — cleanup ran on the error path

### Requirement: Integrity-set recycling

The worker SHALL verify, after each evaluation, the own keys (including symbol
keys), property descriptors, prototype identity, and expected value identity of
a named root set: the `globalThis` baseline keys, the `eval` function, and the
prototypes of `Object`, `Array`, and `Function` (this canonical text reflects
the already-shipped `String`/`Number`/`Boolean` shrink, documented by the
detector-cost benchmark). The per-eval reinstalled bindings `camel` and
`console` are excluded from the value-identity comparison (their object identity
changes each eval by design); their presence and pristinity are enforced by the
protected-invariants requirement. In the global key-set comparison the worker
SHALL ignore CONFIGURABLE additions, because the pre-eval scrub removes them. A
baseline key that disappears, any NON-configurable addition, and an added global
whose descriptor cannot be read SHALL count as drift (fail closed); on drift the
worker SHALL recycle the realm — create a fresh realm, re-capture the baseline,
and drop the wrapper cache — and SHALL never attempt restoration. The detector's
cost SHALL be benchmarked separately; if it rivals the per-eval budget, the
named set shrinks to `globalThis` keys, `eval`, and the
`Object`/`Array`/`Function` prototypes, with the shrink documented.

#### Scenario: configurable global additions do not recycle

- **GIVEN** one engine evaluating a script that creates a top-level name
  (`var y = 1; y`, `function f() {}; 0`, `async function f() {}; 0`, or
  `z = 1; z`)
- **WHEN** the script is evaluated many times
- **THEN** the worker's recycle counter stays zero (no realm rebuild), the named
  root set is unchanged, and a following `typeof y`/`typeof z` evaluates to
  `'undefined'` because the configurable binding was scrubbed

#### Scenario: an unreadable added-global descriptor fails closed

- **GIVEN** an added global whose own-property descriptor the check cannot read
- **WHEN** the integrity check runs
- **THEN** it reports drift and the realm is recycled — an unreadable
  descriptor is never accepted as a configurable addition

#### Scenario: undeletable global recycles the realm

- **GIVEN** a script that ran
  `Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0`
- **WHEN** a later script evaluates `typeof pwn`
- **THEN** the result is `'undefined'`, because the undeletable key was
  detected as drift and the realm recycled

#### Scenario: frozen prototype recycles the realm

- **GIVEN** a script that ran `Object.freeze(Object.prototype); 0`
- **WHEN** a later script evaluates `Object.isFrozen(Object.prototype)`
- **THEN** the result is `false`, because the descriptor drift was detected
  and the realm recycled

### Requirement: Expired jobs never execute

Each queued job SHALL carry its enqueue time and timeout budget. The worker
SHALL check the deadline at dequeue and SHALL skip execution — replying a
timeout error when the receiver still exists — for jobs whose budget expired
while queued.

#### Scenario: expired queued job is skipped

- **GIVEN** a worker busy with a long-running eval and a queued job whose
  timeout budget expires before the worker reaches it
- **WHEN** the worker dequeues the expired job
- **THEN** the job's script does not execute and the caller receives a
  timeout error rather than a late result

### Requirement: Determinism within a realm generation

Between recycles, first and cached evaluations SHALL return structurally
identical `JsEvalResult` values and SHALL produce the same error variant with
the same message. The error mapping SHALL keep today's surface: evaluation
errors (including syntax errors reached at eval time) map to the execution
error variant; `validate` maps parse failures to the parse error variant.

#### Scenario: identical results across repeats

- **GIVEN** one script source and one exchange snapshot
- **WHEN** the script is evaluated three times
- **THEN** all three `JsEvalResult`s are structurally equal

#### Scenario: identical errors first and cached

- **GIVEN** a script that throws `new Error('boom')`
- **WHEN** it is evaluated twice
- **THEN** both evaluations fail with the same variant and message

#### Scenario: error mapping unchanged

- **GIVEN** an invalid script source
- **WHEN** it is evaluated and separately validated
- **THEN** the eval path reports an execution error and the validate path
  reports a parse error, matching today's mapping

#### Scenario: adversarial source round-trip

- **GIVEN** sources containing single quotes, double quotes, backslashes, CR,
  LF, U+2028, U+2029, and a trailing backslash
- **WHEN** they are evaluated through the wrapper and compared against a
  reference evaluation of the raw source
- **THEN** values and errors are identical, repeated execution reuses one
  cache entry, and a syntactically invalid source via the eval path maps to
  the same variant as today (execution error), never a parse error

### Requirement: Worker thread confinement

All Boa state SHALL live and die on one dedicated worker thread per
`BoaEngine` limits-configuration. No Boa type SHALL cross the `JsEngine` trait
boundary. The inbound channel SHALL be bounded and provide backpressure; the
declared serialization tradeoff is that a wedged evaluation stalls later
evaluations up to the backlog times the per-eval limit bound (native-builtin
execution keeps today's admitted unbounded residual). Custom `JsEngine`
implementations SHALL bypass the worker entirely.

#### Scenario: concurrent expressions have no cross-talk

- **GIVEN** two `JsExpression` instances with different stateful scripts
- **WHEN** they are evaluated interleaved via `tokio::join!` on the one worker
- **THEN** each script observes only its own exchange data

#### Scenario: custom engines bypass the worker

- **GIVEN** a custom `JsEngine` implementation such as the `SlowEngine` test
  fake
- **WHEN** it is used through `JsLanguage::with_engine`
- **THEN** it compiles and behaves unchanged, without a worker

### Requirement: Declared non-guarantees are observable as declared

Engine state outside the protected invariants and integrity set MAY survive
across evaluations; the test suite SHALL pin this honesty with at least one
behavioral scenario, so a future change to the isolation model fails a named
test rather than passing silently.

#### Scenario: mutation outside the integrity set persists until recycling

- **GIVEN** a script that ran `Date.prototype.probe = 1; 0`
- **WHEN** a later script evaluates `new Date().probe`
- **THEN** the observed value is `1` — `Date.prototype` is outside the named
  integrity set, and the persistence is the declared non-guarantee in force
- **AND WHEN** a drift-triggering eval (for example, an undeletable global)
  recycles the realm and `new Date().probe` is evaluated again
- **THEN** the result is `undefined` — the recycled realm dropped the
  persisted state

### Requirement: Resource limits unchanged

Caching SHALL NOT relax any DoS cap: loop iterations, recursion depth, stack
size, source-size bound, and value-nesting depth keep their current values and
enforcement points.

#### Scenario: loop and recursion limits still trip

- **GIVEN** the existing limit tests (`while (true) {}` trips the loop cap;
  deep recursion trips the recursion cap)
- **WHEN** they run against the cached engine
- **THEN** they still fail evaluation with limit errors

### Requirement: Worker self-heal on panic

The worker SHALL catch panics in an evaluation, reply with an execution error,
recycle the realm, and drop the wrapper cache, so one bad evaluation cannot
brick the language for the process.

#### Scenario: eval panic does not brick the worker

- **GIVEN** a worker whose evaluation body panics (injected via test seam)
- **WHEN** the panic is caught and a subsequent normal script is evaluated
- **THEN** the panic surfaces as an execution error and the subsequent script
  evaluates successfully on a recycled realm

### Requirement: Bounded cache memory

The wrapper cache SHALL be an LRU with a cap of 256 entries keyed by full
source bytes. Distinct sources beyond the cap evict the least-recently-used
entry; key comparison SHALL compare source bytes, not hashes alone.

#### Scenario: LRU eviction bounds the cache

- **GIVEN** a `WorkerState` cache pre-filled with 256 distinct sources
- **WHEN** a 257th distinct source is inserted
- **THEN** the least-recently-used entry is evicted and the cache length stays
  at 256

### Requirement: Bounded promise-job retention

The worker SHALL NOT retain enqueued engine jobs across evaluations. The
worker's `Context` SHALL use a job executor that discards enqueued promise jobs,
so repeated `.then` evaluations cannot grow per-evaluation memory. `script:`
evaluation SHALL remain synchronous (ADR-0006): promise callbacks are not
executed, matching the pre-change behaviour; only the unbounded retention is
removed. Constructing the executor SHALL NOT fall back to a retaining default
executor. The executor SHALL expose a dropped-job count that tests observe
through `Context::downcast_job_executor`. Memory flatness over many evaluations
SHALL be confirmed by the empirical probe, not by a unit assertion.

#### Scenario: repeated then-scripts drop exactly N jobs

- **GIVEN** one engine evaluating `Promise.resolve(1).then(() => {}); 0` N times
- **WHEN** the evaluations complete
- **THEN** the executor's dropped-job count, read via `downcast_job_executor`,
  equals N and the recycle counter stays zero; queue depth alone is not accepted
  as evidence

#### Scenario: the discarding executor survives a realm recycle

- **GIVEN** the N-evaluation sequence above, then a forced recycle (a
  non-configurable global addition), then N more evaluations
- **WHEN** all evaluations complete
- **THEN** the dropped-job count has advanced by exactly N more and the only
  realm recycle is the forced one — the discarding executor is neither dropped
  nor replaced by the retaining default after recycling

### Requirement: Realm-recycle diagnostics

The worker SHALL emit a debug-level diagnostic when the integrity check detects
drift and when it recycles the realm. The diagnostic SHALL NOT include script
source text, exchange data, user-controlled property names, or JavaScript
values. It MAY include a static cause, fixed root labels (the existing
`root = root.name` uses the static names `eval`, `Object.prototype`,
`Array.prototype`, `Function.prototype`), and numeric counters such as the
recycle count.

#### Scenario: drift emits a debug diagnostic

- **GIVEN** an evaluation that corrupts a non-configurable root (for example
  `Object.freeze(Object.prototype)`)
- **WHEN** the integrity check detects the drift and the worker recycles
- **THEN** a debug-level tracing event is emitted for the drift and for the
  recycle, and the event contains no script source, exchange data, or
  user-controlled key/value

