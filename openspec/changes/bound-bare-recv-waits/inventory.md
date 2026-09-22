# Inventory: bare recv().await sites (lint-unbounded-wait)

REVISED after the inter-phase-2 review: the original inventory was built by
matching `.recv().await` on the finding's start line and missed multi-line
`rx\n.recv()\n.await` shapes. This revision re-derives every site from the
lint's own syn-AST scanner (awaited method == recv): **117 sites total** —
44 done (Phases 1-2; lines at merge-base 7668c9fe) + 73 open (lines at HEAD
after Phase 2; multi-line sites are listed at the line where the receiver
expression STARTS — locate by signature, not line alone).

- **A** — single receive in the test fn body.
- **B** — `while let` drain loop directly in the test body.
- **D** — receive inside a `tokio::spawn(async move { .. })` background task.
- Recvs subsumed by a reported `loop {}` finding (e.g. server_auth_test.rs
  recv inside the :577 loop) belong to mission 210 loopsweep, not this lane.

| # | file | line | fam | status | signature |
|---|------|------|-----|--------|-----------|
| 1 | crates/camel-core/src/lifecycle/adapters/consumer_management.rs | 1186 | A | done→task 1.1 | `let notification = crash_rx.recv().await.expect("crash notification expected");` |
| 2 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 621 | A | done→task 1.2 | `let command = rx.recv().await.expect("command should be received");` |
| 3 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 910 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("stop command");` |
| 4 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 924 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("exists command");` |
| 5 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 938 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("hash command");` |
| 6 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 961 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("route_count command");` |
| 7 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 975 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("stop command");` |
| 8 | crates/camel-core/src/lifecycle/adapters/controller_actor.rs | 989 | A | done→task 1.2 | `let cmd = rx.recv().await.expect("hash command");` |
| 9 | crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs | 486 | A | done→task 1.3 | `emitted.push(emitted_rx.recv().await.expect("first emission"));` |
| 10 | crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs | 487 | A | done→task 1.3 | `emitted.push(emitted_rx.recv().await.expect("second emission"));` |
| 11 | crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs | 560 | A | done→task 1.3 | `let _ = emitted_rx.recv().await.expect("aggregated emission");` |
| 12 | crates/camel-processor/src/resequencer/mod.rs | 785 | A | done→task 1.4 | `let _ = capture_rx.recv().await;` |
| 13 | crates/camel-processor/src/resequencer/mod.rs | 786 | A | done→task 1.4 | `let _ = capture_rx.recv().await;` |
| 14 | crates/camel-processor/src/resequencer/mod.rs | 812 | A | done→task 1.4 | `let _ = capture_rx.recv().await;` |
| 15 | crates/components/camel-component-api/src/consumer_claim_tests.rs | 37 | A | done→task 1.5 | `let envelope = rx.recv().await.expect("envelope must arrive");` |
| 16 | crates/components/camel-component-api/src/consumer_claim_tests.rs | 51 | A | done→task 1.5 | `let envelope = rx.recv().await.expect("envelope must arrive");` |
| 17 | crates/components/camel-component-api/src/consumer_claim_tests.rs | 92 | A | done→task 1.5 | `let envelope = rx.recv().await.expect("envelope must arrive");` |
| 18 | crates/components/camel-component-grpc/src/server.rs | 1122 | A | done→task 2.1 | `match reply_rx.recv().await {` |
| 19 | crates/components/camel-component-grpc/src/server.rs | 1185 | A | done→task 2.1 | `match reply_rx.recv().await {` |
| 20 | crates/components/camel-component-grpc/src/server.rs | 1409 | A | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 21 | crates/components/camel-component-grpc/src/server.rs | 1435 | A | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 22 | crates/components/camel-component-grpc/src/server.rs | 1460 | A | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 23 | crates/components/camel-component-grpc/src/server.rs | 1485 | A | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 24 | crates/components/camel-component-grpc/src/server.rs | 1516 | A | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 25 | crates/components/camel-component-grpc/src/server.rs | 1554 | A | done→task 2.1 | `let received = rx.recv().await;` |
| 26 | crates/components/camel-component-grpc/src/server.rs | 1734 | D | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 27 | crates/components/camel-component-grpc/src/server.rs | 1774 | D | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 28 | crates/components/camel-component-grpc/src/server.rs | 1811 | D | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 29 | crates/components/camel-component-grpc/src/server.rs | 1910 | D | done→task 2.1 | `let envelope = rx.recv().await.unwrap();` |
| 30 | crates/components/camel-component-grpc/tests/integration.rs | 260 | D | done→task 2.2 | `if let Some(envelope) = route_rx.recv().await {` |
| 31 | crates/components/camel-component-grpc/tests/integration.rs | 432 | D | done→task 2.2 | `tokio::spawn(async move { while let Some(_envelope) = route_rx.recv().await {} });` |
| 32 | crates/components/camel-component-grpc/tests/integration.rs | 496 | D | done→task 2.2 | `if let Some(envelope) = route_rx.recv().await {` |
| 33 | crates/components/camel-component-grpc/tests/integration.rs | 570 | D | done→task 2.2 | `while let Some(envelope) = route_rx1.recv().await {` |
| 34 | crates/components/camel-component-grpc/tests/integration.rs | 604 | D | done→task 2.2 | `while let Some(envelope) = route_rx2.recv().await {` |
| 35 | crates/components/camel-component-grpc/tests/integration.rs | 693 | D | done→task 2.2 | `tokio::spawn(async move { while let Some(_envelope) = route_rx.recv().await {} });` |
| 36 | crates/components/camel-component-grpc/tests/integration.rs | 764 | D | done→task 2.2 | `tokio::spawn(async move { while let Some(_envelope) = route_rx1.recv().await {} });` |
| 37 | crates/components/camel-component-grpc/tests/integration.rs | 849 | D | done→task 2.2 | `if let Some(_envelope) = route_rx.recv().await {` |
| 38 | crates/components/camel-component-grpc/tests/integration.rs | 922 | D | done→task 2.2 | `if let Some(envelope) = route_rx.recv().await {` |
| 39 | crates/components/camel-component-grpc/tests/integration.rs | 1567 | D | done→task 2.2 | `while let Some(envelope) = route_rx.recv().await {` |
| 40 | crates/components/camel-component-grpc/tests/integration.rs | 1644 | D | done→task 2.2 | `if let Some(mut envelope) = route_rx.recv().await {` |
| 41 | crates/components/camel-component-grpc/tests/integration.rs | 1746 | D | done→task 2.2 | `while let Some(mut envelope) = route_rx.recv().await {` |
| 42 | crates/components/camel-component-grpc/tests/integration.rs | 1753 | D | done→task 2.2 | `release_rx.recv().await.expect("test alive");` |
| 43 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 262 | D | done→task 2.3 | `let envelope = route_rx.recv().await.expect("exchange reaches route");` |
| 44 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 308 | D | done→task 2.3 | `let envelope = route_rx.recv().await.expect("exchange reaches route");` |
| 45 | crates/camel-core/src/lifecycle/adapters/consumer_management.rs | 1101 | A | open | `let notification = crash_rx` |
| 46 | crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs | 4473 | A | open | `let dispatched = dispatched_rx` |
| 47 | crates/camel-dsl/tests/rest_stream_contract_e2e.rs | 512 | D | open | `while let Some(mut envelope) = rx.recv().await {` |
| 48 | crates/camel-test/tests/http_static_test.rs | 138 | D | open | `while let Some(envelope) = api_rx.recv().await {` |
| 49 | crates/camel-test/tests/mcp_server_auth_test.rs | 264 | D | open | `let envelope = route_rx.recv().await.expect("route received the exchange");` |
| 50 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 398 | D | open | `let envelope = route_rx` |
| 51 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 451 | D | open | `let envelope = route_rx` |
| 52 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 510 | D | open | `let envelope = route_rx` |
| 53 | crates/components/camel-component-grpc/tests/server_auth_test.rs | 649 | D | open | `let envelope = route_rx` |
| 54 | crates/components/camel-component-mcp/tests/dsl_e2e_test.rs | 202 | D | open | `let envelope = tool_rx` |
| 55 | crates/components/camel-component-mcp/tests/dsl_e2e_test.rs | 220 | D | open | `let envelope = customers_rx` |
| 56 | crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 161 | D | open | `let envelope = tool_rx` |
| 57 | crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 179 | D | open | `let envelope = customers_rx` |
| 58 | crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 194 | D | open | `let envelope = logo_rx` |
| 59 | crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 338 | D | open | `let envelope = tool_rx` |
| 60 | crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs | 352 | D | open | `let envelope = resource_rx` |
| 61 | crates/components/camel-component-mcp/tests/server_consumer_test.rs | 203 | A | open | `let envelope = route_rx.recv().await.expect("route received the exchange");` |
| 62 | crates/components/camel-component-mcp/tests/server_consumer_test.rs | 726 | A | open | `let envelope = tool_route_rx` |
| 63 | crates/components/camel-component-mcp/tests/server_consumer_test.rs | 760 | A | open | `let envelope = resource_route_rx` |
| 64 | crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs | 197 | D | open | `let invocation = rx.recv().await.expect("route must receive the invocation");` |
| 65 | crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs | 372 | D | open | `let invocation = rx.recv().await.expect("route must receive the invocation");` |
| 66 | crates/components/camel-component-seda/src/lib.rs | 2240 | D | open | `while let Some(envelope) = rx.recv().await {` |
| 67 | crates/components/camel-component-seda/src/lib.rs | 2367 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 68 | crates/components/camel-component-seda/src/lib.rs | 2973 | D | open | `while let Some(env) = rx_b.recv().await {` |
| 69 | crates/components/camel-component-seda/src/lib.rs | 3221 | D | open | `let held = route_rx.recv().await.expect("pipeline receives exchange");` |
| 70 | crates/components/camel-component-seda/src/lib.rs | 3278 | D | open | `let held = rx_a.recv().await.expect("subscriber A copy");` |
| 71 | crates/components/camel-component-seda/src/lib.rs | 3286 | D | open | `let held = rx_b.recv().await.expect("subscriber B copy");` |
| 72 | crates/components/camel-component-seda/src/lib.rs | 3353 | D | open | `let e1 = route_rx.recv().await.expect("pipeline receives E1");` |
| 73 | crates/components/camel-component-seda/src/lib.rs | 3360 | D | open | `let _ = route_rx.recv().await.expect("successor envelope");` |
| 74 | crates/components/camel-component-wasm/tests/source_integration.rs | 343 | D | open | `while let Some(envelope) = rx.recv().await {` |
| 75 | crates/components/camel-component-wasm/tests/source_stream_integration.rs | 537 | D | open | `while let Some(envelope) = rx.recv().await {` |
| 76 | crates/components/camel-direct/src/direct_tests.rs | 285 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 77 | crates/components/camel-direct/src/direct_tests.rs | 334 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 78 | crates/components/camel-direct/src/direct_tests.rs | 622 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 79 | crates/components/camel-direct/src/direct_tests.rs | 781 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 80 | crates/components/camel-direct/src/direct_tests.rs | 838 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 81 | crates/components/camel-direct/src/direct_tests.rs | 1364 | D | open | `while let Some(envelope) = route_rx.recv().await {` |
| 82 | crates/components/camel-http/src/lib.rs | 8318 | D | open | `while let Some(envelope) = rx.recv().await {` |
| 83 | crates/components/camel-http/src/lib.rs | 11686 | D | open | `if let Some(envelope) = rx.recv().await {` |
| 84 | crates/components/camel-http/src/lib.rs | 11731 | D | open | `while get_rx.recv().await.is_some() {}` |
| 85 | crates/components/camel-http/src/lib.rs | 11757 | D | open | `if let Some(envelope) = rx.recv().await {` |
| 86 | crates/components/camel-http/src/lib.rs | 11859 | D | open | `while get_rx.recv().await.is_some() {}` |
| 87 | crates/components/camel-http/src/lib.rs | 11915 | D | open | `if let Some(env) = tpl_rx.recv().await {` |
| 88 | crates/components/camel-kafka/src/consumer.rs | 1230 | D | open | `while let Some(_req) = rx.recv().await {` |
| 89 | crates/components/camel-kafka/src/consumer.rs | 1469 | D | open | `while let Some(env) = rx.recv().await {` |
| 90 | crates/components/camel-kafka/src/consumer.rs | 1529 | D | open | `while let Some(env) = rx.recv().await {` |
| 91 | crates/components/camel-kafka/src/consumer.rs | 1572 | D | open | `while let Some(env) = rx.recv().await {` |
| 92 | crates/components/camel-kafka/src/consumer.rs | 1618 | D | open | `while let Some(env) = rx.recv().await {` |
| 93 | crates/components/camel-kafka/src/consumer.rs | 1738 | D | open | `while let Some(env) = rx.recv().await {` |
| 94 | crates/components/camel-kafka/src/manual_commit.rs | 139 | A | open | `let req = rx.recv().await.unwrap();` |
| 95 | crates/components/camel-kafka/src/manual_commit.rs | 160 | D | open | `if let Some(req) = rx.recv().await {` |
| 96 | crates/components/camel-kafka/src/manual_commit.rs | 186 | D | open | `if let Some(req) = rx.recv().await {` |
| 97 | crates/components/camel-master/src/leadership.rs | 498 | A | open | `let env = pipeline_rx` |
| 98 | crates/components/camel-master/src/leadership.rs | 539 | A | open | `let env = pipeline_rx.recv().await.unwrap();` |
| 99 | crates/components/camel-master/src/leadership.rs | 574 | A | open | `let env = pipeline_rx` |
| 100 | crates/components/camel-master/src/leadership.rs | 769 | A | open | `let mut received = pipeline_rx.recv().await.expect("envelope must arrive");` |
| 101 | crates/components/camel-master/src/leadership.rs | 810 | A | open | `let mut received = pipeline_rx.recv().await.expect("envelope must arrive");` |
| 102 | crates/components/camel-sql/src/consumer.rs | 776 | D | open | `while let Some(env) = rx.recv().await {` |
| 103 | crates/components/camel-sql/src/consumer.rs | 821 | D | open | `while let Some(env) = rx.recv().await {` |
| 104 | crates/components/camel-sql/src/consumer.rs | 867 | D | open | `while let Some(env) = rx.recv().await {` |
| 105 | crates/components/camel-sql/src/consumer.rs | 922 | D | open | `while let Some(env) = rx.recv().await {` |
| 106 | crates/components/camel-sql/src/consumer.rs | 1007 | D | open | `while let Some(env) = rx.recv().await {` |
| 107 | crates/components/camel-sql/src/consumer.rs | 1042 | D | open | `while let Some(env) = rx.recv().await {` |
| 108 | crates/components/camel-sql/src/consumer.rs | 1095 | D | open | `while let Some(env) = rx.recv().await {` |
| 109 | crates/components/camel-sql/src/consumer.rs | 1141 | D | open | `while let Some(env) = rx.recv().await {` |
| 110 | crates/components/camel-sql/src/consumer.rs | 1180 | D | open | `while let Some(env) = rx.recv().await {` |
| 111 | crates/components/camel-sql/src/consumer.rs | 1243 | D | open | `while let Some(env) = rx.recv().await {` |
| 112 | crates/components/camel-sql/src/consumer.rs | 1283 | D | open | `while let Some(env) = rx.recv().await {` |
| 113 | crates/components/camel-sql/src/consumer.rs | 1324 | D | open | `if let Some(env) = rx.recv().await {` |
| 114 | crates/components/camel-sql/src/consumer.rs | 1399 | D | open | `while let Some(env) = mpsc_rx.recv().await {` |
| 115 | crates/components/camel-sql/src/consumer.rs | 1561 | D | open | `while let Some(env) = rx.recv().await {` |
| 116 | crates/components/camel-timer/src/lib.rs | 531 | B | open | `while let Some(envelope) = rx.recv().await {` |
| 117 | crates/components/camel-ws/src/lib.rs | 2620 | D | open | `if let Some(envelope) = route_rx.recv().await {` |
