# JSONPath language

RFC 9535 JSONPath implementation of the Language SPI over `jsonpath-rust`. It
evaluates expressions and predicates against an Exchange JSON body.

## Trust model

- The JSONPath query is trusted operator configuration. The Language validates its
  `$` prefix and syntax when it creates an Expression or Predicate.
- The JSON body is untrusted, adversary-controlled data under ADR-0032. It is only
  the data target for the trusted query.

Exchange data never enters the query string. The implementation stores the operator's
query and passes body content separately to `jsonpath-rust`. A malicious body therefore
has no JSONPath injection path. The library boundary registers no file, network, or
arbitrary-code execution facility.

## Resource bounds

`max_input_bytes` bounds a text body before JSON parsing. The default is 16 MiB.
Setting it to `None` removes this bound and is an explicit operator choice. The byte
bound does not apply to an already parsed `Body::Json`, because that allocation occurred
upstream.

`max_depth` bounds JSON nesting for text, coerced, and already parsed JSON bodies. The
default is 64 levels. Both bounds are replacement requirements for any future JSONPath
library.

Evaluation has no wall-clock timeout. Expensive recursive-descent queries remain an
operator responsibility because the query is trusted configuration. The input bounds
still constrain untrusted body structure on the paths described above.

## Known limitation: query recompilation

Language creation parses each query to validate it, then stores only the query string.
Every `evaluate` or `matches` call asks `jsonpath-rust` to parse that string again. This
violates the Language SPI compile-once intent and adds per-Exchange work. bd `rc-flny`
tracks the workspace FC-LANG-RECOMPILE epic. A fix must preserve the trust separation
and resource bounds in this document.

## API evolution

ADR-0049 applies `#[non_exhaustive]` to contract enums in the three contract crates.
This leaf implementation crate defines no public enum. `JsonPathConfig` is a public
configuration struct, so the contract-enum policy does not apply to it.

## Authority

- ADR-0032: exchange-data trust boundary
- ADR-0049: workspace `#[non_exhaustive]` policy
