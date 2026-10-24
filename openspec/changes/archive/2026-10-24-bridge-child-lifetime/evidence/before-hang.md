# Bridge child lifetime evidence — before

- date: 2026-10-24T08:39:02Z
- commit: `667773ac3`
- mode: `before`
- evidence token: `m390-1792831142536683199`
- command: `cargo test -p camel-bridge --test bridge_exit_path_evidence leak_parent_then_exit -- --ignored --nocapture 2>&1 | cat`
- pipeline: TIMED OUT after 90s (exit 124)
- timeout exit code: 124
- live token orphans at cancel: 2

## Orphans at cancel (`pgrep -af m390-1792831142536683199`)

```text
2180034 /bin/sh /home/shared/tmp/.tmpdLaiku/m390-1792831142536683199-fake-bridge-2180032-1792831151708943910.sh
2180035 sh -c trap "" TERM; echo armed > '/home/shared/tmp/.tmpdLaiku/gc-marker'; while :; do sleep 1; done m390-1792831142536683199-fake-bridge-2180032-1792831151708943910-gc
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
   Compiling tonic v0.14.6
   Compiling reqwest v0.13.5
   Compiling camel-bridge v0.60.0 (/home/shared/rust-camel-worktrees/390-bridge-child-lifetime/crates/services/camel-bridge)
    Finished `test` profile [unoptimized] target(s) in 9.14s
     Running tests/bridge_exit_path_evidence.rs (target/debug/deps/bridge_exit_path_evidence-af99f1a75a8dd30f)

running 1 test
fake-bridge stderr alive

thread 'leak_parent_then_exit' (2180031) panicked at crates/services/camel-bridge/tests/bridge_exit_path_evidence.rs:290:13:
orphan survivors for m390-1792831142536683199-fake-bridge-2180032-1792831151708943910 after inner exit (exit status: 0): [2180034, 2180035] — bridge children outlived the test process
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test leak_parent_then_exit ... FAILED

failures:

failures:
    leak_parent_then_exit

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.11s

error: test failed, to rerun pass `-p camel-bridge --test bridge_exit_path_evidence`
```

## Verdict

**HANG REPRODUCED**
