# ADR: Default-strict REST content negotiation (415/406)

- **Status:** Proposed — spec blessing gate pending; the ADR file path
  is allocated under `docs/adr/` during archive.
- **Ruling source:** e_opus sealed ruling, amended by the user fact
  ZERO ADOPTION — no retrocompat constraints. Do not re-derive compat.
- **bd:** rc-hlb1q (REST DSL v2 L2).

## Context

The REST DSL already declares per-operation media contracts:
`consumes` / `produces` on `RouteDslRestOperation`, validated at
lowering time (`crates/camel-dsl/src/rest.rs`, `lower_operation`:
binding `json` must declare JSON-family media via
`is_json_media_type`, binding `raw` may declare any valid
`type/subtype` via `is_valid_media_declaration`; both helpers are
private in `rest.rs:461-476`). Nothing enforces these declarations at
runtime:

- The HTTP consumer routes by method+path only:
  `crates/components/camel-http/src/registry.rs:51`
  (`rest_endpoints: Vec<RestEndpoint<T>>`,
  `rest_match.rs:30` `pub struct RestEndpoint<T>` carries method and
  path only). Media is invisible to routing.
- Request `Content-Type` lands in the request `StreamMetadata`
  (`camel-http/src/lib.rs`, axum handler ~1560) and in the exchange
  headers the consumer receive loop installs; `Accept` travels as a
  plain exchange header (see the forwarding pin test at
  `camel-http/src/lib.rs` ~4120).
- Errors from the route pipeline flow back through the finalizer
  `pipeline_error_to_reply` (`camel-http/src/lib.rs:3334`), which maps
  typed `CamelError` variants to statuses (401/403/400/400/503, else
  500).

So a `text/plain` POST against an `application/json` operation today
fails late (unmarshal 400) after the body stream may already have been
partially consumed, and `Accept` is ignored entirely.

## Decision

### D1 — Enforcement point: header-gate processor, injected by lowering

A `ContentNegotiationProcessor` in `crates/camel-processor/src/
content_negotiation.rs` implements the standard Tower service shape
(`BoxProcessor = tower::util::BoxCloneSyncService<Exchange, Exchange,
CamelError>`, `crates/camel-api/src/processor.rs:51`). REST lowering
(`crates/camel-dsl/src/rest.rs`, `lower_operation`) injects it as the
FIRST lowered step — before the `UnmarshalStep` that request binding
pushes today (rest.rs ~338) and before every other step.

**Cycle-free construction (dependency fact).** `camel-dsl` depends on
`camel-processor` (Cargo.toml; `compile.rs` already references
`camel_processor::` types), so the processor MUST NOT call back into
`camel-dsl` — that would be a crate cycle. The split:

- The processor in camel-processor is a header-reading shell. It
  extracts `Content-Type` and `Accept` from exchange headers
  (case-insensitive lookup) and delegates the decision to a check
  closure injected at construction:
  `Arc<dyn Fn(Option<&str>, Option<&str>) -> Result<(), CamelError>
  + Send + Sync>` (content-type, accept). It NEVER polls, wraps,
  replaces, or caches the body (L3 pin: no `StreamCacheService`, no
  consumption). On `Ok` it passes the exchange through untouched; on
  `Err` it fails the exchange with the typed error.
- `camel-dsl/src/compile.rs` constructs that closure over the media.rs
  matcher and a `MediaContract` — the declared `consumes`/`produces`
  parsed ONCE at compile time, not per request. All media semantics
  (parsing, matching, essence rule, q-factors) stay in
  `crates/camel-dsl/src/media.rs` (D3), one home shared with the
  lowering-time declaration validation.

Wiring uses the existing lowering-only step pattern
(`SetHeaderIfAbsent` precedent: `#[serde(skip_deserializing)]` on the
`RouteDslStep` arm, `route_ast.rs:515`): the lowering pushes the step,
and the DSL→declarative conversion — centralized in
`yaml.rs::route_dsl_to_declarative_route`, which the JSON authoring
path reuses (`json.rs` imports it; JSON is a full-DSL authoring format
per ADR-0026) — maps it to a `DeclarativeStep` (`model.rs`). `compile.rs`
then constructs the processor and emits
`BuilderStep::Processor(camel_api::OpaqueProcessor(..))` (construction
precedents in `compile.rs` ~1005-1292). camel-core needs NO change —
`BuilderStep::Processor` already accepts pre-built Tower services
(`crates/camel-core/src/lifecycle/application/route_definition.rs:62`).

The camel-http registry and `rest_match` stay MEDIA-BLIND. The only
camel-http change is the finalizer mapping (D2).

### D2 — Typed errors, mapped by the HTTP finalizer

`crates/camel-api/src/error.rs` (`CamelError` is `#[non_exhaustive]`,
line 78) gains:

- `UnsupportedMediaType { consumed: String, declared: String }` →
  HTTP 415
- `NotAcceptable { accept: String, produced: String }` → HTTP 406

`classify()` (line 177) gains categories `unsupported_media_type` and
`not_acceptable`; `variant_name()` (line 214) gains the arms (the
defining-crate exhaustive match makes omission a compile error).
`pipeline_error_to_reply` (`camel-http/src/lib.rs:3334`) maps the two
variants to 415/406 with JSON error bodies, mirroring the existing
`TypeConversionFailed` → 400 pattern. This is the ONLY camel-http
change.

### D3 — In-tree parser, no new dependency

`crates/camel-dsl/src/media.rs` (new module) hosts the RFC 7231/9110
subset. The private helpers `split_media_base`,
`is_valid_media_declaration`, `is_json_media_type` MOVE here from
`rest.rs` (keeping their current call sites via `pub(crate)`), and the
module adds the negotiation parser, budget ~150-250 LoC:

- Media type: `type/subtype` with optional `+suffix`; `tchar`
  validation for token characters; case-insensitive comparison.
- Parameters: parse and skip all except `q` (Accept side); `q` defaults
  to 1.0 when absent; `q=0` means explicit reject.
- Wildcards `*/*` and `type/*`: valid ONLY as Accept entries. A
  wildcard `Content-Type` is malformed (D5).
- Charset and every other parameter are ignored for matching.

`media.rs` also hosts the match entry the injected closure calls:
`check_request(content_type: Option<&str>, accept: Option<&str>,
contract: &MediaContract) -> Result<(), CamelError>`, where
`MediaContract` is the parsed declaration pair built once at compile
time. The shell in camel-processor holds no media knowledge of its
own — only the closure and the header extraction (D1).

Escape hatch: adopt the `mediatype` crate ONLY if the matcher exceeds
~350 LoC with residual bugs; that path requires a workspace review
(MSRV, cargo audit, q-factor matching must be exposed).

### D4 — Default-strict: no opt-in surface

No flag, no `strict` field, no permissive matrix. Enforcement exists
exactly where declarations exist; undeclared media naturally means
permissive. REST operations always carry `consumes`/`produces` after
lowering defaults (both default to `application/json`), so lowered
REST operations are strict by default. Non-REST routes (direct,
component consumers) never receive the step and are unaffected.

v1 compatibility is BEHAVIORAL, not byte identity of the lowered
sequence: the delta renames the canon requirement to "v1 behavioral
compatibility for omitted binding" and modifies the JSON/raw pipeline
requirements to carry the negotiation prefix. For requests whose
`Content-Type` satisfies the declared `consumes` and whose `Accept`
admits the declared `produces`, observable behavior (statuses,
response bytes) is unchanged; mismatched requests move from late
400/200 to early 415/406 — the L2 contract. The v1 pin suites remain
the internal regression net, with their lowered-sequence assertions
updated to include the prefix (and nothing else).

### D5 — Malformed-header direction (PINNED)

- Malformed `Accept` → treat as `*/*` (permissive). If any entry of
  the comma-separated list fails to parse, the whole header is treated
  as `*/*`.
- Malformed `Content-Type` on an operation with declared `consumes`
  (strict side) → 415. A wildcard in `Content-Type` is malformed.

Asymmetry is deliberate: `Accept` failures must not lock clients out
of representations they can consume; an unverifiable `Content-Type`
against a declared contract must fail closed.

### D6 — Matching semantics

One function, used by both sides, `media_satisfies(candidate,
declared)`:

- Normalize case; compare `type/subtype` equality; ignore parameters
  except `q`.
- Essence rule (L1): if BOTH candidate and declared are JSON-family
  (`application/json` or any `+json` suffix — the existing
  `is_json_media_type` test), they satisfy each other. This makes
  `application/vnd.api+json` satisfy `consumes: application/json` and
  symmetrically satisfy `Accept: application/json` against
  `produces: application/json`.
- Accept-side wildcards: `*/*` satisfies any declared; `type/*`
  satisfies any declared with the same type.
- Media-range precedence (RFC 7231 §5.3.2): among Accept entries that
  satisfy the declaration, the most specific governs — exact
  `type/subtype` over JSON-essence match over `type/*` over `*/*` —
  and the request is acceptable only when the governing entry has
  `q > 0`. Thus `Accept: application/json;q=0, */*;q=1` against
  `produces: application/json` is an explicit reject (406): the exact
  entry outranks the accepting wildcard. When several entries tie at
  the governing specificity (`application/json;q=0,
  application/json;q=1`), the LOWEST quality governs — fail closed,
  deterministic.
- Content-Type side: no wildcards (malformed per D5).

Gate semantics (fail fast, first step):

- 415: request carries a `Content-Type` that does not satisfy declared
  `consumes` — only for verbs with a body (`verb_has_body`,
  `rest.rs:456`); body-less verbs (GET/DELETE) skip the request check.
  Absent `Content-Type` is permissive (v1 unmarshal path decides).
- 406: NO Accept entry with `q > 0` satisfies declared `produces`.
  Absent `Accept` is permissive (RFC default). `q=0` on the only
  matching entry is an explicit reject → 406.
- No server-driven selection: the gate accepts or rejects; the single
  declared `produces` is what the route emits.

## Alternatives considered

- **Enforce in camel-http routing (media-aware registry):** rejected —
  bloats the media-blind `RestEndpoint<T>` matching surface and only
  sees media AFTER route match; the ruling pins registry media-
  blindness.
- **Enforce in the finalizer by inspecting exchange state:** rejected —
  too late (body may be consumed), and couples transport to DSL
  declarations it cannot see.
- **Parser in camel-api (below both crates):** rejected — would split
  media knowledge across crates: declaration validation stays in
  camel-dsl lowering while runtime matching lived in camel-api,
  duplicating the essence rule in two homes. The injected-closure
  split keeps every media decision in `media.rs`.
- **`mediatype` crate from the start:** rejected — the subset fits
  ~150-250 LoC in-tree; a new dependency needs workspace review.
  Escape hatch documented in D3.
- **Opt-in `strict` flag:** rejected — ZERO ADOPTION; a default-off
  flag enforces nothing and a default-on flag IS default-strict with
  extra surface.

## Consequences

- Lowered REST pipelines gain one leading step; v1 pin tests are
  updated to assert the prefix (sequence assertions only) and stay
  green as the internal regression net; well-formed requests keep
  byte-identical responses.
- L1 (`binding: raw`) and L3 (streaming) pin batteries stay green: the
  gate is header-only and never touches `Body::Stream`, metadata, or
  `StreamCacheService` injection.
- `variant_name()`/`classify()` updates are compile-enforced in
  camel-api (`variant_name_tests`).
- doTry catch-by-variant users can catch the two new variant names;
  no existing matcher changes (additive arms only).

## Test strategy

- `media.rs` unit table: tokens, suffixes, params, q, wildcards, case,
  malformed inputs (both pinned directions).
- Processor unit tests (Tower `oneshot`): 415/406 emission, pass-
  through identity, zero body polls (poll-recorder stream, L3 style).
- Lowering tests: step order (negotiation first, before unmarshal),
  both bindings, body-less verbs.
- Compile tests: `BuilderStep::Processor` emission with expected
  declarations.
- camel-http e2e: 415/406 status + JSON error body through the axum
  handler; wildcard/multi-accept/q=0 matrices; malformed directions;
  v1/L1/L3 pin suites stay green.
