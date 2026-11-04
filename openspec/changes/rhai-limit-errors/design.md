# Design: rhai-limit-errors

## Approach

Thread `ResolvedRhaiLimits` (already resolved in every eval path) into
the detail-mapping layer, then replace the generic strings with a
name+bound contract. Baseline evidence: ops and map trips both render
`Some("evaluation limit exceeded")`; the timeout renders `detail: None`
(class `Timeout`). Three distinct error surfaces exist:

1. **Eval-time `EvalAltResult`** (`eval_alt_detail`): gains a
   `&ResolvedRhaiLimits` parameter. Mapping (verified against rhai
   1.27.0 sources):
   - `ErrorTooManyOperations` → `max-operations {bound} exceeded`
   - `ErrorDataTooLarge(label)` → exact-label map (the label is static
     engine prose, used for classification only, never rendered):
     `Length of string` → `max-string-size {bound} exceeded`,
     `Size of array/BLOB` or `Size of BLOB` → `max-array-size {bound}
     exceeded`, `Size of object map` → `max-map-size {bound} exceeded`
     (each bound = the resolved value of that config key); any other
     label → `data-size limit exceeded` (name-only fallback — exact
     match keeps an unknown label containing "string"/"map" from
     acquiring a wrong bound).
   - `ErrorStackOverflow` → `max-call-levels {bound} exceeded` (both
     rhai 1.27 raise sites check only `max_call_levels`; expression
     depth is enforced at parse time).
   - `ErrorTooManyVariables` → `max-variables exceeded`;
     `ErrorTooManyModules` → `max-modules exceeded` (name-only: not
     configured by rust-camel; modules unreachable under `no_module`,
     kept defensive).
   - `ErrorTerminated` → unchanged `script terminated` (no termination
     API is exposed; the value payload stays unrendered).
   - `ErrorFunctionNotFound` → unchanged `function not found`.
2. **Tokio timeout** (`Elapsed`): the three mapping sites
   (`eval_async`, `PredicateGroup::first_match`, mutating
   `evaluate`) attach `execution-timeout {bound}ms exceeded` from the
   resolved `execution_timeout_ms`. Class stays `Timeout`.
3. **Compile time** (`PERR::ExprTooDeep` in `compile_read_only` /
   `create_mutating_expression`): `ParseError.reason` becomes
   `max-expression-depth {d} or max-function-expression-depth {f}
   exceeded`. Rhai raises one variant for both depth settings and
   cannot say which tripped — the string lists the configured candidate
   bounds; it does NOT claim both tripped. This is the documented
   requirement-6 fallback: both limits named, both bounds stated, none
   invented. `ParseError` type and fields unchanged.

Signature threading (all private): `eval_alt_detail`, `map_eval_alt`,
`eval_error` gain `&ResolvedRhaiLimits`; their callers
(`eval_with_engine`, `eval_sync`, `eval_mut_sync`, group clause eval)
already resolve or receive the limits config. `eval_alt_class` and
`map_eval_alt`'s json-guard rewrite are untouched.

## Affected crates

- `camel-language-rhai`: detail strings, signature threading, tests.
  No other crate changes (`LanguageError`, `RhaiLimitsConfig`,
  `camel-language-api` untouched).

## Architecture boundaries

Languages-layer internal. The Languages boundary contract
(`language-value-boundary` canon) is honored: details stay
operand-free static text (limit name + configured bound only), the
class set is unchanged, `EvalFailure` remains the variant. Scripts are
trusted configuration, but no script text is added to details anyway.

## Phases

Omitted — single-phase change (one crate, four ordered tasks, no
independent subsystems).

## Test strategy

TDD per task. Each limit: a script that deterministically trips it with
a **custom** bound, asserting the exact detail string
(`format!("{name} {bound} exceeded")`), plus a default-bound test
(`RhaiLanguage::new()`, operations → `max-operations 100000 exceeded`).

Label mapper: tests cover all four recognized labels exactly
(`Length of string`, `Size of array/BLOB`, `Size of BLOB`,
`Size of object map`), both defensive name-only variants
(`max-variables exceeded`, `max-modules exceeded`), and unknown labels
containing category keywords plus a secret sentinel — every one asserts
the exact detail and the unchanged `Limit` class.

Timeout: exact-detail + unchanged-class assertions cover all three
`Elapsed` mapping sites — ordinary expression evaluation and predicate
evaluation (shared `eval_async` site), grouped predicate `first_match`,
and mutating `evaluate` — each with a custom timeout.

Depth: two independent tests, each tripping exactly one depth setting
with a distinct custom bound while keeping the other setting
non-binding; both assert `ParseError` and the exact reason
`max-expression-depth {d} or max-function-expression-depth {f}
exceeded`. Negative redaction assertions fall out of exact-equality
matching (any leaked value would break equality) plus one explicit
"no script substring" check per mission. Class-unchanged assertions on
every covered path.
