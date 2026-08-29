# Tasks: component-metrics-sweep

- Spec: metrics-collection-wiring — Requirement: Error-path metric
  completeness; owns all five scenarios.
- Anchor: every row cites `audit.md` (G=wire gap, D=drop) — the audit is
  the site enumeration. Per-crate tasks per expert-gated schema (30-min
  units).
- Wire-task template (Tasks 1-5): recording-collector test FIRST (red =
  no increment_errors recorded), then wire, then green. In-tree
  recording pattern: camel-direct lib.rs:527-536.
- Drop-task template (Tasks 6-12): removal only; existing suite green +
  the exact grep below; public signatures preserved via `_` bindings.

## Phase 1: Wire the five gaps

### Task 1: seda G1

- **Files**: `crates/components/camel-component-seda/src/lib.rs` (modified)
- **Command**: `cargo test -p camel-component-seda && cargo fmt --check && cargo clippy -p camel-component-seda -- -D warnings`
- **Steps**: wire `increment_errors(route_id, "b-prime:seda:forward-send")` beside the warn at lib.rs:755-757 (fire-and-forget ctx.send Err); REWORD the consumer comment lib.rs:566-568 to present tense (TRUE now); do NOT touch the producer comment 767-769 (Task 12 owns it).
- **Tests**: `seda_forward_send_failure_counts_b_prime` / consumer with recording rt, forced send Err (Panic helper seam lib.rs:17-19) / consumer tick / exactly one `b-prime:seda:forward-send`. Red-first.
- **Acceptance**: test green; `rg -c "Phase B will use this" crates/components/camel-component-seda/src/lib.rs` = 1 (producer's, until Task 12).
- [x] 1

### Task 2: timer G2

- **Files**: `crates/components/camel-timer/src/lib.rs` (modified)
- **Command**: `cargo test -p camel-component-timer && cargo fmt --check && cargo clippy -p camel-component-timer -- -D warnings`
- **Steps**: TimerConsumer GAINS `runtime: Arc<dyn RuntimeObservability>`; `create_consumer` stops dropping `_rt` (seam sites :176/:186) and stores it; the send-Err site (:289-292) emits `b-prime:timer:fire-send` before the break.
- **Tests**: `timer_fire_send_failure_counts_b_prime` / consumer with recording rt, forced send Err / fire tick / exactly one `b-prime:timer:fire-send`. Red-first.
- **Acceptance**: test green; command green.
- [x] 2

### Task 3: ws G3

- **Files**: `crates/components/camel-ws/src/lib.rs` (modified)
- **Command**: `cargo test -p camel-component-ws && cargo fmt --check && cargo clippy -p camel-component-ws -- -D warnings`
- **Steps**: forward_task sender.send Err site (lib.rs:1161-1165) emits `b-prime:ws:message-dispatch` before the break (jms precedent shape — camel-jms consumer.rs:311-317).
- **Tests**: `ws_message_dispatch_failure_counts_b_prime` / forward_task with failing sender (test_rt seam lib.rs:1705-1710) / forward tick / exactly one `b-prime:ws:message-dispatch`. Red-first.
- **Acceptance**: test green; command green.
- [x] 3

### Task 4: file G4

- **Files**: `crates/components/camel-file/src/lib.rs` (modified); `crates/components/camel-file/src/poll_logic.rs` (modified)
- **Command**: `cargo test -p camel-component-file && cargo fmt --check && cargo clippy -p camel-component-file -- -D warnings`
- **Steps**: emission is restricted to the `context.send` failure ONLY — the send-Err site at poll_logic.rs:604-607 (Err(ChannelClosed) propagation). `scan_candidates` and `poll_one_file` failures must NOT emit the label (they are not the dispatch send). Wire `b-prime:file:poll-send` at the poll_logic.rs:604-607 site via the poll context's runtime (add the runtime/observability plumbing the poll path needs — follow how the recording-rt helper at lib.rs:1467+ reaches consumers; if poll_logic has no runtime access today, thread `Arc<dyn RuntimeObservability>` through the poll context struct from lib.rs). The lib.rs:1097-1103 warn consumer stays as-is (it logs the propagated error).
- **Tests**: `file_poll_send_failure_counts_b_prime` / poll_directory with recording rt + a context whose send returns Err (tests at lib.rs:3836+ construct contexts) / poll tick / exactly one `b-prime:file:poll-send`. Negative: a scan failure (unreadable directory) produces ZERO emissions with this label. Red-first.
- **Acceptance**: test green; command green.
- [x] 4

### Task 5: keycloak G5

- **Files**: `crates/components/camel-component-keycloak/src/keycloak_consumer.rs` (modified); `crates/components/camel-component-keycloak/CONTEXT.md` (modified)
- **Command**: `cargo test -p camel-component-keycloak && cargo fmt --check && cargo clippy -p camel-component-keycloak -- -D warnings`
- **Steps**: rename label `b-prime:keycloak:response-body` → `b-prime:keycloak:send` at keycloak_consumer.rs:173-180 (site is the context.send channel-closed dispatch, not response-body); update the asserting test; update CONTEXT.md's label reference.
- **Tests**: `keycloak_send_label_renamed` / process_event_batch seam (keycloak_consumer.rs:153) with recording rt / run / asserts `b-prime:keycloak:send`. Red-first (old string asserted → fails before rename... invert: write test asserting NEW label first → red until rename).
- **Acceptance**: `rg -c "keycloak:response-body" crates/components/camel-component-keycloak/` = 0; command green.
- [x] 5

## Phase 2: Drop the seven dead fields

### Task 6: template D3

- **Files**: `crates/components/camel-template/src/{producer.rs,reload.rs,lifecycle.rs,endpoint.rs}` (modified)
- **Command**: `cargo test -p camel-template && cargo fmt --check && cargo clippy -p camel-template -- -D warnings`
- **Steps** (audit D3): remove `TemplateProducer.rt` (producer.rs:66) + `route_id` (unread — mandatory); `ReloadHandler.rt` (reload.rs:65); `TemplateLifecycle.rt` (lifecycle.rs:49 + clones 136/214/245/288); `TemplateEndpoint` `Mutex<Option<Arc<rt>>>` (endpoint.rs:43, set at 90). Remove ONLY audit-enumerated metrics-deferral comments (producer.rs:54-55, 63-64, 67, 74-75, 113; reload.rs:63-64, 66-67, 188). ADR-0047 "Phase-5" hot-reload comments (lifecycle.rs:32-33, template_set.rs:29, closure.rs:73, reload.rs:2/91/261) KEPT.
- **Acceptance**: command green; each audit-D3-enumerated marker gone —
  `rg -c "Stored now, read later" crates/components/camel-template/src/` → no matches;
  `rg -n "Phase-5 reload-loop seam" crates/components/camel-template/src/reload.rs` → no matches;
  `rg -n "template_reloads_total" crates/components/camel-template/src/reload.rs` → no matches (deferred-metric note 188);
  producer.rs ponytail/rc-d3pj notes at 54-55/63-64/67/74-75/113 gone
  (`rg -n "rc-d3pj" crates/components/camel-template/src/producer.rs` → no matches);
  ADR-0047 comments SURVIVE: `rg -n "Phase-5" crates/components/camel-template/src/lifecycle.rs` ≥1.
- [x] 6

### Task 7: wasm D1

- **Files**: `crates/components/camel-component-wasm/src/{producer.rs,endpoint.rs}` (modified)
- **Command**: `cargo test -p camel-component-wasm && cargo fmt --check && cargo clippy -p camel-component-wasm -- -D warnings`
- **Steps** (audit D1): remove `WasmProducer.observability` (producer.rs:85) + plumbing (producer.rs:122) + endpoint.rs:58 arg binding → `_rt`; remove comment producer.rs:81. RegistryComponentContext NoOp stays (rc-66he); producer.rs:91 watchdog comment kept.
- **Acceptance**: command green; `rg -c "Phase B" crates/components/camel-component-wasm/src/producer.rs` → no matches (the :91 comment says watchdog, not Phase B).
- [x] 7

### Task 8: opensearch D2

- **Files**: `crates/components/camel-opensearch/src/producer/mod.rs` (modified)
- **Command**: `cargo test -p camel-component-opensearch && cargo fmt --check && cargo clippy -p camel-component-opensearch -- -D warnings`
- **Steps** (audit D2): remove `OpenSearchProducer.runtime` (producer/mod.rs:52) + plumbing; `new(config, runtime)` public signature PRESERVED (bind `_runtime`). OS-022/OS-018 TODOs kept.
- **Acceptance**: command green; `rg -n "runtime" crates/components/camel-opensearch/src/producer/mod.rs` shows only `_runtime` bindings/signature.
- [x] 8

### Task 9: cxf D5

- **Files**: `crates/components/camel-cxf/src/{producer.rs,consumer.rs}` (modified)
- **Command**: `cargo test -p camel-component-cxf && cargo fmt --check && cargo clippy -p camel-component-cxf -- -D warnings`
- **Steps** (audit D5): remove `CxfProducer.runtime` (producer.rs:44) + manual Clone arm (producer.rs:61); `CxfProducer::new(..., runtime)` signature PRESERVED (bind `_runtime`); component.rs:89 arg stays. REWORD consumer.rs:25-28 doc to present tense (field at :30 IS wired b-prime:cxf:response-marshalling) — field NOT removed.
- **Acceptance**: command green; `rg -c "Phase B will use this" crates/components/camel-cxf/src/consumer.rs` → no matches; consumer field intact (`rg -c "runtime" crates/components/camel-cxf/src/consumer.rs` ≥1).
- [x] 9

### Task 10: jms D6

- **Files**: `crates/components/camel-jms/src/{component.rs,consumer.rs}` (modified)
- **Command**: `cargo test -p camel-component-jms && cargo fmt --check && cargo clippy -p camel-component-jms -- -D warnings`
- **Steps** (audit D6): remove `LazyJmsProducer.runtime` (component.rs:826); `create_producer` body binds `_rt` (trait signature 789-792 unchanged); component.rs:822-825 comment removed with field; REWORD consumer.rs:30 "Phase C ADR-0012" comment to present tense (field at :32 wired at 311-317).
- **Acceptance**: command green; `rg -c "Phase B will use this" crates/components/camel-jms/src/component.rs` → no matches; consumer field intact.
- [x] 10

### Task 11: http D7 (SUPERSEDED to RETAINED at implementation — w_fast T11 stop: static_endpoint.rs:171 feeds ServerRegistry::get_or_spawn, the source of the wired e:http:* metrics; field is live)

- **Files**: `crates/components/camel-http/src/static_endpoint.rs` (modified — comment reword ONLY)
- **Command**: `cargo test -p camel-component-http && cargo fmt --check && cargo clippy -p camel-component-http -- -D warnings`
- **Steps**: REWORD the stale "Phase B will use this" comment (static_endpoint.rs:116-119) to present tense stating the field feeds ServerRegistry::get_or_spawn (source of e:http:accept / accept-tls / server-task-exchanged... use the exact three labels from lib.rs). Field, plumbing, and signatures UNTOUCHED.
- **Acceptance**: command green; `rg -c "Phase B will use this" crates/components/camel-http/src/static_endpoint.rs` → no matches; `rg -c "runtime" crates/components/camel-http/src/static_endpoint.rs` ≥1 (field intact).
- [x] 11

### Task 12: seda-producer D8

- **Files**: `crates/components/camel-component-seda/src/lib.rs` (modified)
- **Command**: `cargo test -p camel-component-seda && cargo fmt --check && cargo clippy -p camel-component-seda -- -D warnings`
- **Steps** (audit D8): remove `SedaProducer.runtime` (lib.rs:771) + comment 767-769 ONLY (producer failures return into the pipeline, category (a) handler-owned; consumer comment reword is Task 1's — do not duplicate).
- **Acceptance**: command green; `rg -c "Phase B will use this" crates/components/camel-component-seda/src/lib.rs` → no matches (Task 1 reworded the consumer one, this removes the producer one).
- [x] 12

### Task 13: cross-sweep verification + audit closure

- **Files**: `openspec/changes/component-metrics-sweep/audit.md` (modified — all rows resolved); `crates/components/camel-component-surrealdb/src/consumer.rs` (modified — §4 comment fix at :191: Spanish "métrica"/wrong-category → correct English b-prime note)
- **Command**:
  - `rg -c "Phase B will use this" crates/components/camel-component-seda/src/lib.rs crates/components/camel-jms/src/component.rs crates/components/camel-http/src/static_endpoint.rs crates/components/camel-component-wasm/src/producer.rs crates/components/camel-cxf/src/consumer.rs` → every file 0 / no matches
  - `rg -c "Stored now, read later" crates/components/camel-template/src/` → no matches
  - `rg -c "keycloak:response-body" crates/components/camel-component-keycloak/` → no matches
  - `rg -n "métrica" crates/components/camel-component-surrealdb/src/consumer.rs` → no matches
  - `cargo test --workspace --lib` → green
- **Steps**: surrealdb consumer.rs:191 — replace the Spanish/wrong-category
  comment with exactly: `// b-prime: locally terminal notification-send
  failure (wired at the increment_errors site below)` (verify the wired
  site reference against :194 while editing). audit.md: mark G1-G5
  `[x] wired`, D1-D3/D5-D8 `[x] dropped`, D4 `[x] superseded RETAINED`,
  and append a closure line naming the task commit SHAs.
- **Acceptance**: all checks pass; every audit row marked resolved.
- [x] 13
