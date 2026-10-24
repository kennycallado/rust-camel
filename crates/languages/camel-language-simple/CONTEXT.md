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

## Expression and predicate modes

`SimpleLanguage::create_expression` parses in TEMPLATE mode. `SimpleLanguage::create_predicate`
parses the comparison grammar. Predicate mode is unchanged by template mode.

**Simple template mode**:
`create_expression` source is literal text plus `${...}` interpolation slots. Text outside a slot
stays verbatim: `=`, `<`, `null`, `true`, quotes, and whitespace are not special. Exactly one whole
`${...}` source returns the existing slot value (native number, array, or null from headers or
body fields). A whole `${body}` slot renders a JSON body as serialized text. Any other source
returns a string. A source with no slot is a raw string, so `'abc'`, `true`, `42`, and `null` stay
text. A `${` that is never closed is always a compile-time parse error — never literal text. The
error names the byte offset of the `${` in the trimmed source. An empty `${}` and a nested `${`
inside slot content are parse errors too. Template mode is Simple's own grammar, NOT the MiniJinja
template engine (ADR-0047) and NOT route-template `{{...}}` JSON-tree substitution (ADR-0008).
_Avoid_: MiniJinja, Jinja, route templates, `{{...}}`

**Simple predicate mode**:
`create_predicate` source is the comparison grammar (`==`, `!=`, `>`, `contains`, `&&`, `||`).
Quoted strings protect `${`; an unclosed `${` outside quotes is a compile-time parse error, same
as template mode. `SimplePredicate::matches` requires a real `Value::Bool`. Any other value
(including `null`, an empty string, or the string `"true"`) returns `LanguageError::TypeMismatch`.
There is no truthiness coercion.
_Avoid_: truthiness, `as_bool` coercion, non-boolean predicate

## Null and comparison semantics

- Missing headers, properties, exception messages, and JSON body fields evaluate to `Value::Null`.
- A body-field path against a non-JSON body evaluates to `Value::Null`.
- Null interpolation adds an empty string. A whole-slot missing value is `Value::Null`.
- `&&` and `||` use Simple's own truthiness rule for their operands: null and empty strings are
  false, booleans keep their value, and everything else is true. This rule does not apply to the
  final predicate result, which must be a real boolean.
- `==` compares by value and coerces a string to a number (and the reverse) by parsing the string
  as a number. It does not coerce between a string and a boolean, so `${header.approved} == true`
  is false when the header is the string `"true"`. Compare against `'true'` or store a boolean
  header.
- Ordering and `contains` comparisons with null are false.

## Authority

- No crate-specific ADR defines Simple semantics.
- `camel-language-api` owns the `Language`, `Expression`, and `Predicate` contracts.
- ADR-0004 governs the surrounding hot-reload pipeline snapshot semantics.
