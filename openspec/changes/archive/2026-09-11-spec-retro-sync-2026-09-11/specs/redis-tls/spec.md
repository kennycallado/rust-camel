## MODIFIED Requirements

### Requirement: Standalone TLS connections trust the configured CA

When a standalone Redis endpoint resolves to TLS and a CA certificate
is configured (`tls_ca_cert`, a file path on the global config), the
camel-redis component SHALL propagate that path onto the endpoint
configuration during `apply_defaults`, read the file to PEM bytes at
topology construction, and build the client with those bytes as the
TLS root of trust (server-verified TLS via the CA-trusting
constructor). It SHALL NOT fall back to webpki-only roots or an
insecure bypass on this path. When no CA is configured, client
construction SHALL remain unchanged. A CA file that cannot be read
SHALL fail closed at endpoint creation with a `Config` error whose
message may name the path (paths are metadata) but never file
contents. The sentinel surface carries the same fail-closed CA read
through the per-plane requirement below (landed in 7b750a0b and
a19d9cb4, bd rc-hbde6).

#### Scenario: CA-configured endpoint takes the CA trust branch

- **GIVEN** a standalone endpoint configuration with TLS resolved and
  `tls_ca_cert` set to a readable PEM file
- **WHEN** the topology resolves the client
- **THEN** the endpoint config carries the CA path after
  `apply_defaults`, and the client is built through the CA-trusting
  TLS constructor with the file's PEM bytes as root certificate

#### Scenario: CA-absent endpoint keeps the default constructor

- **GIVEN** a standalone endpoint configuration with TLS resolved and
  no `tls_ca_cert`
- **WHEN** the topology resolves the client
- **THEN** the client is built through the default constructor with no
  custom root certificate

#### Scenario: Unreadable CA file fails closed

- **GIVEN** a standalone endpoint configuration with TLS resolved and
  `tls_ca_cert` set to a path that does not exist or is not readable
- **WHEN** the topology is constructed from config
- **THEN** endpoint creation returns a `Config` error naming the path,
  before any connection attempt

#### Scenario: TLS feature-absent build still fails closed

- **GIVEN** a build of camel-redis without a TLS cargo feature and an
  endpoint that resolves to TLS
- **WHEN** the topology is constructed from config
- **THEN** endpoint creation returns a `Config` error at the
  `validate_tls` choke point, with a message that avoids
  transient-classifier words and embeds no host or URL

## ADDED Requirements

### Requirement: Sentinel CA trust installs per plane and fails closed on mixed schemes

When a sentinel topology configures `tls_ca_cert`, the CA read gate
SHALL consider both planes independently: the endpoint `ssl` flag
(standalone and sentinel DATA links) and any structured `rediss://`
sentinel node URL (the SENTINEL links) — keying on `ssl` alone would
starve a configured CA for a TLS-sentinel/plaintext-data mix. The PEM
SHALL install only on the planes that actually use TLS: the sentinel
links when a node URL carries a TLS scheme, the data links when
`node_tls` is set — and never on a `Tcp` address (redis-rs rejects
certificates there), so mixed TLS-plaintext plane selections each
trust the CA on their own surface. Because redis-rs applies one
certificate setting to every sentinel link, a CA configured against a
sentinel node list mixing `rediss://` and `redis://` schemes SHALL
fail closed at config time with a `Config` error naming the
one-scheme constraint; a mixed list without a CA stays buildable.
(Landed in 7b750a0b and a19d9cb4, bd rc-hbde6.)

#### Scenario: CA read gate covers the sentinel plane

- **GIVEN** a sentinel endpoint whose node list carries a `rediss://`
  URL, with plaintext data links (`ssl` unset) and a `tls_ca_cert`
  path
- **WHEN** the topology is constructed from config
- **THEN** the CA file is read and offered to the TLS sentinel links —
  the gate does not starve the CA just because the endpoint `ssl` flag
  is unset

#### Scenario: Mixed planes each trust their own CA surface

- **GIVEN** sentinel topologies mixing TLS sentinel links with
  plaintext data links, and the reverse, each with `tls_ca_cert`
  configured
- **WHEN** the topologies are constructed
- **THEN** the PEM installs on the TLS plane(s) only, and no
  certificate is offered to a `Tcp` address

#### Scenario: CA with a mixed-scheme node list fails closed

- **GIVEN** a sentinel node list mixing `rediss://` and `redis://`
  URLs and a configured `tls_ca_cert`
- **WHEN** the topology is constructed from config
- **THEN** construction returns a `Config` error naming the one-scheme
  constraint, before any connection attempt

#### Scenario: Mixed schemes without a CA stay buildable

- **GIVEN** the same mixed node list without `tls_ca_cert`
- **WHEN** the topology is constructed from config
- **THEN** construction succeeds — each link speaks its own scheme's
  transport

### Requirement: Sentinel TLS live coverage through rediss-sentinel://

The camel-test workspace SHALL provide a live TLS sentinel topology —
one container running a TLS-only Redis master and a TLS-only sentinel
monitoring it (`tls-replication` enabled), both trusting a
test-generated CA kept process-side for `tls_ca_cert` — and an
integration test that drives `rediss-sentinel://` end-to-end through
the repository connection path, proving both encrypted surfaces at
once: the discovery hop (sentinel link) and the resolved master link.
Plaintext negative controls SHALL prove each port really speaks TLS.
Gated behind the `integration-tests` feature and never `#[ignore]`d
(ADR-0054). (Landed in 854a429c, bd rc-hbde6.)

#### Scenario: Round-trip through rediss-sentinel:// with a shared CA

- **GIVEN** the TLS sentinel container (TLS-only master + TLS-only
  sentinel) and a repository built from a `rediss-sentinel://`
  endpoint configured with the fixture CA via `tls_ca_cert`
- **WHEN** the test round-trips a cache entry through the repository
- **THEN** every operation succeeds over both the sentinel discovery
  link and the resolved master link, each verified by the CA

#### Scenario: Plaintext against the TLS sentinel port fails

- **GIVEN** the same topology
- **WHEN** a plaintext `redis://` client connects to the sentinel's
  TLS port
- **THEN** the connection fails with a TLS/protocol error rather than
  succeeding

#### Scenario: Plaintext against the TLS master port fails

- **GIVEN** the same topology
- **WHEN** a plaintext `redis://` client connects to the master's TLS
  port
- **THEN** the connection fails with a TLS/protocol error rather than
  succeeding
