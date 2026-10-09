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

## JSON host module

A private `json` module holds a process-wide host `rhai::Module`. Every sandbox
engine registers it globally after `StandardPackage`, so `parse_json` and
`to_json` shadow the stock Rhai built-ins on the read-only, mutating, and
expression engines. The helpers read `max-string-size`, `max-array-size`, and
`max-map-size` from the calling engine.

`parse_json` is strict RFC 8259 and stores exact number tokens. Integer-form
tokens that fit `i64` project to a native `INT`; decimals, exponents, and
out-of-`i64` integers are the `json number` wrapper. Objects keep insertion order
in an `IndexMap`. The wrapper has an `Arc` copy-on-write tree, a depth cap of
128, and no iteration.

`to_json`, `to_string`, and `to_debug` emit compact JSON in stored order with raw
UTF-8 and no `/` escape. Native `Map` keys are sorted.

The outbound converter `dynamic_to_value` refuses `JsonValue` and `JsonNumber`
with the stable labels `json value` and `json number`. Direct outbound wrapper
conversion is deferred, so persisting parsed JSON uses `to_json` or a native
leaf. The inbound converter and `make_scope`/`prepare_scope` are unchanged.

Authority: bd rc-qka42. `rc-m01r9` (eager scope conversion) and `rc-141om`
(inbound u64 refusal) remain out of scope.

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
the `header()` and `property()` readers. `set_header()` and `set_property()` are
rejected at create time (the parse error names a `script:` step as the
alternative); the read-only engine never registers them.

A `MutatingExpression` exposes `body`, `headers`, and `properties` as mutable
scope variables. Its write-back is a validate-all-then-commit transaction: the
post-eval scope is compared with the pre-eval snapshot, only changed entries and
an assigned body are converted, and the Exchange is untouched unless the whole
evaluation succeeds. An error leaves the Exchange unchanged.

## Rhai boundary

Direct Rhai use is confined to `src/lib.rs` and its private `converter`,
`stream_body`, and `transaction` modules. Public constructors accept
`RhaiLimitsConfig` from `camel-language-api`, and Language factory methods return
SPI trait objects. No public signature exposes a Rhai type.

## Authority

- ADR-0012: handler-owned log levels
- ADR-0032: exchange-data trust boundary
- ADR-0033: security defaults
- ADR-0051: credential redaction at diagnostic boundaries
- bd rc-dip6: expose the Rhai call-level limit
- bd rc-qka42: strict bounded Rhai JSON host helpers
