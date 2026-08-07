# Design: audit-fix-secret-leak-lint

## Approach

Extend the existing `lint-secrets` xtask with an AST-based derive inspection
pass. The existing regex-based sink scanner (format!/tracing! patterns)
remains unchanged. The new pass uses `syn` to parse `.rs` files, extract
struct/enum definitions with their derive macros and doc comments, and
enforce ADR-0051 consistency rules.

### Semantic classification

A closed-vocabulary doc attribute marks credential boundaries:

```
/// ADR-0051 credential boundary: manual-redaction
```

Three classifications:

| Classification | Debug derive | Serialize derive | Use case |
|---|---|---|---|
| `manual-redaction` | REJECTED | REJECTED | Types carrying raw credentials (`TokenResponse`, `StateStore`, `OtelConfig`, `BridgeProcessConfig`) |
| `redacting-wrapper` | ALLOWED | REJECTED | Types whose Debug impl delegates to nested redacting Debug (`KafkaConfig`, `KafkaBrokerConfig`) |
| `protocol-dto` | REJECTED | ALLOWED | Types that must serialize for wire protocols (documentation of the protocol boundary remains code-review enforced per ADR-0051) |

Unknown, malformed, or conflicting duplicate classifications produce
violations. A typo cannot silently become an unannotated type.

Unannotated types are not inspected by the derive pass (unless they contain
`Zeroizing<...>` fields — see below). Code review owns classification —
the lint enforces consistency after classification, not discovery.

### Zeroizing auto-detection

`Zeroizing<T>` is automatically recognized as credential-capable, including
the qualified path `zeroize::Zeroizing<T>`. Any type containing a
`Zeroizing<...>` field must carry a `manual-redaction` classification
regardless of field name. Nine existing types in `camel-auth` carry
`Zeroizing<String>` fields:

| Type | File | Current derives |
|---|---|---|
| `NativeCredentialSecret` | `native_auth.rs:14` | `Clone` |
| `ResolvedCredential` | `native_auth.rs:20` | `Clone` |
| `M2mClientSecret` | `native_client_store.rs:33` | `Clone` |
| `ResolvedM2mClient` | `native_client_store.rs:38` | (none) |
| `CachingTokenIntrospector` | `introspection.rs:68` | (none) |
| `TokenResponse` (native_issuer) | `native_issuer.rs:122` | manual Debug (A1) |
| `TokenResponse` (oauth2) | `oauth2.rs:30` | `Deserialize` + manual Debug (A1) |
| `CachedToken` | `oauth2.rs:48` | (none) |
| `ClientCredentialsProvider` | `oauth2.rs:70` | (none) |

None derive `Debug` or `Serialize` — they will pass the lint once annotated.

### KafkaConfig as redacting-wrapper

`KafkaConfig` derives `Debug` and delegates credential redaction to the
nested `KafkaBrokerConfig` manual Debug impl (which redacts
`brokers_named`). This makes `KafkaConfig` a `redacting-wrapper`, not
`manual-redaction`: its own Debug derive is safe because the credential
fields are inside a type whose Debug already redacts.

### Implementation shape

1. Add `syn = { workspace = true, features = ["full"] }` to
   `scripts/xtask/Cargo.toml` (the `full` feature is required for
   `syn::parse_file`).
2. Implement `lint_credential_derives(workspace_root) -> Vec<SecretViolation>`.
3. `lint_secrets` calls both the existing regex scanner and the new
   AST scanner, merging violation lists.
4. Each `.rs` file is parsed once with `syn::parse_file`. If parsing fails,
   the lint hard-fails (reports the parse error and exits non-zero) rather
   than silently skipping enforcement.
5. Struct/enum items are checked for: (a) the doc attribute, (b) derive
   macros, (c) field types containing `Zeroizing<...>` (including
   `zeroize::Zeroizing<...>` qualified paths).
6. Line numbers are extracted from `syn`'s `span` information for accurate
   violation reporting.

## Affected crates

- `scripts/xtask`: New `lint_credential_derives` function, `syn` dependency
  with `full` feature, unit tests.
- `crates/services/camel-auth`: Annotate `TokenResponse` ×2 (native_issuer,
  oauth2) as `manual-redaction`. Annotate `NativeCredentialSecret`,
  `ResolvedCredential`, `M2mClientSecret`, `ResolvedM2mClient`,
  `CachingTokenIntrospector`, `CachedToken`, `ClientCredentialsProvider` as
  `manual-redaction`.
- `crates/components/camel-component-wasm`: Annotate `StateStore` as
  `manual-redaction`.
- `crates/services/camel-otel`: Annotate `OtelConfig` as `manual-redaction`.
- `crates/services/camel-bridge`: Annotate `BridgeProcessConfig` as
  `manual-redaction`.
- `crates/components/camel-kafka`: Annotate `KafkaConfig` +
  `KafkaBrokerConfig` as `redacting-wrapper`.
- `docs/adr/0051-...md`: Amend § Enforcement.

## Architecture boundaries

This change touches build tooling (`scripts/xtask`) and documentation only.
No runtime, DSL, component, service, or language boundary is affected.
The doc annotations are ordinary Rust documentation comments — they add
no runtime behavior, no trait bounds, and no public API surface.

## Alternatives considered

- **Field-name heuristic** (Option D): Rejected per ADR-0051 and rc-vh2l.
  Misses opaque containers (`StateStore.data`), false-positives on
  metadata (`client_key_path`).
- **Marker trait** (Option A): Requires proc-macro plumbing for a custom
  attribute. Doc attributes achieve the same semantic marking without
  runtime dependencies.
- **Allowlist** (Option B): Inverts policy; allowlists become stale
  inventories of "safe" types rather than explicit boundary declarations.
- **Separate xtask subcommand**: Rejected — the credential derive check
  belongs in `lint-secrets` because both enforce ADR-0051. One command,
  one mental model.
