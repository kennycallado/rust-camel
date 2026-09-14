# Proposal: redactfix

## Why

`redact_url_for_diagnostics` (crates/components/camel-http/src/lib.rs) has a
reachable credential leak in its parse-failure arm. When the internal
`url::Url::parse` fails, the arm returns the raw input (truncated to 256
bytes) with no credential scrub. An unparseable URL that carries userinfo
credentials — e.g. a malformed authority such as `http://u:p@/x` (empty host)
or `http://u:p@host:99999/x` (invalid port) — therefore echoes `user:pass`
into diagnostics output: tracing fields and error messages, including the
`CamelHttpUri` override fence-rejection error on the resolve path (lib.rs),
whose `uri_host_allowed` predicate returns `Ok(false)` for exactly these
unparseable raw header strings (bd
rc-2i5c5, P2, security-adjacent, found by httpsweep 2026-09-12). The prior
fix (bd rc-u4jk6) hardened the parse-success arm only; the failure arm is the
open hole. This violates the fail-closed redaction law: diagnostics must
redact MORE when in doubt, never less.

## What Changes

- camel-component-http: the parse-failure arm of
  `redact_url_for_diagnostics` becomes fail-closed:
  - **Detection-then-suppress:** if the authority window — the substring
    beginning immediately after the first `//` and ending at the next `/`,
    `?`, or `#` searched from that offset — contains `@`, return the
    constant sentinel `[redacted]` — zero raw bytes of the input survive.
  - **Query redaction:** otherwise, if the string contains `?`, drop from
    the first `?` and append `?[redacted]` (mirrors the parse-success arm)
    BEFORE truncation, so no query byte can survive a 256-byte cut; the
    suffix is present whenever it fits within the cap.
  - **Char-boundary truncation:** the 256-byte cap floors to a UTF-8 char
    boundary via a manual loop (unstable `floor_char_boundary` not used) in
    BOTH arms — a panic in a diagnostics path is not fail-closed.
- Parse-success arm behavior is unchanged (rc-u4jk6 masking, query drop,
  existing golden tests).
- Excluded (filed as follow-up bd, deferrals ledger): fragment leaks
  (`#access_token=...`) which survive on BOTH arms — pre-existing, wider
  blast radius, out of this fix's scope.

## Acceptance criteria

- Unparseable URL with credentials in the authority window renders as
  exactly `[redacted]`; no credential byte reaches output.
- Unparseable credential-free strings remain visible, capped at 256 bytes
  on a char boundary (existing `redact_url_truncates_unparseable` survives).
- Unparseable URLs with query strings: no query byte survives, rendered
  `?[redacted]` pre-truncation whenever the suffix fits within the
  256-byte cap.
- `@` outside the authority window (path-`@`, `mailto:`) is NOT suppressed.
- Fence-rejection error text (CamelHttpUri override fence contract) never
  contains credential bytes from an unparseable override URL.
- bd: rc-2i5c5.

## Risk budget

- Accepted: log scrapers lose visibility into any `@`-bearing unparseable
  URL (suppression, not surgical scrub) — leak cost outweighs utility.
- Accepted: minor behavioral change of Err-arm output for query-bearing
  unparseable strings (now `?[redacted]`).
- Out of bounds: any change to the parse-success arm's masking semantics,
  any crate other than camel-component-http (camel-dsl and camel-cli are
  BUSY — fleet zones), no new dependencies.
