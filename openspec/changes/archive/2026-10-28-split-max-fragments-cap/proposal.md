# Proposal: split-max-fragments-cap

## Why

Mission 418 (rc-ut13q) deleted the never-runtime-wired `SplitterService`. `SplitterConfig`
(camel-api, `splitter.rs:283-390`) still documents and exposes `max_fragments`
(default 100_000, `validate()` rejects 0), but the compiled `SplitSegment`
(`crates/camel-processor/src/split_segment.rs:85`) never carried or enforced it. The
eager path materializes the whole `Vec<Exchange>` via `SplitSource::split` and proceeds
to metadata stamping and processing regardless of size — fail-open. ADR-0090 rule: a
removal must not take away a capability. Restore the cap on the canonical path.

Bd: rc-9hbhj (P2, discovered-from rc-ut13q).

## What Changes

- `camel-processor`: `SplitSegment` gains `max_fragments: usize` (default 100_000). After
  eager materialization and the empty-fragments pass-through, a fragment count above the
  cap fails with
  `CamelError::ProcessorError("Splitter produced {} fragments, exceeding max_fragments {}")`
  — before metadata stamping, before any fragment processing, sequential and parallel.
  No silent truncation. Streaming split stays lazy and uncapped (non-goal).
- `camel-api`: `pub const DEFAULT_MAX_FRAGMENTS: usize = 100_000`; `SplitterConfig::new`
  uses it (no behavior change; single source for the default).
- `camel-core`: `SplittingCompiler` carries `SplitterConfig.max_fragments` into
  `SplitSegment` on the programmatic `BuilderStep::Split` path; the declarative
  `DeclarativeSplit` path keeps the default (no new DSL keyword).
- Mechanical: all existing `SplitSegment { .. }` construction sites (processor tests,
  fanout drain-claim test, processor/camel-bench benches, split trace-restart tests) gain
  the field.
- Docs: short note in `docs/src/eip/splitter.md` (spec check found no cap requirement in
  `openspec/specs/eip-splitter/spec.md` — the restored contract lives in API/component
  docs only, so no delta spec is archived).
- Not included: DSL keyword, runtime-adjustable cap, `AggregationStrategy` or split
  metadata changes, streaming split cap.

## Acceptance criteria

- With a custom cap, `cap - 1` and exactly `cap` fragments pass; `cap + 1` fails
  with the exact message above.
- Rejection happens before any body-segment invocation (sequential and parallel).
- Empty fragments still pass through with a cap set.
- Builder path carries a custom `max_fragments` into the compiled segment (route-level
  check); declarative path uses 100_000.
- `SplitterConfig::validate()` still rejects 0; segment behavior with a 0-constructed
  cap is documented (any non-empty split fails closed).
- Touched crates' tests green; fmt, clippy, xtask lints, doc gates pass;
  `openspec validate --all --strict`.

## Risk budget

Low risk: one added struct field with compiler-enforced construction-site updates.
Acceptable: transient memory of cap-sized fragment vectors (unchanged). Out of bounds:
changing split semantics, aggregation, metadata, or the streaming path.
