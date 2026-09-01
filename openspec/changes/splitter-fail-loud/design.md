# Design: splitter-fail-loud

## Approach

Change the `SplitExpression` alias in `camel-api/src/splitter.rs` in place:

```rust
pub type SplitExpression =
    Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> + Send + Sync>;
```

### Built-in expression arms

`split_body_lines` (`camel-api/src/splitter.rs:319`):

- `Body::Text(s)` → fragments per line. `Body::Text("")` yields `Ok(vec![])`.
- `Body::Empty` → `Ok(vec![])` (explicit arm, before the catch-all).
- Any other variant → `Err(CamelError::TypeConversionFailed(msg_lines))`.

`split_body_json_array` (`camel-api/src/splitter.rs:334`):

- `Body::Json(Array(arr))` → one fragment per element. `Json([])` yields `Ok(vec![])`.
- `Body::Empty` → `Ok(vec![])` (explicit arm).
- `Body::Json(_)` (non-array JSON) → `Err(msg_jsonarray_nonarray)`.
- Any other variant → `Err(msg_jsonarray)`.

### Custom `split_body` stays infallible

`split_body` keeps its closure bound `Fn(&Body) -> Vec<Body>` and wraps the result in `Ok` internally. Custom expressions own their body-type policy. An empty `Vec` from a custom closure keeps pass-through semantics. This is the documented contract, not an oversight. Custom call sites stay unchanged: `camel-processor/benches/splitter.rs:25`, `camel-bench/benches/pipeline.rs:38`, `camel-api` unit tests.

### Error message templates (exact)

Messages use the public `body_type_name(&Body) -> &'static str` helper. The helper moves verbatim from `camel-core/src/shared/observability/adapters/tracer.rs:29` to `camel-api`. Its outputs stay `empty`, `bytes`, `text`, `json`, `xml`, `stream`, `unknown`. The tracer reuses the public helper. Tracing field values do not change, so no migration.

```text
msg_lines                 = "split expression 'body_lines' requires body type text, got {received}; add an unmarshal step before split"
msg_jsonarray             = "split expression 'body_json_array' requires body type json (array), got {received}; add an unmarshal step before split"
msg_jsonarray_nonarray    = "split expression 'body_json_array' requires body type json (array), got json (non-array); add an unmarshal step before split"
msg_streaming             = "streaming split requires body type stream, got {received}; add an unmarshal step before split"
msg_declarative           = "declarative split requires a text or array value, got {received}; add an unmarshal step before split"
```

`{received}` comes from `body_type_name` for body-based paths. The declarative path evaluates a `Value`, not a `Body`. It uses a small `value_type_name(&Value)` helper with outputs `string`, `array`, `number`, `boolean`, `object`, `null`, `other`. No template ever includes body or value content.

### Consumption points

- `SplitterService::call` (`camel-processor/src/splitter.rs:100`): `expression(&exchange)?` replaces the `Vec` binding. The empty-vec branch keeps `Ok(original)`.
- `SplitSegment::run` (`camel-processor/src/split_segment.rs`): a split error becomes `PipelineOutcome::Failed`. `Stopped` handling stays as is (ADR-0025: Stop is successful control flow).
- Declarative split compiler (`camel-core/src/lifecycle/adapters/step_compilers/splitting.rs:147`): the `_ => vec![exchange.clone()]` arm becomes `Err(msg_declarative)`. The compiler closure returns `Result`. Both eager paths then agree.
- `StreamingSplitterService`: already per-item fallible on non-`Body::Stream`. Message becomes `msg_streaming` and the error promotes to `TypeConversionFailed`. No signature change.

Direct `Arc::new` closures that must wrap output in `Ok`: tests at `camel-processor/src/split_segment.rs:524,630,716,921,988` and `camel-processor/src/splitter.rs:839`. `StreamingSplitExpression` closures stay untouched (separate type, already fallible).

Precedent: `UnmarshalService` returns `TypeConversionFailed` on mismatched input (`marshal.rs:150`). ADR-0058 fixed the same silent-success class in `RecipientListService` ("never `Ok(original)`"). ADR-0012 bars a warn log next to the typed error.

### Preserved regressions (do not weaken)

- `test_split_empty_fragments` (`camel-processor/src/splitter.rs:491`): empty pass-through.
- `test_split_body_lines_empty` (`camel-api/src/splitter.rs:382`): empty text yields zero fragments.
- `test_split_body_json_array_not_array` (`camel-api/src/splitter.rs:401`): inverts to assert the typed error. Do not delete it.

### CONTEXT.md updates

- `crates/camel-processor/CONTEXT.md` ("Aggregation contract (divergence from Apache Camel)", ~line 200): document that a wrong-type split body now fails before fragmentation, and that empty-content pass-through still skips aggregation.
- `crates/camel-api/CONTEXT.md`: add a `SplitExpression` entry covering fallibility and the built-in type-mismatch semantics.

### Single-phase decision

This is one compile-atomic slice. The alias break forces the helper, both built-ins, the compiler closure, both consumption points, all direct-closure tests, and the benches to compile together. Any phase split would leave the workspace broken between phases.

## Affected crates

- `camel-api`: `SplitExpression` signature, wrong-type and `Body::Empty` arms, `body_type_name` public helper, unit tests. `test_split_body_json_array_not_array` (`splitter.rs:401`) inverts to assert the typed error. Do not delete it.
- `camel-processor`: `SplitterService::call` propagation, `SplitSegment::run` `Failed` propagation, streaming message upgrade, `Ok` wrap in the five `split_segment.rs` test closures and the `splitter.rs:839` test closure. `benches/splitter.rs` uses `split_body` and stays unchanged.
- `camel-core`: mechanical edits in `commands.rs`, `route_definition.rs`, `step_resolution.rs`, `startup_validation.rs` (constructors pass through), the `splitting.rs` fix, tracer reuse of the public helper.
- `camel-dsl`: `compile.rs` constructors pass through unchanged. Test closures with custom split functions may need `Ok` wrapping.
- `camel-builder`: test closures wrap output in `Ok`.
- `camel-bench`: `benches/pipeline.rs` uses the infallible `split_body` and stays unchanged.
- `camel-tests`: only if workspace compilation requires closure updates.

## Architecture boundaries

Runtime layer change. The DSL surface (`CanonicalSplitExpressionSpec`) stays unchanged. No new data-plane or control-plane crossing. The error flows through the existing `CamelError` channel that processors already own.

## Alternatives considered

- Single-fragment fallback (Java Camel `ObjectHelper.createIterable` precedent). Rejected: silent mis-aggregation replaces a silent timeout. The motivating bug asks for loud failure.
- New `TrySplitExpression` type with deprecated old alias. Rejected: doubles public surface forever. The break is mechanical and pre-1.0.
- Warn log next to pass-through. Rejected: keeps success semantics and double-reports once the error lands (ADR-0012).
- Fallible closure bound for `split_body`. Rejected: every custom closure would churn with no gain. Custom expressions own their type policy.
