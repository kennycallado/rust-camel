# dead-config-policy spec delta

## ADDED Requirements

### Requirement: Config placeholder resolution is exhaustive over all string leaves

Camel.toml placeholder resolution MUST walk every string leaf of the configuration — typed
fields and untyped component/bean value maps — with no per-field allowlist. A newly added
config section's string leaves MUST resolve through the existing walk without any resolver
code change. Resolution MUST be prefix-gated on `${env:` so component-owned expressions
(`${body}`, `${file:...}`, `${1}`) pass through untouched.

#### Scenario: new section resolves without code change

- **Given** a Camel.toml carrying a synthetic `[future_section] value = "${env:SOME_VAR}"` — a section whose fields appear nowhere in the resolver or the typed config struct
- **When** the config loads with `SOME_VAR` set
- **Then** the leaf resolves to the env value with zero resolver code changes required (asserted at the raw-tree stage or via the struct's extra-fields capture)

#### Scenario: component expressions pass through

- **Given** a `[components.*]` value containing `${body}` or `${file:name}` (no `env:` prefix)
- **When** the config loads
- **Then** the value is unchanged — no resolution attempt, no error

### Requirement: Single interpolation syntax across routes and config

Camel.toml MUST use the same placeholder syntax and resolver semantics as route files:
`${env:NAME}` and `${env:NAME:-default}` resolved by the shared `interpolate_env` engine.
Legacy `{{...}}` placeholders in Camel.toml MUST be rejected at load with an actionable error
naming the field and the `${env:}` replacement — they MUST never resolve, warn, or pass
through silently. The STANDALONE `$$` escape MUST produce `$` on ALL string leaves (routes,
non-security config, security, datasource). The full escaped form `$${env:...}` MUST produce
the literal text `${env:...}` on the route surface and on non-security config leaves;
strict-gate leaves (security, datasources, idempotent_repo, cache_repo) reject that form via the
residual-marker gate — credentials have
no legitimate literal-placeholder content.

#### Scenario: legacy braces rejected with guidance

- **Given** any Camel.toml string leaf containing `{{env:FOO}}`
- **When** the config loads
- **Then** `load_config` returns `Err` whose message names the field and states the `${env:NAME}` / `${env:NAME:-default}` replacement forms

#### Scenario: escape yields literal on successful surfaces

- **Given** a Camel.toml non-security value `$${env:FOO}` and a route value `$${env:FOO}`
- **When** each loads through its pipeline
- **Then** both yield the literal text `${env:FOO}`

#### Scenario: standalone dollar escape on every leaf class

- **Given** values `a$$b` on a route, a non-security config leaf, a security leaf (`[security.keycloak] realm = "a$$b"`), a datasource leaf, and repo-section leaves (`[idempotent_repo] backend`, `[cache_repo] backend`)
- **When** each loads through its pipeline
- **Then** all values yield `a$b` — the standalone escape leaves no prohibited marker

#### Scenario: escaped placeholder rejected on security leaves

- **Given** `[security.native] bearer_token = "$${env:FOO}"` (escaped form on a credential leaf)
- **When** the config loads
- **Then** `load_config` returns `Err` via the residual-marker rejection — the literal placeholder text never reaches a credential store

#### Scenario: route interpolation unchanged

- **Given** a route file with `to: "log://${env:ROUTE_VAR}"` and `ROUTE_VAR` set
- **When** the route loads
- **Then** the endpoint resolves to the env value with semantics identical to before this change

### Requirement: Uniform fail-closed on missing environment variables

A `${env:NAME}` placeholder with the variable unset and no `:-default` MUST abort config load
with `ConfigError` naming the field — on every string leaf, security or not. Optional values
MUST declare `:-default`. A declared default MUST be used when the variable is unset.

#### Scenario: optional endpoint without default aborts

- **Given** `[observability.otel] endpoint = "${env:OTEL_EP}"` with `OTEL_EP` unset and no default
- **When** the config loads
- **Then** `load_config` returns `Err` naming `observability.otel.endpoint`

#### Scenario: default declared is used

- **Given** `[observability.otel] endpoint = "${env:OTEL_EP:-http://localhost:4317}"` with `OTEL_EP` unset
- **When** the config loads
- **Then** the endpoint resolves to `http://localhost:4317` and load succeeds

#### Scenario: security credential fails closed under new syntax

- **Given** `[security.native] bearer_token = "${env:APP_TOKEN}"` with `APP_TOKEN` unset
- **When** the config loads
- **Then** `load_config` returns `Err` — the literal placeholder never becomes a credential

#### Scenario: CLI surfaces load errors instead of silent defaults

- **Given** a `Camel.toml` that fails to load (parse error, broken include, or unresolved `${env:...}` without default)
- **When** `camel run` starts with that file
- **Then** the command aborts with an error naming the file and cause — it never boots on empty-config defaults silently
