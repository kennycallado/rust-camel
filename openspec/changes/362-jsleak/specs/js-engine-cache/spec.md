## MODIFIED Requirements

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

## ADDED Requirements

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
