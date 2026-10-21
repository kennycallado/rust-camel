# Tasks: narrow-smoke

## camel-test

### Task 1.1: Add the narrow feature and verify its compilation scope

**Files:**
- `crates/camel-test/Cargo.toml` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/support/mod.rs` (modified)

**Steps:**
1. Run the narrow no-run command before edits and record the missing-feature failure as the red check.
2. Add `integration-tests-cxf = ["dep:camel-component-cxf"]` without changing the full integration-tests feature list.
3. Change the CXF target gate to `any(feature = "integration-tests", feature = "integration-tests-cxf")`.
4. Broaden only the shared CXF module, bridge runtime, crypto initialization, direct-send helper imports and functions, and associated helper tests needed by CXF. Keep unrelated support modules full-feature-only. Add a local dead-code allowance for bridge_bg_rt if narrow all-targets clippy needs it, matching neighboring support helpers.
5. Run fmt and narrow all-targets clippy in the worktree's ordinary target directory. Do not touch dependency versions, unconditional dev-dependencies, or unrelated test bodies.

**Tests:**
- `narrow_feature_declared`: setup current manifest; action run `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`; assert the feature exists and the complete target compiles; expected missing-feature failure before edits, pass after edits. The green run belongs to Task 1.2's cold measurement, avoiding duplicate build cost.
- `narrow_all_targets_clippy`: setup modified feature and support; action run `cargo clippy -p camel-test --all-targets --features integration-tests-cxf -- -D warnings`; assert exit 0, including ungated test targets that import shared support; expected pass after edits.
- `format`: action `cargo fmt --check --all`; assert exit 0.

**Acceptance:**
- Full integration-tests list is unchanged; only one optional dependency is enabled by the new feature.
- Fmt and narrow all-targets clippy exit 0.
- Cargo commands run in the worktree under the containment below, never in main.

- [x] 1.1

### Task 1.2: Measure cold narrow compilation and verify full compatibility

**Files:**
- `openspec/changes/narrow-smoke/mission-report.md` (modified)

**Steps:**
1. Verify `target-cold-narrow` does not exist or is empty; use it as CARGO_TARGET_DIR. Record toolchain, CPU, jobs, wrapper settings, and sccache statistics before and after measurement. Disable `RUSTC_WRAPPER` and `CARGO_BUILD_RUSTC_WRAPPER`; also disable `RUSTC_WORKSPACE_WRAPPER` if configured.
2. Time the narrow no-run command with a monotonic wall clock under the containment below. Preserve its command, elapsed seconds, exit status and concise log reference. Record any interruption as a failed attempt, not a completed compilation time. A successful completed cold measurement is required.
3. List the narrow target's tests and assert all three workflow names are present: obtain their exact spelling from the unchanged workflow grep assertions. Do not execute bridge-dependent tests locally.
4. Time the full integration-tests no-run target using the same target directory and containment. Label it incremental after narrow. Bound it at 30 minutes if needed; report elapsed time and exit status even if stopped. Do not describe a stopped run as a pass.
5. Run `cargo test -p camel-cli --test feature_profiles` under containment. Do not change the golden unless the actual CLI dependency closure changed.
6. Replace earlier blocked-state report with measured evidence, residual unconditional dependencies, timeout recommendation, gate results and verification deferred to CI. Remove only this mission's dedicated cold target after evidence collection, not the ordinary target or another worktree's files.

**Tests:**
- `cold_narrow_compile`: setup dedicated empty target and disabled compiler wrappers; action `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`; assert exit 0 with measured elapsed wall seconds; expected pass.
- `smoke_names_present`: setup compiled narrow target; action `cargo test -p camel-test --features integration-tests-cxf --test cxf_test -- --list`; assert each of the three workflow grep names appears as a test, not a zero-test target; expected pass.
- `full_feature_compatible`: setup narrow artifacts; action `cargo test -p camel-test --features integration-tests --test cxf_test --no-run`; assert exit 0; expected pass, or explicitly report a bounded infrastructure interruption as incomplete verification.
- `feature_profiles`: setup unchanged CLI golden; action `cargo test -p camel-cli --test feature_profiles`; assert all tests pass; expected pass.

**Acceptance:**
- Report contains an actual completed cold narrow wall time and a full-feature wall-clock data point with cache classification and exit status.
- Full-feature compile succeeds or its affordable bounded attempt is explicitly reported incomplete, never silently waived.
- feature_profiles passes without unjustified golden regeneration.
- No claimed speedup ratio based on historical CI timeouts. kube, sqlx and bollard remain acknowledged.

- [x] 1.2

## CI

### Task 2.1: Select the narrow smoke and apply the measured timeout decision

**Files:**
- `.github/workflows/published-bridge-smoke.yml` (modified)
- `openspec/changes/narrow-smoke/mission-report.md` (modified)

**Steps:**
1. Change only the smoke feature selector from integration-tests to integration-tests-cxf.
2. If Task 1.2 records a completed cold narrow build of at most 1500 seconds, change timeout-minutes from 150 to 60; otherwise leave 150 and explain why in the report.
3. Update the header comment to name the narrow feature and remove the unsupported cold-runtime estimate. Preserve download, checksum, environment, filters and all three test-name assertions byte-for-byte.
4. Parse the workflow with any installed YAML parser (Bun.YAML is available) and compare its diff to the baseline. Record parser command and result in the report.

**Tests:**
- `workflow_yaml`: setup edited workflow; action parse it with any installed YAML parser, for example `bun -e 'const y=Bun.YAML.parse(await Bun.file(".github/workflows/published-bridge-smoke.yml").text()); console.log(y.jobs.smoke["timeout-minutes"])'`; assert valid YAML and jobs.smoke timeout matches the measured threshold; expected pass after edit.
- `smoke_scope`: setup baseline `git show 92c68357:.github/workflows/published-bridge-smoke.yml` and edited file; action compare changed lines; assert only header comment, feature selector, and justified timeout changed, with download/checksum and all three grep checks unchanged; expected selector failure before edit, pass afterward.
- `timeout_rule`: setup Task 1.2 elapsed value; action inspect timeout; assert 60 for a successful cold run at or below 1500 seconds, otherwise 150; expected pass.

**Acceptance:**
- YAML parses; r_gpt reviews the feature selector, threshold decision and preservation of existing assertions.
- Final report includes measured numbers, accurate incomplete gates, and no claim of a completed CI smoke run.
- Branch is parked; no merge, archive, bd close, version bump, or push.

- [x] 2.1

## Command containment

Every cargo battery runs from `/home/shared/rust-camel-worktrees/377-narrow-smoke` using a unique unit:

`systemd-run --user --scope --collect --unit=rc377-NAME -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo COMMAND`

For the cold build, add the explicit target and disabled wrapper variables after `env`. Always run `systemctl --user stop rc377-NAME.scope` after the command, including failure paths. Never use /tmp. Logs may use `/home/shared/tmp/rc377-NAME.log`; durable results go in mission-report.md and bd notes from the repo root.
