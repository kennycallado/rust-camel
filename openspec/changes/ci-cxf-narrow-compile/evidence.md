# Evidence: ci-cxf-narrow-compile

## Provenance

- Worktree: `/home/shared/rust-camel-worktrees/386-ci-cxf-narrow`
- Branch: `feature/ci-cxf-narrow`
- Base: `6c4ab151c`
- HEAD at task start: `fba6fe38505d653da24837bc8ba1eb3d6f7e8f49`
- Date: 2026-10-23
- Command template (baseline, sabotage, restore):

```sh
systemd-run --user --unit="$unit" --collect --wait --pipe \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp \
  --setenv=PATH="$PATH" \
  --working-directory=/home/shared/rust-camel-worktrees/386-ci-cxf-narrow \
  -- cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run
```

- `unit` is `386-narrow-baseline`, `386-narrow-sabotage`, or
  `386-narrow-restore`.
- `--wait --pipe` makes each invocation synchronous. All exit codes below are
  Cargo's forwarded exit status, not the systemd unit start status.
- Environment note: the systemd user manager PATH does not include the nix
  sccache wrapper path. The first invocation failed after 76 ms with exit 101
  and `could not execute process 'sccache rustc -vV' ... No such file or
  directory`, before compiling anything. All recorded runs pass
  `--setenv=PATH="$PATH"` so `RUSTC_WRAPPER=sccache` resolves. No sccache
  exit-254 failure occurred, so no empty-`RUSTC_WRAPPER` retry was needed.
- Cache state: warm `target/` (3.1 GB) from prior builds in this worktree.
  `CARGO_INCREMENTAL` is unset locally; CI sets it to `"0"` (`ci.yml:13`).

## (a) Inventory

Jobs in `.github/workflows/*.yml` that compile `camel-test` with any
integration feature:

| Workflow / job | Trigger | Feature scope | Ref |
| --- | --- | --- | --- |
| `ci.yml` / `full-tests-linux` | push `main`, PR | wide `integration-tests` test targets (jms, xml, cxf, container, kafka, component-emission, redis, redis-repositories, cache-payload-offload, opensearch, kubernetes, auth-multi-credential, kernel-fail-closed) | `ci.yml:107`, `:119`, `:129`, `:135`, `:137`, `:139`, `:141`, `:143`, `:145`, `:147`, `:149`, `:151`, `:153` |
| `ci.yml` / `full-tests-linux` | push `main`, PR | wide compile gate `--features integration-tests --tests` | `ci.yml:169` |
| `ci.yml` / `full-tests-linux` | push `main`, PR | **narrow `integration-tests-cxf`** compile gate added by this change | `ci.yml:170-179` |
| `ci.yml` / `quality` | push `main`, PR | clippy `--workspace --all-features`, which activates `integration-tests-cxf` inside a feature union | `ci.yml:303` |
| `published-bridge-smoke.yml` / `smoke` | release `workflow_run` (`cxf-bridge-v*`) or manual dispatch — **not a PR gate** | narrow `integration-tests-cxf` build and e2e run | `published-bridge-smoke.yml:151`, `:172` |

Conclusion: before the step added by Task 1.1, no PR job compiled the narrow
`integration-tests-cxf` feature in isolation. Every PR job used a larger
feature union (`--features integration-tests` or `--all-features`) that also
activates `dep:camel-component-cxf`, so a defect confined to the narrow feature
line could not be observed until the post-release smoke.

## (b) Diff

Step added to `full-tests-linux` by Task 1.1 (`ci.yml:170-179`):

```yaml
      - name: Compile-gate narrow cxf integration feature
        # bd rc-z1ea2: --all-features clippy and the wide
        # --features integration-tests steps activate dep:camel-component-cxf
        # as part of a wider union, so neither can catch a defect confined to
        # the integration-tests-cxf feature line. Without this gate, a compile
        # break in the narrow cxf_test target stays invisible to CI.
        # This command mirrors the compile the published-bridge smoke runs
        # (compile + link, no execution). No Docker or runtime is required,
        # keeping the gate cheap.
        run: cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run
```

## (c) Cost

- Measured local baseline: **173 s wall clock** (systemd service runtime
  `2min 53.199s`), warm target dir, 29 min 3.8 s CPU time.
- Mission 377 cold narrow no-run compile: **449.262 s** — provenance only,
  historical, not an upper bound for CI.
- The step runs inside the existing `full-tests-linux` 120-minute timeout
  (`ci.yml:73`).

## (d) Sabotage

Baseline (`386-narrow-baseline`):

```text
BASELINE_EXIT=0  ELAPSED_S=173
Finished `test` profile [unoptimized] target(s) in 2m 53s
Executable tests/cxf_test.rs (target/debug/deps/cxf_test-fa389289d01a6970)
```

Sabotage (`386-narrow-sabotage`): working-tree-only change on line 122 of
`crates/camel-test/Cargo.toml`, never staged:

```diff
-integration-tests-cxf = ["dep:camel-component-cxf"]
+integration-tests-cxf = []
```

Result: **exit 101** after 35 s. Rustc diagnostics quote:

```text
error[E0433]: cannot find module or crate `camel_component_cxf` in this scope
  --> crates/camel-test/tests/cxf_test/tls.rs:19:5
   |
19 | use camel_component_cxf::config::{CxfSecurityFields, CxfTlsConfig};
   |     ^^^^^^^^^^^^^^^^^^^ use of unresolved module or unlinked crate `camel_component_cxf`

error[E0432]: unresolved import `camel_component_cxf`
  --> crates/camel-test/tests/cxf_test/tls.rs:20:5
   |
20 | use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
   |     ^^^^^^^^^^^^^^^^^^^ use of unresolved module or unlinked crate `camel_component_cxf`

error: could not compile `camel-test` (test "cxf_test") due to 25 previous errors
```

Restore proof (`386-narrow-restore`): `git checkout --
crates/camel-test/Cargo.toml`, then `git status --porcelain
crates/camel-test/Cargo.toml` returned empty (recorded as `PORCELAIN=[]` both
before and after the re-run). Re-run result: **exit 0**, 0 s wall clock
(509 ms systemd service runtime; baseline artifacts still valid). No commit in
this task touches `crates/camel-test/Cargo.toml`.

## (e) Gates

| Gate | Result | Evidence |
| --- | --- | --- |
| fmt / clippy / doc-build / cargo-audit / schema-check / Rust `lint-*` | N/A | Committed diff contains no `*.rs` or `Cargo.toml` files |
| `cargo xtask lint-secrets` | run, exit 0 | `/tmp/386-lint-secrets.log` |
| `cargo xtask lint-ignore` | run, exit 0 | `/tmp/386-lint-ignore.log` |
| `cargo xtask lint-single-source` | run, exit 0 | `/tmp/386-lint-single-source.log` |
| `cargo xtask lint-context-citations` | run, exit 0 | `/tmp/386-lint-context-citations.log` |
| YAML syntax | run, exit 0 | No `actionlint` or `yamllint` exists in the repo. `bun` `Bun.YAML` parsed all 19 workflow files and asserted the narrow step name and `run` string: `YAML_PARSE_OK files=19 narrow_step=present` |
