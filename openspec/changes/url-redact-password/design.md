# Design: url-redact-password

## Approach

`redact_url_for_diagnostics` parses the URL with `url::Url` and masks
userinfo only inside `if !u.username().is_empty()`. RFC 3986 permits
password-only userinfo (`http://:pass@host/`), which the `url` crate
parses with an empty username and `Some(password)` — so the branch is
skipped and the password survives into every diagnostic rendered through
this function (fence rejection, malformed-base error, error event
fields).

Fix: widen the branch condition to
`!u.username().is_empty() || u.password().is_some()` (url 2.5.8:
`password(&self) -> Option<&str>`). Inside the branch,
`set_username("***")` + `set_password(None)` serialize a password-only
URL as `http://***@host/` — the same `***@` masking shape the sibling
`mask_base_url_userinfo` (grammar path, byte-preserving string surgery)
already uses for any userinfo including password-only. No other
statement changes; query redaction and the 256-char truncation are
untouched.

Spec: the `http-url-resolution` capability already binds fence
rejections to "the diagnostics redaction path" (ADR-0071). The delta
MODIFIES the "CamelHttpUri host fence" requirement — restated in full,
all six existing scenarios verbatim — and appends one scenario:
password-only userinfo in the override must not leak into the rejection
error (distinct secret values so the assertions cannot pass vacuously).

Tests: appended to the existing `// Security: credential redaction`
module (lib.rs:3673). New cases:
1. password-only masked — assert `!contains(secret)` BEFORE asserting
   the exact shape `http://***@host/` (security intent stays explicit
   if the shape assertion ever drifts);
2. regressions: username+password still fully masked; no-userinfo URL
   byte-identical;
3. fence path: armed `allowedUriHosts` + `CamelHttpUri` with
   password-only userinfo → resolution error carries no password bytes
   AND the rendered rejected URL shows the `***@` masked authority
   (e.g. `http://***@evil.example.com/x?[redacted]`), pinning the new
   requirement's masking shape on the fence path itself (mirrors the
   existing fence test style).

## Affected crates

- camel-component-http: one condition in
  `redact_url_for_diagnostics` (lib.rs:2892); tests in the existing
  redaction module. No public API change (`pub(crate)` fn).
- openspec/specs (via change delta): `http-url-resolution` — one
  scenario added to the "CamelHttpUri host fence" requirement.

## Architecture boundaries

Component-layer only (camel-component-http). No Runtime, DSL, Services,
or Languages surface moves. The change honors the workspace credential
redaction boundary (CONTEXT-MAP, authority ADR-0051) and the existing
ADR-0071 fence contract: rejection diagnostics render the URL only
through the redaction path. Zone lease: camel-component-http ONLY.

## Alternatives considered

- Unconditional `set_password(None)` outside the branch: rejected —
  leaves the masked shape inconsistent with the sibling's `***@`
  convention and muddies the username-empty case.
- Overloading the existing fence scenario with a second
  `CamelHttpUri`: rejected in pre-flight (e_gpt) — a separate scenario
  keeps each assertion single-purpose; requirement count and prose are
  unchanged, scenarios go 16 → 17.
- Routing the delta to a new "url redaction" capability: rejected —
  the fence requirement already owns "diagnostics redaction path" as a
  named contract; a second home invites drift.

Single-phase change (no `## Phases` section, no phase headings in
tasks.md).
