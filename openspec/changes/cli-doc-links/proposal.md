## Why

`RUSTDOCFLAGS="-D warnings" cargo doc -p camel-cli --no-deps` exits 101.
Three public docs link with intra-doc links to `pub(crate)` items, which
trips `rustdoc::private-intra-doc-links`. The AGENTS.md doc-build gate
excludes `camel-cli`, so the repo gate never reports it (bd rc-7ej4d).

## What Changes

- In `crates/camel-cli/src/compile/materialize.rs` (2 places) and
  `crates/camel-cli/src/compile/policy.rs` (1 place), change the intra-doc
  link to a plain backticked name.
- Comment-only. No visibility change, no behaviour change, no new API.

## Impact

- Affected crate: `camel-cli` only. No spec requirement changes
  (`skip_specs: true`).
