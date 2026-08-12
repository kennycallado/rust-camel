# Tasks: uri-config-skip-impl-required

## camel-endpoint-macros

### Task 1: Add `descriptor` attribute + thread into required inference

Implements the contract change blessed by e_gpt round-3. The new bare-ident
`descriptor` flag on `#[uri_config(..)]` suppresses shape-based `required`
inference for the struct's `#[uri_param]` fields.

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)

**Steps:**
1. Add `descriptor: bool` field to the `UriConfigAttr` struct (at line ~197, alongside `skip_impl: bool`).
2. In `parse_uri_config_attr` (at line ~199), add `let mut descriptor = false;` initialization alongside `let mut skip_impl = false;`.
3. In the `parse_nested_meta` closure (at line ~218, alongside the `if meta.path.is_ident("skip_impl")` arm), add a parallel arm: `if meta.path.is_ident("descriptor") { descriptor = true; return Ok(()); }`.
4. In the `UriConfigAttr` struct literal returned at the end of `parse_uri_config_attr`, populate the new `descriptor` field with the local variable.
5. The per-field `required` computation lives in `build_uri_option_entry` (defined at line 750, signature `(field_ident, field_type, attr: &UriParamAttr, endpoint_crate)`). This is a SEPARATE function from `impl_uri_config` (line ~889) where the struct-level config is parsed. The `descriptor` flag therefore needs to be threaded as a parameter:
   - Add `is_descriptor: bool` as a new parameter to `build_uri_option_entry` (append after `endpoint_crate: &syn::Path`).
   - At the single call site (line 1083, inside `impl_uri_config`'s loop body), pass the new argument: `build_uri_option_entry(field_name, field_type, attr, &endpoint_crate, uri_config_attr.descriptor)`. The `uri_config_attr` binding is in scope at the call site (it is the parsed `UriConfigAttr` from step 1's `parse_uri_config_attr`).
   - Inside `build_uri_option_entry`, the local `is_descriptor` parameter is now in scope at the per-field required computation (line ~858).
6. Replace the `required` computation with:
   ```rust
   let required = if attr.pattern.is_some() {
       false
   } else if attr.required {
       true
   } else if is_descriptor {
       false
   } else {
       !is_option && attr.default.is_none()
   };
   ```
   where `is_descriptor` is the threaded `descriptor` flag (rename to match the local binding).
7. Add 8 unit tests in the `#[cfg(test)] mod tests` block at the bottom of the file. Use the existing test helpers (`parse_attr` and friends) to construct `#[uri_config(..)]` attribute strings and assert the resulting `UriConfigAttr.descriptor` field, AND derive-test scenarios that assert the generated `UriOption.required` for each case.

**Tests:** (executable spec — name, setup, action, assert)
- `descriptor_flag_parses_as_bare_ident`: setup = the attribute string `#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]`; action = parse via the existing `parse_uri_config_attr` test helper; assert = the returned `UriConfigAttr.descriptor == true`.
- `absent_descriptor_defaults_to_false`: setup = the attribute string `#[uri_config(skip_impl, metadata(scheme = "x"))]`; action = parse; assert = `UriConfigAttr.descriptor == false`.
- `descriptor_with_bare_non_option_field_is_not_required`: setup = a test-only struct deriving `UriConfig` with `#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]` and a field `#[uri_param(name = "foo")] pub _foo: String`; action = call the generated `uri_options()` (or `metadata()`); assert = the `foo` entry's `required` flag is `false`.
- `descriptor_with_explicit_required_stays_required`: setup = same struct shape but field is `#[uri_param(name = "foo", required)] pub _foo: String`; action = call `uri_options()`; assert = `foo` entry's `required` flag is `true`.
- `runtime_config_without_descriptor_retains_shape_inference`: setup = a test-only struct deriving `UriConfig` with `#[uri_config(skip_impl, metadata(scheme = "x"))]` (NO `descriptor`) and a field `#[uri_param(name = "foo")] pub foo: String`; action = call `uri_options()`; assert = `foo` entry's `required` flag is `true` (existing behavior preserved).
- `descriptor_with_pattern_field_is_not_required`: setup = a struct with `#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]` and a field `#[uri_param(pattern = "param.")] pub _params: Vec<(String, String)>`; action = call `uri_options()`; assert = the entry's `required` flag is `false`.
- `descriptor_with_default_field_is_not_required`: setup = a struct with `#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]` and a field `#[uri_param(name = "period", default = "1000")] pub _period: u64`; action = call `uri_options()`; assert = the entry's `required` flag is `false`.
- `descriptor_with_option_field_is_not_required`: setup = a struct with `#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]` and a field `#[uri_param(name = "password")] pub _password: Option<String>`; action = call `uri_options()`; assert = the entry's `required` flag is `false`.

**Acceptance:**
- `cargo test -p camel-endpoint-macros` passes all 8 new tests (6 originally listed + 2 added for default/Option coverage) + existing tests.
- `cargo clippy -p camel-endpoint-macros -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0 (no new formatting drift).
- The `required` computation matches the 4-branch form (machine-verifiable: `rg -n 'else if is_descriptor' crates/camel-endpoint-macros/src/uri_config.rs` returns exactly one match, inside the per-field required computation block).
- **Runtime-config regression safety net**: `cargo test -p camel-timer -p camel-cron -p camel-sql -p camel-opensearch -p camel-container -p camel-http` all still pass (these runtime-config crates do NOT set `descriptor`; their shape inference must be byte-identical). Note: camel-timer, camel-cron, camel-sql have no metadata parity tests today, so this acceptance criterion is the only regression net for them — do not skip it.

- [x] 1

## camel-jms

### Task 2: Migrate JmsMetadataDescriptor — descriptor flag + 3 runtime defaults

**Files:**
- `crates/components/camel-jms/src/metadata.rs` (modified)

**Steps:**
1. Add the `descriptor` bare-ident to the `#[uri_config(..)]` attribute on `JmsMetadataDescriptor` (currently at lines 6-15). The attribute becomes `#[uri_config(skip_impl, descriptor, metadata(scheme = "jms", description = "JMS / ActiveMQ / Artemis messaging", producer, consumer), crate = "camel_component_api")]`.
2. Change `#[uri_param(name = "acknowledgementMode")]` to `#[uri_param(name = "acknowledgementMode", default = "Auto")]`. The string `"Auto"` is the exact token the runtime `AcknowledgementMode::from_str` accepts (verified at `crates/components/camel-jms/src/config.rs:90-95`).
3. Change `#[uri_param(name = "transactionMode")]` to `#[uri_param(name = "transactionMode", default = "None")]`. The string `"None"` is the exact token the runtime `JmsTransactionMode::from_str` accepts (verified at config.rs:133-137).
4. Change `#[uri_param(name = "exchangePattern")]` to `#[uri_param(name = "exchangePattern", default = "InOnly")]`. The string `"InOnly"` is the exact token the runtime `ExchangePattern::from_str` accepts (verified at config.rs:173-177).
5. In the `jms_metadata_uri_options_parity` test (currently at line 53), extend it to assert the `required` flag for EVERY `uri_option` entry, AND assert `default_value` for the 3 new defaults. Add a helper `fn find(meta: &[UriOption], name: &str) -> &UriOption` that panics if not found (or reuse the existing inline `iter().find(...).unwrap()` pattern). Add these assertions after the existing name-parity check:
   - `assert!(!find(&meta.uri_options, "broker").required)` — Option<String> field.
   - `assert!(find(&meta.uri_options, "acknowledgementMode").default_value.as_deref() == Some("Auto"))` AND `assert!(!find(...).required)`.
   - `assert!(!find(&meta.uri_options, "messageSelector").required)` — Option<String>.
   - `assert_eq!(find(&meta.uri_options, "concurrentConsumers").default_value.as_deref(), Some("1"))` (existing assertion, keep).
   - `assert!(find(&meta.uri_options, "transactionMode").default_value.as_deref() == Some("None"))` AND `assert!(!find(...).required)`.
   - `assert!(!find(&meta.uri_options, "timeToLive").required)` — Option.
   - `assert!(!find(&meta.uri_options, "priority").required)` — Option.
   - `assert_eq!(find(&meta.uri_options, "persistentDelivery").default_value.as_deref(), Some("true"))` (existing).
   - `assert_eq!(find(&meta.uri_options, "mapJmsHeaders").default_value.as_deref(), Some("true"))` (existing).
   - `assert!(find(&meta.uri_options, "exchangePattern").default_value.as_deref() == Some("InOnly"))` AND `assert!(!find(...).required)`.
   - Additionally assert `required == false` for the 3 defaulted fields (the `default` already implies not-required, but the explicit assertion makes the disposition executable).

**Tests:**
- `jms_metadata_uri_options_parity` (extended): setup = the migrated `JmsMetadataDescriptor` derives `UriConfig` with `descriptor` flag + 3 new defaults; action = call `JmsMetadataDescriptor::metadata()` and inspect each `uri_option`; assert = every entry has `required` flag asserted true OR false explicitly; the 3 defaulted fields have `default_value` of `"Auto"` / `"None"` / `"InOnly"` respectively and `required == false`.
- Regression check via `cargo test -p camel-jms --lib`: all existing tests still pass.

**Acceptance:**
- `cargo test -p camel-jms --lib` passes including the extended `jms_metadata_uri_options_parity`.
- `cargo clippy -p camel-jms -- -D warnings` (run via the workspace clippy invocation per AGENTS.md quality gates — camel-jms is not in the exclude list) exits 0.
- The `descriptor` flag is present in the `#[uri_config(..)]` attribute on `JmsMetadataDescriptor`.
- The 3 defaulted fields' `default_value` matches the runtime `FromStr` accept-set. Machine-verifiable: `rg -n 'default = "(Auto|None|InOnly)"' crates/components/camel-jms/src/metadata.rs` returns exactly 3 matches (one per field).

- [x] 2

## camel-cxf

### Task 3: Migrate CxfMetadataDescriptor — descriptor flag + profile required + attachment Option

**Files:**
- `crates/components/camel-cxf/src/metadata.rs` (modified)

**Steps:**
1. Add the `descriptor` bare-ident to the `#[uri_config(..)]` attribute on `CxfMetadataDescriptor` (currently at lines 6-15). The attribute becomes `#[uri_config(skip_impl, descriptor, metadata(scheme = "cxf", description = "CXF/SOAP WebService consumer/producer", producer, consumer), crate = "camel_component_api")]`.
2. Change the `profile` field's `#[uri_param(name = "profile")]` to `#[uri_param(name = "profile", required)]`. Justification: the runtime at `crates/components/camel-cxf/src/component.rs:67-68` returns `CamelError::ProcessorError("cxf URI requires 'profile' query parameter")` when `profile` is absent. Under the new `descriptor` rule, the implicit-by-shape inference disappears; the explicit annotation preserves the correct `R-URI-known:missing-required-option` diagnostic.
3. Change the `attachment_content_type` field type from `pub _attachment_content_type: String` to `pub _attachment_content_type: Option<String>`. Justification: the runtime config field at `crates/components/camel-cxf/src/config.rs:238` is `Option<String>` defaulting to `None`, consumed only when `mtom_enabled` is true. The `#[uri_param(name = "attachment_content_type")]` attribute stays the same.
4. Leave the `operation` field unchanged (`#[uri_param(name = "operation")] pub _operation: String`). Under the new `descriptor` rule it becomes `required = false` automatically, matching the runtime parser (config.rs:228,292,306 stores `Option<String>` defaulting to `None`; the `CamelCxfOperation` header may override at dispatch time).
5. In the `cxf_metadata_uri_options_parity` test (currently at line 47), extend it to assert the `required` flag for EVERY `uri_option` entry. Add the same `find` helper pattern as Task 2. Add these assertions after the existing name + required-for-wsdl/service/port checks:
   - `assert!(find(&meta.uri_options, "wsdl").required)` (existing — keep).
   - `assert!(find(&meta.uri_options, "service").required)` (existing — keep).
   - `assert!(find(&meta.uri_options, "port").required)` (existing — keep).
   - `assert!(find(&meta.uri_options, "profile").required)` — NEW explicit assertion.
   - `assert!(!find(&meta.uri_options, "operation").required)` — NEW.
   - `assert!(!find(&meta.uri_options, "timeout_ms").required)` — NEW (Option field).
   - `assert!(!find(&meta.uri_options, "mtom_enabled").required)` — NEW (has default).
   - `assert!(!find(&meta.uri_options, "attachment_content_type").required)` — NEW (now Option<String>).

**Tests:**
- `cxf_metadata_uri_options_parity` (extended): setup = the migrated `CxfMetadataDescriptor` derives `UriConfig` with `descriptor` flag + `profile` explicit-required + `attachment_content_type` as `Option<String>`; action = call `CxfMetadataDescriptor::metadata()`; assert = every entry has `required` flag explicitly asserted; `profile.required == true`; `operation.required == false`; `attachment_content_type.required == false`.
- Regression check via `cargo test -p camel-cxf --lib`: all existing tests still pass.

**Acceptance:**
- `cargo test -p camel-cxf --lib` passes including the extended `cxf_metadata_uri_options_parity`.
- `cargo clippy -p camel-cxf -- -D warnings` (run via the workspace clippy invocation — camel-cxf is not excluded) exits 0.
- The `descriptor` flag is present in the `#[uri_config(..)]` attribute.
- `profile` carries explicit `required`. Machine-verifiable: `rg -n 'name = "profile", required' crates/components/camel-cxf/src/metadata.rs` returns exactly 1 match.
- `attachment_content_type` is `Option<String>`. Machine-verifiable: `rg -n '_attachment_content_type: Option<String>' crates/components/camel-cxf/src/metadata.rs` returns exactly 1 match.

- [x] 3

## examples

### Task 4: Fix 3 example route files (controlbus authorizedRoutes + cxf profile)

These are real-defect corrections per ADR-0032 (controlbus fail-closed
allowlist) and runtime enforcement (cxf profile).

**Files:**
- `examples/camel-cli-run/routes/jms.yaml` (modified)
- `examples/master-leader-yaml/routes/master.yaml` (modified)
- `examples/cxf-example/routes/soap-producer.yaml` (modified)
- `examples/cxf-example/Camel.toml` (modified — fix `services` → `profiles` schema bug)

**Steps:**
1. In `examples/camel-cli-run/routes/jms.yaml`, locate the 5 `controlbus:route?routeId=...&action=...` URIs (lines 32, 33, 34, 53, 54 per the lint output). For each, append `&authorizedRoutes=<target_routeId>` where `<target_routeId>` is the routeId named in the existing `routeId=` parameter of that same URI. Specifically:
   - Line 32: `controlbus:route?routeId=artemis-ready-watcher&action=stop` → append `&authorizedRoutes=artemis-ready-watcher`.
   - Line 33: `controlbus:route?routeId=jms-producer&action=start` → append `&authorizedRoutes=jms-producer`.
   - Line 34: `controlbus:route?routeId=jms-consumer&action=start` → append `&authorizedRoutes=jms-consumer`.
   - Line 53: same as line 33 (routeId=jms-producer&action=start).
   - Line 54: same as line 34 (routeId=jms-consumer&action=start).
2. In `examples/master-leader-yaml/routes/master.yaml`, locate the 1 `controlbus:route` URI (line 16: `controlbus:route?routeId=master-route_1&action=status`). Append `&authorizedRoutes=master-route_1`.
3. In `examples/cxf-example/routes/soap-producer.yaml`, the `cxf://` URI at line 8 currently lacks `profile`. Append `&profile=hello` to the URI. The profile name `"hello"` matches `[a-z0-9_]+` per `validate_profile_name` at `crates/components/camel-cxf/src/config.rs:31`.
4. The existing `examples/cxf-example/Camel.toml` declares a `[[components.cxf.services]]` section — this is a SCHEMA BUG: the cxf bundle's `CxfBundle::from_toml` (at `crates/components/camel-cxf/src/bundle.rs:29-33`) deserializes into `CxfPoolConfig`, which expects `profiles` (per `CxfPoolConfig.profiles: Vec<CxfProfileConfig>` at config.rs:117-119 and the test fixtures at config.rs:444,478,503 using `[[profiles]]`). The `services` key is silently ignored (or rejected depending on serde strictness), so the example does not run today. Fix it by replacing the entire `[[components.cxf.services]]` block with a `[[components.cxf.profiles]]` block carrying the 4 required `CxfProfileConfig` fields (config.rs:99-105: `name: String`, `address: Option<String>`, `wsdl_path: String`, `service_name: String`, `port_name: String`). The resulting Camel.toml `[components.cxf]` section SHALL be:
   ```toml
   [components.cxf]
   version = "0.8.1"

   [[components.cxf.profiles]]
   name = "hello"
   wsdl_path = "wsdl/hello.wsdl"
   service_name = "{http://example.com/hello}HelloService"
   port_name = "{http://example.com/hello}HelloPort"
   ```
   The `address` field is `Option<String>` and MAY be omitted (the URI `cxf://http://localhost:8080/hello?...` supplies the address). The `name = "hello"` matches the URI's `profile=hello` parameter (step 3).

**Tests:**
- `lint_corpus_jms_yaml_no_missing_required`: setup = the migrated jms + cxf descriptors (Tasks 2, 3 complete); action = run `cargo test -p camel-cli --test lint_corpus`; assert = `examples/camel-cli-run/routes/jms.yaml` does NOT appear in any FALSE-POSITIVE or MISSING-REGRESSION failure (the gate passes for this file).
- `lint_corpus_master_yaml_no_missing_required`: same setup; action = run the corpus gate; assert = `examples/master-leader-yaml/routes/master.yaml` does NOT appear in any failure.
- `lint_corpus_soap_producer_no_missing_required`: same setup; action = run the corpus gate; assert = `examples/cxf-example/routes/soap-producer.yaml` does NOT appear in any failure for `R-URI-known:missing-required-option` (it MAY still emit `R-SCHEMA` per the existing baseline entry — that is unchanged).
- `camel_lint_jms_yaml_clean_for_missing_required`: action = run `cargo run -p camel-cli -- lint examples/camel-cli-run/routes/jms.yaml`; assert = zero `R-URI-known:missing-required-option` diagnostics in the output.

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` passes (the full corpus gate is green; no baseline changes).
- For each of the 3 files, `cargo run -p camel-cli -- lint <file>` exits 0 AND the stdout contains zero lines matching `R-URI-known:missing-required-option`. Machine-verifiable: `cargo run -p camel-cli -- lint examples/camel-cli-run/routes/jms.yaml 2>&1 | rg -c 'R-URI-known:missing-required-option' || true` prints `0` (or empty); repeat for the other 2 files.
- The 5 jms.yaml controlbus URIs + 1 master.yaml controlbus controlbus URIs + 1 soap-producer.yaml cxf URI carry their respective required parameters. Machine-verifiable: `rg -c 'controlbus:route\?routeId=[^&]+&action=[^&]+&authorizedRoutes=' examples/camel-cli-run/routes/jms.yaml` returns `5`; `rg -c 'authorizedRoutes=master-route_1' examples/master-leader-yaml/routes/master.yaml` returns `1`; `rg -c 'profile=hello' examples/cxf-example/routes/soap-producer.yaml` returns `1`.
- The cxf-example `Camel.toml` defines a `[[components.cxf.profiles]]` entry with `name = "hello"` matching the URI's `profile=hello` parameter. Machine-verifiable: `rg -c '\[\[components\.cxf\.profiles\]\]' examples/cxf-example/Camel.toml` returns `1`; `rg -c 'name = "hello"' examples/cxf-example/Camel.toml` returns `1`; `rg -c '\[\[components\.cxf\.services\]\]' examples/cxf-example/Camel.toml || true` returns `0` or empty (the old schema is removed).

- [x] 4

## camel-endpoint + bd

### Task 5: Document `descriptor` attribute + file 5 follow-up bd issues

**Files:**
- `crates/camel-endpoint/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-endpoint/CONTEXT.md`, locate the `## UriConfig derive contract` section (or equivalent — search for `skip_impl` to find the existing contract documentation). Add a new sub-section documenting the `descriptor` attribute. The text SHALL state:
   - The `descriptor` bare-ident flag on `#[uri_config(..)]` suppresses shape-based `required` inference for the struct's `#[uri_param]` fields.
   - When `descriptor` is set, a field is `required = true` ONLY with explicit `#[uri_param(..., required)]`.
   - When `descriptor` is absent (default), shape inference is unchanged: `non-Option + no default => required`.
   - The flag is opt-in: it does NOT categorically distinguish metadata-only descriptors from runtime configs. Authors declare intent at the struct level.
   - Runtime config structs (including `skip_impl` configs like `TimerConfig`, `CronConfig`, `SqlUriConfig`, `OpenSearchUriConfig`, `ContainerUriConfig`, `HttpStaticUriConfig`) MUST NOT set `descriptor`.
   - Cross-link to bd rc-1pfm and note the e_opus ruling (KB source `rc-1pfm-eopus-ruling`).
2. From the repo ROOT (`/home/kenny/dev/rust-camel`, NOT the worktree), file 5 bd follow-up issues, one per unmigrated descriptor. Each issue SHALL:
   - Have title: `Migrate <DescriptorName> to descriptor attribute (rc-1pfm follow-up)`.
   - Have type `task`, priority `3` (low — no known false positives today).
   - Have `--deps discovered-from:rc-1pfm`.
   - List the descriptor's bare fields (by name) and the migration step (add `descriptor` flag + per-field runtime audit + extend the parity test with per-field `required` assertions).
   - The 5 issues:
     - `GrpcMetadataDescriptor` — 14 bare fields: `service`, `method`, `metadata`, `caCertPath`, `clientCertPath`, `clientKeyPath`, `serverName`, `serverCertPath`, `serverKeyPath`, `clientCaPath`, `bearerToken`, `googleServiceAccount`, `consumerStrategy`, `producerStrategy`.
     - `KafkaMetadataDescriptor` — 12 bare fields: `groupId`, `autoOffsetReset`, `saslUsername`, `saslPassword`, `sslKeystoreLocation`, `sslKeystorePassword`, `sslTruststoreLocation`, `sslTruststorePassword`, `clientId`, `brokerName`, `isolationLevel`, `dlqTopic`.
     - `MqttMetadataDescriptor` — 2 bare fields: `topics`, `clientId`.
     - `RedisMetadataDescriptor` — 2 bare fields: `channels`, `password`.
     - `ValidatorMetadataDescriptor` — 2 bare fields: `type`, `headerName`.

**Tests:**
- `descriptor_attribute_documented`: setup = the updated CONTEXT.md; action = run `rg -n -A 2 '## UriConfig derive contract' crates/camel-endpoint/CONTEXT.md | rg -c 'descriptor'`; assert = the count is `>= 1` (the word `descriptor` appears within the UriConfig derive contract section, not just anywhere in the file). The exact command: `awk '/## UriConfig derive contract/,/^## [A-Z]/' crates/camel-endpoint/CONTEXT.md | rg -c 'descriptor'` returns `>= 1`.
- `five_followup_bd_issues_filed`: setup = the 5 bd issues filed from repo root via `bd create` (each with `--json` to capture the returned id); action = for each returned id, run `bd show <id> --json`; assert = each issue has `"status": "open"`, a title matching `Migrate <DescriptorName> to descriptor attribute (rc-1pfm follow-up)`, AND a dependency entry with `"title": "rc-1pfm"` and `"dependency_type": "discovered-from"`. The 5 `<DescriptorName>` values are: `GrpcMetadataDescriptor`, `KafkaMetadataDescriptor`, `MqttMetadataDescriptor`, `RedisMetadataDescriptor`, `ValidatorMetadataDescriptor`.

**Acceptance:**
- `crates/camel-endpoint/CONTEXT.md` contains a `descriptor` sub-section under the UriConfig derive contract.
- 5 open bd issues exist with the titles and `discovered-from:rc-1pfm` dependency. Machine-verifiable: for each of the 5 expected titles, `bd show <id> --json | jq -e '.[0].status == "open" and .[0].dependencies[] | select(.dependency_type == "discovered-from" and .id == "rc-1pfm")'` returns `true` (where `<id>` is the id returned by the corresponding `bd create` call in step 2).
- `cargo xtask lint-context-citations` passes (the CONTEXT.md change must not break context-citation invariants — verify the new section does not introduce un-cited symbols).

- [x] 5
