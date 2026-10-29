# Design: 449-simple-exception

## Decision

`${exception}` renders `<variant_name>: <Display>` — the error kind followed by
its `Display` text — and evaluates to `Value::Null` when the exchange carries no
error.

Examples:

```text
${exception} -> HttpOperationFailed: HTTP GET http://svc/api failed: 500 Internal Server Error
${exception} -> Value::Null   (no error on the exchange)
```

`${exception.message}` is unchanged: it still renders the `Display` text alone.

## Rationale

**Payload-safe by construction.** The kind prefix comes from
`CamelError::variant_name()`, a static string with no exchange or transport data.
The `Display` half is the same thiserror `Display` that `${exception.message}`
already exposes; that contract deliberately excludes
`HttpOperationFailed.response_body`. Concatenating the two therefore adds no data
that the existing lookup did not already surface.

**Stable kind token.** The variant name gives a catch body a stable, parseable
token to branch on (`HttpOperationFailed`, `TypeConversionFailed`,
`ExpressionFailed`, …) without parsing free-form message text. That is exactly what
`camel-astro` needs and what `${exception.message}` alone cannot provide.

**Debug output is forbidden.** `Debug` formatting prints struct fields, which
include response bodies and other payloads. Using it would leak exchange data
into the rendered value, so the rendering uses `variant_name()` plus `Display`
only.

## Rejected alternatives

- **Alias of `${exception.message}`** (make `${exception}` render only the
  `Display` text): redundant with the existing lookup and loses the stable kind
  token, so it adds surface without adding capability.
- **Full `Debug` form** (`{error:?}`): leaks payloads such as
  `HttpOperationFailed.response_body` and is not a stable consumer contract.

## Boundaries

The change is confined to the Simple lookup surface and its docs. `doTry` catch
semantics, the error taxonomy, and exception serialization are untouched.
