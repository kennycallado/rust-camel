## ADDED Requirements

### Requirement: Idempotent consumer step reservation semantics

The `idempotent_consumer` step SHALL treat an absent or empty message-id as a missing key and SHALL resolve it according to an `on_missing_key` setting with values `fail` and `process`, defaulting to `fail`, on both the synchronous and asynchronous arms of the step. Under `fail`, the step SHALL fail the exchange with an error naming the step and the repository, and SHALL NOT run the child sub-pipeline. Under `process`, the step SHALL log a warning and forward the exchange to the child sub-pipeline without repository interaction. An empty string key SHALL be treated identically to an absent key, and the synchronous arm SHALL NOT register an empty key in the repository.

#### Scenario: sync arm, absent key, fail is the default

- **GIVEN** a route with an `idempotent_consumer` step that omits `on_missing_key` and whose synchronous key extractor returns nothing
- **WHEN** such an exchange is processed
- **THEN** the outcome is `Failed` with an error naming `idempotent_consumer` and the repository, and the child sub-pipeline does not run

#### Scenario: async arm, absent key, fail is the default

- **GIVEN** a route with an `idempotent_consumer` step that omits `on_missing_key` and whose asynchronous key expression evaluates to null
- **WHEN** such an exchange is processed
- **THEN** the outcome is `Failed` with an error naming `idempotent_consumer` and the repository, and the child sub-pipeline does not run

#### Scenario: missing-key process forwards with a warning on both arms

- **GIVEN** an `idempotent_consumer` step configured with `on_missing_key: process`, tested once with a synchronous extractor returning nothing and once with an asynchronous expression evaluating to null
- **WHEN** each exchange is processed
- **THEN** a warning is logged in both cases, the child sub-pipeline runs in both cases, and the repository is never consulted for those exchanges

#### Scenario: empty key is treated as absent on the sync arm

- **GIVEN** an `idempotent_consumer` step whose synchronous key extractor returns `Some("")` for a message
- **WHEN** that message is processed under the default `on_missing_key: fail`
- **THEN** the step behaves exactly as if the key were absent (the exchange fails under `fail`), and no empty-string key is ever registered in or read from the repository

#### Scenario: empty key is treated as absent on the async arm

- **GIVEN** an `idempotent_consumer` step whose asynchronous key expression evaluates to an empty string
- **WHEN** that message is processed under `on_missing_key: process`
- **THEN** the child sub-pipeline runs and the repository is not consulted, identically to an absent key

### Requirement: DSL surface for on_missing_key

The `idempotent_consumer` DSL step SHALL accept an optional `on_missing_key` field with the string values `fail` and `process`; an absent field SHALL default to `fail`; an invalid value SHALL be rejected at parse time with an error naming the field; an unknown sibling field SHALL be rejected by `deny_unknown_fields`.

#### Scenario: valid values parse

- **GIVEN** two routes whose `idempotent_consumer` steps set `on_missing_key: "fail"` and `on_missing_key: "process"`
- **WHEN** each route is parsed
- **THEN** parsing succeeds and the compiled step carries the corresponding `OnMissingKey` value

#### Scenario: invalid value is rejected at parse time

- **GIVEN** a route whose `idempotent_consumer` step sets `on_missing_key: "skip"`
- **WHEN** the route is parsed
- **THEN** parsing fails with an error naming `on_missing_key` and the accepted values

#### Scenario: default is fail

- **GIVEN** a route whose `idempotent_consumer` step omits `on_missing_key`
- **WHEN** the route is compiled
- **THEN** the compiled step carries `on_missing_key = fail`

#### Scenario: unknown field is rejected

- **GIVEN** a route whose `idempotent_consumer` step sets an unknown field `on_duplicate: true`
- **WHEN** the route is parsed
- **THEN** parsing fails with an unknown-field error

### Requirement: Eager reservation rolls back by default

The system SHALL default `remove_on_failure` to `true` when `eager` is enabled, so a failed child sub-pipeline rolls back the eager reservation and a route-level retry re-executes the child. An explicit `remove_on_failure: false` SHALL remain an opt-in at-most-once contract and SHALL raise a load-time warning stating that semantic.

#### Scenario: eager rollback default

- **GIVEN** an `idempotent_consumer` step with `eager: true` and no `remove_on_failure` field, on a route with retries
- **WHEN** the child sub-pipeline fails and the same key is delivered again
- **THEN** the reservation is removed, the retry runs the child sub-pipeline again, and the route does not report success with zero work

#### Scenario: explicit at-most-once opt-in warns at load time

- **GIVEN** an `idempotent_consumer` step with `eager: true` and explicit `remove_on_failure: false`
- **WHEN** the route is compiled
- **THEN** a load-time warning naming the step and the at-most-once contract is emitted, and on a failed child the reservation is kept so the retry skips the child as an observable duplicate

### Requirement: Failed rollback fails the exchange and recovery is single-owner

The step SHALL fail the exchange when an eager rollback fails. The step SHALL return the child's error unchanged as the primary error so that structural `on_exceptions` retry predicates continue to match the child's error variant, and SHALL attach the rollback failure as structured context (an error-level log carrying both errors). The step SHALL record the key in a pending-rollback state shared across clones of the segment. Ownership of a pending key SHALL be taken atomically under the shared lock, and an in-flight marker SHALL span the recovery from ownership take until the key's reservation step resolves: in eager mode until the pre-child reservation resolves, in lazy mode until the post-child bookkeeping resolves — and the marker SHALL be cleared whenever the reservation step resolves, so a settled key never remains in flight. A delivery whose key is in-flight SHALL fail loudly with an error naming the step, the repository, and the recovery-in-progress reason, without repository operations and without running the child. The recovery owner SHALL retry the repository removal: on failure it SHALL return the key to the pending state and fail the exchange with the removal error; on success it SHALL proceed to reserve and run the child exactly once for the key. The step SHALL NOT use redelivery headers to drive this rule.

#### Scenario: failed rollback fails the exchange and preserves the child error type

- **GIVEN** an `idempotent_consumer` step with `eager: true` whose repository removal fails when the child fails
- **WHEN** the child sub-pipeline fails with an `Io` error and the removal also fails
- **THEN** the outcome is `Failed` with a `CamelError::Io` matching the child's error variant, an error-level log carries both the child error and the rollback error, and the key is recorded in the pending-rollback state

#### Scenario: pending removal fails again

- **GIVEN** a pending-rollback key whose repository removal fails again on the next delivery
- **WHEN** a new delivery with that key arrives and the recovery removal fails
- **THEN** the outcome is `Failed` with the removal error, the key returns to the pending state, and the repository contains/add operations are not consulted and the child does not run

#### Scenario: pending rollback is retried before the reserve on re-entry

- **GIVEN** a pending-rollback key after a failed removal
- **WHEN** a new delivery with the same key arrives and the repository removal now succeeds
- **THEN** the step reserves the key and runs the child sub-pipeline exactly once — the delivery is never reported as a duplicate or a success with zero work

#### Scenario: raced delivery during removal, before reservation, fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has taken ownership but has not yet completed the repository removal, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key while the removal is in progress
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add/remove performed by that clone and no child run

#### Scenario: raced delivery after removal, before reservation, fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has completed the repository removal successfully but has not yet resolved its reservation, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key inside that window
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add performed by that clone and no child run, and the recovery owner then reserves and runs the child exactly once — this window is where premature clearing of the in-flight marker would let the second clone reserve and win the race

#### Scenario: raced delivery during in-flight recovery fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has taken ownership but has not yet completed the removal-to-reservation span, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key while recovery is in flight
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add/remove performed by that clone and no child run, the recovery owner completes the removal, reserves, and runs the child exactly once, and no delivery with that key is reported as `Completed` with zero work

### Requirement: Duplicate skips and bookkeeping failures are observable

The step SHALL set the exchange property `camel.idempotent.duplicate` to `true` and SHALL increment the counter `camel.idempotent.duplicates` (label `repository`) whenever the child sub-pipeline is skipped as a duplicate, on both the contains-hit path and the eager race path. A failed non-eager repository add after a successful child SHALL keep the exchange successful, log at error level, and increment the counter `camel.idempotent.add_failures` (label `repository`).

#### Scenario: duplicate skip sets property and counter

- **GIVEN** an `idempotent_consumer` step whose repository already contains the key
- **WHEN** an exchange with that key is processed
- **THEN** the child sub-pipeline is skipped, the exchange carries `camel.idempotent.duplicate = true`, and the counter `camel.idempotent.duplicates` with the repository label is incremented

#### Scenario: eager race duplicate is observable

- **GIVEN** an `idempotent_consumer` step with `eager: true` where a concurrent delivery already reserved the key
- **WHEN** the step's reservation add reports the key already exists
- **THEN** the child sub-pipeline is skipped, the exchange carries `camel.idempotent.duplicate = true`, and the counter `camel.idempotent.duplicates` is incremented

#### Scenario: non-eager add failure keeps success with an error log

- **GIVEN** an `idempotent_consumer` step with `eager: false` whose repository add fails after the child completed
- **WHEN** the child sub-pipeline completes successfully and the post-success add fails
- **THEN** the exchange stays successful, an error-level log is emitted, and the counter `camel.idempotent.add_failures` with the repository label is incremented
