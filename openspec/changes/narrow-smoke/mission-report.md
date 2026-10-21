# Mission 377: implementation evidence

## Specification gate

The final F5 re-bless returned BLESSED for hash
`sha256:0f0861a1c3fc40e69bd5d60d44f7560f392c5170306ae1dee429148133bb577e`.
The plan returned BLESS-WITH-FIXES for hash
`sha256:13892514a227e6523bbf78d308b0c7130f101dd9f37d6f2fd30186fca580339c`.
Its only fix selected an installed YAML parser (Bun.YAML); the expert explicitly
required no re-bless, and the owner authorized a one-round wording waiver.
That fix was applied. Tasks 1.1, 1.2 and 2.1 are the implemented scope.

## Branch and state

- Issue: rc-pqj93; remains in progress. No bd close, merge, archive, version bump, or push.
- Branch: `chore/narrow-smoke`.
- Worktree: `/home/shared/rust-camel-worktrees/377-narrow-smoke`.
- Rebased HEAD: `6f8234d476d59813b1ee9827e913bc64974b8ae0` on main
  `384b54aba6ee2e6c98cdaacc39cb7f2503afb23b`.
- Rebase completed without conflicts. Root `Cargo.toml` and `Cargo.lock`
  remain unchanged from main: wasmtime and wasmtime-wasi retain 49.0.4.
  CXF/JMS `BRIDGE_VERSION` constants and both Gradle default versions retain
  0.8.4, byte-identical to main.
- Task 1.1 feature code (`integration-tests-cxf`, CXF gate and shared-support
  broadening) was reviewed by r_gpt with APPROVE.
- `.github/workflows/published-bridge-smoke.yml` is edited in Task 2.1:
  narrow feature selector, timeout `150` -> `60`, header comment update.
- Workflow comparison baseline is main
  `384b54aba6ee2e6c98cdaacc39cb7f2503afb23b` after the documented rebase, not
  the older `92c683573a2157685e64dce423f3f76bc06e6ea7`. For this file the two
  baselines are byte-identical: `git diff 92c68357 384b54ab --
  .github/workflows/published-bridge-smoke.yml` is empty.

## Measurement environment

Recorded at `2026-10-21T06:01:51Z` in
`/home/shared/tmp/rc377-env-before.log`.

- CPU: AMD Ryzen 5 5600G, `nproc=12`, `CARGO_BUILD_JOBS=6`.
- Toolchain: stable, `rustc 1.98.1 (48a229cea 2026-09-01)`,
  `cargo 1.98.1 (797e8a9bc 2026-08-05)`, `sccache 0.15.0`.
- Containment for every battery:
  `systemd-run --user --scope --collect --unit=rc377-NAME`
  with `MemoryMax=12G`, `MemorySwapMax=2G`, `TasksMax=1024`,
  `OOMPolicy=kill`, `TMPDIR=/home/shared/tmp`.
- Wrapper state before override: shell had `RUSTC_WRAPPER=sccache` and
  `~/.cargo/config.toml` had `[build] rustc-wrapper = "sccache"` plus
  `jobs = 6`. `RUSTC_WORKSPACE_WRAPPER` was unset.
- Cold and full commands explicitly overrode all wrappers:
  `RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER=`.
  A separate trivial-crate probe under the same override logged
  `Running \`rustc ...\`` with no sccache invocation
  (`/home/shared/tmp/rc377-wrapper-probe.log`). The cold build log contains
  zero `sccache` strings.
- `SCCACHE_DIR=/home/shared/sccache` is host-global and shared with concurrent
  agents, so the counters below include their traffic. Our cold/full units had
  wrappers disabled and added no sccache requests themselves.

| sccache counter | Before cold | After cold | After full |
|---|---:|---:|---:|
| Compile requests | 1225 | 2150 | 4074 |
| Compile requests executed | 1072 | 1718 | 3404 |
| Cache hits | 1053 | 1653 | 3063 |
| Cache misses | 10 | 60 | 336 |
| Compilations | 10 | 60 | 336 |

Logs: `/home/shared/tmp/rc377-sccache-after-cold.log`,
`/home/shared/tmp/rc377-sccache-after-full.log`.

## Cold narrow measurement

Dedicated `target-cold-narrow` was absent, then created and verified empty
before the build. Command, run in unit `rc377-cold`:

```text
CARGO_TARGET_DIR=<worktree>/target-cold-narrow \
RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run
```

- Start: `2026-10-21T06:02:00Z`; end: `2026-10-21T06:09:29Z`.
- Completed wall time: **449.262 s** (7m29s displayed by cargo).
- Exit status: **0**; zero warning/error lines; target size after this stage
  2.4 GiB.
- Evidence: `/home/shared/tmp/rc377-cold.log`.

The compiled narrow target listed 44 tests and 0 benchmarks in 0.49 s
(unit `rc377-list`, cached; `/home/shared/tmp/rc377-list.log`). All three
workflow names are present as real tests, module-qualified as `wss_e2e::...`,
which satisfies the workflow's substring `grep -q` checks:

```text
wss_e2e::cxf_native_wss_signed_call_and_verified_signer: test
wss_e2e::wss_e2e_leg_c_opted_in_profile_accepts_legacy_sha1: test
wss_e2e::wss_e2e_leg_c_default_profile_rejects_legacy_sha1: test
```

Evidence: `/home/shared/tmp/rc377-list-workflow-names.log`. Bridge-dependent
tests were not executed locally.

## Full integration-tests compatibility

Same `target-cold-narrow`, incremental after the narrow build; 30-minute hard
bound (`timeout --signal=TERM --kill-after=30s 1800s`), unit `rc377-full`:

```text
cargo test -p camel-test --features integration-tests --test cxf_test --no-run
```

- Start: `2026-10-21T06:10:39Z`; end: `2026-10-21T06:19:39Z`.
- Wall time: **540.478 s** (9m00s displayed by cargo).
- Exit status: **0**. `BOUNDED_RESULT=COMPLETED`; this run was not stopped and
  is reported as a completed compile. Target grew to 5.6 GiB.
- Evidence: `/home/shared/tmp/rc377-full.log`.

## Gate results

- `cargo test -p camel-cli --test feature_profiles` (ordinary target, unit
  `rc377-feature-profiles`): exit **0**, 19 passed, 0 failed, 253 s total
  (4m06s compile, 7.25 s tests). The golden was not regenerated.
  Log: `/home/shared/tmp/rc377-feature-profiles.log`.
- `cargo fmt --check --all` after rebase (unit `rc377-fmt`): exit **0**, 5 s.
  Log: `/home/shared/tmp/rc377-fmt.log`.
- `cargo clippy -p camel-test --all-targets --features integration-tests-cxf
  -- -D warnings` after rebase (unit `rc377-clippy`): exit **0**, 7 s.
  Log: `/home/shared/tmp/rc377-clippy.log`.

## Residual unconditional dependencies

The narrow feature avoids the optional `integration-tests` dependencies, but
unconditional `[dev-dependencies]` of `camel-test` still enter the test build
(`cargo tree -p camel-test --features integration-tests-cxf`, unit
`rc377-tree`, `/home/shared/tmp/rc377-tree.log`):

- `kube v4.2.0` via `camel-config` (dev-dependency with `kubernetes`) ->
  `camel-platform-kubernetes`.
- `sqlx v0.8.6` via `camel-component-sql` dev-dependency.
- `bollard v0.20.2` via `camel-component-container` and `camel-function`
  dev-dependencies.

These are why narrow compilation is still substantial and are acknowledged as
residual cost, not narrowed away in Task 1.1.

## Task 2.1: workflow selection and timeout

The completed cold narrow build is 449.262 s, at most 1500 s. Under the Task
2.1 rule this supports reducing `timeout-minutes` from 150 to 60.

Changed in `.github/workflows/published-bridge-smoke.yml`:

- Smoke feature selector `integration-tests` -> `integration-tests-cxf`.
- `jobs.smoke.timeout-minutes` `150` -> `60`.
- Header comment now names `integration-tests-cxf` and removes the previous
  unsupported "~10-20 min cold" runtime estimate. It records the local measured
  449.262 s input and that the 60-minute budget leaves headroom for CI setup,
  artifact download, and the e2e tests.

Preserved byte-for-byte: the download and checksum step, the `env` block and
`cargo test` filters, and all three test-name grep assertions.

YAML parser check (Bun 1.3.13):

```text
$ bun -e 'const y=Bun.YAML.parse(await Bun.file(".github/workflows/published-bridge-smoke.yml").text()); console.log(y.jobs.smoke["timeout-minutes"])'
60
```

The parser and scope checks also recorded:

```text
YAML_PARSE=ok
TIMEOUT=60
SELECTOR=integration-tests-cxf
BUN_EXIT=0
```

Diff scope check against baseline main `384b54ab`. `diff -u` shows exactly
three hunks: header comment, timeout, feature selector. A Bun scope script
(`/home/shared/tmp/rc377-scope-check.js`) additionally asserts:

```text
download_step_bytes_identical=PASS
three_grep_names_bytes_identical=PASS
three_grep_names_exact=PASS
env_filter_bytes_identical=PASS
filter_line_bytes_identical=PASS
checksum_line_bytes_identical=PASS
changed_lines_only_allowed=PASS
SCOPE_CHECK=PASS
SCOPE_EXIT=0
```

No build was rerun for this task, and the test design is unchanged.

## Measurement clock disclosure

The Task 1.2 runner measured with `start=$(date +%s.%N)`, `end=$(date
+%s.%N)` and `awk` subtraction (`/home/shared/tmp/rc377-cold-run.sh`,
`/home/shared/tmp/rc377-full-run.sh`). That is wall-clock realtime, not a
monotonic clock; a system clock adjustment during a run could bias the elapsed
value. No monotonic measurement is claimed.

Reported runner wall-clock deltas against Cargo's independently displayed,
rounded durations:

| Run | Runner realtime delta | Cargo display |
|---|---:|---:|
| Cold narrow | 449.262 s | 7m29s |
| Full incremental | 540.478 s | 9m00s |

The Cargo lines corroborate the realtime deltas at one-second rounding. The
timeout decision uses 449.262 s against the 1500 s threshold, a margin far
larger than any plausible clock-step error. The reviewer confirmed that this
documentation finding requires no rebuild. Future measurements should use a
monotonic source (for example `clock_gettime(CLOCK_MONOTONIC)` or a runtime
monotonic clock) instead of realtime subtraction.

## Verification deferred and claims avoided

- No bridge-dependent tests were executed locally.
- No completed CI smoke run is claimed; only local compile and listing
  evidence exists.
- No speedup ratio is claimed against historical CI timeouts or wall times.
- `target-cold-narrow` is removed after evidence collection. The ordinary
  worktree `target` is retained.

## Conductor gate rerun

Rerun at `2026-10-21T06:54:41Z` on HEAD
`6f8234d476d59813b1ee9827e913bc64974b8ae0`, branch `chore/narrow-smoke`.
Every executed battery ran from this worktree against the ordinary primed
`target/` under
`systemd-run --user --scope --collect --unit=rc377-gate-<NAME>` with
`MemoryMax=12G`, `MemorySwapMax=2G`, `TasksMax=1024`, `OOMPolicy=kill`,
`TMPDIR=/home/shared/tmp` and `CARGO_BUILD_JOBS=6`. Each scope was stopped
after its command. Logs are
`/home/shared/tmp/rc377-gate-<NAME>.log`.

| Gate | Command | Exit | Wall (s) | Log |
|---|---|---:|---:|---|
| fmt | `cargo fmt --check --all` | 0 | 4.7 | `rc377-gate-fmt.log` |
| build | `cargo build --workspace` | 0 | 402.8 | `rc377-gate-build.log` |
| test-lib | `cargo test --workspace --lib` | 0 | 373.7 | `rc377-gate-test-lib.log` |
| test-hex | `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` | 0 | 56.6 | `rc377-gate-test-hex.log` |
| clippy-workspace | `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` | 0 | 120.9 | `rc377-gate-clippy-workspace.log` |
| clippy-kafka | `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` | 0 | 10.1 | `rc377-gate-clippy-kafka.log` |
| clippy-cli | `cargo clippy -p camel-cli -- -D warnings` | 0 | 51.1 | `rc377-gate-clippy-cli.log` |
| clippy-cli-nodefault | `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings` | 0 | 27.4 | `rc377-gate-clippy-cli-nodefault.log` |
| lint-unwrap | `cargo xtask lint-unwrap` | 0 | 4.6 | `rc377-gate-lint-unwrap.log` |
| lint-secrets | `cargo xtask lint-secrets` | 0 | 3.7 | `rc377-gate-lint-secrets.log` |
| lint-single-source | `cargo xtask lint-single-source` | 0 | 0.5 | `rc377-gate-lint-single-source.log` |
| lint-non-exhaustive | `cargo xtask lint-non-exhaustive` | 0 | 0.4 | `rc377-gate-lint-non-exhaustive.log` |
| lint-commits | `cargo xtask changelog --check --from FETCH_HEAD --to HEAD` | SKIPPED | - | - |
| lint-log-levels | `cargo xtask lint-log-levels` | 0 | 0.9 | `rc377-gate-lint-log-levels.log` |
| lint-log-redaction | `cargo xtask lint-log-redaction` | 0 | 1.6 | `rc377-gate-lint-log-redaction.log` |
| lint-cancel-tokens | `cargo xtask lint-cancel-tokens` | 0 | 0.5 | `rc377-gate-lint-cancel-tokens.log` |
| lint-test-sleep | `cargo xtask lint-test-sleep` | 0 | 2.3 | `rc377-gate-lint-test-sleep.log` |
| lint-unbounded-wait | `cargo xtask lint-unbounded-wait` | 0 | 3.4 | `rc377-gate-lint-unbounded-wait.log` |
| lint-ignore | `cargo xtask lint-ignore` | 0 | 0.7 | `rc377-gate-lint-ignore.log` |
| lint-publish-cycles | `cargo xtask lint-publish-cycles` | 0 | 0.4 | `rc377-gate-lint-publish-cycles.log` |
| lint-publish-registration | `cargo xtask lint-publish-registration` | 0 | 0.4 | `rc377-gate-lint-publish-registration.log` |
| lint-component-deps | `cargo xtask lint-component-deps` | 0 | 0.5 | `rc377-gate-lint-component-deps.log` |
| lint-gate-forwarding | `cargo xtask lint-gate-forwarding` | 0 | 0.4 | `rc377-gate-lint-gate-forwarding.log` |
| lint-context-citations | `cargo xtask lint-context-citations` | 0 | 2.6 | `rc377-gate-lint-context-citations.log` |
| lint-metric-labels | `cargo xtask lint-metric-labels` | 0 | 1.4 | `rc377-gate-lint-metric-labels.log` |
| schema-check | `cargo xtask schema --check` | 0 | 1.4 | `rc377-gate-schema-check.log` |
| doc-build | `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps` | 0 | 15.2 | `rc377-gate-doc-build.log` |
| doc-guide-test | `cd docs && mdbook test` | N/A | - | - |
| cargo-audit | `cargo audit` | **1** | 1.9 | `rc377-gate-cargo-audit.log` |

### Gate exceptions and failure

- `lint-commits` was skipped because the conductor prohibited a remote fetch.
  The gate needs `FETCH_HEAD` from `git fetch origin main --depth=1`. No result
  is claimed.
- `doc-guide-test` is not applicable: no `docs/**` file changed.
- `cargo-audit` failed with exit 1. It reported three vulnerabilities:
  `rustls-webpki 0.103.15` (`RUSTSEC-2026-0382`, fixed in `>=0.103.16`),
  `webpki-root-certs 1.0.9` (`RUSTSEC-2026-0383`, fixed in `>=1.0.10`) and
  `webpki-roots 1.0.9` (`RUSTSEC-2026-0381`, fixed in `>=1.0.10`). It also
  reported four allowed unmaintained warnings (`atomic-polyfill`, `bincode`,
  `paste`, `rustls-pemfile`). `Cargo.lock` is byte-identical to HEAD
  (`sha256:ec27fde1de7dc42f2975e9c352fd4f6e86b70a3cb97aa9f899a94da5e6f98199`),
  so this mission changed no dependency. The failure was not fixed because it
  is unrelated to the mission and the task forbids implementation fixes.
- Baseline verification: from this feature worktree, the same contained
  `cargo audit --file /home/kenny/dev/rust-camel/Cargo.lock` exited 1 with
  the same three advisories. Main's lockfile is unmodified and byte-identical
  to the feature lockfile. No cargo command ran with main as its working
  directory. Follow-up `rc-xjwwk` tracks this pre-existing failure; the
  cargo-audit gate is exempted for this mission, not recorded as green.

No full `cargo test --workspace` was run. The conductor requested only
`--lib` and the hexagonal boundary test. The executed set is 27 gates: 26
passed, 1 failed, with `lint-commits` skipped and `doc-guide-test` not
applicable.

## Review verdicts

- Task 1.1: r_gpt APPROVE after rebase; additive one-dependency feature,
  unchanged full feature list, minimal support gates, and preserved bridge pins.
- Task 1.2: r_gpt approved the timeout decision. Its timing-method disclosure
  finding was resolved in this report; no rebuild was required by the reviewer.
- Task 2.1: r_gpt APPROVE, no findings. Independently checked the selector,
  threshold, YAML parsing, and exact baseline equality after only the allowed
  header, timeout and selector replacements. Artifact acquisition, checksum,
  environment, filters, and all three grep assertions remain byte-identical.
- Fresh holistic review: r_gpt APPROVE, no findings. Reviewed the complete
  implementation and reports against main 384b54ab, including cross-task
  consistency, documentation impact, measured evidence, and gate accounting.
  OpenSpec validation and `git diff --check` also passed after all task updates.

## pending_for_master

- Review the parked branch and arrange the published-binary smoke in CI.
  Local compilation does not verify the three bridge-dependent executions;
  integration-verification-deferred-to-CI.
- Track the unrelated cargo-audit failure in `rc-xjwwk`.
- Any later integration, archive, rc-pqj93 closure, or push belongs to the
  master/human. None was performed in this mission.
