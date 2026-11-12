# Negative control — marker-dependent second-signal test

Proves `job_signal_second_signal_forces_exit` depends on the observed
pipeline-entry seam rather than on timing: with the seam silenced, the
test fails on the marker wait.

## What was done

1. Temporarily commented out the three seam lines in
   `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`
   (the `if std::env::var_os("CAMEL_JOB_PIPELINE_MARKER")` block and its
   `eprintln!`), leaving everything else untouched.
2. Ran the test (expecting failure):

   ```
   cargo test -p camel-cli --test job_signal_test job_signal_second_signal_forces_exit -- --exact
   ```

   Result: exit 101 — `panicked at crates/camel-cli/tests/job_signal_test.rs:317:
   camel job never reached the pipeline-entry seam marker;`
   (`test result: FAILED. 0 passed; 1 failed; ... finished in 30.04s`).

3. Restored the seam with `git checkout --` on that one file (byte-identical
   restore) and verified the empty diff:

   ```
   git -C /home/shared/rust-camel-worktrees/clitesthook diff --exit-code crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs
   ```

   Result: exit 0 (no diff).

4. Re-ran the same test (expecting green):

   ```
   cargo test -p camel-cli --test job_signal_test job_signal_second_signal_forces_exit -- --exact
   ```

   Result: exit 0 (`test result: ok. 1 passed; 0 failed`).

## Conclusion

The test's pass depends on the seam marker being emitted: without it the
INT-after-pipeline-entry synchronization point never arrives and the test
fails. The test is marker-synchronized, not timing-synchronized.

Guard window (observed probe): `Stopping CamelContext` → context stopped
≈ 10 s, still seconds-wide versus the ms-scale TERM latency, so a missed
force-exit would end gracefully and fail `exit == 1`.
