# Tasks: redislog

## camel-component-redis

### Task 1.1: Split recovery-pass log level (idle → DEBUG, backlog/claims → INFO)

**Files:**
- `crates/components/camel-redis/src/streams.rs` (modified)
- `crates/components/camel-redis/src/stream_session_tests.rs` (modified)

**Steps:**
1. In `streams.rs`, in the recovery-pass `Ok(batch)` arm (call site ~1526-1547):
   keep the existing `pending` Option rendering
   (`map_or_else(|| "-".to_string(), |count| count.to_string())`) and the
   comment above it unchanged. Bind `let claimed = batch.recovered.len();`
   BEFORE `queue.extend(batch.recovered)` moves the batch.
2. Wrap the existing `info!` in
   `if claimed > 0 || batch.pending_count.is_some_and(|count| count > 0) {`
   keeping the rendered output identical — same field names (`stream`,
   `group`, `pending`, `claimed`) and same message
   `"Stream recovery pass completed"`; the `claimed` field reads the bound
   `let claimed` (do not re-inline `batch.recovered.len()`).
3. Extend the import at `streams.rs:36` from
   `use tracing::{info, warn};` to `use tracing::{debug, info, warn};`
   (`debug!` is NOT currently imported). Add the `else` branch:
   `debug!(stream = %params.key, group = %params.group,
   pending = %pending, "Stream recovery pass idle")` (three fields).
4. Do NOT touch anything else in the loop: same XAUTOCLAIM, same cursor
   update (`*cursor = batch.next_cursor.clone()`), same
   `queue.extend(batch.recovered)`, same `*attempt = 0`, same
   cancellation/retry handling.
5. In `stream_session_tests.rs`, adjust `recovery_pass_logs_pending_count`
   (~3228): after the existing field assertions, add
   `assert_eq!(summary.level, "INFO", "the backlog diagnostic (pending>0, claimed=0) must stay at INFO")`.
6. In `stream_session_tests.rs`, add three new tests next to it, following
   the existing harness pattern: `capture_logs()`; `FakeRecoveryIo::new()`
   plus `.with_recover_batches(vec![recovery_batch(<pending>, <recovered>,
   "0-0">)])` with the per-test arguments from the Tests block below;
   `spawn_core(Box::new(fake), recovery_params(60_000),
   CancellationToken::new(), None, |_entry: StreamEntry| async {
   DeliveryOutcome::Completed })`; send `ReadTaskMsg::Connected`; await one
   `ReadTaskCmd::BatchDone` via `recv_cmd(&mut run.cmd_rx)`; `finish_ok`;
   then filter `capture.events()` by message substring as asserted below.

**Tests:** (executable spec — name, arrange, act, assert)
- `recovery_idle_pass_logs_debug_not_info`: setup = capture_logs +
  `FakeRecoveryIo` with one `recovery_batch(Some(0), Vec::new(), "0-0")` →
  action = drive the session through one completed recovery pass → assert =
  zero events whose message contains `"recovery pass completed"`, and
  exactly one event whose message contains `"recovery pass idle"` with
  `level == "DEBUG"`, `field("stream") == Some(<params key>)`,
  `field("group") == Some(<params group>)`, `field("pending") == Some("0")`.
  Expected: fails before step 2-3 (idle pass still logs INFO and no idle
  line exists), passes after.
- `recovery_pass_with_claims_logs_info`: setup = capture_logs +
  `FakeRecoveryIo` with one `recovery_batch(Some(0),
  vec![recovered("1-1", &[], Some(1))], "1-1")` → action = drive one pass →
  assert = exactly one event containing `"recovery pass completed"` with
  `level == "INFO"` and all four fields (`stream`, `group`,
  `field("pending") == Some("0")`, `field("claimed") == Some("1")`), and
  zero events containing `"recovery pass idle"`. Expected: green before
  AND after — today every pass logs INFO with the four fields, so the
  assertions already hold; this test is the regression guard for the
  claimed>0 → INFO branch (an idle condition keyed on `pending == 0`
  alone, or an inverted guard, would emit no completed INFO here and
  fail it). It cannot be validated red-first; the red-first proof for
  the split lives in `recovery_idle_pass_logs_debug_not_info`.
- `recovery_idle_pass_without_pending_count_renders_dash`: setup =
  capture_logs + `FakeRecoveryIo` with one `recovery_batch(None,
  Vec::new(), "0-0")` → action = drive one pass → assert = exactly one event
  containing `"recovery pass idle"` with `level == "DEBUG"` and
  `field("pending") == Some("-")`. Expected: fails before (no idle line),
  passes after.

**Acceptance:**
- `cargo test -p camel-component-redis` exits 0, including the existing
  `recovery_pass_logs_pending_count` (now also asserting `INFO`) and the
  three new tests.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings`
  exits 0.
- `cargo xtask lint-log-levels` and `cargo xtask lint-log-redaction` exit 0.
- The recovery loop outside the `Ok(batch)` logging block is unchanged
  (`git diff` shows no edits to cursor/retry/cancel logic).

- [x] 1.1
