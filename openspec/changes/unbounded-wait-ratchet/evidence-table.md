# Evidence: unbounded-wait-ratchet (mission 394, bd rc-yfsw6)

Baseline 299 findings (main 3dfada1a7) -> 0 findings at ceiling 0. Totals: A(bounded)=54 B(marker)=245 C(production)=0 (54/245/0) across 98 files.

| file | sites | bucket split (A/B) | action |
|---|---|---|---|
| crates/camel-cli/src/commands/test/scenario_tests.rs | 1 | 1/0 | 1 timeout |
| crates/camel-core/src/health_registry.rs | 1 | 0/1 | 1 marker |
| crates/camel-core/src/lifecycle/adapters/consumer_management.rs | 1 | 0/1 | 1 marker |
| crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 3 | 0/3 | 3 marker |
| crates/camel-core/src/lifecycle/adapters/inline_dispatcher_tests.rs | 1 | 0/1 | 1 marker |
| crates/camel-core/src/lifecycle/adapters/redb_journal.rs | 1 | 1/0 | 1 timeout |
| crates/camel-core/tests/arc_snapshot_concurrency.rs | 2 | 0/2 | 2 marker |
| crates/camel-core/tests/cb_fallback_lifecycle_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-dsl/tests/rest_stream_contract_e2e.rs | 1 | 1/0 | 1 timeout |
| crates/camel-integration-test/src/boot_scenario_test.rs | 1 | 1/0 | 1 timeout |
| crates/camel-integration-test/src/http_partner_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-integration-test/src/log_capture.rs | 1 | 0/1 | 1 marker |
| crates/camel-integration-test/src/runner/partner_validate_test.rs | 1 | 1/0 | 1 timeout |
| crates/camel-integration-test/src/sql_validate_test.rs | 3 | 0/3 | 3 marker |
| crates/camel-integration-test/tests/http_inbound_test.rs | 5 | 2/3 | 2 timeout, 3 marker |
| crates/camel-integration-test/tests/http_outbound_test.rs | 7 | 0/7 | 7 marker |
| crates/camel-integration-test/tests/http_partner_scripting_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-integration-test/tests/log_assertion_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-integration-test/tests/partner_verification_test.rs | 7 | 6/1 | 6 timeout, 1 marker |
| crates/camel-lsp/src/lib.rs | 1 | 0/1 | 1 marker |
| crates/camel-processor/src/cache_eip.rs | 5 | 0/5 | 5 marker |
| crates/camel-processor/src/streaming_splitter.rs | 1 | 0/1 | 1 marker |
| crates/camel-processor/src/wire_tap.rs | 2 | 0/2 | 2 marker |
| crates/camel-test/src/harness.rs | 3 | 0/3 | 3 marker |
| crates/camel-test/src/time.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/cache_admin_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/cache_eip_smoke.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/direct_top_level_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/do_try_test.rs | 2 | 0/2 | 2 marker |
| crates/camel-test/tests/hot_reload_integration.rs | 4 | 4/0 | 4 timeout |
| crates/camel-test/tests/integration_test.rs | 13 | 2/11 | 2 timeout, 11 marker |
| crates/camel-test/tests/jsonpath_test.rs | 3 | 0/3 | 3 marker |
| crates/camel-test/tests/late_registration_gate_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/loop_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/mcp_server_auth_test.rs | 1 | 1/0 | 1 timeout |
| crates/camel-test/tests/mtls_bridge_test.rs | 1 | 1/0 | 1 timeout |
| crates/camel-test/tests/opensearch_test.rs | 13 | 0/13 | 13 marker |
| crates/camel-test/tests/otel_direct_hop_regression.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/otel_trace_tree_test.rs | 3 | 0/3 | 3 marker |
| crates/camel-test/tests/redis_test.rs | 9 | 0/9 | 9 marker |
| crates/camel-test/tests/script_test.rs | 7 | 0/7 | 7 marker |
| crates/camel-test/tests/seda_test.rs | 4 | 0/4 | 4 marker |
| crates/camel-test/tests/sql_test.rs | 13 | 1/12 | 1 timeout, 12 marker |
| crates/camel-test/tests/surrealdb_test.rs | 13 | 0/13 | 13 marker |
| crates/camel-test/tests/throttle_test.rs | 2 | 0/2 | 2 marker |
| crates/camel-test/tests/tracer_test.rs | 1 | 0/1 | 1 marker |
| crates/camel-test/tests/xpath_test.rs | 3 | 0/3 | 3 marker |
| crates/components/camel-component-api/src/network_retry_tests.rs | 3 | 0/3 | 3 marker |
| crates/components/camel-component-grpc/src/server.rs | 14 | 0/14 | 14 marker |
| crates/components/camel-component-grpc/tests/integration.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-component-grpc/tests/server_auth_test.rs | 7 | 0/7 | 7 marker |
| crates/components/camel-component-llm/src/producer_semaphore_tests.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-component-llm/src/producer_timeout_tests.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-component-llm/src/producer_tools_tests.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-component-llm/tests/edge_cases.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-component-llm/tests/ollama_live.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-component-llm/tests/producer_cache.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-component-mcp/tests/dsl_e2e_test.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 5 | 0/5 | 5 marker |
| crates/components/camel-component-mcp/tests/server_consumer_test.rs | 2 | 2/0 | 2 timeout |
| crates/components/camel-component-mcp/tests/server_registry_test.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-component-seda/src/lib.rs | 4 | 0/4 | 4 marker |
| crates/components/camel-component-wasm/src/epoch.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-component-wasm/tests/return_stream_integration.rs | 4 | 0/4 | 4 marker |
| crates/components/camel-component-wasm/tests/source_stream_integration.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-component-wasm/tests/streaming_integration.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-controlbus/src/lib.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-cron/src/lib.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-cron/src/tokio_impl.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-cxf/src/bridge_client_test.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-cxf/tests/concurrency_test.rs | 4 | 3/1 | 3 timeout, 1 marker |
| crates/components/camel-cxf/tests/consumer_unit_test.rs | 2 | 1/1 | 1 timeout, 1 marker |
| crates/components/camel-cxf/tests/pool_lifecycle_test.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-cxf/tests/producer_unit_test.rs | 14 | 0/14 | 14 marker |
| crates/components/camel-cxf/tests/ws_security_test.rs | 4 | 2/2 | 2 timeout, 2 marker |
| crates/components/camel-direct/src/direct_tests.rs | 5 | 0/5 | 5 marker |
| crates/components/camel-file/src/lib.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-http/src/lib_tests.rs | 6 | 2/4 | 2 timeout, 4 marker |
| crates/components/camel-jms/src/bridge_client_test.rs | 5 | 5/0 | 5 timeout |
| crates/components/camel-jms/src/component.rs | 7 | 0/7 | 7 marker |
| crates/components/camel-jms/src/consumer.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-opensearch/src/producer/mod.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-rabbitmq/src/connection/tests.rs | 4 | 1/3 | 1 timeout, 3 marker |
| crates/components/camel-rabbitmq/src/consumer/tests.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-rabbitmq/tests/consumer_readiness.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-redis/src/executor.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-redis/src/producer.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-sql/src/consumer.rs | 3 | 0/3 | 3 marker |
| crates/components/camel-sql/src/health.rs | 1 | 1/0 | 1 timeout |
| crates/components/camel-sql/src/pool_factory.rs | 3 | 3/0 | 3 timeout |
| crates/components/camel-stream/src/consumer_tests.rs | 10 | 0/10 | 10 marker |
| crates/components/camel-stream/src/lib.rs | 2 | 0/2 | 2 marker |
| crates/components/camel-validator/src/bridge_client_test.rs | 2 | 2/0 | 2 timeout |
| crates/components/camel-ws/src/lib.rs | 1 | 0/1 | 1 marker |
| crates/components/camel-xslt/src/bridge_client_test.rs | 2 | 2/0 | 2 timeout |
| crates/services/camel-bridge/src/process.rs | 2 | 1/1 | 1 wrap (child wait) + 1 marker |
| crates/services/camel-bridge/tests/bridge_exit_path_evidence.rs | 1 | 0/1 | 1 marker |

## Local verification (exit codes)

Commands run in the feature worktree at `0e7077d70` plus the final-checkout fix commit:

- `cargo test --workspace --lib` — exit 0; 67 test suites, 11248 passed, 0 failed.
- `cargo test -p camel-test --features integration-tests --test sql_test` — exit 0; 21 passed, 0 failed (local Docker Postgres via testcontainers).
- `cargo test -p camel-test --features integration-tests --test surrealdb_test --test redis_test --test opensearch_test` — exit 0; 22 / 19 / 26 passed, 0 failed (local Docker containers).
- `cargo test -p camel-integration-test --features http --test partner_verification_test --test http_inbound_test` — exit 0; 19 + 3 passed, 0 failed (in-process partner; no external server needed).
- `cargo test -p camel-component-rabbitmq --test consumer_readiness` — exit 0; 1 passed; `RABBITMQ_ITEST` gate inactive (designed skip notice printed, no silent skip).
- The workspace run exposed and the fix commit resolves a nested-module import in `crates/camel-integration-test/src/boot_scenario_test.rs` (`boot_freshness`); without it the `sql`-feature-unified lib test does not compile.

## Deferred to CI

All items below remain `integration-verification-deferred-to-CI`.

- `crates/camel-test/tests/integration_test.rs` (78 tests) — 3500+ line shared suite; compile-only by plan, execution owned by CI.
- `crates/camel-test/tests/sql_test.rs` (13 sites, 21 tests) — live-DB target; the contained local Docker Postgres run is recorded above, CI remains the execution owner.
- `crates/camel-test/tests/mtls_bridge_test.rs` (10 tests) — 8 support tests pass; 2 bridge tests require an external xml-bridge binary and `CAMEL_XML_BRIDGE_BINARY_PATH` is unset locally.
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (7 ignored tests) — requires a prebuilt guest `wasm32-wasip2` artifact, absent locally.
- `crates/camel-integration-test` crate targets — live partner harness; execution owned by CI.

Touched targets previously misreported as compile-only were in fact executed green:

- `cargo test -p camel-cxf --test concurrency_test` — EXECUTED exit 0; 3 passed (task 2.1).
- `cargo test -p camel-cxf --test consumer_unit_test` — EXECUTED exit 0; 11 passed (task 2.1).
- `cargo test -p camel-cxf --test ws_security_test` — EXECUTED exit 0; 2 passed (task 2.1).
- `cargo test -p camel-component-mcp --test server_consumer_test --test server_registry_test` — EXECUTED exit 0 (task 2.4).

## Deadline tier notes

Two reviewed deviations from the 30s-`src/` / 60s-`tests/` guidance:

- `crates/camel-cli/src/commands/test/scenario_tests.rs`, `crates/camel-integration-test/src/boot_scenario_test.rs` and `crates/camel-integration-test/src/runner/partner_validate_test.rs` `src/` sites use 30s (the `src/`-tier cap). The `tasks.md` 2.3 step text that says "60s tier" for those sites predates the Phase-2 inter-phase review ruling that enforces the cap.
- `crates/camel-test/tests/integration_test.rs` loop sites 3523/3572 use a 5s per-iteration direct `tokio::time::timeout`, matching the site's existing retry pacing. The guidance is a ceiling, not a floor.

Both deviations are stricter than or compliant with the blessed ceilings; no test waits looser than mandated.

## Gate summary

- `cargo xtask lint-unbounded-wait`: 0 findings, exit 0 at ceiling 0.
- Ratchet value `0` in `scripts/xtask/ratchet-unbounded-wait.max`; never raised in branch history.
