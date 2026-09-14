# httpflake — post-fix flake diagnosis (2026-09-14)

## What happened

Task 1.4's first AFTER soak (fix = helper mutex window + 10 s budget only)
still failed under load: `readiness_survives_concurrent_registry_reset`
panicked at its own 10 s deadline (run-4-full, run-1-supplementary in
`logs-after/`), and the panic — unwinding while holding
`REGISTRY_TEST_MUTEX` — poisoned the mutex, cascading 35 sibling failures
(`PoisonError` at every `.lock().unwrap()` site: 6030, 6075, 6157, 6346,
6391, 6440, 9222, 9919, 10089, 10449).

## Method

Six instrumented repro rounds under the same 12-burner load (logs in
`logs-diag/`), progressively narrowing:

1. Deadline dump: `polls=160` (loop ran steadily — scheduler starvation
   ruled out), `entries=1`, `staged=0`, `inner_poisoned=false`.
2. Start-task probes: `start-ENTER` + `start-GOT-REGISTRY` both fired —
   `get_or_spawn` completed; the entry WAS initialized under the correct
   key.
3. Key dump at deadline: map held only `("127.0.0.1", 0)` (a sibling's
   port-0-keyed entry) — our entry gone.
4. eprintln probes on all three `entries` writers (reset/evict/insert)
   showed zero activity — **but libtest swallows stderr of PASSING
   tests**, blinding the probes.
5. File-append probes (capture-immune): 1685 resets visible. The failing
   window trace:
   ```
   t7: start-ENTER port=34197
   t7: cell-SET key=(127.0.0.1:34197)      ← entry inserted
   t7: start-GOT-REGISTRY port=34197
   t4: reset-CALL                          ← 275µs later, MID-WINDOW
   t4: cell-SET key=(127.0.0.1:0)          ← t4 re-inserts its own entry
   t7: poll#32..#160 keys=["127.0.0.1:0"]  ← our entry wiped, 10 s → panic
   ```

## Root cause

`registry_rejects_tls_on_plain_port` (lib.rs ~6194) called
`ServerRegistry::reset()` WITHOUT holding `REGISTRY_TEST_MUTEX` — the
only one of 23 reset call sites unguarded (precise per-function scan;
the "all callers hold the mutex" belief recorded in the REGISTRY_TEST_MUTEX
doc comment and repeated in earlier reviews was wrong). Its reset fired
mid-window in another test, wiping the freshly inserted entry;
`bound_addr` then polled None forever.

This violates the blessed spec's own R2 ("test code that mutates the
global registry SHALL hold REGISTRY_TEST_MUTEX") — the caller was the
last unimplemented piece of that requirement.

## Fix (completing Task 1.3)

1. `registry_rejects_tls_on_plain_port` now takes
   `let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();` before its reset
   (with `#[allow(clippy::await_holding_lock)]`, matching the 29 existing
   guarded sites).
2. Poison-recovering acquire in the helper and the hammer
   (`unwrap_or_else(|p| p.into_inner())`): the mutex guards test
   serialization only — no structural invariant — so a failed sibling
   must not cascade; this kills the 35-test blast radius observed above.

## Verification

Acid test — the previously-failing loaded filter
(`cargo test -p camel-component-http --lib -- content_type_inferred
registry readiness_survives --test-threads=12`), 12 iterations under 12
spin burners: **12/12 green, 0 readiness panics** (before the rogue-guard
fix this filter failed 1-in-1..4). Full formal AFTER evidence: see
`after.md`.
