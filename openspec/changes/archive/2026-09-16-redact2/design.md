# Design: redact2

## Approach

One new std-only module, `crates/camel-api/src/redact.rs`, holds the
converged string surgery as four public items (pre-flight e_glm ruling,
2026-09-16; audit: docs/audits/redaction-surface-2026-09.md):

- `redact_url(raw: &str) -> String` — strict: enumerate authority windows,
  mask userinfo through the last `@` per window (`***`, reverse-offset
  edits, dedup), truncate at the earliest `?`/`#`, append one
  `?[redacted]`/`#[redacted]` sentinel per distinct introducer in
  first-occurrence order, 256-byte cap with the sentinel budget reserved
  before truncation (UTF-8-char-boundary cut).
- `redact_url_fail_closed(raw) -> String` — `[redacted]` wholesale when
  any authority window carries `@`; otherwise `redact_url(raw)`.
- `redact_url_with_query_allowlist(raw, sensitive_key_substrings)` —
  broker semantics: window mask, per-key query redaction (`k=<redacted>`
  when the lowercased key contains any sensitive substring; benign pairs
  kept), fragment dropped with `#[redacted]`, same cap. The parameter is
  a substring denylist; the behavior keeps benign keys by default.
- `window_has_at_sign(raw) -> bool` — same window enumeration as the
  mask; consumed by the camel-http parsed-arm fail-closed guard.

`truncate_utf8_safe` and the window/mask internals stay private.

Window enumeration v2 (closes rc-f05q8): enumerate maximal runs of `/`
and `\` characters; a window runs from the run's end to the next `/`,
`?`, or `#`. Pure-slash runs open a window at length >= 2 (landed
behavior). A backslash-bearing run opens only when an RFC 3986 scheme
prefix (`[a-zA-Z][a-zA-Z0-9+.-]*:`, first char alpha) sits immediately
before the run: length >= 2 needs any prefix (one character or more);
a single-`\` run needs a prefix of at least two characters, or a
one-character prefix when the candidate window content carries `:`
before its last `@` (credential-shaped — `x:\user:pass@evil` windows,
`C:\Users\x@corp\file` does not). Windows-drive paths and UNC paths
(`\\server\x@y`: no scheme) never window. The rule applies uniformly to
the mask, `window_has_at_sign`, and all three variants; no WHATWG
special-scheme list is needed because the string crates never parse —
the url-crate parsed arm stays authoritative for special schemes.

Encoded-credential rule (closes rc-r7v8s), applied inside the allowlist
variant only: a benign-keyed pair whose single-pass minimal decode
(`%40`->`@`, `%3a`->`:`, `%2f`->`/`, case-insensitive, applied to the
whole pair) contains `@` AND (`:` OR `//`) renders as `<redacted>`.
This subsumes the bd's signature rule, catches literal-`@`/literal-`:`
bypasses the signature rule misses, and keeps the lone-`%40` email value
(`contact=admin%40corp.example`: no `:`, no `//`) visible. Over-masking is
accepted per ADR-0051 (over-masking is safe, under-masking is not).
Sensitive-substring key matching runs on the single-pass `%HH`-decoded,
lowercased key (`pass%77ord` decodes to `password` and redacts), while
the rendered key keeps its original encoded bytes. Double-encoding
(`%2540`) is out of scope: single-decode consumers cannot yield
credentials from it.

Call-site migration (rc-924sb):

- camel-config `redact_url` becomes `camel_api::redact::redact_url`.
- camel-jms `component.rs::redact_url` delegates to the canonical strict
  variant; `config.rs::redact_broker_url` becomes a thin wrapper passing
  the unchanged JMS key list (`password, passwd, secret, credential,
  token, username, user`) to `redact_url_with_query_allowlist`.
- camel-http keeps the url-crate parsed arm: guard via
  `window_has_at_sign`, accessor mask, render with query and fragment
  still set, then `redact_url(rendered)`. This deletes
  `mask_rendered_windows`, `window_has_at_sign`, `truncate_utf8_safe`,
  and the in-crate sentinel-compose block. The Err arm delegates to
  `redact_url_fail_closed`. One deliberate unification: sentinels compose
  on the parsed arm too (a fragment containing `?` renders
  `#[redacted]?[redacted]` instead of `#[redacted]`) — strictly more
  sentinel bytes, never fewer; a golden byte-exact pin covers the
  composition.

## Affected crates

- camel-api: new `redact` module (unconditional, no features), lib.rs
  registration, CONTEXT.md entry (lint-context-citations), module docs
  distinguish it from `endpoint_uri::to_redacted_string` (ADR-0051
  wrapper, authored-URI layer, out of scope here).
- camel-config: `redact_url` + `truncate_utf8_safe` deleted, delegation
  in, helper tests migrate.
- camel-jms: `mask_authority_windows` / `truncate_utf8_safe` /
  `redact_broker_url` bodies deleted; wrappers delegate; CONTEXT.md
  redaction paragraph stays accurate.
- camel-http: parsed arm composed over canonical helpers; Err arm
  delegates; duplicate helpers deleted.

`camel-api::endpoint_uri::to_redacted_string` is a fourth redaction
surface (catalog-driven, authored URIs, different threat model) —
explicitly out of scope, named here so the convergence claim is honest.

Dep edges: all three consumers already depend on camel-api; no new edges,
no cycles (pre-flight verified). lint-publish-registration is a
crates.io trustpub manifest gate — a new module needs no registration.

Spec placement (pre-flight ruling): `security` capability gains the
canonical-helper requirement (observable cross-surface behavior, no
import-graph SHALLs — structure lives in this design and in tasks.md);
`jms` gains the broker query-redaction requirement;
`http-url-resolution` has one MODIFIED requirement fixing four stale
drifts (first-`//` -> every window, missing fragment sentinel,
parse-success sentence, no backslash windows) while keeping every
existing scenario name and outcome. `cache-repo-configuration` needs no
delta: its redaction behavior is unchanged delegation.

## Architecture boundaries

Pure string utility in camel-api (contract crate, no url-crate
dependency added). Components and camel-config consume it at diagnostic
boundaries per ADR-0051 (credential redaction at diagnostic boundaries,
CONTEXT-MAP.md:179). No Runtime, DSL, or Services code changes; no
exchange-data validation (ADR-0032 stays with routes).

## Phases

- Phase 1 (rc-924sb): canonical module with landed semantics verbatim,
  call-site migration, test migration, camel-api CONTEXT.md entry.
- Phase 2 (rc-r7v8s): failing tests first (bd example, lowercase variant,
  literal-`@` bypass, email-kept stance, `user%40host%3Aport`
  over-mask stance), then the minimal-decode rule in the allowlist
  variant.
- Phase 3 (rc-f05q8): failing tests first (non-special `\\` authority,
  single-`\` `http:`-class, drive/UNC non-windows), then window
  enumeration v2 in the canonical scan; camel-http spec delta scenarios
  pinned.
