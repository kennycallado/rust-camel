# Design: fault-blame-taxonomy

## Approach

Docs-only change. The deliverable is ADR-0096 plus durable cross-links and a follow-up
implementation issue. The ADR fixes the blame contract for type-conversion errors; the
code change it sanctions is owned by the bd follow-up (discovered-from `rc-v38bd`).

**The decision the ADR records:**

1. `TypeConversionFailed` defaults to **500**. A conversion failure whose failing-step
   input is not by definition the request body is a server or upstream fault: the data
   came from another route, another service, or a producer-side encode.
2. **400 is reserved** for errors whose failing-step input is by definition the request:
   - the REST JSON binding's injected unmarshal + schema check
     (`crates/camel-dsl/src/rest.rs:395-404`);
   - content negotiation — `UnsupportedMediaType` → 415, `NotAcceptable` → 406
     (`crates/components/camel-http/src/lib.rs:3696-3710`);
   - explicit validation steps raising `ValidationError`
     (`crates/camel-processor/src/json_schema_validate.rs:104`,
     `crates/camel-processor/src/validate.rs:61`).

   The explicit-validation exception is declared blame, not provenance: the route author
   marks the step as a request-contract check, while the validation service checks no
   provenance (`crates/camel-processor/src/validate.rs:53-65`). Explicit
   `ValidationError` stays 400; an ordinary mid-route `unmarshal` step inherits the 500
   default.
3. Direction (encode vs decode) does not determine blame. Unmarshal at REST ingress is a
   client fault; unmarshal of an upstream reply is a server fault. The protobuf format
   already proves direction-based rules fail: marshal on invalid bytes → `ProcessorError`
   (500) (`crates/dataformats/camel-dataformat-protobuf/src/lib.rs:164-169`), marshal on
   invalid JSON → `TypeConversionFailed` (400) (`lib.rs:148-153`), unmarshal on invalid
   bytes → `TypeConversionFailed` (`lib.rs:197-203`).
4. The gRPC server mapping (auth errors explicit, all other pipeline errors
   `Status::internal`, `crates/components/camel-component-grpc/src/consumer.rs:57-58,118-121`)
   already matches the default-500 position for non-request-borne conversion failures and
   stays.

**Mechanism note (owned by the implementation issue):** the injected binding unmarshal
raises `TypeConversionFailed` today (`rest.rs:399` wraps the data format). The consumer
sees only the variant, so the request-borne seam must carry its blame explicitly —
reclassify at the binding step, or attach a seam-local request-borne marker (one
variant or wrapper at the binding seam; NOT a variant per raise site). The ADR fixes
the blame contract; the implementation issue picks the mechanism, and documents and
tests any error-category label change a reclassification causes (for example
`type_conversion` → `validation` at the seam, `camel-api/src/error.rs:495,507`).

**Migration position:** pre-1.0, per ADR-0090 ("Deletion, not hiding"). No opt-in window:
a default-off flag that restores 400-on-`TypeConversionFailed` preserves the mislabeling
this ADR removes and is a hiding surface under ADR-0090. Ship with a changelog entry.

## Affected crates

- None. No `crates/**` source changes.

Docs and process surfaces:

- `docs/adr/0096-fault-blame-taxonomy.md`: new ADR (next free number; last is 0095).
- `CONTEXT-MAP.md`: Key Terms entry for fault blame (`## Key Terms`, line 126).
- `crates/components/camel-http/CONTEXT.md`: cross-ref where the error reply path is
  described (line 115 mentions `pipeline_error_to_reply`).
- `crates/components/camel-component-grpc/CONTEXT.md`: cross-ref at the status-mapping
  description (lines 46-58).
- `docs/src/data-formats/protobuf.md`: note the blame split (marshal-on-bytes vs
  marshal-on-JSON) pending the implementation issue.
- bd: new implementation issue filed from the repo root.

## Architecture boundaries

Runtime/DSL/Components split is untouched — this change writes prose only. The ADR
documents a consumer-side contract (camel-http reply mapping, camel-component-grpc
status mapping) and a DSL-owned request-borne seam (rest binding, `camel-dsl`). Error
taxonomies follow ADR-0012 (handler-contract boundary); the trust vocabulary follows
ADR-0032 (exchange-data trust boundary). Compatibility posture follows ADR-0090.

## Alternatives considered

- **Blame by error direction** — rejected: direction is not provenance (same decode
  direction, different blame at REST ingress vs upstream reply).
- **Per-format mapping tables** — rejected: format-specific hacks reproduce the protobuf
  inconsistency and drift with every new format.
- **New error variant per provenance** — rejected as a general scheme (touches 30+
  raise sites for one seam); a single seam-local marker at the REST binding remains
  available to the implementation issue. Revisit the general scheme if request-borne
  seams multiply beyond the binding.
