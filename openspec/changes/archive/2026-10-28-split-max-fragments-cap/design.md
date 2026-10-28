# Design: split-max-fragments-cap

## Approach

Restore the documented `max_fragments` DoS cap (R3-M4) on the canonical eager split
path. The cap is carried config → compiler → segment and enforced once, on the segment,
at the only point where the full fragment count is known: after `SplitSource::split`
materialization and after the empty-fragments pass-through, before metadata stamping.

Enforcement (camel-processor `split_segment.rs`, inside `run()`):

```rust
let max_fragments = self.max_fragments; // captured with the other config
// ... splitter.split(&original) -> fragments
// ... if fragments.is_empty() { return Completed(original) }
if fragments.len() > max_fragments {
    return PipelineOutcome::Failed(CamelError::ProcessorError(format!(
        "Splitter produced {} fragments, exceeding max_fragments {}",
        fragments.len(), max_fragments
    )));
}
// metadata stamping, then parallel/sequential processing
```

Fail-closed semantics: over-cap rejects the whole split (no truncation). A
0-constructed segment (bypassing `SplitterConfig::validate()`, which rejects 0) fails
closed at split time for any non-empty split (`len > 0` always exceeds a cap of 0);
the empty pass-through still applies. Documented in the field doc.

Default single-sourcing (camel-api `splitter.rs`):

```rust
pub const DEFAULT_MAX_FRAGMENTS: usize = 100_000;
```

`SplitterConfig::new` uses the const (replaces the literal; Debug/validate unchanged).
`DeclarativeSplit` compilation uses the same const.

## Affected crates

- `camel-api`: `DEFAULT_MAX_FRAGMENTS` const; `SplitterConfig::new` uses it; field doc
  notes `SplitSegment` enforces the cap after materialization.
- `camel-processor`: `SplitSegment.max_fragments` field + manual `Clone` arm + `run()`
  enforcement + tests; mechanical field addition in existing tests
  (`split_segment.rs` tests, `tests/fanout_drainclaim_tests.rs`) and
  `benches/splitter.rs`.
- `camel-core`: `SplittingCompiler` — `BuilderStep::Split` sets
  `max_fragments: config.max_fragments`; `DeclarativeSplit` sets the default const;
  mechanical field addition in `split_trace_restart_tests.rs`; compiler tests.
- `camel-bench`: mechanical field addition in `benches/pipeline.rs`.
- `docs`: note in `docs/src/eip/splitter.md`.

## Architecture boundaries

Data/control-plane split untouched: the cap is pure control-plane config compiled into
a segment, mirroring `parallel`/`stop_on_exception`. Runtime (camel-processor) enforces;
DSL/builder (camel-core compilers) carries; camel-api owns the default. No component,
language, or service changes. ADR-0025 (OutcomePipeline segments), ADR-0090 (removal
must not remove capability), R3-M4 (DoS cap) referenced. Streaming split stays lazy and
uncapped by design.

Single-phase change: no `## Phases` section; tasks are one coherent slice ordered
API/processor → compiler/bench → docs.

## Alternatives considered

- **Delta spec**: rejected — `openspec/specs/eip-splitter/spec.md` has no cap
  requirement (grep verified); archiving would invent canon. Contract documented in
  API docs + `docs/src/eip/splitter.md` note instead.
- **Truncate to cap**: rejected — silently dropping fragments corrupts results;
  deleted contract rejected.
- **Check during materialization**: rejected — `SplitSource::split` returns the whole
  `Vec`; no incremental hook without changing the trait (out of scope).
- **Cap in `sequential_split`/`parallel_split`**: rejected — one check before the
  mode branch covers both and guarantees rejection before any spawn.
- **Downcast assert of compiled field**: rejected — `OutcomeSegment` exposes no
  `as_any`; compiler tests run the compiled segment instead (mission allows either).
