# CanonicalRouteSpec as Minimal Route Contract

`CanonicalRouteSpec` v1 is a stable minimal route contract for runtime commands, config tooling, and hot-reload paths. It is not a full `RouteDefinition` mirror. The full DSL model remains the place for route authoring features such as templates, advanced lifecycle metadata, error handling, unit-of-work hooks, and route-level security declarations.

The alternative was expanding `CanonicalRouteSpec` toward full `RouteDefinition` parity. That would reduce feature gaps in canonical and hot-reload paths, but it would duplicate DSL validation, increase cross-crate coupling, and pull non-serializable or runtime-bound concerns into `camel-api`. `SecurityPolicy` is the clearest example: the runtime enforces a `SecurityPolicyConfig` containing trait-object policy implementations, while canonical contracts must remain serializable and safe to pass through tooling boundaries.

The chosen direction is a versioned, use-case-driven canonical contract. Fields are added only when runtime commands, config tooling, or hot reload need them and when they can be represented as stable serializable data. Unsupported user-set fields must be rejected or have explicit defaulting behavior; silent loss is not acceptable. Future expansion can add simple lifecycle metadata first (`auto_startup`, `startup_order`, `concurrency`), then richer error or unit-of-work models if their canonical representation is clear. Canonical expansion is not parity-driven.

## v2 Amendment (2026-06-08)

See [ADR-0016](./0016-canonical-route-spec-v2-contract.md) for the full v2 contract.

v2 adds lifecycle metadata: `auto_startup` (Option<bool>), `startup_order` (Option<i32>), `concurrency` (Option<CanonicalConcurrencySpec>). Strict rejection enforced for `error_handler`, `unit_of_work`, `security_policy`. Lossy escape hatch via `allow_loss` parameter. Version bumped from 1 to 2. Backward compatible: v2 runtime accepts v1 specs.
