# Proposal: 488-language-value-integrity

## Why

Three sanctioned follow-ups from the language-value-boundary audit remain open
(bd `rc-u8t5c`, `rc-zwy7e`, `rc-xnpg7`, all P2). They are one class of defect:
a script can observe, lose, or silently reshape exchange data at the
language boundary.

- `rc-u8t5c`: the spec requires `has_header`/`has_property` and a consistent
  reader surface, but no crate registers them. `rc-w1mds` recorded this
  spec-code drift (closed as a duplicate of `rc-u8t5c`). The JSONPath/XPath
  body-only question from the audit matrix section 4 is still undecided in the
  spec.
- `rc-zwy7e`: Simple stringifies `Body::Json`, lossily decodes `Bytes`, and
  silently maps `Stream` to null. JS rewrites header/property maps wholesale on
  every mutating step (so untouched `1.0` values drift to `1`) and maps `Bytes`
  to null.
- `rc-xnpg7`: streaming-body refusal differs four ways. Rhai is access-aware;
  Simple yields null; JS and MiniJinja refuse up front, so a header-only filter
  fails on a stream.

## What Changes

- Expose `has_header`/`has_property` where headers and properties exist
  (Simple, Rhai, JS, MiniJinja) on each language's native view (Rhai
  snapshot-backed, JavaScript live) and absent-vs-null aware.
  Record JSONPath/XPath as body-only (no key surfaces) in the spec.
- Simple body integrity: native JSON, `Bytes` as a JSON integer array,
  `Stream` refused on read only.
- JS value integrity: changed-only, transactional header/property write-back
  with JS-normalized comparison; `Bytes` as a native integer array; key-presence
  readers.
- Access-aware streaming refusal in Simple, Rhai, JS, MiniJinja: a header-only
  expression succeeds on a streaming body; reading `body` refuses.
- Update the `language-value-boundary` spec and user docs.

Explicitly excluded: new languages, expression-syntax changes, streaming EIP
redesign, observability metrics, new crates, and the JSONPath/XPath key-surface
addition (body-only decision).

## Acceptance criteria

- `has_header`/`has_property` distinguish absent from stored null per language;
  JSONPath/XPath are documented body-only.
- Simple and JS expose JSON natively, `Bytes` as an integer array, and never
  lose or stringify values silently.
- JS mutating steps write back only changed map entries and commit nothing on a
  conversion failure.
- A header-only expression succeeds on `Body::Stream` in Simple, Rhai, JS, and
  MiniJinja; reading `body` refuses.
- Spec, docs, and tests agree; all quality gates pass.

## Risk budget

Acceptable: new additive reader functions; changed Simple `${body}`/`Bytes`
representation (intentional, documented); JS lazy `camel.body` accessor in the
internal engine traits. Out of bounds: changing expression syntax, silently
coercing values, breaking redaction or error classes tracked by `rc-fukc6`, or
touching crates outside the language layer and `camel-api` docs.
