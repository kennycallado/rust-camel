# Design: mail-account-host-binding

## Approach

Add `allowedHosts: Vec<String>` (`#[serde(default)]` under the existing
`rename_all = "camelCase"` container attribute) to `MailAccountConfig` in
`crates/components/camel-mail/src/config.rs`, defaulting to empty. Empty
means implicit selection is disabled for that account.

Add a pure validation function in `config.rs`:

```rust
pub(crate) fn validate_implicit_host(
    account: &str, allowed_hosts: &[String], uri_host: Option<&str>,
) -> Result<(), CamelError>
```

It succeeds when any entry matches `uri_host` via `eq_ignore_ascii_case`
(exact host, no subdomains; hosts are ASCII/punycode). On failure it
returns `CamelError::EndpointCreationFailed` — the typed variant the crate
already uses for endpoint-creation configuration failures (same class as
the imap-requires-account error) — with a message that names the account
and both remedies (`?account=<name>` or `allowedHosts` under
`[components.mail.accounts.<name>]`) and deliberately omits the URI host,
port and server URL (ADR-0093/ADR-0076: no server coordinates in errors).

Add `bind_account(accounts, requested, uri_host: Option<&str>) ->
Result<Option<(String, MailAccountConfig)>, CamelError>` in `config.rs`
that composes `resolve_account_name` (unchanged semantics), fetches the
account config, and — only when `requested.is_none()` and exactly one
account was selected implicitly — runs `validate_implicit_host`. Returns
`Ok(None)` when no accounts exist (SMTP anonymous send; IMAP maps `None`
to its existing requires-account error). `MailComponent::create_endpoint`
calls `bind_account` with `Some(&cfg.server.host)` after the endpoint
config parsed, in both the SMTP and IMAP branches, before any connector
or transport is built. The per-scheme behavior matrix:

| Case | Today | After |
| --- | --- | --- |
| explicit `?account=` | resolves | unchanged, `allowedHosts` ignored |
| implicit, one account, host in `allowedHosts` | resolves | resolves |
| implicit, one account, empty/missing list | resolves (risk) | endpoint creation fails |
| implicit, one account, host not listed | resolves (risk) | endpoint creation fails |
| implicit, one account, no URI host (`smtp:/x`, `smtp://`, `smtp://[]`) | generic parse error | typed binding error naming the account |
| explicit, no URI host | parse error | unchanged |
| several accounts, no `?account=` | fails | unchanged |
| several accounts, no URI host | parse error | unchanged (parse error still wins) |
| no accounts, `smtp`/`smtps` | anonymous | unchanged |
| no accounts, no URI host | parse error | unchanged |
| no accounts, `imap`/`imaps` | fails | unchanged |

**Hostless implicit selection.** `parse_authority` rejects
authority-less and empty-host URIs inside `from_uri`, before account
resolution — the generic `InvalidUri` would win and the mission requires
the account-naming binding error there. `MailComponent::create_endpoint`
therefore probes first: when the URI has no `?account=` param, exactly
one account is configured, and the path carries no host, it calls
`bind_account(..., None)` which always fails with the binding error; all
other selection paths fall through to `from_uri` and keep today's
errors. The probe is a small documented helper in `config.rs`:

```rust
pub(crate) fn uri_has_no_host(path: &str) -> bool
```

It answers only "is there a host a binding rule could match", with
`parse_authority`'s dedicated errors keeping priority: no `//` prefix,
empty authority, an empty host part (`//:25`), or an empty bracketed
host (`//[]`, `//[]:25`) → `true`; userinfo (`@` anywhere in the
authority, checked before bracket handling), bracketed-host trailing
junk (`//[]junk`), non-numeric bracketed ports (`//[]:abc`),
unterminated brackets, unbracketed IPv6 → `false`, so those dedicated
error texts are preserved. Unit tests pin every probe case.

No new URI option: `SMTP_OPTIONS`/`IMAP_OPTIONS` and the `UriConfig`
metadata descriptors are untouched (`allowedHosts` is account config, not
a URI parameter).

Tests: the eight mission acceptance tests live in the `component.rs`
test module (the crate's endpoint-creation module, exercising
`MailComponent::create_endpoint`); the pure-function unit tests for
`validate_implicit_host`, `uri_has_no_host` and `bind_account` plus the
TOML parse test live in the `config.rs` test module. The log-capture
test follows `docs/testing/tracing-capture-guards.md` (OnceLock
`ensure_global_tracing_default` guard, thread-local capture subscriber,
per-file copy in the test binary). Existing tests that relied on implicit
selection (component.rs helpers and `default_ports_on_endpoints`,
`create_endpoint_selects_single_account`, and friends) are updated to
declare `allowedHosts` or `?account=`; the integration tests that build
their own accounts TOML for `127.0.0.1` fake servers (`smtp_tls.rs`,
`imap_protocol.rs` refused-login, `greenmail_tier.rs`,
`mailpit_tier.rs`) gain `allowedHosts = ["127.0.0.1"]`.

Cross-crate consequence: `camel-bundles` fixture
`tests/fixtures/mail/Camel.toml` feeds
`boot_hands_memory_repository_to_mail`, which creates an implicit
`imap://127.0.0.1` endpoint; the fixture gains
`allowedHosts = ["127.0.0.1"]` so the boot test keeps exercising the
happy path.

## Affected crates

- `camel-component-mail`: config field, validation, component wiring,
  tests, rustdoc.
- `camel-bundles`: mail boot test fixture only (no code).
- Docs: `docs/adr/0093-...` (paragraph replacement, risk retired, cite
  rc-bcstf), `CHANGELOG.md` (fix/breaking entry + migration),
  `crates/components/camel-mail/CONTEXT.md`,
  `docs/src/components/mail.md` (account-selection paragraph).

## Architecture boundaries

Components stay behind their seams: the check lives in the mail
component's endpoint creation, not in `camel-core` or the URI parser;
`camel-component-api` types are consumed, not extended. No DSL, runtime,
or service changes. The account config surface (`MailComponentConfig`)
keeps `deny_unknown_fields`, so the new field is additive and validated.

## Migration

Operators relying on implicit single-account selection add the host to
`allowedHosts` in `[components.mail.accounts.<name>]`, or append
`?account=<name>` to URIs that intentionally target other hosts. The
CHANGELOG entry names both remedies.
