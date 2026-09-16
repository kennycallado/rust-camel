# Tasks: cachereclaim

## camel-core cache offload

### Task 1.1: Reclaim happy-path and guard tests (TDD — write first, expect failures)

**Files:**
- `crates/camel-core/src/cache/disk_offload_tests.rs` (modified)

**Steps:**
1. Read the blessed delta spec at `openspec/changes/cachereclaim/specs/eip-cache/spec.md` (Requirement "Eager predecessor reclaim on overwrite") before writing any test.
2. Using the existing helpers in the file (`entry`, `inner_repo`, `fixed_clock`, `new_repo`, `dir_names`, `capture_warns`), add the seven tests below. Match the file's existing `#[tokio::test]` style and its directory-assertion idiom (sort `dir_names` before comparing).
3. For the reader-race test, capture the pre-swap row by calling `get` on the inner `MemoryCacheRepository` handle the test constructed (the row carries `payload_path = Some(old_blob_name)` with empty bytes), hold it across the overwrite, then hydrate it through the concrete `DiskOffloadRepository` (the tests module is a child of `disk_offload.rs`, so the private `hydrate` method is callable on a concrete, non-`Arc<dyn>` repository value).
4. For the expired-but-retained test, use ONE repository with a fixed decorator clock (deterministic death epochs, epoch far in the future) and a tiny REAL ttl (e.g. `Some(Duration::from_millis(20))`) on the first write — the inner `MemoryCacheRepository` computes expiry on `SystemTime::now()` (it has no injectable clock), so `tokio::time::sleep(50ms)` after the first `set` makes the inner row invisible to `get` while `peek_stale` still serves it. The second write then overwrites the key.
5. For `same_content_next_second_reclaims_old_blob`, build an advancing clock seam: `let now = Arc::new(Mutex::new(T0));` and a closure `OffloadClock` that reads `*now.lock().unwrap()` — advance it by exactly one second between the two writes. (`unwrap` on a `Mutex` lock is the existing test-file idiom for short critical sections; if the file avoids it, use `lock().unwrap_or_else(|p| p.into_inner())`.)
5. Run the tests; confirm the three positive tests (`overwrite_reclaims_predecessor_bounds_versions_at_one`, `overwrite_reclaim_reader_race_miss_not_error`, `same_content_next_second_reclaims_old_blob`) FAIL against the current implementation (the other four are negative tests that may pass trivially — nothing is unlinked today). Record the failure output.

**Tests:** (all under `cargo test -p camel-core --lib disk_offload`)
- `overwrite_reclaims_predecessor_bounds_versions_at_one`: repo with fixed clock, ttl 1h, stale_retention 168h, sweep_interval huge; set("k", payload A) then set("k", payload B); assert `set` returned `Ok(())`, `dir_names` contains exactly ONE `.blob` file (the fresh fingerprint — derive the expected name via the same blake3 inputs or by asserting the count and that `get("k")` returns payload B). Expected before implementation: FAIL (two blobs present).
- `overwrite_reclaim_reader_race_miss_not_error`: set("k", A); capture inner row; set("k", B); assert `dir_names` has one blob; call the private `hydrate("k", captured_row)`; assert it returns `Ok(None)` (MISS, WARN logged via `capture_warns`) and NOT `Err`; then assert `get("k")` returns B. Expected before implementation: FAIL (hydrate on the captured row finds the old blob still on disk and returns `Ok(Some(A))`).
- `reclaim_never_unlinks_foreign_or_other_key_blobs`: set("k1", A) then ("k2", B) through the decorator (blobs b1, b2); place two foreign files in the payload dir: `readme.txt` and `{blake3_128hex("k1")}.garbage.blob` (key-prefix-matching, epoch-unparseable). First corruption: overwrite k1's index row directly in the inner repo so `payload_path` names b2 (sanitize-passing, epoch-bearing, WRONG key prefix); overwrite "k1" (blob b3); assert `set` is `Ok(())` and the dir holds exactly FIVE files: b1 (k1's old blob — now an unreferenced orphan only the sweeper can reclaim, because the reclaim read the corrupted row and the ownership guard rejected b2), b2 (k2's live blob), b3 (k1's fresh blob), `readme.txt`, `garbage.blob`. Second corruption: overwrite the row again to name `garbage.blob`; overwrite "k1" (blob b4); assert the dir holds SIX files — the garbage file survived the parse-death-epoch guard — and `get("k1")` returns the last payload while `get("k2")` still returns B. Expected: PASS before implementation is acceptable (negative test — nothing is unlinked today) — must stay PASS after.
- `same_second_identical_rewrite_keeps_blob`: fixed clock frozen at one instant; set("k", P, Some(1h)) twice with identical bytes and content type; assert the single blob file still exists after the second set and `get("k")` returns P. Expected: PASS before and after (guard test).
- `same_content_next_second_reclaims_old_blob`: clock seam advancing exactly one second between writes (same bytes, same ttl, same content type — the death epoch changes so the filename changes); assert exactly ONE blob remains after the second set and it is the NEW epoch's file (`get("k")` returns the bytes). Expected before implementation: FAIL (two blobs).
- `expired_retained_row_not_eagerly_reclaimed`: per step 4 — first `set("k", A, Some(20ms))` (fixed decorator clock, real 20ms ttl), `tokio::time::sleep(50ms)`, then `set("k", B, Some(1h))`; assert the second write returned `Ok(())` and the FIRST blob file still exists on disk (the inner `get` hit expiry on its real clock and could not observe the row, so no eager reclaim; the sweeper owns the old blob); `get("k")` returns B from the fresh blob. Expected: PASS before implementation is acceptable — must stay PASS after.
- `inline_row_predecessor_no_unlink`: store an inline row for "k" directly into the inner repo (full bytes, `payload_path = None`) with NO blob on disk; overwrite "k" through the decorator; assert `set` is `Ok(())`, exactly one blob exists afterward (the fresh one), `get("k")` returns the new payload, and NO reclaim WARN was captured (nothing to reclaim — behaves as a first write). Expected: PASS before and after (guard test).

**Acceptance:**
- `cargo test -p camel-core --lib disk_offload` runs the seven new tests; the three marked "Expected before implementation: FAIL" demonstrably fail for the stated reason (blob count / hydrate result), the others pass.
- `cargo fmt --check` clean; `cargo clippy -p camel-core -- -D warnings` clean.

- [x] 1.1

### Task 1.2: Reclaim failure-path tests (TDD — write first)

**Files:**
- `crates/camel-core/src/cache/disk_offload_reclaim_tests.rs` (new)
- `crates/camel-core/src/cache/disk_offload.rs` (modified — one line: `#[cfg(test)] mod disk_offload_reclaim_tests;`)
- `crates/camel-core/src/cache/disk_offload_tests.rs` (modified — move the seven Task 1.1 tests into the new module and mark the shared helpers `pub(super)`)

**Steps:**
0. Split first: create `disk_offload_reclaim_tests.rs` as a second `#[cfg(test)]` child module of `disk_offload.rs`; MOVE the seven Task 1.1 tests into it unchanged; make the helpers they use (`entry`, `inner_repo`, `fixed_clock`, `new_repo`, `dir_names`, `capture_warns`, and any constants like `RETENTION`/`SWEEP`/`MAX_TTL`) `pub(super)` in `disk_offload_tests.rs` and import them in the new module. Run `cargo test -p camel-core --lib disk_offload` — same 25P/3F result as after Task 1.1 before proceeding.
1. Add two test-only inner-backend wrappers in the new module, following the existing test style (they implement `CacheRepository` by delegating every method to a wrapped `Arc<MemoryCacheRepository>`): `FailGetRepo` (a `get` that returns an `Err` built from whichever `CamelError` variant the file already imports; toggle via an `AtomicBool` so the failure fires only on the pre-swap read and the post-swap `set` works) and `FailSetRepo` (a `set` that always returns `Err`). Both need `name()` returning a distinct string.
2. Reuse the existing inline-fallback technique from `blob_write_failure_falls_back_inline` (read-only `payload_dir` permissions) for the two fallback tests. Both fallback tests MUST restore the directory permissions to writable (drop guard preferred, same cleanup discipline as step 3) so assertion failures do not leak read-only temp directories to later tests.
3. For `reclaim_unlink_failure_never_fails_set`: arrangement — first `set` runs with a WRITABLE dir (creates the offloaded predecessor), then `chmod 500` the payload dir, then the second `set` runs: its fresh blob write fails (dir not writable) so it degrades to inline, and the reclaim unlink of the predecessor also fails with EACCES. Assert: `set` is `Ok(())`, a WARN was captured (blob-write fallback WARN and/or reclaim WARN), the predecessor file still exists, and `get("k")` returns the FULL new bytes (inline row). This exercises the best-effort contract end to end on the fallback path; note the arrangement in a test comment. Restore `chmod 700` after the test (drop-guard or explicit tail call) so other tests are unaffected.
4. Add `reclaim_rejects_traversal_predecessor_name` (sanitize-guard leg, per r_glm Task-1.1 review): canary file `canary.blob` written in the tempdir's PARENT directory; corrupt the key's row so `payload_path = "../canary.blob"`; overwrite the key; assert `set` `Ok(())`, the canary file still exists, and the payload dir's own fresh blob is intact. Negative test — PASS before and after implementation; a missing `sanitize_blob_name` guard in the reclaim would delete the canary.
5. Run the tests; confirm each stated expectation. Three tests are positive TDD tests and FAIL before implementation (`preswap_row_read_failure_skips_reclaim` — no pre-swap read exists today so its WARN never fires; `inline_fallback_reclaims_predecessor` and `inline_fallback_equal_name_attempts_reclaim` — no reclaim attempt exists today so the reclaim-specific observable is absent). The others (`inner_set_failure_skips_reclaim`, `reclaim_unlink_failure_never_fails_set`, `reclaim_rejects_traversal_predecessor_name`) PASS trivially before implementation and must keep passing after.

**Tests:** (same command as Task 1.1)
- `preswap_row_read_failure_skips_reclaim`: decorator wraps `FailGetRepo`; first `set("k", A)` runs with the failure toggle OFF (creates the offloaded predecessor blob AND row); flip the toggle ON; second decorator `set("k", B)` runs; assert the write returns `Ok(())` (the inner `set` still works — only `get` fails), a WARN whose message contains `pre-swap row read` was captured, and the predecessor blob file still exists. Expected before implementation: FAIL (no pre-swap read exists; no such WARN fires). After: PASS.
- `inner_set_failure_skips_reclaim`: `FailSetRepo` inner; predecessor blob exists on disk (written via a plain memory-backed decorator `set` first, or by constructing the decorator over a repo whose row+blob a helper wrote); decorator `set("k", new)`; assert the call returns `Err` and the predecessor blob still exists (no unlink). Expected: PASS before — must stay PASS after.
- `reclaim_unlink_failure_never_fails_set`: per step 3 arrangement; assert `Ok(())`, at least one WARN captured (the blob-write fallback WARN always fires in this arrangement), the predecessor file still present, and `get("k")` returns the FULL new bytes (inline row). Expected: PASS before — must stay PASS after.
- `inline_fallback_reclaims_predecessor`: writable-dir first set (offloaded predecessor); then read-only dir; second `set` degrades inline; assert `set` `Ok(())` and the reclaim-specific outcome: the predecessor blob is ABSENT, OR (when the read-only dir also blocks the unlink) a WARN whose message contains the reclaim-failure text (e.g. `eager reclaim` or the helper's warn message — use the exact substring the implementation logs) was captured with the predecessor remaining. Expected before implementation: FAIL (predecessor remains AND no reclaim WARN exists — neither branch holds). After: PASS via whichever branch the platform takes.
- `inline_fallback_equal_name_attempts_reclaim`: clock frozen (the retry would produce the SAME destination filename), first set offloaded, then read-only dir, second `set` identical payload degrades inline; assert a WARN whose message contains the SAME reclaim-failure substring was captured (the unlink was ATTEMPTED without the equal-name guard; under a read-only dir the attempt fails audibly — a wrongly-applied guard would produce no attempt and no such WARN), the predecessor file remains, and `get("k")` returns the payload from the inline row. Expected before implementation: FAIL (no reclaim attempt, no reclaim WARN). After: PASS.

**Acceptance:**
- `cargo test -p camel-core --lib disk_offload` runs the six new tests (five original + the traversal canary); the three marked "FAIL before implementation" demonstrably fail for the stated reason (missing WARN / missing reclaim outcome), the other three pass; each test includes a comment stating its pre/post-implementation expectation.
- `cargo fmt --check` clean; `cargo clippy -p camel-core -- -D warnings` clean.

- [x] 1.2

### Task 1.3: Implement the eager predecessor reclaim in `set()`

**Files:**
- `crates/camel-core/src/cache/disk_offload.rs` (modified)

**Steps:**
1. Add a private helper `async fn reclaim_predecessor(&self, key: &str, old_name: Option<&str>, keep_name: Option<&str>)` on `DiskOffloadRepository` that: (a) returns immediately when `old_name` is `None`; (b) returns immediately when `keep_name` is `Some(n)` and `old_name == Some(n)` (the equal-name guard — only a fresh blob owns that name); (c) requires `sanitize_blob_name(old_name)` to pass, `parse_death_epoch(old_name)` to be `Some`, and `old_name` to start with `format!("{}.", blake3_128hex(key.as_bytes()))` (the key-ownership guard); (d) `tokio::fs::remove_file(self.dir.join(old_name))` — on `Ok` or `ErrorKind::NotFound` return silently, on any other error emit one `warn!` (fields: key, backend = `self.inner.name()`, dir, error; message containing the words `eager reclaim` so tests can match the substring) stating the reclaim failure and the sweeper backstop; the function never returns `Err`.
2. In `set()`: before computing the death epoch, capture the predecessor via `self.inner.get(key).await`: on `Ok(Some(row))` take `row.payload_path` as `Option<String>`; on `Ok(None)` leave `None`; on `Err(e)` emit one `warn!` (key, backend, error: "pre-swap row read failed; skipping eager reclaim") and leave `None` — the write proceeds unchanged.
3. After the successful-blob branch's `self.inner.set(key, entry, Some(effective_ttl)).await` returns `Ok(())`, call `reclaim_predecessor(key, old_name.as_deref(), Some(&dest_name)).await`. When `inner.set` returns `Err`, propagate unchanged WITHOUT reclaim.
4. In the inline-fallback branch (blob write failed): when `self.inner.set(key, entry, Some(effective_ttl)).await` returns `Ok(())`, call `reclaim_predecessor(key, old_name.as_deref(), None).await` — `keep_name: None` disables the equal-name guard (no fresh file owns the name). `Err` propagates without reclaim.
5. All guard evaluation (`sanitize`, epoch, prefix, equality) happens inside `reclaim_predecessor`, so both call sites share one implementation. Add a short doc comment on the helper citing ADR-0065's amendment section.
6. Run the full test file: the Task 1.1 positive tests now pass; the negative tests still pass; Task 1.2 tests still pass.

**Tests:** (verification — these already exist from Tasks 1.1/1.2; run, do not author new ones)
- `cargo test -p camel-core --lib disk_offload` — all tests pass, including all pre-existing tests EXCEPT possibly `concurrent_same_key_different_payload_no_cross_pair` (inspect it: if it asserts the loser blob survives until its epoch in a way that a same-key sequential follow-up write would now reclaim, that is Task 1.4's scope; pure no-cross-pair assertions must pass unchanged here).

**Acceptance:**
- `cargo test -p camel-core --lib disk_offload` — every NEW test passes, every pre-existing test passes EXCEPT at most `concurrent_same_key_different_payload_no_cross_pair` (the sole reconciliation candidate, owned by Task 1.4; if it fails, record which assertion). Full-green is Task 1.4's gate, not this one.
- `cargo fmt --check` clean; `cargo clippy -p camel-core -- -D warnings` clean; `cargo clippy -p camel-core --all-targets -- -D warnings` clean.

- [x] 1.3

### Task 1.4: Existing-test reconciliation and module gates

**Files:**
- `crates/camel-core/src/cache/disk_offload_tests.rs` (modified, only if reconciliation is needed)

**Steps:**
1. Audit every pre-existing test in `disk_offload_tests.rs` against the new behavior: `concurrent_same_key_different_payload_no_cross_pair` (both writers capture the same predecessor; after both settle, the winner's blob remains, the loser's blob remains until its epoch — the eager reclaim unlinks only the shared predecessor; adjust assertions ONLY where they encode the old "orphan survives any next overwrite" wording, per the MODIFIED decorator scenario "concurrent same-key writers never cross-pair content"), `invalidate_delegates_and_blob_survives_until_epoch` (invalidate stays delegate-only — must pass unchanged), `clear_unlinks_dir_and_delegates`, `sweep_*` (sweeper untouched), `blob_reclaimed_after_death` (epoch path untouched).
2. Where an assertion changes, the new assertion must cite the blessed delta spec scenario it now encodes (comment line naming the scenario).
3. Run the whole crate's unit tests and the local quality gates for the touched crate.

**Tests:**
- `cargo test -p camel-core --lib` — whole crate green.
- `cargo fmt --check --all` — clean.
- `cargo clippy -p camel-core --all-targets -- -D warnings` — clean.
- `cargo xtask lint-unwrap` — no new `unwrap()` in the diff.

**Acceptance:**
- All commands above exit 0 in the worktree.
- Every scenario in the blessed delta's ADDED requirement maps to exactly one named test (Tasks 1.1 + 1.2 inventory); a one-paragraph mapping note is added at the bottom of tasks.md's Task 1.4 block in the final report (not in code comments). The note must record that scenario 8's success branch (predecessor physically absent after a fallback-path unlink) is unobservable on POSIX when the same chmod forces the fallback — the WARN-attempt branch carries the coverage there.

- [x] 1.4

**Scenario→test mapping (Task 1.4 acceptance note).** The ADDED
requirement "Eager predecessor reclaim on overwrite" maps to thirteen tests in
`disk_offload_reclaim_tests.rs`: overwrite-reclaims-immediately →
`overwrite_reclaims_predecessor_bounds_versions_at_one`; pre-swap-read-failure →
`preswap_row_read_failure_skips_reclaim`; reader-race miss-not-error →
`overwrite_reclaim_reader_race_miss_not_error`; unlink-failure-never-fails-write →
`reclaim_unlink_failure_never_fails_set`; foreign/unparseable/other-key names →
`reclaim_never_unlinks_foreign_or_other_key_blobs` (5-then-6 file ledger);
same-second identical rewrite → `same_second_identical_rewrite_keeps_blob`;
next-second identical rewrite → `same_content_next_second_reclaims_old_blob`;
inner-set failure → `inner_set_failure_skips_reclaim`; inline-fallback reclaim →
`inline_fallback_reclaims_predecessor`; equal-name fallback →
`inline_fallback_equal_name_attempts_reclaim`; inline-row predecessor →
`inline_row_predecessor_no_unlink`; expired-but-retained →
`expired_retained_row_not_eagerly_reclaimed`; traversal names (sanitize leg,
r_glm Task-1.1 finding) → `reclaim_rejects_traversal_predecessor_name`. The
MODIFIED decorator scenario "concurrent same-key writers never cross-pair
content" maps to the reconciled `concurrent_same_key_different_payload_no_cross_pair`
(sequential reality documented in-test; count 2→1 cites the ADDED overwrite
scenario). Scenario 8's success branch (predecessor physically absent after a
fallback-path unlink) is unobservable on POSIX — the chmod that forces the
fallback also denies the unlink — so the WARN-attempt branch
(`inline_fallback_*`) carries the coverage there.
