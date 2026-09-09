## MODIFIED Requirements

### Requirement: Wire-fidelity lane key and mismatch diagnostics

The strict wire-bytes lane key SHALL keep its fidelity contract. Multiple in-flight sends under one lane key SHALL park their responses in a bounded FIFO ordered by wire arrival; receives under that key SHALL consume the oldest parked response first. The server-role receive SHALL derive its arrival lane from the INTERPOLATED reference's own path-and-query, while the registered key remains the adapter-lookup key: a dynamic-reference receive (`from: http://${MOCK}/billing`) drains the lane of its own path, not the registered key's path. A dynamic reference carrying no path (interpolated authority only) SHALL fail as an apparatus-class error naming the declaration. Under dynamic references, standalone client-role roundtrip receives (parked responses taken by a later `receive`, not `expectReply`) match oldest-first by wire arrival and are PATH-BLIND — the parked-response parking stays keyed on the registered partner key (path-aware keying deferred, bd rc-cr5yf; the recommended per-path send assertion is `expectReply` on the send action or a path-filtered partner validate). Apparatus-class errors that name an authored declaration SHALL redact sensitive query values in the named declaration with the same masking the wire-path diagnostics use (ADR-0051), so naming a declaration never becomes credential disclosure.

#### Scenario: same-key sends park in arrival order

- **GIVEN** three sends to one lane key with no intervening receives
- **WHEN** three receives on that key follow
- **THEN** each receive resolves the oldest parked response first, in wire order

#### Scenario: Query-bearing receive matches after wire fidelity

- **Given** a scenario declaring an HTTP partner `from: http://0.0.0.0:PORT/api?flag=a&x=1` and a route sending to a matching target with the same authored query bytes
- **When** the partner `receive` action runs
- **Then** the request is recorded into the matching lane and the receive asserts against it (arrival key equals the declared endpoint's wire `path_and_query`)
- **And** the same fidelity holds for a dynamic reference: a receive `from: http://${MOCK}/api?x=1` matches the wire lane `/api?x=1` exactly — path AND query — and a divergent query (`?x=2`) times out listing the arrived wire paths

#### Scenario: Dynamic-reference receive drains its own path lane

- **Given** one declared harness endpoint with `bindVar: MOCK` whose listener serves two dial paths `/orders` and `/billing`, both lanes holding arrivals
- **When** a scenario receive uses the dynamic reference `from: http://${MOCK}/billing`
- **THEN** the receive drains the `/billing` lane and asserts the billing arrival — never the declared endpoint's `/orders` lane

#### Scenario: Declared-key receive lane selection unchanged

- **Given** a receive whose declared endpoint string names the registered key (placeholders resolving the same URI)
- **When** the receive runs
- **THEN** it drains the declared path's lane exactly as before — the interpolated-path derivation is a no-op for declared-key receives

#### Scenario: Receive-timeout lists arrived wire paths

- **Given** a partner `receive` that matches no arrival lane
- **When** the receive times out
- **Then** the error message lists every wire path (with query) that arrived in the lane group, so byte divergence is diagnosed without external packet capture

#### Scenario: Count mismatch lists recorded paths

- **Given** a partner `receive` whose count assertion fails (expected N, recorded M ≠ N)
- **When** the mismatch is reported
- **Then** the failure detail prints the recorded request paths, not only the counts

#### Scenario: Empty harness path is an apparatus error

- **Given** a harness HTTP target declaration whose path is empty or absent — including a dynamic reference interpolating to a bare authority (`from: http://${MOCK}` with no path)
- **When** the harness parses the target
- **Then** parsing fails with an apparatus-class error naming the declaration — no silent `/` fallback lane and no silent fallthrough to the registered key's lane

#### Scenario: Empty-path error redacts authored secret query values

- **Given** a harness HTTP target declaration `http://host?authPassword=x` (query present, path absent) where `authPassword` is a sensitive query key under the active secret-key set
- **When** the harness parses the target and the empty-path apparatus error names the declaration
- **THEN** the named declaration carries the sensitive value masked — the error preserves the query's diagnostic shape without echoing the secret

#### Scenario: Diagnostics redact sensitive query values

- **Given** a receive-timeout or count-mismatch diagnostic whose recorded wire path contains a query value classified sensitive under ADR-0051
- **When** the harness renders the diagnostic message
- **Then** the sensitive value is redacted in the printed path — wire-path visibility never becomes credential disclosure

#### Scenario: Standalone roundtrip receives under dynamic references are path-blind

- **Given** two standalone sends (no `expectReply`) dialing `/a` and `/b` of one partner under dynamic references (`to: http://${MOCK}/a`, `to: http://${MOCK}/b`), both roundtrips parked under the registered key
- **WHEN** two standalone roundtrip receives follow
- **THEN** each resolves the oldest parked response first by wire arrival regardless of path — the client-role parking is path-blind by contract (path-aware keying tracked as bd rc-cr5yf)
