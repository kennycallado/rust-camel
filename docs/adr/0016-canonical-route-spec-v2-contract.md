# ADR-0016: CanonicalRouteSpec v2 Contract

**Date:** 2026-06-08
**Status:** Accepted
**Amends:** ADR-0011
**Issues:** rc-5iy, rc-ph7, rc-14b
**Oracle:** ses_158f99ba3ffeAbgqfO9DQ9G3cn

## Decision

CanonicalRouteSpec v2 adds lifecycle metadata fields with strict rejection for unsupported fields and a versioned expansion policy.

## v2 Schema

```rust
pub const CANONICAL_CONTRACT_VERSION: u32 = 2;

pub struct CanonicalRouteSpec {
    // v1
    pub route_id: String,
    pub from: String,
    pub steps: Vec<CanonicalStepSpec>,
    pub circuit_breaker: Option<CanonicalCircuitBreakerSpec>,
    // v2
    pub auto_startup: Option<bool>,       // None = true
    pub startup_order: Option<i32>,       // None = 0
    pub concurrency: Option<CanonicalConcurrencySpec>, // None = Sequential
    pub version: u32,
}

pub enum CanonicalConcurrencySpec {
    Sequential,
    Concurrent { max: usize },
}
```

### Defaults

| Field | None default | Rationale |
|---|---|---|
| `auto_startup` | `true` | Matches current behavior |
| `startup_order` | `0` | Neutral ordering |
| `concurrency` | `Sequential` | Safe default |

## Versioning Policy

- `version` is a monotonic schema counter: 1, 2, 3, ...
- New runtime accepts old canonical specs (backward compatible): `validate_contract` accepts `version >= 1 && version <= CANONICAL_CONTRACT_VERSION`
- Old runtime rejects newer canonical specs with clear error
- Added fields are `Option<T>` — old JSON deserializes in new runtime without breaking
- `CANONICAL_CONTRACT_NAME` is a protocol identifier, not versioned

## Expansion Principle

A field deserves canonical inclusion only if ALL six criteria are met:

1. Needed across a stable boundary (runtime command, config, hot-reload, persisted config)
2. Stable data-only representation exists
3. Runtime can enforce it without the full DSL model
4. Default behavior is safe and deterministic
5. Losing it changes observable behavior
6. Backward compatibility story is clear

Serializability is necessary but not sufficient.

## Strict Rejection Policy

`compile_declarative_route_to_canonical` enforces strict rejection for unsupported fields:

| Field | Behavior |
|---|---|
| `security_policy` | Always error, no override |
| `error_handler` | Error by default, droppable with `allow_loss=true` |
| `unit_of_work` | Error by default, droppable with `allow_loss=true` |
| `DeclarativeConcurrency::Concurrent { max: None }` | Error by default, droppable with `allow_loss=true` |

No silent loss.

### Lossy Escape Hatch

`allow_loss: bool` parameter (default `false`) permits explicit field dropping with structured diagnostics via `CanonicalLossReport`:

```rust
pub struct CanonicalLossReport {
    pub dropped_fields: Vec<CanonicalFieldLoss>,
}

pub struct CanonicalFieldLoss {
    pub field: &'static str,
    pub reason: String,
    pub target_version: u32,
}
```

`security_policy` is never dropped, regardless of `allow_loss`.

## Compile Path Changes

### compile_declarative_route_to_canonical

- Signature: `(route: DeclarativeRoute, allow_loss: bool) -> Result<(CanonicalRouteSpec, Option<CanonicalLossReport>), CamelError>`
- Propagates `auto_startup`, `startup_order`, `concurrency`
- Rejects `error_handler`, `unit_of_work`, unbounded concurrency unless `allow_loss=true`

### compile_canonical_route

- Respects `spec.auto_startup` (default `true` if `None`)
- Propagates `startup_order` and `concurrency`
- No longer forces `auto_startup(true)`

## Roadmap

| Version | Concern | Fields |
|---|---|---|
| v2 | Lifecycle/execution | `auto_startup`, `startup_order`, `concurrency` |
| v3 | Metadata + observability | `description`, `group`, `metrics_disabled`, `trace_enabled` |
| v4+ | Resilience/ops | `supervision_config`, `health_check_config`, `error_handler` |

`unit_of_work` stays out until it has a data-only, registry-safe representation. `security_policy` is permanently rejected (trait-object bound, non-serializable).
