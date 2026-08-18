## ADDED Requirements

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
