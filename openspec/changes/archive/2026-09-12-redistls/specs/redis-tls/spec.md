## ADDED Requirements

### Requirement: Redis TLS trust model is documented

The Redis component documentation SHALL state that Redis TLS authenticates the
server only in v1, using `tls_ca_cert` when configured, and that client
certificate authentication (mTLS) is unsupported.

#### Scenario: Operator reads Redis TLS security guidance

- **GIVEN** an operator reads the Redis component security section
- **WHEN** the operator evaluates `rediss://` and `tls_ca_cert`
- **THEN** the documentation states that server verification is supported and
  client-certificate authentication is not supported, including for Sentinel
  TLS where the configured CA is supported

### Requirement: Redis context records the implementation decision

The camel-redis context documentation SHALL identify the existing
`TlsCertificates.client_tls = None` behavior as intentional and state that the
decision can be revisited when a deployment requires Redis client
authentication.

#### Scenario: Maintainer evaluates the Redis topology

- **GIVEN** a maintainer reads the `tls_ca_cert` trust context
- **WHEN** the maintainer asks whether client credentials are wired
- **THEN** the context identifies `client_tls: None` as the current deliberate
  behavior and names client-auth deployment demand as the change trigger
