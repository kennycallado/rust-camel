# Hexagonal Architecture for camel-core Lifecycle

The lifecycle layer of `camel-core` is structured as a hexagonal application: domain and application logic are isolated behind ports (`RouteRepositoryPort`, `ProjectionStorePort`, `RuntimeEventJournalPort`, `RuntimeExecutionPort`), with concrete adapters providing in-memory and redb implementations.

The extra indirection is intentional — it lets us swap persistence backends, test lifecycle logic without a real runtime, and keep the domain model free of Tokio or storage concerns. A flat concrete implementation would be faster to write but harder to test and extend.
