# Design: fault-blame-mapping

## Approach

ADR-0096 fixes blame: `TypeConversionFailed` defaults to 500; 400 is reserved for
request-borne failures. Two seams implement it.

**1. Consumer mapping (camel-http).** The `TypeConversionFailed` arm of
`pipeline_error_to_reply` (`crates/components/camel-http/src/lib.rs:3683-3686`) moves to
`json_error_reply(500, "conversion_failed", msg)` with log line
"Type conversion failed (internal server error)". The reply stays structured JSON so
observability tooling that parses the `error` field keeps working; only the status, code,
and message change. `Unauthenticated` 401, `Unauthorized` 403, `ValidationError` 400,
negotiation 415/406, and the 500 catch-all stay untouched. Doc comments on
`pipeline_error_to_reply` and `json_error_reply` update with the mapping.

**2. Request-borne seam (camel-dsl → camel-processor).** Chosen mechanism: **reclassify
at the binding step**. Justification: the binding IS the route author's declared request
contract (`consumes`/`produces`/`request_schema`); the ADR's declared-blame semantics for
`ValidationError` apply verbatim. Reclassification reuses the existing, fully tested
ValidationError surface — HTTP 400 arm, `validation_error` JSON code, log line,
`classify() == "validation"`, `on_exceptions` kind — with zero new `CamelError` variants
(no exhaustive-match churn, no `variant_name`/`classify` additions, no new doTry catch
surface). It is symmetric with the established `ResponseMarshal` precedent: a
lowering-injected internal step whose blame differs from the authored step. The cost — the
seam's metric label moves `type_conversion` → `validation` — is exactly the documented,
tested label change the ADR anticipates (binding decision 4).

Transport of the seam marker, mirroring `ResponseMarshal`:
- `rest.rs:397-407` marks the binding-injected `UnmarshalStep` with a
  programmatic-only field (`request_borne: bool`, `#[serde(skip)]` +
  schema/ts skip — YAML cannot author it; `deny_unknown_fields` rejects it in
  route files; the field defaults to `false` for authored steps and is `true`
  only for the lowering-injected request unmarshal). What stays stable is the
  canonical step variant/format sequence — the rendered lowered AST keeps
  `Unmarshal(json)` — not derived-`Debug` byte identity of the marker-bearing
  structs; assertions that structurally match `RouteDslStep::Unmarshal` keep
  passing.
- The yaml.rs lowering carries the marker into the internal `DataFormatDef`
  (plain bool field, default `false`; `DeclarativeStep` is not
  user-deserialized) and into the compiled step.
- `compile.rs` `DeclarativeStep::Unmarshal` arm builds the chain via a new
  `UnmarshalService::for_request_binding(inner, df)` constructor (camel-processor, beside
  the existing `MarshalService::for_response`). Reclassification boundary: only
  the format invocation's `TypeConversionFailed` is mapped —
  `Err(TypeConversionFailed(msg))` → `Err(ValidationError("request body rejected: {msg}"))`.
  Schema-check `ValidationError`, `StreamCacheService` errors, and every
  other variant pass through unchanged. The schema-wrap and
  `StreamCacheService` layering stays identical.
- User-authored `unmarshal` steps keep `UnmarshalService::new` →
  `TypeConversionFailed`.

Planning must cover: marker rejection in YAML (`deny_unknown_fields`),
schemars/TS invisibility of the skipped field, marker transport and default
(`false`) through parse → lower → compile, and the inner-error pass-through
boundary (a test that a non-conversion error from the chain is not
reclassified).

**3. Metrics.** `classify()` labels do not move for `TypeConversionFailed`
(`type_conversion`), `ValidationError` (`validation`), or the negotiation/auth variants.
Only the seam's counter label moves as a consequence of reclassification; pinned by
`error_classification_label_is_pinned`.

**4. Error handlers (ADR-0096 Consequences review).** Route error handlers match kinds via
`exception_kind_matches` (`camel-dsl/src/compile.rs:948`); both `TypeConversionFailed` and
`ValidationError` are already supported kinds, so no matcher change. Review result: an
`on_exceptions: TypeConversionFailed` clause at an ingress route no longer matches
binding-seam failures (they surface as `ValidationError`; a `ValidationError` clause or
`*` now covers them); mid-route unmarshal and authored-marshal failures still match
`TypeConversionFailed`. Pinned by tests; matcher semantics unchanged (non-goal).

**5. gRPC stands.** `pipeline_error_to_status` (`consumer.rs:118-123`) untouched; pinned
by `grpc_conversion_failure_stays_internal`.

**6. Docs.** `crates/camel-processor/src/marshal.rs:36-40` doc comment states the blame
rule instead of the direction heuristic; crate `CONTEXT.md` files stating the old 400
mapping are corrected; `CHANGELOG.md` gains an Unreleased entry; no opt-in window
(ADR-0090). `docs/src/data-formats/protobuf.md` already cites ADR-0096 — consistent, no
edit. gRPC operators see no change (conversion failures were already `INTERNAL`).

## Affected crates

- `camel-http`: mapping arm + doc comments (+ unit tests).
- `camel-dsl`: seam marker on `UnmarshalStep`, lowering, compile-arm constructor choice,
  internal `DataFormatDef` field (+ unit/e2e tests).
- `camel-processor`: `UnmarshalService::for_request_binding`, doc comment (+ unit tests).
- `camel-component-grpc`: pinning test only.
- `camel-api`: `classify()` pinning test only.
- `CHANGELOG.md`, crate `CONTEXT.md`: docs.

## Architecture boundaries

Data formats keep raising `TypeConversionFailed` unchanged (raise sites are out of scope);
blame is applied by the seam (DSL layer) and the consumer (transport layer). The DSL owns
provenance marking; camel-processor owns the reclassifying service; camel-http owns status
mapping. No cross-boundary leakage: camel-core error handlers are untouched.

## Alternatives considered

- **Seam-local marker variant** (e.g. `CamelError::RequestBindingFailed`): allowed by the
  ADR but rejected — new enum variant touches `variant_name`, `classify`, every consumer
  match, and doTry docs, and adds a second JSON code/log site, for the same observable
  outcome. Revisit if request-borne seams multiply beyond the binding (ADR guidance).
- **Direction heuristics / per-format tables**: rejected by ADR-0096 itself.
- **New `DeclarativeStep`/`RouteDslStep` variant for the seam**: rejected — would change
  rendered lowered ASTs and rest-dsl canonical step-sequence scenarios; the field marker
  keeps every canonical scenario verbatim.
