# Environment variable interpolation

Substitute environment variables into YAML route files with `${env:VAR}`
tokens. The tokens expand before YAML parsing, so they work in endpoint
URIs, log messages, header values, and any other string field.

## Syntax

```yaml
{{#include ../../../examples/env-interpolation/routes/routes.yaml:env-routes}}
```

`${env:VAR}` reads the variable `VAR`. `${env:VAR:-default}` uses
`default` when `VAR` is unset. An unset variable with no default fails
route discovery. The error names the variable. Set a default or export
the variable to avoid the failure.

## How it works

The DSL loader (`camel_dsl::interpolate_env`) scans route source for
`${env:...}` patterns and replaces them before YAML parsing. Substituted
values pass through `sanitize_env_value`, which strips control characters
and newlines. This blocks newline injection from a hostile or malformed
variable.

The `PropertiesResolver` type in `camel-config` exposes the same
resolution as a public API for config-value placeholders.

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
