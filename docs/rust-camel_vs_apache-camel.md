# rust-camel vs Apache Camel: Análisis Exhaustivo

**Fecha:** 2026-03-07 (actualizado)  
**Propósito:** Evaluar el estado actual de rust-camel como alternativa a Apache Camel

---

## Resumen Ejecutivo

**rust-camel cubre ~14-20% de Apache Camel** pero con **arquitectura sólida y calidad de producción** en lo implementado.

**Production Readiness:** ~74% (mejorado desde 72% con Bean Registry: dependency injection system elimina acoplamiento de lógica de negocio a rutas)

---

## Cobertura por Área

| Área | Cobertura | Estado |
|------|-----------|--------|
| **Core** | 85% | ✅ Sólido |
| **EIPs** | 18% (12/60+) | ⚠️ Básico |
| **Componentes** | 3% (8/300+) | ❌ Mínimo |
| **DSL/Builder** | 85% | ✅ Bueno |
| **Error Handling** | 80% | ✅ Fuerte |
| **Testing** | 50% | ⚠️ Básico |
| **Security** | 60% | ✅ Producción |
| **Languages** | 30% (2/10+) | ⚠️ Básico |
| **Observability** | 65% | ✅ En progreso |
| **Configuration** | 70% | ✅ Nuevo |
| **DI/Beans** | 70% | ✅ Nuevo |

---

## ✅ Lo Implementado (Calidad Producción)

### Core Features (80%)

| Feature | Status | Notas |
|---------|--------|-------|
| CamelContext | ✅ | Lifecycle completo con graceful shutdown |
| Routes | ✅ | RouteDefinition con builder pattern |
| Exchange | ✅ | InOnly/InOut patterns, properties, error state, **correlation IDs** |
| Message | ✅ | Headers, Body (Text/Json/Bytes/**Stream**/Empty), **lazy evaluation** |
| Processor | ✅ | Tower Service trait, async closures |
| Endpoint | ✅ | Consumer/Producer pattern |
| Component | ✅ | Registry por scheme |
| Registry | ✅ | Component registry, **mutex poisoning recovery**, **Bean registry** |
| Graceful Shutdown | ✅ | CancellationToken con drain |
| Backpressure | ✅ | Channel-based con buffer configurable |
| Concurrency Model | ✅ | Sequential/Concurrent por route |
| RouteController | ✅ | Lifecycle management, autoStartup, startupOrder |
| ControlBus | ✅ | Dynamic route control (start/stop/suspend/resume/status) |
| **SupervisingRouteController** | ✅ | **NEW: Auto-recovery con exponential backoff** |
| **Bean Registry** | ✅ | **NEW: Dependency injection with #[bean_impl] macro, YAML DSL integration** |

**Missing vs Apache Camel:**
- [x] ~~Type Converters (Body type coercion)~~ ✅ DONE: `convert_body_to(BodyType)` with streaming support
- [x] ~~Bean Registry / DI~~ ✅ DONE: BeanRegistry + BeanProcessor trait + proc-macros + YAML integration
- [ ] Lifecycle callbacks (Suspend/Resume reales - actual es alias para stop/start)
- [ ] JMX/Management API
- [ ] Clustering/HA
- [ ] Spring/CDI integration (N/A en Rust)

### Bean Registry / Dependency Injection (NEW)

Apache Camel tiene un sistema de beans para separar lógica de negocio de rutas. rust-camel implementa:

| Feature | Status | Notas |
|---------|--------|-------|
| **BeanRegistry** | ✅ | HashMap<String, Arc<dyn BeanProcessor>>, O(1) lookup |
| **BeanProcessor trait** | ✅ | `async fn call(&self, method, &mut Exchange)` + `fn methods() -> Vec<&str>` |
| **#[bean_impl] macro** | ✅ | Genera BeanProcessor impl desde impl block |
| **#[handler] macro** | ✅ | Marca métodos como invocables desde rutas |
| **Parameter binding** | ✅ | `body: T`, `headers: Headers`, `exchange: &mut Exchange` (inference by name) |
| **Return types** | ✅ | `Result<T, E>` donde `E: Display`, auto JSON serialization |
| **Error handling** | ✅ | BeanError enum (NotFound, MethodNotFound) |
| **YAML DSL integration** | ✅ | `bean: { name: "myService", method: "process" }` |
| **Type safety** | ✅ | Compile-time validation (async, &self, parameter types) |
| **Integration tests** | ✅ | 8 tests (registration, invocation, validation, error cases) |
| **Documentation** | ✅ | 309-line README with API reference + bean-demo example |

**Example:**

```rust
// Define bean with handlers
pub struct OrderService;

#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        Ok(ProcessedOrder { /* ... */ })
    }
    
    #[handler]
    pub async fn validate(&self, body: Order, headers: Headers) -> Result<Order, String> {
        // Validation logic
        Ok(body)
    }
}

// Register and use
let mut registry = BeanRegistry::new();
registry.register("orderService", OrderService);

// Invoke from code
registry.invoke("orderService", "process", &mut exchange).await?;

// Or from YAML DSL
routes:
  - id: "process-order"
    from: "direct:orders"
    steps:
      - bean:
          name: "orderService"
          method: "process"
```

**Missing vs Apache Camel:**
- [ ] Constructor dependency injection
- [ ] Lifecycle callbacks (@PostConstruct, @PreDestroy)
- [ ] Bean scopes (singleton, prototype, request)
- [ ] Conditional bean registration (@Conditional)
- [ ] Service discovery integration
- [ ] @Consume annotation (auto-subscribe to endpoint)

### EIPs Implementados (12 patterns)

Apache Camel tiene 60+ EIPs. rust-camel implementa:

| EIP | Status | Features |
|-----|--------|----------|
| **Filter** | ✅ | Predicate-based routing, nested pipelines |
| **Splitter** | ✅ | Sequential/parallel, aggregation strategies, stop-on-exception |
| **Aggregator** | ✅ | Correlation by header, completion by size/predicate, custom aggregation |
| **Wire Tap** | ✅ | Fire-and-forget, async spawn |
| **Multicast** | ✅ | Sequential/parallel, aggregation strategies, concurrency limiting |
| **Stop** | ✅ | Pipeline termination control |
| **Content-Based Router** | ✅ | Via Filter composition |
| **Choice/When/Otherwise** | ✅ | Content-Based Router, short-circuit, optional otherwise |
| **Message Translator** | ✅ | map_body, set_body, set_body_fn |
| **Set Header** | ✅ | Static and dynamic (set_header_fn) |
| **Log EIP** | ✅ | .log(message, level) con 5 niveles; mensajes en YAML DSL evaluados como Simple Language (soporta `${body}`, `${header.name}`, interpolación mixta) |
| **Tracer EIP** | ✅ | **NEW: Automatic message flow tracing, 3 detail levels (Minimal/Medium/Full)** |

### Components (8 implementados)

Apache Camel tiene 300+ components. rust-camel implementa:

| Component | Consumer | Producer | Features |
|-----------|----------|----------|----------|
| **timer:** | ✅ | ❌ | period, delay, repeatCount |
| **log:** | ❌ | ✅ | showHeaders, showBody, showAll, showCorrelationId |
| **direct:** | ✅ | ✅ | Synchronous in-memory, request-reply |
| **mock:** | ❌ | ✅ | Testing, exchange recording, assertions |
| **file:** | ✅ | ✅ | **Streaming:** Zero-copy file reading (ReaderStream). **Security:** path traversal protection, 10MB materialize limit. **Timeouts:** readTimeout/writeTimeout. Consumer: noop, delete, move, include/exclude, recursive. Producer: fileExist, tempPrefix, autoCreate |
| **http/https:** | ✅ | ✅ | **Streaming:** Stream-to-HTTP piping without materialization. **Security:** SSRF protection (allowPrivateIps, blockedHosts). Consumer: Axum-based server, path routing. Producer: reqwest client, method override, redirects, timeouts |
| **controlbus:** | ❌ | ✅ | Route lifecycle control (start/stop/suspend/resume/status) |
| **redis:** | ✅ | ✅ | **NEW: strings/hashes/lists/sets/sorted-sets/pub-sub** |

### DSL/Builder (85%)

```rust
// Route building - fluent typestate pattern
RouteBuilder::from("timer:tick?period=1000")
    .route_id("my-route")           // route identification
    .auto_startup(false)            // lazy startup
    .startup_order(10)              // ordered startup
    .set_header("key", Value::String("value".into()))
    .process(|ex| async move { Ok(ex) })
    .filter(|ex| ex.input.body.as_text().is_some())
        .to("log:filtered")
        .end_filter()
    .wire_tap("log:monitor")
    .split(SplitterConfig::new(split_body_lines()).parallel(true))
        .to("direct:per-fragment")
        .end_split()
    .multicast()
        .parallel(true)
        .to("direct:a")
        .to("direct:b")
        .end_multicast()
    .aggregate(AggregatorConfig::new("groupId"))
    .choice()
        .when(|ex| ex.input.body.as_text().is_some())
            .to("direct:text")
        .end_when()
        .otherwise()
            .to("direct:other")
        .end_otherwise()
    .end_choice()
    .log("Complete", LogLevel::Info)
    .error_handler(ErrorHandlerConfig::dead_letter_channel("log:dlc"))
    .circuit_breaker(CircuitBreakerConfig::new().failure_threshold(3))
    .concurrent(16)  // or .sequential()
    .build()?
```

| Feature | Status |
|---------|--------|
| from(uri) | ✅ |
| to(uri) | ✅ |
| process(closure) | ✅ |
| set_header / set_header_fn | ✅ |
| map_body / set_body / set_body_fn | ✅ |
| filter().end_filter() | ✅ |
| split().end_split() | ✅ |
| multicast().end_multicast() | ✅ |
| wire_tap(uri) | ✅ |
| aggregate(config) | ✅ |
| stop() | ✅ |
| log(message, level) | ✅ |
| choice().when().end_when().otherwise().end_otherwise().end_choice() | ✅ |
| error_handler(config) | ✅ |
| circuit_breaker(config) | ✅ |
| concurrent(n) / sequential() | ✅ |
| convert_body_to(BodyType) | ✅ |
| route_id(id) | ✅ |
| auto_startup(bool) | ✅ |
| startup_order(n) | ✅ |

### Error Handling (80%)

| Feature | Status | Notas |
|---------|--------|-------|
| Dead Letter Channel | ✅ | Configurable DLC endpoint |
| Retry with backoff | ✅ | Exponential backoff configurable |
| onException matching | ✅ | Predicate-based exception matching |
| handled_by endpoint | ✅ | Route specific exceptions to handlers |
| Per-route handler | ✅ | Overrides global |
| Global handler | ✅ | Fallback for routes without handler |
| Error propagation via direct: | ✅ | Request-reply pattern |
| Circuit Breaker | ✅ | Closed/Open/HalfOpen states, configurable thresholds |

**Missing:**
- [ ] RedeliveryPolicy (fine-grained retry control)
- [ ] Exception clauses by type
- [ ] Handled flag vs propagated
- [ ] Transactions/UnitOfWork

### Security (60%)

| Feature | Status | Notas |
|---------|--------|-------|
| SSRF Protection (HTTP) | ✅ | Block private IPs by default, allowPrivateIps override, blockedHosts |
| Path Traversal (File) | ✅ | validate_path_is_within_base() prevents directory escape |
| Timeouts (File I/O) | ✅ | readTimeout/writeTimeout (default 30s) prevents hanging |
| Mutex Poisoning Recovery | ✅ | Graceful recovery instead of panic |
| Input Validation | ⚠️ | Basic, needs more comprehensive approach |

**Missing:**
- [ ] Rate limiting
- [ ] Authentication/Authorization hooks
- [ ] Secrets management integration
- [ ] Audit logging

### Observability (65%) - Mejorado

| Feature | Status | Notas |
|---------|--------|-------|
| Correlation IDs | ✅ | UUID per exchange for distributed tracing |
| Metrics Hooks | ✅ | MetricsCollector trait (infrastructure) |
| Structured Logging | ✅ | tracing crate integration |
| Log Component | ✅ | showCorrelationId parameter |
| **Tracer EIP** | ✅ | **NEW: Automatic message flow tracing, wrap each step, 3 detail levels** |

**Tracer EIP - 3 niveles de detalle:**
- `Minimal` — exchange ID + route + step type solamente
- `Medium` — + headers + body preview (truncated)
- `Full` — + todas las propiedades + body completo

**Missing:**
- [ ] OpenTelemetry integration (config struct existe, no implementado)
- [ ] Prometheus metrics exporter
- [ ] Health check endpoints
- [ ] Tracer: OutputFormat::Plain (solo JSON funciona ahora)

### Configuration (70%) - NEW

| Feature | Status | Notas |
|---------|--------|-------|
| **CamelConfig** | ✅ | Carga desde `Camel.toml` |
| **Profiles** | ✅ | `[default]`, `[production]`, `[dev]` con deep merge |
| **CAMEL_PROFILE env var** | ✅ | Selección de profile en runtime |
| **CAMEL_* env vars** | ✅ | Override de config por variable de entorno |
| **Route discovery (YAML glob)** | ✅ | Encuentra rutas en archivos YAML por patrón |
| **configure_context()** | ✅ | Configura tracing subscribers + supervision en CamelContext |
| **Supervision config** | ✅ | initial_delay, backoff_multiplier, max_delay, max_attempts |

```toml
# Camel.toml
[default]
log_level = "info"
shutdown_timeout_secs = 30

[production]
log_level = "warn"

[default.supervision]
initial_delay_ms = 1000
backoff_multiplier = 2.0
max_delay_ms = 60000
max_attempts = 5
```

**Missing:**
- [ ] Config watching/hot-reload integration end-to-end
- [ ] Vault/secrets integration
- [ ] Per-component config in TOML

### SupervisingRouteController - NEW

| Feature | Status | Notas |
|---------|--------|-------|
| **Crash detection** | ✅ | Detecta consumer crashes via CrashNotification channel |
| **Exponential backoff** | ✅ | initial_delay × backoff_multiplier^attempt, capped at max_delay |
| **Max attempts** | ✅ | Configurable, después marca route como Failed permanente |
| **Status tracking** | ✅ | Failed(String) con mensaje del crash |
| **Delegation** | ✅ | Envuelve DefaultRouteController |

**Missing vs Apache Camel:**
- [ ] Jitter en backoff
- [ ] Métricas de restarts
- [ ] Callbacks on crash/recovery
- [ ] Shutdown explícito del loop de supervisión

### Hot-Reload (Completo) - ACTUALIZADO

| Feature | Status | Notas |
|---------|--------|-------|
| ArcSwap pipeline | ✅ | Swap atómico lock-free sin detener route |
| `compute_reload_actions()` | ✅ | Diff engine: Swap/Restart/Add/Remove (`reload.rs:51`) |
| `execute_reload_actions()` | ✅ | Ejecuta todas las acciones (`reload.rs:97`) |
| `watch_and_reload()` | ✅ | File-watcher con debounce, filtro YAML, graceful shutdown |
| CLI `--watch` / `--no-watch` | ✅ | Conectado end-to-end en `camel-cli/src/main.rs` |
| `CamelConfig.watch` field | ✅ | Configurable en `Camel.toml` |
| Tests (13+) | ✅ | Unit, integración, concurrencia y file-watcher |

**Gaps pendientes (baja prioridad):**
- [ ] Debounce hardcodeado a 300ms — necesita `watch_debounce_ms` en `Camel.toml` (`reload_watcher.rs:83`)
- [ ] No drena exchanges in-flight antes del swap (swap es "ciego" al conteo en vuelo)
- [ ] No hay acción `Skip` — siempre hace swap aunque el contenido no haya cambiado (necesita hash de steps)

### YAML DSL (Completo) - ACTUALIZADO

| Feature | Status | Notas |
|---------|--------|-------|
| YAML parsing (serde_yaml) | ✅ | YamlRoute → RouteDefinition |
| `to` step | ✅ | Endpoint producer |
| `set_header` step | ✅ | Static header assignment |
| `set_body` step | ✅ | Literal or language expression |
| `log` step | ✅ | Log message |
| `filter` step | ✅ | simple:/rhai: predicates |
| `choice` step | ✅ | when/otherwise with predicates |
| `split` step | ✅ | body_lines, body_json_array, **language expressions** |
| `aggregate` step | ✅ | completion_size, completion_timeout_ms, completion_predicate |
| `multicast` step | ✅ | Parallel/sequential, aggregation |
| `wire_tap` step | ✅ | Fire-and-forget tap |
| `stop` step | ✅ | Pipeline termination |
| `script` step | ✅ | Language expression to body |
| Route-level config | ✅ | error_handler, circuit_breaker, auto_startup, startup_order |
| Glob route discovery | ✅ | camel-dsl::discovery |
| Language shortcuts | ✅ | `simple:` y `rhai:` en predicates y expressions |

**Ejemplo completo:**
```yaml
routes:
  - id: "demo"
    from: "timer:tick?period=1000"
    auto_startup: true
    startup_order: 100
    error_handler:
      dead_letter_channel: "log:errors"
      retry:
        max_attempts: 3
    circuit_breaker:
      failure_threshold: 5
      open_duration_ms: 30000
    steps:
      - set_header:
          key: "type"
          value: "order"
      - filter:
          simple: "${header.type} == 'order'"
          steps:
            - log: "Processing order"
      - split:
          expression:
            simple: "${header.items}"
          aggregation: "collect_all"
          steps:
            - log: "Item: ${body}"
      - choice:
          when:
            - simple: "${header.priority} == 'high'"
              steps:
                - to: "direct:high-priority"
          otherwise:
            - to: "direct:normal"
```

### Languages (30%)

Apache Camel tiene 10+ expression languages. rust-camel implementa:

| Language | Status | Features |
|----------|--------|----------|
| **Simple** | ✅ | `${body}`, `${body.field}`, `${body.a.0.b}` (JSON path dot-notation + array indexing on `Body::Json`), `${header.name}`, operators (`==`, `!=`, `contains`, `starts_with`, `ends_with`, `regex`, `>`, `<`, `>=`, `<=`, `&&`, `\|\|`), mixed interpolation (`"texto ${expr} más texto"`) |
| **Rhai** | ✅ | Full scripting via Rhai engine, `header("name")`, `set_header("name", value)`, body access, return values as expressions or booleans as predicates |

**Missing:**
- [x] ~~`${body.fieldName}` JSON path access en Simple~~ — DONE: `Expr::BodyField` + dot-notation + array indexing
- [ ] Rhai: propagar `set_header`/`set_property` de vuelta al Exchange real
- [ ] Constant language
- [ ] XPath
- [ ] JSONPath
- [ ] Groovy / JavaScript (N/A in Rust, pero podría ser via rhai)
- [ ] OGNL / SpEL (Java-specific, N/A)
- [ ] Tokenize language

### Testing (50%)

| Feature | Status |
|---------|--------|
| Mock Component | ✅ |
| Exchange recording | ✅ |
| Count assertions | ✅ |
| Re-export en camel-test | ✅ |

**Missing:**
- [ ] MockEndpoint expectations (message body, headers)
- [ ] NotifyBuilder / await conditions
- [ ] Testcontainers integration
- [ ] Route adviceWith (runtime route modification)

---

## ❌ Gaps Críticos

### EIPs Faltantes (~48+)

| Categoría | Missing Patterns |
|----------|------------------|
| **Routing** | Routing Slip, Recipient List, Dynamic Router, Load Balancer, Throttler, Delayer |
| **Messaging** | Message Channel, Message Bus, Message Endpoint (partial) |
| **Messaging Systems** | Message, Command Message, Document Message, Event Message |
| **Channel Adapters** | Channel Adapter, Messaging Gateway, Service Activator |
| **System Management** | Detour, Store |

**DSL Missing:**
- [x] ~~Más steps en YAML DSL (filter, split, choice, aggregate)~~ ✅ Completo
- [ ] onException() shorthand en builder
- [ ] delay(), throttle()
- [ ] recipientList()
- [ ] routingSlip()
- [ ] loop()/repeat()

### Components Faltantes (Críticos)

| Prioridad | Component | Use Case |
|----------|-----------|----------|
| Alta | kafka | Event streaming |
| Alta | jms | Enterprise messaging |
| Alta | sql/database | Persistence |
| Media | amqp | RabbitMQ, Azure Service Bus |
| Media | grpc | Microservices |
| Media | websocket | Real-time |
| Baja | sjms | Simple JMS |
| Baja | netty | Raw TCP/UDP |
| Baja | mail | Email |

---

## Arquitectura

### Stack

```
┌─────────────────────────────────────────────────────────┐
│                    camel-builder (DSL)                  │
├─────────────────────────────────────────────────────────┤
│              camel-config (Configuration)               │
│  CamelConfig, profiles, env overrides, YAML discovery   │
├─────────────────────────────────────────────────────────┤
│                     camel-core                          │
│  CamelContext, Route, Registry, RouteController,        │
│  SupervisingRouteController, Tracer EIP, Hot-reload     │
├─────────────────────────────────────────────────────────┤
│                   camel-processor                       │
│  Filter, Splitter, Aggregator, Multicast, WireTap,      │
│  Choice, Stop, Log, SetHeader, ErrorHandler,            │
│  CircuitBreaker                                         │
├─────────────────────────────────────────────────────────┤
│                    languages/                           │
│  camel-language-api (traits), simple, rhai              │
├─────────────────────────────────────────────────────────┤
│                     camel-api                           │
│  Exchange, Message, Processor trait, Body, Value,       │
│  Metrics, Correlation IDs                               │
├─────────────────────────────────────────────────────────┤
│                  camel-component                        │
│  Component trait, Endpoint trait, Consumer trait        │
├─────────────────────────────────────────────────────────┤
│                    components/                          │
│  timer, log, direct, mock, file, http, controlbus,      │
│  redis                                                  │
└─────────────────────────────────────────────────────────┘
```

### Fortalezas Arquitectónicas

1. **Tower-based composition**: Usa patrones probados del ecosistema tower-service
2. **Type-safe DSL**: Compile-time enforcement via typestate pattern
3. **Async-first**: Native async/await, no blocking
4. **Clean separation**: API traits separados de implementación
5. **Test coverage**: Tests comprehensivos para todos los EIPs
6. **Documentation**: Buena documentación inline y ejemplos
7. **Security-first**: SSRF protection, path traversal prevention, timeouts by default
8. **Production-ready patterns**: Graceful shutdown, correlation IDs, metrics hooks
9. **Resilience**: SupervisingRouteController con exponential backoff
10. **Observability**: Tracer EIP + structured logging + correlation IDs
11. **Configuration**: CamelConfig con profiles y env var overrides
12. **Streaming**: Native lazy evaluation, memory limits, zero-copy I/O, Apache Camel patterns

---

## Streaming Body Implementation (Gap 3 - RESUELTO)

### Overview

rust-camel implementa streaming nativo para cuerpos de mensaje, siguiendo el patrón de Apache Camel con lazy evaluation por defecto.

| Feature | Status | Notas |
|---------|--------|-------|
| **Body::Stream variant** | ✅ | `StreamBody` con `BoxStream<'static, Result<Bytes>>` |
| **Lazy evaluation** | ✅ | Streams NO se materializan automáticamente |
| **Clone semantics** | ✅ | `Arc<Mutex<Option<BoxStream>>>` permite clonar sin consumir |
| **Materialization limits** | ✅ | 10MB default (`DEFAULT_MATERIALIZE_LIMIT`), configurable |
| **Zero-copy I/O** | ✅ | File → Stream → HTTP sin cargar en memoria |
| **JSON placeholders** | ✅ | `{"placeholder": true}` cuando stream se consume en EIPs |
| **Apache Camel alignment** | ✅ | No auto-materialization, opt-in explícito, Stream Caching opcional |

### Components con Streaming

| Component | Streaming Support |
|-----------|------------------|
| **camel-file (Consumer)** | ✅ `ReaderStream` para archivos de cualquier tamaño |
| **camel-http (Producer)** | ✅ Piping directo a `reqwest::Body::wrap_stream()` |
| **camel-file (Producer)** | ⚠️ Materializa (futuro: `tokio::io::copy` directo) |
| **camel-http (Consumer)** | ⚠️ Materializa (futuro: Axum native streaming) |

### Memory Safety

```rust
// Default limit: 10MB
pub const DEFAULT_MATERIALIZE_LIMIT: usize = 10 * 1024 * 1024;

// Explicit materialization with limit
let bytes = body.into_bytes(100 * 1024 * 1024).await?; // 100MB max

// Helper method with default
let bytes = body.materialize().await?; // Uses DEFAULT_MATERIALIZE_LIMIT
```

### EIP Behavior con Streams

Los EIPs que consumen streams (Aggregator, Multicast, Splitter) reemplazan el body con un placeholder JSON válido:

```json
{"placeholder": true}
```

Esto asegura que parsers downstream reciban JSON válido en lugar de strings inválidos.

### Integration Test

- **150MB file test**: Demuestra consumo de memoria constante
- **Verified**: `camel-file` lee archivos grandes sin OOM

### Apache Camel Patterns Adoptados

1. **Lazy evaluation por defecto**: Streams no se materializan automáticamente
2. **Opt-in explícito**: Users deben llamar `body.materialize()` conscientemente
3. **Stream Caching opcional**: No es behavior por defecto
4. **Documentación clara**: Memory limits y trade-offs explícitos
5. **Placeholders semánticos**: JSON válido cuando stream se consume

### Code Review Fixes (2026-03-07)

Post-implementación se resolvió feedback crítico/important:

| Issue | Severity | Resolution |
|-------|----------|------------|
| JSON placeholders inválidos | Critical | Cambiado de `"stream_consumed"` a `{"placeholder": true}` |
| Falta documentación de limits | Important | Añadida doc exhaustiva en FileConfig y HttpConfig |
| Magic numbers | Important | Introducida constante `DEFAULT_MATERIALIZE_LIMIT` |
| API surface incompleta | Important | Re-exportado `StreamMetadata` |
| Doctests rotas | Important | Corregidos imports en ejemplos |

Ver `docs/plans/2026-03-07-streaming-body-implementation.md` para postmortem completo.

---

## 📊 Roadmap Recomendado (Actualizado)

### Fase 0: Estabilización (COMPLETADO ✅)

| Feature | Impacto | Estado |
|---------|---------|--------|
| **SupervisingRouteController** | Crítico | ✅ Implementado |
| **Tracer EIP** | Medio | ✅ Implementado |
| **CamelConfig system** | Alto | ✅ Implementado |

**Tests paralelos siguen siendo un gap de DX.**

### Fase 1: Hacerlo Útil (1-3 meses) - EN CURSO

| Feature | Impacto | Esfuerzo | Estado |
|---------|---------|----------|--------|
| Kafka component | Crítico | Alto | ❌ Pendiente |
| Type Converters | Alto | Medio | ✅ Completo |
| YAML DSL (completo) | Alto | Medio | ✅ Completo |
| Hot-reload end-to-end | Medio | Bajo | ✅ Completo |
| Tests paralelos (serial_test) | Alto | Bajo | ✅ Completo |

**Resultado esperado:** 18% → 28% cobertura, usable en proyectos reales

### Fase 2: Producción (3-6 meses)

| Feature | Impacto | Esfuerzo |
|---------|---------|----------|
| OpenTelemetry integration | Crítico | Medio |
| Prometheus metrics | Alto | Bajo |
| SQL component | Alto | Alto |
| RedeliveryPolicy | Medio | Bajo |
| ~~YAML DSL completo (filter/split/choice)~~ | ~~Alto~~ | ~~Medio~~ | ✅ Completo |

**Resultado esperado:** 28% → 38% cobertura, production-ready

### Fase 3: Ecosistema (6+ meses)

| Feature | Impacto | Esfuerzo |
|---------|---------|----------|
| AMQP | Alto | Medio |
| gRPC | Alto | Medio |
| WebSocket | Medio | Medio |
| Kafka | Crítico | Alto |

**Resultado esperado:** 38% → 55% cobertura, ecosistema rico

---

## Comparación Detallada

| Aspecto | rust-camel | Apache Camel | Gap |
|--------|------------|--------------|-----|
| Core Architecture | 85% | 100% | 15% |
| EIPs | 18% (12/60+) | 100% | 82% |
| Components | 3% (8/300+) | 100% | 97% |
| Languages | 30% (2/10+) | 100% | 70% |
| DSL | 85% | 100% | 15% |
| Error Handling | 80% | 100% | 20% |
| Testing | 50% | 100% | 50% |
| Security | 60% | 100% | 40% |
| Observability | 65% | 100% | 35% |
| Configuration | 70% | 100% | 30% |
| DI/Beans | 70% | 100% | 30% |
| **Production Ready** | **74%** | 100% | 26% |

---

## Conclusión

**rust-camel tiene los cimientos correctos + security/observability/resilience base** y ha madurado significativamente.

### Para ser alternativa seria:

1. **Más EIPs** - 12 es mejor, pero necesitas 25-30 mínimos
2. **Componentes clave** - Kafka, SQL, JMS son obligatorios
3. **YAML DSL completo** - ✅ Completo: filter/split/choice/aggregate/multicast/wire_tap/stop/script soportados
4. **Observabilidad completa** - OpenTelemetry + Prometheus
5. **Tests paralelos** - ✅ Resuelto con serial_test

### Timeline estimado (actualizado 2026-03-07)

- **Inmediato** → OpenTelemetry (config struct existe, integración pendiente)
- **3 meses** → 28% cobertura, usable en proyectos específicos
- **6 meses** → 38% cobertura, production-ready para casos de uso principales
- **12+ meses** → 55% cobertura, alternativa seria para muchos casos

### Veredicto Final

rust-camel es un proyecto bien diseñado que demuestra la viabilidad de un framework de integración nativo en Rust. La arquitectura es sólida, el código es de calidad, y ahora incluye **security features críticas**, **observability completa con Tracer EIP**, **resilience con SupervisingRouteController**, y **un sistema de configuración maduro**.

**Fortalezas:**
- Arquitectura limpia (Tower-based)
- Type-safe DSL
- Async-first
- Código de calidad con tests
- **Security-first (SSRF, path traversal, timeouts)**
- **Correlation IDs para distributed tracing**
- **Route lifecycle management completo**
- **Graceful shutdown y error recovery**
- **SupervisingRouteController con exponential backoff**
- **Tracer EIP con 3 niveles de detalle**
- **CamelConfig con profiles y env var overrides**
- **Bean Registry con proc-macros ergonómicos**

**Debilidades:**
- Muy pocos componentes (8 vs 300+)
- Muy pocos EIPs (12 vs 60+)
- Hot-reload: completo (gaps menores: debounce configurable, in-flight drain, Skip action)
- OpenTelemetry y Prometheus no implementados
- Streaming: File Producer y HTTP Consumer aún materializan (futuro: direct streaming)

**Recomendación:**
1. **Inmediato:** OpenTelemetry (config struct existe, integración pendiente)
2. **Corto plazo:** Componentes críticos (Kafka, SQL)
3. **Mediano plazo:** Expandir EIPs (Routing Slip, Recipient List, etc.)
4. **Largo plazo:** Ecosistema de componentes

---

## Diferencias por versión

### Cambios desde 2026-03-08 (Bean Registry - PR #2)

**Nueva feature: Bean Registry / Dependency Injection (Gap 2 RESUELTO)**

| Feature | Estado |
|---------|--------|
| `BeanRegistry` con HashMap O(1) lookup | ✅ `camel-bean` crate |
| `BeanProcessor` trait | ✅ `async fn call(&self, method, &mut Exchange)` + `fn methods() -> Vec<&str>` |
| `#[bean_impl]` proc-macro | ✅ Genera BeanProcessor impl desde impl block |
| `#[handler]` proc-macro | ✅ Marca métodos como invocables |
| Parameter binding | ✅ `body: T`, `headers: Headers`, `exchange: &mut Exchange` (by name) |
| Return types | ✅ `Result<T, E>` donde `E: Display`, auto JSON serialization |
| Error handling | ✅ `BeanError` enum (NotFound, MethodNotFound, BindingFailed) |
| YAML DSL integration | ✅ `bean: { name: "...", method: "..." }` step |
| Integration tests | ✅ 8 tests (registration, invocation, validation, multi-step, errors) |
| Macro unit tests | ✅ 10 tests (attribute detection, parameter parsing, validation) |
| Documentation | ✅ 309-line README + 150-line example README |
| bean-demo example | ✅ OrderService con 3 handlers (process, validate, get_stats) |

**Production Readiness mejoró de 72% → 74%**

**Arquitectura:**
- `camel-bean`: Core crate con BeanRegistry + BeanProcessor trait
- `camel-bean-macros`: Proc-macros para ergonomía
- `camel-core`: Integración con RouteController via `with_beans()`
- `camel-dsl`: YAML step `DeclarativeStep::Bean`
- Thread-safe: `Arc<dyn BeanProcessor>` permite sharing entre routes

**Design decisions:**
- Parameter inference by name (no annotations needed)
- `Result<T, E: Display>` flexible (any error can be string)
- JSON serialization aligned with `Body::Json`
- Placeholder `#[derive(Bean)]` for forward-compatibility

**Deferred (trabajo futuro):**
- Constructor dependency injection
- Lifecycle callbacks (@PostConstruct, @PreDestroy)
- Bean scopes (singleton, prototype, request)
- Conditional bean registration
- Service discovery integration

### Cambios desde 2026-03-07 (Type Converters - PR #1)

**Nueva feature: Type Converters (Gap 1 RESUELTO)**

| Feature | Estado |
|---------|--------|
| `BodyType` enum + `body_converter` module | ✅ Motor centralizado en `camel-api` |
| `ConvertBodyTo<P>` Tower processor | ✅ `camel-processor` |
| `.convert_body_to(BodyType)` DSL step | ✅ `StepAccumulator` → RouteBuilder/SplitBuilder/todos los builders |
| YAML DSL `convert_body_to: text\|json\|bytes\|empty` | ✅ `camel-dsl` |
| `try_into_text/json/bytes_body()` en `Body` | ✅ Convenience methods para componentes |
| `camel-redis` limpiado con `try_into_text()` | ✅ Sin `as_text().ok_or_else(...)` |
| `Body::PartialEq` | ✅ `Stream` variants siempre `false` |
| 560+ tests pasando | ✅ 0 fallos |

**Production Readiness mejoró de 70% → 72%**

**Deferred (trabajo futuro del postmortem):**
- `camel-http`: reemplazar 3 `match` inline `Body → bytes` con `try_into_bytes_body()` (baja prioridad)
- `camel-log`: reemplazar `match` de formateo con `try_into_text()` (baja prioridad)
- Tracer: capturar campo `target` específico de `ConvertBodyTo` en payload JSON

### Cambios desde 2026-03-07 (revisión doc)

**Nueva feature: Streaming Body (Gap 3 RESUELTO)**

| Feature | Estado |
|---------|--------|
| Body::Stream variant | ✅ Lazy evaluation, Arc<Mutex<Option<BoxStream>>> |
| Zero-copy I/O | ✅ File → Stream → HTTP sin materialización |
| Memory limits | ✅ 10MB default, configurable, documentado |
| JSON placeholders | ✅ `{"placeholder": true}` en EIPs |
| Integration test | ✅ 150MB file con memoria constante |
| Code review fixes | ✅ Critical/Important issues resueltos |

**Production Readiness mejoró de 65% → 70%**

**Code Review Fixes:**
- JSON placeholders: `"stream_consumed"` → `{"placeholder": true}` (breaking change necesario)
- Memory limits: Documentación exhaustiva en FileConfig/HttpConfig
- Magic numbers: Constante `DEFAULT_MATERIALIZE_LIMIT`
- API surface: Re-export `StreamMetadata`
- Doctests: Corregidos y verificados
- Integration test: 150MB file con memoria constante

**Correcciones de estado:**

| Feature | Estado en doc | Estado real |
|---------|--------------|-------------|
| Hot-reload end-to-end | ⚠️ Parcial (ejecución no conectada a CLI) | ✅ Completo: CLI `--watch`/`--no-watch`, watcher, diff + execute conectados |

**Gaps documentados:**
- Hot-reload: debounce hardcodeado 300ms (`reload_watcher.rs:83`) — anotado en TODO.md

### Cambios desde 2026-03-07

**Mejoras añadidas:**

| Feature | Estado anterior | Estado actual |
|---------|----------------|---------------|
| Log EIP (YAML DSL) | ⚠️ mensaje como string literal | ✅ evaluado como Simple Language (`${body}`, `${header.name}`, interpolación mixta) |
| Simple Language | ✅ expresiones puras `${...}` | ✅ v2: interpolación mixta `"texto ${expr} más texto"` |
| Simple Language v2: `${body.fieldName}` | ❌ ParseError | ✅ dot-notation + array indexing on `Body::Json` |
| YAML DSL | ⚠️ to/set_header/log steps | ✅ Completo: todos los steps soportados (filter/split/choice/aggregate/multicast/wire_tap/stop/script + language expressions) |
| Tests paralelos | ❌ cargo test bloqueaba | ✅ Resuelto con serial_test crate |
| Redis integration tests | ❌ 7 failing | ✅ 7/7 passing |

**Production Readiness mejoró de 60% → 65%**

### Cambios desde 2026-03-02

**Mejoras añadidas:**

| Feature | Estado anterior | Estado actual |
|---------|----------------|---------------|
| SupervisingRouteController | ❌ No implementado | ✅ Exponential backoff + crash detection |
| Tracer EIP | ❌ No implementado | ✅ TracingProcessor, 3 detail levels |
| CamelConfig system | ❌ No existía | ✅ Profiles, TOML, env vars, configure_context() |
| Hot-reload (infraestructura) | ❌ No existía | ✅ ArcSwap + ReloadCoordinator diff + execute + CLI `--watch` (ver revisión 2026-03-07) |
| YAML DSL | ❌ camel-dsl vacío | ✅ Completo (ver cambios 2026-03-07) |
| Redis component | ❌ En backlog | ✅ Consumer + Producer completo (5 data types + pubsub) |

**Production Readiness mejoró de 50% → 60%**

**Nuevos gaps identificados:**
- Tracer: OpenTelemetry output no implementado (config struct existe)

### Cambios desde 2026-03-01

| Feature | Estado anterior | Estado actual |
|---------|----------------|---------------|
| HTTP Consumer | ❌ No implementado | ✅ Axum-based server |
| RouteController | ❌ No existía | ✅ Lifecycle management completo |
| ControlBus | ❌ No existía | ✅ Dynamic route control |
| Correlation IDs | ❌ No existía | ✅ UUID per exchange |
| SSRF Protection | ❌ No existía | ✅ allowPrivateIps, blockedHosts |
| Path Traversal | ❌ No existía | ✅ validate_path_is_within_base() |
| Timeouts (File) | ❌ No existía | ✅ readTimeout/writeTimeout |
| Mutex Recovery | ❌ No existía | ✅ Graceful recovery |
| Metrics Hooks | ❌ No existía | ✅ MetricsCollector trait |
| Pipeline Concurrency | ❌ No existía | ✅ .concurrent() / .sequential() |

**Production Readiness mejoró de 30% → 50%**
