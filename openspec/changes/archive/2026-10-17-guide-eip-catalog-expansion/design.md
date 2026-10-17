# Design: guide-eip-catalog-expansion

## Approach

New EIP pages follow the include-driven structure used by existing pages (`eip/filter.md`, `eip/circuit-breaker.md`, `eip/aggregator.md`): each page uses `{{#include}}` from an anchored example region and adds explanatory prose. New pages go beyond the existing three by adding ADR citations where the page states rationale, and linking the `crates/camel-processor/CONTEXT.md` authority for contract details.

1. **Anchor addition**: insert `// ANCHOR: <id>` / `// ANCHOR_END: <id>` comment pairs around the relevant route builder block in each example's `src/main.rs`. Anchors are comment-only — no executable code changes.

2. **Page authoring**: each page uses `{{#include ../../../examples/<name>/src/main.rs:ANCHOR_ID}}` to pull the route code block, then adds prose naming the EIP, citing `crates/camel-processor/CONTEXT.md` for the contract, and linking ADRs where the page states architectural rationale. Pages stay 40-100 lines. The Marshal/Unmarshal consolidated page includes from two examples.

3. **New example**: create `examples/content-based-router/` with a timer-driven route that uses the `choice` step with `when`/`otherwise` branches to route exchanges by body content. Follow `examples/content-based-routing/` structure (Cargo.toml, src/main.rs).

4. **Hub regrouping**: rewrite `eip/index.md` with four `##` family headings (Routing, Transformation, Messaging, Resilience and control), each listing its pages. Add a "Deferred patterns" section for tier-2 and catalog-only entries.

5. **SUMMARY wiring**: add 14 new entries under the existing EIP section, flat list (family grouping lives only in the hub page).

## Page-to-example-to-anchor mapping

| Page | Example(s) | Anchor ID |
|------|-----------|-----------|
| `content-based-router.md` | `content-based-router` (NEW) | `cbr-route` |
| `dynamic-router.md` | `dynamic-router` | `dynamic-router-route` |
| `recipient-list.md` | `recipientlist` | `recipient-list-route` |
| `routing-slip.md` | `routing-slip` | `routing-slip-route` |
| `wire-tap.md` | `wiretap` | `wire-tap-route` |
| `multicast.md` | `multicast` | `multicast-route` |
| `load-balancer.md` | `load-balancer` | `load-balancer-route` |
| `convert-body.md` | `convert-body-to` | `convert-body-route` |
| `marshal-unmarshal.md` | `marshal-csv` + `marshal-unmarshal` | `marshal-route` + `unmarshal-route` |
| `poll-enrich.md` | `file-pollenrich` | `poll-enrich-route` |
| `splitter.md` | `splitter` | `splitter-route` |
| `streaming-splitter.md` | `streaming-split` | `streaming-split-route` |
| `do-try.md` | `do-try` | `do-try-route` |
| `throttler.md` | `throttler` | `throttler-route` |

Total: 1 new example + 14 existing examples (15 example directories touched, 15 anchors added). Content Enricher deferred — no compiled `.enrich()` example exists.

## Affected crates

- `examples/`: 1 new example directory (`content-based-router`), 14 existing examples gain anchor comments
- `docs/src/eip/`: 14 new `.md` files, `index.md` rewritten with family grouping
- `docs/src/SUMMARY.md`: 14 new entries

No changes to `camel-core`, `camel-processor`, `camel-component-*`, or any source crate.

## ADR relevance

- ADR-0001 (Tower middleware pipeline): pages cite this for the composable-step model
- ADR-0025 (Outcome-aware structural EIPs): relevant to Multicast, Splitter, Recipient List, Streaming Splitter where partial outcomes and structural segment behavior matter. Backpressure semantics for Streaming Splitter come from the processor authority (`crates/camel-processor/CONTEXT.md`), not a dedicated ADR
- `crates/camel-processor/CONTEXT.md` is the authority for each pattern's contract

## Alternatives considered

**Amend the existing change.** Rejected: `guide-recipes-operations-extending` is already spec-blessed, plan-blessed, implemented, and holistically reviewed. Amending completed, approved work invalidates prior review. A fresh change with its own blessing is cleaner.

**Document all 43 processors.** Rejected: utility processors (Set Header, Set Property, Log) are not EIPs and do not warrant pattern pages. Catalog entries in the hub suffice.

**Inline code snippets for patterns without examples.** Rejected: the two-source rule (blessed in change #1) requires form claims backed by compiled examples. Patterns without examples are deferred to tier-2 or catalog-only until examples exist. Content Enricher falls in this category — the `.enrich()` builder has no runnable example, so the pattern is deferred despite having a processor module.
