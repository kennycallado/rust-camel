# Tasks: aggregator-adr0046-divergences

Single-phase change. All six tasks land documentation in
`crates/camel-processor/CONTEXT.md` under one new top-level section
"Aggregator EIP divergences from Apache Camel (ADR-0046 protocol)".
**Task 1 is a prerequisite for Tasks 2–6**: it creates the section scaffold
(header + preamble + the D-A1 subsection) that Tasks 2–6 append their
subsections to. The conductor dispatches tasks sequentially in numbered order
(1→2→3→4→5→6); do NOT dispatch Tasks 2–6 before Task 1 is committed.
Tests land in the existing inline test modules of the relevant crate.
NO production code change to `aggregator.rs` behavior or `AggregationFn`
signature is permitted — if a task surfaces a real bug, STOP and report it.

Reference for every task: the spike inventory is indexed in the context-mode
KB under source `rc-mybm-spike`; query
`ctx_search(queries:["D-A1 D-A2 D-A3 D-A4 D-A5"], source:"rc-mybm-spike")`
for the classification evidence and forcing ADR/contract per divergence.

## camel-processor

### Task 1: D-A1 — binary-fold strategy contract (no null oldExchange) doc + semantic pin

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-processor/src/aggregator.rs` (modified — new test in existing `#[cfg(test)] mod tests`)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md`, add a new top-level section (level-2 `##`) titled `Aggregator EIP divergences from Apache Camel (ADR-0046 protocol)`. Under it add a one-paragraph preamble stating: this section records divergences surfaced by applying the ADR-0046 protocol to the Aggregator EIP; each divergence names the forcing contract shape or ADR and the observable consequence; the Splitter-specific divergence D2 lives in the separate "Aggregation contract (divergence from Apache Camel)" block above.
2. Under that section, add a level-3 subsection `### D-A1: binary-fold strategy contract — no null oldExchange on first message`. Content: Apache Camel's `AggregationStrategy.aggregate(Exchange oldExchange, Exchange newExchange)` receives `null` for `oldExchange` on the FIRST message of a bucket, letting a strategy initialize. rust-camel's `AggregationFn = Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>` (defined in `crates/camel-api/src/aggregator.rs:8`) ALWAYS receives two exchanges: the first message sits untouched in the bucket, and the strategy is first invoked as `f(ex1, ex2)` when the second message arrives. State the forcing contract shape (`AggregationFn` binary signature + bucket model), and the consequence: a strategy needing initialize-on-first logic must branch on a sentinel in the accumulated body (or check a property) rather than on a null oldExchange.
3. In `crates/camel-processor/src/aggregator.rs` inside the existing `#[cfg(test)] mod tests` block, add the test described below. Use the existing `make_exchange`, `config_size`, `new_test_svc` helpers.

**Tests:**
- `test_da1_strategy_receives_two_exchanges_first_message_preserved`: setup = build an `AggregatorConfig` via `AggregatorConfig::correlate_by("k").complete_when_size(2)` with a custom `AggregationStrategy::Custom(f)` where `f` is an `Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>` that records into a shared `Arc<std::sync::Mutex<Vec<(String, String)>>>` the pair `(old.input.body as string, new.input.body as string)` and returns the `new` exchange; build the service via `new_test_svc` (or `AggregatorService::new` with a throwaway `late_tx`, the default language registry, and a cancellation token). action = `poll_ready` then `call` with `make_exchange("k","1","A")` (bucket not complete, returns pending), then `poll_ready` then `call` with `make_exchange("k","1","B")` (completes). assert = the recorded pair vec has exactly ONE entry `("A", "B")` — proving the strategy was NOT invoked on the first message and WAS invoked with both exchanges present on the second, the first-message body preserved unchanged.

**Acceptance:**
- `rg -n '### D-A1:' crates/camel-processor/CONTEXT.md` returns exactly one match.
- `rg -n 'Aggregator EIP divergences from Apache Camel' crates/camel-processor/CONTEXT.md` returns exactly one match (the new section header).
- `cargo test -p camel-processor --lib test_da1_strategy_receives_two_exchanges_first_message_preserved` passes.
- `cargo fmt --check -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 1

### Task 2: D-A2 — AggregationFn cannot signal failure (no Result) doc + compile-time witness

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-api/src/aggregator.rs` (modified — doctest witness on the `AggregationFn` type alias)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md` under the section created in Task 1, add a level-3 subsection `### D-A2: AggregationFn cannot signal failure — no Result return`. Content: Apache Camel's `AggregationStrategy.aggregate()` may throw, propagating the exception and failing the aggregated exchange. rust-camel's `AggregationFn` returns `Exchange` (not `Result<Exchange, CamelError>`), so a custom strategy has no path to signal invalid aggregation through the return type except by panicking. State the forcing contract shape (the `AggregationFn` alias at `crates/camel-api/src/aggregator.rs:8`), and note this is the D2-family divergence for the Aggregate EIP specifically — distinct from the Splitter EIP's `Vec<Result<Exchange, CamelError>>` shape documented in the "Aggregation contract" block above. State the consequence: error-aware aggregation logic cannot be expressed in the strategy return; it must live outside (e.g. in a downstream doTry/error-handler per ADR-0019) or in a wrapping service.
2. In `crates/camel-api/src/aggregator.rs`, add a `compile_fail` doctest on the `AggregationFn` type alias (the `pub type AggregationFn` definition at line 8). Use EXACTLY this snippet (the crate paths resolve: `camel_api::aggregator::AggregationFn` via lib.rs re-export, `camel_api::Exchange` via lib.rs re-export, `camel_api::CamelError` via lib.rs re-export):

    ```
    /// ```compile_fail
    /// use std::sync::Arc;
    /// use camel_api::aggregator::AggregationFn;
    /// use camel_api::{Exchange, CamelError};
    /// // A strategy that attempts to signal failure via Result does NOT type-check
    /// // as AggregationFn (whose Fn returns Exchange, not Result<Exchange, CamelError>).
    /// let _: AggregationFn = Arc::new(
    ///     |_old: Exchange, _new: Exchange| -> Result<Exchange, CamelError> { unreachable!() }
    /// );
    /// ```
    ```

    The coercion fails because the closure implements `Fn(Exchange, Exchange) -> Result<...>` — a distinct trait from `Fn(Exchange, Exchange) -> Exchange` that `AggregationFn` requires. There is NO positive-doctest fallback; the compile_fail witness is the deliverable.

**Tests:**
- `doctest on AggregationFn` (compile_fail): setup = the doctest block above on `pub type AggregationFn`; action = `cargo test -p camel-api --doc`; assert = the compile_fail block is registered and correctly fails to compile, and the failure references the return-type mismatch (not an import/syntax error). command = `cargo test -p camel-api --doc` (compile_fail doctests pass the doctest harness when they correctly fail to compile). expected = exit 0 with the compile_fail block counted as a passed doctest.

**Acceptance:**
- `rg -n '### D-A2:' crates/camel-processor/CONTEXT.md` returns exactly one match.
- `cargo test -p camel-api --doc` exits 0 (the compile_fail doctest is registered and correctly fails-to-compile).
- `rg -n 'compile_fail' crates/camel-api/src/aggregator.rs` returns the doctest fence.
- `rg -n -- '-> Result<Exchange, CamelError>' crates/camel-api/src/aggregator.rs` returns the witness line inside the doctest.
- `cargo fmt --check -p camel-api` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 2

### Task 3: D-A3 — force-completion-on-stop channel path + drop-under-pressure doc + semantic pin

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-processor/src/aggregator.rs` (modified — augment the existing test module)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md` under the Task 1 section, add a level-3 subsection `### D-A3: force-completion-on-stop channel-mediated emission + drop under pressure`. Content: Apache Camel's `forceCompletionOnStop()` flows pending buckets synchronously through the downstream pipeline during `context.stop()`. rust-camel's `force_complete_all()` (`crates/camel-processor/src/aggregator.rs:166`) is nonblocking (`-> ()`): it cannot return completed exchanges inline, so it emits them through a bounded `late_tx` mpsc channel (capacity 256, see `crates/camel-core/src/lifecycle/adapters/route_controller.rs:776`) that a `select!` arm drains into the post-pipeline. State that the BOOLEAN semantics are equal (`force_completion_on_stop == true` emits pending buckets; `== false` drops them). State the two divergences: (1) channel-mediated async emission vs Camel's synchronous flow; (2) under late-channel-full pressure, `try_send` fails and the force-completed exchange is DROPPED with a `warn!` log (`aggregator.rs:184-189`) — Camel has no equivalent drop path. State the forcing contract shape (nonblocking `force_complete_all() -> ()` + bounded `late_tx`).
2. In `crates/camel-processor/src/aggregator.rs` test module, verify the existing `test_late_channel_full_drops_with_warning` test (around line 1333) covers the timeout-task `late_tx.try_send` drop path. ADD a new test (below it) that covers the `force_complete_all()` drop path specifically (the existing test exercises the timeout-task path, not the force-complete path).

**Tests:**
- `test_da3_force_complete_all_drops_on_saturated_channel`: setup = build an `AggregatorConfig` with `correlate_by("k").complete_when_size(10).force_completion_on_stop(true)` so buckets never complete on size alone; create a `late_tx, late_rx = mpsc::channel::<Exchange>(1)` (capacity 1, deliberately tiny); PRE-saturate the channel by sending one dummy exchange through `late_tx.try_send(make_exchange("k","99","dummy")).expect("pre-fill succeeds")` BEFORE constructing the service — the `mpsc::Sender` is moved into `AggregatorService::new(config, late_tx, registry, cancel)`, so the pre-fill MUST happen before the move so the 1-slot capacity is full at construction time. Then drive `poll_ready` + `call` with 3 exchanges with DIFFERENT correlation keys ("k"=1,2,3) so 3 buckets accumulate (each bucket has 1 exchange, none complete on size; size=10 config). action = call `svc.force_complete_all()`. assert = three EXACT checks: (a) `late_rx.try_recv()` returns `Ok(dummy)` once (the manually-sent pre-fill item); (b) the next `late_rx.try_recv()` returns `Err(mpsc::error::TryRecvError::Empty)` — channel drained, NO force-completed exchange got through (all 3 dropped); (c) `svc.buckets.lock().unwrap().is_empty()` returns `true` — the inline test module has access to the private `buckets` field and confirms all 3 buckets were removed during `force_complete_all()`. Plus a greppable confirmation of the warn source: `rg -n 'aggregator force-complete emit dropped' crates/camel-processor/src/aggregator.rs` returns the warn line at ~184 that this test exercises.

**Acceptance:**
- `rg -n '### D-A3:' crates/camel-processor/CONTEXT.md` returns exactly one match.
- `cargo test -p camel-processor --lib test_da3_force_complete_all_drops_on_saturated_channel` passes.
- `cargo test -p camel-processor --lib test_late_channel_full_drops_with_warning` still passes (unchanged).
- `cargo fmt --check -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 3

### Task 4: D-A4 — per-bucket timeout task vs central checker + knob divergence doc (docs-only)

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md` under the Task 1 section, add a level-3 subsection `### D-A4: per-bucket timeout task vs central completion-timeout-checker + knob divergence`. Content: Apache Camel runs a single background completion-timeout-checker thread that polls all buckets every `completionTimeoutCheckerInterval(ms)` to find expired ones. rust-camel uses a per-bucket dedicated tokio task spawned by `spawn_timeout_task` (`crates/camel-processor/src/aggregator.rs:613`), cancelled and reset on each new exchange for that key, PLUS a `bucket_ttl` background sweep (interval `ttl/2`, `aggregator.rs:278-296`) as a fallback eviction path, PLUS a `max_timeout_tasks` DoS cap (`aggregator.rs:375-419`) that gracefully degrades to TTL-only eviction when the cap is reached. State that the OBSERVABLE completion semantics are EQUAL (a bucket completes after the configured inactivity period) but the MECHANISM differs. State the knob divergence: rust-camel exposes `max_timeout_tasks` and `bucket_ttl` (Camel does not); Camel exposes `completionTimeoutCheckerInterval` (rust-camel does not — the per-bucket task makes it unnecessary). State the forcing contract shape (the existing `CompletionCondition::Timeout`, `max_timeout_tasks`, and `bucket_ttl` configuration contracts already in `AggregatorConfig`).
2. NO new test — the existing `test_timeout_completes_bucket`, `test_timeout_resets_on_new_exchange`, `test_bucket_ttl_eviction`, and `test_aggregator_timeout_task_cap_no_panic_under_flood` already pin the behavior. Reference them by name in the CONTEXT.md subsection.

**Tests:**
- (none new) Verify the four named existing tests still pass, each as a separate command (cargo test treats positional args as a single filter substring, so run them individually):
  - `cargo test -p camel-processor --lib test_timeout_completes_bucket`
  - `cargo test -p camel-processor --lib test_timeout_resets_on_new_exchange`
  - `cargo test -p camel-processor --lib test_bucket_ttl_eviction`
  - `cargo test -p camel-processor --lib test_aggregator_timeout_task_cap_no_panic_under_flood`

**Acceptance:**
- `rg -n '### D-A4:' crates/camel-processor/CONTEXT.md` returns exactly one match.
- The four named existing tests pass via the command above.
- `cargo fmt --check -p camel-processor` exits 0 (CONTEXT.md is not Rust, but confirm no stray edits to .rs files in this task).

- [x] 4

### Task 5: D-A5 — mandatory memory bounds doc + typed-variant pin

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-api/src/aggregator.rs` (modified — one new test in the existing `#[cfg(test)] mod tests` block)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md` under the Task 1 section, add a level-3 subsection `### D-A5: mandatory memory bounds — validate() rejects unbounded configs`. Content: Apache Camel's default in-memory aggregation repository is UNBOUNDED (no mandatory cap; the operator may configure one). rust-camel's `AggregatorConfig::validate()` (`crates/camel-api/src/aggregator.rs:214`) REJECTS any config with no memory-release bound — it returns `CamelError::ConfigValidation(ConfigValidationError::AggregatorMissingMemoryBound)` when none of `max_buckets`, a `Timeout` completion condition, or `bucket_ttl` is set, and `ConfigValidationError::AggregatorTimeoutRequiresTtl` when a `Timeout` completion is present without `bucket_ttl`. The builder defaults are `max_buckets = 10_000` and `bucket_ttl = 300s`. State the forcing ADR: ADR-0033 (security defaults — typed `ConfigValidationError`, operators may match on the variant). State the consequence for an operator migrating from Camel: a config that is valid (if risky) in Camel may be REJECTED at build/validate time here; the operator must set an explicit bound. Reference the existing substring-based tests by name, and note the new typed-variant pin added in step 2.
2. In `crates/camel-api/src/aggregator.rs` test module, add ONE new test that pins the typed variant (the existing `test_aggregator_config_rejects_no_memory_bound` uses a 3-way substring check, not the typed variant the ADR-0033 contract promises). The new test constructs a config with no `max_buckets`, no `Timeout`, no `bucket_ttl` (direct struct construction bypassing builder defaults, mirroring the existing `test_aggregator_config_rejects_no_memory_bound` setup) and asserts the EXACT variant via `matches!`.

**Tests:**
- `test_da5_validate_returns_typed_missing_memory_bound_variant`: setup = construct an `AggregatorConfig` directly (bypass builder defaults) with `completion: CompletionMode::Single(CompletionCondition::Size(2))`, `max_buckets: None`, `bucket_ttl: None`, and the other fields at their trivial defaults (mirror the existing `test_aggregator_config_rejects_no_memory_bound` at line ~627). action = call `config.validate()`. assert = `assert!(matches!(err, CamelError::ConfigValidation(ConfigValidationError::AggregatorMissingMemoryBound)))` — the EXACT typed variant, not a substring. Also run the two existing tests to confirm they still pass.

**Acceptance:**
- `rg -n '### D-A5:' crates/camel-processor/CONTEXT.md` returns exactly one match.
- `cargo test -p camel-api --lib test_da5_validate_returns_typed_missing_memory_bound_variant` passes.
- The two named existing tests still pass, each as a separate command:
  - `cargo test -p camel-api --lib test_aggregator_config_rejects_no_memory_bound`
  - `cargo test -p camel-api --lib test_aggregator_timeout_requires_bucket_ttl`
- `cargo fmt --check -p camel-api` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 5

### Task 6: G-A1 — completionSize as Expression coverage gap note (docs-only)

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md` under the Task 1 section, add a level-3 subsection `### G-A1 (gap-coverage): completionSize as Expression — static Size only`. Content: Apache Camel supports `completionSize(expression)` where the size limit is evaluated per-exchange (e.g. derived from a header). rust-camel's `CompletionCondition::Size(usize)` (`crates/camel-api/src/aggregator.rs:70`) is STATIC — the limit is fixed at config time. State explicitly that this is a COVERAGE GAP (less surface), NOT a forced divergence — no ADR forbids an expression-based size; it is simply not yet implemented. State that implementing it is out of scope for this change (which is documentation-only per ADR-0046); a future feature task may add a `CompletionCondition::SizeExpr { expr, language }` variant mirroring the existing `PredicateExpr` variant.

**Tests:**
- (none) Documentation-only.

**Acceptance:**
- `rg -n '### G-A1' crates/camel-processor/CONTEXT.md` returns exactly one match.
- No Rust files modified in this task (`git diff --name-only` shows only `crates/camel-processor/CONTEXT.md`).

- [x] 6
