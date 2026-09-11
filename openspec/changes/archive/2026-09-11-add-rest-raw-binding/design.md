# Design: add-rest-raw-binding

## Approach

L1 is a pure `camel-dsl` lowering-layer change. The HTTP consumer already
installs every request as a lazy `Body::Stream` and its reply finaliser
already prioritises a user-supplied `Content-Type` header over body-type
inference (`crates/components/camel-http/src/lib.rs:2015-2059`), so raw
binding only changes which steps REST lowering injects.

**AST (`route_ast.rs`).** Add:

```rust
#[serde(rename_all = "lowercase")]
pub enum RouteDslRestBinding { Json, Raw }
```

plus `#[serde(default)] pub binding: Option<RouteDslRestBinding>` on
`RouteDslRestOperation` (absent = `json`). `deny_unknown_fields` stays; an
unknown variant (e.g. `binding: yaml`) fails at deserialize time with serde's
"unknown variant" error — the study's error contract. The enum carries the
same `#[cfg_attr(feature = "schema", ...)]` derives as its siblings; generated
schema/TS artifacts do not cover REST blocks today (`route-schema.json` root
covers only `routes`; no `RouteDslRest*` TS export exists), so `cargo xtask
schema --check` must merely stay green.

**Lowering (`rest.rs`, `lower_operation`).** Replace the two exact-string
gates with a binding-aware gate:

- `is_json_media_type(media)`: split at the first `;`, require `type/subtype`
  shape, subtype (case-insensitive, trimmed) equals `json` or ends with
  `+json`. This accepts `application/json`, `text/json`,
  `application/problem+json`, and parameterised forms.
- `json` mode: non-JSON-essence `consumes`/`produces` return a precise
  `CamelError::RouteError` naming the operation, the field, the offending
  value, and the `binding: raw` remedy. The response `Content-Type` step
  value becomes the trimmed `op.produces` (see the trimming rule below).
- `raw` mode: reject `request_schema` and `response.schema` (a response with
  only `description`/`headers` is allowed); validate each declared medium
  against the shared media-declaration syntax below (malformed media
  strings are invalid declarations, not raw content); inject NO unmarshal
  and NO marshal; after user steps inject `SetHeader(Content-Type:
  <produces>)` and keep the existing default-status `SetHeaderIfAbsent`
  step.

**Trimming rule (both modes).** Each declared media value is trimmed first;
the trimmed value is what lowering validates, injects as the response
`Content-Type`, and keys OpenAPI content by. Outer whitespace therefore
never leaks into a response header or an OpenAPI content key. Note v1
byte-identity is unaffected: v1's exact-string gate rejected any
whitespace-bearing value outright, so no previously-loadable route had
outer whitespace to preserve.

**OpenAPI (`openapi.rs`, `build_operation`).** When `binding == raw`: request
body and response content use `{"type": "string", "format": "binary"}` under
the declared media keys; the weak-stub warnings are suppressed for raw
(binary is the correct representation, not a stub); if a raw op carries
`request_schema`/`response.schema` (load-rejected, but generation reads
unvalidated AST via `extract_rest_blocks`), emit a warning mirroring the
existing 204-ignores-schema pattern and still emit binary.

**Decisions (costly-to-reverse, cited from the study —
`.opencode/fleet/inbox/rest-v2-viability.md`):**

- D1: json mode accepts JSON-essence media parameters/suffixes. Cited from
  §"Current REST v1 machinery" ("Parse and lower"): the exact-string gate
  "also rejects valid JSON media parameters and suffix types such as
  `application/json; charset=utf-8` and `application/problem+json`" — a
  defect of the gate, not a contract. v1-loadable routes had exactly
  `application/json`, so byte-identity holds.
- D2: binding is explicit only — never inferred from media type. Cited from
  §"Scope ladder → L1: explicit raw binding" ("Do not infer binding from
  media type. Explicit mode prevents future `application/xml` support from
  silently changing a route that previously meant raw XML passthrough") and
  §"OpenAPI binding interplay", decision 2 ("Binding versus media type.
  Keep binding explicit.").
- D3: raw `Content-Type` uses plain `SetHeader` (override), mirroring json
  mode: the declared `produces` is the authoritative reply contract, and the
  OpenAPI document advertises the same value. (Design rationale; the study
  is silent on if-absent vs override for raw.)
- D4: `request_schema` stays JSON-Schema-only. Cited from §"OpenAPI binding
  interplay", decision 3 ("Schema dialect. Keep `request_schema` explicitly
  JSON Schema. Do not apply it to raw XML, form fields, or multipart
  bytes.") and §"L1" Errors ("Raw mode with `request_schema` or structured
  response schema must fail route load rather than ignore schema").
- D5: the default-status if-absent step is binding-independent — raw POST
  still defaults to 201 unless the author overrides. (Design rationale;
  the status default is orthogonal to binding in the study's L1 cut.)

**Media declaration syntax (shared by both modes).** A valid media
declaration is `type "/" subtype`, optionally followed by `;`-separated
parameters: `base = type "/" subtype *( ";" parameter )`. `type` and
`subtype` must be non-empty and consist only of RFC 9110 token characters
(alphanumerics and `` !#$%&'*+-.^_`|~ ``); the outer value is trimmed, but
no whitespace is allowed inside `type`/`subtype`. Parameters are opaque —
validity and essence checks ignore them. This is the RFC 9110 subset the
study's L2 section anticipates ("implement only the required RFC subset"),
installed early so raw declarations cannot emit malformed OpenAPI content
keys.

**OpenAPI 204 interplay.** A 204 success response stays contentless (no
`content` key) regardless of binding, per the existing branch. The
existing "204 ignores response.schema" warning fires first, so a raw op
with `response.schema` and 204 gets that single warning — no second
raw-schema warning is stacked.

## Affected crates

- `camel-dsl`: `route_ast.rs` (enum + field + parser tests), `rest.rs`
  (gate rewrite, raw lowering branch, unit tests), `openapi.rs` (raw binary
  schemas + tests), integration tests (`rest_schema_e2e.rs`-style wiring,
  `openapi_integration.rs`), struct-literal fixes in all three modules.
- Docs only (all mandatory, all currently state the v1 JSON-only contract):
  `CONTEXT-MAP.md` (Key Terms → REST DSL entry),
  `crates/camel-dsl/CONTEXT.md` (REST DSL entry),
  `docs/src/yaml-dsl/step-verbs.md` (REST operation field table + JSON-only
  paragraph), and `docs/src/concepts/glossary.md` (REST DSL entry).

## Architecture boundaries

Runtime/Components untouched: no change in `camel-http`, `camel-core`, or
`camel-processor` — raw mode works because lowering simply stops injecting
the JSON pipeline (no `Unmarshal` step ⇒ no `StreamCacheService` wrap in
`compile.rs:1273-1285`, request stays `Body::Stream`). The DSL layer owns
authoring semantics; the HTTP component owns wire behavior. Data/control
plane split unaffected.

## Alternatives considered

- **Infer raw from non-JSON media (no new field):** rejected — silent
  behavior change when structured XML binding lands later (study §L1).
- **`SetHeaderIfAbsent` for raw content type:** rejected — diverges from json
  mode's override semantics and lets user steps accidentally break the
  advertised OpenAPI contract.
- **Block-level `binding`:** rejected — media and schemas are
  per-operation; a block default would force re-declaration in mixed
  resources.
- **Media-keyed content map AST now (study decision 1):** deferred — L1
  keeps scalar fields as the authoring surface; the map is an L2 design
  input, and `RouteDslRestOperation`'s API posture (no `non_exhaustive`) is
  resolved before that redesign, not by this change.
