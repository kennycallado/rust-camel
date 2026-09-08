# Design: on-exceptions-wildcard

## Context

Engine-level error handling is owned by `DefaultRouteErrorHandler`
(`crates/camel-processor/src/error_handler.rs`). Policies are
`(ExceptionPolicy, Option<producer>)` pairs; `ExceptionPolicy.matches` is an
arbitrary predicate, so a catch-all policy is already representable at the
engine layer. `resolve_producer` picks the first matching policy
(`match_policy`), so ordering semantics exist today.

The declarative DSL compiles `error_handler.on_exceptions` clauses in
`crates/camel-dsl/src/compile.rs` (`compile_error_handler`, ~line 756). Two
gates reject a catch-all today:

1. A clause without `kind` and without `message_contains` is a `Config` error.
2. `ensure_known_exception_kind` rejects any kind not in
   `supported_exception_kinds()` (15 fixed variant names).

The clause matcher closure calls `exception_kind_matches(expected, e)`.

## Approach

Treat `"*"` as a reserved kind value, mirroring `CatchMatcher::ByVariant`
wildcard handling in `crates/camel-processor/src/do_try.rs` (line 26: any
`"*"` entry matches all variants).

Changes, all inside `camel-dsl`:

1. `ensure_known_exception_kind`: accept `"*"` as a separately reserved
   token, handled before the list check. `supported_exception_kinds()`
   remains the single authority for named kinds, and the unknown-kind
   diagnostic for names other than `"*"` is unchanged.
2. Clause matcher construction in `compile_error_handler`: when `kind ==
   Some("*")`, build the closure with `kind_ok = true` regardless of the
   error variant. `message_contains` still ANDs with it when present
   (a wildcard plus `message_contains` narrows by message — same combining
   rule as today, useful and consistent).
3. No model change: `DeclarativeOnException.kind` is already
   `Option<String>`; `"*"` is a value, not a shape change. YAML, JSON, and
   the generated schema need no edits.
4. Documentation: `docs/src/concepts/error-handling.md` gains a subsection
   for the wildcard clause: last-clause placement, first-match-wins
   interaction, and the full-response-ownership pattern with `handled: true`
   + `retry.handled_by`.

Rejected alternative (bd rc-tgk3f Option A): adding a `handled` flag beside
top-level `error_handler.retry`. That branch already compiles a catch-all but
duplicates clause machinery and leaves the wildcard vocabulary gap open. The
`kind: "*"` route reuses the entire clause feature set.

## Affected Crates and Boundaries

- `camel-dsl` (Runtime→DSL boundary): `src/compile.rs` matcher + validation,
  unit tests in the same file.
- `camel-test` (integration tier): one HTTP end-to-end test proving response
  ownership across distinct error kinds; one ordering test.
- Docs tier: `docs/src/concepts/error-handling.md`.
- Not affected: `camel-processor`, `camel-core`, `camel-builder`,
  components. No engine or architectural boundary is crossed.

## Ordering and Interaction

- `match_policy` scans policies in declaration order; a wildcard clause
  placed before a specific clause would shadow it for matching errors. The
  docs state: declare the wildcard last. No compile-time enforcement —
  same as Java Camel `onException` ordering, which is user-controlled.
- When `on_exceptions` is present, the top-level `error_handler.retry`
  branch is ignored (`else if` in `compile_error_handler`) — unchanged.
- A wildcard clause with inline `steps` remains subject to the existing
  registry-resolution rejection; the sanctioned full-control pattern stays
  `retry.handled_by` + `handled: true`.

## Testing Strategy

- Unit (`compile.rs` tests): `"*"` passes validation; wildcard policy
  matcher returns true for `ValidationError`, `ProcessorError`, `Io`;
  specific-then-wildcard ordering preserves first-match; `"*"` combined with
  `message_contains` narrows; unknown kinds still rejected.
- Integration (`camel-test`): HTTP consumer route whose steps fail with
  different kinds per request path; single wildcard clause with
  `handled: true` + `retry.handled_by: direct:shaper`; assert 422 + custom
  header + handler body for each kind (the probe pattern from bd rc-tgk3f).
