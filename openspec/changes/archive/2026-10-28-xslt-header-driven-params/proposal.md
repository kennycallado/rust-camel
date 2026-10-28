# Proposal: xslt-header-driven-params

## Why

camel-112 needs `sequenceNumber` and `senderID` as XSLT stylesheet parameters taken from
exchange headers. Today the camel-xslt component only supports URI-static
`param.<name>=<value>` (`XsltEndpointConfig::params`, forwarded unchanged on every
transform). Static values cannot vary per exchange, so per-message identifiers cannot
reach the stylesheet. Apache Camel passes message headers as stylesheet parameters;
this change adds the header-driven form without breaking the static one. (bd rc-oi3wa)

## What Changes

- New endpoint URI option namespace `paramFrom.<name>=<header-key>` on `xslt:` endpoints.
  The value is not a literal: at transform time the producer resolves each `paramFrom`
  entry from the exchange input headers and forwards the result as a stylesheet parameter.
- Documented precedence: a header present for a `paramFrom` entry overrides a static
  `param.<name>` of the same name for that exchange; when the header is absent, the static
  value (if any) is sent; when neither supplies a value, the parameter is omitted.
- Explicit value rules: missing header or `null` header → parameter omitted; string
  forwarded verbatim; number/boolean converted via the canonical `serde_json` Display
  text; arrays/objects fail closed with a payload-free error naming the param and the
  value type (no value content). `camel-api` has no canonical Value-to-string rule, so
  fail-closed is the chosen branch.
- Parser rejects empty `paramFrom` names and empty header keys at endpoint creation.
- Metadata registration for the new prefix option; docs updated
  (`docs/src/components/xml-transform.md`, crate docs).

Explicitly excluded (deferred, bd ticket to follow): expression-driven
`paramExpr.<name>=<expression>`. Non-goals per mission: XML bridge protocol changes,
static param semantics changes, stylesheet caching changes, other components
(`xj:` keeps its own static params), workspace/bridge bump.

## Acceptance criteria

- `paramFrom.<name>=<header-key>` parses in `XsltEndpointConfig::from_uri`; empty name or
  empty header key is rejected; `param.*` behavior is unchanged.
- Dynamic values reach the stylesheet as `<xsl:param>` inputs (verified via the transform
  request captured by the existing `XsltTransformBackend` test seam).
- Missing header omits the parameter; present header overrides same-name static param.
- Concurrent/sequential exchanges with different header values do not leak params.
- `cargo test -p camel-xslt`, fmt, clippy, mission lints, doc build, mdbook test,
  `openspec validate --all --strict` green.

## Risk budget

Contained to the camel-xslt crate plus one docs page. Static-only endpoints behave
byte-identically (no new fields on the transform request). Risk: duplicate URI option
name collisions with `param.` prefix — disjoint prefixes (`param.` vs `paramFrom.`)
cannot collide. Acceptable: last-wins on duplicate identical options in one URI (matches
existing `form_urlencoded` loop behavior).

## Affected crates

- `camel-xslt`: config parsing, endpoint/producer plumbing, metadata, tests.
- `docs` (mdbook): component page update. No spec canon change (no existing requirement
  describes the xslt param surface; the docs carry the contract — mission spec-check).
