## ADDED Requirements

### Requirement: JWKS forced-refresh coalescing and cooldown

The camel-auth JWKS provider SHALL bound unknown-kid-triggered forced
refreshes such that at most one outbound JWKS request STARTS per provider per
forced-refresh interval (default 5 seconds), regardless of the number of
concurrent validation requests or the cardinality of unknown key IDs;
concurrent forced misses SHALL coalesce so that only one outbound request is
started per interval. Successful, failed, and cancelled forced refresh
attempts SHALL each consume the interval. When the interval has elapsed, the next unknown-kid request SHALL
be eligible to trigger a new forced refresh (key-rotation recovery bound — the
cooldown SHALL NOT permanently lock out rotation). Ordinary TTL-driven refresh
behavior (fresh-cache fast path, single-flight fetch, Cache-Control TTL
clamping) SHALL remain unchanged.

#### Scenario: Concurrent distinct unknown kids do not amplify fetches

- **GIVEN** a mock JWKS endpoint serving one key set, and a provider whose cache is primed and TTL-fresh but older than the forced-refresh interval
- **WHEN** 32 concurrent JWTs with syntactically valid headers, distinct unknown kids, and invalid signatures are validated
- **THEN** exactly one forced outbound JWKS request is started (prime fetch plus one forced fetch in total), and every token is rejected with a token-invalid error

#### Scenario: Failing provider honors cooldown

- **GIVEN** a primed, TTL-fresh cache and a mock JWKS endpoint that now returns HTTP 500
- **WHEN** a first unknown-kid token triggers a forced refresh that fails, followed by a second unknown-kid token within the forced-refresh interval
- **THEN** the first attempt performs one outbound request and returns a provider-unavailable error, and the second attempt starts no outbound request

#### Scenario: Cancelled forced refresh consumes the interval

- **GIVEN** a primed, TTL-fresh cache older than the forced-refresh interval
- **WHEN** a forced refresh is started and its future is dropped while the outbound request is in flight, then another unknown-kid token arrives within the interval
- **THEN** the follow-up attempt starts no new outbound request within the interval

#### Scenario: Rotated key is refresh-eligible after the interval

- **GIVEN** a cache primed with key set containing kid1, one forced attempt consumed, and the mock endpoint now serving a key set containing kid2
- **WHEN** the forced-refresh interval elapses and a correctly signed token with kid2 is validated
- **THEN** a new forced refresh is started, kid2 is found, and the token validates successfully

#### Scenario: TTL-driven refresh unchanged

- **GIVEN** a provider whose cached key set has exceeded its TTL
- **WHEN** signing keys are requested
- **THEN** the fresh-cache fast path misses, a single-flight fetch is performed, and concurrent requests coalesce on it exactly as before this change
