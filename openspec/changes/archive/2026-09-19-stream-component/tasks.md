# Tasks: stream-component

## Phase 1: Producers stream:out/stream:err (slim)

### Task 1.1: camel-stream crate scaffold + StreamConfig + StreamComponent

**Files:**
- `crates/components/camel-stream/Cargo.toml` (new)
- `crates/components/camel-stream/src/lib.rs` (new)
- `crates/components/camel-stream/README.md` (new)
- `Cargo.toml` (modified — workspace dependencies)

**Steps:**
1. Create `crates/components/camel-stream/Cargo.toml` mirroring `crates/components/camel-log/Cargo.toml`: package `camel-component-stream`, description "Stream (stdio) component for rust-camel", all metadata `workspace = true`; deps `camel-api.workspace = true`, `camel-component-api.workspace = true`, `async-trait.workspace = true`, `tokio.workspace = true`, `tower.workspace = true`, `tracing.workspace = true`; dev-deps `camel-component-api = { workspace = true, features = ["test-support"] }`, `serde_json.workspace = true`, `tokio = { workspace = true, features = ["macros", "rt"] }`; `[lints] workspace = true`. Do NOT add `camel-core` (lint-component-deps).
2. Add to root `Cargo.toml` `[workspace.dependencies]` beside the camel-component-log entry: `camel-component-stream = { path = "crates/components/camel-stream", version = "=0.49.0" }`. Verify the crate is picked up by the workspace members glob (`cargo metadata --no-deps | grep camel-component-stream` from the worktree).
3. In `src/lib.rs` define `pub enum StreamTarget { Out, Err, In }` with `Display`/`FromStr` (accepting exactly `out`, `err`, `in`).
4. Define `pub struct StreamConfig` with `#[derive(Debug, Clone, UriConfig)]`, `#[uri_scheme = "stream"]`, `#[uri_config(skip_impl, metadata(scheme = "stream", description = "stdio data-plane adapter: out/err producers, in consumer", producer, consumer), crate = "camel_component_api")]` (TimerConfig pattern at `crates/components/camel-timer/src/lib.rs:30-40`). Fields: `pub target: StreamTarget` (URI path), `#[uri_param(name = "appendNewline", default = "true")] pub append_newline: bool`, `#[uri_param(name = "charset", default = "utf-8")] pub charset: String`, `#[uri_param(name = "frame", kind = "enum:line,raw,fixed", default = "line")] pub frame: StreamFrame` (new `pub enum StreamFrame { Line, Raw, Fixed }` with FromStr), `#[uri_param(name = "size")] pub size: Option<u64>`.
5. Implement inherent `fn validate(&self) -> Result<(), CamelError>`: reject non-`utf-8` charset with `CamelError::Config` naming utf-8 as the only v1 charset; reject `frame=fixed` with missing or zero `size` with `CamelError::InvalidUri`; reject unknown path (anything not `out`/`err`/`in`) with `CamelError::InvalidUri`. Implement the manual `impl UriConfig for StreamConfig` (scheme/from_uri/from_components/validate) delegating to the inherent validate, exactly as TimerConfig does.
6. Implement `pub struct StreamComponent;` with `new()`/`Default` and `impl Component for StreamComponent` (scheme `"stream"`, metadata from `StreamConfig::metadata()`, `create_endpoint` parsing `StreamConfig::from_uri`). Implement `struct StreamEndpoint { uri, config }` with `impl Endpoint`: `create_consumer` returns `Err(CamelError::EndpointCreationFailed("stream endpoint consumer not wired yet (Phase 2)".into()))` for now; `create_producer` returns `Err(CamelError::EndpointCreationFailed("stream producer not wired yet (Task 1.2)".into()))` for now — the crate must compile at task end, and both stubs are replaced by Tasks 1.2/2.2.
7. Create `README.md` (one-paragraph component description, scheme table out/err/in, ruling reference).
8. Run `cargo fmt` and `cargo clippy -p camel-component-stream --all-targets -- -D warnings` in the worktree; fix findings.

**Tests:** (in `crates/components/camel-stream/src/lib.rs` `#[cfg(test)] mod tests`)
- **Command:** `cargo test -p camel-component-stream --lib` — fails before the crate exists (Phase-1 start) and passes after this task.
- **Expected:** all six tests below green only after this task lands.
- `config_parses_out_default`: `StreamConfig::from_uri("stream:out")` → target Out, append_newline true, charset utf-8, frame Line.
- `config_parses_in_raw`: `StreamConfig::from_uri("stream:in?frame=raw")` → target In, frame Raw.
- `config_rejects_unknown_path`: `StreamConfig::from_uri("stream:logfile")` → `Err(CamelError::InvalidUri)`.
- `config_rejects_non_utf8_charset`: `StreamConfig::from_uri("stream:out?charset=latin-1")` → `Err(CamelError::Config)` whose message contains `utf-8`.
- `config_rejects_fixed_without_size`: `StreamConfig::from_uri("stream:in?frame=fixed")` and `...&size=0` → `Err(CamelError::InvalidUri)`.
- `component_scheme_is_stream`: `StreamComponent::new().scheme() == "stream"`.

**Acceptance:**
- `cargo test -p camel-component-stream --lib` passes with the six tests above.
- `cargo clippy -p camel-component-stream --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-component-stream` exits 0.
- Root `Cargo.toml` diff contains exactly the one workspace-dependency line.

- [x] 1.1

### Task 1.2: StreamProducer — body as DATA to fd 1/2

**Files:**
- `crates/components/camel-stream/src/lib.rs` (modified — implementation AND the seam-dependent tests below live in its `#[cfg(test)] mod stream_producer_tests`; crate-private seams cannot be reached from `tests/` integration files)

**Steps:**
1. Implement the write path behind a testable seam: `StreamProducer` holds `config: StreamConfig` and `sink: Arc<tokio::sync::Mutex<dyn Sink>>` (the Arc-Mutex exists ONLY to give `Clone` producers `&mut` sink access — it is not the serialization mechanism) where `#[async_trait] trait Sink: Send + Sync { async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>; async fn flush(&mut self) -> std::io::Result<()>; }` (own small trait). Production sinks: `StdoutSink`/`StderrSink` wrapping `tokio::io::stdout()`/`stderr()`. The ONE serialization mechanism (R10 — two endpoints targeting fd 1 must never interleave a body and its newline): module-level `static FD1_LOCK: tokio::sync::Mutex<()>` and `static FD2_LOCK: tokio::sync::Mutex<()>`; the producer acquires the static guard for the whole write+flush of ONE concatenated buffer (body bytes + optional newline appended before the single `write_all` — never two separate writes). Replace the Task 1.1 `create_producer` stub with `Ok(BoxProcessor::new(StreamProducer::new(self.config.clone())))`.
2. `StreamProducer::new(config) -> Self` picks StdoutSink/StderrSink by `config.target`. `pub(crate) fn with_sink(config, sink)` constructor for tests. `#[derive(Clone)]` via `Arc<Mutex<Sink>>` interior.
3. Implement `impl Service<Exchange> for StreamProducer` (LogProducer pattern, `crates/components/camel-log/src/lib.rs:394`): clone config, materialize the body through `Body::into_bytes(100 * 1024 * 1024)` (`crates/camel-api/src/body.rs:215` — 100 MiB bound mirroring camel-file `crates/components/camel-file/src/lib.rs:2368`); on materialization error return the `CamelError` (nothing written); if `append_newline` append exactly one `b'\n'` byte; `write_all` then `flush` under the per-fd mutex; return `Ok(exchange)` unchanged.
4. `impl Clone for StreamProducer` (Arc-based) so the BoxProcessor clone semantics match siblings.
5. Run `cargo fmt`, `cargo clippy -p camel-component-stream --all-targets -- -D warnings`.

**Tests:** (unit tests in `#[cfg(test)] mod stream_producer_tests` inside `src/lib.rs`, using a `VecSink` implementing `Sink` with an internal `Arc<std::sync::Mutex<Vec<u8>>>` and `StreamProducer::with_sink`; drive the boxed `Service` future directly with `futures` poll or the crate's existing test-support helpers)
- **Command:** `cargo test -p camel-component-stream --lib` — each test below fails at Task-1.1 state (stub `create_producer`, no `Service` impl) and passes after this task.
- **Expected:** all listed tests green only after this task lands.
- `line_mode_appends_single_newline`: exchange body `hello` (no newline), `stream:out` default config → sink bytes are exactly `hello\n`.
- `raw_mode_writes_verbatim`: `stream:out?appendNewline=false`, body bytes `a,b,c` → sink bytes exactly `a,b,c` (no newline).
- `err_target_dispatches_stderr`: `stream:err` config → `StreamProducer::new(cfg).target_fd() == 2` (add `pub(crate) fn target_fd(&self) -> i32` returning 1 for Out, 2 for Err, error/panic-free 0 for In which never constructs a producer).
- `structured_body_materializes_to_bytes`: body is a `serde_json::Value` (structured) → produced bytes equal the value's byte serialization via `Body::into_bytes`.
- `credential_body_passes_verbatim`: body containing `password=hunter2` → sink contains the exact string unmodified (no redaction).
- `empty_body_line_mode_writes_just_newline`: empty bytes body, default config → sink exactly `\n`.
- `concurrent_producers_serialize_on_fd`: two `StreamProducer` instances (separate `with_sink` constructions sharing one `Arc`-backed `VecSink`), both targeting fd 1's static guard, spawned from two tasks with multi-line bodies → final bytes are the two intact logical writes in some order (e.g. `aa\naa\n` + `bb\nbb\n` concatenated whole, never interleaved mid-write).

**Acceptance:**
- `cargo test -p camel-component-stream --lib` passes (Task 1.1 + 1.2 tests).
- `cargo clippy -p camel-component-stream --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: camel-bundles slim registration

**Files:**
- `crates/camel-bundles/Cargo.toml` (modified)
- `crates/camel-bundles/src/lib.rs` (modified)

(Decision, resolved: `crates/camel-cli/Cargo.toml` is NOT modified — camel-cli registers stream only through the camel-bundles cascade and never names `StreamComponent` directly, matching how the slim set reaches `camel run`.)

**Steps:**
1. Add `camel-component-stream.workspace = true` to `crates/camel-bundles/Cargo.toml` `[dependencies]` beside the log/timer entries (unconditional — slim, no feature gate).
2. In `crates/camel-bundles/src/lib.rs` cascade (registration block at ~line 300), add `ctx.register_component(camel_component_stream::StreamComponent::new());` after the `camel_component_log` line.
3. Extend the slim-polarity test `boot_slim_registers_core_without_bridges` (~line 503): add `"stream"` to the `for scheme in [...]` list.
4. Run `cargo test -p camel-bundles --lib boot_slim` in the worktree; then `cargo xtask lint-component-deps` and `cargo xtask lint-publish-registration` from the worktree root; follow their guidance (register the crate wherever the publish list requires). Run `cargo xtask schema --check` — if it fails because the new component's endpoint metadata must enter the schema, regenerate with the documented schema command (schema regen is in-scope; ONLY the deptree fixture is regen-forbidden).
5. Deptree drift check (mission-order conflict, executed HERE so the trail is explicit): run `cargo test -p camel-cli --test feature_profiles` and EXPECT the `default_closure_matches_golden` failure. Capture the diff output; verify every `+`/`-` line names `camel-component-stream` only (no third-party crate, no version churn on other crates). Record the diff summary in the task report. Do NOT regenerate `crates/camel-cli/tests/fixtures/default-deptree.txt` — regeneration is the human's exclusive decision (parked report carries it).
6. Do NOT touch `crates/camel-cli/tests/fixtures/default-deptree.txt` (restated: gate handled per step 5).

**Tests:**
- **Command:** `cargo test -p camel-bundles --lib boot_slim` — fails before registration (scheme `stream` absent) and passes after.
- **Expected:** green after this task.
- `boot_slim_registers_core_without_bridges` (modified existing test at `crates/camel-bundles/src/lib.rs:503`): slim boot → `ctx.registry().get("stream").is_some()` for the new scheme, `jms` still absent.

**Acceptance:**
- `cargo test -p camel-bundles --lib` passes (slim test now includes stream).
- `cargo xtask lint-component-deps` and `cargo xtask schema --check` exit 0 from the worktree.
- `cargo xtask lint-publish-registration` yields EXACTLY the intrinsic CaseB finding for `camel-component-stream` (unpublished-crate lifecycle: exits 1 by design until the human first-publishes, registers trustpub, and flips the manifest state to `registered`) — a parked gate state, same treatment as the deptree fixture, recorded in the final report.
- `cargo test -p camel-cli --test feature_profiles` FAILS with drift lines naming `camel-component-stream` only; diff recorded in the task report; fixture untouched (`git diff --stat` shows no change to `default-deptree.txt`).

- [x] 1.3

### Task 1.4: Tracer-stdout collision warning

**Files:**
- `crates/camel-bundles/src/lib.rs` (modified)
- `crates/camel-cli/src/commands/run.rs` (modified — the actual owner of the `camel run` boot path; verified path, adjust only if the file moved)

**Steps:**
1. Add to `crates/camel-bundles/src/lib.rs`: `pub fn warn_stream_stdout_collision(routes: &[camel_core::RouteDefinition], config: &CamelConfig) -> bool`. Implementation: return false unless `config.observability.tracer.enabled && config.observability.tracer.outputs.stdout.enabled` (match the exact field path used at `crates/camel-config/src/context_ext.rs:791`; if the config type reachable from `CamelConfig` differs, follow the real path and note it in the doc comment). Scan each route definition's steps for `to:` URIs with scheme `stream` and path `out` (use the same URI-shape helpers available in camel-core/camel-bundles; a simple prefix scan `to_uri.starts_with("stream:out")` plus query-stripping is acceptable and documented). If both hold, emit `warn!(target: "camel_bundles", ...)` naming: route id(s) using stream:out, that the tracer stdout sink is enabled, and the operator resolution (disable `[observability.tracer.outputs.stdout]` or route the tracer to stderr/file), then return true.
2. Wire the call into the `camel run` boot path immediately after routes are loaded and before `ctx` start: call `camel_bundles::warn_stream_stdout_collision(&routes, &camel_config)`. (`camel job` wiring lands with Phase 2's allowlist task — it boots through the same bundle path; add the job call there.)
3. `cargo fmt`; `cargo clippy -p camel-bundles -p camel-cli --all-targets -- -D warnings` (use the repo's actual clippy invocation shape for camel-cli: `cargo clippy -p camel-cli -- -D warnings`).

**Tests:** (unit tests in camel-bundles `#[cfg(test)]`)
- **Command:** `cargo test -p camel-bundles --lib collision` — fails before the helper exists and passes after.
- **Expected:** all three tests green after this task. The `warn!` emission is one `tracing` call on the same code path the bool return covers; the return-value assertions are the decision-path coverage (recorded weakening: no tracing-capture assertion).
- `collision_warns_when_stream_out_and_tracer_stdout`: a `RouteDefinition` whose steps include a `to` step with URI `stream:out` and a `CamelConfig` with tracer enabled + stdout output enabled → fn returns true.
- `no_warn_when_tracer_stdout_disabled`: same route, tracer stdout disabled → returns false.
- `no_warn_without_stream_out_route`: tracer stdout enabled, route with only `to: log:x` steps → returns false.
(Construct `CamelConfig` via its `Default`/serde path; if the tracer sub-config is not reachable from `Default`, build via `toml::from_str` with a minimal `[observability.tracer]` table mirroring existing camel-bundles test fixtures.)

**Acceptance:**
- `cargo test -p camel-bundles --lib collision` passes.
- `cargo clippy -p camel-bundles --all-targets -- -D warnings` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.4

### Task 1.5: ADR-0080 + crate CONTEXT.md

**Files:**
- `docs/adr/0080-stream-component-stdio-data-plane.md` (new — follow the existing ADR filename convention in `docs/adr/`)
- `crates/components/camel-stream/CONTEXT.md` (new)

(Decision, resolved: `crates/components/CONTEXT.md` is NOT modified — verified it defines domain language, it does not enumerate component crates.)

**Steps:**
1. Write ADR-0080 in accepted-limitations flavor, STE-plain, citing: the binding ruling path `docs/rulings/2026-09-18-stream-stdio-primitive-illumination.md`; ADR-0060 line ~108 as inapplicable (process supervision vs the process's own fd 0/1/2); ADR-0076 (redaction plane covers logging/tracing only — stdio data output is operator-owned trusted egress; future optional `?mask=true` reusing the log masker is NOT v1); the four REJECTED items (blocking mid-route `read` step, `set-prompt` step, REPL/conversational state, isatty-driven control flow) with one-line reasons; the tracer-stdout collision posture (warn, never auto-mux); v1 charset utf-8-only with line-mode skip+metric on invalid UTF-8; the job lifecycle boundary (allowlist entry only; job live-source waits belong to the job epic — rc-d5dgc coordination note; `camel run` hosts the composition today).
2. Write `crates/components/camel-stream/CONTEXT.md` mirroring `crates/components/camel-log/CONTEXT.md` structure: purpose, scheme table, config surface, semantics (line/raw/fixed framing, EOF-graceful, no-redaction data plane), ADR-0080 citation.
3. Apply the `ste-writing` skill discipline to both files (plain sentences, no AI slop).

**Tests:** (no runtime tests — documentation task)
- **Command:** `grep -c "ADR-0060" docs/adr/0080-stream-component-stdio-data-plane.md && grep -c "ruling-e-opus-stream-stdio-2026-09-18" docs/adr/0080-stream-component-stdio-data-plane.md` — fails before the ADR exists, passes after this task.
- **Expected:** both citation counts ≥ 1 after this task.

**Acceptance:**
- ADR file exists with status accepted, cites the ruling and ADR-0060/0076, records all four REJECTED items.
- `cargo xtask lint-context-citations` exits 0.
- `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0 IF ADR inclusion requires it (check how docs/src/SUMMARY.md includes ADRs; if ADRs are auto-included by directory listing, run the build; if the environment lacks nix, record that in the task report — do not block).

- [x] 1.5

## Phase 2: Consumer stream:in + job allowlist + example

### Task 2.1: StreamConsumer — framing, EOF-graceful, backpressure

**Files:**
- `crates/components/camel-stream/src/lib.rs` (modified — implementation AND the seam-dependent tests below live in its `#[cfg(test)] mod stream_consumer_tests`; `with_reader`/`StreamReader` are crate-private)

**Steps:**
1. Implement `pub struct StreamConsumer { config: StreamConfig, started: AtomicBool, runtime: Arc<dyn RuntimeObservability>, reader: Box<dyn StreamReader> }` (TimerConsumer pattern `crates/components/camel-timer/src/lib.rs:200-330`). Constructors — state these exactly once and use consistently: `pub(crate) fn new(config: StreamConfig, runtime: Arc<dyn RuntimeObservability>) -> Self` (reader = tokio stdin impl) and `pub(crate) fn with_reader(config: StreamConfig, runtime: Arc<dyn RuntimeObservability>, reader: Box<dyn StreamReader>) -> Self` (test seam). Define `#[async_trait] pub(crate) trait StreamReader: Send + Sync` with three methods mirroring the framing: `async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize>`, `async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> std::io::Result<usize>` (reads up to n, returns bytes read, 0 = EOF), `async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize>`; one production impl over `tokio::io::Stdin` (`AsyncReadExt::read_until` for line mode — byte-oriented, never `read_line`; `read` loop for fixed; `read_to_end` for raw).
2. `#[async_trait] impl Consumer for StreamConsumer`: `start(&mut self, context: ConsumerContext)` — double-start guard (AtomicBool compare_exchange, error "stream consumer already started"); the main loop uses `tokio::select!` over two arms: arm one awaits `context.cancel_token().cancelled()` and on fire breaks the loop; arm two awaits the next frame from `self.reader` (dispatched by `config.frame`: Line → `read_until_newline` returning byte count 0 as EOF; Raw → single `read_to_end` then EOF; Fixed → `read_exact_chunk(size)` returning 0 as EOF) and, for each frame received, executes the per-frame body below. Per-frame body: line mode strips the trailing `\n` then one preceding `\r` from the buffer, validates UTF-8 — on invalid UTF-8 call `self.runtime.metrics().increment_errors(context.route_id(), "b-prime:stream:decode")` and continue to the next frame without sending; otherwise construct the message body (line mode: `String::from_utf8(bytes)` — already validated, so map a defensive failure into the same decode-skip path; raw/fixed: build from the byte buffer the way `camel-file` constructs byte bodies) into `Exchange::new(message)`, then `context.send(exchange).await` — on `Err` call `increment_errors(context.route_id(), "b-prime:stream:fire-send")` and break; on `Ok` proceed to the next frame (sequential backpressure — the next read starts only after the send completes; never read ahead). After the loop (EOF or cancel or closed channel): `self.started.store(false)`; return `Ok(())`. `stop(&mut self)` → store false, return `Ok(())`.
3. Framing dispatch by `config.frame`: `Line` → read_until_newline per iteration, final unterminated line (bytes > 0 before EOF) still emits; `Raw` → single read_to_end then one frame if non-empty, else zero; `Fixed` → loop read_exact_chunk(size), 0 bytes → EOF, partial (< size) → emit partial chunk then next read returns 0 → EOF.
4. Never call `isatty` or any terminal detection (grep the final file to confirm — no `IsTerminal`/`isatty` import).
5. `cargo fmt`; `cargo clippy -p camel-component-stream --all-targets -- -D warnings`.

**Tests:** (unit tests in `#[cfg(test)] mod stream_consumer_tests` inside `src/lib.rs` — an `Arc<std::sync::Mutex<Vec<u8>>>`-backed `StreamReader` impl, a `RuntimeObservability` test double mirroring how camel-timer tests construct theirs via `camel-component-api` test-support, `ConsumerContext::new(tx, cancel_token, route_id)` with `tokio::sync::mpsc`, collect envelopes then assert)
- **Command:** `cargo test -p camel-component-stream --lib` — each test below fails at Phase-1 state (no consumer) and passes after this task.
- **Expected:** all listed tests green only after this task lands.
- `one_line_one_exchange`: reader `"a\nb\nc\n"` → 3 envelopes, bodies exactly `a`, `b`, `c` (no terminators).
- `final_unterminated_line_emits`: reader `"x\ny"` (no trailing newline) → 2 envelopes, second body `y`.
- `crlf_stripped`: reader `"w\r\n"` → 1 envelope, body `w`.
- `raw_frame_one_exchange`: `frame=raw`, reader 3 lines → 1 envelope with the full input as body.
- `raw_empty_zero_exchanges`: `frame=raw`, reader empty → 0 envelopes, `start` returns `Ok(())`.
- `fixed_frames_and_partial_chunk`: `frame=fixed&size=4`, reader 10 bytes → 3 envelopes: 4, 4, 2 bytes.
- `eof_no_input_completes_gracefully`: default frame, reader empty (EOF immediately) → 0 envelopes, `start` returns `Ok(())` (load-bearing).
- `cancellation_stops_cleanly`: reader blocks forever (pending async), cancel token fires → `start` returns `Ok(())`, no envelope.
- `invalid_utf8_line_skipped_with_metric`: reader `"ok\n\xff\xfe bad\nok2\n"` → 2 envelopes (`ok`, `ok2`), error double recorded one `b-prime:stream:decode` increment.
- `closed_channel_ends_loop_with_metric`: receiver dropped before start → send fails → error double recorded `b-prime:stream:fire-send`, `start` returns `Ok(())`.
- `sequential_backpressure_no_read_ahead`: a gated reader whose second `read_until_newline` panics if called before the first envelope is consumed from the mpsc channel — run consumer, receive first envelope, only then allow the second read → completes with 2 envelopes (proves read-after-send ordering).

**Acceptance:**
- `cargo test -p camel-component-stream --lib` passes (all 11 consumer tests + earlier tasks' tests).
- `cargo clippy -p camel-component-stream --all-targets -- -D warnings` exits 0.
- `grep -c 'isatty\|IsTerminal' crates/components/camel-stream/src/lib.rs` returns 0.

- [x] 2.1

### Task 2.2: StreamEndpoint consumer wiring for stream:in

**Files:**
- `crates/components/camel-stream/src/lib.rs` (modified)

**Steps:**
1. Replace the Phase-1 `create_consumer` stub: for `config.target == In` return `Ok(Box::new(StreamConsumer::new(self.config.clone(), Arc::clone(&rt))))` using the endpoint's `rt: Arc<dyn RuntimeObservability>` argument (Task 2.1 signature `new(config, runtime)`); for `Out`/`Err` keep `Err(CamelError::EndpointCreationFailed("stream:out and stream:err do not support consumers; use stream:in".into()))`.
2. Confirm the consumer startup mode matches what the runtime expects for always-ready sources: mirror exactly how `TimerEndpoint::create_consumer` constructs its consumer (no extra startup handshake; the runtime's `ConsumerStartupMode::Immediate` path applies the way it does for timer).
3. `cargo fmt`; `cargo clippy -p camel-component-stream --all-targets -- -D warnings`.

**Tests:** (unit tests in lib.rs)
- **Command:** `cargo test -p camel-component-stream --lib` — fails at Phase-1 state (stub rejects consumers) and passes after.
- **Expected:** all three tests green after this task.
- `endpoint_creates_consumer_for_in`: `StreamConfig::from_uri("stream:in")` endpoint → `create_consumer(NoOp/runtime test double)` returns Ok.
- `endpoint_rejects_consumer_for_out_and_err`: same for `stream:out` / `stream:err` → `Err(EndpointCreationFailed)` with message naming `stream:in`.
- `endpoint_rejects_producer_for_in`: `create_producer` on `stream:in` → `Err(EndpointCreationFailed)` naming out/err.

**Acceptance:**
- `cargo test -p camel-component-stream` (lib unit tests, all modules) passes.
- `cargo clippy -p camel-component-stream --all-targets -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: camel job consumer allowlist admits exactly stream:in

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified — collision-warn call at the job boot seam if `camel job` owns route load + config there; follow Task 1.4's helper)

**Steps:**
1. In `document.rs`: change `JOB_SAFE_CONSUMER_SCHEMES` from `[&str; 4] = ["direct", "seda", "log", "mock"]` to `[&str; 5] = ["direct", "seda", "log", "mock", "stream"]`.
2. Extend `validate_consumer_uri` (document.rs ~1131): after the scheme allowlist check, when `scheme == "stream"` require the URI path to be exactly `in` (use the existing `uri_base`/path helpers in the file; `from(stream:in?frame=line)` passes, `from(stream:out)` fails). The rejection error must name `stream:in` as the only accepted stream consumer path, include the route's from-URI and the allowlist, and drive the existing exit-code-2 load-error path (unchanged plumbing — the function returns `Err(String)`).
3. Update the doc comment above the const (fail-closed gate description) to mention the stream path restriction.
4. In the `camel job` boot path (where routes are loaded and `camel_config` is in scope — `execute_job` in mod.rs ~970), add `camel_bundles::warn_stream_stdout_collision(&routes, &camel_config);` beside the Phase-1 run-command wiring.
5. `cargo fmt`; `cargo clippy -p camel-cli -- -D warnings`.

**Tests:** (extend the existing document.rs test module — the `document_tests.rs` file owning `validate_consumer_uri` tests; add)
- **Command:** `cargo test -p camel-cli --lib job_gate` — `job_gate_accepts_stream_in` fails before the allowlist change; the rejection tests fail before the path check lands.
- **Expected:** all six tests green after this task.
- `job_gate_accepts_stream_in`: `validate_consumer_uri("stream:in?frame=line")` → `Ok(())`.
- `job_gate_rejects_stream_out_as_consumer`: `validate_consumer_uri("stream:out")` → `Err` whose message contains `stream:in`.
- `job_gate_rejects_stream_err_as_consumer`: `validate_consumer_uri("stream:err")` → `Err`.
- `job_gate_still_rejects_kafka`: `validate_consumer_uri("kafka:topic")` → `Err` (regression preserved).
- `job_gate_still_accepts_direct`: `validate_consumer_uri("direct:in")` → `Ok(())`.
- `job_gate_producers_unrestricted_with_stream_sinks`: a document-level test (same harness as the existing document tests) whose route is `from: direct:in` with `to: stream:out` and `to: stream:err` steps → the document loads (consumer gate passes; `to:` URIs unrestricted, cli-jobs scenario "producers are unrestricted" regression).

**Acceptance:**
- `cargo test -p camel-cli --lib` (or the test target owning document.rs tests) passes with the six new tests.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: Interactive composition example + end-to-end pipe test

**Files:**
- `examples/stream-echo/Cargo.toml` (new — mirror the layout of a minimal existing example, e.g. `examples/config-basic`; verify whether examples are workspace members and follow the convention)
- `examples/stream-echo/src/main.rs` (new)
- `examples/stream-echo/README.md` (new)
- `crates/camel-cli/tests/stream_run_pipe.rs` (new — the design's `echo x | camel run` parity test)
- `crates/camel-cli/tests/fixtures/stream-echo-route.yaml` (new — route document consumed by the test)

**Steps:**
1. Example `stream-echo`: boots a context through the camel-bundles cascade (mirror how `examples/` entries that register slim components construct `CamelContext` — check `examples/circuit-breaker` or nearest equivalent for the current API), defines the route `from("stream:in") → transform (simple: prefix each line, e.g. `echo: ${body}`) → to("stream:out")` with a `to("stream:err")` prompt step before the transform (prompt body `Enter text:`), starts, and awaits route completion. LIFECYCLE FACT (verified, must be reflected in README): EOF completes the consumer's route — the stream:in consumer returns and the route drains — but `camel run`'s PROCESS lifetime is signal-managed (`crates/camel-cli/src/commands/run.rs:563-593` exits on SIGINT/SIGTERM); under a pipe the practical pattern is: pipe closes → route completes → send SIGTERM/SIGINT for graceful process exit 0. README documents both invocations: interactive at a TTY (Ctrl-C to exit) and batch under a pipe (`echo hello | cargo run -p stream-echo`-style composition with the signal caveat).
2. Parity test `stream_run_pipe.rs` (camel-cli package so `env!("CARGO_BIN_EXE_camel")` resolves): write a temp route document (or use the checked-in fixture) with `from: stream:in` → transform `echo: ${body}` → `to: stream:out`; spawn `env!("CARGO_BIN_EXE_camel")` with `run <route.yaml>` and `Stdio::piped()`; write `alpha\nbeta\n` to its stdin, close stdin; OBSEVE stdout until it contains `echo: alpha\necho: beta\n` under a bounded timeout (observe-then-signal pattern, `crates/camel-cli/tests/run_empty_discovery_test.rs:60` `run_observe_then_signal`); then send SIGTERM via `kill -TERM <pid>` (graceful shutdown path); await exit; assert stdout is exactly `echo: alpha\necho: beta\n` and exit code 0. This is the design's `echo x | camel run` end-to-end parity test; NO job-runner or run-runner lifecycle change is made — the signal-managed exit is the existing runner semantic.
3. `cargo fmt`; `cargo clippy -p stream-echo --all-targets -- -D warnings` (adjust target name to the example's crate name; `examples/*` are workspace members per root `Cargo.toml` line 11 — no membership edit needed) and the repo's camel-cli clippy invocation.

**Tests:**
- **Command:** `cargo test -p camel-cli --test stream_run_pipe` — fails at Phase-1 state (stream:in not a consumer; route load or run fails) and passes after this task.
- **Expected:** green only after this task lands (example + fixture + test together).
- `echo_pipe_end_to_end_via_camel_run`: pipe `alpha\nbeta\n` into `camel run <route.yaml>`, observe stdout `echo: alpha\necho: beta\n`, SIGTERM, exit code 0.

**Acceptance:**
- `cargo test -p camel-cli --test stream_run_pipe` passes.
- `cargo build -p stream-echo` (or workspace examples build path) exits 0.
- `cargo clippy` on the example target and camel-cli test target exits 0.

- [x] 2.4
