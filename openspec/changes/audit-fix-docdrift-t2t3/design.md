# Design: audit-fix-docdrift-t2t3

## Context

FC-DOC-DRIFT sweep across T2/T3 crates. 10 findings identified in
audit reports from 2026-08-05. Verification against current main
(2026-08-08) confirmed 5 already resolved by prior oracle/docs commits;
5 remain open.

## Approach

Single-phase mechanical fix. Each finding is an independent README or
rustdoc correction. No architectural decisions, no code behavior
changes, no new types or APIs. The fixes align documentation with the
actual source code surface as of HEAD.

## Finding details

### F-camel-endpoint-1 (Minor)
README derive-macro example (`README.md:49-56`) uses `#[uri(default)]`
and `#[uri(rename)]` — these attributes do not exist. Correct syntax
is `#[uri_param(default)]`, `#[uri_param(name)]`, and the struct needs
`#[uri_scheme = "timer"]`. The sibling CONTEXT.md already documents the
correct contract.

### F-camel-endpoint-macros-2 (Minor)
`lib.rs:18-20` rustdoc `## Struct-level attributes` section lists two
bullets but omits `#[uri_config(crate = "path")]`. Default is
`camel_endpoint`; component crates use `camel_component_api`.

### F-camel-bean-M1 (Minor, 3 sub-issues)
(a) README `BeanError` enum block (`:179-194`) omits `InvalidName`
variant (exists at `error.rs:19`).
(b) README (`:206`) shows `ProcessorError(err.to_string())` — actual
code is `ProcessorErrorWithSource(err.to_string(), Arc::new(err))`.
(c) README (`:49,285,287`) `register()` calls missing `?` (method
returns `Result` since `9f7cada3`).

### F-camel-bean-macros-M1 (Minor)
`lib.rs:27-28` doc-comment says "detected by the Bean derive macro"
but the detector is `#[bean_impl]`. Line 32 inline comment references
dead "Task 2.2".

### F-camel-test-M1 (Minor, 2 sub-issues)
(a) README builder methods table (`:77-84`) omits `.with_seda()`
(exists at `harness.rs:112`).
(b) Examples (`:18,48`) import `StepAccumulator` but never use it.

## Verification

- `cargo test -p camel-endpoint` passes (README example not compiled
  as doctest, but syntax must be correct for human readers).
- `cargo test -p camel-bean` passes.
- `cargo test -p camel-test` passes.
- `cargo fmt --check --all` passes.
- `cargo xtask lint-context-citations` passes (0 violations).
