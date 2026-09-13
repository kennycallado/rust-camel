# Tasks: bootfreshness

## camel-integration-test

### Task 1.1: Make the boot freshness fixture test-unique and document the proof

**Files:**
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. Replace the hard-coded `memdb_isolation_probe` name in
   `named_shared_memory_uri_dies_with_its_boot` with a unique name derived
   from that test's temporary project identity, while keeping the exact same
   URI for boot A and boot B.
2. Preserve the real `boot_scenario` composition root, datasource catalog
   teardown call, and assertion that boot B sees exactly one row after its own
   seed; do not add a delay or replace the SQL pool with a mock.
3. Update the integration-test context to state that the fixture's
   happens-before proof is awaited shutdown plus the landed SQL pool drain,
   and that the per-test name prevents sibling tests from sharing an alias.
4. Add comments at the assertion site naming the distinct SQLite
   named-memory teardown class and excluding JWKS cooldown, HTTP readiness,
   and macOS errno as unrelated classes.

**Tests:**
- `named_shared_memory_uri_dies_with_its_boot`: arrange a temporary project
  with a name-unique `sqlite:file:<name>?mode=memory&cache=shared` URI; act by
  booting A, seeding `a`, awaiting shutdown, booting B, seeding `b`, and
  counting; assert the count is exactly `1` and the test contains no sleep;
   command `cargo test -p camel-integration-test -F sql boot_scenario_test::boot_freshness::named_shared_memory_uri_dies_with_its_boot -- --exact --test-threads=1`; expected: passes before and after the fixture hardening.
- `second_boot_over_same_memory_alias_starts_empty`: arrange the existing
   bare shared-memory fixture; act through two awaited boots; assert boot B
   does not observe boot A's row; command `cargo test -p camel-integration-test -F sql boot_scenario_test::boot_freshness::second_boot_over_same_memory_alias_starts_empty -- --exact`; expected: passes.
- `boot_scenario_test module`: arrange all boot freshness fixtures; act by
  running the module with the normal test scheduler; assert the datasource
  close, named-memory, bare-memory, and file-backed scenarios all pass;
   command `cargo test -p camel-integration-test -F sql boot_scenario_test`; expected:
  exit 0.

**Acceptance:**
- The named-memory URI is unique per test invocation and identical across
  its two boots.
- Both boot freshness tests pass with `--test-threads=1` and default test
  threading.
- `CONTEXT.md` contains the no-sleep determinism argument and known-class
  distinction in English.

- [x] 1.1

## Verification

### Task 2.1: Exercise teardown determinism under repetition and load

**Files:**
- `openspec/changes/bootfreshness/proposal.md` (modified)
- `openspec/changes/bootfreshness/design.md` (modified)
- `openspec/changes/bootfreshness/specs/integration-tier/spec.md` (modified)

**Steps:**
1. Record the determinism mechanism and harness-isolation ruling in the
   design: sqlx named-memory databases remain alive while any connection
   remains open, and the landed pool close path prevents resurrection and
   waits for observable drain; the fixture URI is unique to its test.
2. Record the verification matrix in the proposal: serial focused runs,
   default-thread runs, and 20 repeated runs while another cargo build keeps
   scheduler pressure; state that a failure is a teardown error or a row
   mismatch, never a sleep timeout.
3. Keep the delta spec aligned with the existing timeout policy and all five
   scenarios in the MODIFIED datasource teardown requirement: four landed
   scenarios plus the close-before-next-boot scenario.
4. Run the focused test repeatedly and capture pass counts in the task review
   evidence; do not weaken assertions or convert the test to a file-backed
   database.

**Tests:**
- `boot freshness repetition`: arrange the integration-test executable with
   `cargo test -p camel-integration-test -F sql --no-run`, start an independent
   workspace build with `CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/bootfreshness/load-target cargo build --workspace --tests & load_pid=$!`, and resolve the executable path with `ls -t /home/shared/rust-camel-worktrees/bootfreshness/target/debug/deps/camel_integration_test-*`; act by invoking that raw test binary 20 times without a filter (the full crate suite) while the independent build runs; assert zero boot-B-counts-boot-A failures and a successful load build; command `cargo test -p camel-integration-test -F sql --no-run || exit 1; CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/bootfreshness/load-target cargo build --workspace --tests & load_pid=$!; bin=$(ls -t /home/shared/rust-camel-worktrees/bootfreshness/target/debug/deps/camel_integration_test-* | grep -v '\.d$' | head -1); "$bin" --list | grep -q 'boot_scenario_test::boot_freshness::named_shared_memory_uri_dies_with_its_boot' || exit 1; for i in $(seq 1 20); do "$bin" || { kill $load_pid 2>/dev/null; exit 1; }; done; wait $load_pid`; expected: exit 0. If the fresh load target cannot fit the available disk, record the ENOSPC result and use the documented whole-workspace `cargo check --workspace --tests` load substitute; do not claim the literal build ran.
- `boot freshness parallel-thread check`: arrange the same binary with the
  normal libtest scheduler; act by running the focused test without
  `--test-threads=1`; assert it passes and reports no leaked row; command
   `cargo test -p camel-integration-test -F sql boot_scenario_test::boot_freshness::named_shared_memory_uri_dies_with_its_boot -- --exact`; expected: exit 0.

**Acceptance:**
- The focused test passes 20 consecutive default-thread runs with zero
  leakage under the documented load condition.
- `openspec validate bootfreshness --type change --json` reports valid.
- No task changes the landed datasource close timeout policy without a fresh
  spec review.

- [x] 2.1
