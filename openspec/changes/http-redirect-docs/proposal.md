# http-redirect-docs

Docs claim the HTTP producer follows redirects; `followRedirects` defaults to `false`, and a 3xx error names no remedy.

## Why

Trivial change — no spec breakdown needed.

## What changes

- `docs/src/components/http.md`: correct the producer sentence (redirects are not followed by default; `followRedirects=true` enables the SSRF-validated hop loop, with the egress trade-off) and document `followRedirects` (default `false`) and `maxRedirects` (default `10`, `0..=20`) in an options table, cross-linking ADR-0071.
- `camel-component-http`: when `followRedirects=false` and the status is 301/302/303/307/308, append ` (redirect not followed; set followRedirects=true)` to `HttpOperationFailed.status_text`. Display-only; no `CamelError` variant or field changes.
- Tests: hint present on 302, absent on 304, 302 followed with `followRedirects=true`, and URL redaction preserved in the error.
