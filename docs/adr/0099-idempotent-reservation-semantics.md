# ADR-0099: Idempotent reservation semantics

- Date: 2026-11-14
- Status: Accepted (2026-11-14)
- Context: change `idemon`; bd `rc-36ujy`, bd `rc-v67ye`
- Amends: none
- Cross-refs: [ADR-0023](0023-idempotent-repository-trait.md) (idempotent
  repository contract C1), [ADR-0032](0032-exchange-data-trust-boundary.md)
  (exchange-data trust boundary), [ADR-0090](0090-pre-1.0-compatibility-policy.md)
  (pre-1.0 compatibility policy)

## Context

Two data hazards reached the demo team.

**Hazard 1: a failed child reports success with zero work.** An eager
`idempotent_consumer` step reserves the key before the child sub-pipeline runs.
A route-level retry then reuses the same key. With no rollback, the first
attempt reserves the key, the child fails, and the retry sees the key as
present. The retry skips the child and the route reports success with zero
work. A failed rollback had the same outcome. The step logged a warning, kept
the reservation, and the retry skipped the child again.

**Hazard 2: absent or empty keys silently disable idempotency.** The async arm
mapped an empty key to `None`. The sync arm accepted `Some("")` as a real key
and registered the empty string in the repository. A missing key disabled the
step with no failure. Duplicate skips were also unobservable: no exchange
property and no metric recorded that a skip happened.

Apache Camel's parity behavior is the wrong default here. Camel lets a missing
key silently disable idempotency. Three facts reject that default for
rust-camel:

1. The key comes from untrusted exchange data
   ([ADR-0032](0032-exchange-data-trust-boundary.md)). An absent or malformed
   key is an input defect, not an operator instruction.
2. A duplicate side effect cannot be undone. A payment or an order is not
   reversible after the fact.
3. [ADR-0090](0090-pre-1.0-compatibility-policy.md) asks for loud failure over
   silent fallback. rust-camel is pre-1.0, so the contract changes now.

Contract C1 of [ADR-0023](0023-idempotent-repository-trait.md) already forbids
one form of silent fallback: a repository failure is `Err`, never "key
absent". This ADR applies the same rule to the key itself and to the rollback
path.

## Decision

### 1. Fail closed on a missing key

The step gains `on_missing_key` with the values `fail` and `process`. The
default is `fail`, on both the synchronous and the asynchronous arm.

- An absent key and an empty key are the same condition. The step never
  registers an empty-string key.
- Under `fail`, the step fails the exchange with an error that names the step
  and the repository. The child sub-pipeline does not run.
- Under `process`, the step logs a warning and forwards the exchange to the
  child sub-pipeline with no repository interaction.

### 2. Eager reservation rolls back by default

`remove_on_failure` defaults to `true` when `eager` is enabled. A failed child
then rolls back the reservation, and a route-level retry re-runs the child.

An explicit `remove_on_failure: false` stays available as an opt-in
at-most-once contract. The DSL emits a load-time warning that states the
contract.

A load-time rejection of `eager` with retries is not feasible, and this ADR
does not attempt one. The global error handler is invisible to the DSL.
`on_exceptions` retries are runtime-predicated, so the DSL cannot prove that a
route retries.

### 3. A failed rollback fails the exchange

When an eager rollback fails, the exchange fails.

The step returns the child's error unchanged as the primary error. Route-level
`on_exceptions` predicates match on the error structure, so a wrapper would
silently break retries. The rollback failure is attached as structured
context: an error-level log carries the child error and the rollback error as
separate fields. The key enters the pending-rollback state of decision 4.

### 4. Pending rollback has a single recovery owner

The segment holds one shared, lock-protected rollback state across all clones.
The state carries `pending` and `in_flight` sets. No await holds the lock.

- A failed eager rollback inserts the key into `pending`.
- At run start, before the contains check, one critical section decides the
  path. A key in `in_flight` fails the delivery loudly with an error that names
  the step, the repository, and the recovery-in-progress reason. A key in
  `pending` moves to `in_flight`, and that clone becomes the recovery owner.
- The owner retries the repository removal. A failed removal returns the key
  to `pending` and fails the exchange with the removal error. A successful
  removal proceeds to reserve and run the child exactly once.
- The in-flight marker spans ownership take to reservation resolution. Eager mode
  clears it when the pre-child reservation resolves. Lazy mode clears it when
  the post-child bookkeeping resolves. A settled key never stays in flight.

A raced delivery fails loudly instead of reserving, because recovery may still
end in a failed rollback. If the step told the raced delivery "duplicate,
done", hazard 1 would return. The step does not use redelivery headers for
this rule: a header is untrusted exchange data
([ADR-0032](0032-exchange-data-trust-boundary.md)).

### 5. A retry must never pass as a duplicate

A retry re-runs the child. The rule is bounded to pending-rollback recovery:
the recovery owner always re-runs the child after a successful removal.

The explicit at-most-once opt-in from decision 2 is the one exception. That
mode intentionally permits observable duplicate skips. The load-time warning
states the contract.

### 6. Duplicate skips and bookkeeping failures are observable

- A duplicate skip sets the exchange property `camel.idempotent.duplicate` to
  `true`. It also increments the counter `camel.idempotent.duplicates` with the
  label `repository`.
- Both duplicate paths report: a contains hit and an eager race where the
  reservation add reports an existing key.
- A failed non-eager repository add after a successful child keeps the
  exchange successful. The step logs at error level and increments
  `camel.idempotent.add_failures` with the label `repository`. The child
  already completed, so failing the exchange would re-run completed work.

### 7. Deliberate divergence from Apache Camel

This ADR records a permanent divergence from Apache Camel, on purpose. Without
this record, a later change will read the divergence as a defect and "restore
parity".

| Case | Apache Camel | rust-camel |
| --- | --- | --- |
| Missing key | silently disables idempotency | fails closed by default |
| Empty key | not distinguished from absent | treated as absent |
| Eager child failure | reservation stays unless configured | rollback by default |
| Failed rollback | warn-only | exchange fails, key enters pending recovery |
| Duplicate skip | not observable | property and counter |
| Raced recovery | not coordinated | single owner, raced deliveries fail loudly |

The cause is the trust model, not a preference
([ADR-0032](0032-exchange-data-trust-boundary.md)). Camel runs in-process and
trusts the exchange. rust-camel does not.

## Consequences

Easier:

- Eager mode is safe with route-level retries. A failed child leaves no
  reservation behind.
- Duplicate skips are observable. A route can count them, and an operator can
  see them.
- A missing key fails at the step that needs it. The error names the step and
  the repository, so diagnosis is direct.
- A rollback failure no longer hides behind a warning. The exchange fails, and
  the key enters a recovery path with one owner.

Harder:

- A route that relies on Camel's silent-missing-key behavior must set
  `on_missing_key: process`. Without that setting, the route now fails on such
  an exchange.
- A route that wants at-most-once under `eager` must set
  `remove_on_failure: false` explicitly, and it gets a load-time warning.
- An invalid `on_missing_key` value is a parse error, not a fallback.

## Migration

This is a breaking change to the step semantics. [ADR-0090](0090-pre-1.0-compatibility-policy.md)
applies: no external commitment covers the old behavior, so the contract
changes now instead of carrying a compatibility shim.

The demo team's interim workaround (a filter or validate step before the
consumer) becomes unnecessary. Use the step setting instead.

```yaml
- idempotent_consumer:
    repository: "orders"
    expression: "${header.orderId}"
    eager: true
    on_missing_key: "process" # only for routes that accept a missing key
    steps:
      - to: "log:first-time"
```

The default route needs no change. It keeps `on_missing_key: fail` and now
rolls back a failed eager reservation.
