# Tasks: fix-fanout-claim-split

Binding context for every task (from bd rc-ulmo6 design field + mission order):
- `InFlightClaim` must stay non-`Clone`. `Exchange::clone` keeps setting `in_flight_claim: None`.
- All scratch/log output goes under `/home/shared/tmp/rc-ulmo6/` — NEVER `/tmp`.
- Every `cargo test` battery is scoped (create the dir first: `mkdir -p /home/shared/tmp/rc-ulmo6/{red,green}`):
  ```
  systemd-run --user --scope --collect --unit=fleet-rc-ulmo6-<n> \
    -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
    -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test <args> -j4
  systemctl --user stop fleet-rc-ulmo6-<n>.scope
  ```
  `<n>` is a fresh integer per run. Before and after each battery:
  `pgrep -c compiled_artifa` must print 0 (an exit code 1 with no output also means 0).
- Every task ends with `cargo fmt --check` and `cargo clippy -p <touched-crate> --all-targets -- -D warnings` run from the worktree root `/home/shared/rust-camel-worktrees/rc-ulmo6`.

## camel-api

### Task 1.1: Add Exchange::branch_clone() splitting the in-flight claim

**Files:**
- `crates/camel-api/src/exchange.rs` (modified — new method + tests in the existing `#[cfg(test)]` module at the bottom of the file)

**Steps:**
1. In `impl Exchange` (next to the `Clone` impl at line ~229), add:
   ```rust
   /// Clone for a fanout branch copy. Unlike [`Exchange::clone`], a
   /// carried in-flight claim is split onto the copy (claimfamily):
   /// the sibling counts the copy independently and releases when the
   /// copy completes; the original claim stays live.
   pub fn branch_clone(&self) -> Self {
       let mut copy = self.clone();
       copy.in_flight_claim =
           self.in_flight_claim.as_ref().map(|claim| claim.split());
       copy
   }
   ```
2. Do NOT touch `impl Clone for Exchange`, `InFlightClaim`, or `in_flight.rs`.
3. Add the three tests below to the existing test module in `exchange.rs`, using `crate::in_flight::InFlightGauge` + `std::sync::Arc` to build a gauge and `InFlightClaim::attach(&gauge)` to mint the source claim (same style as existing `in_flight.rs` tests).

**Tests:**
- `branch_clone_splits_claim_original_unaffected`: setup — `let gauge = Arc::new(InFlightGauge::new()); let mut src = Exchange::new(Message::default()); src.in_flight_claim = Some(InFlightClaim::attach(&gauge));` → act — `let copy = src.branch_clone();` → assert — `gauge.total() == 2`, `copy.in_flight_claim.is_some()`, `src.in_flight_claim.is_some()`.
- `branch_clone_sibling_releases_independently`: setup — same as above, then `let copy = src.branch_clone();` → act — `drop(copy);` → assert — `gauge.total() == 1`; act — `drop(src);` → assert — `gauge.total() == 0`.
- `plain_clone_carries_no_claim`: setup — claimed `src` as above, `gauge.total() == 1` → act — `let copy = src.clone();` → assert — `gauge.total() == 1`, `copy.in_flight_claim.is_none()`, `src.in_flight_claim.is_some()`.

**Acceptance:**
- Scoped battery `cargo test -p camel-api --lib` passes (all 3 new tests named above included).
- `cargo fmt --check` and `cargo clippy -p camel-api --all-targets -- -D warnings` exit 0.
- `rg -n "impl Clone for InFlightClaim" crates/camel-api/src/in_flight.rs` has no hits (claim still non-Clone).

- [x] 1.1

## camel-processor

### Task 2.1: Fanout drainclaim integration tests + RED witness capture

**Files:**
- `crates/camel-processor/tests/fanout_drainclaim_tests.rs` (new)

**Steps:**
1. Write a shared test helper in that file: a gated probe endpoint implemented as a tower `Service<Exchange>` (or the crate's endpoint trait used by `WireTapService`/`MulticastService` — match what `crates/camel-processor/src/wire_tap.rs` and `multicast.rs` accept) whose `call`: (a) sends one entry signal through a `tokio::sync::mpsc::UnboundedSender<()>`, (b) awaits `tokio::sync::Notify::notified()` (the release barrier), (c) returns the received exchange or a fresh `Exchange::new(Message::default())` per the variant below. Wrap every await in `tokio::time::timeout(Duration::from_secs(2), fut)` where `fut` is the awaited future — no sleeps (lint-test-sleep / lint-unbounded-wait).
2. Write the three tests (names exact, see below). Pattern for each: attach a real claim to the source exchange against a fresh `Arc<InFlightGauge>` (`crate::in_flight` types are re-exported via `camel_api` — use whatever path `crates/camel-processor` already uses for `camel_api::in_flight::InFlightGauge`), drive the service, drop the parent exchange + parent claim, then assert the gauge is held ONLY by the branch copy. For every final `gauge.total() == 0` assertion, do NOT assert immediately after the done-signal: the branch task drops its copy AFTER signaling done, so a same-instant assert races the drop. Use a bounded poll-until-zero helper instead — loop on `gauge.total()` with `tokio::time::sleep(Duration::from_millis(10))` inside a `tokio::time::timeout(Duration::from_secs(2), poll_fut)` future wrapping the loop (same shape as the existing `await_ctx_total` helper in `crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs:197`).
   - `wiretap_branch_copy_keeps_gauge_nonzero_after_parent_claim_drop`: build `WireTapService` with the gated probe as tap endpoint (see `WireTapService::call` at `crates/camel-processor/src/wire_tap.rs:277` — the tap runs as a detached `tracker.spawn` task). Act: await `call(exchange_with_claim)` → main exchange returns; wait for the tap entry signal; drop the returned main exchange and the parent claim. Assert: `gauge.total() == 1`; release the barrier; await the tap-done signal (bounded); assert `gauge.total() == 0`.
   - `multicast_branch_copy_outlives_parent_claim`: build the multicast service with two recipients: recipient A returns the exchange immediately; recipient B forwards the received exchange through an unbounded mpsc channel (moves it, does NOT clone it) and returns `Ok(Exchange::new(Message::default()))`. A receiver task awaits the channel, sends the entry signal, parks on the barrier. Act: await the multicast call with the claimed exchange; all recipients complete; wait for the receiver's entry signal; drop the returned exchange and the parent claim. Assert: `gauge.total() == 1`; release barrier; await receiver-done (bounded); assert `gauge.total() == 0`.
   - `split_branch_copy_outlives_parent_claim`: same forwarding-recipient pattern against the splitter path used by `crates/camel-processor/src/splitter.rs` / `split_segment.rs` (a segment recipient that forwards its received segment exchange into the channel and completes immediately). Same arrange/act/assert shape as the multicast test.
3. RED WITNESS (the point of this task — do NOT patch any production site): run the scoped battery
   `systemd-run --user --scope --collect --unit=fleet-rc-ulmo6-21 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-processor --test fanout_drainclaim_tests -j4`
   then `systemctl --user stop fleet-rc-ulmo6-21.scope`. All three tests MUST FAIL on the `gauge.total() == 1` assertion (unpatched: `Exchange::clone` drops the claim, so the gauge hits 0). Capture the full output to `/home/shared/tmp/rc-ulmo6/red/fanout-red.log` and record each test's exact failure line (the `assertion \`left == right\` failed / left: 0, right: 1` string) in the task report.
4. If a test fails for a DIFFERENT reason (compile error, panic, timeout, wrong gauge value like 2), fix the TEST harness, not the production code, and re-run until the failure is exactly the gauge assertion.

**Tests:**
- `wiretap_branch_copy_keeps_gauge_nonzero_after_parent_claim_drop`: setup — claimed exchange + WireTapService with gated probe tap → act — complete main, park tap, drop parent claim → assert — gauge reads 1 (copy's split claim), then 0 after barrier release. Expected on UNPATCHED code: FAIL with gauge 0 (RED witness).
- `multicast_branch_copy_outlives_parent_claim`: setup — claimed exchange + multicast with immediate + forwarding recipients → act — complete multicast, park forwarded copy, drop parent claim → assert — gauge reads 1, then 0 after release. Expected on UNPATCHED code: FAIL (RED witness).
- `split_branch_copy_outlives_parent_claim`: setup — claimed exchange + splitter with forwarding segment recipient → act/assert — same shape. Expected on UNPATCHED code: FAIL (RED witness).

**Acceptance:**
- The three tests compile and fail ONLY on the gauge assertions; RED log exists at `/home/shared/tmp/rc-ulmo6/red/fanout-red.log` with the three failure strings.
- `cargo fmt --check` and `cargo clippy -p camel-processor --all-targets -- -D warnings` exit 0.
- No production file modified (`git diff --stat` shows only the new test file).
- `pgrep -c compiled_artifa` is 0 after the battery.

- [x] 2.1

### Task 2.2: Split-fragment claim transfer at the fragment_exchange seam

**Files:**
- `crates/camel-api/src/splitter.rs` (modified — `fragment_exchange` at ~line 418 + test in its existing `#[cfg(test)]` module at ~line 507)

**Steps:**
1. In `fragment_exchange` (crates/camel-api/src/splitter.rs:418), after the existing field copies and before `ex` is returned, add the claim split:
   ```rust
   // claimfamily: each fragment is a fanout branch exchange of the
   // parent — split a sibling claim onto it so drain stays covered
   // while the fragment is in flight.
   ex.in_flight_claim = parent.in_flight_claim.as_ref().map(|claim| claim.split());
   ```
2. Do NOT touch `Exchange::clone`, `InFlightClaim`, or any tar/archive per-entry construction site (those stay out of scope per the delta spec).
3. Add the test below to the `#[cfg(test)] mod tests` at splitter.rs:507, using `crate::in_flight::{InFlightClaim, InFlightGauge}` + `std::sync::Arc`.

**Tests:**
- `fragment_exchange_splits_parent_claim`: setup — `let gauge = Arc::new(InFlightGauge::new()); let mut parent = Exchange::new(Message::new(Body::Text("a,b".into()))); parent.in_flight_claim = Some(InFlightClaim::attach(&gauge));` → act — `let frag = fragment_exchange(&parent, Body::Text("a".into()));` → assert — `gauge.total() == 2`, `frag.in_flight_claim.is_some()`, `parent.in_flight_claim.is_some()`; act — `drop(frag);` → assert — `gauge.total() == 1`.

**Acceptance:**
- Scoped battery `cargo test -p camel-api --lib` passes.
- `cargo fmt --check` and `cargo clippy -p camel-api --all-targets -- -D warnings` exit 0.

- [x] 2.2

### Task 2.3: Route camel-processor fanout clone sites through branch_clone (GREEN)

**Files (audit-seeded list from r_glm review; step 1 audit is the authority — line numbers drift):**
- `crates/camel-processor/src/wire_tap.rs` (modified — site ~279 `let tap_exchange = exchange.clone();`)
- `crates/camel-processor/src/multicast.rs` (modified — sites ~81, ~119, ~165, ~235)
- `crates/camel-processor/src/multicast_segment.rs` (modified — sites ~84, ~145, ~272)
- `crates/camel-processor/src/recipient_list.rs` (modified — sites ~74, ~91, ~125, ~147, ~155, ~158)
- `crates/camel-processor/src/splitter.rs` (modified — sites ~89, ~294)
- `crates/camel-processor/src/split_segment.rs` (modified — audit will surface a production stop-path Exchange clone at ~322 `PipelineOutcome::Stopped(ex) => Some(ex.clone())`; patch it or justify it. Its ~544/650/736 sites sit inside the `#[cfg(test)]` boundary at ~381: classify those as test-harness, do not patch them)
- `crates/camel-processor/src/streaming_splitter.rs` (modified — production site ~60; the ~243 site sits inside the `#[cfg(test)] mod tests` spanning ~185-866: classify as test-harness, do not patch it)
- `crates/camel-processor/src/streaming_split_segment.rs` (modified — site ~76)
- `crates/camel-processor/src/do_try.rs` (modified — sites ~127, ~316)
- `crates/camel-processor/src/do_try_segment.rs` (modified — site ~134)
- `crates/camel-processor/src/content_enricher.rs` (modified — site ~44 `producer.oneshot(original.clone())`)
- `crates/camel-processor/src/error_handler.rs` (modified — sites ~21, ~217, ~295, ~391, ~561, ~583, ~603, ~641, ~713; cfg-test boundary at ~735)
- `crates/camel-processor/src/load_balancer.rs` (modified — sites ~143, ~211)

**Steps:**
1. Audit: run `rg -n '\.clone\(\)' crates/camel-processor/src --glob '!tests/**'` and classify EVERY hit whose receiver carries an `Exchange` (including locals named `ex`, `p`, `original`, `tap_exchange`, `snapshot`, `frag` — not just `exchange.clone()`). Explicitly resolve the known cfg-test sites in the audit table: `resequencer/mod.rs:549` (test-harness, boundary ~518), `split_segment.rs:544/650/736` (test-harness, boundary ~381), `error_handler.rs` post-~735 hits, `multicast.rs` post-~246 hits, `recipient_list.rs` post-~224 hits, `splitter.rs` post-~307 hits. Requires Task 2.2 (fragment transfer) already merged in the worktree.
2. For every branch-escape site (the copy is sent to another task, endpoint, channel, or outlives the parent claim scope): replace `x.clone()` with `x.branch_clone()`. Default rule from design.md: when in doubt, a site is branch-escape.
3. For any site kept as plain `clone()` (copy provably completes before the source's claim drops and never escapes the claim scope): record site + justification in the task report. Expect zero or near-zero keepers under this rule.
4. GREEN run: re-run the Task 2.1 battery with a fresh unit number: `systemd-run --user --scope --collect --unit=fleet-rc-ulmo6-22 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-processor --test fanout_drainclaim_tests -j4`, then `systemctl --user stop fleet-rc-ulmo6-22.scope` — all three tests must now PASS. Capture full output to `/home/shared/tmp/rc-ulmo6/green/fanout-green.log` and record the `test result: ok. 3 passed` line in the task report.

**Tests:**
- Re-run `wiretap_branch_copy_keeps_gauge_nonzero_after_parent_claim_drop`, `multicast_branch_copy_outlives_parent_claim`, `split_branch_copy_outlives_parent_claim` — now expected PASS (GREEN witness).

**Acceptance:**
- GREEN log at `/home/shared/tmp/rc-ulmo6/green/fanout-green.log` showing `3 passed`.
- Scoped battery `cargo test -p camel-processor` passes (lib + all test targets).
- `cargo fmt --check` and `cargo clippy -p camel-processor --all-targets -- -D warnings` exit 0.
- Audit table in the task report: every Exchange-carrying clone site classified (branch_clone | justified-plain-clone | test-harness).
- `pgrep -c compiled_artifa` is 0 after the battery.

- [x] 2.3

## camel-core

### Task 3.1: Route camel-core lifecycle adapter clone sites through branch_clone

**Files (known sites — verify by audit in step 1):**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified — sites ~64, ~70, ~214, ~284, ~375)
- `crates/camel-core/src/lifecycle/adapters/exchange_uow.rs` (modified — sites ~88, ~108, ~111 — the latter two are `fire_hook` dispatches)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified — sites ~602, ~942, ~960, ~971; handler original stash + Security/CircuitBreaker boundary snapshots)

**Steps:**
1. Audit: run `rg -n '\.clone\(\)' crates/camel-core/src/lifecycle/adapters/` and classify every Exchange-carrying clone site (same receiver-name caveat as Task 2.2: `ex`, `p`, `parent`, `original`, `snapshot`).
2. For every site whose copy escapes the parent claim scope (branch dispatch, boundary snapshot that outlives the call, error machinery stash): swap to `x.branch_clone()`. For sites where the copy provably completes before the source's claim drops (e.g. a snapshot dropped at the end of the same sync scope): keep plain `clone()` and record the justification in the task report.
3. `Exchange::branch_clone` reaches camel-core through the existing `camel-api` dependency — no new dependency edges, no `Cargo.toml` changes.

**Tests:**
- `route_controller_drainclaim_tests` (existing, `crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs`) must stay green: scoped battery `systemd-run --user --scope --collect --unit=fleet-rc-ulmo6-31 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-core --lib -j4` then `systemctl --user stop fleet-rc-ulmo6-31.scope`.

**Acceptance:**
- Scoped `cargo test -p camel-core --lib` passes.
- `cargo fmt --check` and `cargo clippy -p camel-core --all-targets -- -D warnings` exit 0.
- Audit table in the task report covering every Exchange-carrying clone site under `lifecycle/adapters/` (branch_clone | justified-plain-clone | test-harness).
- `pgrep -c compiled_artifa` is 0 after the battery.

- [x] 3.1

## docs

### Task 4.1: Update CONTEXT-MAP claimfamily entry

**Files:**
- `CONTEXT-MAP.md` (modified — claimfamily / "In-flight claim (canonical)" entry at ~line 128)

**Steps:**
1. In `CONTEXT-MAP.md` at the claimfamily entry (~line 128), extend the sibling minting-site enumeration ("the three pipeline drain sites and the aggregate loop's oneshot dispatch paths") with the new minting classes: fanout branch copies minted by `Exchange::branch_clone()` (wiretap, multicast/segment, recipient list, splitter/streaming splitter/segment, load balancer, do_try forwarding, error-handler snapshots, camel-core lifecycle adapters) and split fragments minted by `fragment_exchange` (splitter fragment seam).
2. Run `cargo xtask lint-context-citations` from the worktree root — must pass.

**Tests:**
- No runtime tests (docs-only). Verification: `rg -n "branch_clone|fragment_exchange" CONTEXT-MAP.md` has at least one hit inside the claimfamily entry.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo fmt --check` exits 0 (no-op for docs but run it as the standard closing check).

- [x] 4.1
