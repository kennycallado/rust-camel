# bridge-transport-security

## ADDED Requirements

### Requirement: Inbound action validation

The profile builder SHALL reject an explicitly configured inbound action
string whose actions lack their crypto material at build time, so that no
profile can silently skip inbound verification or decryption.

#### Scenario: signature action without truststore is rejected

- **GIVEN** a keystore path is configured
- **WHEN** the builder is given `actions.in` containing `Signature` and no
  truststore path
- **THEN** `build()` throws `IllegalArgumentException` naming
  `cxf.truststore.path` and stating that the keystore fallback applies only
  to the manual consumer path

#### Scenario: encrypt action without keystore is rejected

- **GIVEN** a truststore path is configured
- **WHEN** the builder is given `actions.in` containing `Encrypt` and no
  keystore path
- **THEN** `build()` throws `IllegalArgumentException` naming
  `cxf.keystore.path`

#### Scenario: blank inbound actions stay exempt

- **WHEN** the builder is given no `actions.in` value and no truststore or
  keystore
- **THEN** `build()` succeeds and `createInInterceptor()` returns `null`

#### Scenario: whitespace-only inbound actions stay exempt

- **WHEN** the builder is given `actions.in` of `"   "` and no truststore or
  keystore
- **THEN** `build()` succeeds and `createInInterceptor()` returns `null`

#### Scenario: signature-only with truststore builds

- **GIVEN** a truststore path is configured and no keystore
- **WHEN** the builder is given `actions.in` of `Signature`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for Signature only

#### Scenario: encrypt-only with keystore builds

- **GIVEN** a keystore path is configured and no truststore
- **WHEN** the builder is given `actions.in` of `Encrypt`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for Encrypt only

#### Scenario: both actions with both materials build

- **GIVEN** truststore and keystore paths are configured
- **WHEN** the builder is given `actions.in` of `Signature Encrypt`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for both actions
