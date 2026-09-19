# Design: bridge-otel-observability

## Approach

Two independent, additive mechanisms — one per side of the IPC.

**Rust injection (escalon 1, camel-jms).** The single production-path client
constructor `bridge_service_client()` (`crates/components/camel-jms/src/component.rs`)
gains a tonic interceptor that injects the ambient `opentelemetry::Context::current()`
as `traceparent`/`tracestate` metadata via
`camel_otel::propagation::inject_context` + `TRACE_PARENT_HEADER`. The
generated client has no `add_interceptor`; the constructor switches to the
generated `BridgeServiceClient::with_interceptor(channel, interceptor)`
under the feature, and a `cfg`-selected type alias
(`type BridgeClientChannel = InterceptedService<Channel, ...>` vs `Channel`)
keeps every call site source-identical in both feature states. The
interceptor never overwrites an existing `traceparent` — explicit metadata
wins. Because spans in this codebase are created with explicit contexts
(`start_with_context`) and an opentelemetry `ContextGuard` cannot be held
across `.await` in spawned futures, the producer additionally sets the
metadata explicitly from `exchange.otel_context` using
`camel_otel::propagation::inject_from_exchange` at the Send call site — the
same precedent as camel-http's outgoing header injection. Subscribe/Health
carry ambient context when present (tests, future runtime wiring) and inject
nothing otherwise, which is the specified no-tracing fallback. The
camel-otel dependency is optional behind an `otel` feature, mirroring
camel-http/camel-kafka/camel-ws; injection tests compile under that feature
and follow camel-http's otel-test idiom (`extract_context(...).attach()`,
no extra dev-dependency).

**Java surfacing (escalon 1, bridges/jms).** A
`TraceparentInterceptor` (`io.grpc.ServerInterceptor`, registered globally
with `@io.quarkus.grpc.GlobalInterceptor` + `@ApplicationScoped`) reads the
`traceparent` metadata key, logs it at INFO via java.util.logging, and
stores it in the gRPC `Context` via `Contexts.interceptCall` (the canonical
attach/detach idiom; `Context.call` cannot be used — its `Callable`
declares `throws Exception`) so bridge service code can read it at the
service boundary.
`JmsBridgeService` itself stays unmodified: the interceptor's log line is
the "bridge logs the received traceparent" surface, and Context-key
visibility is what "arrives intact through the bridge service" means (the
service layer reads the same value). This avoids the fragile
`org.jboss.logging`-backend capture that a service-level log assertion
would need under plain JUnit5 (jboss-logging routes to the log4j2 backend
on the test classpath, not JUL). Absent metadata passes through untouched
and logs nothing.

**Structured JSON logs (escalon 2, all bridges).** Add the
`io.quarkus:quarkus-logging-json` extension to each bridge's
`build.gradle.kts` (managed by the enforced platform BOM) and set the
current-spelling key `quarkus.log.console.json.enabled: true` in the
existing `application.yml` (the extension also defaults to JSON when
present; the explicit key keeps the behavior deterministic). This swaps
only the console formatter of the existing JBoss LogManager pipeline (both
JUL and JBoss Logging records route through it) — no framework swap. The
PortAnnouncer `{"status":"ready",...}` line is a stdout protocol, not a
log record, and stays as-is.

## Affected crates

- `crates/components/camel-jms`: optional camel-otel dep + `otel` feature;
  trace-context interceptor module; producer explicit injection; metadata
  capture tests in `bridge_client_test.rs`.
- `crates/services/camel-otel`: additive `Context` re-export only (facade);
  no propagation-logic changes.
- `crates/services/camel-bridge`: no code change (client construction stays
  in camel-jms; `camel-bridge` remains process/lifecycle only).
- `bridges/jms`, `bridges/xml`, `bridges/cxf`: interceptor (jms only),
  logging-json dependency + `application.yml` config, tests.

## Architecture boundaries

Data-plane only: trace metadata rides existing RPCs; no control-plane,
proto, or egress changes. Observability stays a Services concern
(camel-otel owns propagation; components consume), matching ADR-0036's
bridge IPC trust boundary — the traceparent is untrusted correlation data
on the receiving side (logged, never interpreted). Bridge-side logging
respects the handler-contract boundary of ADR-0012: the interceptor logs
at INFO and changes no existing error levels.

## Alternatives considered

- Ambient-only interception (no producer call-site injection): rejected —
  production Send would carry no trace, because route spans use explicit
  contexts and ambient attach cannot span `.await` points.
- Java-side otel SDK span continuation (extract and parent a server span):
  rejected — that is escalon 3 (new egress surface, native-image config),
  explicitly out of scope.
- Custom JUL `Formatter` per bridge instead of quarkus-logging-json:
  rejected — hand-rolled JSON, reflection registration for native-image,
  more code than the Quarkus-native extension.

## Spec path record

Existing specs were inspected (`observability`, `jms`, `jms-message-fidelity`,
`bridge-transport-security`, `otel-lifecycle`): none covers IPC context
propagation or bridge log structure. A spec change was therefore needed and
the OpenSpec flow is used (this change). Per the mission mandate, the plan
gate is r_glm review plus a stage-4 e_glm gate.
