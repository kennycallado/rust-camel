# Proposal: fault-blame-taxonomy

## Why

`CamelError::TypeConversionFailed` (`crates/camel-api/src/error.rs:282`) is the generic
conversion-failure variant. Many unrelated producers raise it: `body_converter`
(`crates/camel-api/src/body_converter.rs:43`), data formats in both directions
(`crates/camel-processor/src/data_format/*.rs`), the gRPC producer
(`crates/components/camel-component-grpc/src/producer/mod.rs:310,312,525,603`), and
OpenSearch outbound parsing (`crates/components/camel-opensearch/src/producer/mod.rs:221-234`).

The two HTTP-consuming surfaces disagree on blame:

- `camel-http::pipeline_error_to_reply` maps every `TypeConversionFailed` to 400
  (`crates/components/camel-http/src/lib.rs:3679-3681`) — client fault.
- The gRPC server maps only auth errors explicitly and turns every other pipeline error
  into `Status::internal` (`crates/components/camel-component-grpc/src/consumer.rs:57-58,118-121`)
  — server fault.

Inside one format the blame already flips on the input spelling, not the operation:
protobuf marshal on invalid bytes → `ProcessorError` (500)
(`crates/dataformats/camel-dataformat-protobuf/src/lib.rs:164-169`), marshal on invalid
JSON → `TypeConversionFailed` (400) (`lib.rs:148-153`). Direction does not determine
blame; only the step knows where the data came from.

The result mislabels server-produced and upstream failures as client faults, which
poisons client-error metrics, on-call triage, and retry policy.

## What Changes

- A new ADR (`docs/adr/0096-fault-blame-taxonomy.md`) fixes the blame contract:
  `TypeConversionFailed` defaults to 500; 400 is reserved for request-borne failures
  (REST binding injected unmarshal + schema check, `ContentNegotiation` 415/406,
  explicit `ValidationError`).
- Cross-links: `CONTEXT-MAP.md` Key Terms, `camel-http`/`camel-component-grpc` CONTEXT.md,
  `docs/src/data-formats/protobuf.md`.
- A bd implementation issue (discovered-from `rc-v38bd`) owns the code change. This
  change implements **no code**.

## Acceptance criteria

- ADR-0096 exists with Decision, Coverage, Consequences, Migration, Alternatives,
  and file:line evidence anchors verified at HEAD.
- No `crates/**` source is modified.
- Cross-links land in the named docs.
- The implementation issue is filed from the repo root with the correct dependency.

## Risk budget

- Docs-only: no runtime risk. Risk is stale anchors — every anchor is re-verified at
  the worktree HEAD before blessing.
- Out of bounds: mapping implementation, new error variants, metrics changes,
  `docs/audits/`, edits to existing ADRs (other than supersession notes if required).

Ref: bd `rc-v38bd`.
