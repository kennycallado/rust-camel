# Proposal: guide-recipes-operations-extending

## Why

Change `guide-foundation-concepts-and-drift-contract` delivered the concept spine, the glossary, and the drift contract (linters). The remaining six SUMMARY sections — EIP patterns, Components, YAML DSL, Operations, Extending, Architecture — are 6-to-11-line stubs that point readers to the GitHub examples directory and stop. A reader who finished the concepts section has no in-guide path from "I understand Exchange/Route/Component" to "I can wire a choice pattern, register a file component, read the YAML schema, operate a running context, or extend the framework with a custom component."

This change fills those six sections with foundation content that follows the two-source rule established in the prior change: every form-bearing page demonstrates current shape via an anchored `{{#include}}` from a compiled example and cites the governing ADR for the why. Navigation hubs (the section index pages) curate links to the per-crate `CONTEXT.md` files and runnable examples so the guide becomes a map, not a dead end.

## What Changes

**In:**
- `docs/src/eip/` — `index.md` as a navigation hub categorizing the EIP patterns (routing, transformation, resilience, messaging) with links to examples; pattern pages for Message Filter (the content-based-routing example demonstrates `.filter()`, not `choice`), circuit breaker, and aggregator, each with a new `ANCHOR` in the corresponding example and an ADR citation.
- `docs/src/components/` — `index.md` as a catalog table of all components (URI scheme, source/sink, link to `CONTEXT.md` or nearest parent per the coverage policy); detail pages for timer+log (reusing the hello-world include), file, and http components using anchored includes.
- `docs/src/yaml-dsl/` — `index.md` as a pure navigation hub (no include); `route-structure.md` explaining the `from`/`steps`/`to` shape with the existing `config-basic/routes/hello.yaml` include, citing the JSON schema at `schemas/dsl/route-schema.json`.
- `docs/src/operations/` — `index.md` as a navigation hub for runtime operations (health endpoints, metrics, route lifecycle); a health page using anchored includes from the `health-demo` example. (Route lifecycle via ControlBus is deferred: the `controlbus` example is stale — it uses the `CamelRouteId` header removed by ADR-0034; a bd follow-up will fix the example before that page lands.)
- `docs/src/extending/` — `index.md` plus a custom-component guide page using an anchored include from `examples/custom-component-bundle`, citing the component-api `CONTEXT.md`.
- `docs/src/architecture/` — `index.md` as a crate-map navigation hub linking every crate `CONTEXT.md` (or nearest parent per the coverage policy), the ADR index, and a link back to `concepts/planes.md`. Index-only this change; the `Service<Exchange>` trait signature is NOT asserted (no compiled example exposes it; the deferral from #1 holds).
- `examples/*/src/main.rs` and `examples/*/routes/*.yaml` — new `ANCHOR` / `ANCHOR_END` comment pairs (10 in Rust files, 1 in a YAML file) added to 8 example files to enable the includes above. No behavioral change to any example; comments only.

**Out:**
- Exhaustive per-pattern documentation (every EIP gets a page). This change covers representative patterns; the rest are linked from the index hub for incremental follow-up.
- Per-component documentation for all 30 components. This change details timer/log/file (already in the hello-world include or simple) and catalogs the rest.
- Changes to `CONTEXT-MAP.md` Key Terms (the guide remains a pure consumer).
- Any change to the linters added in the prior change (they govern this content unchanged).

## Acceptance criteria

- All six section index pages are navigation hubs with categorized links (not stubs).
- `mdbook build docs` succeeds (includes resolve, links valid) via `nix shell nixpkgs#mdbook -c mdbook build docs`.
- `lint-slop`, `lint-adr-cite`, `lint-glossary` each report 0 violations on all new and modified pages.
- Every new `ANCHOR` pair is in a file that still compiles (`cargo check -p <example>` passes for each modified example).
- No page hand-types example code that duplicates a compiled include (the two-source rule from #1 holds).

## Risk budget

- Adding `ANCHOR` comments to example source files is the only code-adjacent change; it is comment-only and cannot alter behavior. Acceptable.
- The architecture data-plane page asserts the `Service<Exchange>` trait signature as a form-claim only if a compiled example provides it; if no example exposes it, the page cites it as ADR-derived rationale (same constraint as #1's planes.md). Acceptable.
- Scope is broad (6 sections) but each page is mechanically lint-checked; a wrong page fails the gate, not the reader. Acceptable.
