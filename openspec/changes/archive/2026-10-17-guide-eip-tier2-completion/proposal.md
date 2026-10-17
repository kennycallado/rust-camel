# Proposal: guide-eip-tier2-completion

## Why

The EIP catalog has 17 pages but 10 patterns remain in the deferred section. Four of those (Zip Splitter, Delayer, Loop, Validator) already have runnable examples — they need only anchor pairs and pages. Six more (Idempotent Consumer, Content Enricher, Claim Check, Sort, Sampling, Resequencer) have processor modules and DSL support but no runnable examples. Completing these closes the currently declared 27-page EIP catalog with zero deferred entries.

## What Changes

**Tier-1 (4 pages, existing examples):** Add anchor comment pairs to `examples/zip-splitter/`, `examples/delayer/`, `examples/loop/`, `examples/validator/`. Write 4 include-backed pages.

**Tier-2 (6 pages, 6 new examples):** Create runnable examples for Idempotent Consumer, Content Enricher, Claim Check, Sort, Sampling, and Resequencer. Each example demonstrates the pattern via the DSL (YAML or Rust RouteBuilder, whichever the DSL exposes). Write 6 include-backed pages.

**Hub update:** Move all 10 patterns from the Deferred section into their family groups. Remove the Deferred section entirely.

**SUMMARY:** Add 10 new entries (27 total EIP pages).

## Explicitly excluded

- No new processors or DSL features — only documentation and examples
- Languages, Connectors, Observability sections (separate future changes)

## Acceptance criteria

- 10 new EIP pages render in `mdbook build` with zero broken links
- 6 new examples compile with `cargo build`
- 4 existing examples still compile after anchor addition
- `lint-slop`, `lint-adr-cite` pass on all new pages
- EIP hub has zero deferred entries
- SUMMARY.md lists 27 EIP pages

## Risk budget

Medium: 6 new examples require understanding the DSL API for each processor. The Idempotent Consumer and Claim Check need repository registration in the Camel context. If any processor's DSL builder method is not available through the Rust RouteBuilder, the example uses YAML DSL instead (following the `file-pollenrich` pattern).
