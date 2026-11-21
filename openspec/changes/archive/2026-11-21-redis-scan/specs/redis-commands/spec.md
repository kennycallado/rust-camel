## ADDED Requirements

### Requirement: SCAN family cursor contract

The system SHALL support the cursor-based iteration commands `SCAN`,
`SSCAN`, `HSCAN`, and `ZSCAN`, consuming exactly ONE exchange per page.
Each page's reply carries the next iteration position on the
`CamelRedis.Cursor` output header (`0` = iteration complete) and sets
`CamelRedis.ScanComplete=true` on exactly the final page; the header is
absent on every non-final page. A request `CamelRedis.Cursor` of an
absent header, JSON `null`, an empty (or whitespace-only) string, or `0`
starts a fresh iteration (the shared numeric resolver trims surrounding
whitespace, so `" 17 "` is cursor `17`; only an all-whitespace value
counts as empty). A cursor is opaque caller state: the component
never interprets, stores, or derives meaning from a cursor value beyond
echoing it into the next wire command, and no server-side or component
cursor store exists. A present cursor that is not a whole number in
`0..=u64::MAX` SHALL fail before send with a named error naming
`CamelRedis.Cursor` (ADR-0032 fail-closed). `KEYS` semantics are
unchanged by this family.

#### Scenario: absent cursor starts a fresh iteration

- **GIVEN** `command=SCAN` with no `CamelRedis.Cursor` header
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SCAN 0 MATCH * COUNT 100`

#### Scenario: explicit cursor 0 is a fresh iteration

- **GIVEN** `CamelRedis.Cursor=0`
- **WHEN** the producer sends `SSCAN` on `CamelRedis.Key=s`
- **THEN** the wire frame is `SSCAN s 0 MATCH * COUNT 100`

#### Scenario: cursor round-trip is opaque

- **GIVEN** a first page reply whose cursor element is `17`
- **WHEN** the route forwards `CamelRedis.Cursor=17` into the next request
- **THEN** the wire frame is `SCAN 17 MATCH * COUNT 100` and the component
  attaches no meaning to `17` beyond the wire argument

#### Scenario: final page signals completion

- **GIVEN** a page reply whose cursor element is `0`
- **WHEN** the component assembles the reply exchange
- **THEN** `CamelRedis.Cursor` is `0` and `CamelRedis.ScanComplete` is `true`

#### Scenario: non-final page carries only the next cursor

- **GIVEN** a page reply whose cursor element is non-zero
- **WHEN** the component assembles the reply exchange
- **THEN** `CamelRedis.Cursor` carries that value and
  `CamelRedis.ScanComplete` is absent

#### Scenario: malformed cursor is a named pre-send error

- **GIVEN** `CamelRedis.Cursor="abc"` (or a negative or non-integer value)
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send with a named error naming
  `CamelRedis.Cursor`; no command reaches Redis

#### Scenario: empty page with a non-zero cursor still emits one exchange

- **GIVEN** a page reply with an empty element array and cursor `918`
- **WHEN** the component assembles the reply exchange
- **THEN** the body is an empty JSON array, `CamelRedis.Cursor` is `918`,
  `CamelRedis.ScanComplete` is absent, and iteration continues

### Requirement: SCAN family options (MATCH, COUNT, TYPE)

The system SHALL translate `MATCH` from the existing `CamelRedis.Pattern`
header (default `"*"`, applied by all four commands), `COUNT` from the
existing `CamelRedis.Count` header (default `100`, always sent, must be a
positive integer), and `TYPE` from `CamelRedis.Type` (valid on `SCAN`
only; requires a Redis ≥ 6.0 server and surfaces a server-version failure
through the transport error mapping without client-side version
sniffing). `TYPE` present on `SSCAN`, `HSCAN`, or `ZSCAN` SHALL fail
before send with a named error naming `CamelRedis.Type` — never silently
ignored. `MATCH`/`COUNT` filters apply to the page's elements server-side;
the wire frame always carries `MATCH` and `COUNT`.

#### Scenario: MATCH pattern reaches the wire on a collection variant

- **GIVEN** `CamelRedis.Key=s`, `CamelRedis.Cursor=5`,
  `CamelRedis.Pattern="a*"`
- **WHEN** the producer sends `SSCAN`
- **THEN** the wire frame is `SSCAN s 5 MATCH a* COUNT 100`

#### Scenario: COUNT default is the repository pager's page size

- **GIVEN** no `CamelRedis.Count` header
- **WHEN** the producer sends `SCAN`
- **THEN** the wire frame carries `COUNT 100`

#### Scenario: explicit COUNT is honored as a hint

- **GIVEN** `CamelRedis.Count=500`
- **WHEN** the producer sends `SCAN`
- **THEN** the wire frame carries `COUNT 500` (a hint: the server may
  return more or fewer elements per page than 500)

#### Scenario: COUNT zero or negative is a named pre-send error

- **GIVEN** `CamelRedis.Count=0` (or a negative, non-integer, or
  non-numeric value)
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send with a named error naming
  `CamelRedis.Count` and the received value

#### Scenario: TYPE on SCAN reaches the wire

- **GIVEN** `CamelRedis.Type="set"`
- **WHEN** the producer sends `SCAN`
- **THEN** the wire frame ends with `TYPE set`

#### Scenario: TYPE on a collection variant is a named pre-send error

- **GIVEN** `CamelRedis.Key=h`, `CamelRedis.Type="hash"`
- **WHEN** the producer resolves `HSCAN`
- **THEN** it fails before send with a named error naming
  `CamelRedis.Type`; no command reaches Redis

### Requirement: SCAN family page batch shape

The system SHALL emit exactly ONE exchange per page whose body is the
page's elements encoded with the crate's existing element encodings:
`SCAN` → JSON array of key strings (the `KEYS` body shape), `SSCAN` →
JSON array of member strings, `HSCAN` → JSON object mapping field → value
(the `HGETALL` body shape, decoded from the flat field/value pair reply),
`ZSCAN` → JSON array of `{"member": <string>, "score": <number>}` objects
(the `ZRANGE ... WITHSCORES` body shape). The family SHALL NOT be a
`command=multi` batch entry: the typed batch allowlist keeps rejecting it
with a named error (cursor round-trips do not compose inside one
pipeline).

#### Scenario: SCAN page body is a key array

- **GIVEN** a `SCAN` page reply with elements `["a","b"]`
- **WHEN** the component assembles the reply exchange
- **THEN** the body is the JSON array `["a","b"]` in one exchange

#### Scenario: SSCAN page body is a member array

- **GIVEN** an `SSCAN` page reply with elements `["x","y"]`
- **WHEN** the component assembles the reply exchange
- **THEN** the body is the JSON array `["x","y"]` in one exchange

#### Scenario: HSCAN page body is a field/value map

- **GIVEN** an `HSCAN` page reply with flat pairs `[f1,v1,f2,v2]`
- **WHEN** the component assembles the reply exchange
- **THEN** the body is the JSON object `{"f1":"v1","f2":"v2"}`

#### Scenario: ZSCAN page body is scored-member pairs

- **GIVEN** a `ZSCAN` page reply with flat pairs `[m1,1.5,m2,2]`
- **WHEN** the component assembles the reply exchange
- **THEN** the body is `[{"member":"m1","score":1.5},{"member":"m2","score":2.0}]`

#### Scenario: SCAN family is rejected as a batch entry

- **GIVEN** a `command=multi` request whose `CamelRedis.Batch` array
  contains a `SCAN` (or `SSCAN`/`HSCAN`/`ZSCAN`) entry
- **WHEN** the producer validates the batch plan
- **THEN** it fails with a named error identifying `CamelRedis.Batch`,
  the entry index, and the entry command; no
  pipeline is sent

### Requirement: SCAN family iteration guarantees

The system SHALL provide full-coverage, not uniqueness: while a route
keeps `MATCH`, `COUNT`, and `TYPE` fixed across an iteration, Redis
guarantees every element present for the whole iteration is returned at
least once and may return elements on multiple pages (the component
forwards repeats unchanged — deduplication is the route's decision; the
component is stateless per exchange and enforces no cross-page option
consistency, so an iteration that varies its options holds no
guarantee). `COUNT` is a server-side hint: a page may hold more or fewer
elements than the hint. The four
commands SHALL classify as retry-safe reads in `is_idempotent_command`:
a transport retry re-issues the same cursor page (converging read; the
repository pager precedent). `KEYS` SHALL remain available with unchanged
wire semantics; documentation MAY note preferring SCAN for large
keyspaces, but no behaviour, classification, or reply shape of `KEYS`
changes. (The `KEYS` scenario below is the minimal KEYS pin, not a full
KEYS specification — it exists to bound this family's blast radius.)

#### Scenario: an element may repeat across pages

- **GIVEN** a keyspace where Redis's cursor walk returns key `a` on two
  different pages
- **WHEN** the route iterates both pages
- **THEN** each page's exchange body contains `a` exactly as the server
  returned it (no component-side dedup)

#### Scenario: retry-safe classification

- **GIVEN** `is_idempotent_command` consulted for `SCAN`, `SSCAN`,
  `HSCAN`, `ZSCAN`
- **WHEN** a transport error follows an unconfirmed page fetch
- **THEN** the command classifies retry-safe and the same cursor page is
  re-issued once

#### Scenario: KEYS remains unchanged

- **GIVEN** `command=KEYS` with `CamelRedis.Pattern="a*"`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `KEYS a*` and the reply body is the full JSON
  key array, exactly as before this family existed
