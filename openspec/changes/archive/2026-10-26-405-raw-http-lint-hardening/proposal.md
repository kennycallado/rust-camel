# Proposal: 405-raw-http-lint-hardening

## Why

Mission 405 (bd rc-xey4z) hardened the raw-HTTP test-server scanner: matching is
now anchored at every line start (so CRLF/BOM continuations and status-line-shaped
substrings inside larger literals are handled correctly), a missing configured
member root is a hard error instead of a silent skip, and the shared harness gained
`Response::with_status_and_headers` with real migrations to it.

The `no-raw-http-lint` spec still describes the old, unanchored match and omits the
new harness constructor from its allowlist, so the spec understates what the lint
guarantees.

## What Changes

- State the line-start anchoring rule and the hard error for a missing member root.
- Add `Response::with_status_and_headers` to the harness-constructor allowlist.
- No change to what counts as compliant or to the test-scope restriction.
