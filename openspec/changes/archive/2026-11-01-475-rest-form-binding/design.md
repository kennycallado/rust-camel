# Design: 475-rest-form-binding

## Context

REST DSL lowering (`crates/camel-dsl/src/rest.rs`) currently branches on
`RouteDslRestBinding::{Json, Raw}` (`route_ast.rs:1598`). Data formats are
built-ins under `crates/camel-processor/src/data_format/` registered in
`builtin_data_format_with_config` (`mod.rs:29`). Relevant ADRs:
ADR-0073 (strict content negotiation — the injected gate stays untouched),
ADR-0030 (exchange-aware DataFormat hooks), ADR-0032 (data trust boundary —
fail-closed parsing), ADR-0091 (style). Data plane only; no control-plane
or component changes (ADR-0001 boundary respected).

## Decisions

**D1 — Key order (recorded decision).** serde_json `preserve_order` is
enabled by `crates/languages/camel-language-rhai/Cargo.toml:21`, and cargo
feature unification makes object-key iteration order build-dependent
(IndexMap where rhai is linked, BTreeMap otherwise). This change SHALL NOT
add or remove that feature anywhere (adding it where absent changes JSON
byte output; removing it changes rhai behavior). Consequence: object key
iteration order of the form-unmarshalled JSON is UNSPECIFIED and
build-dependent. "Preserving first-seen key order" applies to the ORDER OF
VALUES inside duplicate-key arrays (encounter order). All tests and
scenarios compare by JSON VALUE equality or by pair-sets; no test asserts
key order or an exact multi-key marshal string. The invariant is: no lossy
overwrite.

**D2 — Media check.** New `is_form_media_type` in `crates/camel-dsl/src/media.rs`
beside `is_json_media_type`: base (pre-`;`) equals
`application/x-www-form-urlencoded` case-insensitively; every `;`-parameter
must be exactly `charset=utf-8` (case-insensitive name/value, at most one).
Anything else fails validation with a `RouteError` naming the operation,
field, value, and remedy — message shape parallel to the json-mode errors.

**D3 — Data format.** `FormDataFormat` in
`crates/camel-processor/src/data_format/form.rs`, modeled on `csv.rs`:
`FormConfig { max_fields: Option<usize> = 1_000, max_bytes: Option<usize> =
1_048_576 }`, `#[serde(default, deny_unknown_fields)]`. Unmarshal accepts
ONLY `Body::Text`/`Body::Bytes`; `Body::Empty` is a no-op returning
`Body::Empty` (mirrors `json.rs` — a body-less POST or a 204 response must
not fail); `Body::Stream` fails closed (mirrors `json.rs`; the REST
pipeline materializes streams via StreamCacheService before the format).
Byte cap enforced on input length BEFORE parsing; field cap counts total
non-empty pairs (duplicates included) DURING parsing. Parsing: split on
`&`; EMPTY segments (from `&&`, leading/trailing `&`) are skipped and do
NOT count toward the cap; each non-empty pair splits on the FIRST `=`
(missing `=` → empty value; empty key allowed); `+` → space; `%XX` strict
hex else error; decoded bytes validated as strict UTF-8. Duplicate keys
upgrade the value to an array and append in encounter order (D1). Empty
input → `{}`. Marshal: `Body::Empty` no-op; only a top-level `Body::Json`
object marshals — string values pass through; number/bool/null stringify
to their JSON lexical form (`1`, `true`, `null`); arrays repeat the key
per scalar element; nested objects/arrays → typed error; empty object →
`""`. Encoding follows the WHATWG `application/x-www-form-urlencoded`
serializer: ASCII alphanumerics and `*-._` pass unescaped, space → `+`,
everything else uppercase `%XX` (UTF-8 bytes). Errors are
`CamelError::TypeConversionFailed` (the variant json/csv already use) with
a message prefixed `form`, naming the cap kind or the malformed escape and
its byte offset — fail-closed style per ADR-0032, caps per the CSV R3-M1
pattern.

**D4 — Lowering.** `RouteDslRestBinding::Form` arm in validation (D2 checks;
`request_schema`/`response.schema` ALLOWED — the pipeline validates the
JSON produced by form unmarshal, so schema ordering is: form-parse → JSON
object → JsonSchemaValidateService wrap, identical wiring to json binding).
Pipeline injection: same as json with `"form"` in place of `"json"`:
`Unmarshal { unmarshal: "form", schema }` for body-bearing verbs,
`Marshal { marshal: "form" }`; negotiation step, SetHeader Content-Type,
SetHeaderIfAbsent default status, and user-step ordering untouched. OpenAPI
generation follows the non-raw path (authored schemas emitted — the
`raw` boolean at `openapi.rs:167` is false for Form). The spec delta
MODIFIES the canon "OpenAPI generation for REST bindings" requirement so
form binding is contractually the json path: authored schemas under the
declared form media, weak-stub fallback and warnings identical to json.

**D5 — Schema/artifacts.** The enum derives `schemars::JsonSchema` + `ts_rs`;
adding `Form` flows into generated artifacts; regenerate via
`cargo xtask schema` (check gate must pass). Deserialization stays
deny-unknown + lowercase rename, so unknown binding strings still fail at
deserialize with an unknown-variant error.

**D6 — Gate param-blindness (documented behavior).** The L2 media
negotiation gate treats `;`-parameters as opaque (canon rest-dsl §"REST
request media enforcement"). A request declaring
`Content-Type: application/x-www-form-urlencoded; charset=iso-8859-1`
therefore PASSES the gate and is rejected later, inside unmarshal, on
non-UTF-8 percent-decoded bytes. Declared DSL `consumes`/`produces` with a
non-UTF-8 charset still fails at route load (D2). Both behaviors are
pinned by scenarios.

## Cap summary (parser limits)

`max_fields = 1_000` counts total non-empty pairs (duplicates included;
empty segments skipped and uncounted), `max_bytes = 1 MiB` on input length,
both configurable via the format `config:` block, both settable to `None`
(documented as not recommended for untrusted input — same caveat as CSV).

## Affected surfaces

- `crates/camel-dsl/src/route_ast.rs` (enum variant)
- `crates/camel-dsl/src/media.rs` (form media check)
- `crates/camel-dsl/src/rest.rs` (validation arm + lowering strings + tests)
- `crates/camel-dsl/src/openapi.rs` (no behavior change; parity test)
- `crates/camel-processor/src/data_format/{mod.rs,form.rs}` (format + tests)
- `crates/camel-dsl/tests/rest_form_e2e.rs` (executed pipeline, mirrors
  `rest_raw_e2e.rs`)
- `docs/src/yaml-dsl/top-level-blocks.md`, `docs/src/data-formats/{index,form}.md`
- Spec deltas: `openspec/changes/475-rest-form-binding/specs/{rest-dsl,data-formats}/spec.md`

## Testing strategy

TDD per task: unit matrices in `form.rs` (parser: empty, one pair, repeated
keys, `+`/`%20`/`%2B`, UTF-8, malformed percent, invalid UTF-8, empty
key/value, missing `=`, empty segments, over-cap fields/bytes, exactly-at
cap passes; marshal: object, array values, nested rejection, empty object,
scalar stringification, WHATWG encoding, Empty/Stream), DSL validation
matrix + lowering-sequence tests in `rest.rs`, executed e2e in
`rest_form_e2e.rs` (form request → JSON body observed by a step; JSON →
form response), raw/json regression suites untouched and green. Assertions
compare by JSON value equality or pair-sets — never by key order or exact
multi-key marshal strings (D1).

## Phases

Single delivery phase — one cohesive slice, ~6 tasks, no cross-phase
ordering hazards.
