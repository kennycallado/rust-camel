# Rhai language

Rhai implementation of the Language SPI. It implements `Expression`,
`Predicate`, and `MutatingExpression` with an unconditional in-process sandbox.

## Trust model

Rhai source is trusted operator configuration. Exchange bodies, headers, and
properties are untrusted data under ADR-0032. The implementation binds exchange
data as Rhai values and never evaluates those values as source code.

## Sandbox posture

The crate closes filesystem, module, and network access through independent
layers:

- The workspace enables Rhai's `no_module` feature.
- Each evaluation uses `Engine::new_raw()` instead of `Engine::new()`.
- `StandardPackage` adds the standard language operations without installing a
  `FileModuleResolver`.
- `disable_symbol("eval")` and `disable_symbol("import")` provide defense in
  depth against later package changes.

The sandbox has no configuration opt-out. Timing functions from
`StandardPackage` remain available. Resource limits, not the sandbox boundary,
bound their use.

## Resource limits

`[languages.rhai.limits]` configures seven limits. `None` selects the rust-camel
runtime default.

| Limit | Default |
|---|---:|
| `max-operations` | 100,000 |
| `max-string-size` | 1 MiB |
| `max-array-size` | 10,000 elements |
| `max-map-size` | 10,000 entries |
| `max-expression-depth` | 64 |
| `max-function-expression-depth` | 32 |
| `execution-timeout-ms` | 5,000 ms |

The timeout wraps synchronous evaluation in `spawn_blocking`. It returns control
to the route after five seconds by default, but it does not cancel the blocking
task. The operation limit eventually stops a CPU-bound task.

`max_call_levels` is exposed in `RhaiLimitsConfig` (default 64). This pins a
single value and removes the upstream `Engine::new_raw()` asymmetry (8 levels
in debug, 64 in release) — rc-dip6.

## Mutation model

Read-only expressions and predicates expose `body` and `headers` variables plus
the `header()`, `set_header()`, `property()`, and `set_property()` host
functions. Their writes affect only the current evaluation.

A `MutatingExpression` exposes `body`, `headers`, and `properties` as mutable
scope variables. The implementation writes all three back only after successful
evaluation. An error leaves the Exchange unchanged.

## Rhai boundary

All direct Rhai use is confined to `src/lib.rs`. Public constructors accept
`RhaiLimitsConfig` from `camel-language-api`, and Language factory methods return
SPI trait objects. No public signature exposes a Rhai type.

## Authority

- ADR-0012: handler-owned log levels
- ADR-0032: exchange-data trust boundary
- ADR-0033: security defaults
- ADR-0051: credential redaction at diagnostic boundaries
- bd rc-dip6: expose the Rhai call-level limit
