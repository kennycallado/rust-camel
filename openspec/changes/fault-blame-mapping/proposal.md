# Proposal: fault-blame-mapping

## Why

ADR-0096 (`docs/adr/0096-fault-blame-taxonomy.md`, bd rc-v38bd) fixed the blame contract:
`TypeConversionFailed` defaults to **500** (server/upstream fault); 400 is reserved for
failures whose failing-step input is by definition the request. Today the two consumer
surfaces disagree: `camel_http::pipeline_error_to_reply` maps every `TypeConversionFailed`
to 400 with the JSON code `bad_request`, while the gRPC server already maps every
non-auth pipeline error to `Status::internal`. The HTTP mapping mislabels upstream
failures as client faults, and the step-layer side heuristic (`camel-processor/marshal.rs`)
keys on direction, not provenance — a mid-route `unmarshal` failing on upstream data still
reports 400. This change implements the ADR; it does not re-open the decision (bd rc-zwd3l).

## What Changes

- `camel-http` `pipeline_error_to_reply`: `TypeConversionFailed` → 500 with a JSON error
  code and log line that no longer claim a bad request. `Unauthenticated` 401,
  `Unauthorized` 403, negotiation 415/406, explicit `ValidationError` 400 unchanged.
- REST binding seam (`camel-dsl` rest lowering + step compiler): the binding-injected
  unmarshal/schema check carries request-borne blame explicitly by reclassifying its
  failure to `ValidationError` at the seam. Mid-route user-written `unmarshal` keeps
  `TypeConversionFailed` (500 default).
- Metrics: `TypeConversionFailed` keeps `classify() == "type_conversion"`; the binding-seam
  label moves `type_conversion` → `validation` — documented and pinned.
- Error handlers: `on_exceptions` vocabulary already covers both kinds; policies matching
  `TypeConversionFailed` at ingress routes reviewed and the result tested (no matcher
  change).
- Docs: correct the side-based doc comment in `camel-processor/marshal.rs`, crate
  `CONTEXT.md` statements of the old mapping, `CHANGELOG.md` Unreleased entry. No opt-in
  window (ADR-0090).
- gRPC mapping stands unchanged, pinned by a test.

Out of scope: new error variants per raise site, direction heuristics, ADR-0096
re-litigation, unrelated `pipeline_error_to_reply` refactor, data-format raise-site changes.

## Acceptance criteria

- `conversion_failure_maps_to_500` — non-request-borne `TypeConversionFailed` yields 500;
  JSON code and log line no longer say `bad_request`.
- `binding_seam_failure_maps_to_400` — REST-binding injected unmarshal/schema failure
  yields 400 through the chosen mechanism.
- `mid_route_unmarshal_failure_maps_to_500` — user-written `unmarshal` failing on pipeline
  data yields 500 (seam distinction, not direction).
- `validation_error_still_maps_to_400`.
- `content_negotiation_codes_unchanged` — 415 `UnsupportedMediaType`, 406 `NotAcceptable`.
- `grpc_conversion_failure_stays_internal` — pinning test on the unchanged gRPC mapping.
- `error_classification_label_is_pinned` — `classify()` pinned for every variant the
  change touches.
- Error-handler review result stated in the mission park; delta specs validated.

## Risk budget

Acceptable: observable 400→500 shift for non-request-borne conversion failures (breaking,
covered by CHANGELOG entry, no opt-in window per ADR-0090); binding-seam metric label move.
Out of bounds: new `CamelError` variants, gRPC/negotiation mapping changes, error-handler
matcher semantics changes, raise-site changes in data formats.
