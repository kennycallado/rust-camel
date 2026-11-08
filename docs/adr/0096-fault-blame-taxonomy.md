# ADR-0096: Fault blame for type-conversion errors

- Status: Accepted (decided 2026-11-05)
- Source: bd `rc-v38bd` (owner/e_opus review request)
- Ruling: expert blessing under fleet mission 499 (e_gpt gate, 2026-11-05)
- Cross-refs: ADR-0012 (handler-contract boundary), ADR-0032 (exchange-data trust
  boundary), ADR-0073 (content negotiation), ADR-0090 (pre-1.0 compatibility policy)

## Context

`CamelError::TypeConversionFailed` (`crates/camel-api/src/error.rs:282`) is the generic
conversion-failure variant. Unrelated sites raise it:

- `body_converter` (`crates/camel-api/src/body_converter.rs:43,56-83`).
- Data formats in both directions (for example
  `crates/camel-processor/src/data_format/json.rs:52-83`).
- The gRPC producer, on outbound encode and decode
  (`crates/components/camel-component-grpc/src/producer/mod.rs:310,312,525,603`;
  `producer/convert.rs:23-47`).
- OpenSearch reply parsing (`crates/components/camel-opensearch/src/producer/mod.rs:221-234`).

The two consumer surfaces disagree on blame:

- `camel-http::pipeline_error_to_reply`
  (`crates/components/camel-http/src/lib.rs:3659`) maps every `TypeConversionFailed` to
  400 (`lib.rs:3679-3681`). Client fault.
- The gRPC server maps auth errors explicitly. Every other pipeline error becomes
  `Status::internal` (`crates/components/camel-component-grpc/src/consumer.rs:57-58`,
  `consumer.rs:118-121`). Server fault.

The step layer adds a side-based heuristic. Response-side marshal failures are wrapped
in `ProcessorErrorWithSource` and map to 500. Request-side unmarshal keeps
`TypeConversionFailed` and maps to 400 (`crates/camel-processor/src/marshal.rs:36-40`).
The heuristic keys on step direction. It does not key on data provenance. An `unmarshal`
step placed mid-route on data fetched from an upstream service fails on upstream data
and still maps to 400.

Inside one format the blame already flips on the input spelling, not the operation:

- Protobuf marshal on invalid bytes raises `ProcessorError` and maps to 500
  (`crates/dataformats/camel-dataformat-protobuf/src/lib.rs:164-169`).
- Protobuf marshal on invalid JSON text raises `TypeConversionFailed` and maps to 400
  (`lib.rs:148-153`).
- Protobuf unmarshal on invalid bytes raises `TypeConversionFailed` and maps to 400
  (`lib.rs:197-203`).

Direction does not determine blame. Unmarshal of a request body at REST ingress fails on
client data. Unmarshal of an upstream reply fails on server-side data. Only the failing
step knows where the input came from.

## Decision

Blame follows input provenance at the failing step. Consumers map status by blame.

1. `TypeConversionFailed` defaults to **500**. A conversion failure on data that is not
   by definition request-borne is a server or upstream fault.
2. **400 is reserved** for failures whose failing-step input is by definition the
   request. Content negotiation reserves the neighbouring request-fault codes 415 and
   406:
   - the REST binding's injected unmarshal and schema check
     (`crates/camel-dsl/src/rest.rs:395-404`);
   - content negotiation: `UnsupportedMediaType` maps to 415, `NotAcceptable` maps to
     406 (`crates/components/camel-http/src/lib.rs:3696-3710`; ADR-0073);
   - explicit validation steps that raise `ValidationError`
     (`crates/camel-processor/src/json_schema_validate.rs:104`;
     `crates/camel-processor/src/validate.rs:61`).

   The explicit-validation exception is declared blame, not provenance. The route
   author marks the step as a request-contract check. The validation service itself
   checks no provenance (`crates/camel-processor/src/validate.rs:53-65`). An explicit
   `ValidationError` therefore stays at 400. An ordinary mid-route `unmarshal` step
   declares nothing and inherits the 500 default.
3. The gRPC server mapping stands. Auth errors map to `unauthenticated` and
   `permission_denied` (`consumer.rs:57,120`). Every other pipeline error maps to
   `Status::internal` (`consumer.rs:58,121`). After this ADR the two consumers agree on
   blame for conversion failures: non-request-borne conversion failures mean HTTP 500
   and gRPC `INTERNAL`.

Mechanism scope. This ADR fixes the blame contract. It does not fix the mechanism.
Today the binding-injected unmarshal raises `TypeConversionFailed`
(`rest.rs:399` wraps the data format; the format raises the variant, for example
`camel-processor/src/data_format/json.rs:52`). The consumer sees only the variant. The
implementation must make the request-borne seam carry its blame: reclassify at the
binding step, or attach a seam-local request-borne marker (one variant or wrapper at
the binding seam — not a variant per raise site). The implementation issue (bd,
discovered-from `rc-v38bd`) decides and implements this. A user-written `unmarshal` step
mid-route is not the binding seam. It inherits the 500 default.

## Consequences

Error handlers. Route error handlers match on `CamelError` variants. Consumer status
mapping does not touch them. If the implementation reclassifies the binding seam,
policies that match on `TypeConversionFailed` at ingress routes need review. The
implementation issue owns that review and its tests.

Metrics. Pipeline error metrics key on `CamelError::classify()`
(`crates/camel-core/src/shared/observability/adapters/tracer.rs:200-203`). Label
stability holds only for variants whose classification does not change:
`TypeConversionFailed` keeps returning `type_conversion`
(`crates/camel-api/src/error.rs:484,495`). If the implementation reclassifies the
binding seam (for example to `ValidationError`, which classifies as `validation`,
`crates/camel-api/src/error.rs:507`), that seam's counter label moves from
`type_conversion` to `validation`. The implementation issue documents and tests every
such label change. Status-code-derived observability (access logs, HTTP client/server
error splits) shifts: conversion failures move from the 4xx side to the 5xx side.

Logging and replies. Operators see three visible changes in camel-http:

- The reply status for non-request-borne conversion failures moves from 400 to 500.
- The JSON error code `bad_request` (`lib.rs:3681`) no longer names those failures.
- The log line "Type conversion failed (bad request)" (`lib.rs:3680`) changes with the
  mapping.

Doc sites that state the old pattern change with the implementation, not in this ADR:
the side-based doc comment at `crates/camel-processor/src/marshal.rs:36-40`. gRPC
operators see no change: conversion failures were already `Status::internal`.

## Migration

The workspace is pre-1.0. ADR-0090 governs. It forbids deprecation windows and
default-off compatibility surfaces ("Deletion, not hiding",
`docs/adr/0090-pre-1.0-compatibility-policy.md:55`). Position: **no opt-in window**. A
flag that restores 400-on-`TypeConversionFailed` preserves the mislabel this ADR
removes. It protects no external contract, so it is not a kept surface under ADR-0090
categories (a)-(d). The shipped `fix(http)` and `fix(rest)` commit subjects name the new
status mappings. Release notes generated from Conventional Commits record the migration.
The implementation issue is filed from the repo root, discovered-from `rc-v38bd`.

## Alternatives considered

**Blame by error direction (encode vs decode). Rejected.** Direction is not provenance.
Protobuf falsifies direction-based rules within one format: marshal on bytes is an
encode and maps to 500; marshal on JSON text is the same encode and maps to 400
(`lib.rs:164-169` versus `lib.rs:148-153`). The step-side heuristic
(`marshal.rs:36-40`) fails the same way for mid-route unmarshal of upstream data.

**Per-format mapping tables. Rejected.** A table per data format re-decides blame at
every format. The tables drift. The protobuf inconsistency is the predicted failure
mode of per-format rules.

**A new error variant per provenance. Rejected as a general scheme.** A distinct
variant at every request-borne seam gives type-level blame and a direct consumer
mapping. The cost: every request-borne seam must raise its variant, consumer match
arms change, and the 30+ existing raise sites stay on the default variant. The
rejection covers proliferation across raise sites. It does not cover one seam-local
marker at the REST binding; the implementation issue may use a binding-seam marker (a
single variant or wrapper) to carry request-borne blame. Revisit the general scheme if
request-borne seams multiply beyond the binding.

## Supersedes

Partially supersedes ADR-0073. Its 415/406 negotiation mapping stands; this ADR reserves
those codes. Its reference to "the existing `TypeConversionFailed` → 400 pattern"
(`docs/adr/0073-rest-strict-content-negotiation.md:102-103`) does not carry forward.
No other ADR states the 400 mapping.

## Evidence

Anchors verified at `main` `03c14bf29`, 2026-11-05:

| Claim | Anchor |
| --- | --- |
| Variant definition | `crates/camel-api/src/error.rs:282` |
| `classify()` category | `crates/camel-api/src/error.rs:484,495` |
| HTTP 400 mapping | `crates/components/camel-http/src/lib.rs:3679-3681` |
| HTTP `ValidationError` 400 | `crates/components/camel-http/src/lib.rs:3683-3685` |
| Negotiation 415/406 | `crates/components/camel-http/src/lib.rs:3696-3710` |
| HTTP 500 catch-all | `crates/components/camel-http/src/lib.rs:3712-3720` |
| gRPC auth + internal mapping | `crates/components/camel-component-grpc/src/consumer.rs:53-58,118-121` |
| gRPC internal serialize paths | `crates/components/camel-component-grpc/src/consumer.rs:522-530` |
| Side-based heuristic doc | `crates/camel-processor/src/marshal.rs:36-40` |
| Protobuf blame split | `crates/dataformats/camel-dataformat-protobuf/src/lib.rs:148-153,164-169,197-203` |
| REST binding seam | `crates/camel-dsl/src/rest.rs:395-404` |
| Schema validation errors | `crates/camel-processor/src/json_schema_validate.rs:104`; `crates/camel-processor/src/validate.rs:61` |
| Error metrics key on `classify()` | `crates/camel-core/src/shared/observability/adapters/tracer.rs:200-203` |
| gRPC producer raise sites | `crates/components/camel-component-grpc/src/producer/mod.rs:310,312,525,603`; `producer/convert.rs:23-47` |
| OpenSearch reply parsing | `crates/components/camel-opensearch/src/producer/mod.rs:221-234` |
| `body_converter` raise sites | `crates/camel-api/src/body_converter.rs:43,56-83` |
| JSON data format raise sites | `crates/camel-processor/src/data_format/json.rs:52-83` |
| ADR-0073 stale pattern reference | `docs/adr/0073-rest-strict-content-negotiation.md:102-103` |
