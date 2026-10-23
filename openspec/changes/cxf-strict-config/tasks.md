# Tasks: cxf-strict-config

## camel-cxf

### Task 1.1: Strict shape walk over the cxf TOML surface

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)

**Steps:**
1. Add a private fields-introspection helper near the top of the config
   section of `crates/components/camel-cxf/src/config.rs`:
   - `struct FieldsRecorder(Option<&'static [&'static str]>)` implementing
     `serde::Deserializer<'de>` FOR `&mut FieldsRecorder` with
     `type Error = serde::de::value::Error`, so the caller retains the
     recorder: `deserialize_struct` stores the `fields` slice in `self.0`
     and then calls `visitor.visit_map(EmptyMapAccess)` with an empty
     `serde::de::MapAccess` (its `next_key_seed` returns `None`); all other
     `Deserializer` methods return `Err(serde::de::Error::custom("fields
     recorder"))`.
   - `fn struct_fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str]`
     keeps a local `let mut rec = FieldsRecorder(None);`, calls
     `let _ = T::deserialize(&mut rec);`, ignores the resulting
     "missing field" error (fields are captured before values are
     requested), and returns
     `rec.0.expect("derive passed FIELDS to deserialize_struct")`.
2. Add a private violation collector and walker in `config.rs`:
   - `fn validate_table_shape(table: &toml::Table) -> Result<(), camel_component_api::CamelError>`
     walks, in this order: (a) the pool table keys against
     `struct_fields::<CxfPoolConfigRaw>()`, (b) each element of the
     `profiles` array (skipping non-table elements — serde reports those)
     against `struct_fields::<CxfProfileConfig>()`, (c) each element's
     `security` sub-table (only when it is a table) against
     `struct_fields::<CxfSecurityFields>()`, (d) that sub-table's `tls`
     sub-table (only when it is a table) against
     `struct_fields::<CxfTlsConfig>()`.
   - Identity rendering: profile-scoped violations use `profiles.<name>`
     (entry's `name` when it is a TOML string), else `profiles[<index>]`;
     pool-level violations use `[components.cxf]`; security/tls scoped
     messages extend the profile identity (e.g. `profiles.p.security.tls`).
   - Classification of a key absent at its level: build the list of other
     levels whose field slice contains the key — pool renders
     `[components.cxf]`, profile renders `profiles.<name>`, security renders
     `profiles.<name>.security`, tls renders `profiles.<name>.security.tls`.
     Non-empty → misplaced-key message: `'<key>' is accepted under
     <loc1>[ or <loc2>…], not under <identity>` (list EVERY location for
     multi-location keys — `keystore_path`, `keystore_password`,
     `truststore_path`, `truststore_password` exist in BOTH
     `CxfSecurityFields` and `CxfTlsConfig`). Empty → unknown-key message:
     `unknown key '<key>' for <identity>; accepted keys: <sorted,
     comma-joined accepted keys at this level>`.
   - The key `listener_tls` at pool level gets its own message: it is not a
     TOML key; the accepted surface is the five flat `soap_tls_*` keys
     (`soap_tls_cert_path`, `soap_tls_key_path`, `soap_tls_client_ca_path`,
     `soap_tls_pkcs12_path`, `soap_tls_pkcs12_password`).
   - The walk does NOT recurse into `reconnect` and prints no config values
     anywhere (only key names, accepted locations, identity).
   - Collect ALL violations, sort them by `(location, key)`, and return them
     joined by newlines in one `CamelError::Config`; `Ok(())` when none.
3. Switch the serde attribute on `CxfPoolConfig` from
   `#[serde(try_from = "CxfPoolConfigRaw")]` to
   `#[serde(try_from = "toml::Table")]` and add
   `impl TryFrom<toml::Table> for CxfPoolConfig` that first calls
   `validate_table_shape(&table)?`, then performs
   `<CxfPoolConfigRaw as TryFrom<toml::Table>>`-equivalent decoding via
   `table.clone().try_into().map_err(|e: toml::de::Error|
   CamelError::Config(e.to_string()))?` and delegates to the EXISTING
   `impl TryFrom<CxfPoolConfigRaw> for CxfPoolConfig` (assembly +
   `validate()` stay unchanged). Keep the existing `TryFrom<CxfPoolConfigRaw>`
   impl as-is.
4. Add the tests below to the existing `#[cfg(test)] mod tests` in
   `config.rs`, following its current `toml::from_str::<CxfPoolConfig>`
   style. Run `cargo test -p camel-cxf --lib`.

**Tests:** (all via `toml::from_str::<CxfPoolConfig>(...)`; command
`cargo test -p camel-cxf --lib`)
- `unknown_key_in_profile_table_rejected`: profile entry `name = "p"`,
  `wsdl_path`/`service_name`/`port_name` valid, plus
  `exposd_soap_headers = true` at profile level → load errors;
  message contains `exposd_soap_headers`, `profiles.p`, `unknown`, and the
  accepted-keys list contains `wsdl_path`.
- `unknown_key_in_security_table_rejected`: `sig_usrname = "x"` under
  `profiles.p.security` → error contains `sig_usrname`, `profiles.p.security`,
  and accepted-keys list contains `sig_username`.
- `unknown_key_in_security_tls_table_rejected`: `keystore_paht = "x"` under
  `profiles.p.security.tls` → error contains `keystore_paht`, `unknown`,
  `profiles.p.security.tls`, and the accepted-keys list contains
  `keystore_path`.
- `unknown_key_in_pool_table_rejected`: `max_bridge = 2` at pool level →
  error contains `max_bridge`, `[components.cxf]`, `unknown`, and the
  accepted-keys list contains `max_bridges` and `profiles`.
- `profile_without_name_falls_back_to_index`: profile entry with no `name`
  carrying misplaced `exposed_soap_headers` → error contains `profiles[0]`.
- `non_string_name_falls_back_to_index`: profile entry with `name = 42` and
  misplaced `exposed_soap_headers` → error contains `profiles[0]` (walker
  identity; no panic).
- `misplaced_exposed_soap_headers_names_path`: `exposed_soap_headers = ["{http://a}b"]`
  at profile level of `name = "p"` → error contains `exposed_soap_headers`,
  `profiles.p.security`, and `profiles.p`.
- `multi_location_key_lists_both_locations`: `keystore_path = "x"` at profile
  level → error contains BOTH `profiles.p.security` AND
  `profiles.p.security.tls`.
- `single_location_key_names_one_location`: `keystore_type = "PKCS12"` at
  security level → error contains `accepted under \`profiles.p.security.tls\``
  and does NOT contain `accepted under \`profiles.p.security\``.
- `pool_level_profile_key_names_per_profile_path`: `exposed_soap_headers = ["{http://a}b"]`
  at pool level → error contains `exposed_soap_headers`, `[components.cxf]`,
  and a `profiles.` + `.security` location.
- `listener_tls_table_rejected_with_flat_key_hint`:
  `listener_tls = { cert_path = "x" }` at pool level → error contains
  `listener_tls` and ALL five flat key names `soap_tls_cert_path`,
  `soap_tls_key_path`, `soap_tls_client_ca_path`, `soap_tls_pkcs12_path`,
  `soap_tls_pkcs12_password`.
- `violations_aggregate_in_one_sorted_error`: one config whose `profiles`
  array has a misplaced-key violation in `profiles[2]`, another in
  `profiles[10]`, plus `max_bridge = 2` at pool level → exactly one `Err`;
  the three violation lines appear in this relative order:
  `[components.cxf]`-scoped line, then `profiles[10]` line, then
  `profiles[2]` line (string-sorted location order, which differs from both
  array-traversal order and any first-found order); two consecutive loads
  produce the byte-identical message.
- `walker_diagnostics_never_echo_values`: unknown key
  `api_key = "supersecret-value"` at profile level → error message contains
  `api_key` and NOT `supersecret-value`.
- `env_var_assembly_unchanged`: fixed valid pool TOML with one profile `p`
  setting `address`, `sig_username`, `keystore_path`+`keystore_password`
  under `security`, `keystore_path`+`keystore_password`+
  `truststore_path`+`truststore_password` under `security.tls` (so both
  `TLS_KEYSTORE_TYPE` and `TLS_TRUSTSTORE_TYPE` defaults apply), and
  `exposed_soap_headers` under `security` → after load,
  `crate::pool::profile_env_vars_for(&cfg.profiles)[0].to_env_vars()` has a
  key set equal EXACTLY to {`CXF_PROFILE_P_WSDL_PATH`,
  `CXF_PROFILE_P_SERVICE_NAME`, `CXF_PROFILE_P_PORT_NAME`,
  `CXF_PROFILE_P_ADDRESS`, `CXF_PROFILE_P_KEYSTORE_PATH`,
  `CXF_PROFILE_P_KEYSTORE_PASSWORD`, `CXF_PROFILE_P_SIG_USERNAME`,
  `CXF_PROFILE_P_EXPOSED_SOAP_HEADERS`, `CXF_PROFILE_P_TLS_KEYSTORE_PATH`,
  `CXF_PROFILE_P_TLS_KEYSTORE_PASSWORD`, `CXF_PROFILE_P_TLS_KEYSTORE_TYPE`,
  `CXF_PROFILE_P_TLS_TRUSTSTORE_PATH`,
  `CXF_PROFILE_P_TLS_TRUSTSTORE_PASSWORD`,
  `CXF_PROFILE_P_TLS_TRUSTSTORE_TYPE`} — assert full set equality (any
  addition, removal, or rename fails) and spot-check the
  `EXPOSED_SOAP_HEADERS` value is the comma-joined list.
- `correct_placement_still_loads`: profile `p` with `exposed_soap_headers`
  under `profiles.p.security` (reuse the existing `exposed_soap_header_config`
  helper or equivalent valid TOML) → `toml::from_str::<CxfPoolConfig>` is
  `Ok`, the loaded profile's `security.exposed_soap_headers` equals the
  configured list, and `crate::pool::profile_env_vars_for(&cfg.profiles)`
  emits `CXF_PROFILE_P_EXPOSED_SOAP_HEADERS` with the comma-joined list
  (env-var assembly identity).

**Acceptance:**
- `cargo test -p camel-cxf --lib` exits 0 with all new tests present and
  passing.
- Pre-existing `config.rs` tests (e.g. the `valid toml` cases at load sites)
  still pass unchanged.
- `cargo fmt --check -p camel-cxf` exits 0.
- `cargo clippy -p camel-cxf --all-targets -- -D warnings` exits 0.
- `cargo check -p camel-test --tests` exits 0 (struct-literal consumers
  unaffected; no infra run).

- [x] 1.1

### Task 1.2: Document the accepted-keys-per-level contract

**Files:**
- `docs/src/components/ws-soap.md` (modified)
- `crates/components/camel-cxf/src/config.rs` (modified — test only)

**Steps:**
1. In `docs/src/components/ws-soap.md`, in the configuration section, add a
   subsection `#### Accepted configuration keys per level` with a table of
   the four levels and their exact keys, derived from the config structs:
   - `[components.cxf]` (pool): `profiles`, `max_bridges`,
     `bridge_start_timeout_ms`, `health_check_interval_ms`,
     `bridge_cache_dir`, `version`, `bind_address`, `soap_tls_cert_path`,
     `soap_tls_key_path`, `soap_tls_client_ca_path`, `soap_tls_pkcs12_path`,
     `soap_tls_pkcs12_password`, `reconnect`.
   - `[[components.cxf.profiles]]`: `name`, `address`, `wsdl_path`,
     `service_name`, `port_name`, `security`.
   - `profiles.<name>.security`: `keystore_path`, `keystore_password`,
     `truststore_path`, `truststore_password`, `sig_username`,
     `sig_password`, `enc_username`, `security_actions_out`,
     `security_actions_in`, `signature_algorithm`,
     `signature_digest_algorithm`, `signature_c14n_algorithm`,
     `signature_parts`, `allowed_signature_algorithms`,
     `allowed_digest_algorithms`, `allow_expired_pinned_certificate`,
     `timestamp_ttl_seconds`, `timestamp_future_tolerance_seconds`,
     `fault_status`, `exposed_soap_headers`, `tls`.
   - `profiles.<name>.security.tls`: `keystore_path`, `keystore_password`,
     `keystore_type`, `truststore_path`, `truststore_password`,
     `truststore_type`.
2. Below the table, state the contract in prose: an unknown key, or a known
   key in the wrong table, fails the load with an error naming the profile
   (or `[components.cxf]`), the key, and where the key is accepted; keys
   accepted at more than one level (`keystore_path`, `keystore_password`,
   `truststore_path`, `truststore_password`) name every location; the list
   is the contract because it is derived from the config structs and
   enforced fail-closed at load (dead-config-policy). Note explicitly: a
   `listener_tls` table is not TOML surface — use the five flat
   `soap_tls_*` keys; and the `CXF_PROFILE_*` environment-variable surface
   handed to the bridge is unchanged (env vars are not config keys and are
   not validated here).
3. Add a parity test `docs_key_table_matches_enforced_keys` to the existing
   `#[cfg(test)] mod tests` in `crates/components/camel-cxf/src/config.rs`
   (test only — no production change): read
   `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/src/components/ws-soap.md")`
   (three levels up: `crates/components/camel-cxf` → repo root),
   and for each of the four levels assert the documented key list equals the
   enforced list (compare against the same sorted key sets the walker uses:
   `struct_fields::<CxfPoolConfigRaw>()`, `::<CxfProfileConfig>()`,
   `::<CxfSecurityFields>()`, `::<CxfTlsConfig>()`) — parse the doc section
   by locating the level headings and extracting the backticked key tokens,
   or fail with a message stating which level's list diverged. Skip nothing:
   a missing section is a failure.
4. Verify the docs build.

**Tests:**
- `doc-guide-test`: `cd docs && mdbook test` → setup: docs tree as modified;
  action: run mdbook's test pass over the guide; assert exit 0 with no new
  failures introduced by the section (command `cd docs && mdbook test`;
  expected: passes after the change; would also pass before since the
  section adds prose/table only — the change keeps it that way).

**Acceptance:**
- `docs/src/components/ws-soap.md` contains the four-level table and the
  contract paragraph.
- `cargo test -p camel-cxf --lib docs_key_table_matches_enforced_keys`
  exits 0 (documented lists == enforced lists).
- `cd docs && mdbook test` exits 0.

- [x] 1.2
