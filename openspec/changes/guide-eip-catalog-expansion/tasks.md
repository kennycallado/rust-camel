# Tasks: guide-eip-catalog-expansion

## Examples

### Task 3.1: Create content-based-router example

**Depends on:** (none)

**Files:**
- `examples/content-based-router/Cargo.toml` (new)
- `examples/content-based-router/src/main.rs` (new)

**Steps:**
1. Create `examples/content-based-router/Cargo.toml` based on `examples/content-based-routing/Cargo.toml`. Set package name to `content-based-router`. Keep the same dependencies.
2. Create `examples/content-based-router/src/main.rs`. Use a timer source (`timer:tick?period=1000&repeatCount=6`) with a process step that sets the exchange body to `"high"`, `"medium"`, or `"low"` based on a counter. Then use the `choice` step with two `when` branches (body == `"high"` routes to `log:high-priority`, body == `"medium"` routes to `log:medium-priority`) and one `otherwise` branch (routes to `log:low-priority`). Add `// ANCHOR: cbr-route` immediately before the `let route = RouteBuilder::from(` line and `// ANCHOR_END: cbr-route` immediately after the `.build()?;` line that closes the route.
3. The workspace `members` glob `"examples/*"` in root `Cargo.toml` auto-includes the new directory — do NOT add an explicit member entry.
4. Verify: `cargo build -p content-based-router` exits 0.

**Tests:**
- `choice-example-compiles`: clean target → `cargo build -p content-based-router` → exit code 0
- `choice-step-present`: read `src/main.rs` → grep for `.choice(` → at least one match
- `two-when-branches`: read `src/main.rs` → grep for `.when(` → at least two matches
- `otherwise-branch`: read `src/main.rs` → grep for `.otherwise(` → at least one match
- `anchor-present`: read `src/main.rs` → grep for `ANCHOR: cbr-route` → exactly one match, grep for `ANCHOR_END: cbr-route` → exactly one match

**Acceptance:**
- `cargo build -p content-based-router` exits 0
- `cargo clippy -p content-based-router -- -D warnings` exits 0
- The route uses `.choice(` with at least two `.when(` branches and one `.otherwise(`

- [x] 3.1

### Task 3.2: Add anchor comment pairs to 14 existing examples

**Depends on:** (none)

**Files (all comment-only modifications):**
- `examples/dynamic-router/src/main.rs` (modified)
- `examples/recipientlist/src/main.rs` (modified)
- `examples/routing-slip/src/main.rs` (modified)
- `examples/wiretap/src/main.rs` (modified)
- `examples/multicast/src/main.rs` (modified)
- `examples/load-balancer/src/main.rs` (modified)
- `examples/convert-body-to/src/main.rs` (modified)
- `examples/marshal-csv/src/main.rs` (modified)
- `examples/marshal-unmarshal/src/main.rs` (modified)
- `examples/file-pollenrich/src/main.rs` (modified)
- `examples/splitter/src/main.rs` (modified)
- `examples/streaming-split/src/main.rs` (modified)
- `examples/do-try/src/main.rs` (modified)
- `examples/throttler/src/main.rs` (modified)

**Steps:**
For each file, insert `// ANCHOR:` and `// ANCHOR_END:` comment lines (with the specific anchor ID between the colon and the closing backtick) around the specified code section. Comments only — do NOT modify executable code.

1. `dynamic-router/src/main.rs` — anchor `dynamic-router-route` around lines 36–63: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
2. `recipientlist/src/main.rs` — anchor `recipient-list-route` around lines 49–60: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
3. `routing-slip/src/main.rs` — anchor `routing-slip-route` around lines 30–54: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
4. `wiretap/src/main.rs` — anchor `wire-tap-route` around lines 28–34: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
5. `multicast/src/main.rs` — anchor `multicast-route` around lines 32–62: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
6. `load-balancer/src/main.rs` — anchor `load-balancer-route` around lines 27–42: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
7. `convert-body-to/src/main.rs` — anchor `convert-body-route` around lines 38–52: the FIRST route block (route_id `"convert-json-demo"`), from `let route = RouteBuilder::from` through `.build()?;`. This route contains `.convert_body_to(BodyType::Json)` at line 45.
8. `marshal-csv/src/main.rs` — anchor `marshal-route` around lines 47–56: the SECOND route block (route_id `"marshal-csv-demo"`), which contains `.unmarshal("json")?` and `.marshal("csv")?`.
9. `marshal-unmarshal/src/main.rs` — anchor `unmarshal-route` around lines 39–55: the FIRST route block (route_id `"marshal-json-demo"`), which contains `.unmarshal("json")?` and `.marshal("json")?`.
10. `file-pollenrich/src/main.rs` — anchor `poll-enrich-route` around lines 34–42: the block starting with `let route = RouteBuilder::from` through `.build()?;`. This route contains `.poll_enrich(` at line 36.
11. `splitter/src/main.rs` — anchor `splitter-route` around lines 29–63: the block starting with `let route = RouteBuilder::from` through `.build()?;`.
12. `streaming-split/src/main.rs` — anchor `streaming-split-route` around lines 92–125: the section inside `async fn main()` that creates the `StreamingSplitterService`, builds the exchange, and calls `splitter.call(exchange).await`. This example uses the service directly (no `RouteBuilder`). Place the anchor before the `let mut splitter = StreamingSplitterService::new(` line and after the line that prints the final result.
13. `do-try/src/main.rs` — anchor `do-try-route` around lines 50–59: the FIRST route block (route_id `"do-try-catch"`), which contains `.do_try()`.
14. `throttler/src/main.rs` — anchor `throttler-route` around lines 29–36: the block starting with `let route = RouteBuilder::from` through `.build()?;`. This route contains `.throttle(2, Duration::from_secs(1))` at line 33.

**Tests:**
- `anchor-present-dynamic-router`: grep `examples/dynamic-router/src/main.rs` for `ANCHOR: dynamic-router-route` → one match, `ANCHOR_END: dynamic-router-route` → one match
- `anchor-present-recipientlist`: grep `examples/recipientlist/src/main.rs` for `ANCHOR: recipient-list-route` → one match each open/close
- `anchor-present-routing-slip`: grep for `ANCHOR: routing-slip-route` → one match each
- `anchor-present-wiretap`: grep for `ANCHOR: wire-tap-route` → one match each
- `anchor-present-multicast`: grep for `ANCHOR: multicast-route` → one match each
- `anchor-present-load-balancer`: grep for `ANCHOR: load-balancer-route` → one match each
- `anchor-present-convert-body`: grep for `ANCHOR: convert-body-route` → one match each
- `anchor-present-marshal-csv`: grep for `ANCHOR: marshal-route` → one match each
- `anchor-present-marshal-unmarshal`: grep for `ANCHOR: unmarshal-route` → one match each
- `anchor-present-poll-enrich`: grep for `ANCHOR: poll-enrich-route` → one match each
- `anchor-present-splitter`: grep for `ANCHOR: splitter-route` → one match each
- `anchor-present-streaming-split`: grep for `ANCHOR: streaming-split-route` → one match each
- `anchor-present-do-try`: grep for `ANCHOR: do-try-route` → one match each
- `anchor-present-throttler`: grep for `ANCHOR: throttler-route` → one match each
- `all-examples-compile`: run `cargo build -p dynamic-router -p recipientlist -p routing-slip -p wiretap -p multicast -p load-balancer -p convert-body-to -p marshal-csv -p marshal-unmarshal -p file-pollenrich -p splitter -p streaming-split -p do-try -p throttler` → exit code 0

**Acceptance:**
- All 14 anchor pairs present (one open, one close per file)
- `cargo build` of all 14 examples exits 0
- No executable code changed: `git diff` shows only lines starting with `//` added or context lines

- [x] 3.2

## Documentation

### Task 3.3: Write 7 routing family EIP pages

**Depends on:** 3.1, 3.2

**Files (all new):**
- `docs/src/eip/content-based-router.md`
- `docs/src/eip/dynamic-router.md`
- `docs/src/eip/recipient-list.md`
- `docs/src/eip/routing-slip.md`
- `docs/src/eip/wire-tap.md`
- `docs/src/eip/multicast.md`
- `docs/src/eip/load-balancer.md`

**Steps:**
1. For each page, write a 40-100 line markdown file with this structure:
   - A level-1 heading with the EIP name (e.g., `# Content-Based Router`)
   - One sentence naming the EIP and its Hohpe/Woolf category (e.g., "Content-Based Router — a Message Routing pattern")
   - The `{{#include}}` directive pulling the route code (exact directives listed in step 2 below)
   - 2-4 paragraphs explaining: what the pattern does in rust-camel, how the route builder composes it as a Tower middleware step (ADR-0001), and key configuration parameters
   - Link to the `crates/camel-processor/CONTEXT.md` authority (relative: `../../../crates/camel-processor/CONTEXT.md`)
   - Cite ADR-0001 for the composable-step model on every page
   - Cite ADR-0025 (outcome-aware structural EIPs) on `multicast.md` and `recipient-list.md` where partial outcomes matter
   - Example link with the example directory name substituted into `https://github.com/kennycallado/rust-camel/tree/main/examples/`

2. Use these exact include directives:
   - `content-based-router.md` → `{{#include ../../../examples/content-based-router/src/main.rs:cbr-route}}`
   - `dynamic-router.md` → `{{#include ../../../examples/dynamic-router/src/main.rs:dynamic-router-route}}`
   - `recipient-list.md` → `{{#include ../../../examples/recipientlist/src/main.rs:recipient-list-route}}`
   - `routing-slip.md` → `{{#include ../../../examples/routing-slip/src/main.rs:routing-slip-route}}`
   - `wire-tap.md` → `{{#include ../../../examples/wiretap/src/main.rs:wire-tap-route}}`
   - `multicast.md` → `{{#include ../../../examples/multicast/src/main.rs:multicast-route}}`
   - `load-balancer.md` → `{{#include ../../../examples/load-balancer/src/main.rs:load-balancer-route}}`

**Tests:**
- `lint-slop-routing`: run `cargo run -p xtask -- lint-slop --deny docs/src/eip/content-based-router.md docs/src/eip/dynamic-router.md docs/src/eip/recipient-list.md docs/src/eip/routing-slip.md docs/src/eip/wire-tap.md docs/src/eip/multicast.md docs/src/eip/load-balancer.md` → zero violations
- `lint-adr-cite-routing`: run `cargo run -p xtask -- lint-adr-cite --deny` on the same files → zero violations
- `line-count-routing`: `wc -l` on each file → each between 40 and 100 lines
- `multicast-cites-adr-0025`: grep `multicast.md` for `ADR-0025` → at least one match
- `recipient-list-cites-adr-0025`: grep `recipient-list.md` for `ADR-0025` → at least one match

**Acceptance:**
- All 7 files exist
- `lint-slop` zero violations
- `lint-adr-cite` zero violations
- Each page has exactly one `{{#include}}` directive
- `multicast.md` and `recipient-list.md` cite ADR-0025

- [x] 3.3

### Task 3.4: Write 3 transformation family EIP pages

**Depends on:** 3.2

**Files (all new):**
- `docs/src/eip/convert-body.md`
- `docs/src/eip/marshal-unmarshal.md`
- `docs/src/eip/poll-enrich.md`

**Steps:**
1. `convert-body.md` — include from `{{#include ../../../examples/convert-body-to/src/main.rs:convert-body-route}}`. Explain how the convert body step transforms the exchange body type (`BodyType::Json`, `BodyType::Bytes`, `BodyType::Text`) in the pipeline. Cite ADR-0001 for the step model. Link `crates/camel-processor/CONTEXT.md` for the convert body contract.
2. `marshal-unmarshal.md` — include from BOTH `{{#include ../../../examples/marshal-csv/src/main.rs:marshal-route}}` AND `{{#include ../../../examples/marshal-unmarshal/src/main.rs:unmarshal-route}}`. Explain serialization format selection (the string parameter like `"json"`, `"csv"`, `"xml"`) and the bidirectional marshal and unmarshal operations. Cite ADR-0001. Link `crates/camel-processor/CONTEXT.md`.
3. `poll-enrich.md` — include from `{{#include ../../../examples/file-pollenrich/src/main.rs:poll-enrich-route}}`. Explain how the consumer polls a resource and the default `UseEnrichedBody` strategy replaces the exchange body with the polled result (not merges). Cite ADR-0001. Link `crates/camel-processor/CONTEXT.md` for the `EnrichmentStrategy` trait and available strategies (`UseEnrichedBody`, `ThrowOnNoPoll`).
4. Each page has: a level-1 heading with the EIP name, one sentence naming the EIP and Hohpe/Woolf category, the include block(s), 2-4 paragraphs of prose, a link to `crates/camel-processor/CONTEXT.md`, ADR-0001 citation, and an example source link.

**Tests:**
- `lint-slop-transformation`: run `cargo run -p xtask -- lint-slop --deny docs/src/eip/convert-body.md docs/src/eip/marshal-unmarshal.md docs/src/eip/poll-enrich.md` → zero violations
- `lint-adr-cite-transformation`: run `cargo run -p xtask -- lint-adr-cite --deny docs/src/eip/convert-body.md docs/src/eip/marshal-unmarshal.md docs/src/eip/poll-enrich.md` → zero violations
- `marshal-page-has-two-includes`: grep `marshal-unmarshal.md` for `{{#include` → exactly 2 matches
- `poll-enrich-mentions-strategy`: grep `poll-enrich.md` for `UseEnrichedBody` → at least one match
- `line-count-transformation`: `wc -l` on each file → each between 40 and 100 lines

**Acceptance:**
- All 3 files exist
- `lint-slop` zero violations
- `marshal-unmarshal.md` has exactly 2 `{{#include}}` directives
- `poll-enrich.md` mentions `UseEnrichedBody`

- [x] 3.4

### Task 3.5: Write 4 messaging and resilience EIP pages

**Depends on:** 3.2

**Files (all new):**
- `docs/src/eip/splitter.md`
- `docs/src/eip/streaming-splitter.md`
- `docs/src/eip/do-try.md`
- `docs/src/eip/throttler.md`

**Steps:**
1. `splitter.md` — include from `{{#include ../../../examples/splitter/src/main.rs:splitter-route}}`. Explain how the splitter breaks a composite message into individual exchanges using a split expression. Cite ADR-0001 for the step model and ADR-0025 (outcome-aware structural EIPs) for structural segment behavior where the splitter produces multiple output exchanges. Link `crates/camel-processor/CONTEXT.md`.
2. `streaming-splitter.md` — include from `{{#include ../../../examples/streaming-split/src/main.rs:streaming-split-route}}`. Cite ADR-0025 for structural segment behavior. Source backpressure semantics from `crates/camel-processor/CONTEXT.md`. Do NOT cite ADR-0011 — it governs the route-spec contract, not backpressure.
3. `do-try.md` — include from `{{#include ../../../examples/do-try/src/main.rs:do-try-route}}`. Explain the do-try block as a scoped error-handling construct with do-catch and do-finally branches. Cite ADR-0001. Link `crates/camel-processor/CONTEXT.md`.
4. `throttler.md` — include from `{{#include ../../../examples/throttler/src/main.rs:throttler-route}}`. Explain the rate-limiting semantics: the throttle step accepts a maximum number of requests per time window (in the example, 2 per second). Cite ADR-0001. Link `crates/camel-processor/CONTEXT.md`.
5. Each page has: a level-1 heading with the EIP name, one sentence naming the EIP and Hohpe/Woolf category, the include directive, 2-4 paragraphs of prose explaining the pattern and the Tower step model, a link to `crates/camel-processor/CONTEXT.md`, ADR-0001 citation, and an example source link.

**Tests:**
- `lint-slop-messaging-resilience`: run `cargo run -p xtask -- lint-slop --deny docs/src/eip/splitter.md docs/src/eip/streaming-splitter.md docs/src/eip/do-try.md docs/src/eip/throttler.md` → zero violations
- `lint-adr-cite-messaging-resilience`: run `cargo run -p xtask -- lint-adr-cite --deny docs/src/eip/splitter.md docs/src/eip/streaming-splitter.md docs/src/eip/do-try.md docs/src/eip/throttler.md` → zero violations
- `splitter-cites-adr-0025`: grep `splitter.md` for `ADR-0025` → at least one match
- `streaming-splitter-cites-adr-0025`: grep `streaming-splitter.md` for `ADR-0025` → at least one match
- `streaming-splitter-no-adr-0011`: grep `streaming-splitter.md` for `ADR-0011` → zero matches
- `throttler-mentions-rate-limit`: grep `throttler.md` for `rate\|throttle\|per` → at least one match
- `line-count-messaging-resilience`: `wc -l` on each file → each between 40 and 100 lines

**Acceptance:**
- All 4 files exist
- `lint-slop` zero violations
- `splitter.md` and `streaming-splitter.md` cite ADR-0025
- `streaming-splitter.md` does NOT cite ADR-0011

- [x] 3.5

### Task 3.6: Rewrite EIP hub and wire SUMMARY

**Depends on:** 3.3, 3.4, 3.5

**Files:**
- `docs/src/eip/index.md` (modified)
- `docs/src/SUMMARY.md` (modified)

**Steps:**
1. Rewrite `docs/src/eip/index.md` with four `##` family headings plus a deferred section. Each family lists its pages with one-line descriptions and relative links:

   **`## Routing`** (8 pages):
   - [Message Filter](filter.md) — pass or drop an exchange by predicate (existing)
   - [Content-Based Router](content-based-router.md) — route exchanges to different destinations by predicate
   - [Dynamic Router](dynamic-router.md) — compute the destination at runtime from exchange content
   - [Recipient List](recipient-list.md) — broadcast an exchange to a list of endpoints computed at runtime
   - [Routing Slip](routing-slip.md) — attach a sequence of endpoints and route through each in order
   - [Wire Tap](wire-tap.md) — send a copy of the exchange to a tap endpoint without blocking the main flow
   - [Multicast](multicast.md) — send the exchange to multiple destinations in parallel
   - [Load Balancer](load-balancer.md) — distribute exchanges across destination endpoints in round-robin

   **`## Transformation`** (3 pages):
   - [Convert Body](convert-body.md) — transform the exchange body type in the pipeline
   - [Marshal and Unmarshal](marshal-unmarshal.md) — serialize and deserialize the body to/from a format
   - [Poll Enrich](poll-enrich.md) — poll a resource and replace the body with the result

   **`## Messaging`** (3 pages):
   - [Aggregator](aggregator.md) — accumulate exchanges by correlation key and emit a batch (existing)
   - [Splitter](splitter.md) — break a composite message into individual exchanges
   - [Streaming Splitter](streaming-splitter.md) — split a stream body incrementally with backpressure

   **`## Resilience and control`** (3 pages):
   - [Circuit breaker](circuit-breaker.md) — trip after repeated failures and recover after a cool-down (existing)
   - [Do Try](do-try.md) — scoped error handling with catch and finally branches
   - [Throttler](throttler.md) — limit the rate of exchanges through a route

   **`## Deferred patterns`** (10 entries, each with the pattern name bolded, one-line description, NO links):
   - **Zip Splitter** — split a ZIP archive into individual file exchanges (has example, deferred for scope)
   - **Delayer** — delay exchange delivery by a fixed duration (has example, deferred for scope)
   - **Loop** — repeat a sub-route a fixed number of times (has example, deferred for scope)
   - **Validator** — validate exchange content against a schema (has example, deferred for scope)
   - **Idempotent Consumer** — reject duplicate exchanges by correlation key (deferred: no runnable example)
   - **Content Enricher** — enrich the exchange body from an external resource (deferred: no compiled `.enrich()` example)
   - **Claim Check** — store and retrieve large payloads by claim ticket (deferred: no example)
   - **Sort** — reorder exchanges by a comparator (deferred: no example)
   - **Sampling** — pass through a fraction of exchanges (deferred: no example)
   - **Resequencer** — reorder exchanges by sequence number (deferred: no example)

2. Keep the intro paragraph (Enterprise Integration Patterns description and the Tower middleware pipeline model).

3. Update `docs/src/SUMMARY.md`: replace the existing 3-entry EIP list with all 17 entries in this order (matching the hub family order):
   ```
   - [Message Filter](eip/filter.md)
   - [Content-Based Router](eip/content-based-router.md)
   - [Dynamic Router](eip/dynamic-router.md)
   - [Recipient List](eip/recipient-list.md)
   - [Routing Slip](eip/routing-slip.md)
   - [Wire Tap](eip/wire-tap.md)
   - [Multicast](eip/multicast.md)
   - [Load Balancer](eip/load-balancer.md)
   - [Convert Body](eip/convert-body.md)
   - [Marshal and Unmarshal](eip/marshal-unmarshal.md)
   - [Poll Enrich](eip/poll-enrich.md)
   - [Aggregator](eip/aggregator.md)
   - [Splitter](eip/splitter.md)
   - [Streaming Splitter](eip/streaming-splitter.md)
   - [Circuit breaker](eip/circuit-breaker.md)
   - [Do Try](eip/do-try.md)
   - [Throttler](eip/throttler.md)
   ```

**Tests:**
- `hub-has-five-sections`: read `eip/index.md` → grep for `^## ` → exactly 5 matches (Routing, Transformation, Messaging, Resilience and control, Deferred patterns)
- `summary-has-17-eip-entries`: read `SUMMARY.md` → count lines matching `eip/` → exactly 17
- `deferred-has-10-patterns`: read `eip/index.md` → in Deferred section → count bold pattern names → exactly 10
- `deferred-has-no-links`: read `eip/index.md` → in Deferred section → grep for `.md)` → zero matches (deferred entries have no links)

**Acceptance:**
- `eip/index.md` has 5 section headings
- `SUMMARY.md` EIP section has 17 entries in the specified order
- Deferred section lists exactly 10 patterns with no links

- [x] 3.6

## Verification

### Task 3.7: Build and verify the complete guide

**Depends on:** 3.1, 3.2, 3.3, 3.4, 3.5, 3.6

**Files:**
- (no new files — verification only)

**Steps:**
1. Run `nix shell nixpkgs#mdbook -c mdbook build docs 2>&1`. Capture combined stdout+stderr. Verify exit code 0. Grep the captured output for `broken link`, `not found`, `failed to resolve` — verify zero matches.
2. Verify relative Markdown links resolve: run `find docs/src -name '*.md' | while read f; do dir=$(dirname "$f"); grep -oP '\]\(\K[^)#]+\.md' "$f" | while read link; do [ -f "$dir/$link" ] || echo "BROKEN: $f -> $link"; done; done`. Verify zero output (zero broken links).
3. Run `cargo run -p xtask -- lint-slop --deny docs/src/eip/`. Verify zero violations.
4. Run `cargo run -p xtask -- lint-adr-cite --deny docs/src/eip/`. Verify zero violations.
5. Run `cargo build -p content-based-router -p dynamic-router -p recipientlist -p routing-slip -p wiretap -p multicast -p load-balancer -p convert-body-to -p marshal-csv -p marshal-unmarshal -p file-pollenrich -p splitter -p streaming-split -p do-try -p throttler`. Verify exit code 0.

**Tests:**
- `mdbook-build-exit-0`: `nix shell nixpkgs#mdbook -c mdbook build docs` → exit code 0
- `mdbook-output-clean`: capture mdbook output → grep for `broken link\|not found\|failed to resolve` → zero matches
- `relative-links-resolve`: run the `find docs/src` link-check command from step 2 → zero output
- `lint-slop-eip-zero`: `cargo run -p xtask -- lint-slop --deny docs/src/eip/` → zero violations
- `lint-adr-cite-eip-zero`: `cargo run -p xtask -- lint-adr-cite --deny docs/src/eip/` → zero violations
- `all-15-examples-compile`: `cargo build` of all 15 example crates → exit code 0

**Acceptance:**
- `mdbook build docs` exits 0
- mdbook output contains zero broken-link or missing-include warnings
- `lint-slop` zero violations on `docs/src/eip/`
- `lint-adr-cite` zero violations on `docs/src/eip/`
- All 15 examples compile

- [x] 3.7
