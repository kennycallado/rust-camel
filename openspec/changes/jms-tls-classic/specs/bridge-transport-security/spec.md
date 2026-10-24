## ADDED Requirements

### Requirement: Classic broker TLS verifies chain and name with no skip-verification knob

The JMS bridge SHALL support TLS connections to ActiveMQ Classic brokers
configured through the provider-neutral per-broker `tls` table
(`ca_cert_path`, `client_cert_path`, `client_key_path`, `server_name`).
Verification SHALL always be on: the certificate chain SHALL be verified
against the pinned CA bundle when `ca_cert_path` is set (replacing, never
adding to, platform trust) and against the platform trust store otherwise; the
peer identity SHALL be verified against the URL host (DNS name against a DNS
SAN, IP literal against an IP SAN). When `server_name` is set it SHALL be the
exclusive reference identity for verification: the bridge SHALL bind the name
as the socket's peer host AND preserve the `server_name` server-name list on
the socket across ActiveMQ transport initialization (which otherwise
overwrites it with the URL host), using public JDK APIs (relabeling the
resolved address with `InetAddress.getByAddress(server_name, …)`; a
socket wrapper that re-asserts the server-name list when ActiveMQ applies
its own SSL parameters; endpoint identification `HTTPS`), so a certificate
that does not name
`server_name` fails even when the dialed host's own SAN would match;
verification against the dialed host SHALL NOT be accepted as a substitute
and no custom hostname matcher SHALL be written. No configuration
key SHALL skip or weaken verification, and no URL option SHALL either: on any
`ssl` URL member, member query options outside the `jms.*` namespace SHALL be
rejected at validation, and on composite URLs outer options SHALL be
restricted to `jms.*` plus the documented reconnect knobs
(`maxReconnectAttempts`, `startupMaxReconnectAttempts`) with the `nested.`
prefix rejected. The Classic Java path SHALL select the TLS seam by
URL transport security (any `ssl` member builds the SSL connection factory),
install key/trust material programmatically on the connection factory (shared
with the health probe), build all TLS objects at run time, and use no global
JSSE system properties. When a `failover:` composite contains an `ssl`
member, broker-directed transport replacement SHALL be disabled
programmatically (both the advertised-URI-update and reconnect-to switches of
the failover transport, applied before any transport I/O) so no plaintext
member can be substituted at runtime, with or without a `tls` table;
ordinary failover between the configured members SHALL remain.

#### Scenario: Pinned CA replaces platform trust

- **GIVEN** a Classic broker with a certificate from a private CA and
  `tls.ca_cert_path` pointing at that CA's PEM bundle
- **WHEN** the bridge builds the Classic connection factory
- **THEN** trust is anchored only in the given bundle and the TLS handshake
  against the broker succeeds

#### Scenario: Absent tls table means platform trust

- **GIVEN** a Classic broker whose URL uses `ssl://` and no `tls` table
- **WHEN** the bridge builds the Classic connection factory
- **THEN** the platform trust store is used, no client identity is sent, and
  the behaviour is logged at info level

#### Scenario: Mutual TLS from PEM pair

- **GIVEN** `tls.ca_cert_path`, `tls.client_cert_path`, and
  `tls.client_key_path` (unencrypted PKCS#8) configured
- **WHEN** the broker requires client authentication
- **THEN** the handshake presents the configured certificate chain and
  succeeds

#### Scenario: Hostname verification by IP literal

- **GIVEN** a broker certificate whose SAN carries only a DNS name, dialed by
  IP literal, with no `server_name`
- **WHEN** the TLS handshake runs
- **THEN** the handshake fails with failure class `TLS_HOSTNAME`

#### Scenario: server_name restores IP-dialed verification

- **GIVEN** the same DNS-SAN-only broker dialed by IP literal with
  `tls.server_name` set to a DNS name in the certificate
- **WHEN** the TLS handshake runs
- **THEN** SNI carries `server_name` and the handshake succeeds

#### Scenario: Wrong CA fails closed

- **GIVEN** `tls.ca_cert_path` pointing at a CA that did not issue the
  broker certificate
- **WHEN** the TLS handshake runs
- **THEN** the handshake fails with failure class `TLS_TRUST` and no
  plaintext fallback is attempted

#### Scenario: No skip knob exists

- **GIVEN** a `tls` table attempting an unknown key such as
  `insecure_skip_verify`
- **WHEN** the Rust pool config validates
- **THEN** validation fails on the unknown key and no verification-weakening
  key exists anywhere in the config surface

#### Scenario: Verification-weakening URL options are rejected

- **GIVEN** an `ssl://` member URL carrying a member query option outside the
  `jms.*` namespace, such as `socket.verifyHostName=false`
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the forbidden option

#### Scenario: Nested option injection through a composite is rejected

- **GIVEN** `broker_url =
  "failover:(ssl://broker:61617)?nested.socket.verifyHostName=false"`
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the forbidden `nested.` outer option

#### Scenario: Wrong server_name fails even when the dialed host would match

- **GIVEN** a certificate whose SAN names only the dialed IP host, with
  `tls.server_name` set to a different DNS name
- **WHEN** the TLS handshake runs through the actual ActiveMQ transport path
  (bridge socket factory + `SslTransport`), not raw sockets
- **THEN** the handshake fails with failure class `TLS_HOSTNAME`; identity
  verification against the dialed host does not substitute for
  `server_name`

#### Scenario: Broker-advertised failover update cannot downgrade TLS

- **GIVEN** a `failover:` composite containing an `ssl` member, with or
  without a `tls` table
- **WHEN** the bridge builds the connection
- **THEN** both broker-directed replacement switches (advertised URI updates
  and reconnect-to) are disabled programmatically before any transport I/O,
  so a broker-sent `tcp://` replacement can never replace an `ssl` member,
  and ordinary failover between the configured members still works

### Requirement: Classic composite failover URLs are accepted with uniform transport security

Rust validation SHALL accept the canonical Classic composite syntax
`failover:(uri1,uri2,…)?opts` in addition to `failover://…` for `activemq`
brokers, SHALL parse (never rewrite) the member list, and SHALL apply the
`tls` table to every member. A composite mixing `tcp://` and `ssl://` members
SHALL be rejected. Outer composite options SHALL be restricted to `jms.*`
and the documented reconnect knobs, and any composite containing an `ssl`
member SHALL have broker-directed transport replacement disabled
programmatically in the bridge. All existing Artemis failover rejections
remain unchanged.

#### Scenario: All-ssl composite with tls table is accepted

- **GIVEN** `broker_url = "failover:(ssl://a:61617,ssl://b:61617)"` and a
  valid `tls` table
- **WHEN** the Rust pool config validates
- **THEN** validation passes and the TLS material applies to both members

#### Scenario: Mixed plaintext and TLS composite is rejected

- **GIVEN** `broker_url = "failover:(tcp://a:61616,ssl://b:61617)"`
- **WHEN** the Rust pool config validates
- **THEN** validation fails with a config-class error naming the mixed
  members, regardless of whether a `tls` table is present

### Requirement: JMS TLS capability gate refuses downgraded bridges

The bridge SHALL report its supported capabilities (including
`broker-tls-v1`) in the health response. Capabilities describe the bridge
build and are provider-neutral. When TLS is configured for a broker
(possible only for Classic in this change) and the running bridge does not
report `broker-tls-v1`, the Rust side SHALL kill the bridge process and fail
permanently with an error naming the bridge binary/version and the missing
capability. When no TLS is configured, a missing capability SHALL NOT be
fatal. An old bridge reporting an empty capability set SHALL fail closed by
construction.

#### Scenario: Capable bridge proceeds

- **GIVEN** a `tls` table and a bridge whose health response lists
  `broker-tls-v1`
- **WHEN** the startup health check completes
- **THEN** the broker proceeds to readiness

#### Scenario: TLS configured without capability is fatal

- **GIVEN** a `tls` table and a health response without `broker-tls-v1`
- **WHEN** the startup health check completes
- **THEN** the bridge process is killed and the broker fails permanently with
  an error naming the missing `broker-tls-v1` capability

## MODIFIED Requirements

### Requirement: Secure broker URI schemes activate TLS on the JMS sidecar

The JMS sidecar SHALL map Artemis broker URI schemes honestly: `ssl://` and
`wss://` SHALL configure the Artemis remote locator with SSL enabled using the
bridge's TLS material; plaintext-scheme URIs SHALL remain plaintext. A secure
scheme without usable TLS material SHALL abort sidecar startup with an
actionable error. A URI whose scheme is none of `tcp`, `ws`, `ssl`, `wss`
SHALL abort transport setup with an actionable error naming the scheme and the
remediation (configure HA broker-side); no scheme SHALL be silently downgraded
to plaintext, and no URI SHALL be silently redirected to a default host — a
URI without a host aborts setup. Classic brokers are governed by the Classic
TLS requirements of this specification, not by this requirement.

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

- **GIVEN** an Artemis broker URI whose scheme is `failover` (parenthesized
  inner or `failover://`-prefixed)
- **WHEN** the JMS client factory builds the Artemis transport configuration
- **THEN** setup throws `IllegalStateException` naming the unsupported
  scheme and the remediation (configure HA broker-side)
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
  single-primary-URL remediation and broker-side HA

#### Scenario: Rust config accepts failover URLs for Classic brokers

- **GIVEN** an `activemq` (Classic) broker entry whose `broker_url` starts
  with `failover://` or `failover:(`
- **WHEN** the Rust pool config validates
- **THEN** validation passes (the Classic path hands the URL to
  `ActiveMQConnectionFactory`, which supports failover natively)
