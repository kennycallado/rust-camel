# Design: guide-eip-tier2-completion

## Approach

Follow the same include-driven page template established by prior EIP changes. Two categories of work:

1. **Tier-1 existing examples:** Insert anchor comment pairs in `examples/zip-splitter/src/main.rs`, `examples/delayer/src/main.rs`, `examples/loop/src/main.rs`, `examples/validator/src/main.rs`. Write 4 pages with includes from these anchors.

2. **Tier-2 new examples:** Create 6 new example directories. Each demonstrates one EIP pattern. The DSL exposes all 6 as `RouteDslStep` variants in `crates/camel-dsl/src/route_ast.rs`. Most compile in `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs`; `Enrich` compiles in `step_compilers/transforms.rs`; `Resequence` is intercepted by route helpers/controller (core.rs rejects any unintercepted resequencer). If the Rust RouteBuilder does not expose a builder method for a pattern, the example uses a YAML route file (following the `examples/file-pollenrich/routes.yaml` pattern) loaded via `camel_dsl::yaml::load_from_file`, registered via `add_route_definition(...)`, and verified by a successful `CamelContext::start()`. Write 6 pages with includes.

3. **Hub:** Move all 10 entries from Deferred into their families. Remove the Deferred section. Family assignments (totals must equal 27):
   - **Routing (8):** (existing 8, no additions)
   - **Transformation (4):** (existing 3) + Content Enricher
   - **Messaging (8):** (existing 3) + Zip Splitter, Sort, Sampling, Resequencer, Claim Check
   - **Resilience and control (7):** (existing 3) + Idempotent Consumer, Delayer, Loop, Validator

4. **SUMMARY:** Add 10 entries (27 total).

## Tier-2 example approach per pattern

| Pattern | DSL step | Repository needed | Example approach |
|---------|---------|-------------------|-----------------|
| Idempotent Consumer | `IdempotentConsumer(IdempotentConsumerStep)` | `MemoryIdempotentRepository` registered via `context.register_idempotent_repository()` | YAML route or Rust builder with repo registration |
| Content Enricher | `Enrich(EnrichStep)` | None (uses producer + strategy) | YAML route with enrich step + resource URI |
| Claim Check | `ClaimCheck(ClaimCheckStep)` | Claim check repository registered via `context.register_claim_check_repository()` | YAML route with set/get operations |
| Sort | `Sort(SortStep)` | None | YAML or Rust with sort expression |
| Sampling | `Sampling(SamplingStep)` | None | YAML or Rust with period config |
| Resequencer | `Resequence(ResequenceStep)` | None (batch/stream policy, intercepted as top-level step in `core.rs`) | YAML or Rust with sequence config |

## Affected crates

- `examples/`: 6 new example directories, 4 existing examples gain anchor comments
- `docs/src/eip/`: 10 new `.md` files, `index.md` rewritten (deferred entries moved to families)
- `docs/src/SUMMARY.md`: 10 new entries

No changes to `camel-core`, `camel-processor`, `camel-component-*`, or any source crate.

## ADR relevance

- ADR-0001 (Tower middleware pipeline): pages cite this for the composable-step model
- ADR-0025 (outcome-aware structural EIPs): relevant to Idempotent Consumer (outcome-aware segment), Resequencer (batch/stream reordering)
- `crates/camel-processor/CONTEXT.md` is the authority for each pattern's contract

## Alternatives considered

**Split into two changes (tier-1 and tier-2).** Rejected: the user wants the catalog completed in one run. The combined scope (10 pages + 6 examples) is manageable as a single change.

**Defer tier-2 patterns without examples.** Rejected: all 6 processors have DSL support and test coverage. Creating minimal examples is feasible.
