# Design: lintwiden

## Context

`lint-unbounded-wait` (scripts/xtask/src/lint_unbounded_wait.rs) walks
the `syn` AST of every `#[test]` / `#[tokio::test]` fn body
(`is_test_fn`), prunes nested fn items and associated fns, unrolls
inline closures, and ratchets unadjudicated wait findings against
`scripts/xtask/ratchet-unbounded-wait.max` (currently 296, tight).
`scan_items` descends inline modules with an import chain but only
ever dispatches `scan_test_fn` for test-attributed fns. Helpers one
fn boundary away are invisible (bd rc-h2qwr, discovered-from
rc-j27pc: bare `while-let` drain in `start_tls_consumer`,
crates/components/camel-component-grpc/tests/integration.rs:1293,
since converted by drainscope 378dbe6d).

Baseline: at main d9ac1ca7 the scanner reports exactly 296 test-fn
findings (scratch max-0 run, list committed as
`baseline-sites.txt` in this change dir). The ceiling is tight —
any unadjudicated helper finding after widening breaks the gate.

## Decisions

### D1 — Scope rule: path component `tests` OR `#[cfg(test)]` ancestor

A non-test `Item::Fn` is scanned when EITHER holds:

- the file's path has a component named exactly `tests` (integration
  test trees `crates/*/tests/**`, also `examples/tests/**`,
  `benchmarks/*/tests/**`, `fuzz/tests/**` — the rule is
  path-uniform; a file named `tests.rs` is NOT a match, only a
  directory); OR
- the fn is lexically inside an INLINE module annotated `#[cfg(test)]`
  (attribute path `cfg`, meta list tokens exactly `test`), including
  nested inline submodules of a `#[cfg(test)]` module.

`#[cfg(all(test, ..))]` and `cfg_attr` shapes are NOT treated as
cfg(test) (corpus-zero today; a miss is a false negative, never a
false positive). Out-of-line `#[cfg(test)] mod X;` declarations
(non-inline module files, e.g. camel-integration-test/src/lib.rs
`mod runner_test;`) are a DECLARED false-negative class: each file
is parsed standalone and `Item::Mod` without content is skipped, so
non-test helpers in those module files stay invisible — recorded
here as a known blind spot (same treatment as the rc-eow0s
binding-indirection class), not silently claimed. Test-attributed fns keep their existing scope
(unchanged dispatch). Associated fns (`impl` / trait default
methods) stay out of scope at file level, mirroring the in-body
pruning rule. `fn main()` in a `tests/` file is a non-test fn and
IS scanned (a blocking wait in main parks the whole binary — the
exact failure class this lint exists for).

Implementation: `scan_source` derives `under_tests_dir` from the
file path components (public signature unchanged — unit fixtures
select scope by path: `fixture.rs` = src scope, `tests/fixture.rs`
= widened scope). `scan_items` threads a scan context
(`under_tests_dir`, `in_cfg_test`) through module recursion;
`Item::Mod` sets `in_cfg_test` when the cfg(test) attribute is
present and never clears it downward. Known nuance: the
path-component rule also captures `src/tests/` directories (e.g.
camel-master's); a non-cfg-gated production fn there would be
scanned — corpus-zero findings today, and the spec delta codifies
the component rule as-is.

### D2 — Shared machinery, one scan path

`scan_test_fn` is the generic fn-body scan (body_top imports,
body_nested scopes, spawn/timeout/inline-closure passes, WaitFinder).
The widened dispatch calls the same function for helper fns; no
separate helper scanner exists. This structurally guarantees
"test-fn findings unchanged": the only behavioral delta is WHICH fns
enter the scan.

### D3 — Spawned-closure traversal inside helper fns: stay pruned

Closures that do not execute in the scanned body (spawned work,
route builders, callbacks) stay pruned inside helper fns, exactly as
in test bodies today. Rationale: a closure passed to `tokio::spawn`
runs in its own task scope; unrolling it would attribute the wait to
the helper's ratchet entry without a deadline region being possible
there (the deadline belongs in the closure). Binding-indirection
false negatives remain tracked in bd rc-eow0s. Unit test pins this:
a spawn-closure recv inside a helper fn under `tests/` is NOT
reported.

### D4 — Adjudication classes (per-site, no silent sites)

Every newly-visible finding gets exactly one disposition:

1. **Global test-lock acquisition** (`.lock().await` on a
   process-global `Mutex<()>` test-serialization lock in a helper):
   convert to `acquire_deadline(&LOCK, "NAME (context)",
   TEST_LOCK_DEADLINE).await` from
   `camel_component_api::test_support` (precedent c2a48f20, bd
   rc-88old; helper is `#[track_caller]`, deadline 900 s).
2. **Channel drain / receive in a helper**: per-iteration deadline
   D-recipe (drainscope shape): `loop { match timeout(d,
   rx.recv()).await { Ok(Some(v)) => .., Ok(None) | Err(_) => break } }`
   with the file's existing deadline convention.
3. **Connect-style wait**: `tokio::time::timeout` wrap with a named
   deadline.
4. **Marker**: `// allow-test-wait: <reason>` only where a deadline
   is semantically wrong for that site (ADR-0069 R1 escape hatch);
   reason must name the semantics.
5. **Ceiling entry**: only if genuinely unreachable by 1–4; requires
   the ONE mission-allowed ceiling bump with justification in park
   notes. Target is zero entries here.

Dev-dep note: `camel-integration-test` declares
`camel-component-api.workspace = true` in `[dependencies]` only —
class-1 conversions there add a SEPARATE `[dev-dependencies]` entry
`camel-component-api = { workspace = true, features =
["test-support"] }` and never touch the production-dependency line
(the feature's own doc forbids production enablement — the stub
panics on use; feature unification would propagate it downstream).
camel-test, camel-dsl, camel-cxf already declare the dev-dep.
Class-1 mechanism also fits per-test STATE mutexes inside bounded
retry loops (cxf `wait_for_*`, integration-test helpers):
`acquire_deadline` is generic over `Mutex<T>`, but the 900 s
`TEST_LOCK_DEADLINE` rationale (rc-88old holder-chain math) does not
transfer to state mutexes — Appendix B records those rows as
"acquire_deadline mechanism, state mutex" with a file-appropriate
deadline.

### D5 — Ratchet policy

Baseline 296 is ALL test-fn findings (tight). Widening adds helper
findings; conversions (classes 1–3) bound them, markers (class 4)
suppress them. Final target: total = 296 exactly (zero ceiling
movement). Monotone rule: no increase without review justification;
decrease is always allowed. The acceptance run is
`cargo run -p xtask -- lint-unbounded-wait` exiting 0 at ceiling 296
(or the single documented bump), plus the diff of the scratch max-0
site list against `baseline-sites.txt` showing test-fn sites
untouched.

### D6 — Inventory procedure (Appendix B)

After the scanner widens (Task 1), capture the full finding list via
scratch max-0 run, subtract `baseline-sites.txt` (296 entries), and
record the remainder as design.md Appendix B: file:line, helper fn
name, wait class, adjudication class (D4.1–D4.5), and status. The
widened AST scanner is the source of truth; drainscope Appendix A
(the lexical seed: 30 candidates, 27 unenclosed, 18 files) is only a
cross-check — AST-only shapes (spawned-handle awaits, loops,
blocking_recv, cfg(test)-in-src helpers) may add sites the seed
never saw. The grpc site from the seed is already converted
(drainsscope 378dbe6d) and must show as bounded.

## Affected crates

- scripts/xtask (scanner + unit tests).
- Test trees only: crates/camel-test, crates/camel-integration-test,
  crates/camel-dsl (tests/), crates/components/camel-cxf, plus
  residual files the AST inventory reveals. No runtime/DSL/component
  API surface touched.

## Architecture boundaries

Test-support code and tooling only; the lint stays a structural
`syn` AST walk with no type information (ADR-0069 R1 accepts
narrow-scope imprecision; R6 job-level timeouts remain the backstop).

## Alternatives considered

- **cfg(test)-only widening** (skip tests/ dirs): rejected — the
  motivating defect class lives in `tests/` helper fns; half a
  widening re-opens the next mission for the same review cost.
- **Inventory-first, lint-widening-second**: rejected — the AST
  scanner IS the inventory source of truth (D6); shipping it first
  avoids adjudicating a lexical list that then shifts under the AST.
- **Unadjudicated ceiling bump to absorb the seed**: rejected —
  drainscope contract forbids bumps without per-site adjudication;
  mission allows ONE justified bump only for genuinely unreachable
  sites.

## Appendix B — AST inventory of newly-visible helper-fn sites

Source: scratch max-0 run of the widened scanner (Task 1 commit 337ab9d0)
against baseline-sites.txt: 372 total findings − 296 baseline test-fn
findings = 76 remainder sites below. Every row carries one D4
adjudication class, an owner task, and status `pending` until its owner
task lands the conversion. Class counts: D4.1S 30, D4.2 18, D4.3 28;
D4.1 zero (no remainder site locks a process-global `static` test
lock — every lock row is a per-test state mutex), D4.4 and D4.5 zero.
Owner counts: task 3 (camel-test) 37, task 4 (camel-integration-test,
camel-dsl tests, camel-cxf, camel-component-grpc) 18, task 5
(residual: camel-component-wasm, camel-component-mcp, camel-http,
camel-sql, camel-redis, camel-template, camel-ws) 21. `CamelTestContext::ctx`
returns `&Arc<Mutex<CamelContext>>` and the integration-test `ctx` is
`Arc::new(Mutex::new(run.ctx))` — both per-test state, hence D4.1S with a
file-appropriate deadline, never the 900 s TEST_LOCK_DEADLINE rationale.

| file:line | helper fn | wait class | adjudication class | owner task | status |
|---|---|---|---|---|---|
| `crates/camel-dsl/tests/common/mod.rs:107` | `wait_listening` | connect (retry loop) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/camel-dsl/tests/common/mod.rs:138` | `http_roundtrip` | connect | D4.3 | task 4 | done (converted: D4.3) |
| `crates/camel-dsl/tests/rest_negotiation_e2e.rs:325` | `serve_gate` | recv (bounded for-loop drain) | D4.2 | task 4 | done (converted: D4.2) |
| `crates/camel-integration-test/src/adapters/http.rs:1328` | `raw_request` | connect | D4.3 | task 4 | done (converted: D4.3) |
| `crates/camel-integration-test/tests/circuit_fallback_test.rs:171` | `run_two_docs` | lock (per-test ctx state mutex) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/camel-integration-test/tests/common/mod.rs:91` | `run_logs_document` | lock (per-test ctx state mutex) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/camel-integration-test/tests/direct_reply_test.rs:53` | `run_direct` | lock (per-test ctx state mutex) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/camel-integration-test/tests/http_partner_scripting_test.rs:1415` | `run_doc_route_dialed` | lock (per-test ctx state mutex) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/camel-integration-test/tests/partner_verification_test.rs:594` | `raw_post` | connect | D4.3 | task 4 | done (converted: D4.3) |
| `crates/camel-integration-test/tests/partner_verification_test.rs:617` | `raw_get` | connect | D4.3 | task 4 | done (converted: D4.3) |
| `crates/camel-test/tests/cache_resilience.rs:32` | `send_to_direct_tolerant` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/cache_test_support/mod.rs:31` | `send_to_direct` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/cache_test_support/mod.rs:70` | `send_to_direct_result` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/component_emission_test.rs:201` | `wait_for_started` | loop (status poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/controlbus_test.rs:15` | `route_status` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/controlbus_test.rs:38` | `start_route` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/controlbus_test.rs:54` | `suspend_route` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/controlbus_test.rs:70` | `resume_route` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/direct_top_level_test.rs:107` | `stop_route` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/direct_top_level_test.rs:122` | `start_route` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/direct_top_level_test.rs:37` | `send_to_direct_ignoring_error` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/direct_top_level_test.rs:63` | `send_to_direct_until_delivered` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/direct_top_level_test.rs:89` | `route_status` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/do_try_test.rs:25` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/integration_test.rs:372` | `send_await_reply` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/inventory_metrics_test.rs:201` | `await_queue_depth` | loop (scrape poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/inventory_metrics_test.rs:251` | `wait_for_started` | loop (status poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/jsonpath_test.rs:23` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/loop_test.rs:17` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/marshal_test.rs:15` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/mcp_server_auth_test.rs:122` | `raw_json_rpc_post` | connect (retry loop) | D4.3 | task 3 | done (converted: D4.3) |
| `crates/camel-test/tests/metrics_wiring_test.rs:231` | `wait_for_started` | loop (status poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/otel_direct_hop_regression.rs:32` | `route_started` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/otel_direct_hop_regression.rs:43` | `wait_for_started` | loop (status poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/otel_direct_hop_regression.rs:73` | `drive_entry_in_out` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/otel_trace_tree_test.rs:154` | `route_started` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/otel_trace_tree_test.rs:164` | `wait_for_started` | loop (status poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/otel_trace_tree_test.rs:194` | `drive_direct_in_out` | lock (ctx state mutex, retry loop) | D4.1S | task 3 | done (converted: D4.1S, loop timeout wrap) |
| `crates/camel-test/tests/redis_repositories_test.rs:1099` | `trigger_acl_failover` | connect (retry loop) | D4.3 | task 3 | done (converted: D4.3) |
| `crates/camel-test/tests/redis_sentinel_test.rs:418` | `poll_get` | loop (send poll await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/script_test.rs:399` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/script_test.rs:423` | `send_to_direct_ignore_error` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/camel-test/tests/sql_test.rs:32` | `create_pool` | connect (sqlx pool) | D4.3 | task 3 | done (converted: D4.3) |
| `crates/camel-test/tests/support/artemis.rs:18` | `wait_for_broker_port` | connect (retry loop) | D4.3 | task 3 | done (converted: D4.3) |
| `crates/camel-test/tests/support/kafka.rs:53` | `kafka_probe` | connect | D4.3 | task 3 | done (converted: D4.3) |
| `crates/camel-test/tests/support/mod.rs:173` | `retry_direct_not_registered` | loop (op retry await) | D4.2 | task 3 | done (converted: D4.2) |
| `crates/camel-test/tests/xpath_test.rs:23` | `send_to_direct` | lock (ctx state mutex) | D4.1S | task 3 | done (converted: D4.1S) |
| `crates/components/camel-component-grpc/src/server.rs:1694` | `open_bidi_request` | recv (drain in spawned task) | D4.2 | task 4 | done (converted: D4.2, idle-relay re-arm: timeout re-arms instead of break — the request side must stay open, only channel close ends the loop) |
| `crates/components/camel-component-mcp/tests/server_protocol_test.rs:96` | `raw_json_rpc_post_with_host` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_auth_e2e.rs:213` | `wait_for_bind` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_auth_e2e.rs:233` | `send_http_post_with_headers` | connect | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_bind_gate.rs:151` | `wait_for_bind` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_bind_gate.rs:213` | `send_http_post` | connect | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_integration.rs:142` | `send_http_post` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_integration.rs:176` | `wait_for_bind` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_stream_integration.rs:107` | `wait_for_bind` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/source_stream_integration.rs:122` | `send_http_post` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/staged_listener_source.rs:138` | `send_http_post` | connect | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-component-wasm/tests/staged_listener_source.rs:172` | `wait_for_bind` | connect (retry loop) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-cxf/tests/concurrency_test.rs:19` | `make_producer` | connect (tonic channel) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/components/camel-cxf/tests/concurrency_test.rs:43` | `make_pool_with_ready_slot` | connect (tonic channel) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/components/camel-cxf/tests/consumer_unit_test.rs:31` | `open_consumer_stream` | connect (tonic channel) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/components/camel-cxf/tests/consumer_unit_test.rs:51` | `wait_for_consumer_request_sender` | lock (MockState mutex, bounded retry loop) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/components/camel-cxf/tests/consumer_unit_test.rs:64` | `wait_for_recorded_responses` | lock (MockState mutex, bounded retry loop) | D4.1S | task 4 | done (converted: D4.1S) |
| `crates/components/camel-cxf/tests/pool_lifecycle_test.rs:13` | `connect_mock_channel` | connect (tonic channel) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/components/camel-cxf/tests/producer_unit_test.rs:30` | `make_producer` | connect (tonic channel) | D4.3 | task 4 | done (converted: D4.3) |
| `crates/components/camel-http/src/lib.rs:12289` | `spawn_responder` | recv (spawned task) | D4.2 | task 5 | done (converted: D4.2) |
| `crates/components/camel-http/src/lib.rs:13680` | `spawn_failing_auth_route` | recv (drain in spawned task) | D4.2 | task 5 | done (converted: D4.2) |
| `crates/components/camel-http/src/lib.rs:6397` | `start_capture_server` | loop (accept drain, spawned task) | D4.2 | task 5 | done (converted: D4.2, idle-relay re-arm: every caller drops the returned handle (bound to `_handle`), so the accept loop is a detached server whose premise is to stay open for the test process lifetime; only process exit ends it) |
| `crates/components/camel-http/src/lib.rs:6437` | `start_redirect_capture_server` | loop (accept drain, spawned task) | D4.2 | task 5 | done (converted: D4.2, idle-relay re-arm: every caller drops the returned handle (bound to `_handle`), so the accept loop is a detached server whose premise is to stay open for the test process lifetime; only process exit ends it) |
| `crates/components/camel-redis/tests/common/mod.rs:197` | `serve_connection` | loop (frame drain await) | D4.2 | task 5 | done (converted: D4.2, idle-relay re-arm: detached per-connection task (spawn handle dropped); the peer disconnect is the designed exit and quiet frame gaps are normal — client response deadlines in these targets reach 10s) |
| `crates/components/camel-redis/tests/common/mod.rs:226` | `wait_until_released` | loop (notified await) | D4.2 | task 5 | done (converted: D4.2) |
| `crates/components/camel-sql/src/consumer.rs:718` | `sqlite_pool` | connect (sqlx pool) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-sql/src/producer.rs:624` | `sqlite_pool` | connect (sqlx pool) | D4.3 | task 5 | done (converted: D4.3) |
| `crates/components/camel-template/tests/common/mod.rs:93` | `send_title` | loop (send retry await) | D4.2 | task 5 | done (converted: D4.2) |
| `crates/components/camel-ws/src/lib.rs:2761` | `spawn_echo_route` | recv (spawned task) | D4.2 | task 5 | done (converted: D4.2) |

Cross-check (D6): all 30 drainscope Appendix A seed candidates are
accounted for in this table — 29 map to rows (six with the expected
statement-line drift, where the AST scanner reports the enclosing
`loop`/`for` line instead of the lexical await line in five cases:
cache_resilience 34→32, direct_top_level 65→63, integration_test
374→372, otel_direct_hop 75→73, otel_trace_tree 196→194; the sixth,
rest_negotiation_e2e 324→325, runs the opposite direction — the seed
noted the `for` line, the row reports the recv line), and the 30th,
the motivating `start_tls_consumer` drain at
crates/components/camel-component-grpc/tests/integration.rs:1293, was
already converted by drainscope (378dbe6d) and correctly does not
reappear; the 47 AST-only sites the seed never saw are the spawned-task
drains, cfg(test)-in-src helpers, and poll loops D6 predicted.
