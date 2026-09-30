# Tasks: redissrc

## Task 1: HEADER_VALUE const + 59 literal swaps + embedded message

- [x] 1.1 add `pub const HEADER_VALUE: &str = "CamelRedis.Value"` to
  `crates/components/camel-redis/src/lib.rs` with doc comment, placed
  after the `pub use` block with a section comment
- [x] 1.2 commands/mod.rs: import the const; swap the `require_value`
  argument; rewrite the embedded error message with `format!`
- [x] 1.3 hash.rs (6), list.rs (15), set.rs (13), string.rs (11),
  zset.rs (13): add `use crate::HEADER_VALUE;`, swap every
  `"CamelRedis.Value"` token; must NOT touch `"CamelRedis.Values"`
- [x] 1.4 verify: `grep -rn '"CamelRedis.Value"' src/` returns 0; plain
  substring grep returns only the 10 plural sites; total swapped = 59
- [x] 1.5 gates: `cargo fmt --check` clean; `cargo clippy -p
  camel-component-redis --all-targets -- -D warnings` clean;
  `cargo test -p camel-component-redis` = 608 passed (lib subset 584);
  `cargo test -p camel-redis-repo` = 61 passed; runs under systemd-run
  scope fleet-redissrc, CARGO_BUILD_JOBS=6, test -j4, sccache ON
