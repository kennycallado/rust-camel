# Bridge child lifetime evidence — after

- date: 2026-10-24T10:24:15Z
- commit: `7c341d6d5`
- mode: `after`
- evidence token: `m390-1792837455176243718`
- command: `cargo test -p camel-bridge --test bridge_exit_path_evidence -- --ignored --nocapture 2>&1 | cat`
- pipeline: completed (exit 0)
- timeout exit code: 0
- wall time: 33s
- live token orphans at cancel: 0

## Per-test results

```text
test panic_parent_cleans_group ... ok
test sigterm_parent_cleans_group ... ok
test sigint_parent_cleans_group ... ok
test sigkill_parent_cleans_group ... ok
test term_during_teardown_window_parent_death ... ok
test watchdog_leader_exited_descendants_still_cleaned ... ok
test caller_thread_retirement_keeps_bridge ... ok
test leak_parent_then_exit ... ok
test startup_timeout_leaves_no_member ... ok
test stderr_file_capture_unchanged ... ok
test chatty_stderr_does_not_delay_readiness ... ok
test graceful_stop_reaps_no_running_members ... ok
test launcher_grandchild_dies_with_group ... ok
test concurrent_spawns_keep_eof_discipline ... ok
```

## Post-run fixtures (`pgrep -af m390-1792837455176243718`)

```text

```

## Captured output (tail)

```text
   Compiling aws-lc-sys v0.45.0
   Compiling ring v0.17.14
   Compiling x509-parser v0.18.1
   Compiling rcgen v0.14.10
   Compiling aws-lc-rs v1.18.1
   Compiling rustls v0.23.46
   Compiling rustls-webpki v0.103.16
   Compiling tokio-rustls v0.26.6
   Compiling rustls-platform-verifier v0.7.0
   Compiling hyper-rustls v0.27.9
   Compiling reqwest v0.13.5
   Compiling tonic v0.14.6
   Compiling camel-bridge v0.60.0 (/home/shared/rust-camel-worktrees/390-bridge-child-lifetime/crates/services/camel-bridge)
    Finished `test` profile [unoptimized] target(s) in 27.73s
     Running tests/bridge_exit_path_evidence.rs (target/debug/deps/bridge_exit_path_evidence-af99f1a75a8dd30f)

running 14 tests
test panic_parent_cleans_group ... ok
test sigterm_parent_cleans_group ... ok
test sigint_parent_cleans_group ... ok
test sigkill_parent_cleans_group ... ok
test term_during_teardown_window_parent_death ... ok
test watchdog_leader_exited_descendants_still_cleaned ... ok
test caller_thread_retirement_keeps_bridge ... ok
test leak_parent_then_exit ... ok
test startup_timeout_leaves_no_member ... ok
test stderr_file_capture_unchanged ... ok
test chatty_stderr_does_not_delay_readiness ... ok
test graceful_stop_reaps_no_running_members ... ok
test launcher_grandchild_dies_with_group ... ok
test concurrent_spawns_keep_eof_discipline ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.16s
```

## Verdict

**CLEAN**
