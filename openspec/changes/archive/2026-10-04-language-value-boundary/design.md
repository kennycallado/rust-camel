# Design: language-value-boundary

## Approach

The defect sits in the glue, not the languages. `await_eval`/`await_matches`
(`step_resolution.rs`) discard `LanguageError` and substitute `Null`/`false`;
the processor callbacks they feed (`DynamicSetProperty`, `DynamicSetHeader`,
`DynamicSetHeaderIfAbsent`, dynamic `SetBody`, `FilterPredicate` users, and the
split/router/slip/recipient/idempotent/sort/claim-check closures) are infallible,
so no error path exists today.

1. **Fallible glue.** Make evaluation async and fallible end to end: change the
   camel-processor callback types to return `Result<_, CamelError>`, build the
   step services to propagate, and delete `block_in_place`+`block_on`. One
   behavior change for every Language at once (sealed Q5).
2. **Typed error.** Add `CamelError::ExpressionFailed { language, route_id,
   step_id, verb, position, class, conversion, cause }` to camel-api.
   `conversion: Option<ConversionDetail{source_type, target}>` preserves the
   refused type and the trusted destination (compile-time names only).
   `cause` is an optional boxed `CamelError`: for a failed catch-`when` or
   catch-`on_when` predicate it holds the original error, so the chain stays
   retrievable. Class enum: `runtime`, `arithmetic`, `type-mismatch`,
   `function-not-found`, `limit`, `timeout`, `conversion`, `parse`. No
   `ProcessorError` alias (sealed Q2, pre-1.0).
3. **Redaction, all Languages.** `LanguageError` transports the class and
   position as structured fields, not only as message text. Each language
   crate maps its engine errors to class + position; value-bearing error
   text never passes through. The mission-340 audit
   (`language-boundary-audit-matrix-20261002.md`) fixed the per-crate scope:
   js forwards the full Boa Display and leaks exchange data LIVE today via
   `script:` (repro R5); simple embeds coercion operands; jsonpath and
   minijinja pass engine Display through; xpath is redaction-conformant but
   owns `warn!` levels it must not (ADR-0012). All five fixes land in
   Phase 1. `throw` values stay redacted. Script source excerpts stay
   permitted (ADR-0032). Redaction lands with the glue, never after (ruling
   2.1: fixing the swallow without redaction is an ADR-0051 regression).
   Per-language VALUE defects the audit found (simple body stringification,
   xpath nodeset flattening and Inf-to-Null, js wholesale header rewrite and
   Bytes-to-null) are follow-up changes, not this change.
4. **Read-only mutation rejection.** At expression compile time, an AST walk
   in each mutating-capable language rejects `set_header()`/`set_property()`
   calls inside read-only Expressions and Predicates. The compile error names
   the call and points to `script:` (sealed Q1: immediate, no deprecation
   release).
5. **Outbound conversion.** Replace `dynamic_to_json` and `dynamic_to_value`
   with one recursive, fallible converter. Map becomes a JSON object, Array a
   JSON array. NaN/Inf, `FnPtr`, timestamps, and custom types yield a
   `conversion` error naming the type and the target. JSON u64 > i64::MAX is
   refused on the inbound path (sealed Q4). `char` becomes a one-character
   string; `Blob` becomes a byte array (one-way type changes, documented).
6. **Script integrity.** `ScriptMutator` runs a transaction. The mutating
   language evaluator captures write intent during evaluation: body assignment
   plus changed, added, and removed header/property entries. Detection lives
   inside the evaluator, before any boundary conversion. After evaluation, the
   orchestrator converts and validates every pending change first; only when
   all conversions succeed does it commit them to the Exchange. A conversion
   failure leaves the Exchange with no partial mutation. Body exposure is
   native (`Json` as Map/Array, `Bytes` as Blob, `Empty` as `()`); reading a
   stream body's value fails with a `conversion` error via a clone-counting
   marker with registered read guards (spike-verified on workspace-pinned
   rhai 1.26: materializing reads count; guard hits whose failures are
   handled in-script are forgiven per the in-script error-handling rule;
   assignment does not count; `type_of` and discarded statement reads
   materialize nothing under the Simple execution AST and are documented
   exemptions) (sealed Q6).
7. **Strict predicates.** The SPI predicate contract becomes: result is `bool`
   or `Err(LanguageError::type-mismatch)`. rhai drops its truthiness coercion;
   the other five language crates are audited in the same phase and strictened
   where they coerce (sealed Q3, no staging).
8. **Log levels.** The rhai crate downgrades its evaluation-failure `warn!`
   lines to `debug!` or removes them; the route error handler owns the signal
   (ADR-0012 handler contract).

## Affected crates

- `camel-api`: `CamelError::ExpressionFailed` + class enum (public, BREAKING).
- `camel-language-api`: predicate contract wording; error-class helper if
  shared.
- `camel-core`: `step_resolution.rs` rewritten; step compilers in
  `core.rs`, `control_flow.rs`, `splitting.rs`, `routing.rs` emit fallible
  services.
- `camel-processor`: callback types become fallible (public, BREAKING;
  `lint-non-exhaustive` clean).
- `camel-language-rhai`: redaction, unified converter, strict bool, native body
  exposure, write-back-on-change.
- `camel-language-{simple,js,jsonpath,xpath,minijinja}`:
  predicate-strictness and error-redaction audit and fix.
- `camel-test`: GH #62 regression suite.

## Architecture boundaries

Languages stay behind the Language SPI (`camel-language-api`): the contract
change is wording plus the refusal duty, not new traits. camel-core owns
orchestration and error propagation; camel-processor owns step semantics; the
language crates own conversion and redaction. Data plane never gains control
authority from an expression error: the failure is an ordinary route exception
in the ADR-0012 taxonomy, so `do_try`/`on_exception` and error metrics keep
their existing contracts. ADR-0032 fixes the redaction boundary: exchange data
never enters error text; ADR-0051 governs credential-bearing values.

## Phases

### Phase 1: fallible expression glue + redaction + strict predicates
- **Goal:** every language evaluation error fails the step with a typed,
  redacted `ExpressionFailed`; predicates are strictly boolean; read-only
  expressions cannot carry Exchange-mutating calls.
- **Dependencies:** none (first phase). Sealed decisions Q1, Q2, Q3, Q5.
- **Externally-visible types/interfaces:** `CamelError::ExpressionFailed`
  (camel-api); fallible camel-processor callback types.
- **Deliverable:** BREAKING commit(s) annotated `BREAKING:`, tagged bd rc-16aft.
- **Exit-criteria:** per-verb propagation tests (all verb rows, each with a
  do_try variant and an on_exception variant) asserting `Err` reaches both
  handlers; redaction tests prove no `SECRET` in error text, logs, and DLC
  payload for rhai and for the js live-leak repro (R5); catch-`when` and
  catch-`on_when` tests prove the predicate error is matchable as
  `ExpressionFailed` with the original error retrievable as its cause;
  strict-bool tests for simple, js, jsonpath, xpath (minijinja refuses
  predicates at compile); route-add tests reject read-only mutations
  (`set_property()`/`set_header()` in rhai, `camel.*` writes in js) with a
  message pointing to `script:`.

### Phase 2: native structured values + script integrity
- **Goal:** structured values round-trip; `script:` mutates only what changed.
- **Dependencies:** Phase 1 (typed error carries `conversion` class).
- **Externally-visible types/interfaces:** none beyond Phase 1 (behavior).
- **Deliverable:** unified converter + integrity commits, bd rc-33q5v,
  rc-7qv3r.
- **Exit-criteria:** GH #62 repro 2 prints `type=map`; header-only script
  leaves JSON body bit-identical and leaves an `Xml` (or `Bytes`) body
  variant-identical; untouched Map property stays an object; Stream body read
  fails loudly; a conversion failure during write-back leaves the Exchange
  with no partial mutation.

### Phase 3: GH #62 regression suite
- **Goal:** both GH #62 YAML repros verbatim plus the `try` statement form,
  end to end in camel-test.
- **Dependencies:** Phases 1 and 2.
- **Externally-visible types/interfaces:** none (tests only).
- **Deliverable:** regression suite commit, bd rc-qzd64.
- **Exit-criteria:** suite green in the worktree; documents both repros.

## Alternatives considered

- **Per-language gating of the swallow fix** — rejected (sealed Q5): the swallow
  is wrong for every Language; one behavior change, one release note.
- **`ProcessorError` alias for one release** — rejected (sealed Q2): pre-1.0
  clean break.
- **Truthiness with documented rules** — rejected (sealed Q3): strict bool is
  safer; staged rollout rejected (no staging).
- **Deprecation period for read-only `set_property()`/`set_header()`** —
  rejected (sealed Q1): compile-time rejection lands in Phase 1 of this
  change, without a deprecation release.
- **Materializing `Body::Stream` up to a cap** — rejected (sealed Q6): no
  working use case; fail loudly.
