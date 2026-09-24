# Gate ledger: protofix (stage 4, 2026-09-24)

Runner: detached sequential run in worktree, results file exit codes.
Base fec12d64, HEAD before final commit. Rust gates apply (2 `.rs`
files in diff). No N/A gates. No pre-existing-failure exemptions.

## Task-level gates (tasks 1-4, per-task r_glm reviewed)

- `cargo test -p camel-proto-compiler`: PASS — 19 passed, 0 failed
  (18 after task 2; +1 empty-override pin after holistic)
- `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings`:
  PASS
- `cargo fmt --check --all`: PASS
- `cargo clippy -p camel-dsl --features protobuf -- -D warnings`: PASS
- `cargo clippy -p camel-dataformat-protobuf -- -D warnings`: PASS
- `cargo clippy -p camel-component-grpc -- -D warnings`: PASS
- `cargo clippy -p camel-cli --features grpc -- -D warnings`: PASS
- `rg 'VendoredProtoc' crates/`: PASS — zero hits
- `openspec validate protofix --type change --json`: PASS — valid,
  0 issues

## Stage-4 full list (AGENTS.md QUALITY GATES + conductor additions)

- `cargo build --workspace`: PASS (exit 0)
- `cargo fmt --check --all`: PASS
- `cargo clippy --workspace --all-features` (exclude camel-cli,
  camel-component-kafka, security-keycloak, security-wasm-policy)
  `-D warnings`: PASS
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`:
  PASS
- `cargo clippy -p camel-cli -- -D warnings`: PASS
- 16 xtask lints — `lint-unwrap`, `lint-secrets`,
  `lint-single-source`, `lint-non-exhaustive`, `lint-log-levels`,
  `lint-log-redaction`, `lint-cancel-tokens`, `lint-test-sleep`,
  `lint-unbounded-wait`, `lint-ignore`, `lint-publish-cycles`,
  `lint-publish-registration`, `lint-component-deps`,
  `lint-gate-forwarding`, `lint-context-citations`,
  `lint-metric-labels`: all PASS (exit 0 each)
- `cargo run -p xtask -- schema --check`: PASS
- `cargo test --workspace --lib`: PASS (9988 ok test lines)
- `cargo test -p camel-core --test
  hexagonal_architecture_boundaries_test`: PASS
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p
  camel-builder -p camel-dsl -p camel-endpoint --no-deps`: PASS
- `cargo audit`: PASS (exit 0; 5 pre-allowlisted warnings, unchanged)

## Skipped

- `lint-commits`: SKIPPED — conductor deviation (runs `git fetch
  origin main`, a remote op; CI owns branch-diff checks). Recorded per
  gate-coverage self-check.

## Not run (CI-owned per conductor policy)

- `cargo test --workspace` (full, Docker + native bridges). This
  change touches no infra-guarded or `#[ignore]` tests; the protobuf
  dataformat route-load behavior is covered by the crate suite above.
