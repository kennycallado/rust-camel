# redis-tls Specification

## Purpose
TBD - created by archiving change redis-live-tls-and-ready-race. Update Purpose after archive.
## Requirements
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
contents. CA trust is standalone-only in this change; the sentinel
surface is follow-up bd rc-hbde6.

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

### Requirement: camel-test integration feature enables the repo TLS client

The `camel-test` crate's `integration-tests` feature SHALL enable the
TLS client feature of `camel-redis-repo` (transitively
`camel-component-redis/tls`), so the repo connection path can build
TLS clients under integration tests.

#### Scenario: Repo path builds TLS clients under integration-tests

- **GIVEN** `camel-test` built with `--features integration-tests`
- **WHEN** a `rediss://` endpoint is resolved through the repo
  connection path
- **THEN** the build has the redis TLS client feature enabled and the
  connection attempt proceeds to a TLS handshake (no feature-absent
  `Config` error)

### Requirement: rediss:// live coverage through the repository connection path

The camel-test workspace SHALL provide a live TLS Redis topology
(self-signed CA generated at test time, container with the plaintext
port disabled) and an integration test that drives `rediss://`
end-to-end through the camel-redis-repo connection path, gated behind
the `integration-tests` feature and never `#[ignore]`d (ADR-0054).

#### Scenario: Round-trip through rediss:// with a self-signed CA

- **GIVEN** a TLS-enabled Redis container whose certificate chains to
  a test-generated CA, and a repo built from a `rediss://` endpoint
  configured with that CA via `tls_ca_cert`
- **WHEN** the test puts, gets, and deletes a key through the repo
- **THEN** every operation succeeds and the get returns the put value,
  proving TLS handshake plus full command path

#### Scenario: Plaintext against the TLS port fails

- **GIVEN** the same TLS-only Redis container (plaintext port
  disabled)
- **WHEN** a plaintext `redis://` client connects to the TLS port
- **THEN** the connection fails with a TLS/protocol error rather than
  succeeding

#### Scenario: rediss:// against a plaintext port fails

- **GIVEN** a standard plaintext-only Redis container (the existing
  shared fixture)
- **WHEN** a `rediss://` client connects to its plaintext port
- **THEN** the connection fails rather than succeeding

#### Scenario: Wrong CA is rejected

- **GIVEN** the TLS-enabled Redis container and a client configured
  with `tls_ca_cert` pointing to a DIFFERENT test-generated CA
- **WHEN** the client connects
- **THEN** the TLS handshake is rejected (certificate verification
  failure), proving the CA actually verifies the server certificate

#### Scenario: Live TLS coverage is discoverable

- **GIVEN** the merged change
- **WHEN** searching the workspace for `rediss://`
- **THEN** live test surfaces (not only config/unit surfaces) contain
  the scheme

