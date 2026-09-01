# Proposal: splitter-fail-loud

## Why

The splitter EIP returns zero fragments when the body type does not match the split expression. `camel-processor` then returns the original exchange with success semantics (`splitter.rs:103`). A route that forgets `unmarshal` before `split` becomes a silent no-op. Downstream aggregation never fires. A marker-deadline times out with zero diagnostics.

bd issue: rc-90ez (P2). Design ruling: e_opus, 2026-09-01, committed with this change at `ruling.md` in this change directory. The ruling selects a typed error over a single-fragment fallback. The fallback would trade a silent timeout for silent mis-aggregation.

## What Changes

- Wrong body type in a split expression returns `CamelError::TypeConversionFailed`. The error names the expression kind, the received body variant, the expected type, and an unmarshal hint. It never prints the payload.
- `SplitExpression` changes in place to `Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> + Send + Sync>`. Pre-1.0 break. About 20 call sites pass the constructors through unchanged. Two consumption points propagate the error (`SplitterService::call`, `SplitSegment::run`).
- Genuinely empty content keeps pass-through semantics: `Body::Empty`, `Body::Text("")` under `BodyLines`, `Body::Json([])` under `BodyJsonArray`. This matches Apache Camel ("null or empty body continues the original exchange").
- The declarative split compiler (`camel-core` `splitting.rs:147`) drops its `_ => vec![exchange.clone()]` fallback. Both eager paths then agree.
- `StreamingSplitterService` already fails loud per item. It only gets the improved message and the `TypeConversionFailed` variant.

Excluded: the aggregate DSL `correlation_key` split-brain (bd rc-q8ng). Excluded: any lenient single-fragment mode.

## Acceptance criteria

- `SplitExpression` is fallible: `Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> + Send + Sync>`. The workspace compiles after the break.
- Split on a wrong-type body returns `TypeConversionFailed`. The message uses the pinned templates in `design.md`: expression kind, received type name, expected type, and the phrase "add an unmarshal step before split". The message contains no body or value content.
- Split on `Body::Empty`, `Body::Text("")`, or `Body::Json([])` returns the original exchange unchanged.
- The declarative split compiler drops its single-fragment fallback and returns the same typed error.
- A split error in `SplitSegment` propagates as `PipelineOutcome::Failed`, never `Stopped`.
- The streaming splitter promotes its non-`Body::Stream` error to `TypeConversionFailed` with the pinned message.
- No new log line accompanies the typed error (ADR-0012).
- Regression test red to green: split over `Body::Text` with `split_body_json_array` asserts the typed error, not pass-through.
- Existing test `test_split_empty_fragments` keeps passing.
- `cargo test -p camel-processor --lib` and `cargo test -p camel-api --lib` exit 0.

## Risk budget

Accepted: public signature break in `camel-api` (pre-1.0), mechanical edits in dependent crates. Out of bounds: behavior change for correct-type routes, new log lines (ADR-0012 forbids double reporting), any payload text in error messages.
