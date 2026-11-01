# Design: 483-rest-xml-followups

## Approach

### D1 — Phase-tagged response marshal (bd rc-fus2z)

`pipeline_error_to_reply` (`crates/components/camel-http/src/lib.rs`) receives
only the error, never the exchange: a pipeline error loses the exchange in the
consumer task. The phase must therefore ride on the error itself, not on a
header.

REST lowering (`crates/camel-dsl/src/rest.rs`) is the only producer of a
response encode step. It currently pushes `RouteDslStep::Marshal`. Add an
internal twin, `RouteDslStep::ResponseMarshal(MarshalStep)`, marked
`#[serde(skip_deserializing)]` with `schemars(skip)` / `ts(skip)`, following the
`ContentNegotiation` precedent (ADR-0017: lower to the same declarative AST,
never a second runtime). `yaml.rs` maps it to a matching internal
`DeclarativeStep::ResponseMarshal(DataFormatDef)`.

The shared format-resolution block in `compile.rs` (protobuf/builtin/config)
moves into one helper used by both `Marshal` and `ResponseMarshal`; the
`schema: Some` rejection covers both. `Marshal` compiles to
`MarshalService::new`; `ResponseMarshal` compiles to
`MarshalService::for_response`. Both are `BuilderStep::Processor`, so route step
counts, trace labels, and metric labels stay identical
(`declarative_step_name` still reports `"marshal"`).

`MarshalService::for_response` wraps any marshal error `e` as
`CamelError::ProcessorErrorWithSource("response marshal failed (<format>)",
Arc::new(e))`. That variant already aliases to `ProcessorError`, already has its
own `on_exceptions` kind, and already falls into the finalizer's 500 arm with
the fixed body `Internal Server Error`; the source chain carries the detail to
server logs. No `CamelError` variant is added, so the rc-5u8co vocabulary guard
and `variant_name` tables stay untouched. The identity change is intentional
and recorded: `do_try` catches written for `TypeConversionFailed` stop matching
the response step, `ProcessorError` catches start matching it, and error
trace/metric labels report the processor family. Request-side `UnmarshalService`
continues to return `TypeConversionFailed` → 400. Declared `marshal:` steps stay
on `MarshalService::new` and keep `TypeConversionFailed` (including an authored
`marshal:` step inside a REST operation's user steps, an accepted residual).

### D2 — XML marshal body acceptance (bd rc-26wzq)

`XmlDataFormat::marshal` (`crates/camel-processor/src/data_format/xml.rs`):

- `Body::Xml(s)` → pass through unchanged. The body is already-serialized XML;
  no re-parse and no well-formedness rejection.
- `Body::Bytes(b)` → `std::str::from_utf8(&b)`. On success, pass the bytes
  through unchanged (no `Body::Text` conversion, so no re-encoding). On failure,
  return `TypeConversionFailed` naming `Body::Bytes`, UTF-8, and
  `valid_up_to()` as the byte offset.
- `Body::Json`, `Body::Text`, `Body::Empty` behavior is unchanged.
  `Body::Stream` stays fail-closed.

UTF-8-only is consistent with the REST media gate, which already rejects
`produces: application/xml; charset=iso-8859-1` (`rest-dsl` spec, XML media
acceptance). A valid UTF-8 `Body::Bytes` keeps the `Body::Bytes` shape; the
HTTP reply finalizer infers no Content-Type for it, but REST lowering always
sets the declared `produces` header explicitly.

## Affected crates

- `camel-dsl`: internal `ResponseMarshal` AST step in `route_ast.rs` /
  `model.rs` (mapped to the existing `DeclarativeStepKind::Marshal`, so
  `contract.rs` is untouched), lowering in `rest.rs`, mapping in `yaml.rs`,
  compile arm and format-resolution helper in `compile.rs`, `parity_tests.rs`,
  and updated step-sequence pins.
- `camel-processor`: `MarshalService::for_response` plus tests; `XmlDataFormat`
  marshal arms plus tests.
- `camel-component-http`: no production change required (generic 500 arm
  already covers `ProcessorErrorWithSource`); unit pin for that mapping and the
  HTTP-boundary e2e in `camel-dsl/tests`.
- `openspec/changes/483-rest-xml-followups/specs/`: deltas for `camel-http`,
  `rest-dsl`, `data-formats`.
- `docs/src/data-formats/xml.md`: marshal body table and REST response note.
- `docs/src/yaml-dsl/top-level-blocks.md`: binding paragraph names the
  internal response-marshal step and its 500 failure class.

## Architecture boundaries

- **DSL**: the phase distinction is an internal lowering artifact. The
  user-visible declarative schema (`marshal:`) is unchanged and reversible YAML
  stays identical (ADR-0017).
- **Runtime (`camel-processor`)**: `MarshalService` owns the error wrapping;
  no core pipeline branching is added.
- **Components (`camel-http`)**: the component keeps a single finalizer and
  gains no phase awareness; it observes only the typed error (ADR-0019 error
  disposition unchanged).
- **Data plane**: no data-plane/control-plane changes; no context mutation.

## Alternatives considered

- **Add `CamelError::ResponseMarshalFailed`.** Rejected: it forces edits to
  `variant_name`, `variant_name_covers_all_variants`, `ALL_CAMEL_ERROR_VARIANTS`,
  the classification guard, and the HTTP finalizer, for no observable gain over
  `ProcessorErrorWithSource` (which already maps to 500 and carries the source).
- **Flag field on `MarshalStep`.** Rejected: `MarshalStep` uses
  `deny_unknown_fields` and the flag would have to thread through
  `DataFormatDef`, which is shared with `Unmarshal` and built by ~20 test
  literals.
- **Exchange header to mark the response phase.** Rejected: the exchange is
  dropped when the pipeline returns an error, so the finalizer cannot see it.
- **Bytes → `Body::Text` on marshal.** Rejected: converting the body shape is a
  silent re-shape; passthrough after UTF-8 validation is faithful and keeps the
  `Body::Bytes` reply path.
- **XML well-formedness validation in marshal.** Rejected: that is content
  sniffing/re-parsing, explicitly out of scope.
