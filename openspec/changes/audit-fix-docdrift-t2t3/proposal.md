# Proposal: audit-fix-docdrift-t2t3

## Why

T2/T3 doc-drift residual (rc-acd3 epic). Investigation found 5 of 10
FC-DOC-DRIFT findings still open; the other 5 were already resolved by
prior oracle/docs commits (grpc, keycloak, wasm, auth, redis CONTEXT.md
all updated). The 5 remaining are mechanical README/rustdoc/doc-comment
fixes — no code behavior changes.

## What

- **camel-endpoint README**: fix derive-macro example syntax (`#[uri]`
  → `#[uri_param]`, add `#[uri_scheme]`)
- **camel-endpoint-macros lib.rs**: add `#[uri_config(crate)]` bullet
  to rustdoc `## Struct-level attributes`
- **camel-bean README**: add missing `InvalidName` variant, fix
  `ProcessorError` signature, add `?` to `register()` calls
- **camel-bean-macros lib.rs**: fix doc-comment (Bean derive →
  `#[bean_impl]`, remove dead Task 2.2 ref)
- **camel-test README**: add `.with_seda()` to builder methods table,
  remove unused `StepAccumulator` import in examples

## Out of scope

- The 5 already-resolved findings (grpc/keycloak/wasm/auth/redis)
- Non-FC-DOC-DRIFT findings (async lifecycle, API stability, etc.)
- CONTEXT.md creation for crates that lack one (bean, bean-macros) —
  that is a separate DP (Decision Proposal) track, not doc-drift repair
