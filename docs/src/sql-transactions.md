# SQL transactions

The transaction surface is the explicit `transacted` route block. A route names
the datasource and the steps that share one database transaction:

```yaml
- transacted:
    datasource: pg
    steps:
      - to: "sql:update account set balance = balance - 10 where id = 1?datasource=pg"
      - to: "sql:insert into outbox (id, payload) values (#, #)?datasource=pg"
```

The scope and the commit point are lexical: they appear in the route text and
are never inferred from exchange completion. The design ruling, the rejected
alternatives, the pool cost, and the at-least-once window are recorded in
[ADR-0101](../adr/0101-transacted-block-surface.md).

## Status

The surface, the model, the load validation and the manager registration
exist; execution does not. Bundle boot registers one `SqlTransactionManager`
per catalog datasource, so a `transacted` block passes load through the
single walk and fails closed at route-add with `JoinSessionMissing` (the
executor slice is pending).

Beyond the slice-1 rejections listed below, the load walk now rejects, each
typed and naming the offender:

- a joined sql step issuing a transaction-control or implicit-commit
  statement (`TransactionControlSql`);
- a joined sql step whose SQL comes from a runtime source, including a
  `file:` URI path (`RuntimeSqlInJoinedStep`);
- a URL scheme whose transaction semantics cannot be proven
  (`DialectUnprovable`);
- a dialect that does not support joined transactions in v1
  (`UnsupportedJoinDialect`);
- a dynamic routing target inside a block (`DynamicTargetUnprovable`).

## What exists after slice 1 (`sqltrans-portmodel`)

All of it is dark: it compiles and is tested, and it changes no runtime
behavior.

- **Port.** `camel-api` exposes the provider-neutral `TransactionManager` and
  `TransactionSession` traits with typed `TransactionState`,
  `TransactionOutcome` and `TransactionError`, and the
  `CamelError::Transaction` variant. `camel-api` gains no sqlx dependency and
  no row/value model.
- **Registry.** `camel-core` owns a `TransactionManagerRegistry` keyed by
  datasource name. Resolution fails closed: an absent name produces the typed
  `ManagerMissing { datasource }` error, never a default or an auto-commit.
- **Route model.** `transacted { datasource, steps }` is expressible through
  the fluent builder, YAML and the canonical form, with identical lowering.
  Unknown keys inside the block are a load error naming the key, and the
  canonical round trip preserves every field.
- **Load rejections.** Each is fail-closed, typed, and names the offender and
  the datasource:
  - StreamList production anywhere inside a block (unconditional);
  - a parallel split, multicast or wiretap reaching a joined SQL step;
  - an async broker handoff (`kafka`, `jms`, `activemq`, `artemis`, `mqtt`) or
    a `seda:` escape reaching a joined SQL step;
  - a SQL step on a datasource other than the block's, or without a named
    datasource (a bare `sql:` step included);
  - a nested `transacted` block;
  - a block on a datasource with no registered manager (`ManagerMissing`).

Blocks are found wherever they are authored: the route body, every
`error_handler.on_exceptions` clause, and `circuit_breaker.fallback`.

`transactionMode=Managed` keeps its current endpoint-creation rejection; slice
3 replaces that rejection with route-level validation.

## Lands in slice 2 (`sqltrans-runtime`)

- Beginning the transaction lazily at the first joined SQL step and committing
  on a clean body exit, rolling back on error, cancellation or timeout.
- Joining every SQL step on the block datasource into one transaction,
  including sequential splits; `direct:` callees do not inherit the session.
- Finalization semantics: bounded commit/rollback, uncertain-outcome detach,
  pool-exhaustion and block-timeout errors, the `Busy` join error.
- The `camel-sql` `TransactionManager` implementation over sqlx, and the
  Docker PostgreSQL suite that proves the runtime scenarios.

### Operational note

Lands in slice 2. It will cover admission hygiene, detach semantics, block and
acquire timeouts, restart-on-upgrade statement-cache clearing, and the
PgBouncer requirement (session mode with `server_reset_query=DISCARD ALL`).
It is not written here in advance, because slice 2 owns the behavior it
describes.

## Lands in slice 3 (`sqltrans-modes`)

- `transactionMode=Managed` becomes join-required: a `Managed` step outside a
  block is a typed load error, and the endpoint-creation rejection is removed.
- `Auto` inside a block joins the block transaction.
- The `txnreject` tests are rewritten to the new contract; URI-secret
  redaction is retained.

## PostgreSQL-only in v1 (lands in slice 2)

Joined steps are probed for their dialect. PostgreSQL is supported; SQLite,
MySQL and any dialect whose transaction-ending behavior cannot be proven from
configuration are rejected at load. Standalone MySQL, SQLite and manual
transaction usage outside blocks is unchanged.

## At-least-once window (lands in slice 2)

Place the block inside the `idempotent_consumer` body. A rollback then fails
the exchange and key disposition follows
[ADR-0099](../adr/0099-idempotent-reservation-semantics.md). The window between
a successful DB commit and the idempotent-key confirmation is an inherent
at-least-once window: outbox rows must carry a unique message id, and the relay
or downstream must deduplicate on it. Exactly-once delivery is not claimed.
