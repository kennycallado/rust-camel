# Contributing to rust-camel

Thanks for your interest in contributing! This guide covers build setup, the
coding standards every PR must meet, and the commit convention.

## Build & test

```sh
cargo build --workspace
cargo test  --workspace
```

- **Rust:** 1.89 or newer (stable). `rust-toolchain.toml` pins `stable`.
- **Examples** live under `examples/`; most are workspace members. Run one
  with `cargo run -p <name>`.

## Quality gates

PRs must pass the CI quality gates. The exact, up-to-date command set lives in
[`AGENTS.md`](AGENTS.md) (fmt, clippy across the workspace plus component-
specific checks for Kafka and the CLI, `cargo xtask lint-*`, `schema --check`,
and `cargo audit`). Run them locally before requesting review.

Coverage baseline (currently 75%) is enforced via `coverage.toml`; run
`scripts/coverage.sh` locally (requires `cargo-llvm-cov`).

## Coding standards

- Follow the existing style — `cargo fmt` is authoritative.
- No `unwrap()`/`expect()` in non-test code (enforced by `xtask lint-unwrap`).
- No secrets/credentials in source (enforced by `xtask lint-secrets`).
- Keep logging levels consistent (enforced by `xtask lint-log-levels`).
- Prefer the standard library and existing dependencies over new crates.

## Commit convention

Conventional Commits, tight and imperative.

```
<type>(<scope>): <imperative summary>   # ≤50 chars when possible, 72 max
```

Types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `build`,
`ci`, `style`, `revert`.

- Body only when the "why" is non-obvious, breaking, or a migration. Wrap at
  72, use `-` bullets, keep it to 3–8 lines.
- No AI attribution, no emoji, no restating what the diff already says.

## Pull requests

1. Open an issue first for anything beyond a small fix.
2. Keep PRs focused; split large changes into reviewable commits.
3. Make sure all quality gates pass locally before requesting review.

## Issue tracking

External contributors: open a [GitHub issue](https://github.com/kennycallado/rust-camel/issues)
or pull request — you don't need `bd`. Internal work is also tracked with
`bd` (beads); regular contributors can ask about access.
