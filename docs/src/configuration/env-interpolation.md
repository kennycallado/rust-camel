# Environment variable interpolation

Substitute environment variables into route files and `Camel.toml` with
`${env:VAR}` tokens. The tokens work in endpoint URIs, log messages,
header values, and any other string field.

The expansion point differs by surface. Route files expand in the raw
route source, before YAML parsing. `Camel.toml` expands every string
leaf of the parsed, merged tree. The tree combines the main file,
include files, and `CAMEL_*` environment overrides. Expansion runs
before typed deserialization.

## Syntax

```yaml
{{#include ../../../examples/env-interpolation/routes/routes.yaml:env-routes}}
```

`${env:VAR}` reads the variable `VAR`. `${env:VAR:-default}` uses
`default` when `VAR` is unset. An unset variable with no default fails
load. The error names the variable. Set a default or export the variable
to avoid the failure.

The same syntax works in `Camel.toml`:

```toml
[security.keycloak]
client_secret = "${env:KC_SECRET}"

[observability.otel]
endpoint = "${env:OTEL_ENDPOINT:-http://localhost:4317}"
```

## Escapes

| Input | Output |
|-------|--------|
| `${env:VAR}` | Value of `VAR`; fails closed if unset and no default |
| `${env:VAR:-default}` | Value of `VAR`, or `default` when unset |
| `$$` | A single `$` |
| `$${env:VAR}` | The literal text `${env:VAR}` |

The standalone `$$` escape works on every surface: route files and all
`Camel.toml` leaves. The full-form escape `$${env:VAR}` yields the
literal placeholder text on route files and plain `Camel.toml` leaves.

**Exception — credential leaves:** `Camel.toml` sections that hold
credentials or connection secrets reject the escaped full form. On
`security`, `datasources`, `idempotent_repo`, and `cache_repo` leaves, a
`$${env:VAR}` leaves a residual `${env:VAR}` marker, which fails load.
There is no legitimate reason for a credential field to hold the literal
text of a placeholder.

## Fail-closed

Both surfaces fail closed. An unset variable without a default aborts:

- Route discovery fails with an error naming the variable.
- `Camel.toml` load aborts with an error naming the field.

Use `${env:VAR:-default}` for optional values.

## Legacy `{{...}}` syntax

`Camel.toml` rejects the legacy `{{...}}` placeholder syntax. Any `{{` in
a string leaf fails load with an actionable message: placeholders use
`${env:NAME}` or `${env:NAME:-default}`. Route files never supported the
`{{...}}` form.

## How it works

The DSL loader (`camel_dsl::interpolate_env`) scans raw route source
before YAML parsing. `camel-config` walks the merged `Camel.toml` tree
after the builder merges the main file, include files, and `CAMEL_*`
environment overrides (`resolve_tree_placeholders`). The walk replaces
`${env:...}` patterns before typed deserialization. Substituted values
pass through `sanitize_env_value`, which strips control characters and
newlines. This blocks newline injection from a hostile or malformed
variable.

The `PropertiesResolver` type in `camel-config` retains the legacy
`{{...}}` API for compatibility. `Camel.toml` loading does not use it.

## Setup

```rust,ignore
{{#include ../../../examples/env-interpolation/src/main.rs:env-interpolation}}
```

The `Camel.toml` for this example is minimal. Route discovery and
component registration follow the standard pattern.

```toml
{{#include ../../../examples/env-interpolation/Camel.toml:env-config}}
```

## When to use

- **Twelve-factor apps**: inject configuration that varies per deploy
  through the environment, not through files in source control.
- **Secrets**: pass credentials and tokens from the environment. The
  route file never stores the secret value.
- **Per-environment endpoints**: point routes at different brokers, HTTP
  hosts, or databases without editing route files.

**Reference**: `PropertiesResolver` in the [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
