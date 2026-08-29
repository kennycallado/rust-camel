# Fresh Error-Path Metrics Audit — component-metrics-sweep

Conductor-run PRE-PLAN gate per design D1. Executed 2026-08-29 in worktree
`component-metrics-wiring` against the blessed design (D1/D2) and ADR-0012
(taxonomy + label regex `^(b-prime|e):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$`).
Scope: every crate under `crates/components/` (30 crates). READ-ONLY for
`src/`; this file is the single source of truth for wiring tasks (D1).

Method: rg sweep (`send_and_wait`, `.send(`, `log-policy`, `increment_errors`,
`RuntimeObservability`, stale-deferral phrases) ∪ `// log-policy:
outside-contract` sites, then manual semantic review of every hit: an
Err/handler failure FORWARDED to a caller who can absorb it (route pipeline,
reply channel to an invoking caller, Route supervision) is NOT b′; a failure
consumed/dropped/logged locally with no absorption path is b′; accept/retry
transients are (e). Wired `increment_errors` production call sites were
subtracted; every surviving label was checked against the ADR-0012 regex.
(g) sites (force_unhealthy_for_route) are OUT of scope per design.

## 1. Summary verdict table

| Component | Eligible candidates | Wired (prod) | Gaps | Drop | Risk |
|---|---|---|---|---|---|
| camel-component-api | 0 (contract crate) | n/a | 0 | 0 | — |
| camel-component-exec | 0 | 0 | 0 | 0 (field live) | — |
| camel-component-grpc | accept/tls-accept (+g prod sites) | 2 | 0 | 0 | low |
| camel-component-keycloak | send + auth-material + poll-auth | 2 | 1 (label fix) | 0 | low |
| camel-component-llm | 0 | 0 | 0 | 0 (field live) | — |
| camel-component-mcp | 2 disputed | 0 | 0–2 (OQ-1) | 0 | medium |
| camel-component-seda | forward send | 0 | 1 | 1 field | low |
| camel-component-surrealdb | notification | 1 | 0 (+comment fix) | 0 | low |
| camel-component-wasm | 0 | 0 | 0 | 1 field | low |
| camel-container | events/logs connect/stream | 4 | 0 | 0 | low |
| camel-controlbus | 0 | 0 | 0 | 0 (no stored field) | — |
| camel-cron | send forwarded to supervision | 0 | 0 | 0 (no stored field) | — |
| camel-cxf | response-marshalling (+route-fault OQ-2) | 1 | 0–1 (OQ-2) | 1 field (producer) | medium |
| camel-direct | send-and-wait | 1 | 0 | 0 | low |
| camel-file | poll send swallowed | 0 | 1 | 0 | low |
| camel-http | accept/accept-tls/server-task-exited | 3 | 0 | 0 (static field live — D7 superseded RETAINED) | low |
| camel-jms | consumer-send | 1 | 0 | 1 field (producer) | low |
| camel-kafka | 6 commit/commit-reply sites | 6 | 0 | 0 | low |
| camel-log | 0 (passthrough logger) | 0 | 0 | 0 (no stored field) | — |
| camel-master | 0 (all system-broken (c)) | 0 | 0 | 0 (fields live — D4 superseded to RETAINED) | n/a |
| camel-mock | 0 (test component) | 0 | 0 | 0 (no stored field) | — |
| camel-mqtt | stop/subscribe/ack/pipeline | 4 | 0 | 0 | low |
| camel-opensearch | 0 (system-broken init) | 0 | 0 | 1 field | low |
| camel-redis | pubsub/blpop close + transient budget | 6 | 0 | 0 | low |
| camel-sql | on-consume/batch/stream-list/poll-failed | 4 | 0 | 0 | low |
| camel-template | 0 (render=(a), reload=control-plane) | 0 | 0 | 4 field sites | low |
| camel-timer | fire send swallowed | 0 | 1 | 0 | low |
| camel-validator | reconnect-reseed | 1 | 0 | 0 | low |
| camel-ws | authn ×2 (+dispatch OQ-confirmed gap) | 2 | 1 | 0 | low |
| camel-xj | 0 (transform=(a)) | 0 | 0 | 0 (no stored field) | — |
| camel-xslt | reconnect-reseed | 1 | 0 | 0 | low |

Conductor-count deltas (2026-08-28 counts superseded): master 2 → 0 (the 2
hits are test no-ops; all master production sites are annotated
system-broken (c)); sql 5 → 4 distinct production labels (complete); direct
3 → 1 production site (complete; other hits are tests). seda/wasm/opensearch/
template 0 and cxf/validator/xslt/surrealdb/ws counts confirmed.

## 2. GAPS TO WIRE (Phase-1/Phase-2 task anchors)

Each row: site, category + semantic verdict, proposed label (regex-checked),
test seam (D3). No waivers requested; all rows have a viable seam.

| ID | Component | Site | Category | Semantic verdict | Proposed label | Test seam |
|---|---|---|---|---|---|---|
| G1 [x] wired | seda | `camel-component-seda/src/lib.rs:755-757` (`forward_envelope`, fire-and-forget branch: `ctx.send` Err → `warn!` only) | (b′) | Locally terminal: no reply_tx path, Err consumed by forwarder; design D1 cites this exact site | `b-prime:seda:forward-send` | `forward_envelope` is a free fn over `ConsumerContext`; crate tests already construct SedaConsumer with an rt (lib.rs:17-19 Panic helper); add per-crate recording collector (pattern: camel-direct lib.rs:527-536) and assert label on a no-consumer send |
| G2 [x] wired | timer | `camel-timer/src/lib.rs:289-292` (`context.send(exchange).await.is_err()` → silent `break`) | (b′) | Locally terminal: Err swallowed, loop exits with no signal beyond debug-level lifecycle logs | `b-prime:timer:fire-send` | timer tests construct consumers with rt (lib.rs:176/186 params); recording collector + short-interval timer; assert label when route channel is closed mid-run. NUANCE: `send` Err currently only arises from ChannelClosed (route stopping) — see OQ-3 |
| G3 [x] wired | ws | `camel-ws/src/lib.rs:1161-1165` (`forward_task`: `sender.send(envelope).is_err()` → silent `break`) | (b′) | Locally terminal dispatch of an inbound WS message; identical shape to the already-wired `b-prime:jms:consumer-send` (camel-jms consumer.rs:311-317) — inconsistency, not new doctrine | `b-prime:ws:message-dispatch` | WS integration harness exists in-crate (lib.rs:1852+ real listener tests; `rt`/Noop helpers at 1705-1710); recording rt through `WsConsumer` (fields at lib.rs:111/215) |
| G4 [x] wired | file | `camel-file/src/poll_logic.rs:604-607` (send Err → `Err(ChannelClosed)`) consumed at `camel-file/src/lib.rs:1097-1103` (`warn!` + continue loop) | (b′) | Locally terminal: poll loop absorbs the failure and keeps polling; nothing reaches supervision | `b-prime:file:poll-send` | Extensive file test suite (lib.rs:1467+ rt helper; poll_directory directly callable — tests at 3836+ call it with a context); recording collector + closed-channel context |
| G5 [x] wired | keycloak (label CORRECTION, not new site) | `camel-component-keycloak/src/keycloak_consumer.rs:175-180` | (b′) | Wired label `b-prime:keycloak:response-body` sits on the `context.send()` channel-closed site — regex-valid but semantically wrong (site is a dispatch send, not response-body processing) | rename to `b-prime:keycloak:send` (or `event-dispatch`) | Existing consumer unit harness (process_event_batch is exercised with `&dyn RuntimeObservability`, keycloak_consumer.rs:153); recording collector assert on renamed label |

Note on G2/G3/G4: `ConsumerContext::send` maps every failure to
`ChannelClosed` (camel-component-api/src/consumer.rs:277-283), i.e. the
pipeline receiver is gone (route stopped/crashed). The audit still classifies
the swallowed branches as (b′) per design D1, which explicitly lists
"ctx.send errors consumed locally" as b′ discovery targets and cites seda
lib.rs:754-756. camel-jms already wires the identical shape. If plan-bless
prefers to treat route-stopped sends as shutdown (no metric), G2/G3/G4/G5 and
the jms/kafka precedent need an explicit ADR-0012 amendment — that is an
OQ-3 decision, not an audit omission.

## 3. DROP ROWS (D2: dead stored fields + stale metrics-deferral comments)

Public signatures are PRESERVED in all rows: trait params
(`create_producer`/`create_consumer`/`Endpoint::new`-adjacent) stay, binding
to `_rt`/`_runtime`. `OpenSearchProducer::new(runtime)` and `CxfProducer::new
(..., runtime)` are public and keep their parameters.

| ID | Component | What is removed | Stale metrics-deferral comments removed/updated | Kept |
|---|---|---|---|---|
| D1 [x] dropped | camel-component-wasm | `WasmProducer.observability` (producer.rs:85) + constructor/storage plumbing (producer.rs:122) + `endpoint.rs:58` arg binding → `_rt` (endpoint.rs:43 already `_rt`) | producer.rs:81 ("`Arc<dyn RuntimeObservability>` for Phase B metric/health calls") | host_functions.rs:112 NoOp rt for GUEST-created producers (rc-66he non-goal); producer.rs:91 watchdog-deferral comment (not metrics) |
| D2 [x] dropped | camel-opensearch | `OpenSearchProducer.runtime` (producer/mod.rs:52) + plumbing; `OpenSearchProducer::new(config, runtime)` public signature PRESERVED (bind `_runtime`) | none metrics-specific — lib.rs:3 TODO(OS-022) and producer/mod.rs:95 TODO(OS-018) are feature/SigV4 debt, NOT metrics debt: keep | — |
| D3 [x] dropped | camel-template | `TemplateProducer.rt` (producer.rs:66) + `route_id` (unread — remove mandatorily); `ReloadHandler.rt` (reload.rs:65); `TemplateLifecycle.rt` (lifecycle.rs:49, incl. clones at 136/214/245/288); `TemplateEndpoint` `Mutex<Option<Arc<rt>>>` (endpoint.rs:43, set at 90) | producer.rs:54-55 (ponytail rc-d3pj note), 63-64, 67, 74-75, 113; reload.rs:63-64, 66-67 ("Phase-5 reload-loop seam: read in 5.3/5.4"), 188 (`template_reloads_total` deferred, rc-d3pj) | reload.rs `route_id`/`root`/`generation` (live reload machinery); template's OWN "Phase-5" hot-reload comments (lifecycle.rs:32-33, template_set.rs:29, closure.rs:73, reload.rs:2/91/261) are ADR-0047 plan phases, NOT metrics debt — DO NOT delete |
| D4 [x] superseded RETAINED | ~~camel-master~~ RETAINED | ~~MasterConsumer.runtime (consumer.rs:19); leadership.rs:196 runtime field; ReconcileContext.runtime (leadership.rs:181) + threading~~ — SUPERSEDED by plan-bless round 5 (e_gpt): leadership.rs:251 passes ctx.runtime to endpoint.create_consumer() — LIVE delegation plumbing for delegate consumers; removal would break delegate Consumer creation and metrics propagation. Master keeps ALL runtime fields (metrics: fields already live via emit_lifecycle). tests.rs "Phase B" labels kept (false positives) | — | — |
| D5 [x] dropped | camel-cxf | `CxfProducer.runtime` (producer.rs:44) + manual Clone arm (producer.rs:61) + `CxfProducer::new(..., runtime)` public signature PRESERVED (bind `_runtime`); component.rs:89 arg stays | consumer.rs:25-28 doc ("Phase B will use this") — field at consumer.rs:30 IS wired (G-row none, see §4): REWORD to present tense, do not remove field | consumer.rs:30 field (wired b-prime:cxf:response-marshalling) |
| D6 [x] dropped | camel-jms | `LazyJmsProducer.runtime` (component.rs:826) + `create_producer` body binding `runtime` → `_rt` (trait signature at component.rs:789-792 unchanged) | component.rs:822-825 ("Phase B will use this") removed with field; consumer.rs:30 "Phase C ADR-0012: used for…" reworded (field at consumer.rs:32 is wired at 311-317) | consumer.rs:32 field (wired) |
| D7 [x] superseded RETAINED | ~~camel-http (static)~~ RETAINED | ~~HttpStaticConsumer.runtime (static_endpoint.rs:120)~~ — SUPERSEDED at implementation (w_fast T11 stop): static_endpoint.rs:171 passes self.runtime to ServerRegistry::get_or_spawn, the SOURCE of the wired e:http:accept / accept-tls / server-task-exited metrics (lib.rs:1063-1125). Field is live plumbing. REWORD the stale comment :116-119 to present tense; field stays | — | — |
| D8 [x] dropped | seda (partial) | `SedaProducer.runtime` (lib.rs:771) + comment 767-769 — producer failures return into the pipeline (category (a), handler-owned); zero eligible sites on the producer | lib.rs:767-769 removed with field; lib.rs:566-568 comment on `SedaConsumer.runtime` becomes TRUE when G1 wires it — reword to present tense | `SedaConsumer.runtime` (lib.rs:570) — wired by G1 |

NOT drops (D2 interpretation, flagged for plan reviewer): exec
(`ExecProducer.rt`, producer.rs:30) and llm (`rt`, producer.rs:56) stored
fields are READ for success-path MetricsCollector calls (exec
record_counter/record_histogram at producer.rs:290-325; llm
`emit_cost_metric` record_histogram at producer.rs:74-80). They are not dead
weight — D2's "zero eligible sites" removal is applied only to fields with no
MetricsCollector traffic at all. Removing exec/llm fields would delete live
success-path metrics, a design non-goal (rc-6s6h).

## 4. COMPLETE COMPONENTS (wired, verified — no task)

Production labels verified against `^(b-prime|e):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$` — all valid.

- **kafka** (6): `b-prime:kafka:auto-commit-side-effect` ×2, `auto-commit-dispatch`,
  `manual-commit-dispatch`, `async-commit-reply`, `async-commit-failed`
  (consumer.rs:304/318/334/378/507/524). Manual path asserted NOT to use
  send_and_wait (consumer.rs:1618-1637 test). Producer handler-owned (producer.rs:254).
- **redis** (6): `b-prime:redis:pubsub-channel-closed`, `b-prime:redis:blpop-channel-closed`,
  `e:redis:message-transient-budget` ×2, `e:redis:message-non-transient` ×2
  (consumer.rs:298/329/340/402/432/444). Retry/pubsub/executor transient `warn!`
  sites terminate into supervision `Err` (retry.rs:55-62 doc) — forwarded, no metric required.
- **mqtt** (4): `b-prime:mqtt:stop-task-error`, `subscribe-failed`, `ack-failed`,
  `pipeline-failed` (consumer.rs:163/261/281/288).
- **sql** (4 labels): `b-prime:sql:on-consume`, `on-consume-batch` (consumer.rs:151/190 via
  `record_post_process_failure` helper at :44-55), `stream-list` (:268-276),
  `poll-failed` (:477-484 non-bridged; bridged path handler-owned :361-366, bridge-channel
  break system-broken :367-371). Pool-init failures are (g) health sites
  (consumer.rs:470-483 `g:sql:consumer-pool-init`, producer.rs:216-229
  `g:sql:producer-pool-init`) — out of scope. ADR-0012's deferred stream-list
  metric (rc-mf3) has landed. Complete.
- **container** (4): `e:container:events-connect`/`events-stream`/`logs-connect`/`logs-stream`
  (lib.rs:1562/1592/1649/1731); regression test at lib.rs:2691+.
- **grpc** (2): `e:grpc:accept` (server.rs:401), `e:grpc:tls-accept` (server.rs:427);
  producer sites all (g) `g:grpc:producer-create` + health (producer/mod.rs:139-280);
  `g:grpc:tls-read` (server.rs:131). Recording-collector test exists (server.rs:827-836, 1960+).
- **http** (3): `e:http:accept`, `e:http:accept-tls`, `e:http:server-task-exited`
  (lib.rs:1065/1099/1125). Per-request failures are response-mapped
  (handler-owned annotations, lib.rs:2360/2375 etc.).
- **ws** (2, plus G3 gap): `e:ws:authn` ×2 (lib.rs:527/537, kernel authn at transport edge).
- **jms** (1, plus D6 drop): `b-prime:jms:consumer-send` (consumer.rs:311-317).
- **keycloak** (2, plus G5 label fix): `b-prime:keycloak:response-body` (mislocated, G5),
  `e:keycloak:auth-material` (keycloak_consumer.rs:255-258); max-auth-errors
  abort is system-broken → supervision (:322-333).
- **surrealdb** (1): `b-prime:surrealdb:notification` (consumer.rs:191-197). Stream-level
  errors crash the consumer for supervision (consumer.rs:204-207) — forwarded. Producer
  handler-owned (producer.rs:650); pool/bundle sites are (g)/health. MINOR FIX ROW (not a
  wiring gap): consumer.rs:191 comment says "log-policy: handler-owned, métrica required" —
  wrong category wording for a (b′) site and a Spanish word; reword to
  "log-policy: outside-contract (b-prime), metric wired" in English.
- **cxf** (1, plus D5 drop + OQ-2): `b-prime:cxf:response-marshalling` (consumer.rs:308-313).
- **validator** (1): `e:validator:reconnect-reseed` (xsd_bridge.rs:397-401).
- **xslt** (1): `e:xslt:reconnect-reseed` (client.rs:352-356); render failures handler-owned
  (producer.rs:159/185).
- **direct** (1): `b-prime:direct:send-and-wait` (lib.rs:330-341) + regression test
  `test_send_and_wait_error_increments_errors_metric` (lib.rs:1233+). ADR-0012's
  DIR-005 bridging removal already landed.

Zero-site, zero-field components (nothing to do): controlbus (`_rt` only),
cron (send forwarded to CronService → Route supervision, lib.rs:275/283-284),
log (passthrough logger; no sites), mock (`_rt` only), xj (`_rt` only,
transform failures are (a)). camel-component-api: contract crate; hosts
`RuntimeObservability`, `ConsumerContext::send/send_and_wait` contracts and
`test_support` (Noop/Panic only — no shared recording collector; each crate
rolls its own, e.g. camel-direct lib.rs:527, sql consumer.rs:637, container
lib.rs:2704, grpc server.rs:827, kafka consumer.rs:666).

## 5. OPEN QUESTIONS (for plan reviewer; audit's best guess recorded)

- **OQ-1 — MCP pipeline-failure absorption** (`camel-component-mcp/src/consumer.rs:451-472`
  tools, :505-527 resources): normal-data `send_and_wait` Err is converted to a
  protocol-level error reply (`is_error: true` JSON-RPC tool result / error-content
  resource) sent to the invoking client. Best guess: NOT b′ — the failure is forwarded to
  the caller over the protocol (mirrors the http 500 / cxf SOAP-fault posture), and the
  crate has zero `error!`/log-policy sites. If the reviewer classifies these as b′ (client
  visibility ≠ operational signal), wire `b-prime:mcp:tool-pipeline` and
  `b-prime:mcp:resource-read` at the two `match result` sites; seam: adapter/server
  integration tests exist.
- **OQ-2 — CXF route-failure fault path** (`camel-cxf/src/consumer.rs:295-296, 323-334`):
  `send_and_wait` Err → `warn!` (handler-owned annotation) + SOAP fault to client. Same
  class as OQ-1. Tension: ADR-0012's normal-data send_and_wait rule re-categorized sql:205
  and direct:296 to b′ — but both of those lacked any reply channel; cxf replies a fault.
  Best guess: absorbed via fault reply, keep as-is. If overruled: `b-prime:cxf:route-failure`
  at consumer.rs:325.
- **OQ-3 — send-Err-as-shutdown semantics**: `ctx.send` failure is always
  `ChannelClosed` (route stopped). G2/G3/G4 (and the wired jms/keycloak precedents) count
  it as b′ per design D1's explicit seda example. Alternative reading: pure shutdown, no
  error signal. The audit keeps them as gaps for consistency with the blessed design and
  the existing jms wiring; plan-bless should either confirm or amend ADR-0012 wording.
- **OQ-4 — log component passthrough** (`camel-log/src/lib.rs:420`
  `LogLevel::Error => error!("{msg}")`): data-plane level-passthrough with no
  `// log-policy:` annotation; not in `scripts/xtask/allowlist-log-levels.txt`
  (which has no component entries at all). Pre-existing lint surface question,
  out of scope here; flagged so it is not lost.
- **OQ-5 — D2 scope interpretation**: drop rows are per-field (a component can
  wire one field and drop another — seda, cxf, jms, http), and fields with live
  non-`increment_errors` MetricsCollector traffic (exec, llm, master's
  `emit_lifecycle` metrics field) are retained. Design text reads per-component;
  this per-field/live-usage reading is the one that avoids deleting working
  metrics. Plan-bless should ratify.

## 6. Reproducibility — commands run (worktree root)

```
cd crates/components
# landscape counts per crate
for c in */; do rg -c "send_and_wait|log-policy|increment_errors|RuntimeObservability" $c/src; done
# candidate + annotation + wiring sweeps (all with -n --no-heading, src only)
rg -n --no-heading -A1 "send_and_wait" */src
rg -n --no-heading -A1 "log-policy" */src
rg -n --no-heading -A1 "increment_errors" */src
rg -ni --no-heading "Phase B|Phase-5|Phase 5|deferred|read later|will use this" */src
# plumbing / dead-field discovery
rg -n "RuntimeObservability" */src   (comment lines filtered)
rg -n "\.send\(" camel-timer camel-cron camel-file camel-log camel-mock \
   camel-controlbus camel-xj camel-component-exec camel-component-llm \
   camel-component-mcp camel-ws -g '*.rs'
# deferral-marker / escape checks
rg -n "TODO\(ADR-0012|allow-log-levels" */src        # 0 hits anywhere
rg -n "components/" scripts/xtask/allowlist-log-levels.txt   # 0 entries
# manual review slices: sed -n over the ~40 sites listed in §2-§4
```

Semantic classifications were made per ADR-0012's tie-breaker ("does this
error path produce an Exchange that flows into a Route pipeline with an
ErrorHandlerLayer?") plus the reply-channel discriminator recorded in §2/§5.


## Rulings addendum (r_glm-reviewed, conductor-approved)

- OQ-1 MCP: ABSORBED (JSON-RPC is_error reply forwards it) — not b′, out of scope.
- OQ-2 cxf SOAP fault: ABSORBED (fault reply forwards it) — not b′, out of scope.
- OQ-3 send-Err/route-stopped: WIRE ANYWAY — jms precedent (b-prime:jms:consumer-send on the identical ChannelClosed shape) + blessed D1 rule.
- OQ-4 camel-log error! passthrough: out of metrics scope; filed as bd rc-idu5 (P3, discovered-from rc-q25t).
- OQ-5 exec/llm rt fields: RETAIN — they carry live success-path metrics; rc-6s6h (success-path vocabulary) is PARTIALLY ALIVE in exec (rt reads at 291/307/314/321/325) and llm (emit_cost_metric histogram 74-80).
- G2 prerequisite: TimerConsumer must GAIN a runtime field — create_consumer currently drops `_rt`; the wiring task adds the field + passes it through.
- §4 correction: 15 wired entries (not 14); ws rows are OQ-confirmed wired.

## Closure (2026-08-29, Task 13)

All gaps wired, all drops applied, all rulings folded: sweep CLOSED.
Task commits: 6676cabd (G1 seda), fd56cc78 (G2 timer), 8fd6b1bb (G3 ws),
b35cd553 (G4 file), d2e43c40 (G5 keycloak relabel), 9c595f52 (D3 template),
a29470f5 (D1 wasm), 5522cffa (D2 opensearch), a42c9b71 (D5 cxf),
93792e7f (D6 jms), e8602be1 (D8 seda), 4f7e7f3f (D7 comment reword).
Closing commit: this one (`chore(openspec): close error-path metrics audit`,
Task 13 — surrealdb §4 comment fix + audit row marks).


## Post-merge supersede record (2026-08-29, merge of main 6c94f353)

Parallel session landed `dashboard-observability` (265625c1) + `wasm-registry-metrics` (71b7c8a3) on main while this branch was in flight:
- G1 seda forward-send: SUPERSEDED — main's uniform facade emits `e:seda:consume` at the fire-and-forget site (failures ALWAYS reach the error family; double emission avoided). Our b-prime emission + test removed in the merge resolution.
- D2 opensearch producer field: SUPERSEDED — main wires component_metrics() on the producer (opensearch emission at mod.rs:666); file restored to main shape.
- D1 wasm producer field: SUPERSEDED — main Task 4.2 wires `component_metrics()` on the producer (`wasm:invoke` emission); field LIVE as `observability`.
- D8 seda producer field: SUPERSEDED — main wires `component_metrics()` on the producer (`seda:produce` emission); field + rt binding restored per main's shape.
- Final sweep accounting: wires G2-G5 live (b-prime labels), G1 superseded-by-facade; drops D2/D3/D5/D6/D7 executed-or-superseded as recorded above; D1/D8 superseded live.
