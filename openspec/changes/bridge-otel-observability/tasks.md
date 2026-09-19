# Tasks: bridge-otel-observability

## camel-jms (Rust, escalon 1)

### Task 1.1: Feature wiring, metadata-capture harness, failing traceparent tests

**Files:**
- `crates/components/camel-jms/Cargo.toml` (modified)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified)

**Steps:**
1. In `crates/components/camel-jms/Cargo.toml`: add
   `camel-otel = { workspace = true, optional = true }` to
   `[dependencies]` and a `[features]` section with
   `otel = ["dep:camel-otel"]`, exactly mirroring
   `crates/components/camel-http/Cargo.toml`. (No `opentelemetry`
   dev-dependency: the tests build contexts through camel-otel's own API,
   see step 6.)
2. Extend `MockJmsBridge` with metadata capture: add fields
   `captured_send: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>`,
   `captured_subscribe: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>`,
   `captured_health: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>`.
   In each handler (`send`, `subscribe`, `health`) store
   `request.metadata().clone()` into the matching slot before processing.
   Change the `send` handler to return
   `Ok(Response::new(SendResponse::default()))` (Send is now exercised).
3. Change `spawn_mock_bridge` to return a struct `CapturedMetadata`
   holding the three `Arc<Mutex<Option<MetadataMap>>>` slots alongside the
   port (e.g. `struct CapturedMetadata { port: u16, send: ..., subscribe:
   ..., health: ... }`). Update the three EXISTING call sites that
   destructure the returned port (`near_cap_body_decodes_end_to_end`,
   `content_type_round_trips_through_stream`,
   `duplicate_subscription_id_surfaces_already_exists`) to the new shape —
   they keep working unchanged otherwise.
4. Add helper `fn well_formed_traceparent(v: &str) -> bool`: split on
   `-`, require exactly 4 parts, part 0 == `"00"`, parts of length 32/16/2,
   all lowercase hex.
5. Add test `traceparent_injected_on_every_rpc_shape`, gated
   `#[cfg(all(test, feature = "otel"))]`: build a context whose span
   context has trace id `4bf92f3577b34da6a3ce929d0e0e4736`, span id
   `00f067aa0ba902b7`, sampled — mirror the camel-http otel-test idiom
   (`camel_otel::propagation::extract_context(&headers).attach()` from a
   headers map containing that traceparent; see the otel tests in
   `crates/components/camel-http/src/lib.rs` around line 8621 for the exact
   imports/idiom, and keep the attach guard alive for the whole test
   body); spawn the mock bridge; connect via `bridge_service_client`;
   issue `send` (`SendRequest { destination: "queue.tp.test", body:
   vec![1], headers: HashMap::new(), content_type:
   "application/octet-stream" }`), `health` (`HealthRequest {}`), and
   `subscribe` (`SubscribeRequest { destination: "queue.tp.test",
   subscription_id: "tp-test" }`); assert each captured `MetadataMap`
   contains key `traceparent` with value exactly
   `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01` and that
   `well_formed_traceparent` accepts it.
6. Add test `no_traceparent_without_active_context`, gated
   `#[cfg(all(test, feature = "otel"))]`: with NO context attached, issue
   `send` and `health` against the mock; assert the captured
   `MetadataMap`s have no `traceparent` key.
7. Run the new tests and CONFIRM `traceparent_injected_on_every_rpc_shape`
   FAILS at the assertion (missing traceparent) while
   `no_traceparent_without_active_context` passes. This task ends with a
   failing test on record — do not implement the interceptor here.

**Tests:** (executable spec)
- `traceparent_injected_on_every_rpc_shape`: mock bridge with capture →
  attach context via the camel-http idiom, issue Send/Subscribe/Health →
  each captured metadata has `traceparent` == `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`;
  command: `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel traceparent_injected`;
  expected: FAIL before Task 1.2.
- `no_traceparent_without_active_context`: mock bridge with capture → no
  attach, issue Send + Health → captured metadata lacks `traceparent`;
  command: `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel no_traceparent_without_active_context`;
  expected: pass.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel traceparent_injected no_traceparent_without_active_context`
  runs both tests; the shape test fails at the assertion (not a compile
  error, not a missing-feature error).
- `RUSTC_WRAPPER= cargo test -p camel-component-jms` (no features) still
  compiles and passes existing tests.

- [x] 1.1

### Task 1.2: Trace-context interceptor and producer exchange injection

**Files:**
- `crates/components/camel-jms/src/trace_context.rs` (new)
- `crates/components/camel-jms/src/lib.rs` (modified: `mod trace_context;`)
- `crates/components/camel-jms/src/component.rs` (modified:
  `bridge_service_client` attaches the interceptor under
  `#[cfg(feature = "otel")]` via a type alias)
- `crates/components/camel-jms/src/producer.rs` (modified: Send request
  built as explicit `tonic::Request` with exchange trace metadata under
  `#[cfg(feature = "otel")]`)

**Steps:**
1. Create `src/trace_context.rs` with two `#[cfg(feature = "otel")]`
   items:
   - `pub(crate) struct TraceContextInterceptor;` implementing
     `tonic::service::Interceptor`: if
     `request.metadata().get(camel_otel::propagation::TRACE_PARENT_HEADER)`
     is `None`, call
     `camel_otel::propagation::inject_context(&camel_otel::opentelemetry::Context::current(), &mut HashMap::new())`
     (use whatever opentelemetry re-export path camel-otel exposes; check
     `crates/services/camel-otel/src/lib.rs` for `pub use opentelemetry`)
     and insert every produced header (traceparent, tracestate) into
     `request.metadata_mut()` via
     `tonic::metadata::MetadataKey::from_str` /
     `MetadataValue::from_str` (skip entries that fail to parse). Never
     overwrite an existing traceparent.
   - `pub(crate) fn apply_exchange_trace_context(exchange: &Exchange,
     metadata: &mut tonic::metadata::MetadataMap)`: call
     `camel_otel::propagation::inject_from_exchange(exchange, &mut headers)`
     into a fresh `HashMap<String, String>`, then insert each entry into
     `metadata` with the same skip-existing-traceparent rule.
2. In `component.rs`, handle the return-type change of
   `bridge_service_client`: the generated tonic client has no
   `add_interceptor` — use the generated constructor
   `BridgeServiceClient::with_interceptor(channel, TraceContextInterceptor)`
   under the feature. Introduce a cfg-selected type alias so all call
   sites stay source-identical in both states:
   `#[cfg(feature = "otel")] pub(crate) type BridgeClientChannel =
   tonic::service::interceptor::InterceptedService<Channel,
   TraceContextInterceptor>;` /
   `#[cfg(not(feature = "otel"))] pub(crate) type BridgeClientChannel =
   Channel;` and change the fn signature to return
   `BridgeServiceClient<BridgeClientChannel>` (keep
   `.max_decoding_message_size(bridge_decode_limit())` applied in both
   arms). `bridge_service_client` remains the single attachment point so
   every call site (producer.rs Send, consumer.rs Subscribe, health.rs and
   component.rs Health) inherits it.
3. In `producer.rs`, replace the implicit `SendRequest` wrapping: build
   `let mut request = tonic::Request::new(SendRequest { destination,
   body, headers, content_type });` (same field values as today), then
   `#[cfg(feature = "otel")] crate::trace_context::apply_exchange_trace_context(&exchange,
   request.metadata_mut());` and pass the `Request` to `client.send` (the
   generated `send` accepts `impl IntoRequest<SendRequest>`).
4. Run the Task 1.1 tests to green, then fmt/clippy in both feature
   states (clippy feature runs are per-package — cargo rejects `--features`
   alongside multiple `-p` flags).

**Tests:** (executable spec)
- `traceparent_injected_on_every_rpc_shape` (from Task 1.1) now passes via
  the interceptor (ambient attach on the current-thread tokio test
  runtime);
  command: `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel traceparent_injected`;
  expected: pass.
- `no_traceparent_without_active_context` still passes (interceptor
  injects nothing without a valid context);
  command: `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel no_traceparent_without_active_context`;
  expected: pass.
- Existing suites unchanged without the feature;
  command: `RUSTC_WRAPPER= cargo test -p camel-component-jms`;
  expected: pass.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-jms --features otel` exits 0.
- `RUSTC_WRAPPER= cargo test -p camel-component-jms` exits 0.
- `RUSTC_WRAPPER= cargo fmt --check` exits 0 (or formatting applied).
- `RUSTC_WRAPPER= cargo clippy -p camel-component-jms -p camel-bridge --all-targets -- -D warnings`
  exits 0, and
  `RUSTC_WRAPPER= cargo clippy -p camel-component-jms --all-targets --features otel -- -D warnings`
  exits 0.

- [x] 1.2

## bridges/jms (Java, escalon 1)

### Task 2.1: TraceparentInterceptor and unit tests

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/TraceparentInterceptor.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/TraceparentInterceptorTest.java` (new)

**Steps:**
1. Create `TraceparentInterceptor` implementing `io.grpc.ServerInterceptor`,
   annotated `@ApplicationScoped` +
   `@io.quarkus.grpc.GlobalInterceptor` (global registration; Quarkus 3.39
   has no `@GrpcInterceptor`). Define
   `static final Metadata.Key<String> METADATA_KEY = Metadata.Key.of("traceparent", Metadata.ASCII_STRING_MARSHALLER);`
   and
   `static final io.grpc.Context.Key<String> CONTEXT_KEY = io.grpc.Context.key("bridge-traceparent");`.
   In `interceptCall`: read `headers.get(METADATA_KEY)`; when non-null, log
   `LOG.info("bridge received traceparent=" + value)` using
   `java.util.logging.Logger` and return
   `io.grpc.Contexts.interceptCall(Context.current().withValue(CONTEXT_KEY,
   value), call, headers, next)` (canonical attach→startCall→detach;
   `Context.call` is unusable here — its `Callable` throws `Exception`);
   when null, forward `next.startCall(call, headers)` unchanged with no
   log line. `JmsBridgeService` is NOT modified — the interceptor's JUL
   line is the bridge's traceparent log surface, and `CONTEXT_KEY` is the
   service-boundary view.
2. Create `TraceparentInterceptorTest` following the plain JUnit 5 +
   Mockito style of `JmsBridgeServiceTest` (no `@QuarkusTest`). Tests:
   - `traceparentArrivesIntactThroughServiceBoundary`: register a
     recording `java.util.logging.Handler` on the interceptor's JUL
     logger; call `interceptor.interceptCall(call, metadata, handler)`
     where `metadata` has `traceparent` =
     `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`, and the
     recording `ServerCallHandler` captures the listener start inside the
     forwarded call; assert (a) one captured JUL record contains the exact
     traceparent value, and (b) inside the forwarded listener start
     `CONTEXT_KEY.get()` returns the exact same value (intact arrival at
     the service boundary).
   - `absentMetadataPassesThroughUntouched`: metadata without
     `traceparent` → forward invoked once, no JUL record, and
     `CONTEXT_KEY.get()` is null inside the forwarded call.
   - `unknownMetadataIsIgnored`: metadata with unrelated keys only →
     behaves as absent.
3. Run `./gradlew spotlessApply` then `./gradlew test` in `bridges/jms`.

**Tests:** (executable spec)
- `traceparentArrivesIntactThroughServiceBoundary`: interceptor → metadata
  traceparent → JUL record contains exact value AND `CONTEXT_KEY.get()`
  equals it at the forwarded call; command:
  `cd bridges/jms && ./gradlew test --tests '*TraceparentInterceptorTest*'`;
  expected: pass.
- `absentMetadataPassesThroughUntouched` / `unknownMetadataIsIgnored`:
  no traceparent → no log, null context key; same command; expected:
  pass.

**Acceptance:**
- `cd bridges/jms && ./gradlew test` exits 0 (all existing + new suites).
- `cd bridges/jms && ./gradlew spotlessCheck` exits 0.

- [x] 2.1

## bridges (Java, escalon 2)

### Task 3.1: Structured JSON console logs in all three bridges

**Files:**
- `bridges/jms/build.gradle.kts` (modified)
- `bridges/xml/build.gradle.kts` (modified)
- `bridges/cxf/build.gradle.kts` (modified)
- `bridges/jms/src/main/resources/application.yml` (modified)
- `bridges/xml/src/main/resources/application.yml` (modified)
- `bridges/cxf/src/main/resources/application.yml` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JsonConsoleLoggingTest.java` (new)

**Steps:**
1. In each bridge's `build.gradle.kts` dependencies block add
   `implementation("io.quarkus:quarkus-logging-json")` (version managed by
   the enforced quarkus platform BOM — no explicit version).
2. Read each `application.yml` and merge into the existing `quarkus:` tree
   the current-spelling key `quarkus.log.console.json.enabled: true`
   (nested as `log: console: json: enabled: true` in YAML). Do not add
   legacy or deprecated keys; do not reorder or rewrite unrelated keys.
3. Verify the PortAnnouncer ready line is untouched (it writes via
   `System.out` directly, never through a logger — confirm by reading
   `bridges/jms/src/main/java/org/rustcamel/jms/PortAnnouncer.java`; no
   change expected).
4. Add ONE executed verification of the JSON formatter in bridges/jms,
   choosing the path by a mechanical rule:
   - First check public constructibility:
     `javap -classpath $(find ~/.gradle -name 'quarkus-logging-json-*.jar' | head -1) io.quarkus.logging.json.JsonFormatter io.quarkus.logging.json.Config`.
   - Path A (preferred, if the ctor and `Config` are public): a plain
     JUnit test `JsonConsoleLoggingTest` that formats a synthetic
     `LogRecord` through `JsonFormatter` and asserts the output is a
     single line parsing as JSON (use Jackson from the test classpath or
     a minimal brace/key check with `java.util.regex`) containing the
     message, level, and logger name.
   - Path B (fallback): mirror the boot pattern of
     `bridges/xml/src/test/java/org/rustcamel/xmlbridge/HealthIntegrationTest.java`
     as a `@QuarkusTest` `JsonConsoleLoggingTest` in bridges/jms whose
     body asserts that the root logger's console handler's formatter
     class name is the extension's JSON formatter
     (`LogManager.getLogManager().getLogger("").getHandlers()` → first
     handler → `getFormatter().getClass().getName()` contains `Json`).
   Record which path was taken and why in the task result.
5. Run each bridge's test suite.

**Tests:** (executable spec)
- `JsonConsoleLoggingTest` (either path): formatter output / wiring is
  JSON; command: `cd bridges/jms && ./gradlew test --tests '*JsonConsoleLoggingTest*'`;
  expected: pass.
- Existing suites remain green with the new formatter dependency:
  `cd bridges/jms && ./gradlew test` exits 0; same for `bridges/xml` and
  `bridges/cxf`; expected: pass.
- Ready-protocol invariant: `PortAnnouncer` source still emits
  `{"status":"ready","port":...}` via `System.out.println` — verified by
  reading the file (no logger involved); expected: unchanged.

**Acceptance:**
- `./gradlew test` exits 0 in `bridges/jms`, `bridges/xml`, `bridges/cxf`.
- `git diff` on the three `application.yml` files shows only the
  `quarkus.log.console.json.enabled` addition.
- `JsonConsoleLoggingTest` exists and passes (executed verification, not
  conditional).

- [x] 3.1
  - Result: Path A — io.quarkus.logging.json.runtime.JsonFormatter (public no-arg ctor); jms 46 / xml 13 / cxf 163 tests green.

## Bookkeeping

### Task 4.1: Escalon-3 bd split, docs

**Files:**
- `crates/components/camel-jms/CONTEXT.md` (modified)
- `bridges/README.md` (modified)

**Steps:**
1. From the repo root `/home/kenny/dev/rust-camel` (never the worktree),
   run:
   `bd create "Bridge otel escalon 3: quarkus-opentelemetry with own OTLP export" --description="<English lead sentence> Escalon 3 verbatim from rc-qtn1: (3) quarkus-opentelemetry con export OTLP propio (M/L, nueva superficie egress — decidir si el collector es trusted, config reflection native-image). Ventana natural: tras el bump Quarkus del change bridge-lockstep-hardening (una sola re-verificación native-image). Motivado durante pre-release 0.6.0." -t feature -p 3 --deps discovered-from:rc-qtn1 --json`
   and record the new issue id.
2. In `crates/components/camel-jms/CONTEXT.md`, add a short section
   documenting: the `otel` cargo feature, that bridge RPCs carry
   `traceparent` gRPC metadata when a context is active (constructor-level
   interceptor + producer exchange injection), and the no-tracing
   fallback. Follow the citation style already used in that file.
3. In `bridges/README.md`, add a short note under the existing structure:
   bridges log JSON lines to stdout (quarkus-logging-json console
   formatter) and the JMS bridge logs received `traceparent` metadata at
   info level.
4. Record the new bd id in the task result — it is required for the
   rc-qtn1 close note and the parked report.

**Tests:** (executable spec)
- `cd /home/kenny/dev/rust-camel && bd show <new-id> --json` returns the
  issue with `priority: 3`, `issue_type: feature`, and a dependency on
  rc-qtn1; expected: pass.

**Acceptance:**
- New bd issue exists with the verbatim escalon-3 text, P3, feature type,
  discovered-from rc-qtn1.
- Both docs updated, English, no unrelated edits.

- [x] 4.1
  - Result: escalon-3 bd filed as rc-cuteb (P3, feature, discovered-from rc-qtn1, verbatim text).
