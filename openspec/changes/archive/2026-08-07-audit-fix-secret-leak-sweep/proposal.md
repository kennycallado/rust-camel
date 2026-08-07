# Proposal: audit-fix-secret-leak-sweep

## Why

ADR-0051 (Credential Redaction at Diagnostic Boundaries, Accepted 2026-08-06) mandates that types holding credential bytes must not expose them through `Debug` or general-purpose `Serialize`. The audit found 6 violations across 5 crates plus 1 missing regression test. These are security findings: `format!("{:?}", token_response)` or `toml::to_string(&kafka_config)` would print plaintext passwords, JWTs, API keys, or SASL credentials to logs, error messages, or serialized output.

**bd issues:** rc-c9xo (P1), rc-zb1b (P1), rc-fvl5 (P2), rc-2g5v (P2), rc-4tbt (P2), rc-xbl1 (P2), rc-ryl0 (P3).

## What Changes

### Debug rule (5 types across 4 crates)
Replace `#[derive(Debug)]` with a manual `impl fmt::Debug` that replaces each credential value with a redaction marker. Each manual implementation gets a regression test (sentinel-in-assert pattern per ADR-0051 §Debug rule).

- **camel-auth** (`native_issuer.rs:121`): `pub struct TokenResponse { access_token: Zeroizing<String>, token_type: String, expires_in: u64, scope: String }` — redact `access_token` only. `token_type`, `expires_in`, `scope` are not credential bytes.
- **camel-auth** (`oauth2.rs:28`): `struct TokenResponse { access_token: Zeroizing<String>, token_type: String, expires_in: u64 }` (private) — redact `access_token` only. Drop `Debug` from derive, keep `Deserialize`.
- **camel-component-wasm** (`state_store.rs:10`): `StateStore { data: Arc<Mutex<HashMap<String, String>>> }` — redact the `data` field entirely. Guest secrets (API keys/tokens) live there per README SDK doc.
- **camel-otel** (`config.rs:24`): `OtelConfig { endpoint, service_name, protocol, sampler, resource_attrs: Vec<(String,String)>, logs_enabled, metrics_interval_ms }` — redact `endpoint` (may carry basic-auth credentials in URL) and `resource_attrs` values (may carry API keys as key-value pairs). Other fields are not credential bytes.
- **camel-bridge** (`process.rs:185`): `BridgeProcessConfig { spec, binary_path, broker_url: String, broker_type, username, password: Option<Redacted<String>>, start_timeout_ms, env_vars: Vec<(String,String)> }` — redact `broker_url` (credential-bearing URL per ADR-0051) and `env_vars` values (passwords unwrapped from `Redacted<String>` by `to_env_vars()`). The `password` field is already safe via the `Redacted` wrapper.

### Serialize rule (2 types, 1 crate)
- **camel-kafka** (`config.rs:149` + `broker_config.rs:25`): Drop `serde::Serialize` from `KafkaConfig` and `KafkaBrokerConfig`. These are configuration types — ADR-0051 allows `Deserialize` without `Serialize`. This is a **deliberate API break**: any code that serializes these types must be removed or converted to an explicit redacted view. Verification: `cargo build --workspace --all-features --all-targets` succeeds (the compiler is the checker — if any code serializes these types, the build fails at that call-site).
- `KafkaBrokerConfig` Debug is already manually redacted (`broker_config.rs:45`). `KafkaConfig` Debug is derived but safe: its only nested credential-capable type is `brokers_named: HashMap<String, KafkaBrokerConfig>`, which delegates to the manual redacting Debug. No manual Debug needed for `KafkaConfig`.

### Test gap (1 crate)
- **camel-cxf** (`config.rs:49`): `CxfSecurityFields` already has a manual `Debug` impl that redacts `keystore_password`, `truststore_password`, `sig_password` with `<redacted>`. The redaction works. rc-ryl0 = no regression test exists. This task adds a sentinel test in the camel-cxf crate.

**Explicitly excluded:** the ADR-0051 enforcement lint (rc-vh2l) is change A2. No new ADRs. No runtime behavior change.

## Acceptance criteria

- `format!("{:?}", TokenResponse)` (native_issuer) does NOT contain the sentinel JWT value.
- `format!("{:?}", TokenResponse)` (oauth2) does NOT contain the sentinel token value.
- `format!("{:?}", StateStore)` does NOT contain sentinel guest-secret values.
- `format!("{:?}", OtelConfig)` does NOT contain sentinel `resource_attrs` values or credentials in `endpoint`.
- `format!("{:?}", BridgeProcessConfig)` does NOT contain sentinel values in `broker_url` or `env_vars`.
- `serde::Serialize` is NOT implemented for `KafkaConfig` or `KafkaBrokerConfig` — verified by `cargo build --workspace --all-features --all-targets` succeeding after removing the derive (if any code serialized these types, the build would fail).
- `format!("{:?}", CxfSecurityFields)` does NOT contain sentinel password values (regression test added).

## Risk budget

Low-moderate. Manual Debug impls are mechanical but must cover ALL credential fields. Dropping Serialize is an API break — verified by `cargo build --workspace --all-features --all-targets` succeeding. Each fix has a regression test per ADR-0051 mandate.
