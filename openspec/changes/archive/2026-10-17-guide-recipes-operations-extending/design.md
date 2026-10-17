# Design: guide-recipes-operations-extending

## Context

Builds on the blessed `guide-foundation-concepts-and-drift-contract` (concepts, glossary, 3 advisory linters, two-source rule). That change left six SUMMARY sections as stubs. This change fills them. The linters (`lint-glossary`, `lint-slop`, `lint-adr-cite`) and the two-source rule govern all new content unchanged. Run them with `--deny` to enforce (advisory exit 0 by default).

## Approach

Two structural moves, repeated per section:

1. **Navigation hub.** Each section `index.md` is a curated map: a one-paragraph frame, then categorized links to runnable examples (`examples/<name>`) and per-crate `CONTEXT.md` files. A hub owns no form-claims (no `{{#include}}`, no inline code fence) and no ADR paraphrase; it routes only.

2. **Foundation pages.** Representative pages per section demonstrate current FORM via an anchored `{{#include}}` from a compiled example, then cite the governing ADR for the WHY with a one-sentence paraphrase. Prose is connective only; no domain-term definitions (glossary owns those); no ADR-reasoning restatement.

### ANCHOR strategy (enables the two-source rule)

New `ANCHOR` / `ANCHOR_END` comment pairs added to example source (comment-only; no behavioral change). Rust files use `// ANCHOR: <name>`; YAML files use `# ANCHOR: <name>`. Each modified example must still `cargo check`. Anchors (from the example survey):

| File | Anchor | Lines | Syntax |
|------|--------|-------|--------|
| `examples/content-based-routing/src/main.rs` | `filter-route` | 24-47 | `//` |
| `examples/circuit-breaker/src/main.rs` | `circuit-breaker-route` | 49-73 | `//` |
| `examples/aggregator/src/main.rs` | `aggregator-route` | 37-89 | `//` |
| `examples/file-pipeline/src/main.rs` | `file-pipeline-route` | 27-52 | `//` |
| `examples/custom-component-bundle/src/main.rs` | `echo-bundle-impl`, `echo-bundle-register` | 142-161; 192-201 | `//` |
| `examples/health-demo/src/main.rs` | `health-config`, `health-route` | 28-37; 53-56 | `//` |
| `examples/controlbus/src/main.rs` | `controlbus-suspend-route` | 30-41 | `//` (anchor-only; route-lifecycle page deferred per ADR-0034 — example is stale) |
| `examples/http-server/src/main.rs` | `http-health-route` | 66-92 | `//` |
| `examples/hot-reload/routes/route.yaml` | `hot-reload-route` | 1-4 | `#` |

That is **10 Rust anchor pairs across 8 files + 1 YAML anchor pair = 11 new anchors across 9 example files**. The existing `config-basic/routes/hello.yaml:first-route` and `hello-world/src/main.rs:first-route` anchors (from #1) are reused unchanged.

### DEFERRED: ControlBus route-lifecycle page

`examples/controlbus/src/main.rs` uses the `CamelRouteId` exchange header to target routes (lines 36, 50). ADR-0034 removed this header for security (intra-process privilege escalation) and requires `controlbus:route?routeId=<static>&action=...` plus a mandatory `authorizedRoutes` allowlist. The example is therefore STALE and must NOT be included as a form-claim. This change DEFERS the route-lifecycle page and files a bd follow-up to fix the stale example. Operations covers health only this change.

### Section-by-section structure

**EIP patterns (`docs/src/eip/`)** — index hub categorizes patterns (routing, transformation, resilience, messaging) with example links. Three foundation pages:
- `filter.md` — Message Filter EIP. Include `content-based-routing:filter-route`. NOTE: this example uses `.filter()`/`.end_filter()`, NOT `choice` (it is a Message Filter, not a Content-Based Router). Page title: "Message Filter". Cite the filter semantics.
- `circuit-breaker.md` — resilience. Include `circuit-breaker:circuit-breaker-route`. Cite ADR-0019 (CircuitBreaker compiles into a gate on `RouteChannelService` that wraps the pipeline; it is not a Pipeline Step).
- `aggregator.md` — messaging. Include `aggregator:aggregator-route`. Cite the AggregatorConfig correlation semantics.

**Components (`docs/src/components/`)** — index hub is a catalog TABLE: columns URI-scheme | direction (source/sink/both) | crate | link to the component's local `CONTEXT.md` or nearest parent (`components/CONTEXT.md`) per the coverage policy. The URI-scheme values are authority-derived reference data (each scheme comes from its component's `CONTEXT.md`), NOT form-claims needing an include — the table maps to authorities, it does not assert compiled form. Three foundation pages:
- `timer-log.md` — reuse the `hello-world:first-route` include (already shows `register_component(TimerComponent)` + `register_component(LogComponent)` + `timer:tick?period=` + `log:info?`). Cite the timer PollingConsumer behavior via ADR-0015 only if the page makes that specific claim; otherwise omit the citation.
- `file.md` — Include `file-pipeline:file-pipeline-route` (`file:{input}?delete=true` source, `file:{output}?fileExist=Override` sink). Link `crates/components/CONTEXT.md`.
- `http.md` — Include `http-server:http-health-route` (`http://0.0.0.0:8080/health` consumer). The page claims ONLY what the anchored region shows; it does not assert URI params (`maxResponseBody`, `maxInflightRequests`) unless they appear in the included lines.

**YAML DSL (`docs/src/yaml-dsl/`)** — index is a pure navigation hub (no include; it routes to the structure page and the schema). One foundation page:
- `route-structure.md` — includes `config-basic/routes/hello.yaml:first-route` (the `routes:`/`id`/`from`/`steps` shape) AND `hot-reload/routes/route.yaml:hot-reload-route`. Cites `schemas/dsl/route-schema.json` as the authoritative schema.

**Operations (`docs/src/operations/`)** — index hub for runtime concerns (health, metrics, route lifecycle — the last deferred). One foundation page:
- `health.md` — Include `health-demo:health-config` + `health-demo:health-route`. Explain `ObservabilityConfig.health` wiring + the `/readyz`/`/healthz` contract. Reference the Degraded/Unhealthy distinction (glossary).

**Extending (`docs/src/extending/`)** — index overview. One foundation page:
- `custom-component.md` — building a `ComponentBundle`. Include `custom-component-bundle:echo-bundle-impl` + `custom-component-bundle:echo-bundle-register`. Explain `config_key()`/`from_toml`/`register_all`/`register_component_dyn`. Link `crates/components/camel-component-api/CONTEXT.md`.

**Architecture (`docs/src/architecture/`)** — index hub ONLY (no deep data-plane page; the `Service<Exchange>` contract lacks a compiled example exposing the trait signature, so asserting it remains deferred as in #1). The index is a **crate map**: a table of every crate that has a local `CONTEXT.md` (per the CONTEXT-MAP coverage policy, not every crate has one; thin adapters defer to a parent `CONTEXT.md`) with a one-line role + link. Plus an ADR-index pointer (`docs/adr/`) and a link back to `concepts/planes.md` for the data-plane/control-plane split. No hand-typed trait signature.

### SUMMARY.md wiring

`docs/src/SUMMARY.md` gains nested sub-page entries under each of the six section headings (EIP, Components, YAML DSL, Operations, Extending, Architecture), mirroring the pattern established for Core concepts in #1.

## Phases

Single-phase. All work is one coherent deliverable. Task order: anchors first (enables includes), then SUMMARY wiring, then the six section batches (independent directories, no file overlap).

## Risks & mitigations

- **Anchor comments break example compilation.** Mitigation: comment-only inserts between existing lines; `cargo check -p <example>` per modified example in the anchor task's acceptance.
- **Linters are advisory by default.** Mitigation: all gates run with `--deny` so violations fail the task. Note: `lint-glossary` does not govern these pages (they introduce no new glossary terms); `lint-slop` and `lint-adr-cite` do.
- **mdbook build does not validate every Markdown link.** Mitigation: after `mdbook build`, run an explicit link-existence check (every `(xxx.md)` relative link resolves to a file) as a separate verification step.
- **`Service<Exchange>` form-claim temptation in architecture.** Mitigation: architecture is index-only; the trait signature is NOT asserted. The page links to `concepts/planes.md` and ADR-0001 instead.
- **Catalog table staleness.** Mitigation: the components index links to `CONTEXT.md` (the authority) rather than restating definitions; the table is a map.
- **Stale ControlBus example.** Mitigation: deferred + bd follow-up; not included this change.
