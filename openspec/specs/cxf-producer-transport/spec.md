# cxf-producer-transport Specification

## Purpose
Transport TLS for the CXF producer conduit. The producer takes a nested
`[profiles.security.tls]` keystore/truststore configuration, all-or-nothing,
and retains hostname verification with no option to disable it.

## Requirements
### Requirement: Producer transport TLS is a nested, all-or-nothing profile config

The producer dispatch conduit SHALL take transport TLS from a nested
`[profiles.security.tls]` table with these keys:
`keystore_path`, `keystore_password`, `keystore_type`, `truststore_path`,
`truststore_password`, `truststore_type`. `*_type` SHALL be an enum of `JKS` or
`PKCS12` with default `PKCS12`. The block SHALL be serialized as
`CXF_PROFILE_<NAME>_TLS_*` environment variables and applied to the CXF
`HTTPConduit` via `TLSClientParameters`. For each of keystore and truststore,
`*_path` and `*_password` SHALL be supplied together; a truststore password
SHALL be required whenever a truststore path is supplied. A wrong store password
SHALL fail Java startup before the gRPC listener becomes ready. Credentials
SHALL be redacted from Debug/log output (ADR-0051). A profile's TLS material
SHALL NOT be applied to another profile.

#### Scenario: Configured client certificate is presented

- **GIVEN** a profile with `tls.keystore_path`/`tls.keystore_password` (and type)
  and `tls.truststore_path`/`tls.truststore_password`, and a server requiring
  mutual TLS
- **WHEN** a SOAP operation is invoked through that profile
- **THEN** the handshake completes with the configured client certificate

#### Scenario: Store type defaults to PKCS12

- **GIVEN** a profile with a `tls.keystore_path`/`tls.keystore_password` pair and
  no `tls.keystore_type`
- **WHEN** the conduit builds key managers
- **THEN** the store is read as PKCS12

#### Scenario: Absent tls block uses JVM defaults

- **GIVEN** a profile with no `[profiles.security.tls]` block
- **WHEN** a SOAP operation is invoked
- **THEN** the dispatch uses the JVM default SSL context and installs no
  explicit `TLSClientParameters`

#### Scenario: Truststore-only enables server authentication without a client cert

- **GIVEN** a profile with only `tls.truststore_path` and
  `tls.truststore_password`
- **WHEN** the dispatch connects to a TLS server
- **THEN** the server certificate is validated and no client certificate is sent

#### Scenario: Incomplete store pair fails startup

- **GIVEN** a profile with `tls.keystore_path` set and `tls.keystore_password`
  unset (or a truststore path without a password)
- **WHEN** the pool config is validated at startup
- **THEN** startup fails with an actionable error naming the missing pair member

#### Scenario: Wrong store password fails Java startup

- **GIVEN** a profile whose `tls.keystore_password` does not open the keystore
- **WHEN** the bridge starts
- **THEN** startup fails before the gRPC listener becomes ready

#### Scenario: Cross-profile isolation

- **GIVEN** profile `baleares` configured with TLS material and profile `cyl`
  without
- **WHEN** both invoke against a mutual-TLS server
- **THEN** only `baleares` presents a certificate; `cyl` fails the handshake

### Requirement: Hostname verification is retained on the producer conduit

The producer transport TLS configuration SHALL retain hostname verification;
the component SHALL NOT expose an option that disables it.

#### Scenario: Wrong-SAN host is rejected

- **GIVEN** a profile with valid key/trust material whose certificate SAN does
  not match the endpoint host
- **WHEN** the dispatch connects
- **THEN** the handshake fails with a hostname-verification error

