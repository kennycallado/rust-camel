# Design: 362-jsleak

## Approach

Primary — realm leak. Make the global own-key check in
`IntegrityBaseline::verify` (integrity.rs:181-202) addition-aware:

- every baseline key MUST still be present (deletion remains drift);
- a current key absent from the baseline is drift ONLY if its descriptor
  resolves `configurable = false`; a configurable addition is ignored;
- if the added key's descriptor lookup fails, treat it as drift (fail closed).

Basis (pinned engine + probe): the wrapper is `eval("<source>")`, so user code
runs as ECMAScript eval code. Boa's eval path creates top-level
`var`/`function` bindings with `configurable = true`
(`eval_declaration_instantiation` → `create_globals(codeblock, true)`);
undeclared assignment and `async function` bindings are configurable too. The
existing `scrub_globals` (worker.rs:547) deletes exactly those keys on the next
eval, so ignoring them removes the false drift without weakening enforcement:
non-configurable tamper and deleted baseline keys still recycle. No wrapper
change, so JS semantics and 359's `camel.body` write-back are untouched. The
canonical requirement states the named prototype set as `Object`/`Array`/
`Function`, aligning the spec with the already-shipped
`String`/`Number`/`Boolean` shrink (~47 µs detector cost).

Diagnostics. Emit `tracing::debug!` in `verify` (static cause, plus counts) and
in `recycle` (worker.rs:613, `recycle_count`). Never log script source,
exchange data, user-controlled property names, or JS values; fixed root labels
are allowed (the existing `root = root.name` uses the static names `eval`,
`Object.prototype`, `Array.prototype`, `Function.prototype`). A
`js_realm_rebuilds_total` metric is not trivial (no metrics binding in the
crate) and is out of scope.

Secondary — job retention. Install an in-crate job executor that discards
enqueued promise jobs. It preserves observable behaviour (jobs already do not
run) and keeps `script:` synchronous (ADR-0006). `ContextBuilder::build()`
returns `JsResult<Context>` and `WorkerState::new` returns `Self`, so
construction uses `.expect("...")  // allow-unwrap` with a one-line explanation
— the same panic-on-failure behaviour `Context::default()` already has
(`build().expect(...)`) — and MUST NOT fall back to the retaining default
executor on failure. The executor is created inside `WorkerState::new`, so the
panic rebuild path re-installs it. It retains nothing, so there is no
`run_jobs()` loop, no budget interaction, and nothing to clean on error paths.
Because `recycle()` keeps the same `Context` (`create_realm`), the executor and
its dropped count survive a recycle; the regression re-runs the sequence after
a forced recycle to prove no fallback.

Regression evidence. The promise test reads the executor's dropped-job count
through `Context::downcast_job_executor` and asserts it equals N with zero
recycles — queue depth alone is vacuous. Memory flatness is empirical (probe:
20k evals 17–31MB; 20k jobs dropped), not a unit assertion. `recycle_count`
stays the realm test seam.

## Affected crates

- camel-language-js: `engines/integrity.rs` (verify logic, drift debug),
  `engines/worker.rs` (executor install, recycle debug, module docs), crate
  `CONTEXT.md` (isolation-contract wording).

## Architecture boundaries

Languages only. No API, DSL, Component, Service, or Runtime change. No public
signature changes; `WorkerState` stays worker-thread-confined. ADR-0006
(synchronous `script:`), ADR-0032 (exchange data never interpolated), and the
JS `CONTEXT.md` sandbox contract govern the wording.

## Alternatives considered

- Wrap user code in a function scope. Rejected: unnecessary (eval-created
  bindings are already configurable) and it changes completion-value, `this`,
  and `arguments` semantics.
- `ctx.run_jobs()` after each evaluation. Rejected: unbounded by design (a job
  can enqueue more), spends the evaluation budget, and would run `.then`
  callbacks for the first time — changing observable async semantics and
  risking mutations applied after `extract_camel_state`, breaking 359.
- String-parse the source to exclude `var`/`function` names. Rejected: brittle
  heuristics for no gain.
- Falling back to the default job executor when `build()` fails. Rejected:
  silently re-introduces the retention leak; panic instead, as
  `Context::default()` already does.
- A bounded executor that runs up to N jobs. Rejected: still changes async
  semantics and adds budget/error-path complexity with no contract benefit.
