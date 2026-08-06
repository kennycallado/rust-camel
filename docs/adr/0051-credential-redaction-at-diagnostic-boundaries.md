# ADR-0051: Credential Redaction at Diagnostic Boundaries

**Date:** 2026-08-06
**Status:** Accepted
**Amends:** none
**Related:** ADR-0012, ADR-0032, ADR-0033
**Origin:** `FC-DEBUG-SECRET-LEAK` (`rc-c9xo`, `rc-zb1b`) and
`FC-SERIALIZE-SECRET-LEAK` (`rc-xbl1`)

## Decision

Types that hold credential bytes must not expose those bytes through `Debug` or
general-purpose `Serialize` implementations.

### Secret scope

Credential bytes include:

- passwords and passphrases;
- bearer, access, refresh, session, and identity tokens;
- API keys and client secrets;
- private or signing key material;
- credential-bearing URLs and connection strings;
- values in a container whose contract permits guest or operator credentials.

Usernames, client identifiers, public keys, certificates, secret hashes, and
paths or locations of credential files are not credential bytes. A crate can
redact this metadata under a stricter local policy.

### `Debug` rule

A type that holds credential bytes must not derive `Debug`. It must use one of
these patterns:

1. Implement `Debug` manually and replace each credential value with
   `[REDACTED]` or omit the field.
2. Store each credential in a dedicated wrapper whose `Debug` implementation
   redacts the value. The wrapper must have a regression test.

Each manual implementation must have a regression test. The test formats a
unique sentinel and verifies that output does not contain the sentinel.

`Zeroizing<String>` protects memory after drop. It does not redact formatted
output. A type that contains `Zeroizing<String>` must still follow this rule.

### `Serialize` rule

A runtime or configuration type that holds credential bytes must not derive
`Serialize`. Configuration types can derive `Deserialize` without deriving
`Serialize`.

A dedicated wire type can serialize credential bytes only when transmission of
the credential is its explicit protocol contract. Such a type must not also
serve as a configuration, diagnostic, or general-purpose state type. Its docs
must name the protocol boundary.

Diagnostic export must use a separate redacted view. It must not reuse a wire
serializer that emits credential bytes.

## Context

The workspace already has manual redaction in Redis, Keycloak, gRPC,
SurrealDB, JMS, SQL, Kafka, and other components. Recent audits found the same
failure mode in public token responses and WASM guest state. Kafka also exposes
an adjacent serialization vector through configuration derives.

The WASM `StateStore` shows why field-name checks are insufficient. Its field is
named `data`, but the store contract permits guest API keys and tokens. The HTTP
TLS finding shows the opposite problem. A field named `client_key_path` contains
metadata, not private-key bytes.

`Debug` and `Serialize` are transitive. A safe outer type can become unsafe when
a nested type adds a credential field. The redaction contract therefore belongs
to the type that owns the credential boundary.

## Why This Is a New ADR

ADR-0012 assigns log levels and signal ownership. It does not define which
payloads a formatter can expose. This decision also covers panic diagnostics,
test output, and serialization outside logging, so an ADR-0012 amendment would
be too narrow.

ADR-0032 defines exchange data as untrusted. Credentials can come from trusted
operator configuration and still require confidentiality. Trust and disclosure
are different concerns, so this decision does not amend ADR-0032.

ADR-0033 governs secure defaults and startup validation. It does not govern
diagnostic representation or serialization.

## Enforcement

Code review and crate-local regression tests enforce this policy now. Existing
`cargo xtask lint-secrets` remains a sink lint for format and tracing macros.

Do not add a derive-name lint yet. A field-name heuristic would miss opaque
containers such as `StateStore.data`. It would also flag non-secret metadata
such as `client_key_path`, token types, and cancellation tokens. A type-aware
lint without semantic annotations would create both false negatives and false
positives.

Revisit mechanical enforcement at the T2 audit sweep, or when another confirmed
secret-bearing derive appears, whichever comes first. The revisit must evaluate
a semantic marker or redacting wrapper contract before it expands
`lint-secrets`.

## Considered Options

### Keep crate-local conventions

Rejected. The same representation bug crossed service and component crates.
Local examples did not prevent new derived implementations.

### Adopt the policy and add a field-name lint now

Rejected. The known positive and negative examples prove that names do not
model the credential boundary accurately.

### Adopt the policy and defer only mechanical enforcement

Chosen. Remaining audits can cite one rule now. The T2 sweep can design a lint
from a larger verified corpus without delaying the security contract.

## Consequences

- Audit findings distinguish credential bytes from file-path metadata.
- Secret-bearing types use manual redaction or a tested redacting wrapper.
- General-purpose configuration serialization cannot expose credentials.
- Explicit protocol DTOs can transmit credentials when that is their sole
  contract.
- `cargo xtask lint-secrets` keeps its current sink-focused scope until the
  enforcement revisit.

## Self-Grill Record

**Questions generated:**

1. [glossary] Does "credential boundary" conflict with the exchange-data trust
   boundary or handler-contract boundary?
2. [sharpen] Which values are credentials, and does a private-key file path
   count as credential bytes?
3. [scenario] Can a field-name lint catch WASM guest secrets without flagging
   TLS paths and cancellation tokens?
4. [cross-ref] Does the workspace already use the proposed redaction pattern,
   and do existing ADRs already own this rule?

**Answers:**

1. [glossary] No. `CONTEXT-MAP.md` defines the exchange-data trust boundary as
   an input-validation rule and the handler-contract boundary as a log-ownership
   rule. This ADR defines confidentiality at representation boundaries.
2. [sharpen] Credential bytes grant access or prove identity. A path identifies
   a file but does not contain its private-key bytes. `camel-http::TlsConfig`
   stores `client_key_path: Option<String>`, while the auth token responses store
   `Zeroizing<String>` token values (`camel-http/src/config.rs`,
   `camel-auth/src/native_issuer.rs`, `camel-auth/src/oauth2.rs`).
3. [scenario] No. `StateStore` stores arbitrary values under `data`, so a name
   check misses the documented `api-key = secret-123` case. The same check over
   `key` or `token` flags `client_key_path` and runtime cancellation tokens.
   These cases require semantic type information
   (`camel-component-wasm/README.md`,
   `camel-component-wasm/src/state_store.rs`, `camel-http/src/config.rs`).
4. [cross-ref] Redis, Keycloak, gRPC, SurrealDB, JMS, SQL, Kafka, and auth types
   already implement redacting `Debug`. ADR-0012 covers log levels. ADR-0032
   covers untrusted exchange data. Neither covers `Debug` and `Serialize`
   confidentiality across trusted and untrusted sources.

**Outcome:** refine. Adopt the workspace policy now. Exclude credential-file
paths from the credential-byte rule. Defer only mechanical enforcement. A new
ADR is warranted because disclosure cannot be undone, `Zeroizing<String>` is a
surprising non-redactor, and the design trades derive ergonomics against
confidentiality and protocol serialization needs.

**Self-grill mode:** `self-grill-proposals` skill.
