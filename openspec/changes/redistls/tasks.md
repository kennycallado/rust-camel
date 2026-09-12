# Tasks: redistls

## Task 1: Document Redis TLS trust model

**Files:**

- `docs/src/components/redis.md` (modified)
- `crates/components/camel-redis/CONTEXT.md` (modified)

**Steps:**

1. Correct `tls_ca_cert` guidance for standalone and Sentinel TLS endpoints.
2. State that Redis TLS authenticates servers only in v1.
3. State that client-certificate authentication is unsupported and identify
   `TlsCertificates.client_tls = None` as deliberate.
4. State that deployment demand for `--tls-auth-clients yes` is the trigger
   for future mTLS work.

**Tests:**

- **Name:** Redis TLS documentation consistency check
- **Arrange:** Read both edited documents and search for `tls_ca_cert`,
  `client_tls`, `mTLS`, and `--tls-auth-clients yes`.
- **Act:** Run `openspec validate redistls --type change --json` and inspect the
  edited security paragraphs.
- **Assert:** Validation passes; docs describe Sentinel CA support, server-only
  verification, unsupported client authentication, and the revisit trigger;
  no stale claim says Sentinel ignores `tls_ca_cert`.

**Acceptance criteria:**

- Public Redis docs state the server-only TLS posture and unsupported mTLS.
- camel-redis context records the deliberate `client_tls: None` behavior.
- No Rust source, fixture, URI parameter, or connection behavior changes.

- [x] 1.1
