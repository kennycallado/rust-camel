# Verification

## Quality gates (worktree, 2026-09-05)

All gates green on `feature/itest-bound-ports` at commit `5fcfc609` (base
`1c8ea2f1`); `lint-commits` skipped per conductor policy (remote operation).

- `cargo fmt --check --all` — exit 0
- clippy trio (`--workspace --all-features` minus kafka/cli/security
  exclusions; `-p camel-component-kafka --all-targets`; `-p camel-cli`)
  with `-D warnings` — all exit 0
- 10 xtask lints (unwrap, secrets, non-exhaustive, log-levels, ignore,
  publish-cycles, component-deps, gate-forwarding, context-citations,
  metric-labels) — all exit 0
- `cargo xtask schema --check` — exit 0
- `cargo audit` — exit 0 (5 allowed warnings, unchanged set)
- `cargo build --workspace` — exit 0 (after sccache daemon restart; the
  daemon had cached a dead TMPDIR — transient, no repo impact)
- `cargo test --workspace --lib` — exit 0 (68 ok result lines, 0 failed)
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`
  — 33 passed, 0 failed

## Per-crate test evidence

- `cargo test -p camel-component-http --all-targets` — 290 passed, 0
  failed (280 existing + 8 staged-listener tests + 1 single-resolver race
  regression test + doc target).
- `cargo test -p camel-component-ws --all-targets` — 158 passed, 0 failed
  (153 existing + 5 staged tests).
- camel-test migrated binaries (full targets, not library-only, per bd
  rc-h0aw oracle requirement):
  `cargo test -p camel-test --features integration-tests --test http_test
  --test audience_substitution_test --test auth_multi_credential_test
  --test kernel_fail_closed_test --test late_registration_gate_test`
  — exit 0: 52 + 6 + 4 + 7 + 4 = 73 passed, 0 failed.
- `grep -rn find_free_port crates/camel-test/` — empty (exit 1).

## Review trail

- Task 1 r_glm APPROVE (1 legitimate minor: per-caller source resolution
  race → fixed in `6d467051`, single-resolver inside init winner,
  re-approved).
- Task 2 r_glm APPROVE (1 minor: nested reset locks → fixed in
  `c779aef6`).
- Task 3 r_glm APPROVE (1 plan-conformant dedup minor, discarded).
- Holistic r_glm APPROVE-WITH-FINDINGS: ws CONTEXT.md staged-listener
  bullet added; this verification.md written. Spec/plan blessings by
  e_gpt: spec 4 rounds (`9c94280e…`), plan 4 rounds (`46b54987…`,
  covering post-spec-bless factual corrections per the hash-supersession
  ruling).
