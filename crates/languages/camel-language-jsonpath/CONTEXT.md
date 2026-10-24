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

## Compilation model

Language creation parses each query into a compiled `JpQuery` once and stores
the compiled artifact. Every `evaluate` or `matches` call reuses the stored
`JpQuery` without reparsing. This satisfies the Language SPI compile-once intent.

## Predicate modes

`predicate_mode` selects how a predicate converts its query result to a boolean.
`Strict` (the default) accepts only a JSON boolean and reports any other value
kind as a `type-mismatch` error — no truthiness coercion. `Matches` coerces the
raw match sequence before value-source normalization: zero matches (a missing
path included) are `false`, a sole boolean match keeps its value, and every
other non-empty sequence is `true`. `Matches` is cardinality-only and does not
import XPath `boolean()` value rules: an empty string and `0` stay `true`.
Evaluation failures stay typed errors in every mode. Expression (value-source)
typing is the same in both modes.

## API evolution

ADR-0049 applies `#[non_exhaustive]` to contract enums in the three contract crates.
This leaf implementation crate defines one public enum, `JsonPathPredicateMode`,
which is configuration-only and not a contract enum. `JsonPathConfig` is a public
configuration struct, so the contract-enum policy does not apply to either.

## Authority

- ADR-0032: exchange-data trust boundary
- ADR-0049: workspace `#[non_exhaustive]` policy
