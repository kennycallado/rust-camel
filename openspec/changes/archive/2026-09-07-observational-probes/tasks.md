# Tasks: observational-probes

## Phase 1: step identity via divert probes + arrival sequence

### Task 1.1 — camel-mock global arrival indices

**Files:**
- crates/components/camel-mock/src/lib.rs (modified) — component counter threaded to inners
- crates/components/camel-mock/src/inner.rs (modified) — stamping, lockstep truncation, accessor, tests

**Steps:**
1. In `lib.rs`, add a private component-wide arrival counter `arrival_counter: Arc<AtomicU64>` to `MockComponent` (initialize at 0) and clone it into every `MockEndpointInner` created in `create_endpoint` (new private field `arrival_counter: Arc<AtomicU64>`).
2. In `inner.rs`: thread the counter Arc through `MockProducer` (private field + clone in `create_producer`, inner.rs:381-389 — the record path is `MockProducer::call`'s async block holding the `received` lock, inner.rs:465-474): inside the SAME critical section that holds the endpoint's `received` lock and pushes the exchange, (a) `arrival_counter.fetch_add(1, Ordering::Relaxed)` and (b) push the returned index into a new `arrival_indices: Mutex<Vec<u64>>` (or extend the state guarded by the existing received lock — pick whichever keeps one lock scope). Per-endpoint index order must hold by construction.
3. In the SAME bounded-retention truncation branch (`if len >= max_retained { pop_front }`), pop the paired index from `arrival_indices` in lockstep so indices always pair with retained exchanges.
4. In `reset()`: clear `arrival_indices` but do NOT touch the component-wide counter (post-reset arrivals keep increasing; no index reuse).
5. Add public accessor on `MockEndpointInner`: `pub async fn get_arrival_indices(&self) -> Vec<u64>` returning a snapshot in arrival order. `get_received_exchanges()` stays `Vec<Exchange>` — zero change to the step-2 expectation surface.

**Tests** (in-module `#[cfg(test)]`, `command: cargo test -p camel-component-mock --lib`):
- name: `arrival_indices_strictly_increasing_across_endpoints`
  - setup: one `MockComponent` with endpoints `a` and `b`
  - action: send exchanges interleaved a, b, a, b (awaiting each send)
  - assert: `a.get_arrival_indices() == [0, 2]`, `b.get_arrival_indices() == [1, 3]`; merged strictly increasing
  - expected: fails before implementation (method absent), passes after
- name: `arrival_indices_truncate_in_lockstep_with_retention`
  - setup: endpoint created with `retain=2` (URI param `mock:x?retain=2`), send three exchanges
  - assert: `get_arrival_indices()` has length 2 and equals the LAST two stamped indices (e.g. `[1, 2]` when stamped 0,1,2), pairs align with `get_received_exchanges()` lengths
  - expected: fails before (no indices), passes after
- name: `reset_clears_indices_but_counter_stays_monotonic`
  - setup: one endpoint, send two exchanges, `reset()`, send one more
  - assert: after reset indices empty; the post-reset send yields index 2 (not 0)
  - expected: fails before, passes after
- name: `concurrent_sends_preserve_per_endpoint_order`
  - setup: one endpoint, 32 concurrent sends with distinct bodies
  - action: `tokio::join` all sends
  - assert: `get_arrival_indices()` is strictly increasing (per-endpoint order by construction under the lock)
  - expected: fails if stamped outside the lock, passes after

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green (existing + new)
- `cargo clippy -p camel-component-mock -- -D warnings` exits 0
- `cargo xtask lint-unwrap` introduces no new hits
- `get_received_exchanges()` signature and behavior unchanged

- [x] 1.1

### Task 1.2 — camel-cli `sequence:` grammar, runner evaluation, divert driver lock

**Files:**
- crates/camel-cli/src/commands/test/document.rs (modified) — `sequence` field, normalization, error family
- crates/camel-cli/src/commands/test/runner.rs (modified) — post-settle sequence evaluation
- crates/camel-cli/src/commands/test/document_tests/parsing.rs (modified) — grammar tests
- crates/camel-cli/src/commands/test/driver_tests.rs (modified) — end-to-end sequence + divert-copy lock

**Steps:**
1. In `document.rs`: add `pub sequence: Option<Vec<String>>` to the parsed-document struct. Parse and normalize each entry exactly as `expects` keys (strip `mock:` scheme to bare endpoint name; reject non-`mock:` scheme and empty path). Add error variants `SequenceTooShort` (message: "sequence needs at least two entries") and `SequenceBadRef { entry }` (message: "sequence entry `{entry}` must be a mock: URI with a non-empty endpoint path"). Validation: entries >= 2, every entry normalized; duplicates allowed. The `camel run` path does not read the block (it already ignores test-document keys — no action needed beyond keeping the field test-side).
2. In `runner.rs`: widen the settle name set to `expects ∪ sequence` entries (runner.rs:487 currently samples only `doc.expects.keys()`) so sequence-listed endpoints absent from `expects` still quiesce. Then, after the settle window and after per-endpoint expectation evaluation, when `sequence` is present: collect `(arrival_index, endpoint_name)` for every endpoint named in the declared list by reading `get_arrival_indices()` per listed endpoint from the mock registry (a listed endpoint never created → treated as zero arrivals); sort by index; compare the name projection to the declared list element-wise. On mismatch produce the existing verdict-class assertion failure (exit 1 path) with the first divergence: `sequence mismatch at position {p}: expected {expected}, got {actual}` — symmetric: when arrivals ran out, `got <no further arrival>`; when the declared list is exhausted with arrivals remaining, name the unexpected arrival as `expected <end of sequence>, got {actual}` at the first extra position.
3. In `driver_tests.rs` add the divert-copy end-to-end lock (the canon scenario currently untested at driver level): route `from: direct:start` → `to: seda:audit` → `to: mock:sink`, second route `from: seda:audit` → `to: mock:drained`, document `intercepts: {seda:audit: {divertCopyTo: mock:audit}}` + `expects: {mock:audit: {count: 1}, mock:drained: {count: 1}}` — assert both counts satisfied, exit 0.
4. In `driver_tests.rs` add sequence tests per the spec scenarios (causally-ordered probes over two `log:` diverts pass; reversed fails with the position-0 message naming expected `probe-b` actual `probe-a`, exit 1; repeats-and-narrowing: three sends `[a, b, a]` declared, `mock:noise` traffic ignored, pass).
5. In `document_tests/parsing.rs`: grammar error tests — single-entry sequence → `SequenceTooShort` (exit-2 document error); entry `direct:x` → `SequenceBadRef` naming the entry; normalization test (`mock:probe-a` key form accepted alongside bare name identical to expects normalization).

**Tests** (command: `cargo test -p camel-cli --lib`; the test module target that holds driver_tests/parsing per existing layout):
- name: `sequence_passes_causally_ordered_sends` (driver)
  - setup: route with direct awaited sends `to: mock:probe-a` → `to: mock:probe-b` (deterministic causal order; divert-copy probes deliver detached wire-tap copies, so order over taps is happened-order only — see the 0b4d2528 fix), `sequence: [mock:probe-a, mock:probe-b]`, expects count 1 each
  - action: run the driver
  - assert: exit 0
  - expected: fails before implementation (sequence key unknown/ignored), passes after
- name: `sequence_reversed_fails_naming_first_divergence` (driver)
  - setup: same, `sequence: [mock:probe-b, mock:probe-a]`
  - assert: exit 1; failure text contains "position 0", "expected probe-b", "got probe-a"
  - expected: fails before (no such assertion), passes after
- name: `sequence_repeats_and_narrowing` (driver)
  - setup: route sending a→b→a plus traffic to `mock:noise`; `expects: {mock:probe-a: {count: 2}, mock:probe-b: {count: 1}, mock:noise: {count: 1}}`; `sequence: [mock:probe-a, mock:probe-b, mock:probe-a]`
  - assert: exit 0
  - expected: passes only after implementation
- name: `divert_copy_locked_end_to_end` (driver)
  - assert: divert target records count 1 AND real seda consumer endpoint records count 1, exit 0
  - expected: may pass before (weave exists) — this test LOCKS behavior, regression-guard
- name: `sequence_too_short_is_document_error` (parsing) — assert the `SequenceTooShort` variant (via the parsing tests' `err_of`/`matches!` precedent; the exit-2 family is already locked by the existing `parse_error_continues_and_exits_two`)
- name: `sequence_bad_ref_is_document_error` (parsing) — assert the `SequenceBadRef { entry: "direct:x" }` variant the same way

**Acceptance:**
- `cargo test -p camel-cli` green (existing + new)
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- Failure messages match the symmetric first-divergence contract exactly
- No behavior change for documents without `sequence:` (existing suite green)

- [x] 1.2

### Task 1.3 — docs, ADR-0072 second amendment, CONTEXT-MAP

**Files:**
- docs/src/testing/index.md (modified) — `sequence:` reference + probe pattern
- docs/adr/0072-test-pyramid-v2.md (modified) — second dated amendment
- CONTEXT-MAP.md (modified) — 0072 index entry refresh

**Steps:**
1. In `docs/src/testing/index.md`: extend the grammar reference after the `expects` paragraph with a `sequence` paragraph — list of `mock:` refs, at least two entries, duplicates allowed, normalized like expects keys; filtered-complete-interleaving semantics over listed endpoints; exit-1 verdict-class failure naming first divergence; grammar errors exit 2. Add a short "Probe pattern" subsection: observe intermediate route sends with `intercepts: {<real-uri>: {divertCopyTo: mock:probe-N}}` and assert order with `sequence:`; state the causality caveat — cross-endpoint order is deterministic only between causally-ordered sends; concurrent branches assert happened-order only.
2. In `docs/adr/0072-test-pyramid-v2.md`: append `## Amendment 2 — 2026-09-07 — step 3 delivery shape` recording: the observational weave predated the ADR (2026-08-23 declarative-intercepts); step 3 lands as divert-copy probes plus the arrival-sequence assertion (component-wide arrival indices, filtered interleaving); the ADR-0064 §5 gate reads precisely: `skipTo` exists only as a test-document construct, never in the production route DSL; Decision sections stand otherwise unchanged.
3. In `CONTEXT-MAP.md`: update the 0072 index entry to mention step-3 delivery (arrival-sequence assertion over divert-copy probes) alongside the shared algebra.

**Tests:**
- name: context citations
  - action: `cargo xtask lint-context-citations`
  - assert: exits 0 (0 violations)
  - command: `cargo xtask lint-context-citations`
  - expected: passes; any new CONTEXT-MAP/ADR citation must resolve

**Acceptance:**
- The testing guide names both error cases and the causality caveat verbatim-semantically matching the spec
- ADR amendment follows the dated-amendment format (ADR-0050 precedent, no parenthetical section refs)
- `cargo xtask lint-context-citations` exits 0

- [x] 1.3
