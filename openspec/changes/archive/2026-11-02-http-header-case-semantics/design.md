# Design: http-header-case-semantics

## Approach

Keep `Message.headers` as `HashMap<String, Value>` so the public type and all
direct map access remain source-compatible. Canonicalize at the API surface
(modified option B from the pre-flight e_gpt ruling):

- `header(key)` stays exact-case (non-HTTP callers unaffected).
- `header_ic(key)` becomes deterministic: exact match first, then an
  ASCII-case-insensitive match ordered by key, so duplicate spellings inserted
  through the raw map field have a stable winner.
- `set_header(key, v)` removes every case-insensitive duplicate before
  inserting the caller's spelling, so a later `set_header("content-type")`
  replaces `Content-Type` instead of adding a second entry.
- `remove_header(key)` removes every case-insensitive match and returns the
  exact-case value when that entry existed, otherwise `None`.
- `set_header_normalized` deduplicates before inserting the lowercase key.
- Route-step processors (`SetHeader`, `SetHeaderIfAbsent`, `DynamicSetHeader`,
  `DynamicSetHeaderIfAbsent`, `RemoveHeader`) call these methods instead of
  writing the map directly, so route `set_header`/`remove_header` obey the
  same contract. The if-absent presence check is case-insensitive and skips
  expression evaluation when any spelling exists.

The raw `headers` field stays a documented escape hatch: direct writes bypass
normalization and duplicate resolution, so reads at those sites must use
`header_ic`.

HTTP reply content type: at ingress the consumer records the request's
`content-type` value in a Camel-namespaced exchange property,
`CamelHttpRequestContentType`. The reply finalizer resolves the route
content type with `header_ic("Content-Type")`; if that value equals the
recorded inherited value the route did not change it, so it is ignored and
the body inference applies. The same comparison suppresses a `Body::Stream`
metadata content type copied from the request at ingress, so stream inference
cannot reintroduce it. Equality is the provenance boundary (a route setting
exactly the request value is treated as inherited). `select_response_headers`
keeps dropping content-type keys case-insensitively through
`header_policy::excluded_response` and appends exactly one `Content-Type` when
a value resolves, none otherwise.

## Affected crates

- `camel-api`: `Message` helper methods and their tests; documented contract.
- `camel-processor`: route-step `SetHeader`/`DynamicSetHeader`/`RemoveHeader`
  delegate to the helper methods.
- `camel-http`: ingress snapshot property, case-insensitive reply lookup,
  `select_response_headers` regression tests.

## Architecture boundaries

`camel-api` owns the header-map contract (Runtime contract crate).
`camel-processor` implements route steps against that contract (Runtime).
`camel-http` owns wire policy (Components): request/response provenance and
content-type precedence stay in the component, not in `camel-api`. No DSL or
schema change.

## Alternatives considered

- **Replace the field with a `CaseInsensitiveMap` newtype.** Rejected for this
  mission: 172 direct `headers.get` and 73 direct `headers.insert` call sites,
  a public field, and struct-literal construction make this a broad breaking
  migration. Parked as a separate compatibility decision.
- **Document exact-case as the contract and only fix the HTTP finalizer.**
  Rejected: `set_header: content-type` would still drop the key and
  `remove_header` would still miss mixed-case entries.
- **Provenance channel for route-header writes.** Rejected: it pushes
  HTTP-specific bookkeeping into every route step. The identical-value
  boundary (route sets the request's exact content type) is documented
  instead.
