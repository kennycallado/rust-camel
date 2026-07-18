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

**CQRS contract gaps:**

- **Single store wired for repository + projection + events + dedup**
  (`context_builder.rs`) — target: separate the wiring or document why one store is acceptable for
  the in-process control plane.
- **In-flight reads bypass the projection port** (`lifecycle/application/queries.rs`) — target:
  route through the projection port, or record as an explicit low-latency read exception here.

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

- **`abort_context` takes concrete `&RouteControllerHandle`**
  (`lifecycle/application/context_lifecycle.rs::abort_context`) — ACCEPTED EXCEPTION
  (`rc-d0pu.3`): the abort use-case holds the concrete `RouteControllerHandle` to invoke the
  destructive `shutdown()` method. `start_context` and `stop_context` go through the
  `RouteOrderingPort` abstraction, but `shutdown()` is destructive and non-restartable
  (vs. the restartable start/stop ordering queries), so it does not fit the ordering
  port. The narrow-port alternative (`RouteDestructiveTeardownPort` with only `shutdown()`)
  was rejected as YAGNI — one rare destructive call. **Precedent:** none at the use-case
  ring — C1 (`ReloadExecutorPort`) removed `hot_reload/application`'s concrete-handle
  inversion, making this the first use-case→concrete-adapter exception in the crate.
  Closest analog is adapter-internal: `lifecycle/adapters/runtime_execution.rs` holds a
  concrete `RouteControllerHandle`, and `hot_reload/adapters/reload_watcher.rs` holds a
  concrete `RuntimeExecutionHandle` — both within the adapters ring (adapter-to-adapter,
  not use-case-to-adapter). The exception here stands on YAGNI alone. Health
  cancellation still goes through the cancel token (no whole-registry leak).

**Port → adapter type leak (Tier C C1):**

- **`ReloadExecutorPort` references `PreparedRoute` from `lifecycle::adapters`**
  (`hot_reload/ports/mod.rs`) — ACCEPTED EXCEPTION (`rc-d0pu.3`): the port's
  `prepare_route_definition_with_generation` / `insert_prepared_route` methods
  reference `PreparedRoute`, which lives in `lifecycle::adapters::route_controller`.
  `PreparedRoute::managed: ManagedRoute` bundles adapter-internal state
  (`JoinHandle`, `CancellationToken`, `SharedPipeline`, `Arc<AggregatorService>`,
  `CompiledRoute`) and cannot be relocated to domain without a port-semantics
  redesign (thin `{ route_id }` contract + controller-internal `HashMap` keyed by
  route_id). That redesign changes `prepare`/`insert` semantics and is out of
  Tier C's "extract the port" scope. The companion `CompiledPipeline` WAS
  relocated to `lifecycle/domain/route_compilation.rs` (pure contract type).
  The exception is bounded to exactly this one import, pinned by the
  `port_traits_do_not_import_from_adapter_ring` boundary test allow-list
  (`["PreparedRoute"]`); when the redesign lands, the allow-list goes empty.

**Domain ring framework field types (pre-existing, surfaced by Tier C C3):**

- **`health_registry/domain.rs` + `datasource/domain.rs` hold framework-typed
  fields** — ACCEPTED EXCEPTION (pre-existing, made explicit by C3's slice
  labeling): the aggregates `HealthCheckRegistry` (fields: `tokio_util::sync::CancellationToken`,
  `tokio::time::Duration`, `parking_lot::RwLock`) and `RuntimeDatasourceCatalog`
  (fields: `dashmap::DashMap`, `tokio::sync::OnceCell`) plus `camel_api` traits
  (`AsyncHealthCheck`) carry framework field types despite living in the
  domain-ring file. These imports are identical to the pre-Tier-C flat modules
  (verified at `4aca5ac6`); C3's 3-ring split labeled the file `domain.rs`,
  making the pre-existing impurity explicit rather than introducing it. Strict
  domain purification (extracting the stateful registries behind ports, leaving
  only pure value types in domain) is a separate refactor out of Tier C's
  "slice homes" scope. The aggregates are documented in-file as "field types
  — the domain does not perform I/O; orchestration lives in the sibling
  application module." The `domain_does_not_import_application_or_adapters`
  boundary test still holds (no cross-ring imports); the framework-field
  impurity is crate-internal `pub(crate)` state, not a public contract leak.

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
| `health_registry/domain.rs` | full-slice submodule | Entities |
| `health_registry/application.rs` | full-slice submodule | Use Cases |
| `health_registry/adapters.rs` | full-slice submodule | Interface Adapters |
| `datasource/domain.rs` | full-slice submodule | Entities |
| `datasource/application.rs` | full-slice submodule | Use Cases |
| `datasource/adapters.rs` | full-slice submodule | Interface Adapters |
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
