# Endpoint URI Parsing

camel-endpoint parses Camel-style endpoint URIs and maps their values into typed component
configuration. It owns no I/O, route lifecycle, or component SPI. The architectural `Endpoint`,
`Consumer`, and `Producer` terms belong to
[`crates/components/CONTEXT.md`](../components/CONTEXT.md).

This file is a consistency aid and posture record. The crate does not require a local context file
under the `CONTEXT-MAP.md` coverage policy.

## Language

Crate-specific vocabulary. Cross-cutting component terms remain in `crates/components/CONTEXT.md`.

**UriComponents**:
The parsed value of a Camel-style endpoint URI. It contains a scheme, path, and query-parameter
map. `parse_uri()` produces this value.
_Avoid_: Endpoint, URI config

**UriConfig**:
The trait and derive macro that map `UriComponents` into a typed component configuration struct.
The trait lives in this crate. The derive macro lives in `camel-endpoint-macros` and is re-exported
from this crate.
_Avoid_: Endpoint config, URI parser

## URI grammar

The format is `scheme:path[?key=value&key=value]`. This is Camel endpoint URI grammar, not full
RFC 3986 parsing.

- The scheme is the first segment before `:`. It accepts ASCII alphanumeric characters and `-`.
- The path is between the first `:` and the first `?`. The parser percent-decodes it.
- The first `?` separates the path and query. A second `?` remains literal in a query value.
- `#` remains literal in paths and query values. It is not a fragment separator. SQL and other
  component languages use it as a placeholder character.
- `+` remains a plus character. Camel endpoint URIs are not form-encoded.
- `RAW(...)` prevents percent-decoding of a query value.

The second-`?` and `#` rules are the R4-L2 behavior. Callers can still use `%3F` and `%23`, which
decode to the same literal characters.

## Sensitive parameter redaction

The sensitive keys are `password`, `secret`, `token`, `credential`, `apikey`, `accesskey`, and
`privatekey`. Matching is case-insensitive after key percent-decoding.

`UriComponents` has a manual `Debug` implementation. It renders every sensitive value as `"***"`.
The parser stores a sensitive non-`RAW` value literally without percent-decoding. For a sensitive
`RAW(...)` value, it removes the wrapper and stores the inner value. Non-sensitive `RAW(...)`
values retain the wrapper for downstream handling.

## `UriConfig` derive contract

`#[derive(UriConfig)]` accepts these attributes:

| Attribute | Level | Required | Purpose |
|---|---|---|---|
| `#[uri_scheme = "name"]` | struct | Yes | Declares the URI scheme for the config type. |
| `#[uri_config(skip_impl)]` | struct | No | Generates the parsing helper without the trait implementation. |
| `#[uri_param]` | field | No | Maps the field from a query parameter with the same name. |
| `#[uri_param(default = "value")]` | field | No | Supplies a value when the query parameter is absent. |
| `#[uri_param(name = "key")]` | field | No | Maps the field from a different query key. |
| `#[uri_param(pattern = "prefix.")]` | field | No | Declares an open namespace: the field accepts `<prefix>.<name>=<value>` pairs. Valid only on `Vec<(String, String)>` fields. |

The first field without `#[uri_param]` receives the path. `Option<T>` parameters are optional.
Other parameter fields require either a query value or a declared default.

### `pattern` key guardrails

The `#[uri_param(pattern = "<separator>")]` key is valid only on fields of type
`Vec<(String, String)>`. The macro rejects these combinations at compile time:

- `pattern` with `required`, `default`, `secret`, `name`, or `aliases` — an open
  namespace has no single key, value, or alias.
- `pattern` with a non-`string` `kind` — the namespace value type is always string.
- `pattern = ""` — an empty separator would match every key.
- `pattern` whose value does not end with `.` — the trailing `.` is the only
  permitted separator shape in this version; the name derivation algorithm
  (separator with trailing `.` removed → `name`) relies on this precondition.

When `pattern` is present, the generated `UriOption` has `kind = OptionKind::String`,
`name` derived from the separator (trailing `.` stripped), and
`pattern = Some(UriOptionMatch::Prefix { separator })`.

These names are the macro contract: `#[uri_scheme]`, `#[uri_param]`, and `#[uri_config]`.
`#[uri(...)]` is not supported.

## `#[non_exhaustive]` posture

camel-endpoint is outside the binding scope of ADR-0049. It has no public enums. The table records
the crate-local decision for its public parsing surface. It does not extend ADR-0049.

| Type | Posture | Rationale |
|---|---|---|
| `UriComponents` | Stays exhaustive | This small parse-result value is read-mostly and supports struct-literal construction. `#[non_exhaustive]` would block external literals without enough forward-compatibility benefit. Most callers receive it from `parse_uri()`. |
| `UriConfig` | N/A | A trait can add methods with default implementations. |
| Public enums | N/A | This crate has no public enums. |

## Related decisions

- ADR-0049 provides the workspace contract-enum policy. camel-endpoint is outside its scope.
- ADR-0012 is not active in this pure parsing crate because it has no log sites.
