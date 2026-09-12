# Proposal: url-redact-password

## Why

`redact_url_for_diagnostics` (crates/components/camel-http/src/lib.rs:2892)
gates userinfo masking on `!u.username().is_empty()`. A valid RFC 3986 URL
with password-only userinfo — `http://:pass@evil.com/` — skips
`set_password(None)`, so diagnostics that must render the URL only through
the redaction path echo the raw password. The reported leak path is the
`allowedUriHosts` fence-rejection error (ADR-0071), which the
`http-url-resolution` spec already binds to "the diagnostics redaction
path". This is an ADR-0051 redaction-law violation (credential-bearing
URLs are credential bytes; they must never render in diagnostics).

Found during http-contract-surface task 1.2 review. bd: rc-u4jk6 (P2).

## What Changes

- Fix the masking gate in `redact_url_for_diagnostics`: enter the masking
  branch when the username is empty but a password is present
  (`!u.username().is_empty() || u.password().is_some()`). Password-only
  userinfo then renders as `***@`, matching the established masking shape
  of the sibling `mask_base_url_userinfo`.
- Amend the `http-url-resolution` capability, requirement "CamelHttpUri
  host fence": add a scenario covering password-only userinfo in the
  fence-rejection error (requirement restated in full per delta format).
- Tests appended to the existing "Security: credential redaction" module
  in camel-component-http (no new harness): password-only masked with
  exact shape; username+password and no-userinfo regressions; fence
  rejection with password-only userinfo.

Excluded: the malformed-input truncation fallback of
`redact_url_for_diagnostics` (separate audit class); the grammar-path
`mask_base_url_userinfo` (already masks password-only correctly — masking
through the final `@` does not depend on a non-empty username).

## Acceptance criteria

- `redact_url_for_diagnostics("http://:pass@host/")` never contains
  `pass`; exact masked shape is `http://***@host/`.
- Username+password URLs still mask both; no-userinfo URLs are unchanged.
- Armed-fence rejection of a `CamelHttpUri` carrying password-only
  userinfo produces an error whose text carries no password bytes.
- `cargo test -p camel-component-http` passes; clippy, fmt, the xtask
  lints (incl. lint-secrets), schema-check, and the doc gate pass in the
  worktree.

## Risk budget

One-condition change in a security-redaction function: low blast radius,
high review sensitivity. Any behavior change beyond the password-only
case (existing masked shapes, query redaction, truncation) is out of
bounds and reverts the change.
