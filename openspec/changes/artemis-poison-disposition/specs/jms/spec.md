# artemis-poison-disposition — Delta specs

## ADDED Requirements

### Requirement: Artemis fixture exposes a broker-side disposition query

The dedicated Artemis test fixture (`ArtemisBroker`) SHALL expose a stable
broker-side queue-stat query that reads what the broker holds per queue:
`MESSAGE COUNT` and `MESSAGES ADDED`, for a caller-named queue. The query
SHALL run inside the broker container through `docker exec` using the
container's own instance CLI
(`/var/lib/artemis-instance/bin/artemis queue stat --queueName <queue>`),
with the fixture's credentials as CLI arguments. Each exec SHALL carry a
process deadline (poll, then kill and reap on expiry; an expired exec is an
error). The parser SHALL be a pure function over the pinned 2.36.0 table
output returning three distinguishable outcomes: a matching row with numeric
counts, a well-formed filtered table without the row (`Ok(None)`, documented
as unreachable for the poison scenario because the source queue, `DLQ`, and
`ExpiryQueue` all exist on the image-created broker), and malformed output
(malformed headers or non-numeric counts) as an error naming the mechanism.
During an observation, `Ok(None)` for one of the scenario's expected queues
SHALL fail the observation; absence SHALL NOT be read as zero counts or as an
acknowledged-removed outcome. A failed exec, an expired deadline, or an
unparsable table SHALL NOT be reported as any disposition.

#### Scenario: Query reads broker-held counts

- **GIVEN** a running dedicated Artemis container
- **WHEN** the fixture queries a queue that holds messages
- **WHEN** the fixture queries `DLQ` after a dead-letter delivery
- **THEN** the returned counts come from the broker's own management data
- **AND** no client-side delivery observation is involved

#### Scenario: Malformed output is an error

- **GIVEN** table output with malformed headers or non-numeric counts
- **WHEN** the parser runs
- **THEN** it SHALL return an error naming the mechanism
- **AND** it SHALL NOT return counts

#### Scenario: Absent expected queue fails the observation

- **GIVEN** a well-formed table without a row for an expected scenario queue
- **WHEN** the disposition observation reads that queue
- **THEN** the observation fails
- **AND** it SHALL NOT be read as zero counts or acknowledged-removal

#### Scenario: Query failure is an error, not a disposition

- **GIVEN** the exec fails or its deadline expires
- **WHEN** the fixture is asked for a queue stat
- **THEN** it SHALL return an error naming the mechanism
- **AND** it SHALL NOT return counts or an inferred disposition

### Requirement: Artemis poison disposition test asserts the broker-side outcome

The Artemis redelivery-disposition case SHALL read the broker-side state
through the fixture query and its assertion SHALL name the broker-side
outcome: acknowledged-and-removed (source `MESSAGE COUNT` 0 with zero
`MESSAGES ADDED` on `DLQ` and `ExpiryQueue`), or native dead-letter/expiry
evidence. Native-first precedence SHALL be reported only with numeric
broker-side evidence, never inferred from a missing bridge `COMMIT`. The case
SHALL fail when the query is removed, errors, or otherwise yields no broker
evidence. The six durable reservation/rollback assertions, the single
terminal `COMMIT`, and the native terminal-record assertions SHALL remain
unchanged; this evidence is additive.

#### Scenario: Broker-side outcome is asserted

- **GIVEN** the threshold scenario ran on the dedicated Artemis broker
- **WHEN** the disposition case reads the broker-side counts
- **THEN** the assertion names the observed outcome
- **AND** acknowledged-and-removed requires an empty source queue and zero
  `DLQ`/`ExpiryQueue` deliveries

#### Scenario: Query removal fails the case

- **GIVEN** the broker-side query is removed, errors, or reports unknown
- **WHEN** the disposition case runs
- **THEN** the case fails
- **AND** the durable and terminal assertions still run unchanged

### Requirement: Disposition query evidence limits are documented

The test-support documentation SHALL state which broker mechanism backs the
query, why it is stable for the verified
`apache/activemq-artemis:2.36.0-alpine` image pin (scoped to that pin, not
extrapolated to other Artemis 2.x versions), and what the query does not
prove: point-in-time management counts under the isolated
dedicated-container scenario only (not journal layout, not byte persistence,
not a universal proof against message loss), no crash-proofness claim, and
broker-lifetime accumulation of `MESSAGES ADDED` (exact because each case
owns a fresh dedicated container). The documentation SHALL keep the two
terminal kinds distinct: the bridge's terminal discard is an acknowledgement
(source drains, zero `DLQ` additions), while a broker-native dead-letter
moves the message to `DLQ` (non-zero `MESSAGES ADDED` there); with this
image's default addressing (`dead-letter-address=DLQ` under `match="#"`)
there is no silent-drop route, so the counts always distinguish
acknowledgement from dead-letter routing.

#### Scenario: Documentation names mechanism and limits

- **GIVEN** a reader of the Artemis fixture docs
- **WHEN** they consult the disposition-query documentation
- **THEN** the mechanism and its stability argument are stated
- **AND** the explicit non-proofs are stated
