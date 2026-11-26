# ADR-0101: Transacted block surface

- Date: 2026-11-20
- Status: Accepted (2026-11-20)
- Context: change `sqltrans-portmodel` (slice 1 of the family); bd `rc-qv2iz`,
  `rc-qv2iz.5`
- Amends: none. Slices 2 (`sqltrans-runtime`, `rc-qv2iz.6`) and 3
  (`sqltrans-modes`, `rc-qv2iz.7`) AMEND this ADR; they do not introduce new
  ones for the transaction surface.
- Cross-refs: [ADR-0011](0011-canonical-route-spec-minimal-contract.md)
  (canonical route contract), [ADR-0017](0017-dsl-yaml-snake-case-naming-convention.md)
  (YAML key naming), [ADR-0025](0025-outcome-aware-structural-eips.md)
  (outcome-aware structural EIPs), [ADR-0032](0032-exchange-data-trust-boundary.md)
  (exchange-data trust boundary), [ADR-0099](0099-idempotent-reservation-semantics.md)
  (idempotent reservation semantics)

## Context

The RC-46 outbox requires two SQL writes — a business UPDATE and an outbox
INSERT — to commit atomically. No component-private mechanism can express that:
the commit point is a property of the ROUTE, not of either endpoint, and a
route that retries or routes on must be able to name where the transaction
begins and ends.

Two design rounds (14 expert rounds across the family spec) considered three
surfaces, labelled (a), (b) and (c) in the blessed family spec. The e_opus
ruling of 2026-11-15 selected (b) and rejected (a) and (c) for the reasons
recorded below. The e_opus split ruling of 2026-11-20 divided the family spec
into three slices so that no gate re-litigates the design: slice 1 ships the
port and the route model DARK, slice 2 execution and finalization, slice 3
mode semantics.

The prior `txnreject` work (`9f5c0bc33`) already removed per-endpoint
transaction handling: `transactionMode=Managed` is rejected at endpoint
creation today, because an endpoint cannot own a boundary it cannot see.
`sqlbinds` (`e49494534`) landed a single bind path. This ADR fixes the surface
those two decisions left open.

## Decision

### (a) Chosen surface: an explicit `transacted { datasource }` route block

A route composes steps; the block is one more composite step:

```yaml
- transacted:
    datasource: pg
    steps:
      - to: "sql:update account set balance = balance - 10 where id = 1?datasource=pg"
      - to: "sql:insert into outbox (id, payload) values (#, #)?datasource=pg"
```

- Authorship is identical through the fluent builder
  (`RouteBuilder::transacted("pg", steps)`), YAML (`transacted:` object), and
  the canonical form (`CanonicalStepSpec::Transacted`, ADR-0011 strict —
  every field survives the round trip, no silent drop). Unknown keys are a
  load error naming the key (ADR-0017).
- Scope and commit point are LEXICAL. They are visible in the route text and
  are never inferred from exchange completion, exchange extensions, task
  membership or call chains.
- Executed semantics (slice 2): the transaction begins lazily at the first
  joined `sql:` step on the block's datasource, commits when the body returns
  `Ok` or a clean `PipelineOutcome::Stopped` (propagated unchanged per
  ADR-0025), and rolls back on `Err`, cancellation or timeout. A `direct:`
  callee's SQL steps do NOT inherit the session: the scope is the block, not
  the call stack.
- Every `sql:` step on the block's datasource inside the block joins the same
  transaction. Membership is the lexically baked scope, not per-step opt-in.
  This is what makes `max_connections=1` safe: a joined step reuses the block's
  connection instead of checking out a second one and deadlocking.
- Fail-closed authoring: a block that contains a step the loader cannot prove
  is joined — StreamList production, a parallel split, multicast, wiretap, an
  async broker handoff (`kafka`, `jms`, `activemq`, `artemis`, `mqtt`) or a
  `seda:` escape reaching a joined SQL step, a SQL step on a foreign or unnamed
  datasource, or a nested block — is rejected at load with a typed error naming
  the offender and the datasource. Nothing is downgraded to auto-commit. Blocks
  are found wherever they are authored: route body,
  `error_handler.on_exceptions` clauses and `circuit_breaker.fallback`, since
  handler lowering keeps only processor steps.

### (b) Rejected surface (a): per-exchange implicit transactions

The first candidate opened a transaction at exchange ingress on the route's SQL
datasource and committed at exchange completion. It is rejected for two
reasons from the 2026-11-15 ruling:

- **Invisible scope.** The boundary is inferred, not authored. A route author
  cannot see where the transaction starts or what runs inside it; adding an
  HTTP call or a wiretap silently widens the transaction. Reading the route no
  longer answers "what is atomic here?", which is the only question an outbox
  needs answered.
- **Cancellation ambiguity.** Exchange completion is not a single event: a
  clone can outlive its parent, an error handler can recover, a `Stopped`
  outcome is a clean exit, and a cancellation races finalization. Each case
  forces the runtime to guess commit-versus-rollback, and no guess is both
  safe and useful.

It also holds a pooled connection across every later step, including slow HTTP
or broker sends, and couples core exchange completion to a component resource.
Clones would share the connection invisibly.

### (c) Rejected surface (c): per-endpoint `Managed` everywhere

The second candidate made every `transactionMode=Managed` endpoint own its
transaction on checkout. It is rejected because:

- **No block boundary owns commit/rollback.** Endpoint lifecycle starts and
  stops with exchange handling, not with a route region. Two endpoints on the
  same datasource have no shared owner that can decide to commit both writes.
  The outbox requires exactly that: one commit for two endpoint invocations.
- **Silent auto-commit holes.** Where the endpoint cannot own a boundary it
  falls back to auto-commit, which is the precise degrade class `txnreject`
  removed: the route believes it is transactional while individual writes
  commit independently. A transaction whose failure mode is "some writes
  survive" is worse than no transaction, because it fails silently.

Per-endpoint config also cannot express "the next two steps share a
transaction" without a route-level composition concept — which is surface (b).

### (d) Concurrency and pool cost

- One pooled connection plus one dedicated session per ACTIVE block. Two
  concurrent exchanges through the same route use two connections and two
  PostgreSQL backends; connection sharing between blocks never happens.
- Joined steps never check out from the pool; they borrow the block's guarded
  connection. `max_connections=1` therefore does not self-deadlock when a
  block has several SQL steps.
- `begin` and connection acquisition are bounded by the datasource
  `acquire_timeout` (sqlx default 30 s). Expiry surfaces the typed
  `PoolExhausted` error naming the datasource, `max_connections` and time
  waited — never an unbounded wait.
- A session admits at most one concurrent join (`try_lock`); a second join
  returns the typed `Busy` error naming the datasource. The block timeout
  (default 30 s, override lands in slice 2) bounds body execution, with a 5 s
  finalize grace for commit and rollback.
- Finalization cost is bounded and the connection is detached
  (`DiscardOnDrop`) when its outcome is uncertain, so a poisoned connection
  never re-enters the pool.

### (e) At-least-once window between DB commit and idempotent-key confirm

The block goes INSIDE the `idempotent_consumer` body, so a rollback fails the
exchange and key disposition follows ADR-0099 exactly (`remove_on_failure`
default `true` removes the key and permits redelivery; explicit `false` retains
the reservation with its load-time warning; a failed removal enters the
pending-set single-owner recovery path; `on_missing_key: process` with an
absent key writes nothing on the failure path).

The window between a successful DB COMMIT and the idempotent-key confirmation
is unavoidable and is an inherent AT-LEAST-ONCE window, not a bug to close:
the exchange can fail after commit and before the key is marked, and
redelivery will then replay the work. The required mitigation is documented
and structural:

- outbox rows carry a UNIQUE message id;
- the relay/downstream deduplicates on that id.

This ADR records the window explicitly so no later review treats a duplicate
delivery after commit as a transaction defect. Exactly-once end-to-end is not
claimed.

### (f) Restart-on-upgrade clears statement caches

sqlx statement caches are per connection and do not survive process exit. A
pre-upgrade binary can seed a poisoned cache entry on a fresh connection
(`sqlbinds` finding); an upgraded process must be restarted rather than
hot-patched so those per-connection entries are gone. On upgrade: restart. The
agreement rule landed by `sqlbinds` already neutralizes disagreeing entries on
the migrated paths (the entry is bypassed, not repaired); this ADR records the
operational consequence, not a new repair mechanism.

### (g) PgBouncer session mode must keep `server_reset_query=DISCARD ALL`

The block relies on session-level state: an open transaction, and a poison
discard on detach. PgBouncer in session mode must keep its default
`server_reset_query = DISCARD ALL`, so that a returned connection carries no
prepared statements, no transaction state and no `SET LOCAL` residue into the
next borrower. Disabling that reset turns connection reuse into cross-tenant
state leakage, including a `DISCARD ALL`-less return of an aborted transaction.
Transaction-pooling mode is not supported for blocks: it cannot guarantee that
a session's statements reach the same backend.

### (h) Slice order

Slices are separate changes with separate bd issues; each amends this ADR.

- **S1 — `sqltrans-portmodel` (`rc-qv2iz.5`)**, this change, DARK. Ships the
  `camel-api` `TransactionManager` / `TransactionSession` port and typed
  errors, the `camel-core` `TransactionManagerRegistry` with fail-closed
  resolution, `BuilderStep::Transacted`, and the declarative authoring surfaces
  (YAML, JSON, template) with ALL load rejections. Those rejections hold on the
  DECLARATIVE load path and on the canonical lowering path through ONE shared
  load walk over the lowered step tree (`sqltrans-exec` step 3; pinned by
  `transacted-canonical-walk`). A `RouteDefinition` registered already lowered
  (`CamelContext::add_route_definition` → `RuntimeBus::register_route` →
  `handle_register_internal`) passes no lowering entry and does not run the
  registry-backed walk: a hand-authored block skips the structural and per-arm
  checks, and every pre-lowered block skips manager and dialect resolution.
  Today such a block fails closed at route-add with the typed
  `Transaction(TransactionError::JoinSessionMissing { datasource })` error,
  surfaced stringified; the executor slice may not remove that backstop before
  the walk runs there (bd rc-ltfyp, blocks `rc-ggo3d`). Bundle boot registers
  one manager per catalog datasource, so a block on a catalog datasource passes
  load; `Managed` keeps the current endpoint-creation rejection. Runtime
  behavior is unchanged.
- **S2 — `sqltrans-runtime` (`rc-qv2iz.6`)**: execution, finalization, join
  semantics, hygiene, detach, timeout, pool-exhaustion surface, camel-sql
  manager implementation over sqlx, and the docker suite. The registry seam
  S2 implements is the canonical lowering arm added in S1: load-time
  resolution is already the choke point, so S2 supplies managers, not a new
  path.
- **S3 — `sqltrans-modes` (`rc-qv2iz.7`)**: `transactionMode` semantics
  rework. `Managed` becomes join-required (a `Managed` step outside a block is
  a typed load error), `Auto` inside a block joins, and the endpoint-creation
  rejection is removed in favor of route-level validation. `txnreject` tests
  are rewritten to the new contract, not deleted; URI-secret redaction stays.

### Recorded decision: the async-handoff classifier is the telemetry table

Load validation for the async-handoff rejection (req 6) reuses
`camel_core::is_async_producer_scheme` — the wrapper over the
`PRODUCER_SCHEMES` table (`kafka`, `jms`, `activemq`, `artemis`, `mqtt`) that
the span-kind classifier already uses. There is no second scheme list.

`seda:` is handled by its own dedicated check in the validator, because
`seda` is deliberately absent from the producer table (an in-process queue
handoff is not a broker send). `uri_span_kind`'s `CLIENT_SCHEMES` table is NOT
consulted: a synchronous client call is not an async handoff.

The coupling is recorded because it makes the validator semantic depend on a
telemetry table: adding a broker scheme to `PRODUCER_SCHEMES` changes load
rejection behavior as a side effect. Slice 2 inherits this consciously. If
the two concerns ever diverge (a scheme that is async for tracing but safe
inside a block, or vice versa), the fix is an explicit validation set with a
test that pins both directions — not a silent copy.

## Consequences

Easier:

- Atomicity is authored, visible and reviewable in the route text.
- The outbox is expressible: two writes, one commit, no component-private
  mechanism.
- The port is provider-neutral; `camel-api` gains no sqlx dependency and no
  value/row model, so slice 2 can implement it in `camel-sql` without a
  signature change.
- Fail-closed load validation means a block that cannot be proven safe never
  runs, rather than running in a degraded auto-commit mode.
- `max_connections=1` is safe by construction for joined steps.

Harder:

- Routes that use SQL inside a block are PostgreSQL-only in v1. A joined step
  resolving to SQLite, MySQL or an unprovable dialect is rejected
  (`UnsupportedJoinDialect` / `DialectUnprovable`); standalone MySQL, SQLite and
  manual-transaction usage outside blocks is unchanged.
- A per-exchange "just wrap this exchange" workflow is not available and will
  not be added; such a route must author a block.
- The dynamic-routing and runtime-SQL convenience cases (`header:`/body SQL,
  `file:` SQL) are unavailable inside blocks, because a joined step's SQL must
  be static route-configured text.
- A block holds one connection for its duration; a slow non-SQL step inside a
  block lengthens connection occupancy. That is the intended cost of a visible
  boundary, and the block timeout bounds it.
- The at-least-once window remains. Exactly-once is an outbox-plus-dedup
  property, not a transaction property.
