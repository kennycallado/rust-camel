# Proposal: add-rest-strict-negotiation

## Why

REST DSL v2 landed L1 (`binding: raw`, commit-f5f2ace8-era) and L3
(streaming contract, archived 2026-09-11). The consumer still accepts
ANY request media type against ANY declared `consumes` and answers ANY
`Accept` header: a `text/plain` POST against an `application/json`
operation fails late as a 400 from JSON unmarshal, and an
`Accept: application/xml` request still receives JSON. The HTTP
registry (`crates/components/camel-http/src/registry.rs:51`,
`rest_endpoints: Vec<RestEndpoint<T>>`) is method+path only — media is
invisible to routing, so nothing today enforces the media contract the
DSL already declares.

bd: rc-hlb1q (REST DSL v2 L2). Sealed ruling: e_opus, amended by the
user fact ZERO ADOPTION (no retrocompat constraints).

## What Changes

**In:**

- `openspec/changes/add-rest-strict-negotiation/specs/rest-dsl/spec.md`
  — three ADDED requirements on the `rest-dsl` capability: "REST
  request media enforcement", "REST response representation
  negotiation" (media-range precedence, equal-specificity ties fail
  closed), "Negotiation pipeline position and body neutrality"; plus
  three MODIFIED requirements carrying the negotiation prefix —
  "JSON binding mode pipeline", "Raw binding mode pipeline" — and the
  RENAMED "v1 behavioral compatibility for omitted binding" (was
  "v1 byte-identity": compatibility is now behavioral for
  well-formed requests, not byte identity of the lowered sequence).
- `crates/camel-api/src/error.rs` — two typed variants on the
  `#[non_exhaustive]` `CamelError`: `UnsupportedMediaType { consumed,
  declared }` and `NotAcceptable { accept, produced }`, with
  `classify()` and `variant_name()` arms.
- `crates/camel-dsl/src/media.rs` (new) — in-tree RFC 7231/9110 subset
  parser. The private helpers `split_media_base`,
  `is_valid_media_declaration`, `is_json_media_type`
  (`crates/camel-dsl/src/rest.rs:461-476`) move here and are extended:
  type/subtype (+ structured syntax suffix), parameters (`;q=` with
  default 1.0, `q=0` = reject), `*/*` and `type/*` wildcards,
  case-insensitive comparison, other parameters skipped.
- `crates/camel-processor/src/content_negotiation.rs` (new) —
  `ContentNegotiationProcessor`, a `BoxProcessor`-shaped Tower shell
  that reads `Content-Type` / `Accept` from exchange headers and
  delegates the verdict to a check closure injected at construction
  (cycle-free: camel-processor cannot depend on camel-dsl; the closure
  is built by `compile.rs` over `media.rs` with declarations parsed
  once at compile time). It never polls, wraps, or replaces the body.
- `crates/camel-dsl/src/rest.rs` — `lower_operation` injects the
  negotiation step as the FIRST lowered step (before `Unmarshal`).
- Step-chain plumbing in leased crates only: `route_ast.rs`
  (lowering-only `RouteDslStep` variant, `#[serde(skip_deserializing)]`
  like `SetHeaderIfAbsent`), the DSL→declarative conversion
  centralized in `yaml.rs::route_dsl_to_declarative_route` (reused by
  the JSON authoring path), `model.rs` `DeclarativeStep` variant,
  `compile.rs` emitting
  `BuilderStep::Processor(OpaqueProcessor(..))`.
- `crates/components/camel-http/src/lib.rs` — the ONLY camel-http
  change: `pipeline_error_to_reply` (line 3334) maps the typed errors
  to 415 / 406. Registry and `rest_match` stay media-blind.
- `crates/components/camel-http/CONTEXT.md` — the finalizer mapping
  paragraph (currently "all other errors map to 500") gains the
  415/406 typed mappings.
- `crates/camel-processor/CONTEXT.md` — catalog/public-surface entry
  for the exported `ContentNegotiationProcessor`.
- Tests: media parser table tests, processor unit tests, lowering
  order tests, compile emission tests, camel-http e2e 415/406 battery.

**Out:**

- No server-driven representation selection (single declared
  `produces`; the gate is accept/reject, not content selection).
- No opt-in flags, no `strict` field, no permissive matrix.
- No new external dependency (parser is in-tree; escape hatch to the
  `mediatype` crate only if the matcher exceeds ~350 LoC with bugs).
- No OpenAPI schema changes (declared media already flow through).

## Capabilities

### Specification: rest-dsl

Delta: three ADDED requirements pinning the 415 path (unsupported
media, parameterized match, `+json` suffix per L1 essence rule,
body-less verbs no-op, malformed Content-Type → 415), the 406 path
(no acceptable representation, `q=0` explicit reject, wildcards,
multiple Accept entries, case-insensitivity, charset ignored,
malformed Accept → treat as `*/*`), and the pipeline pins
(negotiation before unmarshal, never touches body, L1/L3 pins stay
green).
