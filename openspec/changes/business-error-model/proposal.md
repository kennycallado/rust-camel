# Proposal: business-error-model

## Why

A YAML route cannot raise a business error on purpose. A step that detects
`ORDER_NOT_FOUND` can only fail with a generic variant and a message string, so
downstream catch clauses and HTTP mapping match on message text. That is brittle
and leaks operator diagnostics to clients. The demo-team proposal (bd `rc-c3vds`)
asks for Apache Camel mechanics (`throwException` + `onException`) with a runtime
envelope: raise a typed business error, catch it by code or kind, and map it to
HTTP without per-operation branches.

Today: `CamelError` has no code/kind payload (`crates/camel-api/src/error.rs:261`),
`doTry` catch matches variant names or a predicate (`yaml.rs:1838`),
`on_exceptions` matches variant name plus `message_contains` (`model.rs:219`), and
`pipeline_error_to_reply` maps by variant only (`camel-http/src/lib.rs:3683`).

## What Changes

- `CamelError` gains a `Business` variant: `{ code, kind, message, data: Option<RedactedJson> }` (additive; the enum is `#[non_exhaustive]`; `data` is a Debug-redacting newtype).
- A `throw` step for YAML/JSON routes, with Simple template resolution like `set_header`.
- `doTry` catch clauses gain `code` and `business_kind` matchers, combinable with `on_when`.
- `error_handler.on_exceptions` clauses gain `code` matching. The existing `kind` namespace (variant names) stays unchanged.
- Simple exposes `${exception.code}`, `${exception.kind}` (the `classify()` name; parse lands via prerequisite `rc-7qv90`), `${exception.business_kind}`, and `${exception.data.<field>}`.
- HTTP: an error mapping (kind to status, code to status; kebab-case kinds) rides the route as a separate field in `camel_api::http_error_mapping` (never the error-handler config) and reaches `pipeline_error_to_reply`; `rest:` blocks inherit it while keeping `error_handler: None`. Unmapped business errors return 500, never 2xx; the reply arm also maps `Ok` exchanges that still carry an error (bd `rc-7lkp2`). Reply bodies contain only the business `code` and `kind`; `message` and `data` never reach a client (ADR-0032).
- Policy: business errors log at debug (raise), info (handled), and at info/warn on propagation (transport-owned on seam routes: info below 500, warn at 500-or-above including the built-in 500 closure; non-seam unmatched propagates log warn); `data` is redacted; declarative retry never redelivers them; they bypass the default dead-letter channel only on seam routes (mapping present — review C1; non-seam routes keep Camel-parity DLC dispatch); they count as SUCCESS for both circuit breakers (Q4).

Excluded (their own missions/issues): `errors:` catalog, lint/openapi work
(phase 3); WASM (`rc-1d1ix`), validator (`rc-fqaqg`), `camel test` expected
failure, SQL constraint log level (`rc-y6fkh`) (phase 4). `${exception.kind}`
parsing is bug `rc-7qv90`; this change specifies the final surface but does not
implement that parser fix.

This change ships design artifacts (proposal, design, spec deltas, tasks, ADR
draft `docs/adr/0097-business-error-envelope.md`). Implementation runs as a
follow-up mission after owner review.

## Acceptance criteria

- `throw` raises `CamelError::Business` with resolved template values.
- Catch by `code` works in `doTry` and combines with `on_when`; `on_exceptions` matches by `code`.
- An unmapped business error maps to 500 and never produces a 2xx reply.
- Mapped kinds and codes produce the configured status without per-operation branches; `rest:` inherits the mapping.
- Business errors log at debug/info/warn per the policy (warn for mapped-500-or-above propagation) with `data` redacted; declarative retry never redelivers them; circuit-breaker failure counters stay flat (business errors count as success).

## Risk budget

- Additive variant on a `#[non_exhaustive]` enum: compatible with the API contract (see `contract-api-stability`).
- Namespace risk (new `business_kind` field vs overloading `kind`): resolved by distinct field names; see design decision table.
- Trust-boundary risk on reply bodies: governed by ADR-0032; `message` and `data` never reach a client — no mapping authorizes their disclosure.

Affected crates: camel-api, camel-dsl, camel-processor, camel-http, camel-language-simple, camel-core (tests in camel-tests). Bd: `rc-c3vds`.
