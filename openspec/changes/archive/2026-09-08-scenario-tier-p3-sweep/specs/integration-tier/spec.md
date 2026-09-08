## ADDED Requirements

### Requirement: Log-content assertion vocabulary

A scenario document MAY declare one document-level `logs:` block with
`contains`, `regex`, and `noLevelAbove` clauses, evaluated after the action
list completes. The harness SHALL capture every tracing event (level,
target, rendered message) emitted while the document runs, through a
harness-owned capture layer installed at the tier entry BEFORE any
composition-root boot; the capture layer takes the global subscriber seat
first-wins, and later boot attempts degrade to their existing warn-and-skip
path. The capture window SHALL open at document start and close at action
list completion; an event SHALL be attributed to every window open at the
event's timestamp. No new ambient environment reads SHALL be introduced
(ADR-0069 section 4). When several clauses are declared, all SHALL hold
(conjunction); within `contains`/`regex`, every listed entry SHALL be
satisfied.

#### Scenario: malformed logs block is a load error

- **GIVEN** a `logs:` block carrying an unknown level value (for example `noLevelAbove: verbose`), an invalid `regex` pattern, or an unknown key
- **WHEN** the document loads
- **THEN** loading fails as a load error naming the offending clause — never a runtime surprise mid-scenario

#### Scenario: contains passes when a substring occurs in the window

- **GIVEN** a route processor that emits `cache served HIT` during the document
- **WHEN** the document declares `logs: {contains: ["cache served HIT"]}`
- **THEN** the document passes and the logs clause is reported as satisfied

#### Scenario: contains fails when no matching event is captured

- **GIVEN** a document whose run emits no event containing `cache served MISS`
- **WHEN** the document declares `logs: {contains: ["cache served Miss"]}`
- **THEN** the document fails with a diagnostic naming the unsatisfied `contains` entry

#### Scenario: regex matches within the window

- **GIVEN** a document whose run emits an event whose message matches `^processor .*emitted`
- **WHEN** the document declares `logs: {regex: ["^processor .*emitted"]}`
- **THEN** the document passes

#### Scenario: noLevelAbove passes on a clean window

- **GIVEN** a document whose run emits events at `info` and below only
- **WHEN** the document declares `logs: {noLevelAbove: info}`
- **THEN** the document passes

#### Scenario: noLevelAbove fails on a route-processor warning

- **GIVEN** a route processor that emits one `warn`-level event during the document
- **WHEN** the document declares `logs: {noLevelAbove: info}`
- **THEN** the document fails with a diagnostic naming the offending event's level, target, and message

#### Scenario: noLevelAbove counts harness-task events

- **GIVEN** the harness itself emits a `warn`-level event from one of its spawned tasks (for example an arrival-lane overflow) during the document
- **WHEN** the document declares `logs: {noLevelAbove: info}`
- **THEN** the document fails — the level cap spans all targets, not only route processors

#### Scenario: noLevelAbove passes on an empty window

- **GIVEN** a document whose run captures zero events
- **WHEN** the document declares `logs: {noLevelAbove: warn}`
- **THEN** the document passes — a level cap over zero events is vacuously satisfied

#### Scenario: events outside the window never satisfy clauses

- **GIVEN** an event containing `outside marker` emitted before the document's window opens
- **WHEN** the document declares `logs: {contains: ["outside marker"]}`
- **THEN** the document fails — attribution is bounded by the window, never by process lifetime

#### Scenario: foreign subscriber holding the seat is an apparatus error

- **GIVEN** a process where a subscriber other than the tier's capture layer already holds the global tracing seat
- **WHEN** a document declaring a `logs:` block opens its window
- **THEN** the harness fails the run with an apparatus-class error — log assertions never silently pass without capture

#### Scenario: later boots degrade to warn-and-skip and capture persists

- **GIVEN** two documents booted sequentially in one process, the tier capture layer holding the seat
- **WHEN** the second document's composition root attempts its own subscriber install
- **THEN** the install degrades to the existing warn-and-skip path and the second document's events are still captured by the tier layer

#### Scenario: multi-thread runtime events are captured

- **GIVEN** a document run under a multi-threaded tokio runtime (the `camel test` CLI driver) whose events are emitted from worker threads
- **WHEN** the document declares `logs: {contains: [...]}` over an emitted marker
- **THEN** the events are captured — attribution does not depend on the emitting thread

#### Scenario: concurrent windows attribute conservatively

- **GIVEN** two documents with open windows running concurrently in one process
- **WHEN** an event is emitted while both windows are open
- **THEN** the event is attributed to both windows — `contains`/`regex` are at-least semantics and `noLevelAbove` is a conservative superset (a sibling document's warning can fail this document's cap)

## MODIFIED Requirements

### Requirement: Wire-fidelity lane key and mismatch diagnostics

The strict wire-bytes lane key SHALL keep its fidelity contract. Multiple in-flight sends under one lane key SHALL park their responses in a bounded FIFO ordered by wire arrival; receives under that key SHALL consume the oldest parked response first. Apparatus-class errors that name an authored declaration SHALL redact sensitive query values in the named declaration with the same masking the wire-path diagnostics use (ADR-0051), so naming a declaration never becomes credential disclosure.

#### Scenario: same-key sends park in arrival order

- **GIVEN** three sends to one lane key with no intervening receives
- **WHEN** three receives on that key follow
- **THEN** each receive resolves the oldest parked response first, in wire order

#### Scenario: Query-bearing receive matches after wire fidelity

- **Given** a scenario declaring an HTTP partner `from: http://0.0.0.0:PORT/api?flag=a&x=1` and a route sending to a matching target with the same authored query bytes
- **When** the partner `receive` action runs
- **Then** the request is recorded into the matching lane and the receive asserts against it (arrival key equals the declared endpoint's wire `path_and_query`)

#### Scenario: Receive-timeout lists arrived wire paths

- **Given** a partner `receive` that matches no arrival lane
- **When** the receive times out
- **Then** the error message lists every wire path (with query) that arrived in the lane group, so byte divergence is diagnosed without external packet capture

#### Scenario: Count mismatch lists recorded paths

- **Given** a partner `receive` whose count assertion fails (expected N, recorded M ≠ N)
- **When** the mismatch is reported
- **Then** the failure detail prints the recorded request paths, not only the counts

#### Scenario: Empty harness path is an apparatus error

- **Given** a harness HTTP target declaration whose path is empty or absent
- **When** the harness parses the target
- **Then** parsing fails with an apparatus-class error naming the declaration — no silent `/` fallback lane

#### Scenario: Empty-path error redacts authored secret query values

- **Given** a harness HTTP target declaration `http://host?authPassword=x` (query present, path absent) where `authPassword` is a sensitive query key under the active secret-key set
- **When** the harness parses the target and the empty-path apparatus error names the declaration
- **THEN** the named declaration carries the sensitive value masked — the error preserves the query's diagnostic shape without echoing the secret

#### Scenario: Diagnostics redact sensitive query values

- **Given** a receive-timeout or count-mismatch diagnostic whose recorded wire path contains a query value classified sensitive under ADR-0051
- **When** the harness renders the diagnostic message
- **Then** the sensitive value is redacted in the printed path — wire-path visibility never becomes credential disclosure
