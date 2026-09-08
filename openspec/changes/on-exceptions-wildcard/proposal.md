# Proposal: on-exceptions-wildcard

## Why

Route-level error handling (`error_handler.on_exceptions`) requires each clause
to match an explicit error shape: `kind:` from a fixed vocabulary or
`message_contains:`. Routes that fail for many reasons — per-field validation
messages, upstream HTTP failures with dynamic message text, circuit-breaker
open — cannot express "handle every failure the same way" without enumerating
patterns. An unmatched error falls through to the default behavior without any
signal, so a missed pattern silently drops handler output (for example, a CORS
header that must appear on every error response).

Today the only sanctioned wildcard with full response ownership is
`do_try/catch: exception: ["*"]` (spec: `builder-error-policy`,
`CatchMatcher::ByVariant`). Users who want route-scoped behavior must wrap
whole routes in `do_try`, which changes route structure only to work around a
vocabulary gap. The builder API already permits a catch-all
(`on_exception(|_e| true)`), so the gap exists only in the declarative DSL.

## What Changes

Accept `kind: "*"` in an `error_handler.on_exceptions` clause as a
matches-all wildcard, symmetric with the `do_try` catch wildcard.

- `camel-dsl/src/compile.rs`: `ensure_known_exception_kind` accepts `"*"`;
  the clause matcher matches every `CamelError` variant when `kind` is `"*"`.
- First-match-wins policy order is unchanged: a wildcard clause matches only
  errors that no earlier clause matched. Documentation states that a wildcard
  clause is normally the last clause.
- All existing clause features (`handled`, `continued`, `retry.handled_by`,
  `steps`) work unchanged with the wildcard. In particular
  `kind: "*"` + `handled: true` + `retry.handled_by` gives the handler route
  full ownership of the HTTP response (status, body, headers), verified
  end-to-end on 0.41.0 during the investigation (bd rc-tgk3f).
- Documentation (`docs/src/concepts/error-handling.md`) covers the wildcard.

Out of scope: extending the `kind` vocabulary for
`ValidationError`/`Unauthenticated`/`Unauthorized` (tracked separately as bd
rc-fu1of) and any engine changes — the runtime already supports catch-all
policies.

## Acceptance Criteria

1. A route in YAML or JSON with `error_handler.on_exceptions: [{kind: "*",
   handled: true, retry: {handled_by: direct:shaper}}]` compiles, and for
   every tested error kind (`ValidationError`, `ProcessorError`, `Io`) the
   handler route output becomes the final exchange — HTTP status, body, and
   custom (string-valued) headers honored.
2. A specific clause placed before the wildcard clause still wins for its
   kind; the wildcard handles the rest.
3. Unknown kinds other than `"*"` are still rejected at compile time with the
   existing error message.
4. `cargo fmt --check`, clippy `-D warnings`, and the camel-dsl unit suite
   pass in the feature worktree.

## Risk Budget

Low. The change is additive to the DSL vocabulary in one file plus docs: the
engine (`camel-processor`, `camel-core`) is untouched. `kind: "*"` was
previously a compile error, so no existing route changes behavior. The only
new semantic — wildcard matching — reuses the existing matcher closure
machinery.
