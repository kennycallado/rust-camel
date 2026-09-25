# Mutation evidence: stallflake

Mutant applied to `WasmRuntime::drive_with_drain_watchdog` Phase 1 (`Some(t)`
loop): the stall sleep armed once before the loop
(`let mut stall_sleep = Box::pin(tokio::time::sleep(t));` with `_ = &mut stall_sleep`
as the select arm), progress `continue` branch untouched — exactly the
design.md "Mutation check" mutant. The rewritten paused-clock test detects
the broken reset: the deadline stays pinned at virtual t=40 (pings at
t=0/25/50/75 cannot move it), so the test fails deterministically with
`invoke stalled` instead of flaking.

Command: `cargo test -p camel-component-wasm --lib invoke_stall_progress_resets_timer`

Captured failure output:

```
   Compiling camel-component-wasm v0.54.0 (/home/shared/rust-camel-worktrees/stallflake/crates/components/camel-component-wasm)
    Finished `test` profile [unoptimized] target(s) in 10.99s
     Running unittests src/lib.rs (target/debug/deps/camel_component_wasm-e0f378b849738e6c)

running 1 test
test runtime::tests::invoke_stall_progress_resets_timer ... FAILED

failures:

---- runtime::tests::invoke_stall_progress_resets_timer stdout ----

thread 'runtime::tests::invoke_stall_progress_resets_timer' (3951942) panicked at crates/components/camel-component-wasm/src/runtime.rs:1126:9:
progress should have reset the stall timer, got: Err(GuestPanic("wasm: invoke stalled — no input progress (upstream stalled or guest deadlocked)"))
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    runtime::tests::invoke_stall_progress_resets_timer

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 177 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p camel-component-wasm --lib`
```
