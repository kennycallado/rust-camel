# jms-sensitive-assertions-split

Extract the `sensitive_assertions` module from `crates/camel-test/tests/support/jms_disposition_cases.rs` (1164 lines) into its own support file `jms_disposition_sensitive.rs` via a `#[cfg(test)] #[path]` include.

## Why

Trivial change — no spec breakdown needed.

## What changes

- Move the module body (former lines 850–1163) byte-for-byte into `jms_disposition_sensitive.rs`, de-indented one level (`cargo fmt` canonical); the doc comment moves onto the include.
- Replace the module in `jms_disposition_cases.rs` with a `#[cfg(test)] #[path = "jms_disposition_sensitive.rs"] mod sensitive_assertions;` include; `use super::*;` keeps resolving (parent is still `jms_disposition_cases`).
- Pure move, zero behaviour change, zero test renames; file drops to 850 lines.
