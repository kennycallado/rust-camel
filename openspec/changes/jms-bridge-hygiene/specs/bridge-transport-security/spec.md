# bridge-transport-security

## MODIFIED Requirements

### Requirement: Secure broker URI schemes activate TLS on the JMS sidecar

The JMS sidecar SHALL map broker URI schemes honestly: `ssl://` and `wss://`
SHALL configure the Artemis remote locator with SSL enabled
using the bridge's TLS material; plaintext-scheme URIs SHALL remain plaintext.
A secure scheme without usable TLS material SHALL abort sidecar startup with
an actionable error. A URI whose scheme is none of `tcp`, `ws`, `ssl`,
`wss` SHALL abort transport setup with an actionable error naming the scheme
and the remediation (unwrap to a single primary broker URL); no scheme SHALL
be silently downgraded to plaintext, and no URI SHALL be silently redirected
to a default host — a URI without a host aborts setup.

#### Scenario: ssl scheme enables SSL transport

- **GIVEN** broker URI `ssl://broker:61617` and valid TLS material configured
  for the sidecar
- **WHEN** the JMS client factory builds the Artemis remote locator
- **THEN** the transport configuration has `SSL_ENABLED_PROP_NAME=true` and
  key/trust store properties set from the sidecar TLS material

#### Scenario: secure scheme without TLS material fails startup

- **GIVEN** broker URI `ssl://broker:61617` and missing or placeholder TLS
  material
- **WHEN** the sidecar starts
- **THEN** startup aborts with an `IllegalStateException` naming the missing
  material, and no plaintext connection is attempted

#### Scenario: plaintext scheme stays plaintext

- **GIVEN** broker URI `tcp://broker:61616`
- **WHEN** the JMS client factory builds the Artemis remote locator
- **THEN** the transport configuration has no SSL properties and connects
  without TLS

#### Scenario: failover-wrapped URI aborts transport setup

- **GIVEN** a broker URI whose scheme is `failover` (parenthesized inner or
  `failover://`-prefixed)
- **WHEN** the JMS client factory builds the Artemis transport configuration
- **THEN** setup throws `IllegalStateException` naming the unsupported
  scheme and the remediation (unwrap to a single primary broker URL,
  HA broker-side or via multiple broker entries)
- **AND** no connection to `localhost` or any default host is attempted

#### Scenario: URI without a host aborts transport setup

- **GIVEN** a broker URI whose scheme is known (`ssl`) but whose host is
  missing or blank (e.g. `ssl://:61617`)
- **WHEN** the JMS client factory builds the Artemis transport configuration
- **THEN** setup throws an `IllegalStateException` naming the URL and
  stating that no default host is assumed

#### Scenario: Rust config rejects failover URLs for Artemis at validation

- **GIVEN** an `artemis` broker entry whose `broker_url` starts with
  `failover://`
- **WHEN** the Rust pool config validates
- **THEN** validation fails with an error naming the URL and pointing at
  single-primary-URL or multiple-broker-entries remediation

#### Scenario: Rust config accepts failover URLs for Classic brokers

- **GIVEN** an `activemq` (Classic) broker entry whose `broker_url` starts
  with `failover://`
- **WHEN** the Rust pool config validates
- **THEN** validation passes (the Classic path hands the URL to
  `ActiveMQConnectionFactory`, which supports failover natively)

### Requirement: JMS consumer caps message body allocation

The JMS sidecar consumer SHALL cap forwarded message body allocation at
`JMS_MAX_BODY_BYTES` (ceiling 19 MiB, default 16 MiB), staying at or below
the Rust IPC decode limit. `BytesMessage` bodies SHALL be checked against
the pre-read body length without attempting the full allocation;
`TextMessage` bodies SHALL be materialized and UTF-8 encoded first, then
checked against the encoded byte length — the cap bounds the forwarded
body size, not the peak sidecar allocation. A body whose measured size
exceeds the cap SHALL be rejected with a warn-level diagnostic naming the
measured size in bytes and the cap, and SHALL NOT be forwarded; the reject
is a bridged error outcome (the consumer logs at warn and forwards the
error, the route owns the operational signal). The
bridge README SHALL document that the TextMessage cap counts UTF-8 bytes.

#### Scenario: TextMessage whose UTF-8 encoding exceeds the cap is rejected

- **GIVEN** a `TextMessage` whose text is at or below the cap in UTF-16
  code units but whose UTF-8 encoding exceeds the cap (e.g. CJK-heavy text)
- **WHEN** the consumer converts the message
- **THEN** conversion throws a `JMSException` whose diagnostic reports the
  UTF-8 byte size and the cap
- **AND** the message body never reaches the protobuf body or the stream

#### Scenario: ASCII text at exactly the cap passes

- **GIVEN** a `TextMessage` whose ASCII text encodes to exactly
  `JMS_MAX_BODY_BYTES` UTF-8 bytes
- **WHEN** the consumer converts the message
- **THEN** the message is forwarded with the full body intact

#### Scenario: oversized BytesMessage rejected without full allocation

- **GIVEN** a consumer with `JMS_MAX_BODY_BYTES=1024` and a mocked
  `BytesMessage` whose `getBodyLength()` reports 4096
- **WHEN** the message is consumed
- **THEN** no allocation of 4096 bytes occurs, the exchange carries the
  error outcome, and a `warn`-level log names the cap

## ADDED Requirements

### Requirement: Bridge consumer teardown destroys each consumer exactly once

The JMS bridge service SHALL destroy each consumer exactly once across all
interleavings of stream cleanup and sidecar shutdown: a teardown path SHALL
destroy a consumer only when it wins the owner-checked removal of that
consumer's map entry. Shutdown SHALL set an admission flag before draining
the map, and `subscribe` SHALL refuse new streams once the flag is set
(destroying its freshly created consumer). A late stream-termination path
racing or following `@PreDestroy` shutdown SHALL NOT trigger a second
destroy of any consumer, and no consumer present in the map at shutdown
SHALL leak (never destroyed).

#### Scenario: late stream termination after shutdown does not double-destroy

- **GIVEN** an active subscription whose consumer was already stopped and
  destroyed by `@PreDestroy` shutdown (entry removed by the shutdown drain)
- **WHEN** the stream's termination path subsequently runs its cleanup
- **THEN** cleanup stops nothing new, removes no entry, and does NOT call
  the factory destroy for that consumer a second time

#### Scenario: shutdown and cleanup racing on the same consumer destroy exactly once

- **GIVEN** an active subscription while shutdown begins iterating the
  consumer map
- **WHEN** stream cleanup and shutdown both attempt teardown of the same
  consumer
- **THEN** exactly one of the two destroys the consumer (the winner of the
  owner-checked removal), and the loser performs no destroy

#### Scenario: normal stream completion still destroys its consumer

- **GIVEN** an active subscription not racing shutdown
- **WHEN** the stream completes and its cleanup runs
- **THEN** the consumer is stopped and destroyed exactly once

#### Scenario: subscribe after shutdown begins is refused

- **GIVEN** the shutdown admission flag is set
- **WHEN** a new subscribe stream arrives
- **THEN** it is rejected with an unavailable status, and its freshly
  created consumer is destroyed without ever registering

#### Scenario: registration racing the shutdown flag is linearized

- **GIVEN** a subscribe stream and `@PreDestroy` shutdown racing
- **WHEN** both execute their admission/registration and flag-set critical
  sections
- **THEN** exactly one order holds: the registration completed first (the
  entry is in the map when shutdown drains, so the consumer is destroyed
  by the drain) or the flag was set first (the subscribe refuses and
  destroys its own consumer) — no registration both passes admission and
  escapes the drain
