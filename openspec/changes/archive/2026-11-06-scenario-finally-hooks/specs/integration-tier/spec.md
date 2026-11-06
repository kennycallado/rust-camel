# Delta: integration-tier

## ADDED Requirements

### Requirement: Document teardown with finally

A scenario document MAY declare a document-level `finally:` key, sibling of `scenario:`,
carrying an ordered teardown action list. The allowed set is exactly: `sleep`, `sql`,
`surreal`, and the `redis` delete prepare action. A `send`, `receive`, or `validate` entry
in `finally:` SHALL fail at load with a `doc-validation` error naming the action key and
the `finally:` position. A declared-but-empty `finally:` list SHALL fail at load, mirroring
the empty-`prepare` law. A document without the key SHALL behave exactly as before. Grammar
validation of `finally:` entries SHALL run before the feature demand gate (the `sql:`
precedent): a featureless build reports the grammar error for a malformed entry and the
named demand-gate error for a valid one.

Teardown SHALL run after the scenario verdict is fixed, before the report finalises, on
BOTH verdict paths. Each item SHALL run inside panic containment at the document boundary:
a panicking item SHALL be reported as the apparatus class `teardown-failure` (exit 2) and
SHALL NOT abort the process; the next item and the next document still run. A teardown
failure SHALL be recorded as `teardown-failure` after the recorded verdict and SHALL NEVER
mask or rewrite it: a passed scenario stays a pass with a `teardown-failure` record; a
failed scenario keeps its verdict-class failure. When teardown and boot shutdown both
fail, both records SHALL render.

The document outcome SHALL record an execution-order trace: one event per completed
scenario action, the verdict-fixed marker when a pass verdict is fixed, the logs-evaluated
marker after the logs evaluation, and one event per completed teardown item, strictly in
execution order. A failed scenario SHALL record no verdict-fixed marker.

#### Scenario: finally key parses and accepts prepare actions

- **GIVEN** a document whose `finally:` carries a `redis` del action and a `sleep` action
- **WHEN** the document loads
- **THEN** both entries parse and the document loads

#### Scenario: finally rejects a verdict action at load

- **GIVEN** a document whose `finally:` contains a `send` action
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming `send` and the `finally:` position

#### Scenario: finally is optional

- **GIVEN** a document without a `finally:` key
- **WHEN** the scenario runs
- **THEN** behavior is unchanged from the current runner

#### Scenario: declared-but-empty finally list is a load error

- **GIVEN** a document declaring `finally: []`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming `finally`

#### Scenario: finally runs after the scenario verdict

- **GIVEN** a passing scenario with two `finally:` items
- **WHEN** the scenario runs
- **THEN** the recorded trace reads verdict-fixed after the last action event and teardown
  events after it, in declared order

#### Scenario: finally failure does not mask a pass

- **GIVEN** a passing scenario whose single `finally:` item fails
- **WHEN** the run reports
- **THEN** the verdict stays recorded and a `teardown-failure` record renders after it,
  with exit 2

#### Scenario: finally panic is contained and reported

- **GIVEN** a `finally:` item that panics
- **WHEN** the teardown runs
- **THEN** the panic is reported as `teardown-failure` and the runner still executes the
  next document

#### Scenario: finally runs when the scenario fails

- **GIVEN** a scenario whose `validate` action asserts a variable target with an
  expectation no extract ever satisfies
- **WHEN** the scenario fails its verdict
- **THEN** every `finally:` item still runs after the failure and the recorded trace
  carries no verdict-fixed marker

#### Scenario: grammar validation precedes the demand gate

- **GIVEN** a build without the `sql` feature and a document whose `finally:` carries a
  malformed `sql:` entry
- **WHEN** the document loads
- **THEN** the load fails with the grammar error naming the `finally:` position, not the
  demand-gate error

#### Scenario: teardown failure and shutdown failure both render

- **GIVEN** a scenario whose `finally:` item fails and whose boot shutdown also fails
- **WHEN** the run reports
- **THEN** the `teardown-failure` record renders first and the `shutdown-failure` record
  renders after it, both apparatus-class

### Requirement: Redis delete prepare action

The scenario grammar SHALL accept a delete-only redis prepare action under the key
`redis: {datasource: <name>, del: [<keys>...]}`, in BOTH the `scenario:` list and the
`finally:` list. The `datasource` SHALL be a configured datasource name and every key
SHALL be a document-authored literal; neither is env-interpolated (the identifier law).
The `del` list SHALL be non-empty. A key containing a glob metacharacter (`* ? [ ] { }`)
SHALL fail at load naming the key. The action MUST be visibly distinct from the `validate`
redis target grammar (`datasource/key/type/ttl`): the prepare form carries `datasource` and
`del`. There SHALL be no `FLUSHDB`, no `FLUSHALL`, no pattern delete, and no generic
command runner; a pattern delete is a later, namespace-scoped exception.

Load-time grammar rules SHALL run in every build before the `redis` demand gate (the
`sql:` precedent); when the feature is off, a valid entry SHALL fail at load with a named
demand-gate error, and a directly-constructed document SHALL fail closed at action time.

Execution (feature `redis`) SHALL resolve the datasource through the same
`DatasourceCatalog` the booted composition root built (one datasource name, one connection
handle; the executor SHALL NOT construct its own), SHALL issue one `DEL` over the literal
keys, and SHALL treat missing keys as success. A driver error SHALL surface as an
apparatus-class failure carrying the datasource name and ADR-0051-sanitized driver text,
with the resolved URL redacted. The boot selection scan SHALL route a document whose
`scenario:` or `finally:` needs the catalog (including this action) to the full boot.

#### Scenario: finally redis del removes exactly the listed keys

- **GIVEN** a document whose `finally:` carries a redis del action listing key `k1`
- **WHEN** the teardown runs against the catalog datasource with keys `k1` and `sentinel`
  seeded
- **THEN** `k1` is gone and `sentinel` is untouched

#### Scenario: redis del rejects a pattern key

- **GIVEN** a redis del action whose `del` list contains `orders:*`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the key and the metacharacter

#### Scenario: empty del list is a load error

- **GIVEN** a redis del action with `del: []`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action

#### Scenario: redis del in the scenario list selects the full boot

- **GIVEN** a document whose only action is a redis del (no wired endpoints, no validate
  target)
- **WHEN** `camel test` selects the execution path
- **THEN** the document boots with the datasource catalog, not the smoke path

## MODIFIED Requirements

### Requirement: Failure taxonomy

Scenario runs SHALL keep the epistemic exit split: 0 = pass, 1 = verdict-class (system under test failed), 2 = apparatus-class (harness failure) and load/misuse errors. Harness-side queue overflow conditions SHALL surface as apparatus-class failures carrying their own name, never as verdict-class receive timeouts. A document-declared teardown failure SHALL surface as its own apparatus class `teardown-failure` after the recorded verdict, never masking it.

#### Scenario: arrival lane overflow is an apparatus failure

- **GIVEN** a partner path whose arrival lane dropped arrivals after filling
- **WHEN** a subsequent receive on that path times out
- **THEN** the run fails with exit 2 as an apparatus arrival-lane-overflow naming the dropped count, not as a verdict receive-timeout

#### Scenario: client lane FIFO overflow is an apparatus failure

- **GIVEN** more same-key in-flight sends than the client lane FIFO bound
- **WHEN** the overflowing send launches
- **THEN** the send fails apparatus-class naming the lane key and the bound
#### Scenario: receive timeout is a verdict failure

- **GIVEN** a `receive` with a deadline whose partner never sends
- **WHEN** the deadline elapses
- **THEN** the run reports `receive-timeout` and exits 1

#### Scenario: infra absence fails named, never hangs

- **GIVEN** a scenario requiring a broker that is absent
- **WHEN** the runner starts
- **THEN** the run reports `infra-unavailable` naming the requirement and
  exits 2 within its startup deadline

#### Scenario: shutdown failure does not mask the verdict

- **GIVEN** a scenario that passed its assertions
- **WHEN** teardown times out during shutdown
- **THEN** the verdict stays recorded and the run reports `shutdown-failure`
  with exit 2

#### Scenario: teardown failure does not mask the verdict

- **GIVEN** a scenario that passed its assertions and whose declared `finally:` item fails
- **WHEN** the teardown runs after the verdict is fixed
- **THEN** the verdict stays recorded, the run reports `teardown-failure` after it with
  exit 2, and a failed scenario's verdict-class failure is likewise never rewritten
