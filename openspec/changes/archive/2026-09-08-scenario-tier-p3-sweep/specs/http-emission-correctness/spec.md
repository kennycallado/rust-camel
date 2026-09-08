## MODIFIED Requirements

### Requirement: bridgeEndpoint URL bridging

When `bridgeEndpoint` is `true`, the HTTP producer `resolve_url` SHALL return the endpoint base URL plus configured query params and SHALL ignore exchange `CamelHttpPath` and `CamelHttpQuery`. The bridged emission SHALL assemble the URL from the base URL's authored bytes verbatim — no dot-segment collapse, no default-port stripping, no scheme or host lowercasing (an intentional alignment with the authored-byte canon; it is a visible change for downstreams that relied on WHATWG normalization from the bridge arm). When `bridgeEndpoint` is `false` (the default), the existing path/query merge behaviour is unchanged. For the same declared base URL and resolved query, emission SHALL be byte-identical whether bridged or not — no arm normalizes.

#### Scenario: bridgeEndpoint true ignores exchange path

- **GIVEN** an exchange carrying `CamelHttpPath=/foo` and a producer endpoint `http://x?bridgeEndpoint=true`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://x` with no `/foo` appended

#### Scenario: bridgeEndpoint false keeps merge behaviour

- **GIVEN** an exchange carrying `CamelHttpPath=/foo` and a producer endpoint `http://x` (bridgeEndpoint defaults to false)
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL appends `/foo`, preserving the existing behaviour

#### Scenario: configured query params still applied under bridging

- **GIVEN** a producer endpoint `http://x?bridgeEndpoint=true&token=secret`
- **WHEN** the producer resolves the outbound URL with an exchange carrying `CamelHttpQuery=dropme=1`
- **THEN** the outbound URL carries `token=secret` and does NOT carry `dropme=1`

#### Scenario: Bridge arm preserves dot-segment paths

- **GIVEN** a bridged producer endpoint whose base URL path contains dot segments, `http://h/a/../b`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound path is `/a/../b` byte-for-byte — dot segments are not collapsed

#### Scenario: Bridge arm preserves explicit default port

- **GIVEN** a bridged producer endpoint `http://h:80/p`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL keeps `:80` — the explicit default port is not stripped

#### Scenario: Bridge arm preserves scheme and host case

- **GIVEN** a bridged producer endpoint `HTTP://ExAMPLE.COM/p`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL keeps `HTTP://ExAMPLE.COM/p` as authored — no lowercasing of scheme or host

#### Scenario: Bridge arm with no query emits base verbatim

- **GIVEN** a bridged producer endpoint `http://h/p` with no resolved query (no raw query, no query_params)
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://h/p` exactly as authored, with no dangling `?`

#### Scenario: Bridge and non-bridge arms emit byte-identical URLs

- **GIVEN** the same declared base URL and the same resolved query composition
- **WHEN** one exchange resolves through the bridged arm and another through the non-bridge path (for example the `CamelHttpQuery` composition arm)
- **THEN** both emissions are byte-identical — normalization divergence between arms cannot occur
