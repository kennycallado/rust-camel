# Proposal: docsweep

## Why

Four small documentation debts make repository guidance inaccurate: two DSL
comments misstate YAML null handling and an existing type, the testing build table confuses the
default and featureless builds, the processor catalog has stale source-line
citation numbers, and component context pages omit current b-prime metric
labels. Correcting these records keeps documentation aligned with the code
and current feature configuration.

## What Changes

- Correct the two stale comments in `crates/camel-dsl/src/route_ast.rs`.
- Clarify the testing build table in `docs/src/testing/index.md`.
- Regenerate processor catalog line citations in
  `crates/camel-processor/CONTEXT.md`.
- Document the WS and file b-prime labels and add the missing timer context
  page.

The unrelated component test-harness extraction from bd `rc-8t20` is
explicitly excluded. Affected files include `crates/camel-dsl/src/route_ast.rs`,
`crates/camel-processor/CONTEXT.md`,
`crates/components/camel-ws/CONTEXT.md`,
`crates/components/camel-file/CONTEXT.md`,
`crates/components/camel-timer/CONTEXT.md`, and testing documentation.
This change addresses bd `rc-obbs1`, `rc-sox53`, `rc-nchfq`, and `rc-8t20`.

## Acceptance criteria

- All four bd debts are addressed without production behavior changes.
- Documentation citations and metric labels match the current source.
- The docs-only verification gates pass, including context-citation and
  applicable rustdoc checks.

## Risk budget

Only comments and Markdown context documentation may change. No test harness
extraction, refactor, dependency change, or runtime behavior change is in
scope. Rust formatting and rustdoc checks are required because one task edits
Rust doc comments.
