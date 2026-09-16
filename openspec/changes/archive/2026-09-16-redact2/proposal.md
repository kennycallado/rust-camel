# Proposal: redact2

## Why

Mission `redactleaks` (bd rc-zf06t, rc-y6101, rc-eh49, landed 6f904f09)
converged the URL-redaction semantics of camel-config, camel-jms, and
camel-http, but the implementation stayed as three in-crate copies of the
same string surgery. Two credential leaks remain open against that landed
surface:

- rc-r7v8s (P2 bug): the camel-jms broker URL Debug keeps benign query keys
  by documented exception. A benign-keyed pair whose value embeds a
  percent-encoded URL with credentials (`?redirect=http%3A%2F%2Fuser%3Asecret%40host`)
  survives verbatim — the encoded form contains no literal `//` or `@`
  window, so the window mask never fires.
- rc-f05q8 (P4 residual, reopen trigger fired): backslash authority
  separators on non-special schemes (`foo:\\user:pass@evil/`) contain no
  `//` run, so they reach the failure arm unscanned and render raw
  userinfo. The url crate normalizes backslashes only for special schemes,
  so the camel-http parsed arm does not cover non-special input.

rc-924sb (P3, structural half of rc-eh49): the converged helpers need one
canonical home that all zone crates can import. camel-api is the only such
crate (all three consumers already depend on it).

## What Changes

- New std-only module `camel-api::redact`: `redact_url` (strict:
  every-authority-window mask, compose-both `?[redacted]`/`#[redacted]`
  sentinels, 256-byte sentinel-safe cap), `redact_url_fail_closed`
  (wholesale `[redacted]` when any window carries `@`), and
  `redact_url_with_query_allowlist` (broker per-key semantics with a
  minimal-decode rule that masks credential-shaped benign values, closing
  rc-r7v8s), plus `window_has_at_sign` for the camel-http parsed-arm
  guard. Backslash-bearing authority runs open windows when an RFC 3986
  scheme prefix precedes them, closing rc-f05q8.
- Call-site migration only, no behavior drift outside the two leak fixes:
  camel-config `redact_url`, camel-jms `redact_url` / `redact_broker_url`
  delegate wholesale (broker keeps its ActiveMQ key list exactly);
  camel-http keeps its url-crate parsed arm, composed over the canonical
  `redact_url`, and delegates the failure arm.
- Landed helper-behavior tests migrate to the canonical module; each
  consumer keeps thin delegation pins and its Debug-surface tests.

## Acceptance Criteria

- One canonical redactor; the three in-crate string-surgery copies are
  deleted.
- Broker allowlist semantics unchanged for normal params; the
  percent-encoded credential example, a lowercase variant, and a
  constructed literal-`@` bypass all mask; a lone-`%40` email value stays
  visible (documented stance).
- Backslash authority separators mask on scheme-prefixed runs, including
  the single-backslash `http:`-class case; Windows-drive and UNC-shaped
  inputs without scheme-prefixed runs stay visible.
- All migrated tests pass; `cargo doc -p camel-api` under `-D warnings`
  passes; no new dependency edges (camel-api gains none).

## Affected crates / bd

crates/camel-api (new module), crates/camel-config, crates/components/camel-jms,
crates/components/camel-http (call sites + test migration).
Bd: rc-924sb rc-r7v8s rc-f05q8 (parent rc-eh49 closed partial-by-design).
