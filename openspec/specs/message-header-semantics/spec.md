# message-header-semantics Specification

## Purpose
Header-key semantics for `Message.headers`. The map stays a plain
`HashMap<String, Value>`; this capability fixes the contract of the helper
methods so route steps and transport components agree on case handling
without a breaking map-type migration (API-003, bd rc-qtpq1).
## Requirements
### Requirement: Header lookup modes

The message API SHALL expose two distinct lookup modes. `header(key)` SHALL
match the stored key exactly. `header_ic(key)` SHALL match ASCII
case-insensitively with a deterministic winner: an exact-case entry wins; with
no exact entry, the entry whose key sorts first by byte order wins. `header_ic`
MUST NOT depend on `HashMap` iteration order.

#### Scenario: exact lookup stays case-sensitive

- **GIVEN** a message with header `Content-Type` set
- **WHEN** `header("content-type")` is called
- **THEN** it returns `None`

#### Scenario: case-insensitive lookup finds any spelling

- **GIVEN** a message with header `Content-Type` set
- **WHEN** `header_ic("content-type")` and `header_ic("CONTENT-TYPE")` are called
- **THEN** both return the same value

#### Scenario: duplicate spellings resolve deterministically

- **GIVEN** a message whose raw `headers` map was populated directly with
  keys `content-type` and `Content-Type` holding different values
- **WHEN** `header_ic("Content-Type")` is called
- **THEN** the exact-case `Content-Type` value is returned
- **AND** `header_ic("CONTENT-TYPE")` returns the value whose stored key sorts
  first, on every call and across process runs

### Requirement: Case-insensitive header mutation

`set_header(key, value)` SHALL remove every existing entry whose key equals
`key` ASCII case-insensitively, then insert the caller's spelling.
`set_header_normalized(key, value)` SHALL additionally lowercase the inserted
key. `remove_header(key)` SHALL remove every entry whose key equals `key`
ASCII case-insensitively; it SHALL return the exact-case entry's value when
that entry existed, and `None` otherwise — including when case variants were
removed but none matched the requested spelling exactly. After any of these
operations, at most one case-insensitive equivalent of `key` SHALL remain, so
iteration and serialization are deterministic.

Header-presence checks used by if-absent route steps SHALL be ASCII
case-insensitive: an existing entry under any spelling of `key` suppresses the
write, and the value expression MUST NOT be evaluated in that case.

#### Scenario: lowercase set replaces title-case header

- **GIVEN** a message with `Content-Type: application/xml`
- **WHEN** `set_header("content-type", "application/json")` is called
- **THEN** exactly one content-type entry remains and its spelling is
  `content-type`
- **AND** `header_ic("Content-Type")` returns `application/json`

#### Scenario: mixed-case set replaces existing entry

- **GIVEN** a message with `content-type` set
- **WHEN** `set_header("Content-Type", "text/plain")` is called
- **THEN** the map holds exactly one content-type entry and it carries the
  `Content-Type` spelling

#### Scenario: remove is case-insensitive and complete

- **GIVEN** a message with `X-Request-Id` set
- **WHEN** `remove_header("x-request-id")` is called
- **THEN** `header("X-Request-Id")` and `header_ic("x-request-id")` both
  return `None`

#### Scenario: removing an absent header is a no-op

- **GIVEN** a message with no `X-Missing` entry
- **WHEN** `remove_header("x-missing")` is called
- **THEN** it returns `None` and every other header is unchanged

#### Scenario: removal returns nothing without an exact-case match

- **GIVEN** a message whose raw `headers` map holds `X-Dup: 1`
- **WHEN** `remove_header("x-dup")` is called
- **THEN** the `X-Dup` entry is removed and the call returns `None`

#### Scenario: if-absent presence check is case-insensitive

- **GIVEN** a message with `X-Flag` set and an if-absent `set_header` step for
  `x-flag` whose value expression would fail or increment a counter
- **WHEN** the step runs
- **THEN** the existing `X-Flag` is preserved and the expression is never
  evaluated

### Requirement: Spelling retention and raw-map escape hatch

`set_header` SHALL store the caller's spelling verbatim; iteration and JSON
serialization SHALL expose that spelling. Direct writes through the public
`headers` field SHALL remain possible but bypass normalization, and are
documented as an escape hatch whose readers MUST use `header_ic` when case
fold is required.

#### Scenario: iteration and serialization preserve spelling

- **GIVEN** a message where `set_header("X-Request-Id", "abc")` was called
- **WHEN** the headers are iterated and serialized with `serde_json`
- **THEN** the key `X-Request-Id` appears with no case folding

#### Scenario: raw map write is visible to case-insensitive lookup

- **GIVEN** a message whose raw `headers` map was written directly with
  `X-Raw: 1`
- **WHEN** `header_ic("x-raw")` is called
- **THEN** it returns `1`, while `header("x-raw")` returns `None`

