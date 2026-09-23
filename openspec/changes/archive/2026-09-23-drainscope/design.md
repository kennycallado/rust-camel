# Design: drainscope

## Approach

Single-site conversion, path (b) from the mission order.

Current shape (integration.rs:1286-1304, inside `start_tls_consumer`):
a `tokio::spawn` pipeline simulator draining
`while let Some(envelope) = route_rx.recv().await`.

Converted shape (D-recipe per the unbounded-wait-bounding scenario
"Receive inside a spawned background task is per-iteration bounded";
mpsc `recv()` yields `Option<T>`, so `timeout` wraps it into
`Result<Option<T>, Elapsed>`):

```rust
let pipeline_task = tokio::spawn(async move {
    loop {
        match timeout(Duration::from_secs(2), route_rx.recv()).await {
            Ok(Some(envelope)) => { /* unchanged body */ }
            Ok(None) => break,      // channel closed: drain complete
            Err(_elapsed) => break, // stalled producer: end the drain
        }
    }
});
```

`timeout` / `Duration` are already imported (used at :262 etc).
Per-iteration — not a whole-task timeout — because the task spans the
whole test. On stall the loop exits, the closure ends and drops
`route_rx`; the next RPC then fails observably at the consumer's route
send with `Status::internal("pipeline channel closed")`
(consumer.rs:990-992). Deadline 2 s matches the file-wide convention.

Accepted flake exposure: the first idle window (helper start to first
request) is now bounded at 2 s; the TLS trio startup path (100 ms bind
wait + handshake) sits two orders of magnitude inside it. The trio is
the stability gate.

No ratchet interaction: the site is inside a spawn closure argument,
pruned by the closure rule everywhere (blind spot documented in the
lint module doc; binding-indirection kin in bd rc-eow0s). Count stays
393; `lint-unbounded-wait` exit 0 is a gate.

## Path decision: why (b), not (a)

Mission rule: pick (a) — widen lint scope to helper fns under `tests/` —
only if it fits ~1.5 tasks. Corpus probe (Appendix A): 30 wait-class
candidates in non-test fn bodies, 27 with no timeout enclosure within
3 lines, over 18 files. Precedent: bound-bare-recv-waits converted 117
sites in 16 reviewed tasks ⇒ 27 candidates ≈ 4 tasks, plus the lint
scope change, its unit tests, and ceiling-bump adjudication — far past
1.5 tasks. Halving or doubling the probe error leaves the verdict
unchanged. Also: widened helper scope would still NOT see this site
(spawn closures stay pruned), so (a) alone cannot fix the motivating
defect.

### Follow-up-bd contract (path (a), deferred)

The follow-up bd (discovered-from rc-j27pc) MUST require:

1. AST-derived full inventory of newly-visible helper-fn sites under
   `tests/` dirs (the widened scanner is the source of truth; Appendix A
   is only the lexical seed).
2. Per-site adjudication: bounded in-tree, `allow-test-wait` marker with
   site-specific justification, or ceiling entry — no silent sites.
3. Explicit decision on spawned-closure traversal inside helper fns.
4. Lint scope unit tests (helper visible, test fn unchanged) and a
   monotone ceiling (no increase without review justification).
5. Spec delta landing with enforcement (requirement text extension to
   helper fns under `tests/`).

## Affected crates

- camel-component-grpc: tests/integration.rs only (one helper body).

## Architecture boundaries

Test-support code only; no Runtime/DSL/Components API surface touched.

## Alternatives considered

- **(a) widen lint scope**: rejected on budget (27 unbounded candidates
  ≈ 4+ tasks vs ~1.5 allowed) and partial enforcement (spawn-closure
  sites stay pruned). Deferred to the follow-up bd, mirroring how prior
  blind-spot widenings landed (structural scan seed, closure-unroll
  probe rc-q2l8u): review-visible, inventory-first.
- **Hybrid (widen + unadjudicated ceiling bump)**: rejected — the
  mission forbids bumps without full per-site adjudication.
- **Do nothing / marker**: rejected — a parked pipeline task burns the
  runner on producer stall; `allow-test-wait` marks waits where a
  deadline is semantically wrong, not invisible ones.

## Appendix A — corpus probe: wait-class candidates in non-test fns under `tests/`

Method: brace-counted lexical scan (comments/strings stripped) over all
297 `.rs` files under any `tests/` dir in crates/, scripts/, examples/,
benchmarks/, fuzz/; awaited `recv/lock/acquire/wait/join_next/connect`
plus `blocking_recv(` inside non-test fn spans. Candidates for the
structural scanner, NOT confirmed violations. Status: U = no
`timeout` within 3 lines above; B = timeout nearby.

- crates/camel-dsl/tests/rest_negotiation_e2e.rs:324 (serve_gate) recv U
- crates/camel-integration-test/tests/circuit_fallback_test.rs:171 (run_two_docs) lock U
- crates/camel-integration-test/tests/direct_reply_test.rs:53 (run_direct) lock U
- crates/camel-integration-test/tests/http_partner_scripting_test.rs:1415 (run_doc_route_dialed) lock U
- crates/camel-test/tests/cache_resilience.rs:34 (send_to_direct_tolerant) lock B
- crates/camel-test/tests/controlbus_test.rs:15 (route_status) lock U
- crates/camel-test/tests/controlbus_test.rs:38 (start_route) lock U
- crates/camel-test/tests/controlbus_test.rs:54 (suspend_route) lock U
- crates/camel-test/tests/controlbus_test.rs:70 (resume_route) lock U
- crates/camel-test/tests/direct_top_level_test.rs:37 (send_to_direct_ignoring_error) lock U
- crates/camel-test/tests/direct_top_level_test.rs:65 (send_to_direct_until_delivered) lock B
- crates/camel-test/tests/direct_top_level_test.rs:89 (route_status) lock U
- crates/camel-test/tests/direct_top_level_test.rs:107 (stop_route) lock U
- crates/camel-test/tests/direct_top_level_test.rs:122 (start_route) lock U
- crates/camel-test/tests/do_try_test.rs:25 (send_to_direct) lock U
- crates/camel-test/tests/integration_test.rs:374 (send_await_reply) lock B
- crates/camel-test/tests/jsonpath_test.rs:23 (send_to_direct) lock U
- crates/camel-test/tests/loop_test.rs:17 (send_to_direct) lock U
- crates/camel-test/tests/marshal_test.rs:15 (send_to_direct) lock U
- crates/camel-test/tests/otel_direct_hop_regression.rs:32 (route_started) lock U
- crates/camel-test/tests/otel_direct_hop_regression.rs:75 (drive_entry_in_out) lock U
- crates/camel-test/tests/otel_trace_tree_test.rs:154 (route_started) lock U
- crates/camel-test/tests/otel_trace_tree_test.rs:196 (drive_direct_in_out) lock U
- crates/camel-test/tests/script_test.rs:399 (send_to_direct) lock U
- crates/camel-test/tests/script_test.rs:423 (send_to_direct_ignore_error) lock U
- crates/camel-test/tests/xpath_test.rs:23 (send_to_direct) lock U
- crates/components/camel-component-grpc/tests/integration.rs:1293 (start_tls_consumer) recv U
- crates/components/camel-cxf/tests/consumer_unit_test.rs:31 (open_consumer_stream) connect U
- crates/components/camel-cxf/tests/consumer_unit_test.rs:51 (wait_for_consumer_request_sender) lock U
- crates/components/camel-cxf/tests/consumer_unit_test.rs:64 (wait_for_recorded_responses) lock U
