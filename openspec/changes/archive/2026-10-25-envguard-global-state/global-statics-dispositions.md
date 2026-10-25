# `::global()` test-scope disposition audit (mission 403, task 6.1)

Baseline: `258c8ef25` on `fix/envguard-global-state`.

## Method

Inventory command:

```
rg -n "::global\(\)" crates --type rust
```

Raw result: **212 occurrences / 36 files**. Test-scope filter applied per task 6.1:

- path contains `/tests/`, or
- filename matches `*_tests.rs` or `test_*.rs`, or
- the line is inside a `#[cfg(test)]` module/function in a `src/` file.

Result: **184 test-scope occurrences** across **8 production singleton types**:
177 code calls plus 7 documentation references. The remaining **28 production-scope
occurrences** are excluded from this inventory (noted for completeness at the end).

Two inventory notes:

- `crates/camel-cli/src/commands/run_tests.rs` matches the `*_tests.rs` filename rule but is
  production command code. It is retained as a `(c)` row so the filter rule stays honest.
- Documentation references (`(doc)` below) mention `::global()` but do not touch runtime
  state. They are grouped with the static they describe and carry disposition `(c)`.

No `::global()` type is test-only: every `fn global()` definition is an unconditional
production singleton. Consequently there are **zero `(a)` and zero `(b)` dispositions**:
no test-only mutable static exists to convert to `Mutex<Option<T>>`, so no reset-observed
test is required. Grouping is by static + file; every callsite line is listed.

## Dispositions

Definitions: `(a)` migrated to per-test state; `(b)` `#[cfg(test)]`-scoped plus sibling-test
isolation; `(c)` production static, recorded and not redesigned.

| file:line | static | test-scope? | disposition | isolation mechanism (or exclusion reason) |
|---|---|---|---|---|
| `crates/camel-test/tests/support/mod.rs:20` | `ServerRegistry` (http, `crates/components/camel-http/src/lib.rs:940`) | yes | (c) production-excluded | Production `OnceLock`; tests take test-only `reset()` (`lib.rs:1231`) under `REGISTRY_TEST_MUTEX` / `lock_registry_test_mutex()` and use unique ephemeral ports. |
| `crates/camel-test/tests/http_static_test.rs:125, 647, 859` | `ServerRegistry` (http) | yes | (c) production-excluded | Same as above: production registry exercised through its test-only `reset()` and unique ports. |
| `crates/components/camel-http/src/static_endpoint.rs:354, 422, 441, 492, 498` | `ServerRegistry` (http) | yes | (c) production-excluded | Test module calls `crate::lock_registry_test_mutex()` + `ServerRegistry::reset()`; unique ephemeral ports. |
| `crates/components/camel-http/src/lib.rs:1233` (`Self::global()` inside `#[cfg(test)] fn reset`) | `ServerRegistry` (http) | yes | (c) production-excluded | The callsite is the test-only reset helper; the static itself is production. Isolation is the reset + test mutex. |
| `crates/components/camel-http/src/lib_tests.rs:2855, 2856, 2869, 2881, 2917, 2955, 3000, 3015, 3023, 3044, 3058, 3086, 3200, 3245, 3294` | `ServerRegistry` (http) | yes | (c) production-excluded | Production `OnceLock`; tests serialize through `REGISTRY_TEST_MUTEX`, call test-only `reset()`, and use unique ephemeral ports. |
| `crates/components/camel-http/src/lib_tests.rs:3345, 3381, 3434, 3481, 3545, 3598, 3690, 3757, 3861, 4014, 4121, 4230, 4283, 4339, 4439` | `ServerRegistry` (http) | yes | (c) production-excluded | Same isolation: test mutex + `reset()` + unique ports. |
| `crates/components/camel-http/src/lib_tests.rs:4520, 4578, 4637, 4698, 4773, 4836, 4916, 5096, 5167, 5303, 5379, 5438, 5490, 6651, 6695` | `ServerRegistry` (http) | yes | (c) production-excluded | Same isolation: test mutex + `reset()` + unique ports. |
| `crates/components/camel-http/src/lib_tests.rs:8376, 8456, 8951, 9699, 9847` | `ServerRegistry` (http) | yes | (c) production-excluded | Same isolation: test mutex + `reset()` + unique ports. |
| `crates/camel-test/tests/support/mod.rs:42` | `ServerRegistry` (ws, `crates/components/camel-ws/src/lib.rs:154`) | yes | (c) production-excluded | Production `OnceLock` re-exported by the WS component; tests reset via the test-only `reset()` and unique ports. |
| `crates/components/camel-ws/src/lib.rs:3172, 3388, 3418, 3436, 3444, 3604, 3628, 3716, 3740, 3749` | `ServerRegistry` (ws) | yes | (c) production-excluded | Production `OnceLock`; test-only `reset()` aborts tasks and clears entries/staged listeners; tests also use unique ports. |
| `crates/camel-test/tests/mcp_server_auth_test.rs:229, 260` | `McpServerRegistry` (`crates/components/camel-component-mcp/src/registry.rs:226`) | yes | (c) production-excluded | Production `OnceLock` keyed by bind; each test uses a unique loopback host/port and unique route ids. |
| `crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs:69, 334` | `McpServerRegistry` | yes | (c) production-excluded | Unique bind keys per test; registry entries are never reused across tests. |
| `crates/components/camel-component-mcp/tests/server_resource_dispatch_test.rs:67, 181, 223` | `McpServerRegistry` | yes | (c) production-excluded | Unique bind keys per test. |
| `crates/components/camel-component-mcp/tests/server_registry_test.rs:37, 59, 63, 95, 100, 116, 135, 170, 200, 228, 236, 252, 260, 281, 290` | `McpServerRegistry` | yes | (c) production-excluded | Unique loopback bind per test (`127.0.0.x:0`); no shared key collision. |
| `crates/components/camel-component-mcp/tests/server_registry_test.rs:934, 979, 1006, 1044` | `McpServerRegistry` | yes | (c) production-excluded | Unique `bind` strings per test. |
| `crates/components/camel-component-mcp/tests/server_protocol_test.rs:53, 403, 444, 581` | `McpServerRegistry` | yes | (c) production-excluded | Unique bind keys per test. |
| `crates/components/camel-component-mcp/tests/server_consumer_test.rs:151, 165, 184, 245, 292, 439, 553, 601, 644, 720, 858, 892` | `McpServerRegistry` | yes | (c) production-excluded | Unique binds; bind-ack map replaced and explicitly reset to empty after mutation (`:165`). |
| `crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs:154, 327` | `McpServerRegistry` | yes | (c) production-excluded | Unique bind keys per test. |
| `crates/components/camel-component-mcp/tests/server_config_test.rs:133` | `McpServerRegistry` | yes | (c) production-excluded | Fail-closed reset of the global ack map; unique bind keys. |
| `crates/components/camel-component-mcp/tests/dsl_e2e_test.rs:195, 316` | `McpServerRegistry` | yes | (c) production-excluded | Unique bind keys per test. |
| `crates/camel-core/tests/tls_reload_test.rs:4 (doc), 70, 103, 143, 165, 184, 209` | `TlsReloadRegistry` (`crates/components/camel-component-api/src/tls_source.rs:158`) | yes | (c) production-excluded | Production registry; tests register with RAII guards and `unregister` on unique `(scheme, host, port)` keys. Line 4 is a doc reference. |
| `crates/components/camel-component-grpc/tests/integration.rs:1499 (doc), 1509, 1521, 1645, 1651` | `TlsReloadRegistry` | yes | (c) production-excluded | Unique `(scheme, host, port)` keys; explicit `unregister` cleanup. Line 1499 is a doc reference. |
| `crates/components/camel-ws/src/lib.rs:3727 (doc), 3730, 3741, 3743, 3750, 5967, 5968, 6042, 6043, 6077` | `TlsReloadRegistry` | yes | (c) production-excluded | Uses unique ports and explicit `unregister`; assertions verify no leak from isolated registries. Line 3727 is a doc reference. |
| `crates/components/camel-component-api/src/tls_source.rs:370, 378, 383, 385, 398` | `TlsReloadRegistry` | yes | (c) production-excluded | Production singleton asserted for pointer identity / RAII registration under unique keys; no redesign. |
| `crates/camel-core/tests/template_reload_test.rs:5 (doc), 132, 161, 239, 312` | `TemplateReloadRegistry` (`crates/components/camel-component-api/src/template_reload.rs:93`) | yes | (c) production-excluded | Production registry; RAII `register` guard unregisters by unique route id on drop. Line 5 is a doc reference. |
| `crates/components/camel-template/tests/template_reload_integration.rs:24 (doc), 117 (doc), 120, 129` | `TemplateReloadRegistry` | yes | (c) production-excluded | Unique route ids per test; RAII guard cleanup. Lines 24/117 are doc references. |
| `crates/components/camel-component-api/src/template_reload.rs:367, 378, 409, 428, 455, 476` | `TemplateReloadRegistry` | yes | (c) production-excluded | Production singleton; tests use unique ids and RAII guards. |
| `crates/components/camel-template/src/lifecycle.rs:296, 328` | `TemplateReloadRegistry` | yes | (c) production-excluded | Tests use unique route ids; start registers and shutdown drops the guard (unregister by id). |
| `crates/components/camel-component-grpc/src/server.rs:913, 914, 1030, 1260, 1278, 1331, 1353, 2378, 2397, 2419` | `GrpcServerRegistry` (`crates/components/camel-component-grpc/src/server.rs:97`) | yes | (c) production-excluded | Production `Mutex<HashMap>` keyed by host/port; tests use unique loopback/unused ephemeral ports and the test-only `contains_server` probe. |
| `crates/components/camel-component-grpc/src/consumer.rs:1497, 1557` | `GrpcServerRegistry` | yes | (c) production-excluded | Unique unused ephemeral ports; rejects-before-registration assertions. |
| `crates/camel-cli/tests/compiled_asset_roundtrip_test.rs:513` | `AssetRegistry` (`crates/camel-bundles/src/asset_registry.rs:188`) | yes | (c) production-excluded | Immutable `OnceLock<HashMap>`; read-only assertion in its own integration-test process. |
| `crates/camel-bundles/tests/asset_registry_population_test.rs:33` | `AssetRegistry` | yes | (c) production-excluded | Population happens once in this dedicated integration-test binary; immutable afterwards, no reset required. |
| `crates/camel-bundles/src/asset_registry.rs:311` | `AssetRegistry` | yes | (c) production-excluded | `--lib` test asserts the process-global registry stays unpopulated; private test process, immutable read-only. |
| `crates/components/camel-component-wasm/tests/source_bind_gate.rs:65 (doc), 361, 401, 427, 549` | `WasmSourceBindAcks` (`crates/components/camel-component-wasm/src/lib.rs:178`) | yes | (c) production-excluded | Production `OnceLock<RwLock<HashMap>>`; `set` replaces the map, tests serialize through the test-local `ACK_TEST_LOCK` and reset to empty. Line 65 is a doc reference. |
| `crates/camel-bundles/src/security_boot.rs:622` | `WasmSourceBindAcks` | yes | (c) production-excluded | Unique test bind constant; production installer exercised once in the lib test. |
| `crates/camel-cli/src/commands/run_tests.rs:282` | `WasmSourceBindAcks` | yes (path pattern) | (c) production-excluded | Filename matches `*_tests.rs` but the callsite is production `run_tests` command code (the test runner's `wasm_bind_acks_wired_from_config`); no test isolation applies. |

## Coverage summary

| static | test-scope occurrences |
|---|---|
| `ServerRegistry` (http) | 60 |
| `ServerRegistry` (ws) | 11 |
| `McpServerRegistry` | 47 |
| `TlsReloadRegistry` | 27 |
| `TemplateReloadRegistry` | 17 |
| `GrpcServerRegistry` | 12 |
| `WasmSourceBindAcks` | 7 |
| `AssetRegistry` | 3 |
| **total** | **184** |

Every line from the filtered `rg` inventory appears in the table above. Disposition counts:
**(a) 0, (b) 0, (c) 184**.

## Migrations applied

None. All test-scope callsites resolve to production singletons, so the scope rule forbids
redesigning them. No `Mutex<Option<T>>` conversion was warranted and therefore no
`<static>_reset_observes_between_tests` test was added. Existing test-side isolation
mechanisms are recorded in the table.

## Production-scope occurrences excluded (28)

These are the same statics read from production code paths, not redesigned:

- `crates/camel-integration-test/src/inbound.rs:36` — `ServerRegistry` (http).
- `crates/camel-bundles/src/security_boot.rs:54, 61` — MCP / wasm bind-ack installers.
- `crates/camel-bundles/src/asset_registry.rs:178, 179` — doc references.
- `crates/camel-cli/src/compile/runtime.rs:507` — `AssetRegistry`.
- `crates/components/camel-component-grpc/src/server.rs:182, 192, 265, 275` — TLS reload registration/comments.
- `crates/components/camel-component-grpc/src/consumer.rs:658, 911, 933` — `GrpcServerRegistry`.
- `crates/components/camel-component-api/src/tls_source.rs:131, 143` — doc references.
- `crates/components/camel-component-mcp/src/consumer.rs:230` — `McpServerRegistry`.
- `crates/components/camel-http/src/tls_reload.rs:17` — doc reference.
- `crates/components/camel-http/src/static_endpoint.rs:169` — `ServerRegistry` (http).
- `crates/components/camel-http/src/lib.rs:1081, 1360, 1369, 1870, 2242, 3435` — TLS/HTTP registry reads and docs.
- `crates/components/camel-component-wasm/src/source_consumer.rs:284` — `WasmSourceBindAcks`.
- `crates/camel-core/src/lifecycle/application/runtime_bus.rs:195, 231` — TLS/template reload dispatch.
- `crates/components/camel-template/src/lifecycle.rs:141` — `TemplateReloadRegistry` registration.
