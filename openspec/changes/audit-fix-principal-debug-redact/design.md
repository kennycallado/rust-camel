# Design: audit-fix-principal-debug-redact

## Approach

Replace the derived `Debug` on `Principal` with a manual `impl std::fmt::Debug`
that redacts the untrusted `claims` field. The fix is ~8 lines, mirroring the
`ServerTlsConfig` redaction convention already established in camel-http
(`config.rs:90-97`):

```rust
impl std::fmt::Debug for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Principal")
            .field("subject", &self.subject)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("scopes", &self.scopes)
            .field("roles", &self.roles)
            .field("claims", &"[REDACTED]")
            .finish()
    }
}
```

The derive list becomes `#[derive(Clone, PartialEq, Serialize, Deserialize)]`
(`Debug` removed). All other fields render normally because `subject`, `issuer`,
`audience`, `scopes`, and `roles` are an intentional operator-visible allowlist
of identity descriptors — only `claims` is the arbitrary untrusted blob and is
suppressed.

**Achievable contract:** a claim value may coincidentally equal a retained
descriptor (e.g. a `sub` claim equal to `subject`). The redaction targets the
claims payload itself: the `claims` field renders as `[REDACTED]`, and any value
present ONLY in `claims` is absent from Debug output. It does not require
absence of strings that also appear in the retained allowlist.

## Affected crates

- `camel-api`: `security_policy.rs` — replace derived `Debug` on `Principal`
  with the redacting manual impl. Add a unit test in the same crate.

## Architecture boundaries

`Principal` lives in the contract crate `camel-api` (ADR-0010) so that
`camel-core` (enforcement) and `camel-dsl` can reference it without depending on
the `camel-auth` service. This change touches the contract type's `Debug`
representation only; no behavioral boundary changes. It respects the ADR-0032
untrusted-data boundary by ensuring the untrusted `claims` payload cannot leak
through the `Debug` formatting path that tracing/logging uses.

It does NOT alter the `ClaimsMapper` mapping contract (camel-auth), the
`SecurityPolicy`/`AuthorizationDecision` types, or any control-plane trait.

## Alternatives considered

- **Redact at the tracing layer (event filter).** Rejected: `Debug` is used by
  many call sites (`format!`, assertions, error messages), not only tracing. A
  per-call-site filter leaves the latent footgun in the type itself. The type
  must be safe-by-default.
- **Wrapper type `Redacted<Principal>`.** Rejected: it would change the public
  API surface and force every call site to opt in. The established pattern in
  this codebase (ServerTlsConfig, HttpAuth, and the manual-Debug impls in
  `crates/services/camel-auth`) is an inline manual impl.
- **Redact all fields.** Rejected: `subject`/`scopes`/`roles` are identity
  descriptors operators need in logs for debugging access decisions. Only
  `claims` is arbitrary untrusted data.
