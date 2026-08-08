# Proposal: audit-fix-principal-debug-redact

## Why

`Principal` (camel-api `security_policy.rs:13`) derives `Debug`. Its `claims:
serde_json::Value` field carries untrusted, provider-mapped token claims
(ADR-0032 untrusted-data boundary). Any `format!("{principal:?}")` or
`tracing::debug!(?principal)` discloses the full claim set — PII and custom
claims (email, phone, national IDs, internal tenant identifiers).

This is the F-camel-api-M3 finding, escalated Minor to Important (rc-yv1m). It
is DISTINCT from the ADR-0051 secret-leak sweep (audit-fix-secret-leak-sweep):
claims are untrusted user data, not operator credentials. The sweep covered
credentials; this change covers untrusted identity data.

## What Changes

- Remove `Debug` from the `#[derive(...)]` list on `Principal`.
- Add a manual `impl std::fmt::Debug for Principal` that renders `subject`,
  `issuer`, `audience`, `scopes`, `roles`, and redacts `claims` to
  `[REDACTED]`. Mirrors the established `ServerTlsConfig` redaction pattern
  (camel-http `config.rs:90`).
- Add a regression test proving no claim value appears in the `Debug` output.

Explicitly excluded: `Serialize`/`Deserialize` stay — `claims` is the
principal's legitimate data payload that flows through the auth boundary.
`PartialEq`, `Clone` stay. No field renames, no API shape change.

## Acceptance criteria

- `format!("{:?}", principal)` (and `{:#?}`) renders `claims` as `[REDACTED]`;
  any value present ONLY in `claims` is absent from the output, while retained
  descriptor values (`subject`/`issuer`/`audience`/`scopes`/`roles`) remain.
- Manual `Debug` mirrors the `ServerTlsConfig` redaction convention.
- Named regression test covers compact AND pretty Debug formatting with a unique
  claim sentinel.
- Existing camel-api tests pass; `cargo fmt --check`, `cargo clippy -p camel-api
  -- -D warnings`, and `cargo xtask lint-secrets` are green.

## Risk budget

Low. Single struct in the contract crate; the public surface (`Principal`
fields, `Serialize`, `PartialEq`, `Clone`) is unchanged. Only the derived
`Debug` formatting is replaced. Acceptable risk: a caller that relied on
`Debug` printing claims (none found in-tree — claims are operator-untrusted
data, never a logging contract). Out of bounds: touching `Serialize`, renaming
fields, or changing the `ClaimsMapper` mapping contract.
