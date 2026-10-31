# Expression languages

rust-camel evaluates expressions and predicates against Exchange data through pluggable languages. Each language compiles a script into an executable that pipeline steps invoke.

Every language implements the `Language` trait from `camel-language-api`. Languages register into `CamelContext` by name at startup. Pipeline steps resolve them by name to evaluate predicates and expressions.

## Available languages

| Language | Crate | Use case |
|---|---|---|
| [Simple](simple.md) | `camel-language-simple` | Header, body, and property access with `${...}` syntax |
| [JSONPath](jsonpath.md) | `camel-language-jsonpath` | Query JSON bodies with `$.` syntax |
| [XPath](xpath.md) | `camel-language-xpath` | Query XML message bodies |
| [JavaScript](js.md) | `camel-language-js` | Full JS expressions via embedded engine |
| [Rhai](rhai.md) | `camel-language-rhai` | Rust-native embedded scripting |
| [MiniJinja](minijinja.md) | `camel-language-minijinja` | Python Jinja2-inspired templating |

Missing values differ by language: null, empty string, or a hard error. [Value boundaries across languages](value-boundary.md) is the authoritative table for missing-value, conditional, and null-versus-empty behavior.

**Reference**: [Languages overview](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/CONTEXT.md) · [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md)
