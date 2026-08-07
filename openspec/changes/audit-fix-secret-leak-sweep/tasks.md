# Tasks: audit-fix-secret-leak-sweep

## camel-auth

### Task 1: Manual Debug for both TokenResponse structs (rc-c9xo + rc-fvl5)

**Files:**
- `crates/services/camel-auth/src/native_issuer.rs` (modified)
- `crates/services/camel-auth/src/oauth2.rs` (modified)

**Steps:**
1. In `native_issuer.rs`, remove the `#[derive(Debug)]` attribute on `pub struct TokenResponse` (line 121). Add a manual `impl fmt::Debug for TokenResponse` that redacts `access_token` and shows `token_type`, `expires_in`, `scope` normally:
   ```rust
   impl fmt::Debug for TokenResponse {
       fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
           f.debug_struct("TokenResponse")
               .field("access_token", &"[REDACTED]")
               .field("token_type", &self.token_type)
               .field("expires_in", &self.expires_in)
               .field("scope", &self.scope)
               .finish()
       }
   }
   ```
2. In `oauth2.rs`, change `#[derive(Debug, Deserialize)]` on `struct TokenResponse` (line 28) to `#[derive(Deserialize)]`. Add a manual `impl fmt::Debug for TokenResponse` that redacts `access_token` and shows `token_type`, `expires_in` normally.
3. Add regression tests in a `#[cfg(test)]` module in each file using sentinel values.

**Tests:**
- `native_token_response_debug_redacts_access_token`:
  - setup: `TokenResponse { access_token: Zeroizing::new("SENTINEL-JWT-SECRET".into()), token_type: "Bearer".into(), expires_in: 3600, scope: "read".into() }`
  - action: `format!("{:?}", resp)`
  - assert: result does NOT contain `"SENTINEL-JWT-SECRET"`
  - command: `cargo test -p camel-auth native_token_response_debug_redacts`
  - expected: pass after impl; fail before (derived Debug leaks the sentinel)
- `oauth2_token_response_debug_redacts_access_token`:
  - setup: `TokenResponse { access_token: Zeroizing::new("SENTINEL-OAUTH-TOKEN".into()), token_type: "Bearer".into(), expires_in: 3600 }`
  - action: `format!("{:?}", resp)`
  - assert: result does NOT contain `"SENTINEL-OAUTH-TOKEN"`
  - command: `cargo test -p camel-auth oauth2_token_response_debug_redacts`
  - expected: pass after impl; fail before

**Acceptance:**
- `cargo test -p camel-auth` passes.
- `cargo clippy -p camel-auth -- -D warnings` exits 0.

- [x] 1

## camel-component-wasm

### Task 2: Manual Debug for StateStore (rc-zb1b)

**Files:**
- `crates/components/camel-component-wasm/src/state_store.rs` (modified)

**Steps:**
1. Add `use std::fmt;` at the top of `state_store.rs` if not already present.
2. Change `#[derive(Debug, Clone)]` on `StateStore` (line 10) to `#[derive(Clone)]`.
3. Add a manual `impl fmt::Debug for StateStore` that redacts the `data` field:
   ```rust
   impl fmt::Debug for StateStore {
       fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
           f.debug_struct("StateStore")
               .field("data", &"[REDACTED]")
               .finish()
       }
   }
   ```
4. Add a regression test in a `#[cfg(test)]` module using a sentinel value.

**Tests:**
- `state_store_debug_redacts_data`:
  - setup: `StateStore` with `store("api-key", "SENTINEL-GUEST-SECRET")`
  - action: `format!("{:?}", store)`
  - assert: result does NOT contain `"SENTINEL-GUEST-SECRET"`
  - command: `cargo test -p camel-component-wasm state_store_debug_redacts`
  - expected: pass after impl; fail before

**Acceptance:**
- `cargo test -p camel-component-wasm` passes.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 2

## camel-otel

### Task 3: Manual Debug for OtelConfig (rc-2g5v)

**Files:**
- `crates/services/camel-otel/src/config.rs` (modified)

**Steps:**
1. Add `use std::fmt;` at the top of `config.rs` if not already present.
2. Change `#[derive(Debug, Clone)]` on `OtelConfig` (line 24) to `#[derive(Clone)]`.
3. Add a manual `impl fmt::Debug for OtelConfig` that redacts `endpoint` and `resource_attrs`:
   ```rust
   impl fmt::Debug for OtelConfig {
       fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
           f.debug_struct("OtelConfig")
               .field("endpoint", &"[REDACTED]")
               .field("service_name", &self.service_name)
               .field("protocol", &self.protocol)
               .field("sampler", &self.sampler)
               .field("resource_attrs", &"[REDACTED]")
               .field("logs_enabled", &self.logs_enabled)
               .field("metrics_interval_ms", &self.metrics_interval_ms)
               .finish()
       }
   }
   ```
4. Add a regression test in a `#[cfg(test)]` module using sentinel values.

**Tests:**
- `otel_config_debug_redacts_credentials`:
  - setup: `OtelConfig` with `resource_attrs = vec![("api.key".into(), "SENTINEL-API-KEY".into())]` and `endpoint = "https://user:SENTINEL-PASS@collector:4317".into()`
  - action: `format!("{:?}", cfg)`
  - assert: result does NOT contain `"SENTINEL-API-KEY"` and does NOT contain `"SENTINEL-PASS"`
  - command: `cargo test -p camel-otel otel_config_debug_redacts`
  - expected: pass after impl; fail before

**Acceptance:**
- `cargo test -p camel-otel` passes.
- `cargo clippy -p camel-otel -- -D warnings` exits 0.

- [x] 3

## camel-bridge

### Task 4: Manual Debug for BridgeProcessConfig (rc-4tbt)

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)

**Steps:**
1. Remove `#[derive(Debug)]` from `pub struct BridgeProcessConfig` (line 185).
2. Add a manual `impl fmt::Debug for BridgeProcessConfig` that redacts `broker_url` and `env_vars`. The `password` field is already safe via `Redacted<T>`. Show `spec`, `binary_path`, `broker_type`, `username`, `start_timeout_ms` normally:
   ```rust
   impl fmt::Debug for BridgeProcessConfig {
       fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
           f.debug_struct("BridgeProcessConfig")
               .field("spec", &self.spec)
               .field("binary_path", &self.binary_path)
               .field("broker_url", &"[REDACTED]")
               .field("broker_type", &self.broker_type)
               .field("username", &self.username)
               .field("password", &self.password)
               .field("start_timeout_ms", &self.start_timeout_ms)
               .field("env_vars", &format!("[REDACTED; {} entries]", self.env_vars.len()))
               .finish()
       }
   }
   ```
3. Add a regression test in a `#[cfg(test)]` module using sentinel values.

**Tests:**
- `bridge_process_config_debug_redacts_credentials`:
  - setup: `BridgeProcessConfig` with `broker_url = "amqp://u:SENTINEL-BRIDGE-PASS@host:5672"` and `env_vars` containing `("KEYSTORE_PASSWORD", "SENTINEL-ENV-PASS")`
  - action: `format!("{:?}", cfg)`
  - assert: result does NOT contain `"SENTINEL-BRIDGE-PASS"` and does NOT contain `"SENTINEL-ENV-PASS"`
  - command: `cargo test -p camel-bridge bridge_process_config_debug_redacts`
  - expected: pass after impl; fail before

**Acceptance:**
- `cargo test -p camel-bridge` passes.
- `cargo clippy -p camel-bridge -- -D warnings` exits 0.

- [x] 4

## camel-kafka

### Task 5: Drop Serialize from KafkaConfig + KafkaBrokerConfig (rc-xbl1)

**Files:**
- `crates/components/camel-kafka/src/config.rs` (modified)
- `crates/components/camel-kafka/src/broker_config.rs` (modified)

**Steps:**
1. In `config.rs` line 149, change `#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]` to `#[derive(Debug, Clone, PartialEq, serde::Deserialize)]`.
2. In `broker_config.rs` line 25, change `#[derive(Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]` to `#[derive(Clone, PartialEq, Default, serde::Deserialize)]`.
3. Run `cargo build --workspace --all-features --all-targets` — if any code in the workspace depends on serializing these types, the build fails at that call-site, proving the API break is real. If the build succeeds, zero call-sites exist.

**Tests:**
- `serialize_absence_via_workspace_build`:
  - setup: both types have `serde::Serialize` removed from derive lists
  - action: `cargo build --workspace --all-features --all-targets`
  - assert: build succeeds (exit 0), proving no code (including tests, examples, feature-gated) depends on serializing these types
  - command: `cargo build --workspace --all-features --all-targets`
  - expected: exit 0 (workspace compatibility confirmed — the compiler rejects code that tries to serialize a type without the trait)

**Acceptance:**
- `cargo build --workspace --all-features --all-targets` exits 0 (this IS the Serialize-absence verification — if any code serialized these types, the build would fail).
- `cargo test -p camel-component-kafka` passes.
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.

- [x] 5

## camel-cxf

### Task 6: Add regression test for CxfSecurityFields Debug redaction (rc-ryl0)

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified — test only)

**Steps:**
1. In a `#[cfg(test)]` module at the end of `config.rs`, add a test that constructs `CxfSecurityFields` with sentinel password values:
   ```rust
   #[test]
   fn cxf_security_fields_debug_redacts_passwords() {
       let fields = CxfSecurityFields {
           keystore_password: Some("SENTINEL-KEYSTORE-PASS".into()),
           truststore_password: Some("SENTINEL-TRUSTSTORE-PASS".into()),
           sig_password: Some("SENTINEL-SIG-PASS".into()),
           ..Default::default()
       };
       let debug = format!("{:?}", fields);
       assert!(!debug.contains("SENTINEL-KEYSTORE-PASS"));
       assert!(!debug.contains("SENTINEL-TRUSTSTORE-PASS"));
       assert!(!debug.contains("SENTINEL-SIG-PASS"));
   }
   ```
2. Verify the existing manual Debug impl (line 66: `impl fmt::Debug for CxfSecurityFields`) already redacts with `<redacted>`.

**Tests:**
- `cxf_security_fields_debug_redacts_passwords`:
  - setup: `CxfSecurityFields` with 3 sentinel passwords
  - action: `format!("{:?}", fields)`
  - assert: result does NOT contain any of the 3 sentinel values
  - command: `cargo test -p camel-component-cxf cxf_security_fields_debug_redacts`
  - expected: pass immediately (redaction already works; this is the missing regression test)

**Acceptance:**
- `cargo test -p camel-component-cxf` passes.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0.

- [x] 6
