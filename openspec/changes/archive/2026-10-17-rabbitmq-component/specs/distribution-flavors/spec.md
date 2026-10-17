# distribution-flavors Specification (delta)

## MODIFIED Requirements

### Requirement: Three distribution flavors with principled bodies

The camel CLI SHALL ship in exactly three flavors, selected only via the
marker features `flavor-slim`, `flavor-regular`, `flavor-full` in
`crates/camel-cli/Cargo.toml`. Bodies SHALL be CHAINED (each flavor's list
includes the marker of the flavor below it, making slim ⊆ regular ⊆ full
structural). Exclusion from regular SHALL be justified ONLY by a named
principle: a C dependency unportable to musl (kafka), a non-OSI license
(surrealdb, BUSL-1.1), arbitrary host-binary execution (exec, ADR-0037), or
an infrastructure-daemon client (containers: camel-function +
camel-component-container as ONE feature). `flavor-full` SHALL cover the
complete feature universe of camel-cli except non-flavor axes — a
`full_covers_universe` closure test SHALL fail CI when any feature is
placed in no flavor. Enabling no marker SHALL report flavor `custom`.

#### Scenario: slim body is the edge pack

- **WHEN** `cargo build --no-default-features --features flavor-slim` resolves
- **THEN** the closure contains the base components (core, direct, seda,
  log, file, timer, http with REST DSL, stream in/out/err) plus mqtt
  (+tls), http-static, sql
  (sqlite), lang-jsonpath and lang-rhai, and contains no kafka, no exec,
  no surrealdb, no containers (camel-function/camel-component-container),
  no lang-js, no lang-xpath, and no security features

#### Scenario: regular body excludes only by principle

- **WHEN** `cargo build --features flavor-regular` resolves (default)
- **THEN** the closure is slim plus otel, grpc, wasm, llm, mcp, security,
  redis (+tls), jms, rabbitmq, cxf, xj, xslt, opensearch, ws,
  lang-xpath, lang-js, lang-minijinja, lsp, kubernetes,
  integration-http and integration-sql, and contains none of the four
  principled exclusions (kafka, surrealdb, exec, containers)

#### Scenario: full is regular plus exactly the principled deltas

- **WHEN** `cargo build --features flavor-full` resolves
- **THEN** the closure is exactly the regular closure plus `exec`, `kafka`,
  `surrealdb` and `containers`, the kafka component is reachable (registry
  resolves `kafka` — the historical kafka-less-full defect stays fixed),
  and the container/function services are registered

#### Scenario: full covers the feature universe

- **WHEN** the feature-closure suite runs the full_covers_universe test
- **THEN** every camel-cli feature except the non-flavor axes (jemalloc,
  dynamic-linking, itest-e2e, `default`, the legacy `full` closure list,
  and the flavor markers themselves) is reachable from `flavor-full`, and
  an unplaced feature fails the test
