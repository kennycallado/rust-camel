# protoquiet — quality gate ledger

Branch `feature/protoquiet`, base `2ee93cd2` (main). All cargo runs in
the worktree `/home/shared/rust-camel-worktrees/protoquiet`; main
checkout stayed cold.

## Green

| Gate | Result |
| --- | --- |
| `cargo fmt --check --all` | exit 0 |
| `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` | exit 0 |
| `cargo test -p camel-proto-compiler` | 20 passed, 0 failed (round 1 and round 2; r_glm and e_glm re-ran independently) |
| `cargo test -p camel-proto-compiler -- --test-threads=8` | 20/0 (e_glm flake probe on the process-global recording hook) |
| `cargo clippy -p camel-proto-compiler --all-targets --all-features -- -D warnings` | clean (r_glm, e_glm) |
| `cargo xtask lint-unwrap` | exit 0, no violations |

## N/A (mission-conditional gates)

| Gate | Why |
| --- | --- |
| clippy legs 2-4 (`camel-component-kafka`, `camel-cli` x2) | camel-cli / kafka untouched (order: "4th for camel-cli if touched — likely not") |
| lint-unbounded-wait ratchet 296 | no test waits added |
| doc-build | no docs touched; `docs/src/data-formats/protobuf.md` verified — no panic-noise mention, failure-mode wording already truthful |
| schema-check | no schema inputs touched |
| cargo-audit | no dependency changes |
| lint-commits | conductor deviation (remote fetch; CI owns) |
| full workspace tests | Docker/infra; CI owns |
