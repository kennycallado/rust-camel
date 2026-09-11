# Proposal: add-rest-raw-binding

## Why

The REST DSL (`rest:` blocks) rejects every `consumes`/`produces` value other
than the exact string `application/json` at route load
(`crates/camel-dsl/src/rest.rs:223-237`). The gate also rejects valid JSON
media forms such as `application/json; charset=utf-8` and
`application/problem+json`. Authors of binary download/upload, text, proxy,
and streaming APIs must fall back to hand-written `http:` routes and lose the
REST authoring surface (path templates, status defaults, OpenAPI generation).

The viability study (`.opencode/fleet/inbox/rest-v2-viability.md`, blessed by
e_gpt) rates the raw-binding lift as S (250-450 LoC): the HTTP consumer
already preserves `Body::Stream` and already honors a user-supplied response
`Content-Type`, so the restriction lives entirely in the DSL lowering layer.
This change delivers L1 of the viability ladder. bd: rc-01har.

## What Changes

**In:**

- `crates/camel-dsl/src/route_ast.rs` — new `RouteDslRestBinding` enum
  (`json` | `raw`) and an optional per-operation `binding` field on
  `RouteDslRestOperation` (absent = `json`). Invalid values fail at
  deserialize time.
- `crates/camel-dsl/src/rest.rs` — the JSON-only gate becomes a binding-aware
  gate: `json` mode accepts any JSON-essence media type (subtype `json` or
  `+json` suffix, parameters allowed) and returns a precise route-load error
  otherwise; `raw` mode accepts declared non-JSON media, injects no automatic
  `unmarshal`/`marshal`, and sets the declared `produces` as the response
  `Content-Type` after user steps. `raw` plus `request_schema` or
  `response.schema` fails route load. Binding is never inferred from media
  type.
- `crates/camel-dsl/src/openapi.rs` — raw operations generate binary string
  schemas (`type: string`, `format: binary`) under the declared media keys
  instead of weak `type: object` stubs.
- Tests: parser, lowering, compile-path wiring, OpenAPI, and v1
  byte-identity pins.
- Docs (mandatory — every site currently pins the v1 JSON-only contract):
  `CONTEXT-MAP.md` (Key Terms → REST DSL), `crates/camel-dsl/CONTEXT.md`
  (REST DSL entry), `docs/src/yaml-dsl/step-verbs.md` (REST section),
  `docs/src/concepts/glossary.md` (REST DSL entry).

**Out:** L2 strict negotiation, L4 multipart/forms/XML structured binding,
the L3 streaming contract test suite (stage-2 mission), any production change
in `camel-http`/`camel-processor`/`camel-core`, and the media-keyed content
map AST redesign (v2 posture decision, not needed for L1).

## Acceptance criteria

- A route with `binding: raw` and non-JSON `consumes`/`produces` loads and
  lowers to: user steps, `SetHeader(Content-Type: <produces>)`,
  `SetHeaderIfAbsent(<default status>)` — no unmarshal, no marshal.
- The request body remains the HTTP-provided `Body::Stream` (no
  `StreamCacheService` wrap, because no unmarshal step exists).
- `json` mode (default) accepts `application/json`,
  `application/json; charset=utf-8`, and `application/problem+json`; it
  returns a precise `RouteError` for media that is not JSON-essence.
- `raw` + `request_schema` or `raw` + `response.schema` fail route load with
  precise errors.
- Every existing route that loaded under v1 lowers to the same URI and an
  identical step sequence (all existing test pins stay green, unmodified).
- OpenAPI generation emits binary string schemas for raw media content.
- YAML and JSON authoring paths accept `binding` identically.
- All four doc sites (`CONTEXT-MAP.md`, `crates/camel-dsl/CONTEXT.md`,
  `docs/src/yaml-dsl/step-verbs.md`, `docs/src/concepts/glossary.md`)
  describe the binding field and no longer state the JSON-only restriction
  as the whole contract.

## Risk budget

The hard gate is v1 byte-identity: omitted `binding` must not change any
lowered step. Acceptable: adding one field to the public non-`non_exhaustive`
`RouteDslRestOperation` (source break for struct literals, contained in this
workspace). Not acceptable: behavior changes in `camel-http` production code,
media-type inference of binding mode, or silent schema ignoring in raw mode.
