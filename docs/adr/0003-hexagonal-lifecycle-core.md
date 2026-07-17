# Hexagonal Architecture for camel-core Lifecycle

The lifecycle layer of `camel-core` is structured as a hexagonal application: domain and application logic are isolated behind ports (`RouteRepositoryPort`, `ProjectionStorePort`, `RuntimeEventJournalPort`, `RuntimeExecutionPort`), with concrete adapters providing in-memory and redb implementations.

The extra indirection is intentional — it lets us swap persistence backends, test lifecycle logic without a real runtime, and keep the domain model free of Tokio or storage concerns. A flat concrete implementation would be faster to write but harder to test and extend.

## Scope (amended by ADR-0045)

This decision was originally scoped to the `lifecycle/` layer only. ADR-0045 extends the hexagonal
discipline **crate-wide** as the primary pillar of the camel-core architecture charter: every
behavioral area is a vertical slice with its own internal `domain` / `application` / `ports` /
`adapters` layout. The flat root modules (`context.rs`, `health_registry.rs`, `datasource.rs`,
`template.rs`, `registry.rs`, `language_registry.rs`, `component_metadata_catalog.rs`,
`startup_validation.rs`) are vertical slices awaiting that internal organization — they are
remediation targets, not a permanent exception.

ADR-0045 also declares the **ceiling**: because camel-core deliberately stays one crate through
1.0, the rings are enforced by module discipline + boundary tests rather than by compiler-enforced
crate isolation.
