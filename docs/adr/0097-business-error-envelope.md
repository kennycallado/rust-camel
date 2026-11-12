# ADR-0097: Business error envelope, namespace, and HTTP mapping trust boundary

- Date: 2026-11-12
- Status: Proposed (rev 2; owner decisions Q1-Q8 applied, bd `rc-c3vds`, mission 574)
- Amends: none
- Cross-refs: ADR-0019 (error disposition / RouteErrorHandler), ADR-0024
  (PipelineOutcome), ADR-0025 (outcome-aware structural EIPs), ADR-0032
  (exchange-data trust boundary), ADR-0092 (Simple scalar expressions),
  ADR-0096 (fault blame for type-conversion errors), bd `rc-7qv90`
  (`${exception.kind}` parse prerequisite), bd `rc-7lkp2` (Ok-with-error 2xx
  hole, closed by this epic's reply arm), spec `dsl` "on_exceptions
  exception-kind vocabulary coverage"

## Context

A YAML route cannot raise a business error deliberately. The only failure
channel is a generic `CamelError` variant with a message string, so catch
clauses and HTTP replies discriminate by message text (`message_contains`) or
by variant. Neither scales to application-level outcomes such as
`ORDER_NOT_FOUND`, and message-text replies leak operator diagnostics to
clients. The demo-team proposal (bd `rc-c3vds`) asks for the Apache Camel
mechanics this framework already has (`throwException`/`onException` parity)
plus a runtime envelope: raise a typed business error, catch it by code or
kind, expose it to expressions, and map it to HTTP without per-operation
branches. The owner reviewed the first park (e_opus recommendations Q1-Q8,
2026-11-12) and this revision incorporates them.

## Decision

### 1. Envelope

`CamelError` gains one variant:

```rust,ignore
Business {
    code: String,
    kind: String,
    message: String,
    data: Option<RedactedJson>,
}
```

The name is neutral: this is integration plumbing, not a DDD type system; the
demo team's own review rejected a domain-typed hierarchy. `code` identifies the
application outcome (`ORDER_NOT_FOUND`); `kind` is a coarse category consumed
by default HTTP mapping, written in kebab-case matching the existing error
kind strings (`invalid-argument`, `conflict`, `not-found`, `forbidden`,
`failed`); `message` is log-and-diagnostics only; `data` carries optional
structured diagnostics. `data` is typed as `RedactedJson`, a newtype over
`serde_json::Value` whose `Debug` prints a redaction marker and never the
payload, so accidental structured logging cannot leak exchange data; the
newtype preserves `PartialEq`/`Eq` through the inner value.

The constructor contract is documentation-grade, not type-enforced: the fields
are plain public strings, and the variant's rustdoc (this ADR is the second
half of that contract, per review finding M1) states that builders validate
non-blank `code`/`kind` at their own boundary — the `throw` step rejects blank
identifiers at load time — that `message` is log-only and never reaches a
client, and that `data` is redacted diagnostics. `classify()` returns
`"business"`; `variant_name()` returns `"Business"`, so the variant joins the
existing exception-kind vocabulary and the wildcard catch keeps working.

The `throw` step takes `code` (required literal), `kind` (optional literal,
default `"failed"`), `message` (optional Simple template, defaulting to
`code`), and `data` (optional object; string leaves are Simple templates,
non-string leaves pass through, nested containers interpolate recursively).
Template resolution applies to `message` and `data` only. Structural problems
(blank identifiers, non-object `data`) fail at load time; template evaluation
failures fail the step at runtime with a typed error
(`language-value-boundary`).

### 2. Namespace

Clause matchers gain two new field names, `code` and `business_kind`. The
existing `kind` field on `on_exceptions` (and `exception` on `doTry` catch)
keeps its current meaning: `CamelError` variant names, guarded by the
vocabulary-coverage test. Overloading `kind` was rejected: it would make one
field name two namespaces (`"Io"` a variant, `"conflict"` a payload category),
break the vocabulary guard's mental model, and make every existing route
document ambiguous. `business_kind` mirrors the envelope's field name and is
grep-able.

The two clause grammars extend differently, and that asymmetry is deliberate:

- `doTry` catch: `code` and `business_kind` are main matchers in the
  exactly-one-main-matcher grammar (`exception`, `when`, `code`,
  `business_kind` mutually exclusive); `on_when` remains a refinement
  predicate combining with any main matcher except `when`.
- `on_exceptions` clauses: `code` is one more conjunctive matcher beside
  `kind` and `message_contains`; the clause keeps its flat
  all-conditions-must-hold shape. `business_kind` on `on_exceptions` is
  deferred to the phase-3 catalog (owner decision Q6); variant `Business` +
  `code` covers current needs.

Simple resolution (owner decision Q2): `${exception.kind}` resolves to the
`classify()` name for every error — `business` for business errors — never to
the payload category; the payload category surfaces as
`${exception.business_kind}`. This keeps `kind` single-meaning with the
`CamelExceptionKind` property. The `${exception.kind}` parse support is bd
`rc-7qv90`, which the owner set as a PREREQUISITE of the implementation
mission: it lands first, and the business Simple paths (`code`,
`business_kind`, `data`) build on it.

### 3. HTTP mapping trust boundary

The mapping type `HttpErrorMapping { codes, kinds }` lives in its own
camel-api module `camel_api::http_error_mapping` (a sibling of, not inside,
the error-handler module) and rides the ROUTE as a separate route-level field — `RouteDslRoute.
http_error_mapping` in the DSL AST, carried through the route translation into
the compiled route — NOT as a field of `ErrorHandlerConfig` (owner decision
Q1). The mapping is orthogonal to the error handler: `rest:` lowering keeps
`error_handler: None` exactly as today and only sets the new field from the
rest block's `error_mapping` tables. A route may carry a global dead-letter
channel and a mapping at once; the two do not couple. The DSL AST field is
`#[serde(skip_deserializing)]`, schema-skipped, and `#[ts(skip)]`: plain-route
YAML authoring is deferred to the phase-3 `errors:` catalog (owner decision
Q5), and the rest block is the only document-level authoring path in this
scope. Rest lowering always sets `http_error_mapping: Some(_)` on its lowered
routes — with empty tables when the block declares none — marking them as
seam routes (see the dead-letter rule below).

Load-time validation is fail-closed: blank keys and every status outside
400-599 — informational, success, and redirect values such as 101, 200, and
304 — are rejected with a typed error. Because programmatic construction can
bypass document validation, the finalizer also defends: a mapping entry whose
value falls outside 400-599 is treated as absent at reply time, and normal
status resolution resumes — the remaining tables, then the built-in default
kind map, then 500. Status resolution precedence: explicit code map, explicit
kind map, built-in default kind map (`invalid-argument` 400, `conflict` 409,
`not-found` 404, `forbidden` 403, `failed` 500 — kebab-case, owner decision
Q3), 500.

The mapping reaches the transport through the trusted registration seam: the
route's mapping travels as an immutable snapshot on the consumer's registration
context (`ConsumerContext`), persisted in the route runtime state and
installed on both start and resume, captured per registration and passed to
`pipeline_error_to_reply` per invocation. No global or ambient lookup exists.

The reply body for a business error is exactly `{"error": <code>, "kind":
<kind>}`. The boundary follows ADR-0032. `code` and `kind` are operator-authored
constants from the `throw` step: trusted, safe to echo. `message` is a Simple
template evaluated against exchange data, and `data` is exchange-derived
payload: untrusted, and therefore never serialized into a reply body and never
logged (`RedactedJson` makes accidental Debug logging print `[redacted]`). No
mapping configuration, disposition, or status assignment authorizes `message`
or `data` disclosure — an explicit mapping changes the status, never the
body's trust class. A business error that no mapping covers still maps to
500 — an unmapped business outcome must never produce a 2xx reply. This
extends, rather than amends, the ADR-0096 fault-blame ruling: client-fault
statuses (4xx) come only from explicit configuration or the built-in kind map,
never from error payload.

One boundary hole this epic closes along the way (review finding C1, bd
`rc-7lkp2`): the HTTP `Ok` reply arm can receive an exchange whose error is
still present (the boundary Propagate path returns `Ok(ex_with_error)`).
The reply arm SHALL map any error-carrying exchange like any other pipeline
error — the route's error mapping for a business error, the existing variant
mappings for non-business errors — and never answer
2xx for it. `rc-7lkp2` remains the tracking issue; the fix rides this epic's
reply-arm work and the closure tests cover the path.

### 4. Policy and transport scope

Global policy, derived from the variant (owner decisions Q4 and Q7):

- Redelivery: declarative route redelivery never redelivers a business error
  (clause-level and route-level retry blocks; the matching disposition still
  runs once). This governs declarative route redelivery only — broker-level
  client redelivery after an unhandled consumer failure is a transport
  concern and is NOT governed or changed (M2 note).
- Dead-letter (C1 correction): the business dead-letter bypass is CONDITIONAL
  on the HTTP reply seam — routes carrying `http_error_mapping: Some(_)` (rest
  lowering always sets it) skip default dead-letter dispatch so mapping fires
  behind a global DLC; the bypass also covers a matched clause without
  `handled_by`, which otherwise falls back to the DLC, and a business error
  never reaches the no-handler `error!` terminal site. Non-seam routes keep
  Apache Camel parity: `throwException` lands in the dead-letter channel when
  one is configured. An unconditional bypass was rejected: it silently loses
  business messages on seda/kafka/jms InOnly routes behind a global DLC.
  Explicit `on_exceptions` clauses still catch business errors as configured
  on every route; the DLC keeps receiving non-business errors everywhere.
- Circuit breakers: a business error counts as a SUCCESS for both breaker
  implementations (`CircuitBreakerService`, `CircuitBreakerGate`) in every
  state — a half-open probe ending in any business error completes as a
  success and the breaker closes. The tracer's error metrics keep the
  `CircuitOpen`-style exclusion as observability; the tracer is not a breaker
  implementation.
- Logs: raise = debug; handled (clause, catch, `handled_by`/send-to-handler
  delegate) = info; propagate logging is transport-owned and never doubled —
  on seam routes the error handler logs debug only and the finalizer logs
  info below 500 / warn at 500-or-above including the built-in 500 closure
  (owner decision Q7); on non-seam routes the error handler logs the unmatched
  propagate at warn. Records carry `code` and `kind` and never serialize
  `data`.

Transport scope: `direct:` propagates the error unchanged in every mode. The
producer-blind rule applies to fire-and-forget delivery only (InOnly
`seda:`/broker sends the producer does not wait for): the consumer-side
matched clauses own the failure. SEDA InOut and `waitForTaskToComplete=Always`
keep their existing propagation contracts and deliver the consumer-side
result, including the business error, to the producer; a broker InOut producer
whose consumer fails unhandled observes its existing reply-timeout contract.

## Consequences

- Business errors are policy-adjacent by construction: the global policy
  derives from the variant, so no per-kind configuration exists to drift.
- The mapping/error-handler separation (Q1) means mapping adoption never
  changes retry behavior, and vice versa; the dead-letter interaction is the
  one deliberate coupling: mapping presence (the seam) toggles the business
  DLC bypass. The phase-3 `errors:` catalog can promote the rest-block tables
  to plain-route YAML authoring without re-plumbing.
- `rc-7qv90` is a hard prerequisite for the Simple work (owner decision Q2);
  `rc-7lkp2` is fixed en route by the reply-arm work and closed with the
  implementation commit reference.
- Adding `business_kind` to `on_exceptions` clauses later is additive if a
  demonstrated need appears; the phase-3 catalog is the natural home (Q6).
