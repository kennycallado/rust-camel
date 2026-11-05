# Proposal: mail-account-host-binding

## Why

ADR-0093 accepts a risk: with exactly one configured mail account, every
endpoint without `?account=` logs in with it to **whatever host its URI
names**. With `toD`, `recipientList`, or URIs assembled from exchange data,
that forwards the account's credentials to a host the operator never
intended (bd rc-bcstf, found in the rc-q8zur closing review). This change
retires the accepted risk by binding implicit single-account selection to a
per-account host allow-list.

## What Changes

- `MailAccountConfig` gains an optional `allowedHosts: Vec<String>`
  (TOML `allowedHosts`, default empty).
- Implicit single-account selection (no `?account=`, exactly one account)
  now succeeds ONLY when the URI host exactly matches (case-insensitive)
  an entry in that account's `allowedHosts`. Empty or missing list, a
  non-matching host, and URIs without a host all fail endpoint creation.
- Explicit `?account=<name>` stays unchanged: it is the escape hatch for
  intentional multi-host use and ignores `allowedHosts`.
- Multi-account without `?account=` and no-account behavior are unchanged.
- The failure message names the account and both remedies (`?account=`
  or `allowedHosts`) and never contains the host, port or server URL
  (ADR-0093/ADR-0076 posture); a log-capture test asserts the host never
  appears.
- Docs: ADR-0093 accepted-risk paragraph replaced by the new rule;
  `CHANGELOG.md` fix entry with migration; crate `CONTEXT.md` and the
  mdBook mail page updated where account selection is described.

Out of scope (non-goals): changes to explicit `?account=` semantics, URI
userinfo rejection, TLS posture, header classes, a new ADR, account
host/port fields, redaction of operator-supplied account names.

## Acceptance criteria

- The eight mission acceptance tests pass (see tasks.md), written first
  and verified red before implementation.
- `cargo test -p camel-component-mail` green; all AGENTS.md quality
  gates green.
- The canonical mail-component requirement carries the new rule with all
  pre-existing scenarios preserved.

## Risk budget

Small, contained change: one config field, one validation function, wiring
in `MailComponent::create_endpoint`, plus test/doc updates. Acceptable
risk: existing implicit-selection users must add `allowedHosts` or
`?account=` — this is the intended breaking fix, surfaced in CHANGELOG.
No risk to other components: only `camel-mail` and the `camel-bundles`
mail boot fixture are touched.
