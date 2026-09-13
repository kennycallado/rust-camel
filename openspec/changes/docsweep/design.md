# Design: docsweep

## Approach

Apply four debt-driven documentation edit batches. First verify the route AST null
visitor behavior against the existing serde tests and current dependencies.
Regenerate processor citations from the `pub mod` declarations rather than
applying a constant offset. Use existing context-page structure and
ADR-0012 terminology for metric-label entries. Keep the broad `rc-8t20`
test-harness extraction out of this docs-only change.

## Affected crates

- `camel-dsl`: correct stale Rust doc comments only.
- `camel-processor`: correct `CONTEXT.md` source-line citations only.
- `crates/components/camel-ws/CONTEXT.md`,
  `crates/components/camel-file/CONTEXT.md`, and
  `crates/components/camel-timer/CONTEXT.md`: document existing component
  behavior and metric labels only.
- `docs/src/testing`: clarify the build matrix table.

## Architecture boundaries

The change does not alter the Runtime, DSL behavior, data plane, control
plane, component implementations, services, languages, or functions. The
Rust edit is comment-only. Context pages describe existing component
contracts and cite existing source; they do not introduce APIs. Metric names
follow the handler-contract boundary and side-effect-failure terminology in
ADR-0012 as indexed by `CONTEXT-MAP.md`.

## Alternatives considered

- **Update only the four issue titles:** rejected because the stale claims
  remain in user-facing documentation.
- **Extract the shared component test harness:** rejected because it changes
  code and is outside this docs-only mission.
- **Shift all processor citations by one value:** rejected because inserted
  modules caused non-uniform drift. Citations must be derived per module.
