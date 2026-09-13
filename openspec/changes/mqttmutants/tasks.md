# Tasks: mqttmutants

## camel-component-mqtt configuration tests

### Task 1.1: Cover QoS conversion variants

**Files:**
- `crates/components/camel-mqtt/src/config.rs` (modified)

**Steps:**
1. Add a unit test in the existing `config.rs` test module that evaluates `QosLevel::AtMostOnce`, `QosLevel::AtLeastOnce`, and `QosLevel::ExactlyOnce` through `to_rumqttc`.
2. Assert each result equals `rumqttc::QoS::AtMostOnce`, `rumqttc::QoS::AtLeastOnce`, and `rumqttc::QoS::ExactlyOnce`, respectively.
3. Run the focused MQTT library test command and format the modified file.

**Tests:**
- `qos_level_maps_each_variant_to_rumqttc_qos`: setup the three existing `QosLevel` variants; action call `to_rumqttc` for each; assert the three corresponding `rumqttc::QoS` variants; command `cargo test -p camel-component-mqtt --lib qos_level_maps_each_variant_to_rumqttc_qos`; expected the test passes after implementation and would fail under `Default::default()` because the non-default variants differ.

**Acceptance:**
- The test names and asserts all three mappings explicitly.
- `cargo test -p camel-component-mqtt --lib qos_level_maps_each_variant_to_rumqttc_qos` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: Cover reconnect override and fallback selection

**Files:**
- `crates/components/camel-mqtt/src/config.rs` (modified)

**Steps:**
1. Construct a non-default `NetworkRetryPolicy` for an endpoint override and a different non-default fallback policy using the existing public policy fields.
2. Add a unit test that sets the endpoint reconnect override and asserts `effective_reconnect` returns the override policy.
3. Add a unit test that leaves the endpoint override absent and asserts `effective_reconnect` returns the fallback policy.
4. Run both focused tests and the MQTT clippy command.

**Tests:**
- `effective_reconnect_prefers_endpoint_override`: setup an endpoint with a non-default reconnect policy and a distinct non-default fallback; action call `effective_reconnect`; assert equality with the endpoint policy; command `cargo test -p camel-component-mqtt --lib effective_reconnect_prefers_endpoint_override`; expected pass and failure under `Default::default()`.
- `effective_reconnect_uses_fallback_without_override`: setup an endpoint with no reconnect override and a distinct non-default fallback; action call `effective_reconnect`; assert equality with the fallback; command `cargo test -p camel-component-mqtt --lib effective_reconnect_uses_fallback_without_override`; expected pass and failure under `Default::default()`.

**Acceptance:**
- Both branches are tested with policies distinguishable from `NetworkRetryPolicy::default()`.
- `cargo test -p camel-component-mqtt --lib effective_reconnect_` exits 0.
- `cargo clippy -p camel-component-mqtt --lib -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Cover endpoint keep-alive validation boundary

**Files:**
- `crates/components/camel-mqtt/src/config.rs` (modified)

**Steps:**
1. Add a unit test using `MqttEndpointConfig::default()` plus the existing `#[allow(clippy::field_reassign_with_default)]` pattern with `keep_alive_secs = u64::from(u16::MAX)` and assert validation succeeds.
2. In the same test or a paired test, set `keep_alive_secs = u64::from(u16::MAX) + 1` and assert validation returns an error.
3. Run the focused boundary test and format the modified file.

**Tests:**
- `endpoint_validation_enforces_keep_alive_u16_boundary`: setup two endpoint configurations at `65535` and `65536` seconds; action call `validate` on each; assert the first is `Ok(())` and the second is `Err`; command `cargo test -p camel-component-mqtt --lib endpoint_validation_enforces_keep_alive_u16_boundary`; expected pass and failure if `>` changes to `>=`.

**Acceptance:**
- The exact accepted and rejected boundary values are asserted.
- `cargo test -p camel-component-mqtt --lib endpoint_validation_enforces_keep_alive_u16_boundary` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.3

### Task 1.4: Cover broker URL validation polarities

**Files:**
- `crates/components/camel-mqtt/src/config.rs` (modified)

**Steps:**
1. Add a unit test using the existing `MqttBrokerConfig` struct-literal pattern from `broker_config_requires_url` for `mqtt://localhost:1883` and `mqtts://localhost:8883`, asserting both validate successfully.
2. Add a non-MQTT URL such as `http://localhost:1883` and assert broker validation returns an error.
3. Run the focused broker validation test and the MQTT clippy command.

**Tests:**
- `broker_validation_accepts_mqtt_schemes_and_rejects_other_schemes`: setup broker configs with `mqtt://`, `mqtts://`, and `http://` URLs; action call `validate` for each; assert the first two are `Ok(())` and the last is `Err`; command `cargo test -p camel-component-mqtt --lib broker_validation_accepts_mqtt_schemes_and_rejects_other_schemes`; expected pass and failure if either validation negation is removed.

**Acceptance:**
- Both valid schemes and one invalid scheme are asserted in one broker-only unit test.
- `cargo test -p camel-component-mqtt --lib broker_validation_accepts_mqtt_schemes_and_rejects_other_schemes` exits 0.
- `cargo clippy -p camel-component-mqtt --lib -- -D warnings` exits 0.

- [x] 1.4
