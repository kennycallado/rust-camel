## MODIFIED Requirements

### Requirement: Scenario boot shares the camel run composition root

The scenario tier SHALL boot through the same composition root as `camel run`: sealed config load and root-anchored route resolution follow the boot root, which is the nearest ancestor directory (including the document's own) containing a `Camel.toml`. Relative `routeFiles` entries SHALL resolve against the scenario document's own directory. A document with no `Camel.toml` ancestor SHALL fail named with exit 2 before boot.

#### Scenario: boot root is the nearest Camel.toml ancestor

- **GIVEN** a scenario document at `tests/integration/layers/map/static.test.yaml` and a `Camel.toml` at `tests/integration/`
- **WHEN** the tier boots the document
- **THEN** the sealed config loads from `tests/integration/Camel.toml` and the boot succeeds

#### Scenario: no Camel.toml ancestor fails named

- **GIVEN** a scenario document with no `Camel.toml` in any ancestor directory
- **WHEN** the tier loads it
- **THEN** the run fails with exit 2 naming the missing project root

#### Scenario: relative routeFiles stay document-anchored

- **GIVEN** a nested scenario document declaring `routeFiles: [routes.yaml]` with `routes.yaml` colocated next to the document
- **WHEN** the tier boots from the ancestor root
- **THEN** `routeFiles` resolve against the document directory and the boot succeeds

#### Scenario: routeFilesFromRoot follows the resolved root

- **GIVEN** a nested scenario document declaring `routeFilesFromRoot: [routes/x.yaml]` present under the ancestor root's route space
- **WHEN** the tier boots from the ancestor root
- **THEN** the root-anchored paths resolve against the resolved boot root, not the document directory

### Requirement: Ordered scenario actions

Scenario actions SHALL execute in document order. A receive SHALL carry a mandatory deadline. A document MAY declare a top-level `sendDeadline` bounding every send action; when absent, the thirty-second default applies. An unparseable `sendDeadline` SHALL fail at load time with exit 2.

#### Scenario: document send deadline bounds every send

- **GIVEN** a document declaring `sendDeadline: 500ms` and a send action whose connection hangs
- **WHEN** the action runs
- **THEN** the send fails apparatus-class at the document deadline, naming the bound

#### Scenario: absent send deadline keeps the thirty-second default

- **GIVEN** a document without `sendDeadline` and a send that completes
- **WHEN** the actions run
- **THEN** send behavior is unchanged from the fixed default

#### Scenario: invalid send deadline is a load error

- **GIVEN** a document declaring `sendDeadline: soon`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming `sendDeadline`

### Requirement: Failure taxonomy

Scenario runs SHALL keep the epistemic exit split: 0 = pass, 1 = verdict-class (system under test failed), 2 = apparatus-class (harness failure) and load/misuse errors. Harness-side queue overflow conditions SHALL surface as apparatus-class failures carrying their own name, never as verdict-class receive timeouts.

#### Scenario: arrival lane overflow is an apparatus failure

- **GIVEN** a partner path whose arrival lane dropped arrivals after filling
- **WHEN** a subsequent receive on that path times out
- **THEN** the run fails with exit 2 as an apparatus arrival-lane-overflow naming the dropped count, not as a verdict receive-timeout

#### Scenario: client lane FIFO overflow is an apparatus failure

- **GIVEN** more same-key in-flight sends than the client lane FIFO bound
- **WHEN** the overflowing send launches
- **THEN** the send fails apparatus-class naming the lane key and the bound

### Requirement: Wire-fidelity lane key and mismatch diagnostics

The strict wire-bytes lane key SHALL keep its fidelity contract. Multiple in-flight sends under one lane key SHALL park their responses in a bounded FIFO ordered by wire arrival; receives under that key SHALL consume the oldest parked response first.

#### Scenario: same-key sends park in arrival order

- **GIVEN** three sends to one lane key with no intervening receives
- **WHEN** three receives on that key follow
- **THEN** each receive resolves the oldest parked response first, in wire order

## ADDED Requirements

### Requirement: Load-time document validation

The scenario document loader SHALL reject statically-detectable document defects at load time with exit 2, before partner binding or boot, following the reserved-provisioning gate pattern.

#### Scenario: inline routes rejected at load

- **GIVEN** a scenario document whose route source is inline `routes:`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 directing the author to `routeFiles`, before any partner binds

#### Scenario: harness provisioning without bound authority rejected at load

- **GIVEN** a `provisioning: harness` entry with a `direct:` or `fake:` ref that also declares a `bindVar`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the entry and the missing bound authority

#### Scenario: expectReply on a partner send is a load error

- **GIVEN** a send to an `http` partner ref or a `fake:` ref declaring `expectReply`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the action index and the `expectReply` field

### Requirement: Direct reply assertion

The scenario tier SHALL assert the synchronous reply of a `direct:` send through an `expectReply` field parsed into the shared matcher algebra (`camel-matchers::Expectation`, the same verbs as `validate`). `expectReply` on any non-`direct:` target SHALL fail at load time (the fake adapter records sends and produces no synchronous reply). An `expectReply` mismatch SHALL fail verdict-class naming the expectation and the actual reply body.

#### Scenario: expectReply matches the direct reply body

- **GIVEN** a request/reply route behind `direct:reply` and a send declaring `expectReply: {contains: "ack"}`
- **WHEN** the send completes
- **THEN** the reply body satisfies the expectation and the action passes

#### Scenario: expectReply mismatch is a verdict failure

- **GIVEN** a send declaring `expectReply` whose reply body violates the expectation
- **WHEN** the send completes
- **THEN** the run fails verdict-class naming the expectation and the actual reply body

### Requirement: Inbound listener provisioning

The scenario harness SHALL provision inbound listeners on an ephemeral port and expose the bound address to route URIs through variable interpolation, reusing staged listener consumption. Documents pinning literal ports SHALL keep working unchanged.

#### Scenario: inbound listener binds port zero

- **GIVEN** a document declaring an inbound listener with a `bindVar`
- **WHEN** the harness provisions it
- **THEN** the listener binds `127.0.0.1:0` and the variable carries the bound `http://host:port` address

#### Scenario: route consumer serves on the staged listener

- **GIVEN** a route URI interpolating the inbound `bindVar`
- **WHEN** the document boots and a request targets the bound address
- **THEN** the route consumer serves from the staged listener without a second bind

#### Scenario: fixed-port inbound documents stay valid

- **GIVEN** an inbound document pinning a literal port in the route URI (the pre-change shape)
- **WHEN** it loads and boots
- **THEN** the behavior is unchanged
