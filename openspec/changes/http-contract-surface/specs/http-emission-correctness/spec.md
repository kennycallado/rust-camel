# http-emission-correctness delta

## MODIFIED Requirements

### Requirement: Raw-preserving outbound query serialization

When the producer endpoint URI carries an authored raw query, `resolve_url` MUST emit those bytes byte-for-byte minus explicitly consumed option keys. Option-key consumption MUST have a single metadata-driven owner (ADR-0041 `uri_options()`): it is the sole authority for OUTBOUND option filtering — the raw filter consults it at runtime — while `from_uri`'s manual typed parsing (`skip_impl`) stays direct and unchanged; no duplicated handwritten key lists. Authored non-option pairs are carried ONLY by `raw_query` (no structured re-collection). "Byte-for-byte" is bounded to wire-legal authored bytes: a raw byte forbidden in a query component MUST produce a resolve error, never silent re-encoding. Query-source precedence MUST hold: `bridgeEndpoint` bridging > `CamelHttpUri` override > `CamelHttpQuery` header (composed per ADR-0071: pairs ride verbatim, append for keys absent from the higher-precedence source, and lose key collisions) > endpoint `raw_query` base > programmatic `query_params` (the only structured path — minimal RFC-3986 encoding, declaration order, `%20` never `+` for spaces). `RAW(...)` wrappers MUST NOT be unwrapped or re-encoded by the serializer. URL resolution failures MUST propagate as errors — malformed operator input never panics the producer task.

#### Scenario: Authored order and encoding survive to the wire

- **Given** a producer endpoint URI `http://h/p?a=1&b=x,y&c=t:1&connectTimeout=5s` where `connectTimeout` is a consumed camel-http option
- **When** the producer resolves the outbound URL
- **Then** the outbound query is `a=1&b=x,y&c=t:1` — authored order, authored separators, no form-encoding of `,` `:` or space, consumed option removed

#### Scenario: Query that is all consumed options disappears

- **Given** a producer endpoint URI `http://h/p?connectTimeout=5s` whose only query pair is a consumed option
- **When** the producer resolves the outbound URL
- **Then** the outbound URL is `http://h/p` with no query component — no dangling `?`

#### Scenario: Authored empty-query marker is preserved

- **Given** a producer endpoint URI `http://h/p?` with a bare empty-query marker
- **When** the producer resolves the outbound URL
- **Then** the outbound URL preserves the empty query distinctly — an authored `?` is not conflated with an all-consumed query

#### Scenario: Percent-encoded option keys are consumed by decoded match

- **Given** a producer endpoint URI whose raw query authors the option as `connect%54imeout=5s`
- **When** the producer resolves the outbound URL
- **Then** the pair is consumed — the raw filter matches the decoded key, not the encoded bytes

#### Scenario: CamelHttpQuery composes with the endpoint query

- **Given** a producer exchange carrying a `CamelHttpQuery` header and an endpoint with raw query `x=1`
- **When** the producer resolves the outbound URL
- **Then** the header pairs ride verbatim (not structured-serialized), append after the endpoint pairs for absent keys, and lose key collisions to the higher-precedence query source (ADR-0071)

#### Scenario: Programmatic fallback stays deterministic

- **Given** query parameters attached programmatically via endpoint `query_params` config with no raw source, declared as `[("z", "1"), ("a", "2")]` in that order
- **When** the producer resolves the outbound URL
- **Then** the query is `z=1&a=2` — declaration order (not lexical), minimal RFC-3986 percent-encoding, `%20` — never `+` — for spaces

#### Scenario: Wire-forbidden raw byte errors instead of re-encoding

- **Given** an authored raw query containing a byte forbidden in a query component (e.g. a literal space or `#`)
- **When** the producer resolves the outbound URL
- **Then** resolution fails with an error naming the offending byte — the serializer never silently re-encodes authored bytes

#### Scenario: RAW wrapper neither unwrapped nor re-encoded

- **Given** a producer endpoint URI whose raw query contains a `RAW(...)`-wrapped value
- **When** the producer resolves the outbound URL
- **Then** the outbound query carries the wrapper bytes exactly as authored — the serializer neither strips the wrapper nor double-encodes it (the rc-g4isv regression)

#### Scenario: Malformed base URL errors instead of panicking

- **Given** an operator-supplied base URL that `url::Url::parse` rejects
- **When** the producer resolves the outbound URL
- **Then** the failure propagates as a producer error result and the producer task keeps running — no `.expect`/panic path (the rc-ph7z2 regression)
