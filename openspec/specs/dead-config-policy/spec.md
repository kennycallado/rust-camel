# dead-config-policy Specification

## Purpose
TBD - created by archiving change audit-fix-dead-config. Update Purpose after archive.
## Requirements
### Requirement: No silently ignored config fields

The system SHALL NOT parse a config field from URI parameters or TOML configuration unless the field is consumed by runtime logic. When a removed parameter is present in a URI, the system SHALL reject it with an error indicating the parameter is not supported.

**Exception:** A field may be retained on a config struct for serde deserialization compatibility if `validate()` rejects any non-default value with an explicit error (fail-closed). Such a field is not considered silently ignored because the operator receives an error at validation time.

#### Scenario: Removed xj transformDirection rejected

- **GIVEN** a `camel-xj` endpoint URI with `transformDirection=XML2JSON`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `transformDirection` is not supported (use `direction` instead)

#### Scenario: Removed xj resourceUri rejected

- **GIVEN** a `camel-xj` endpoint URI with `resourceUri=classpath:extra.xslt`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `resourceUri` is not supported

#### Scenario: Removed http cookieHandling rejected

- **GIVEN** a `camel-http` endpoint URI with `cookieHandling=InMemory`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `cookieHandling` is not supported

#### Scenario: Removed direct block rejected

- **GIVEN** a `camel-direct` endpoint URI with `block=true`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `block` is not supported

#### Scenario: Removed direct exchange_pattern rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchange_pattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchange_pattern` is not supported

#### Scenario: Removed direct exchangePattern (camelCase) rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchangePattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchangePattern` is not supported

### Requirement: proxy_url validation rejection

The system SHALL retain the `proxy_url` field on `HttpConfig` for serde deserialization compatibility but SHALL reject any non-None value at validation time with an SSRF-specific error.

#### Scenario: proxy_url set to valid URL rejected at validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `Some("http://proxy:8080")`
- **WHEN** `validate()` is called
- **THEN** validation fails with an error stating proxy_url is incompatible with SSRF DNS pinning

#### Scenario: proxy_url None passes validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `None`
- **WHEN** `validate()` is called
- **THEN** validation succeeds

#### Scenario: proxy_url from TOML rejected at validation

- **GIVEN** TOML config with `proxy_url = "http://proxy:8080"` (deserializes successfully because the field is retained)
- **WHEN** `validate()` is called
- **THEN** validation fails with the SSRF incompatibility error

### Requirement: WebSocket send timeout enforcement (client mode)

The system SHALL enforce `send_timeout` on the client-mode WebSocket send path (`ws_stream.send`). Server-send mode (internal mpsc `try_send_with_backpressure`) SHALL NOT be affected. When the timeout elapses before the client-mode send completes, the system SHALL return an error.

#### Scenario: Client send completes within timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=5000`
- **WHEN** a message is sent via `ws_stream.send` and the sink accepts it within 5 seconds
- **THEN** the send succeeds with no error

#### Scenario: Client send exceeds timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=100`
- **WHEN** a message is sent via `ws_stream.send` and the sink does not accept it within 100 milliseconds
- **THEN** the send returns a timeout error

#### Scenario: Default send timeout

- **GIVEN** a `camel-ws` endpoint with no `sendTimeoutMs` specified
- **WHEN** the endpoint configuration is resolved
- **THEN** the default send timeout is 30 seconds

### Requirement: Removal of never-consumed native issuer surface

The `token_issuer` and `clients` config fields and their backing runtime code
(`native_issuer`, `native_client_store`, `native_jwks`, `ApiKeyAuthenticator`,
`camel-http::auth` wrapper) SHALL be removed. `deny_unknown_fields` SHALL reject
stale configs loudly. The scalar `api_key` field SHALL remain and SHALL be wired
as a single-entry credential.

#### Scenario: Stale token_issuer config fails loudly

- **GIVEN** a Camel.toml containing `[security.native.token_issuer]`
- **WHEN** the CLI starts
- **THEN** startup fails with an unknown-field configuration error rather than silently ignoring the block

#### Scenario: Dead code is gone from the workspace

- **GIVEN** the merged main branch
- **WHEN** searching the workspace for `NativeTokenIssuer`, `M2mClientStore`, `NativeJwksProvider`, `ApiKeyAuthenticator`
- **THEN** no definitions or references remain outside git history

### Requirement: No documented placeholder recipe ships a literal credential

Documentation for `security.*` credential fields SHALL NOT present placeholder
syntax that the resolver does not support. Where a placeholder form is
documented for a credential field, the documented behavior SHALL match the
fail-closed semantics: unset variable means startup failure, not a live
literal-credential. The ambiguous `{{env:VAR:-default}}` double-dash form SHALL
be documented as rejected.

#### Scenario: Docs recipes match resolver behavior

- **GIVEN** `docs/src/configuration/schema.md` and `crates/camel-config/README.md` after this change
- **WHEN** a reader follows any placeholder recipe for `bearer_token`, `api_key`, or `client_secret` exactly as written
- **THEN** the recipe either resolves the secret from the environment or fails closed with a `ConfigError` — no recipe produces a config where the literal placeholder string is the accepted credential

#### Scenario: Double-dash default form is documented as rejected

- **GIVEN** the configuration docs after this change
- **WHEN** a reader writes `{{env:X:-changeme}}` in a security credential field with `X` unset
- **THEN** the documented and actual behavior is a `ConfigError` at startup, and the docs state this explicitly in the syntax-boundary note

### Requirement: cache_repo cross-backend field rejection

The `cache_repo` configuration section SHALL fail validation, per the dead-config-policy
fail-closed principle, when fields that do not apply to the configured `backend` are set
to non-default values: with `backend = "memory"`, any of `path`, `stale_retention`,
`max_entries`, `cache_size`, or `sweep_interval` set SHALL be rejected; with
`backend = "redb"`, `max_capacity` set SHALL be rejected. An omitted `stale_retention`
SHALL deserialize as `None` (the serde default SHALL NOT materialize a value for an
absent field), and the 7-day fallback SHALL apply in wiring only after validation
passes for the redb backend — so memory-backend configs that omit the field validate
unchanged.

#### Scenario: cache_size on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.cache_size = "512MiB"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size` as not applicable
  to the `memory` backend

#### Scenario: path on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.path = "data/cache.redb"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.path` as not applicable to the
  `memory` backend

#### Scenario: max_capacity on redb backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with valid redb fields
  and `cache_repo.max_capacity = 5000`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.max_capacity` as not
  applicable to the `redb` backend

#### Scenario: omitted stale_retention stays None on memory backend

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` with only
  `max_capacity` set (stale_retention omitted)
- **WHEN** the config is deserialized and validated
- **THEN** `stale_retention` deserializes as `None` and validation succeeds

#### Scenario: omitted stale_retention falls back in wiring for redb

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with a path and
  `cache_size`, stale_retention omitted
- **WHEN** the context is built from that config
- **THEN** the `"persistent"` repository is constructed with a 7-day stale retention
  applied by wiring, and validation did not treat the field as set

