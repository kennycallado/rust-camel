# Tasks: guide-recipes-operations-extending

<!--
  Single-phase change. Fills the 6 remaining guide sections.
  Task order: anchors (1) enable the includes the page tasks (3-8) cite;
  SUMMARY wiring (2) lands alongside the pages. Each page task is
  independent (distinct directory, no file overlap).
-->

## Example anchors

### Task 2.1: Add ANCHOR comment pairs to 9 example files

**Files:**
- `examples/content-based-routing/src/main.rs` (modified)
- `examples/circuit-breaker/src/main.rs` (modified)
- `examples/aggregator/src/main.rs` (modified)
- `examples/file-pipeline/src/main.rs` (modified)
- `examples/custom-component-bundle/src/main.rs` (modified)
- `examples/health-demo/src/main.rs` (modified)
- `examples/controlbus/src/main.rs` (modified — see note)
- `examples/http-server/src/main.rs` (modified)
- `examples/hot-reload/routes/route.yaml` (modified)

**Steps:**
1. In `examples/content-based-routing/src/main.rs`, insert `// ANCHOR: filter-route` before the `RouteBuilder::from` builder call (line 24) and `// ANCHOR_END: filter-route` after the `.build()?;` line (line 47). The wrapped region is the filter route.
2. In `examples/circuit-breaker/src/main.rs`, insert `// ANCHOR: circuit-breaker-route` before line 49 (the main timer route `RouteBuilder::from("timer:cb-test?period=1000&repeatCount=15")`) and `// ANCHOR_END: circuit-breaker-route` after line 73 (its `.build()?;`).
3. In `examples/aggregator/src/main.rs`, insert `// ANCHOR: aggregator-route` before line 37 and `// ANCHOR_END: aggregator-route` after line 89.
4. In `examples/file-pipeline/src/main.rs`, insert `// ANCHOR: file-pipeline-route` before line 27 and `// ANCHOR_END: file-pipeline-route` after line 52.
5. In `examples/custom-component-bundle/src/main.rs`, insert `// ANCHOR: echo-bundle-impl` before line 142 and `// ANCHOR_END: echo-bundle-impl` after line 161; insert `// ANCHOR: echo-bundle-register` before line 192 and `// ANCHOR_END: echo-bundle-register` after line 201.
6. In `examples/health-demo/src/main.rs`, insert `// ANCHOR: health-config` before line 28 and `// ANCHOR_END: health-config` after line 37; insert `// ANCHOR: health-route` before line 53 and `// ANCHOR_END: health-route` after line 56.
7. In `examples/controlbus/src/main.rs`, insert `// ANCHOR: controlbus-suspend-route` before line 30 and `// ANCHOR_END: controlbus-suspend-route` after line 41. (NOTE: this example is stale per ADR-0034 — it uses the removed `CamelRouteId` header. The anchor is added so a future bd follow-up can fix the example and the operations route-lifecycle page can include it. Do NOT fix the stale code in this task; comments only.)
8. In `examples/http-server/src/main.rs`, insert `// ANCHOR: http-health-route` before line 66 and `// ANCHOR_END: http-health-route` after line 92 (the `RouteBuilder` block inside `create_health_route`).
9. In `examples/hot-reload/routes/route.yaml`, insert `# ANCHOR: hot-reload-route` before line 1 and `# ANCHOR_END: hot-reload-route` after line 4 (YAML comment syntax, not `//`).

**Tests:** (verification — shell)
- `rg -F -c 'ANCHOR: filter-route' examples/content-based-routing/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: circuit-breaker-route' examples/circuit-breaker/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: aggregator-route' examples/aggregator/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: file-pipeline-route' examples/file-pipeline/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: echo-bundle-impl' examples/custom-component-bundle/src/main.rs` AND `rg -F -c 'ANCHOR: echo-bundle-register' examples/custom-component-bundle/src/main.rs` → each 1.
- `rg -F -c 'ANCHOR: health-config' examples/health-demo/src/main.rs` AND `rg -F -c 'ANCHOR: health-route' examples/health-demo/src/main.rs` → each 1.
- `rg -F -c 'ANCHOR: controlbus-suspend-route' examples/controlbus/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: http-health-route' examples/http-server/src/main.rs` → 1.
- `rg -F -c 'ANCHOR: hot-reload-route' examples/hot-reload/routes/route.yaml` → 1.
- Every opening anchor has a matching closer: `rg -F -c 'ANCHOR_END' examples/content-based-routing/src/main.rs examples/circuit-breaker/src/main.rs examples/aggregator/src/main.rs examples/file-pipeline/src/main.rs examples/custom-component-bundle/src/main.rs examples/health-demo/src/main.rs examples/controlbus/src/main.rs examples/http-server/src/main.rs examples/hot-reload/routes/route.yaml` → each file returns its expected closer count (the Rust files with two anchors return 2; the rest return 1).
- The YAML route still parses (the `# ANCHOR` comments are valid YAML): `nix shell nixpkgs#yq -c yq eval '.' examples/hot-reload/routes/route.yaml > /dev/null` exits 0.
- Each modified Rust example still compiles: `cargo check` in each modified example directory succeeds (resolve the package name from each `examples/<name>/Cargo.toml`). The controlbus check may emit an ADR-0034 deprecation note but must still compile.

**Acceptance:**
- All 11 anchor pairs present (10 Rust + 1 YAML) — each opening `ANCHOR:` and matching `ANCHOR_END` check returns its expected count.
- `cargo check` succeeds for every modified Rust example (comments are inert).

- [x] 2.1

## SUMMARY wiring

### Task 2.2: Wire all foundation pages as nested SUMMARY entries

**Files:**
- `docs/src/SUMMARY.md` (modified)

**Steps:**
1. Under `- [EIP patterns](eip/index.md)`, add three nested sub-page entries (2-space indent):
   ```
     - [Message Filter](eip/filter.md)
     - [Circuit breaker](eip/circuit-breaker.md)
     - [Aggregator](eip/aggregator.md)
   ```
2. Under `- [Components](components/index.md)`, add three nested entries:
   ```
     - [Timer & log](components/timer-log.md)
     - [File](components/file.md)
     - [HTTP](components/http.md)
   ```
3. Under `- [YAML DSL](yaml-dsl/index.md)`, add one nested entry:
   ```
     - [Route structure](yaml-dsl/route-structure.md)
   ```
4. Under `- [Operations](operations/index.md)`, add one nested entry:
   ```
     - [Health](operations/health.md)
   ```
5. Under `- [Extending rust-camel](extending/index.md)`, add one nested entry:
   ```
     - [Custom component](extending/custom-component.md)
   ```
6. Architecture gets no sub-page entries (index-only this change).

**Tests:** (verification — shell)
- Each of the 9 foundation paths appears exactly once: `rg -F -c 'eip/filter.md' docs/src/SUMMARY.md` → 1; repeat for `eip/circuit-breaker.md`, `eip/aggregator.md`, `components/timer-log.md`, `components/file.md`, `components/http.md`, `yaml-dsl/route-structure.md`, `operations/health.md`, `extending/custom-component.md` — each returns 1.

**Acceptance:**
- All 9 `rg -F -c` checks return 1.

- [x] 2.2

## EIP patterns section

### Task 2.3: Author eip/index.md (hub) + filter.md, circuit-breaker.md, aggregator.md

**Files:**
- `docs/src/eip/index.md` (modified — rewrite stub as navigation hub)
- `docs/src/eip/filter.md` (new)
- `docs/src/eip/circuit-breaker.md` (new)
- `docs/src/eip/aggregator.md` (new)

**Steps:**
1. Rewrite `docs/src/eip/index.md` as a navigation hub: one paragraph framing EIPs as the routing/transformation/resilience/messaging vocabulary rust-camel implements as Tower middleware, then categorized links to the three pattern pages below and to runnable examples (`examples/aggregator`, `examples/circuit-breaker`, `examples/content-based-routing`). No `{{#include}}`, no inline code fence, no ADR paraphrase. STE-clean (no em-dash, no banned slop words).
2. Author `docs/src/eip/filter.md`: explain the Message Filter EIP. Include `{{#include ../../../examples/content-based-routing/src/main.rs:filter-route}}` inside a ```` ```rust,ignore ```` fence. Explain that the `.filter` predicate passes the exchange forward only when it holds; `.end_filter()` closes the filter scope. Title: "Message Filter". Link back to `../concepts/routes-pipelines.md` for route structure.
3. Author `docs/src/eip/circuit-breaker.md`: explain the Circuit Breaker resilience pattern. Include `{{#include ../../../examples/circuit-breaker/src/main.rs:circuit-breaker-route}}`. Cite ADR-0019 (CircuitBreaker compiles into a gate on `RouteChannelService` that wraps the pipeline; it is not a Pipeline Step) with a one-sentence paraphrase. Explain the `CircuitBreakerConfig::new()` builder and its `failure_threshold` and `open_duration` methods shown in the include.
4. Author `docs/src/eip/aggregator.md`: explain the Aggregator EIP. Include `{{#include ../../../examples/aggregator/src/main.rs:aggregator-route}}`. Explain `AggregatorConfig::correlate_by("orderId").complete_when_size(3)` correlation semantics (exchanges with the same correlation key accumulate until the size threshold completes the batch).

**Tests:** (verification — linters from prior change)
- Each of the 3 foundation pages has an include: `rg -F -c '{{#include' docs/src/eip/filter.md docs/src/eip/circuit-breaker.md docs/src/eip/aggregator.md` → each ≥ 1.
- The index hub has NO include: `rg -F -c '{{#include' docs/src/eip/index.md` → 0.
- `cargo xtask lint-slop --deny docs/src/eip/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/eip/` → 0 violations.
- Each foundation page cites at least one ADR or explains a pattern without an ADR claim (filter.md may have no ADR — that is fine; circuit-breaker.md cites ADR-0019).

**Acceptance:**
- All verification tests pass.

- [x] 2.3

## Components section

### Task 2.4: Author components/index.md (catalog) + timer-log.md, file.md, http.md

**Files:**
- `docs/src/components/index.md` (modified — rewrite stub as catalog table)
- `docs/src/components/timer-log.md` (new)
- `docs/src/components/file.md` (new)
- `docs/src/components/http.md` (new)

**Steps:**
1. Rewrite `docs/src/components/index.md` as a catalog TABLE. Columns: URI scheme | direction (source/sink/both) | crate | authority (link to the component's local `CONTEXT.md` or nearest parent `../../../crates/components/CONTEXT.md` per the coverage policy). Discover every component by running `find crates/components -name Cargo.toml` from the worktree root (this finds ALL component crates, including thin ones without a local `CONTEXT.md`); for each, map to its nearest `CONTEXT.md` (local if present, else `components/CONTEXT.md`) and determine direction (source/sink/both) from that authority. The markdown LINKS in the page use `../../../crates/...` depth (page is at `docs/src/components/`). No `{{#include}}` (the hub routes). STE-clean.
2. Author `docs/src/components/timer-log.md`: explain timer (source, fires exchanges on a schedule) and log (sink, prints exchange state). Include `{{#include ../../../examples/hello-world/src/main.rs:first-route}}` (already shows `register_component(TimerComponent::new())`, `register_component(LogComponent::new())`, `timer:tick?period=1000&repeatCount=5`, `log:info?showHeaders=true`). Explain the URI query parameters shown. Do not cite ADR-0015 unless the page makes a specific PollingConsumer claim about the timer.
3. Author `docs/src/components/file.md`: explain the file component (polls a directory as source, writes as sink). Include `{{#include ../../../examples/file-pipeline/src/main.rs:file-pipeline-route}}`. Explain `file:{input}?delete=true` (consume + delete) and `file:{output}?fileExist=Override`. Link `../../../crates/components/CONTEXT.md`.
4. Author `docs/src/components/http.md`: explain the HTTP component (server consumer + client producer). Include `{{#include ../../../examples/http-server/src/main.rs:http-health-route}}`. Explain `http://0.0.0.0:8080/health` as a consumer endpoint. Claim ONLY what the included lines show (do not assert `maxResponseBody` or `maxInflightRequests` unless they appear in the anchored region).

**Tests:**
- Each of the 3 foundation pages has an include: `rg -F -c '{{#include' docs/src/components/timer-log.md docs/src/components/file.md docs/src/components/http.md` → each ≥ 1.
- The index hub has NO include: `rg -F -c '{{#include' docs/src/components/index.md` → 0.
- The catalog links at least one CONTEXT.md: `rg -F -c 'CONTEXT.md' docs/src/components/index.md` → ≥ 1.
- `cargo xtask lint-slop --deny docs/src/components/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/components/` → 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 2.4

## YAML DSL section

### Task 2.5: Author yaml-dsl/index.md (hub) + route-structure.md

**Files:**
- `docs/src/yaml-dsl/index.md` (modified — rewrite stub as navigation hub)
- `docs/src/yaml-dsl/route-structure.md` (new)

**Steps:**
1. Rewrite `docs/src/yaml-dsl/index.md` as a pure navigation hub: one paragraph framing the YAML DSL as the declarative route format (alternative to the Rust builder), then links to `route-structure.md` and to the schema at `../../../schemas/dsl/route-schema.json`. No `{{#include}}`, no inline code fence. STE-clean.
2. Author `docs/src/yaml-dsl/route-structure.md`: explain the `routes:` / `id` / `from` / `steps` / `to` structure. Include `{{#include ../../../examples/config-basic/routes/hello.yaml:first-route}}` inside a ```` ```yaml ```` fence. Also include `{{#include ../../../examples/hot-reload/routes/route.yaml:hot-reload-route}}` showing the list-form variant. Cite `../../../schemas/dsl/route-schema.json` as the authoritative schema and explain that `steps` is an ordered list of step verbs (`to`, `log`, `process`, `filter`, etc.).

**Tests:**
- `rg -F -c '{{#include' docs/src/yaml-dsl/route-structure.md` → ≥ 1 (expect 2: hello.yaml + hot-reload).
- `rg -F -c '{{#include' docs/src/yaml-dsl/index.md` → 0.
- `cargo xtask lint-slop --deny docs/src/yaml-dsl/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/yaml-dsl/` → 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 2.5

## Operations section

### Task 2.6: Author operations/index.md (hub) + health.md

**Files:**
- `docs/src/operations/index.md` (modified — rewrite stub as navigation hub)
- `docs/src/operations/health.md` (new)

**Steps:**
1. Rewrite `docs/src/operations/index.md` as a navigation hub: one paragraph framing operations (health endpoints, metrics, route lifecycle), then links to `health.md`, to runnable examples (`examples/health-demo`), and a note that route-lifecycle via ControlBus is pending a stale-example fix (ADR-0034). No `{{#include}}`. STE-clean.
2. Author `docs/src/operations/health.md`: explain the health subsystem. Include `{{#include ../../../examples/health-demo/src/main.rs:health-config}}` (the `ObservabilityConfig.health` wiring) and `{{#include ../../../examples/health-demo/src/main.rs:health-route}}` (the route). Explain the `/readyz` and `/healthz` endpoints and the Degraded vs Unhealthy distinction (link `../concepts/glossary.md`).

**Tests:**
- `rg -F -c '{{#include' docs/src/operations/health.md` → ≥ 1 (expect 2: health-config + health-route).
- `rg -F -c '{{#include' docs/src/operations/index.md` → 0.
- `cargo xtask lint-slop --deny docs/src/operations/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/operations/` → 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 2.6

## Extending section

### Task 2.7: Author extending/index.md (hub) + custom-component.md

**Files:**
- `docs/src/extending/index.md` (modified — rewrite stub)
- `docs/src/extending/custom-component.md` (new)

**Steps:**
1. Rewrite `docs/src/extending/index.md` as a navigation hub: one paragraph framing extension points (custom components via `ComponentBundle`, custom languages), then a link to `custom-component.md` and to `examples/custom-component-bundle`. No `{{#include}}`. STE-clean.
2. Author `docs/src/extending/custom-component.md`: explain building a custom component as a `ComponentBundle`. Include `{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-bundle-impl}}` (the `EchoBundle` struct + `impl ComponentBundle` with `config_key`/`from_toml`/`register_all`/`register_component_dyn`) and `{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-bundle-register}}` (the registration in main). Link `../../../crates/components/camel-component-api/CONTEXT.md` as the component-api authority.

**Tests:**
- `rg -F -c '{{#include' docs/src/extending/custom-component.md` → ≥ 1 (expect 2: echo-bundle-impl + echo-bundle-register).
- `rg -F -c '{{#include' docs/src/extending/index.md` → 0.
- `cargo xtask lint-slop --deny docs/src/extending/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/extending/` → 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 2.7

## Architecture section

### Task 2.8: Author architecture/index.md (crate map)

**Files:**
- `docs/src/architecture/index.md` (modified — rewrite stub as crate map)

**Steps:**
1. Rewrite `docs/src/architecture/index.md` as a crate map. One paragraph framing the crate structure (contract crates, runtime, processors, DSL, components, languages, services, platforms). Then a TABLE: columns Crate | Role (one line) | Authority (link to local `CONTEXT.md` or nearest parent per the coverage policy). Discover every crate by running `find crates -name Cargo.toml` from the worktree root; for each crate, map to its nearest `CONTEXT.md` (local if present, else the nearest parent directory's `CONTEXT.md` per the CONTEXT-MAP coverage policy). The markdown LINKS in the page use `../../../crates/<crate>/CONTEXT.md` depth (page is at `docs/src/architecture/`).
2. Add a pointer to the ADR index (`../../adr/`) and a link back to `../concepts/planes.md` for the data-plane/control-plane split.
3. Do NOT write a code fence asserting `Service<Exchange>` as current form. If the trait is mentioned, cite ADR-0001 as rationale, not as a form-claim.

**Tests:**
- The index links at least 10 CONTEXT.md files: `rg -F -c 'CONTEXT.md' docs/src/architecture/index.md` → ≥ 10.
- The index has NO include (it is a hub): `rg -F -c '{{#include' docs/src/architecture/index.md` → 0.
- The index links the ADR directory: `rg -F -c 'adr/' docs/src/architecture/index.md` → ≥ 1.
- The index links concepts/planes: `rg -F -c 'concepts/planes.md' docs/src/architecture/index.md` → ≥ 1.
- No `Service<Exchange>` code fence: `rg -F -c 'Service<Exchange>' docs/src/architecture/index.md` → 0.
- `cargo xtask lint-slop --deny docs/src/architecture/` → 0 violations.
- `cargo xtask lint-adr-cite --deny docs/src/architecture/` → 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 2.8

## Integration verification

### Task 2.9: Verify full guide builds and all links resolve

**Files:** (none — verification only)

**Steps:**
1. Run `nix shell nixpkgs#mdbook -c mdbook build docs` and confirm exit 0 with no `broken link` or `missing file` warnings.
2. Run the link-existence check and confirm empty output (zero DANGLING lines):
   ```bash
   find docs/src -name '*.md' -print0 | while IFS= read -r -d '' f; do
     dir=$(dirname "$f")
     rg -No '\]\(([^)]+\.md)\)' -r '$1' "$f" 2>/dev/null | while IFS= read -r target; do
       case "$target" in http* | /*) continue ;; esac
       [ -f "$dir/$target" ] || echo "DANGLING: $f -> $target"
     done
   done
   ```
3. Run both linters across every new/modified page: `cargo xtask lint-slop --deny docs/src/eip/ docs/src/components/ docs/src/yaml-dsl/ docs/src/operations/ docs/src/extending/ docs/src/architecture/` and `cargo xtask lint-adr-cite --deny` on the same set. (lint-glossary is not applicable — these pages introduce no glossary terms.) Confirm 0 violations each.

**Tests:**
- mdbook build exit 0, no broken-link warnings.
- link-check output empty.
- both linters 0 violations.

**Acceptance:**
- All three checks pass.

- [x] 2.9
