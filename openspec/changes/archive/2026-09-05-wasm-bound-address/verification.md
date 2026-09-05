# Verification: wasm-bound-address (Task WASM-3)

- Date: 2026-09-05
- Branch: `feature/wasm-bound-address`, worktree
  `/home/shared/rust-camel-worktrees/wasm-bound-address`, base HEAD `220ebcfe`
  (WASM-2 tick; WASM-1 `72012ba0`, WASM-2 `af9f7c0c` landed).
- Every command ran inside the worktree. Nothing ran in the main checkout.

## Gate battery

| Gate | Command | Exit | Notes |
|---|---|---|---|
| fmt | `cargo fmt --check --all` | 0 | |
| clippy (workspace) | `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` | 0 | |
| clippy (kafka) | `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` | 0 | |
| clippy (cli) | `cargo clippy -p camel-cli -- -D warnings` | 0 | first run exit 101, infra, see reruns below |
| lint-unwrap | `cargo xtask lint-unwrap` | 0 | |
| lint-secrets | `cargo xtask lint-secrets` | 0 | |
| lint-non-exhaustive | `cargo xtask lint-non-exhaustive` | 0 | |
| lint-log-levels | `cargo xtask lint-log-levels` | 0 | |
| lint-ignore | `cargo xtask lint-ignore` | 0 | first run exit 1, see reruns below |
| lint-publish-cycles | `cargo xtask lint-publish-cycles` | 0 | |
| lint-component-deps | `cargo xtask lint-component-deps` | 0 | |
| lint-gate-forwarding | `cargo xtask lint-gate-forwarding` | 0 | |
| lint-context-citations | `cargo xtask lint-context-citations` | 0 | `OK (0 violations)` |
| lint-metric-labels | `cargo xtask lint-metric-labels` | 0 | |
| schema | `cargo xtask schema --check` | 0 | |
| build | `cargo build --workspace` | 0 | 11m49s |
| test (libs) | `cargo test --workspace --lib` | 0 | |
| hexagonal architecture | `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` | 0 | |
| audit | `cargo audit` | 0 | `5 allowed warnings found` — allowed set unchanged |
| lint-commits | `cargo xtask changelog --check --from FETCH_HEAD --to HEAD` | SKIPPED | conductor policy for this task: remote op (`git fetch origin main`) |

### Reruns (both in-scope, both recorded)

1. **lint-ignore exit 1 on first run.** WASM-1 added
   `tests/staged_listener_source.rs` with three `requires pre-built` tests but
   no ADR-0054 allowlist entry. Fix: added the path to
   `scripts/xtask/allowlist-ignore.txt` (the file holds only `requires
   pre-built` tests, so it meets the allowlist contract). Rerun: exit 0.
2. **clippy `-p camel-cli` exit 101 on first run.** Infrastructure, not a lint
   finding. The detached runner inherited a `TMPDIR` pointing at a deleted
   sandbox directory, so the `camel-xslt` prost build script could not create
   its temp dir (`No such file or directory` under `/tmp/.ctx-mode-*`). Same
   failure class as the documented sccache dead-TMPDIR quirk. Rerun with
   `TMPDIR=/tmp`: exit 0. The workspace and kafka clippy runs completed before
   the directory was removed and needed no rerun.

## Per-target wasm test evidence

Full-crate run `cargo test -p camel-component-wasm`, exit 0:

| Target | Result | Pre-built gated |
|---|---|---|
| lib (unit tests) | 178 passed, 0 failed | 0 |
| `staged_listener_source` (new, WASM-1) | 0 run, 3 ignored | 3 of 3 |
| `source_integration` | 0 run, 6 ignored | 6 of 6 |
| `source_stream_integration` | 0 run, 7 ignored | 7 of 7 |
| `source_auth_e2e` | 0 run, 9 ignored | 9 of 9 |
| `source_bind_gate` | 0 run, 6 ignored | 6 of 6 |
| `integration` | 14 passed, 1 ignored | 1 |
| `bean_streaming_integration` | 6 passed | 0 |
| `hardening` | 10 passed | 0 |
| `perf_bench` | 1 passed | 0 |
| `return_stream_integration` | 5 passed | 0 |
| `security_policy` | 3 passed | 0 |
| `security_policy_init_config` | 2 passed | 0 |
| `source_auth` | 3 passed | 0 |
| `state_persistence` | 6 passed | 0 |
| `streaming_integration` | 9 passed | 0 |

- The lib run includes the three `staged_listener` unit tests from WASM-1
  (`stage_take_exact_key_roundtrip`, `duplicate_staging_rejected_first_stays`,
  `wrong_host_take_conflicts_and_preserves`), all green.
- Migrated staging sites (WASM-2): `source_integration` 6,
  `source_stream_integration` 7, `source_auth_e2e` 9, `source_bind_gate` 6.
  The one remaining former probe site, `port_b` in
  `conflicting_binds_fail_before_socket`, is now the fixed address
  `127.0.0.1:1` (`grep -c 'port_b = 1u16'` matches exactly once, with the
  explanatory comment).
- `source_bind_gate --ignored` evidence from WASM-2 (local): the
  conflicting-bind guest fixture was built locally before the suite ran —
  `tests/fixtures/conflicting-bind-guest/target/wasm32-wasip2/debug/
  conflicting_bind_guest.wasm`, built 2026-09-05 19:25, before the WASM-2 tick
  commit at 20:12. That opportunistic `--ignored` run is what surfaced the
  ERRATUM below. CI re-validates (see CI-deferred).

## No-probe greps (MODIFIED scenario, both halves)

- `grep -rn 'free_port' crates/components/camel-component-wasm/` — exit 1,
  zero matches (no calls, no definitions).
- `grep -rn find_free_port crates/camel-test/` — exit 1, zero matches
  (rc-h0aw state preserved).

## Helper accounting

- `grep -rn 'stage_wasm_source_listener(' crates/components/camel-component-wasm/tests/ | grep -c '('` — **32**
  (28 migration call sites + 1 definition in `tests/common/mod.rs` + 3 WASM-1
  test calls; `unstaged_bind_starts` deliberately uses no helper).
- Raw mentions of `stage_wasm_source_listener`: 36 (the 32 above + 3 helper
  panic-message lines + 1 doc-comment self-mention).

## ERRATUM — exception to the blanket 127.0.0.1 staging rule

Worker-discovered during the WASM-2 opportunistic `--ignored` run;
r_glm-endorsed; recorded in tasks.md (WASM-2 erratum) and the `af9f7c0c`
commit message. The blanket "stage under 127.0.0.1" rule has one exception:
`non_loopback_public_gate_with_and_without_ack` phase 2 binds `0.0.0.0:{port}`
WITH the `allow_public_exposure` ack. That bind passes the exposure gate and
reaches the bind site, so it stages under `0.0.0.0`. Exact host-string keys,
no normalization (design.md): `0.0.0.0` and `127.0.0.1` are different keys.

## Review trail

- Spec blessing: 3 + 2 rounds; final hash `cf25a381…`.
- Plan blessing: 5 rounds, including one REJECT that required a fix to the
  conflicting-bind guest semantics; blessed plan landed as `c07733ba`.
- Task reviews (r_glm):
  - WASM-1: APPROVE, clean.
  - WASM-2: APPROVE with 2 documentation minors — both resolved
    (`220ebcfe`).

## CI-deferred

Per ADR-0054 (pre-built guest wasm fixtures):

- `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored`
  (full 6-test suite; compile-checked locally, `--ignored` run re-validated in
  CI).
- The fixture-gated tests of `tests/staged_listener_source.rs` (3 tests; CI
  compiles the binary and reports them ignored).
- The fixture-gated tests of the other three source binaries
  (`source_integration`, `source_stream_integration`, `source_auth_e2e`) and
  the one gated test in `integration.rs`.
