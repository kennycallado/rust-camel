## MODIFIED Requirements

### Requirement: WSS replay protection on consumer inbound path

The CXF sidecar's inbound WS-Security processing SHALL attach a replay cache spanning the
endpoint lifetime to `RequestData` (both timestamp and nonce caches), so a captured,
still-fresh signed SOAP message is rejected when replayed. The publisher SHALL construct at
most one `WssSecurityProcessor` per security profile so that cache state persists across
requests. The emitted `wsu:Timestamp` SHALL be covered by the signature action, and inbound
verification SHALL require a Timestamp on messages whose profile declares the Timestamp
action — so an attacker cannot mint a fresh cache key by rewriting or stripping the
unsigned timestamp of a captured message.

#### Scenario: replayed fresh signed message rejected at processor level

- **GIVEN** a `WssSecurityProcessor` with signing configured and actions `Timestamp Signature`,
  and a signed+timestamped SOAP envelope produced by `processOutbound`
- **WHEN** `processInbound` processes the identical envelope bytes twice on the same processor
- **THEN** the first invocation succeeds and the second invocation throws a
  `WSSecurityException`

#### Scenario: replayed message rejected through the published endpoint

- **GIVEN** a published consumer endpoint whose profile enables inbound verification with
  actions `Timestamp Signature`, and a validly signed+timestamped SOAP request
- **WHEN** the identical request bytes are POSTed to the endpoint twice
- **THEN** the first request succeeds and the second request fails with a security failure

#### Scenario: timestamp rewrite cannot mint a fresh cache key

- **GIVEN** a validly signed+timestamped SOAP request accepted by the endpoint, and the same
  request bytes with the `wsu:Timestamp` element rewritten to fresh Created/Expires values
  (unsigned, signature broken)
- **WHEN** the original bytes and the timestamp-rewritten variant are each POSTed
  again after the original was accepted
- **THEN** both are rejected with a security failure (replay or signature validation), and
  neither is processed as a fresh message
