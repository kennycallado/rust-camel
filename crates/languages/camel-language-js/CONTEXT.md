# JavaScript language

Boa-backed JavaScript implementation of the Language SPI. It implements
`Expression`, `Predicate`, and `MutatingExpression` for the synchronous
`script:` path defined by ADR-0006.

## Trust model

- Script source is trusted operator configuration.
- Exchange bodies, headers, and properties are untrusted data under ADR-0032.
- The implementation converts exchange data to JavaScript values. It does not
  concatenate exchange data into script source or evaluate exchange data as code.
- Untrusted JavaScript must run through the out-of-process `function:` path from
  ADR-0005.

## Sandbox posture

Each evaluation creates a fresh Boa `Context`. This prevents state from leaking
between evaluations. Boa receives no filesystem, network, environment, stdio, or
WASI capability. The only host bindings are the exchange snapshot under `camel`
and a tracing-backed `console`.

All host-created JavaScript objects in `bindings.rs` use
`JsObject::with_null_proto()`. This includes `camel`, `console`, map wrappers,
and their backing data objects. The null prototypes prevent inherited prototype
members from affecting host-bound exchange data.

## `camel` global

| Member | Behavior |
|---|---|
| `camel.headers` | Map-like exchange header view with `get`, `set`, `has`, `remove`, and `keys` |
| `camel.properties` | Map-like exchange property view with the same operations |
| `camel.body` | Exchange body value; writable in a `MutatingExpression` |
| `camel.property(name)` | Reads an exchange property |
| `camel.set_property(name, value)` | Writes an exchange property |

Read-only expressions and predicates evaluate a snapshot and discard changes.
A `MutatingExpression` writes body, header, and property changes back only after
successful evaluation. Failure leaves the Exchange unchanged.

## Resource limits

`[languages.js.limits]` configures four limits. `None` selects the rust-camel
runtime default.

| Limit | Default | Enforcement |
|---|---:|---|
| `execution-timeout-ms` | 5,000 ms | `tokio::time::timeout` around `spawn_blocking` |
| `max-loop-iterations` | 100,000 | Boa runtime limit |
| `max-recursion-depth` | 512 | Boa runtime limit |
| `max-stack-size` | 10,240 slots | Boa runtime limit |
| Source size | 1 MiB | Fixed pre-evaluation byte bound |

Value conversion also rejects nesting deeper than 128 levels. The timeout
returns control to the route, but it cannot cancel a running blocking task. That
task continues until a Boa limit trips or the script finishes.

Boa 0.21 has no heap-size limit. The source-size bound does not prevent a small
script from allocating a large object graph. Route authors must not run
untrusted JavaScript in-process.

## Boa boundary

Direct Boa use is confined to three implementation files:

- `src/engines/boa.rs`
- `src/bindings.rs`
- `src/value.rs`

The public API exposes rust-camel types such as `JsEngine`, `JsExchange`, and
`JsEvalResult`. No public signature exposes a Boa type. A future engine can
implement `JsEngine` without changing Language SPI consumers.

## API evolution

ADR-0049 applies `#[non_exhaustive]` by default to contract enums in
`camel-language-api`, not to enums in leaf implementation crates. Therefore
`JsLanguageError` is intentionally outside that policy. Any future promotion of
this error enum into the shared Language SPI requires a separate contract review.

## Authority

- ADR-0005: out-of-process `function:` execution
- ADR-0006: synchronous `script:` execution
- ADR-0032: exchange-data trust boundary
- ADR-0049: workspace `#[non_exhaustive]` policy
