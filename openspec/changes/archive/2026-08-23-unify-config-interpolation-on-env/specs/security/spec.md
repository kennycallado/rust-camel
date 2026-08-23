# security spec delta

## MODIFIED Requirements

### Requirement: Security credential placeholder resolution

ALL `[security.*]` string leaves SHALL be resolved through the shared `interpolate_env`
engine (`${env:NAME}` / `${env:NAME:-default}` syntax) under the strict gate — credential
leaves (`native.bearer_token`, `native.api_key`, every `native.credentials[]` entry's
`secret`/`secret_env`/`subject`/`roles`/`scopes`, `keycloak.client_secret`,
`oidc.client_secret`) and non-credential leaves (samples: `subject`, `issuer`,
`keycloak.realm`, `oidc.jwks_uri`; the walk is structural over every string leaf under
`[security.*]`, not an enumerated allowlist) — plus `[datasources.*]` connection leaves
(`db_url`, `provider`, the `ssl_mode`/`ssl_root_cert`/`ssl_cert`/`ssl_key` TLS fields, every
string under `extra` including SurrealDB `password`) and `[idempotent_repo]`/`[cache_repo]` leaves (`url` — a connection URL that may carry userinfo — and `sentinel_password`), which are treated as credential-class. Strict-class criterion: a
section is strict iff its string leaves reach an external authenticator or connection
secret; the prefix set (`security`, `datasources`, `idempotent_repo`, `cache_repo`) is declared once
(`STRICT_PREFIXES`) and consumed by both dispatch and tests.
EVERY leaf covered by the strict gate SHALL keep uniform fail-closed semantics: an unset env
var without a default yields `ConfigError`; a residual unresolved marker (legacy `{{`, or a
malformed/unconsumed `${` such as truncated `${env:` or `${notenv:x}`) after resolution
yields `ConfigError`; an escaped `$${env:...}` on a strict-gate leaf yields `ConfigError`
(the `:-` separator is NATIVE syntax under `${env:}` and resolves normally). The authenticator boundary guard (`NativeCredentialStore` construction
via `ensure_no_placeholder_markers`) SHALL carry forward unchanged as defense-in-depth.

#### Scenario: Placeholder resolves to real secret

- **Given** Camel.toml with `[security.native] bearer_token = "${env:APP_TOKEN}"` and env `APP_TOKEN=real-secret`
- **When** the config loads
- **Then** the native credential store accepts `real-secret` as the static token and rejects the literal `${env:APP_TOKEN}`

#### Scenario: Unset env var on a covered leaf fails closed

- **Given** `[security.native] bearer_token = "${env:APP_TOKEN}"` with `APP_TOKEN` unset
- **When** the config loads
- **Then** `load_config` returns `Err` (`ConfigError`) naming the field — the literal placeholder string is never installed as a live credential

#### Scenario: Single-colon default resolves normally

- **Given** `[security.native] bearer_token = "${env:APP_TOKEN:-fallback-tok}"` with `APP_TOKEN` unset
- **When** the config loads
- **Then** the credential resolves to `fallback-tok` without error

#### Scenario: credentials array secret resolves and fails closed

- **Given** `[[security.native.credentials]] secret = "${env:CRED_SECRET}"` with `CRED_SECRET` set (resp. unset, no default)
- **When** the config loads
- **Then** the entry's secret equals the env value (resp. `load_config` returns `Err` naming the entry)

#### Scenario: legacy braces rejected in security fields

- **Given** `[security.native] bearer_token = "{{env:APP_TOKEN}}"` (legacy syntax)
- **When** the config loads
- **Then** `load_config` returns `Err` with an actionable message naming the field and the `${env:}` replacement — regardless of whether `APP_TOKEN` is set

#### Scenario: Dash-prefixed default fails closed on any covered leaf

- **Given** `[security.native] bearer_token = "{{env:X:-changeme}}"` (legacy braces with shell separator)
- **When** the config loads
- **Then** `ConfigError` via the legacy pre-scan — never `-changeme`; whereas `[security.native] bearer_token = "${env:X:-changeme}"` (native syntax) resolves successfully to `changeme`

#### Scenario: malformed dollar marker rejected after resolution

- **Given** a security field whose resolved value still contains an unconsumed `${` marker (e.g. truncated `${env:` or `${notenv:x}`)
- **When** the config loads
- **Then** `load_config` returns `Err` naming the field (residual-marker strictness preserved)

#### Scenario: valid new-syntax security config passes the strict gate

- **Given** `[security.keycloak] client_secret = "${env:KC_SECRET}"` with `KC_SECRET` set
- **When** the config loads
- **Then** the resolved secret equals the env value — the residual check does not fire on consumed `${env:}` placeholders

#### Scenario: keycloak client secret fails closed when missing

- **Given** `[security.keycloak] client_secret = "${env:KC_SECRET}"` with `KC_SECRET` unset and no default
- **When** the config loads
- **Then** `load_config` returns `Err` (`ConfigError`) naming the field

#### Scenario: escaped placeholder rejected on strict-gate leaves

- **Given** `[security.native] bearer_token = "$${env:APP_TOKEN}"` (escape form)
- **When** the config loads
- **Then** `load_config` returns `Err` via residual-marker rejection — the literal placeholder text never reaches a credential store

#### Scenario: oidc client secret resolves

- **Given** `[security.oidc] client_secret = "${env:OIDC_SECRET}"` with `OIDC_SECRET` set
- **When** the config loads
- **Then** the resolved secret equals the env value

#### Scenario: Non-credential security leaf resolves

- **Given** `[security.keycloak] realm = "${env:KC_REALM:-main}"` with `KC_REALM` unset
- **When** the config loads
- **Then** the realm resolves to `main` and load succeeds (non-credential leaf, declared default)

#### Scenario: Datasource leaves resolve

- **Given** `[datasources.main] db_url = "${env:DB_URL}"` with `DB_URL=postgres://u:p@h/db`, `[datasources.main.extra] password = "${env:SUR_PASS}"` with `SUR_PASS=sur-secret`, `provider = "surrealdb"`, and TLS fields unset
- **When** the config loads
- **Then** `db_url` resolves to `postgres://u:p@h/db`, `extra.password` to `sur-secret`, and the provider/TLS leaves resolve through the same strict gate when placeholders are present

#### Scenario: Authenticator boundary guard rejects marker secrets

- **Given** a `NativeCredentialStore::try_new` call whose plaintext secret contains `{{` or an unconsumed `${env:` marker
- **When** the store constructs
- **Then** construction returns `Err` (defense-in-depth independent of config resolution)

#### Scenario: repository leaves resolve under the strict gate

- **Given** `[idempotent_repo] url = "${env:REDIS_URL}"` and `[cache_repo] sentinel_password = "${env:SENTINEL_PASS}"` with both vars set
- **When** the config loads
- **Then** both resolve to their env values; with `SENTINEL_PASS` unset and no default, load fails closed naming `sentinel_password`; an escaped `$${env:SENTINEL_PASS}` on either leaf is rejected (strict-class full-form rule)
