# http-url-resolution delta

## ADDED Requirements

### Requirement: Outbound query composition

When the exchange carries a `CamelHttpQuery` header, the producer SHALL
compose the outbound query from the higher-precedence query source followed
by the header pairs whose keys are absent from that set. The
higher-precedence source SHALL win any key collision. In the base arm (no
`CamelHttpUri` header) the higher-precedence source is the endpoint query
(`raw_query` + programmatic `query_params`, consumed-option-filtered); in
the override arm it is the override URI's own query — the override URI
remains untrusted exchange data under ADR-0032, and the endpoint base query
does not ride an override. `CamelHttpPath` SHALL apply to the path
component before query composition in both arms. Header pair bytes SHALL be
carried verbatim; a raw byte forbidden in a query component inside a header
value SHALL produce a resolve error naming the offending byte, never a
re-encoding. When `CamelHttpQuery` is present but empty, the
higher-precedence query source SHALL be emitted unchanged with no
additional `?` marker.

#### Scenario: header composes with endpoint query

- **GIVEN** an endpoint `to: http://upstream/api?apiKey=secret&lang=en` and
  an exchange carrying `CamelHttpQuery=lang=es&page=2`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound query carries `apiKey=secret` and `lang=en` from the
  endpoint and `page=2` from the header — `lang` is NOT replaced by the
  header value

#### Scenario: header alone still rides

- **GIVEN** an endpoint `to: http://upstream/api` with no endpoint query and
  an exchange carrying `CamelHttpQuery=page=2`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://upstream/api?page=2`

#### Scenario: empty reflected query leaves endpoint query intact

- **GIVEN** an endpoint `to: http://upstream/api?apiKey=secret` and an
  exchange carrying an empty `CamelHttpQuery` (as the consumer installs on
  requests without a query string)
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://upstream/api?apiKey=secret` — no
  second `?` marker, no dropped endpoint pair

#### Scenario: forbidden byte in header query errors

- **GIVEN** an exchange carrying `CamelHttpQuery=q=ab<cd` where `<` is
  forbidden in a query component
- **WHEN** the producer resolves the outbound URL
- **THEN** resolution fails with an error naming the forbidden byte, and no
  re-encoded URL is sent

#### Scenario: bridgeEndpoint ignores URL headers

- **GIVEN** an endpoint with `bridgeEndpoint=true` and an exchange carrying
  `CamelHttpUri`, `CamelHttpPath`, and `CamelHttpQuery`
- **WHEN** the producer resolves the outbound URL
- **THEN** all three headers are ignored and the endpoint base URL plus its
  own query is sent, exactly as before this change

### Requirement: Override URI query merge

When a `CamelHttpUri` override carries its own query and the exchange also
carries `CamelHttpQuery`, the producer SHALL merge them at pair level:
override-URI pairs first (winning collisions), header pairs appending for
absent keys — instead of concatenating a second `?` marker.

#### Scenario: override URI with query plus header query

- **GIVEN** an exchange carrying `CamelHttpUri=http://host/api?a=1` and
  `CamelHttpQuery=a=2&b=3`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://host/api?a=1&b=3` — one `?` marker,
  `a` from the override URI, `b` from the header

#### Scenario: path applies before query composition

- **GIVEN** an exchange carrying `CamelHttpUri=http://host/api?a=1`,
  `CamelHttpPath=/extra`, and `CamelHttpQuery=b=2`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://host/api/extra?a=1&b=2`

### Requirement: Default inbound reflection

The producer SHALL consume `CamelHttpPath` and `CamelHttpQuery` exchange
headers by default (inbound reflection), including headers set
automatically by the HTTP consumer from the inbound wire request; the
`CamelHttpQuery` header composes with the endpoint query under the
composition rule above.

#### Scenario: plain proxy keeps working

- **GIVEN** a route `from: http://0.0.0.0:8080/in` to
  `to: http://upstream/api?apiKey=secret` and an inbound request
  `GET /in/extra?page=2`
- **WHEN** the consumer sets `CamelHttpPath=/in/extra` and
  `CamelHttpQuery=page=2` from the wire and the producer resolves
- **THEN** the outbound URL is
  `http://upstream/api/in/extra?apiKey=secret&page=2` — the operator pair is
  not replaced

### Requirement: CamelHttpUri host fence

The endpoint URI SHALL accept an `allowedUriHosts` option: comma-separated
exact host entries, each optionally `host:port`. Empty comma segments are
dropped; a declared option that yields zero valid entries SHALL fail
endpoint creation, as SHALL any other malformed entry. DNS names SHALL
compare case-insensitively; IPv6 literals compare in bracketed canonical
form; a host-only entry permits any port; a `host:port` entry matches only
the override's effective port. When the option is declared and a
`CamelHttpUri` override resolves to a host not matching any entry, or the
override URI fails to yield a host, the producer SHALL fail resolution with
an error that renders the rejected URL only through the diagnostics
redaction path. When the option is absent, override behavior is unchanged.
The option SHALL be consumed: it never appears in the outbound query.

#### Scenario: armed fence rejects unknown host with redacted error

- **GIVEN** an endpoint declaring `allowedUriHosts=api.internal:8443,cdn.example.com`
  and an exchange carrying
  `CamelHttpUri=http://user:pass@evil.example.com/x?token=s3cret`
- **WHEN** the producer resolves the outbound URL
- **THEN** resolution fails, and the error text contains neither `pass` nor
  `s3cret` — the rejected URL is rendered only through the diagnostics
  redaction path

#### Scenario: armed fence allows listed host

- **GIVEN** the same endpoint and an exchange carrying
  `CamelHttpUri=http://cdn.example.com/x`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://cdn.example.com/x`

#### Scenario: host-only entry permits any port

- **GIVEN** an endpoint declaring `allowedUriHosts=cdn.example.com` and an
  exchange carrying `CamelHttpUri=http://cdn.example.com:9443/x`
- **WHEN** the producer resolves the outbound URL
- **THEN** the override is honored

#### Scenario: unarmed endpoint unchanged

- **GIVEN** an endpoint without `allowedUriHosts` and an exchange carrying
  `CamelHttpUri=http://any.example.com/path`
- **WHEN** the producer resolves the outbound URL
- **THEN** the override is honored exactly as before the fence existed
