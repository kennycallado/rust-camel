# Testing

This section describes how to test routes with the lean `camel test` boot and with route interception. Interception rewrites `to:` send points at compile time. It supports isolated unit tests without `mock:` lines in production routes.

For Rust-level integration tests, the workspace ships a harness crate: `crates/camel-test` provides `CamelTestContext` and a `TimeController` for deterministic polling, plus the shared matcher re-exports (ADR-0072). See [`crates/camel-test/README.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-test/README.md) and its CONTEXT.md.

## Chapters

- [Route interception](route-interception.md): compile-time send rewriting for isolated unit tests
- [Declarative camel test](camel-test.md): the `*.test.yaml` document grammar, stubs, filters, and reports
- [Scenario documents](scenario-documents.md): the integration-tier `scenario:` document contract
- [SQL state assertions](scenario-sql.md): sql prepare and validate actions over datasource state
