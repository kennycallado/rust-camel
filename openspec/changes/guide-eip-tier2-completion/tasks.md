# Tasks: guide-eip-tier2-completion

## Tier-2 Examples (require new code)

### Task 4.1: Create Idempotent Consumer, Content Enricher, and Claim Check examples

**Depends on:** (none)

**Files:**
- `examples/idempotent-consumer/Cargo.toml` (new)
- `examples/idempotent-consumer/src/main.rs` (new)
- `examples/content-enricher/Cargo.toml` (new)
- `examples/content-enricher/src/main.rs` (new)
- `examples/claim-check/Cargo.toml` (new)
- `examples/claim-check/src/main.rs` (new)

**Steps:**
1. Read `crates/camel-processor/src/idempotent_consumer.rs` to understand the `IdempotentConsumerSegment` and `IdempotentRepository` trait. Search for `MemoryIdempotentRepository` and `register_idempotent_repository` in `crates/camel-core/src/` to find the registration API and in-memory implementation. Create `examples/idempotent-consumer/` with a route that registers a memory idempotent repository and uses the idempotent consumer step to reject duplicate exchanges by message ID. Add `// ANCHOR: idempotent-consumer-route` around the route definition.
2. Read `crates/camel-processor/src/content_enricher.rs` to understand `EnrichService` and `EnrichmentStrategy`. Read `crates/camel-dsl/src/route_ast.rs` for the `EnrichStep` variant. Search for enrich step compilation in `crates/camel-core/src/` (the enrich step compiles in a transforms step compiler, not the core compiler). Create `examples/content-enricher/` with a route that uses the enrich step to call a resource and replace the exchange body with the result (default `UseEnrichedBody` strategy). Add `// ANCHOR: content-enricher-route` around the route definition.
3. Read `crates/camel-processor/src/claim_check.rs` to understand `ClaimCheckService` and `ClaimCheckOp` (Set, Get, GetAndRemove, Push, Pop). Search for `register_claim_check_repository` and claim check repository implementations in `crates/camel-core/src/`. Create `examples/claim-check/` with a route that registers a claim check repository and uses the set and get operations to store and retrieve large payloads. Add `// ANCHOR: claim-check-route` around the route definition.
4. Use YAML DSL if the Rust RouteBuilder does not expose a builder method for a pattern. For YAML-backed examples, create a `routes.yaml` (or `routes/route.yaml`) file with the EIP step. Put the `// ANCHOR:` / `// ANCHOR_END:` anchor pair (using `#` comment syntax for YAML) around the EIP step in the YAML file. The mdBook include then references the YAML file: `{{#include ../../../examples/<dir>/routes.yaml:<anchor-id>}}`. For Rust RouteBuilder examples, anchor in `src/main.rs` as usual. Search for `MemoryIdempotentRepository` and `ClaimCheckRepository` implementations in `crates/camel-core/src/` to find the exact registration API and paths.
5. Verify all 3 examples compile: `cargo build -p idempotent-consumer -p content-enricher -p claim-check`. For YAML-backed examples, additionally verify the route loads and starts without panic by running `timeout 3 cargo run -p <name>`. Accept exit code 0 (graceful) or 124 (timeout killed a running route). Any other exit code is a failure.

**Tests:**
- `idempotent-consumer-compiles`: `cargo build -p idempotent-consumer` exits 0
- `content-enricher-compiles`: `cargo build -p content-enricher` exits 0
- `claim-check-compiles`: `cargo build -p claim-check` exits 0
- `yaml-startup-check`: for each YAML-backed example, `timeout 3 cargo run -p <name>` exits 0 or 124
- `anchors-present`: each example has exactly one `ANCHOR:` and one `ANCHOR_END:` with the correct ID

**Acceptance:**
- All 3 examples compile with zero errors
- `cargo clippy` clean on all 3
- Each example has the correct anchor pair
- YAML-backed examples start without panic (exit 0 or 124 from timeout)

- [x] 4.1

### Task 4.2: Create Sort, Sampling, and Resequencer examples

**Depends on:** (none)

**Files:**
- `examples/sort/Cargo.toml` (new)
- `examples/sort/src/main.rs` (new)
- `examples/sampling/Cargo.toml` (new)
- `examples/sampling/src/main.rs` (new)
- `examples/resequencer/Cargo.toml` (new)
- `examples/resequencer/src/main.rs` (new)

**Steps:**
1. Read `crates/camel-processor/src/sort.rs` for `SortService::new(expression, reverse)`. Read `crates/camel-dsl/src/route_ast.rs` for `SortStep`. Create `examples/sort/` with a route that sorts exchanges by a body expression (e.g., sort by numeric value). Add `// ANCHOR: sort-route` around the route definition.
2. Read `crates/camel-processor/src/sampling.rs` for `SamplingService::new(period)`. Read `crates/camel-dsl/src/route_ast.rs` for `SamplingStep`. Create `examples/sampling/` with a route that samples 1 out of every N exchanges. Add `// ANCHOR: sampling-route` around the route definition.
3. Read `crates/camel-processor/src/resequencer/mod.rs` for `ResequencerService`, `ResequencerConfig`, `BatchPolicy`, `StreamPolicy`. Note: Resequence is intercepted by route helpers/controller (not compiled in the core step compiler like other steps). Search for resequencer route usage patterns in `crates/camel-core/src/` to understand how the route is registered. Create `examples/resequencer/` with a route that reorders exchanges by sequence number. Add `// ANCHOR: resequencer-route` around the route definition.
4. Use YAML DSL if the Rust RouteBuilder does not expose a builder method. For YAML-backed examples, create `routes.yaml` with the EIP step and anchor it in the YAML file (using `#` comment syntax). The include references the YAML file. Follow the `examples/file-pollenrich/` structure for YAML-backed examples.
5. Verify all 3 examples compile: `cargo build -p sort -p sampling -p resequencer`. For YAML-backed examples, additionally verify with `timeout 3 cargo run -p <name>`. Accept exit code 0 or 124. Any other exit code is a failure.

**Tests:**
- `sort-compiles`: `cargo build -p sort` exits 0
- `sampling-compiles`: `cargo build -p sampling` exits 0
- `resequencer-compiles`: `cargo build -p resequencer` exits 0
- `yaml-startup-check`: for each YAML-backed example, `timeout 3 cargo run -p <name>` exits 0 or 124
- `anchors-present`: each example has exactly one `ANCHOR:` and one `ANCHOR_END:` with the correct ID

**Acceptance:**
- All 3 examples compile with zero errors
- `cargo clippy` clean on all 3
- Each example has the correct anchor pair
- YAML-backed examples start without panic (exit 0 or 124 from timeout)

- [x] 4.2

## Tier-1 Anchors (existing examples)

### Task 4.3: Add anchor comment pairs to 4 existing tier-1 examples

**Depends on:** (none)

**Files (all comment-only modifications):**
- `examples/zip-splitter/src/main.rs` (modified)
- `examples/delayer/src/main.rs` (modified)
- `examples/loop/src/main.rs` (modified)
- `examples/validator/src/main.rs` (modified)

**Steps:**
For each file, read the source, locate the route builder block that demonstrates the EIP pattern, and insert `// ANCHOR:` and `// ANCHOR_END:` comments around it. Comments only.

1. `zip-splitter/src/main.rs` — anchor `zip-splitter-route` around the primary route block. This example demonstrates ZIP data format marshal/unmarshal and streaming splitter.
2. `delayer/src/main.rs` — anchor `delayer-route` around the route block that contains the delay step.
3. `loop/src/main.rs` — anchor `loop-route` around the route block that contains the loop step.
4. `validator/src/main.rs` — anchor `validator-route` around the FIRST route block (XSD validation). This example has multiple routes; anchor the one that validates XML orders.

Verify all 4 examples still compile.

**Tests:**
- `anchor-present-zip-splitter`: grep for `ANCHOR: zip-splitter-route` and `ANCHOR_END: zip-splitter-route` in the file
- `anchor-present-delayer`: same pattern for `delayer-route`
- `anchor-present-loop`: same pattern for `loop-route`
- `anchor-present-validator`: same pattern for `validator-route`
- `all-4-compile`: `cargo build -p zip-splitter -p delayer -p loop -p validator` exits 0

**Acceptance:**
- All 4 anchor pairs present
- All 4 examples compile

- [x] 4.3

## Documentation

### Task 4.4: Write 10 EIP pattern pages

**Depends on:** 4.1, 4.2, 4.3

**Files (all new):**
- `docs/src/eip/zip-splitter.md`
- `docs/src/eip/delayer.md`
- `docs/src/eip/loop.md`
- `docs/src/eip/validator.md`
- `docs/src/eip/idempotent-consumer.md`
- `docs/src/eip/content-enricher.md`
- `docs/src/eip/claim-check.md`
- `docs/src/eip/sort.md`
- `docs/src/eip/sampling.md`
- `docs/src/eip/resequencer.md`

**Steps:**
1. For each page, write a markdown file following the established template (read `docs/src/eip/filter.md` or `docs/src/eip/multicast.md` as reference). Each page has: level-1 heading, one-sentence EIP name + Hohpe/Woolf category, include directive, 2-4 paragraphs of prose, link to `../../../crates/camel-processor/CONTEXT.md`, ADR-0001 citation, example source link.

2. Include directives (adjust file if YAML-backed — see Task 4.1/4.2 step 4):
   - `zip-splitter.md` → `{{#include ../../../examples/zip-splitter/src/main.rs:zip-splitter-route}}`
   - `delayer.md` → `{{#include ../../../examples/delayer/src/main.rs:delayer-route}}`
   - `loop.md` → `{{#include ../../../examples/loop/src/main.rs:loop-route}}`
   - `validator.md` → `{{#include ../../../examples/validator/src/main.rs:validator-route}}`
   - `idempotent-consumer.md` → include from whichever file holds the anchor (main.rs or routes.yaml)
   - `content-enricher.md` → include from whichever file holds the anchor
   - `claim-check.md` → include from whichever file holds the anchor
   - `sort.md` → include from whichever file holds the anchor
   - `sampling.md` → include from whichever file holds the anchor
   - `resequencer.md` → include from whichever file holds the anchor

3. ADR citations per page: all cite ADR-0001. Idempotent Consumer cites ADR-0025 (outcome-aware segment). Resequencer cites ADR-0025 (batch/stream reordering semantics).

4. Run `cargo run -p xtask -- lint-slop --deny` and `cargo run -p xtask -- lint-adr-cite --deny` on all 10 files. Fix any violations.

**Tests:**
- `lint-slop-all-10`: `cargo run -p xtask -- lint-slop --deny docs/src/eip/zip-splitter.md docs/src/eip/delayer.md docs/src/eip/loop.md docs/src/eip/validator.md docs/src/eip/idempotent-consumer.md docs/src/eip/content-enricher.md docs/src/eip/claim-check.md docs/src/eip/sort.md docs/src/eip/sampling.md docs/src/eip/resequencer.md` → zero violations
- `lint-adr-cite-all-10`: same file list with `lint-adr-cite` → zero violations
- `idempotent-cites-adr-0025`: grep `idempotent-consumer.md` for `ADR-0025` → at least one match
- `resequencer-cites-adr-0025`: grep `resequencer.md` for `ADR-0025` → at least one match

**Acceptance:**
- All 10 files exist
- `lint-slop` and `lint-adr-cite` zero violations
- Idempotent Consumer and Resequencer cite ADR-0025

- [x] 4.4

### Task 4.5: Rewrite EIP hub and wire SUMMARY

**Depends on:** 4.4

**Files:**
- `docs/src/eip/index.md` (modified)
- `docs/src/SUMMARY.md` (modified)

**Steps:**
1. Rewrite `docs/src/eip/index.md`: remove the Deferred section entirely. Distribute all 10 patterns into their family groups:

   **Routing (8):** Message Filter, Content-Based Router, Dynamic Router, Recipient List, Routing Slip, Wire Tap, Multicast, Load Balancer (unchanged)

   **Transformation (4):** Convert Body, Marshal and Unmarshal, Poll Enrich, Content Enricher

   **Messaging (8):** Aggregator, Splitter, Streaming Splitter, Zip Splitter, Sort, Sampling, Resequencer, Claim Check

   **Resilience and control (7):** Circuit Breaker, Do Try, Throttler, Idempotent Consumer, Delayer, Loop, Validator

2. Update `docs/src/SUMMARY.md`: add 10 new entries to the EIP section in family order. The section should have 27 indented entries total (excluding the hub line).

**Tests:**
- `hub-has-four-sections-no-deferred`: read `eip/index.md` → grep for `^## ` → exactly 4 matches (Routing, Transformation, Messaging, Resilience and control), zero matches for "Deferred"
- `summary-has-27-entries`: read `SUMMARY.md` → count indented lines matching `eip/` → exactly 27

**Acceptance:**
- Zero Deferred section in hub
- SUMMARY has 27 EIP entries

- [x] 4.5

## Verification

### Task 4.6: Build and verify the complete guide

**Depends on:** 4.1, 4.2, 4.3, 4.4, 4.5

**Steps:**
1. Run `nix shell nixpkgs#mdbook -c mdbook build docs 2>&1`. Verify exit code 0. Grep output for `broken link`, `not found`, `failed to resolve` — verify zero matches.
2. Verify relative Markdown links resolve: run `find docs/src -name '*.md' | while read f; do dir=$(dirname "$f"); grep -oP '\]\(\K[^)#]+\.md' "$f" | while read link; do [ -f "$dir/$link" ] || echo "BROKEN: $f -> $link"; done; done`. Verify zero output.
3. Run `cargo run -p xtask -- lint-slop --deny docs/src/eip/`. Verify zero violations.
4. Run `cargo run -p xtask -- lint-adr-cite --deny docs/src/eip/`. Verify zero violations.
5. Run `cargo build` on all 10 examples (6 new + 4 existing). Verify all exit 0.

**Tests:**
- `mdbook-build-exit-0`: exit code 0
- `mdbook-output-clean`: zero broken-link/not-found matches
- `relative-links-resolve`: zero output from link-check command
- `lint-slop-eip-zero`: zero violations
- `lint-adr-cite-eip-zero`: zero violations
- `all-10-examples-compile`: all exit 0

**Acceptance:**
- mdbook build exits 0, zero warnings
- Link check zero broken
- All linters clean
- All 10 examples compile

- [x] 4.6
