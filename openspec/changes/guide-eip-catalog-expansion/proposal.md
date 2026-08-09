# Proposal: guide-eip-catalog-expansion

## Why

The EIP pattern section of the mdBook guide covers 3 of the ~25 classic Enterprise Integration Patterns that rust-camel implements. A user browsing the EIP hub finds Message Filter, Circuit Breaker, and Aggregator — and nothing else. This gives the false impression that the framework supports only a handful of routing patterns. The `camel-processor` crate ships 43 processor modules; at least 20 are classic EIPs with runnable examples, but the guide does not surface them.

## What Changes

Add 14 tier-1 EIP pattern pages across 4 families:

- **Routing (7):** Content-Based Router, Dynamic Router, Recipient List, Routing Slip, Wire Tap, Multicast, Load Balancer
- **Transformation (3):** Convert Body, Marshal/Unmarshal (consolidated, two examples), Poll Enrich
- **Messaging (2):** Splitter, Streaming Splitter
- **Resilience (2):** Do Try, Throttler

Each page follows the existing include-driven template: `{{#include}}` from an anchored example region, prose naming the EIP, CONTEXT.md authority link, and ADR citations where the page states rationale.

Create one new runnable example for the Content-Based Router pattern (`choice` step), which currently has no dedicated example. Add anchor comment pairs to 14 existing examples (Marshal/Unmarshal consolidated page includes from two examples).

Regroup `eip/index.md` into four family headings. List tier-2 and catalog-only patterns (Zip Splitter, Delayer, Loop, Validator, Idempotent Consumer, Content Enricher, Claim Check, Sort, Sampling, Resequencer) as deferred catalog entries with one-line descriptions.

## Explicitly excluded

- Tier-2 pattern pages (no runnable example or specialized)
- Utility processor pages (Set/Map Body, Set Header/Property, Log, Error Handler, JSON Schema Validate)
- Content Enricher (no compiled `.enrich()` example; deferred until one exists)
- Claim Check, Sort, Sampling, Resequencer (catalog-only)

## Acceptance criteria

- 14 new EIP pages render in `mdbook build` with zero broken links
- Every page includes at least one `{{#include}}` from a compiled example
- EIP hub groups pages under 4 family headings
- New `examples/content-based-router/` compiles with `cargo build`
- `lint-slop`, `lint-adr-cite` pass on all new pages
- SUMMARY.md lists all 17 EIP pages

## Risk budget

Low risk: documentation-only change plus 14 comment-only example edits and one new executable example. The only new executable artifact is the choice example, which follows existing route-builder patterns already proven in 90+ examples.
