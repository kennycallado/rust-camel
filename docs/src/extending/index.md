# Extending rust-camel

rust-camel exposes four extension points. Each one has a Rust trait or WIT contract and registers into `CamelContext` at startup.

| Extension | Contract | Registers |
|---|---|---|
| [Custom component](custom-component.md) | `ComponentBundle` in `camel-component-api` | URI scheme; config under `[components.<key>]` |
| [Expression language](../languages/index.md) | `Language` in `camel-language-api` | Name; six built-in (Simple, JSONPath, XPath, JS, Rhai, MiniJinja) |
| [Data format](../data-formats/index.md) | `DataFormat` in `camel-api` | Name; JSON, CSV, XML, Protobuf built-in |
| WASM plugin | `camel:plugin` WIT in [`camel-wit`](https://github.com/kennycallado/rust-camel/tree/main/crates/camel-wit) | Guest component, bean, or source via `camel-component-wasm` |

Start with the [custom component walkthrough](custom-component.md).
