# ADR-0045: camel-core Architecture Charter

## Status
Accepted

## Context
camel-core was promised as Clean Architecture + DDD + CQRS + vertical slices + hexagonal. That
promise was never codified in a single ADR — it was spread across ADR-0002 (CQRS) and ADR-0003
(hexagonal, scoped only to the lifecycle layer). During stabilization toward 1.0, drift appeared
because there was no constitution to consult:

- a domain file derived Serde (`lifecycle/domain/runtime_event.rs`) and derived
  `thiserror::Error` on `LanguageRegistryError` (`lifecycle/domain/error.rs`);
- the hexagonal layering applied only to `lifecycle/`, `hot_reload/`, `shared/` while ~9 flat root
  modules (~3700 LOC) ignored it;
- the `internal-adapters` feature gated public exports but not compilation;
- CQRS reads bypassed the projection port on the hot path without a documented exception.

This charter codifies the promise in one place so future drift is detectable against a single
authoritative source.

## Decision

### 1. The five-pillar promise (crate-scoped)
camel-core is, in priority order:
- **Hexagonal** — every behavioral area exposes ports (traits) inboard of its adapters.
- **Clean Architecture** — the dependency rule: dependencies point inward toward domain. Domain
  depends on nothing outside `std`/`crate::`. No framework types (Tower, Tokio, Serde, redb) in
  entities.
- **DDD** — aggregates, value objects, and domain events model the route lifecycle. Bounded
  contexts are the unit of decomposition (not technical layers).
- **CQRS** — scoped per bounded context (see §3). NOT global.
- **Vertical slices** — each bounded context is a self-contained vertical slice with its own
  internal hexagonal layout, NOT a horizontal technical layer shared across contexts.

### 2. Ceiling: module discipline, not crate-split
Canonical Clean Architecture is compiler-enforced via separate crates (domain / application /
adapters / drivers). camel-core **deliberately stays one crate** for 1.0. The ceiling is therefore
**strong module discipline + boundary tests**, not compiler-enforced ring isolation. Public paths
are kept stable; internal module paths enforce the rings. This is an explicit trade: lower purity
ceiling in exchange for no Semver break and no workspace churn during stabilization. A crate split
remains a post-1.0 option if module discipline proves insufficient.

### 3. CQRS is scoped per bounded context
CQRS is a bounded-context-level decision, never a crate-wide one:

| Bounded context | CQRS flavor | Why |
|---|---|---|
| Route lifecycle control plane | **Synchronous-projection CQRS** (strong consistency, no projection lag) — command/query buses, event journal, projection updated within the same UnitOfWork as the command | Safety: supervision decisions need consistent reads; ADR-0002 + ADR-0018 |
| Data plane (Exchange / Pipeline processing) | **Not CQRS** — Tower data plane | Hot path; ADR-0001 |
| Hot-reload, metrics, audit history | Event-sourced / eventually consistent where tolerated | Lag acceptable |

The lifecycle CQRS uses **synchronous projection** (not eventual consistency): the projection is
updated within the same optimistic-versioned UnitOfWork as the command, so projection lag cannot
break supervision decisions. This terminology avoids confusion with CAP-theorem "strong
consistency". (Clarifies ADR-0002.)

> §3 describes the **design contract** (ADR-0002 + ADR-0018). §4 below records the **implementation
> gaps** where the code currently deviates from that contract; the gaps are remediation targets, not
> part of the contract.

### 4. Accepted exceptions (explicit, not accidental)
Implementation deviations from the design contract are recorded here so they do not multiply
silently. Each is either remediated pre-1.0 or recorded as an accepted exception with
justification; every new bypass must land a line here, or it is a boundary violation.

**CQRS contract gaps (ACCEPTED — not remediated; remediation would regress correctness):**

- ✅ **Single backing store for repository + projection + events + dedup**
  (`context_builder.rs` wires one `InMemoryRuntimeStore` via `Arc::clone` into 4 typed
  ports) — ACCEPTED PERMANENT. The ports are fully segregated at the trait level
  (`RouteRepositoryPort` / `ProjectionStorePort` / `EventPublisherPort` /
  `CommandDedupPort`); `RuntimeBus` depends only on `dyn Port`, never on the adapter, so
  the dependency rule holds. The *unified backing store is required*, not incidental:
  Synchronous-projection CQRS (ADR-0002/0045) mandates that the aggregate and its
  projection be written in the same optimistic-versioned UnitOfWork
  (`RuntimeUnitOfWorkPort::persist_upsert`) under a single lock. Splitting the store into
  separate instances would break UoW atomicity and reintroduce projection lag — i.e. the
  eventual-consistency CQRS that ADR-0045 explicitly rejected. One adapter satisfying
  multiple segregated ports is a legitimate hexagonal pattern for a transactionally-unified
  infrastructure store. No boundary violation.

- ✅ **`InFlightCount` is served by the execution port, not the projection port**
  (`lifecycle/application/queries.rs` returns an exhaustiveness-guard error;
  `RuntimeBus::ask` intercepts and delegates to `RuntimeExecutionPort::in_flight_count`) —
  ACCEPTED PERMANENT as an explicit low-latency operational read. The in-flight counter is
  volatile runtime telemetry held by the controller actor; it is never persisted,
  journaled, or recovered, and is therefore not a CQRS read model. Routing it through
  `ProjectionStorePort` would require mirroring the live counter into the projection store
  on every in-flight increment/decrement — adding projection writes to the data-plane hot
  path to serve an O(1) in-memory read. The execution port *is* the correct port for
  volatile operational state; this is not a bypass of the right port but selection of it.

**Entity-purity gaps (the dependency rule):**

- ✅ **`RuntimeEvent` derived `Serialize`** (`lifecycle/domain/runtime_event.rs`) — REMEDIATED in
  Tier B (`rc-d0pu.2`): the `RuntimeEventRecord` DTO (`lifecycle/adapters/runtime_event_record.rs`)
  now carries Serde via a `#[serde(with)]` bridge on `JournalEntry.event`; the entity derive is
  stripped. Wire format byte-compatible (roundtrip test). Kept here for the historical record.
- ✅ **`LanguageRegistryError` derived `thiserror::Error`** (`lifecycle/domain/error.rs:33`) —
  REMEDIATED in Tier B (`rc-d0pu.2`): relocated to `language_registry.rs` (languages application
  slice); `domain/error.rs` no longer imports thiserror (only `DomainError`'s manual impl remains).
  A `#[deprecated]` re-export shim at `lifecycle::domain` keeps the old path compiling; shim removal
  tracked in bd `rc-rfr9`. Kept here for the historical record.

**Use-case purity gaps (Tier C C2):**

- ✅ **`abort_context` takes concrete `&RouteControllerHandle`** — REMEDIATED (`rc-d0pu.3`-purge): abort_context now takes `&dyn RouteOrderingPort + &dyn RouteDestructiveTeardownPort` (two-param split — Rust stable does not support multi-trait objects); the destructive `shutdown()` is exposed via the new narrow `RouteDestructiveTeardownPort` (one method). Impl on the concrete controller handle lives in `lifecycle/adapters/route_ordering_impl.rs`. Kept here for the historical record.

**Port → adapter type leak (Tier C C1):**

- ✅ **`ReloadExecutorPort` references `PreparedRoute` from `lifecycle::adapters`** — REMEDIATED (`rc-d0pu.3`-purge): `PreparedRoute` is now a thin `{ route_id: String }` token in `lifecycle/domain/route_compilation.rs`; the heavy `ManagedRoute` is staged internally on `DefaultRouteController.prepared_staging`. The `port_traits_do_not_import_from_adapter_ring` allow-list is now `[]`. Kept here for the historical record.

**Domain ring framework field types** — REMEDIATED (`rc-d0pu.3`-purge): `health_registry` + `datasource` were collapsed to single-ring adapter modules (stateful infra + framework-typed value types are correctly labeled Interface Adapters, no false "domain" ring remains). Kept here for the historical record.

**Pre-1.0 deprecation removals (Tier C C5):**

- **`camel_core::lifecycle::ports::*` glob-shim re-export**
  (`crates/camel-core/src/lib.rs:140-144`, formerly `lifecycle/ports.rs`) — REMOVED
  (`rc-rfr9`): the compatibility shim that re-exported `lifecycle::application::ports::*`
  at the old Tier B path during the slice-homes transition. Canonical replacement:
  `crate::lifecycle::application::ports::*`. Pre-1.0 `#[deprecated]`-item removal
  with a canonical replacement available since Tier B; conventional pre-1.0
  practice. NOT a wire-format break (no serialized form changes).
- **`camel_core::lifecycle::domain::LanguageRegistryError`** (`lifecycle/domain/mod.rs:6-10`)
  — REMOVED (`rc-rfr9`): the `#[deprecated]` re-export of `LanguageRegistryError` at
  the `lifecycle::domain` path. Canonical replacement: `camel_core::LanguageRegistryError`
  (re-exported at `lib.rs:115` from `crate::language_registry`). Pre-1.0
  `#[deprecated]`-item removal with a canonical replacement available since
  Tier B (`rc-d0pu.2`); conventional pre-1.0 practice. NOT a wire-format break
  (no serialized form changes). Completes the remediation recorded in the
  entity-purity gap for `LanguageRegistryError` above.

### 5. Ring → module mapping
The four Clean Architecture rings map to camel-core modules as follows (the decomposition unit is
the vertical slice):

| Module | Classification | Ring |
|---|---|---|
| `context.rs` | composition root (thinned in C2) | Frameworks & Drivers |
| `context_builder.rs` | composition root (wiring) | Frameworks & Drivers |
| `health_registry.rs` | single-ring adapter module (stateful probe registry: `RwLock<HashMap>` + `CancellationToken` + `Duration` fields; `check_all` use-case orchestrates `tokio::timeout` + `futures::join_all`) | Interface Adapters |
| `datasource.rs` | single-ring adapter module (`DashMap<CacheKey, OnceCell<Handle>>` + `RwLock<HashMap<PoolFactory>>` fields; `get_pool`/`resolve_factory` orchestrate lazy pool creation + health wiring) | Interface Adapters |
| `startup_validation.rs` | single-ring slice (has trait, zero I/O) | Entities |
| `template.rs` | single-ring slice (`TemplateRegistry` Mutex store) | Interface Adapters |
| `language_registry.rs` | single-ring slice | Use Cases |
| `component_metadata_catalog.rs` | single-ring adapter ("thin wrapper") | Interface Adapters |
| `registry.rs` | single-ring (private shared-infra types) | Interface Adapters |
| `claim_check/` | single-ring (already ring-stable; out of Tier C scope) | Interface Adapters |
| `idempotent/` | single-ring (already ring-stable; out of Tier C scope) | Interface Adapters |

## Consequences
- The boundary test (`hexagonal_architecture_boundaries_test.rs`) is extended to cover root/shared
  slices and the CQRS read-path exceptions, not only the lifecycle tree.
- New bounded contexts must declare their CQRS flavor in §3 before adding command/query handlers.
- This charter does not supersede ADR-0002 or ADR-0003 — it frames them crate-wide and clarifies
  the CQRS consistency model.
- Cross-cutting vocabulary introduced here (vertical slice, bounded context, synchronous-projection
  CQRS, module-discipline ceiling) also lands in `CONTEXT-MAP.md` Key Terms per the project's
  term-landing rule.

### Self-grill record

**Questions generated:**
1. [glossary] Do the charter's terms conflict with / duplicate existing CONTEXT-MAP Key Terms?
2. [sharpen] Is "strong-consistency CQRS" precise, or does it collide with CAP-theorem usage?
3. [scenario] §1 forbids Serde in entities; §5 maps `RuntimeEvent` (which derives Serialize) to Entities — does a constructed cross-check break?
4. [cross-ref] Does the code today implement §3's claim, or is §3 describing design intent?

**Answers (with citations):**
1. [glossary] No conflict — CONTEXT-MAP Key Terms (`CONTEXT-MAP.md:74-90`) define Message, CircuitBreaker, Supervision, etc. but NOT vertical-slice / bounded-context / CQRS-flavor. The terms are new cross-cutting vocabulary; per the term-landing rule (`CONTEXT-MAP.md` term-landing rule) they must also land in Key Terms. → added a Consequences bullet.
2. [sharpen] "Strong consistency" collides with CAP/linearizability usage. Canonical term is **synchronous-projection CQRS** (projection updated in the same UoW as the command). → §3 table + paragraph sharpened.
3. [scenario] Constructed check: reader reads §1 ("no Serde in entities"), opens `domain/runtime_event.rs`, finds `#[derive(Serialize)]`, cross-refs §5 mapping → contradiction with no exception listed. → added `RuntimeEvent`/`DomainError` to §4 entity-purity gaps.
4. [cross-ref] §3 matches the design contract (ADR-0018: "command handling, projections, event publication, and journal replay stay consistent"; `RuntimeUnitOfWorkPort` exists). The code has the two CQRS shortcuts already listed in §4. §3 = contract, §4 = gaps; made explicit with a callout block.

**Outcome:** refine — terminology sharpened, entity-purity gap reconciled, contract/implementation split made explicit, term-landing consequence added.
**Post-grill verification:** path-checking the cited files revealed the `thiserror` coupling belongs to `LanguageRegistryError` (`error.rs:33`), not `DomainError` (which is already clean via manual `impl std::error::Error`). Body corrected to match; this is the kind of imprecision the cross-ref technique is meant to catch.
**Self-grill mode:** self-grill-proposals skill
