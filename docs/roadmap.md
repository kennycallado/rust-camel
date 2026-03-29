# Roadmap

> Last updated: 2026-03-29

---

## Next Up

> Some items in "Later" are blocked on open decisions — see [decisions.md](decisions.md).

Work that is ready to execute (no pending decisions blocking it).

### Grupo A — Component debt

| Item | Priority | Notes |
|---|---|---|
| Kafka: configurable consumer group rebalance strategy | Medium | Missing from current implementation |
| HTTP: configurable connection pool | Medium | reqwest client reuses defaults |
| OpenTelemetry: configurable exporter (OTLP endpoint) | Medium | Currently hardcoded/env-based |
| `camel-config` contract: env variable injection | Medium | `${env:VAR}` syntax in YAML DSL values |
| `camel-config` contract: per-component config in TOML | Low | Currently global only |
| SQL: configurable `separator` for IN clause | Low | Currently hardcoded to comma |
| SQL: SSL/TLS via explicit parameters (not only in db_url) | Low | |
| File: idempotent consumer (seen-file tracking) | Medium | `noop=true` currently re-sends duplicates on restart |

### Grupo B — EIPs and DSL

| Item | Priority | Notes |
|---|---|---|
| Aggregator v2: completion by timeout | Medium | `tokio::spawn` per bucket + `CancellationToken` |
| Aggregator v2: correlation key by function | Low | Change `header_name: String` to accept a closure |
| Aggregator v2: `forceCompletionOnStop` / `discardOnAggregationFailure` | Low | Parity with Apache Camel options |
| Transform EIP (`.transform()` DSL step) | Medium | `map_body`/`set_body_fn` exist functionally but the named step is missing from builder and YAML DSL |
| Delayer EIP | Low | Configurable delay between pipeline steps |
| `delay()` DSL step | Low | Expose Delayer as a builder step |
| `onException()` DSL shorthand | Medium | Currently only via `error_handler(config)`; `.on_exception(pred).to("log:dlc")` improves DX; also enables exception clauses by type in YAML DSL |
| `recipientList()` DSL step | Low | Dynamic recipient list from header/body |
| `routingSlip()` DSL step | Low | Expose Routing Slip EIP as a builder step |
| `loop()` / `repeat()` DSL steps | Low | Repeat N times or while predicate is true |
| WireTap v2: `prepare`/processor function | Low | Transform copy before sending |
| Multicast v2: `onPrepare` processor | Low | Transform copy per endpoint before send |
| Multicast v2: streaming mode (out-of-order) | Low | Aggregate results in arrival order |
| Splitter: streaming split (async Stream instead of Vec) | Low | For large splits that don't fit in memory |
| XML DSL | Medium | Route definitions in `.xml` files |
| JS DSL (`camel-dsl-js`) | Medium | Route definitions in `.js` files — separate crate from `camel-language-js` |

### Grupo C — Data formats

| Item | Priority | Notes |
|---|---|---|
| `marshal` / `unmarshal` EIP step | Medium | Serialize/deserialize between formats (JSON, XML, CSV, Protobuf) as a route step |
| JSONPath language (`camel-language-jsonpath`) | Medium | `$.items[*].id` style path expressions for set_header/set_body in YAML DSL |
| XPath language (`camel-language-xpath`) | Medium | Extract fields from `Body::Xml`; viable now that `Body::Xml` exists |
| JSON↔XML transformer | Low | Convert between `Body::Json` and `Body::Xml` with configurable convention (BadgerFish, Parker) |
| XSLT processor | Low | Apply XSL stylesheet to `Body::Xml`; requires libxslt bindings or pure-Rust |
| CSV data format | Low | |
| Protobuf data format | Low | |

### Grupo D — Testing and quality

| Item | Priority | Notes |
|---|---|---|
| ~~Integration test coverage baseline~~ | ~~High~~ | **Done** — 77.4% lines, `scripts/coverage.sh`, CI enforcement via `coverage.toml`, `cargo-llvm-cov` in flake.nix |
| `camel-test`: test route isolation | Medium | Run routes in isolated context per test without shared state |
| `camel-test`: time control for timer-based tests | Medium | Inject clock to avoid real waits |
| `camel-test`: `adviceWith` (runtime route modification for tests) | Low | Intercept and replace steps at test time |
| Testcontainers integration helpers | Low | Postgres/Redis/Kafka helpers for integration tests (wrapper over `testcontainers-rs` or `camel-container`) |
| Performance benchmarks | Low | Baseline throughput and latency per component |

### Gaps (confirmed, from architecture review)

| Item | Priority | Notes |
|---|---|---|
| Hot-reload: drain in-flight exchanges before swap | Medium | Prevents mid-flight disruption on reload |
| Hot-reload: configurable debounce (`watch_debounce_ms`) | Low | Currently hardcoded to 300ms |
| Hot-reload: `Skip` action (no-op when content unchanged) | Low | Currently always swaps; needs step hash comparison |
| Health check endpoints | Medium | `/health` and `/ready` via HTTP |
| Lifecycle callbacks (true Suspend/Resume) | Low | Currently aliases for stop/start |
| Security: rate limiting | Low | |
| Security: authentication/authorization hooks | Low | |
| Security: secrets management integration | Low | Vault, env-based secrets |
| Security: audit logging | Low | |

---

## Later (Phase 3+)

Valid work, no urgency. Revisit once "Next Up" is done.

| Item | Notes |
|---|---|
| JS Phase 2 (npm, TypeScript imports) | Blocked on engine + npm scope decisions above |
| `camel.send()` / ProducerTemplate in scripts | Requires async engine (rquickjs) |
| `camel.context.routeIds()` / `camel.context.addRoute()` | Same dependency |
| Lua scripting language | Clean embedding story, pure-Lua packages work well, no async assumption |
| Constant language | Constant value expressions in predicates/YAML DSL |
| Tokenize language | Split by token; useful in Splitter YAML DSL without scripting |
| `camel-language-bean`: invoke methods from expressions | `bean("MyService", "method")` in predicates |
| Unit of Work | Needed for Saga/transaction patterns; `on_complete`/`on_failure` hooks at exchange level |
| Type Converters (centralized) | Reduces component boilerplate; not blocking anything currently |
| Aggregator v2: shared state cross-route | Requires aggregator registry |
| Aggregator v2: persistent state | Buckets are in-memory; lost on restart |
| Multicast v2: `shareUnitOfWork` | Coordinate transactions across endpoints |
| JMS component | Enterprise messaging |
| AMQP component | RabbitMQ, Azure Service Bus |
| gRPC component | Microservices |
| WebSocket component | Real-time |
| Beans: constructor dependency injection | `#[bean_impl]` currently requires manual wiring |
| Beans: lifecycle callbacks (`@PostConstruct` / `@PreDestroy`) | |
| Beans: scopes (singleton, prototype, request) | |
| Beans: `@Consume` annotation (auto-subscribe to endpoint) | |
| SupervisingRouteController: jitter in backoff | Avoid thundering herd on mass restart |
| SupervisingRouteController: restart metrics | Count restarts per route |
| SupervisingRouteController: callbacks on crash/recovery | Hook for alerting or custom logic |
| `camel-config`: vault/secrets integration | |
| Clustering / HA | Long-term |
