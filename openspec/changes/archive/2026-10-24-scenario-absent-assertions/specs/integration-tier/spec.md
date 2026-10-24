## ADDED Requirements

### Requirement: Absence assertion

The scenario tier SHALL provide an explicit absence assertion: an `{absent: null}` matcher key on a `lastReceived` validate target that carries a `select` field. The `select` field SHALL project the last received message through the extract selector grammar (`headers.<name>` ASCII-case-insensitively, `body.<path>` dotted). The absent matcher SHALL pass exactly when the projection resolves to nothing, and SHALL fail otherwise with a mismatch naming the selector and the fact that a value was observed — the kind only (for example `present (string)`), never the value's content, because an absence diagnostic is a leak diagnostic and scenario output is shared. The absence claim is path-scoped: it proves the selected path resolves to nothing, not that the whole message part is absent. The `receive.extract` behavior SHALL NOT change: an extract whose source is absent keeps failing the scenario.

The loader SHALL reject at load time, each naming the action index: a `select` whose head is not `headers` or `body`, a `select` without a non-empty sub-path, a `select` paired with any expectation other than `absent`, an `absent` matcher on any target other than `lastReceived`, an `absent` matcher on a `lastReceived` target without `select`, an `absent` matcher with a non-null payload, and an `absent` matcher in every other grammar position that reuses the expectation parser — `send.expectReply`, partner `requests` body expectations, and sql/surreal/redis row cells. A misspelled selector head SHALL therefore fail loudly at load, never as a vacuous pass. A misspelled header name or body path under a valid head is indistinguishable from absence and SHALL pass the absent matcher; the documentation SHALL state this limit and recommend a positive control.

#### Scenario: absent header is assertable

- **GIVEN** a scenario whose route responds without a header named `X-Internal-Token`
- **WHEN** the document validates `lastReceived` on the receive endpoint with `select: headers.X-Internal-Token` and `{absent: null}`
- **WHEN** the scenario runs
- **THEN** the validate passes

#### Scenario: present header fails the absent matcher without disclosing the value

- **GIVEN** a scenario whose route responds with header `X-Internal-Token: s3cr3t`
- **WHEN** the document validates `lastReceived` with `select: headers.X-Internal-Token` and `{absent: null}`
- **THEN** the validate fails with a mismatch naming the selector and the observed kind, and the failure text does not contain `s3cr3t`

#### Scenario: header carrying a null value fails the absent matcher

- **GIVEN** a message whose header map contains `X-Flag` mapped to null
- **WHEN** the absent matcher projects `headers.X-Flag`
- **THEN** the matcher fails, and an extract of `headers.X-Flag` into a variable followed by a `variable` validate with `equals: null` passes

#### Scenario: body leaf carrying null fails the absent matcher

- **GIVEN** a message whose body is `{"receiptId": null}`
- **WHEN** the absent matcher projects `body.receiptId`
- **THEN** the matcher fails (the path resolves to a null value; it is present-null, not absent)

#### Scenario: misspelled selector head is a load error

- **GIVEN** a validate declaring `select: header.X-Internal-Token` (unknown head)
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the action index and the selector

#### Scenario: bare or scalar select head is a load error

- **GIVEN** a validate declaring `select: body`, `select: headers`, or `select: status`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 explaining that the head always resolves and cannot prove absence

#### Scenario: absent outside lastReceived-with-select is a load error

- **GIVEN** a validate declaring `{absent: null}` on a `variable`, `partner`, `sql`, `surreal`, or `redis` target, or on a `lastReceived` target without `select`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the action index and the restriction

#### Scenario: select paired with a non-absent expectation is a load error

- **GIVEN** a validate declaring `select: headers.X` with `expectation: {equals: "v"}`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the restriction that `select` exists solely for absence assertions

#### Scenario: absent response body path is assertable

- **GIVEN** a scenario whose route responds with a null (empty) body
- **WHEN** the document validates `lastReceived` with `select: body.receiptId` and `{absent: null}`
- **THEN** the validate passes

#### Scenario: absent matcher on an endpoint with no received message fails

- **GIVEN** a validate declaring `select: headers.X` with `{absent: null}` on an endpoint that has not received any message
- **WHEN** the action runs
- **THEN** the validate fails with the existing "no message has been received" error; absence never passes on a missing subject

#### Scenario: absent in expectReply, partner body, or row cell is a load error

- **GIVEN** a document declaring `{absent: null}` as a `send.expectReply` expectation, inside a partner `requests` body expectation, or as a sql/surreal/redis row cell
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the action index and the restriction

#### Scenario: literal absent string stays a literal equals

- **GIVEN** a validate declaring the bare string expectation `absent`
- **WHEN** the document loads and the subject equals the literal string `absent`
- **THEN** the expectation behaves as `equals "absent"`, not as the absence matcher

#### Scenario: extract keeps failing on an absent source

- **GIVEN** a receive action extracting `headers.X-Internal-Token` when the header is absent
- **WHEN** the action runs
- **THEN** the scenario fails with the unchanged "resolved to nothing" failure naming the selector and variable
