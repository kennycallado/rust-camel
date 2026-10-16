# cxf-listener-tls Specification

## Purpose
TBD - created by archiving change 361-cxfsoap. Update Purpose after archive.
## Requirements
### Requirement: Listener TLS is a pool-level, validated shape

Listener transport TLS SHALL be configured at the pool level, beside
`bind_address`, with exactly these keys: `soap_tls_cert_path`,
`soap_tls_key_path`, `soap_tls_client_ca_path`, `soap_tls_pkcs12_path`, and
`soap_tls_pkcs12_password`. They SHALL be forwarded as the five
`CXF_SOAP_TLS_*` uppercase environment variables through the non-profile env
path (`bridge_config_with_env_trace`), not through per-profile
`CXF_PROFILE_<NAME>_*` variables. The `soap_tls_pkcs12_password` SHALL be
redacted. There SHALL be exactly one Vert.x listener with one TLS identity
serving all profiles.

The configuration SHALL supply exactly one key material form: either
`soap_tls_cert_path` + `soap_tls_key_path`, or `soap_tls_pkcs12_path` +
`soap_tls_pkcs12_password` — never both and never neither.
`soap_tls_client_ca_path` SHALL always be required. Supplying TLS material with
an `http` bind address SHALL be rejected. Validation SHALL run both when Rust
loads the config and when the Java bridge starts, before any socket binds.

#### Scenario: Cert/key form is accepted

- **GIVEN** `bind_address = https://...`, `soap_tls_cert_path` and
  `soap_tls_key_path` set, plus `soap_tls_client_ca_path`
- **WHEN** the pool is validated and the bridge starts
- **THEN** the listener binds with client authentication

#### Scenario: PKCS12 form is accepted

- **GIVEN** `soap_tls_pkcs12_path` and `soap_tls_pkcs12_password` set, no PEM
  pair, and `soap_tls_client_ca_path`
- **WHEN** the pool is validated and the bridge starts
- **THEN** the listener binds using the PKCS12 store

#### Scenario: Implicit plaintext is rejected

- **GIVEN** no key material configured and `bind_address` using `https`
- **WHEN** the pool is validated at startup
- **THEN** startup fails with an actionable error

#### Scenario: Both forms or neither is rejected

- **GIVEN** a config supplying both the PEM and PKCS12 forms, or neither, with
  `https`
- **WHEN** the pool is validated at startup
- **THEN** startup fails with an actionable error

#### Scenario: Missing client CA is rejected

- **GIVEN** key material set with no `soap_tls_client_ca_path`
- **WHEN** the pool is validated at startup
- **THEN** startup fails before any socket binds

#### Scenario: TLS with http bind address is rejected

- **GIVEN** any `soap_tls_*` material with `bind_address` using `http`
- **WHEN** the pool is validated at startup
- **THEN** startup fails with an actionable error

#### Scenario: TLS keys travel as pool env vars

- **GIVEN** a TLS-configured pool
- **WHEN** the bridge process is launched
- **THEN** the `CXF_SOAP_TLS_*` variables are present and no per-profile TLS
  variable is emitted

#### Scenario: One listener identity serves all profiles

- **GIVEN** a pool with several profiles
- **WHEN** the listener binds
- **THEN** a single HTTP server with one certificate serves every
  `/cxf/<profile>` path

### Requirement: Listener requires client authentication and fails closed

The listener SHALL configure TLS with `ClientAuth.REQUIRED` validated against
`soap_tls_client_ca_path`. Listener TLS material SHALL be independent of the WSS
message-layer stores. An incomplete or unreadable configuration SHALL fail
before any socket binds.

#### Scenario: Mutual TLS request succeeds

- **GIVEN** a listener with cert, key, and client CA and a client presenting a
  CA-signed certificate
- **WHEN** the client posts a SOAP request
- **THEN** the handshake succeeds and the request is processed

#### Scenario: Missing or wrong client certificate is rejected

- **GIVEN** a listener requiring client authentication
- **WHEN** a client connects without a certificate or with a certificate signed
  by a different CA
- **THEN** the handshake is rejected before any SOAP processing

#### Scenario: Listener TLS material is independent of WSS stores

- **GIVEN** a profile with WSS `security.keystore_*` set but no pool TLS material
- **WHEN** an `https` listener is started
- **THEN** it fails closed rather than reusing the WSS store

#### Scenario: Plaintext http remains available

- **GIVEN** a bind address using `http`
- **WHEN** the listener starts
- **THEN** it binds without TLS, preserving the ingress-termination workaround

### Requirement: The actual bound SOAP port is announced and consumed

The bridge SHALL, after the SOAP listener binds, print exactly one JSON line
`{"status":"soap-ready","port":<N>}` on stdout and flush, where `<N>` is the
actual bound port. The gRPC line `{"status":"ready","port":<N>}` SHALL remain
unchanged and distinct. `camel-bridge`'s `BridgeProcess` SHALL parse both lines,
in either order, bounded by the same `bridge_start_timeout_ms`, and SHALL store
and expose an optional bound SOAP port. A malformed `soap-ready` line SHALL be a
startup failure. When no SOAP listener is configured (no bind), the supervisor
SHALL NOT wait for a `soap-ready` line.

#### Scenario: Bound port is announced

- **GIVEN** a listener bound to an ephemeral port
- **WHEN** binding completes
- **THEN** the bridge prints `{"status":"soap-ready","port":<N>}` with the real
  port and tests connect using that port without a bind/drop probe

#### Scenario: Either ready order is accepted

- **GIVEN** a bridge that prints `soap-ready` before or after `ready`
- **WHEN** the supervisor reads startup output
- **THEN** it accepts both orders within the single startup timeout

#### Scenario: Malformed soap-ready line fails startup

- **GIVEN** a bridge that prints a `soap-ready` line with a missing or
  non-numeric port
- **WHEN** the supervisor parses startup output
- **THEN** startup fails with an actionable error

#### Scenario: No soap wait absent a bound listener

- **GIVEN** a pool with no SOAP bind configured
- **WHEN** the supervisor starts the bridge
- **THEN** it does not wait for a `soap-ready` line

### Requirement: SOAP-ready expectation is an explicit BridgeProcessConfig opt-in

`BridgeProcessConfig` SHALL gain an `expect_soap_ready: bool` field defaulting to
`false`. Only a CXF bridge configured with a SOAP bind SHALL set it to `true`;
every other bridge, and a CXF pool without a bind, SHALL leave it `false` and
await only the gRPC `ready` line. Readiness SHALL be bounded by
`bridge_start_timeout_ms`. This is a planning note: other bridges are unchanged.

#### Scenario: Default false waits only for ready

- **GIVEN** a `BridgeProcessConfig` with the default `expect_soap_ready = false`
- **WHEN** the supervisor starts the bridge
- **THEN** it awaits only the gRPC `ready` line within the timeout

#### Scenario: CXF with a bind opts in

- **GIVEN** a CXF pool configured with a SOAP bind
- **WHEN** the supervisor builds the config
- **THEN** `expect_soap_ready` is `true` and both lines are awaited within the
  timeout

#### Scenario: Other bridges are unchanged

- **GIVEN** a JMS or XML bridge config
- **WHEN** the supervisor starts it
- **THEN** `expect_soap_ready` is `false` and behavior is unchanged

