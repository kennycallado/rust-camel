# Simple language

Lightweight implementation of the [Language SPI](../camel-language-api/CONTEXT.md). It reads
headers, body fields, Exchange properties, and exception messages. It also supports interpolation,
comparisons, predicates, and delegation to other registered Languages.

## Compile and evaluation model

`SimpleLanguage::create_expression` and `SimpleLanguage::create_predicate` parse source once into
an internal AST. `SimpleExpression::evaluate` and `SimplePredicate::matches` reuse that AST for
each Exchange. Simple therefore caches its parsed form and is not an FC-LANG-RECOMPILE instance.

**Language delegation**:
`${lang:expr}` resolves `lang` through `ResolverFn` during each evaluation. It then asks that
Language to create and evaluate an Expression. Per-evaluation resolution avoids retaining a stale
Language when the registry changes. The delegated Expression itself is not cached by Simple.
_Avoid_: static language binding, cached delegated expression

## Trust boundary

The normal route path treats Simple source as trusted operator configuration. Exchange bodies,
headers, properties, and errors are untrusted data. The parser keeps source separate from Exchange
data. Delegation passes the configured nested source to the selected Language, whose own trust and
resource limits apply.

## Null and predicate semantics

- Missing headers, properties, exception messages, and JSON body fields evaluate to `Value::Null`.
- A body-field path against a non-JSON body evaluates to `Value::Null`.
- Null interpolation adds an empty string.
- At the `Predicate::matches` boundary, null is false and booleans keep their value. All other
  values, including an empty string, are true.
- Within `&&` and `||`, null and empty strings are false. Ordering and `contains` comparisons with
  null are false.

## Authority

- No crate-specific ADR defines Simple semantics.
- `camel-language-api` owns the `Language`, `Expression`, and `Predicate` contracts.
- ADR-0004 governs the surrounding hot-reload pipeline snapshot semantics.
