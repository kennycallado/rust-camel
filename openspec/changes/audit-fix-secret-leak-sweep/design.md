# Design: audit-fix-secret-leak-sweep

## Approach

Apply ADR-0051's two rules uniformly across 6 crates. The workspace already has a manual-Debug redaction pattern to mirror (`crates/services/camel-auth/src/credential_source.rs:49`: `.field("token", &"[REDACTED]")`).

Each affected type gets a manual `impl fmt::Debug` that replaces credential values with a redaction marker. The existing `Redacted<T>` newtype in camel-bridge stays as-is — no new wrapper types are introduced in this change.

## Affected crates

### camel-auth (rc-c9xo + rc-fvl5)

**`native_issuer.rs:121`** — `pub struct TokenResponse` (`#[derive(Debug)]` → manual `impl fmt::Debug`). The struct is `#[non_exhaustive]` with 4 fields:
- `access_token: Zeroizing<String>` — **credential** → redact.
- `token_type: String` — not credential → visible.
- `expires_in: u64` — not credential → visible.
- `scope: String` — not credential → visible.

Manual Debug: `f.debug_struct("TokenResponse").field("access_token", &"[REDACTED]").field("token_type", &self.token_type).field("expires_in", &self.expires_in).field("scope", &self.scope).finish()`.

**`oauth2.rs:28`** — `struct TokenResponse` (private, `#[derive(Debug, Deserialize)]` → `#[derive(Deserialize)]` + manual `impl fmt::Debug`). Fields:
- `access_token: Zeroizing<String>` — **credential** → redact.
- `token_type: String` — not credential → visible.
- `expires_in: u64` — not credential → visible.

Regression test (both types): construct a `TokenResponse` with sentinel token value `"SENTINEL-SECRET-TOKEN"`, assert `!format!("{:?}", resp).contains("SENTINEL-SECRET-TOKEN")`.

### camel-component-wasm (rc-zb1b)

**`state_store.rs:10`** — `StateStore` (`#[derive(Debug, Clone)]` → `#[derive(Clone)]` + manual `impl fmt::Debug`). The `data: Arc<Mutex<HashMap<String, String>>>` field holds arbitrary guest secrets (API keys, tokens per README SDK doc). Redact entirely: `f.debug_struct("StateStore").field("data", &"[REDACTED]").finish()`.

Regression test: store `"api-key" → "SENTINEL-SECRET-123"`, assert `!format!("{:?}", store).contains("SENTINEL-SECRET-123")`.

### camel-otel (rc-2g5v)

**`config.rs:24`** — `OtelConfig` (`#[derive(Debug, Clone)]` → `#[derive(Clone)]` + manual `impl fmt::Debug`). Credential-capable fields:
- `endpoint: String` — may carry basic-auth credentials in URL → redact.
- `resource_attrs: Vec<(String, String)>` — may carry API keys as values → redact values.

Non-credential fields remain visible: `service_name`, `protocol`, `sampler`, `logs_enabled`, `metrics_interval_ms`.

Manual Debug: redact `endpoint` and `resource_attrs`, show the rest normally.

Regression test: set `resource_attrs = vec![("api.key".into(), "SENTINEL-API-KEY".into())]` and `endpoint = "https://user:SENTINEL-PASS@collector".into()`, assert Debug excludes both sentinels.

### camel-bridge (rc-4tbt)

**`process.rs:185`** — `BridgeProcessConfig` (`#[derive(Debug)]` → manual `impl fmt::Debug`). Credential-capable fields:
- `broker_url: String` — credential-bearing URL per ADR-0051 → redact.
- `env_vars: Vec<(String, String)>` — passwords unwrapped from `Redacted<String>` by `to_env_vars()` → redact values.
- `password: Option<Redacted<String>>` — already safe via `Redacted` wrapper → keep as-is.
- `username: Option<String>` — NOT credential bytes (username is identity, not secret per ADR-0051) → visible.

Non-credential fields remain visible: `spec`, `binary_path`, `broker_type`, `start_timeout_ms`.

Manual Debug: redact `broker_url` and `env_vars` values. Show `username` and other non-credential fields normally.

**Note:** the `Redacted<T>` newtype and `to_env_vars()` method are NOT modified. The env_vars must still contain plaintext values for subprocess passing — the redaction is only on the Debug representation.

Regression test: construct `BridgeProcessConfig` with `broker_url = "amqp://u:SENTINEL-PASS@host"` and `env_vars` containing `("KEYSTORE_PASSWORD", "SENTINEL-PASS")`, assert Debug excludes both sentinels.

### camel-kafka (rc-xbl1)

**`config.rs:149`** — `pub struct KafkaConfig`: change `#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]` → `#[derive(Debug, Clone, PartialEq, serde::Deserialize)]`.

**`broker_config.rs:25`** — `pub struct KafkaBrokerConfig`: change `#[derive(Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]` → `#[derive(Clone, PartialEq, Default, serde::Deserialize)]`.

**Debug safety:** `KafkaConfig`'s derived Debug delegates to `KafkaBrokerConfig`'s manual redacting Debug (`broker_config.rs:45`) for each entry in `brokers_named`. The remaining `KafkaConfig` fields (`brokers`, `group_id`, `security_protocol`, etc.) are not credential bytes. No manual Debug needed for `KafkaConfig`.

**API break note:** dropping `Serialize` removes a public trait impl. The workspace build (`cargo build --workspace --all-features --all-targets`) IS the verification: if any code serializes these types, the build fails at that call-site. If the build succeeds, zero call-sites exist.

### camel-cxf (rc-ryl0)

**`config.rs:49`** — `CxfSecurityFields` already has a manual `Debug` impl that redacts `keystore_password`, `truststore_password`, `sig_password` with `<redacted>`. The redaction works. No code change needed.

Task: add a regression test in `crates/components/camel-cxf/tests/` (or inline `#[cfg(test)]` module in `config.rs`) that constructs `CxfSecurityFields` with sentinel password values and asserts `!format!("{:?}", fields).contains("SENTINEL")`.

## Architecture boundaries

All changes are within **Components** (wasm, kafka, cxf) and **Services** (auth, otel, bridge). No Runtime, DSL, or data-plane logic changes. The changes affect diagnostic representation (`Debug`) and serialization capability (`Serialize`), not runtime behavior.

## Alternatives considered

- **Adopt `Redacted<T>` wrapper everywhere:** rejected — would change field types and public APIs. Manual Debug is less invasive.
- **Field-name lint (rc-vh2l):** deferred to change A2. ADR-0051 explicitly rejects name-based detection.
- **Keep derive(Debug) with redaction attributes:** rejected — no standard Rust attribute does this. Manual impl is canonical per ADR-0051 §Debug rule.

## Phases

Single-phase change — all 6 tasks share the same fix-shape and are independent. No inter-task dependencies.
