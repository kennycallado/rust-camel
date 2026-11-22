# Proposal: gates-workspace-suites

## Why

`cargo test -p camel-test` and `cargo test --workspace` disagree. Under
`--workspace`, cargo feature unification enables `camel-core/lang-rhai`
(through `camel-cli` and `camel-integration-test`), which changes runtime
behavior: bd rc-lhd0y recorded an `AlreadyRegistered` panic in
`crates/camel-test/tests/dev_watch_script_test.rs` that only reproduces under
unification. The local gate set runs every lane with `-p`, so this class of
regression is invisible locally and only surfaces in CI's full workspace run
(verified in CI run 43560699207). Mission 585 (change `cired`) fixed the test
bugs; bd rc-4xka4 asks for the missing gate.

## What Changes

- `AGENTS.md ## COMPONENT LANES` gains one **declared local** `camel-test`
  row that runs the affected suites under workspace feature unification via
  an explicit suite list (`CAMEL_TEST_WORKSPACE_SUITES`, defaulting to
  `dev_watch_script_test dev_watch_proto_test` — the suites the rc-lhd0y fix
  touched), executed as `cargo test --workspace --test <suite>` per suite.
  The default list lives in the row; the env var overrides it. An explicitly
  empty list fails the lane (fail-closed, no cargo invocation). Skippable
  with `GATES_SKIP=lanes` like every lane.
- `scripts/gates-local.sh`: header comment only — the lane parser already
  picks the row up from the registry table.
- `scripts/gates-lanes-test.sh` gains one case pinning the row contract:
  exact argv under the default list, env-var override, empty-list failure
  before any cargo call, and a malformed twin row rejected by the parser.

**Explicitly excluded:** full `cargo test --workspace` (Docker and native
bridges stay CI-owned); any `crates/**/src/**` edit (the test bugs were
already fixed on main in 8393998ad).

## Acceptance criteria

- The lane row parses and dispatches exactly
  `cargo test --workspace --test dev_watch_script_test` and
  `cargo test --workspace --test dev_watch_proto_test` for a touched
  `camel-test`.
- `CAMEL_TEST_WORKSPACE_SUITES=""` fails the lane without invoking cargo.
- The new parser-test case is green; `bash scripts/gates-local.sh` exits 0.
- The lane command, run directly in the worktree, exits 0.

## Risk budget

Process/tooling only: no runtime code, no crate manifests, no CI workflow
files. The only behavioral risk is sweep time for changes that touch
`camel-test` (two extra suite runs under unification) — acceptable; the
registries stay the single source of truth.

Affected crates: none (`camel-test` is a lane subject, not an edit target).
Reference: bd rc-4xka4, discovered-from rc-lhd0y.
