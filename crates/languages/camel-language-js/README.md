# camel-language-js

Boa-backed JavaScript implementation of the rust-camel Language SPI. It
provides expressions, predicates, and mutating expressions for the synchronous
`script:` path defined by ADR-0006.

## Script API

Scripts receive exchange data through the `camel` global:

| Member | Behavior |
|---|---|
| `camel.headers` | Map-like header view with `get`, `set`, `has`, `remove`, and `keys` |
| `camel.properties` | Map-like property view with the same operations |
| `camel.body` | Reads the exchange body and supports writes in mutating expressions |
| `camel.property(name)` | Reads an exchange property |
| `camel.set_property(name, value)` | Writes an exchange property |

Exchange data crosses the JavaScript boundary as values, not executable code.
Host-binding container objects have null prototypes. This prevents inherited
prototype members from affecting bound exchange data.

## Resource limits

Each evaluation uses a fresh Boa context and these defaults:

| Limit | Default |
|---|---:|
| Execution timeout | 5,000 ms |
| Loop iterations | 100,000 |
| Recursion depth | 512 |
| Stack size | 10,240 slots |
| Source size | 1 MiB |

Boa 0.21 has no heap-size limit. A timeout also cannot cancel an already running
blocking task. Do not run untrusted JavaScript in-process. Use the out-of-process
`function:` path from ADR-0005 instead.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-language-js = { workspace = true }
```

See the crate-level Rust documentation for API details. See
[`CONTEXT.md`](./CONTEXT.md) for the sandbox, trust model, resource limits, and
Boa dependency boundary.
