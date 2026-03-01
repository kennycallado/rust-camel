---

Resumen de lo pendiente/futuro que emergen de los planes:

| Ítem                                                                 | Origen                                 | Prioridad implícita                                                            | Estado |
| -------------------------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------ | ------ |
| Graceful shutdown / drain                                            | Tower reconnection Non-Goals           | Alta — importante para producción                                              | DONE   |
| Backpressure (poll_ready always-ready)                               | Tower reconnection Non-Goals           | Alta — necesario para sistemas de carga real                                   | DONE   |
| Circuit breaker                                                      | Error handling Non-Goals               | Media                                                                          | DONE   |
| Splitter EIP                                                         | Core design + Tower reconnection       | Media — primer EIP con sub-pipeline                                            | DONE   |
| File component (consumer + producer)                                 | Core design Future Extensions          | Media — primer componente I/O real                                             | DONE   |
| HTTP component (producer)                                            | Core design Future Extensions          | Media — integración con APIs externas                                          | DONE   |
| WireTap EIP                                                          | Splitter design Future EIPs            | Media-baja — Exchange clone + fire-and-forget, casi trivial                    | DONE   |
| Aggregator EIP                                                       | Splitter design Future EIPs            | Media —                                                                        | DONE   |
| Filter real stopping behavior                                        | Aggregator postmortem                  | Media — Filter devuelve exchange as-is cuando predicate=false;                 |        |
|                                                                      |                                        | pipeline no se detiene;                                                        |        |
|                                                                      |                                        | afecta Multicast, cualquier EIP que quiera filtrar downstream                  |        |
| Aggregator v2: completion by timeout                                 | Aggregator design Deferred             | Media-baja — requiere `tokio::spawn` interno por bucket + `CancellationToken`  |        |
| Aggregator v2: correlation key por función                           | Aggregator design Deferred             | Baja — cambiar `header_name: String` a `CorrelationKeyFn`;                     |        |
|                                                                      |                                        | exponer `correlate_by_header` y `correlate_by_fn` en el builder                |        |
| Aggregator v2: estado compartido cross-route                         | Aggregator design Deferred             | Baja — requiere registry de agregadores nombrados en `CamelContext`            |        |
| Aggregator v2: estado persistente                                    | Aggregator design Deferred             | Baja — buckets son in-memory, se pierden en restart                            |        |
| Aggregator v2: `forceCompletionOnStop`/`discardOnAggregationFailure` | Aggregator design Deferred             | Baja — opciones Java Camel diferidas                                           |        |
| WireTap v2: prepare/processor function                               | WireTap postmortem                     | Baja — transformar copia antes de enviar; Apache Camel tiene esto              |        |
| WireTap v2: synchronous tap with timeout                             | WireTap design Non-Goals               | Baja — viola fire-and-forget, añade complejidad                                |        |
| Multicast EIP                                                        | Splitter design Future EIPs            | Media — mismo patrón que Splitter (clone N, aggregate)                         |        |
| Routing Slip EIP                                                     | Core design                            | Media-baja                                                                     |        |
| HTTP consumer (server/webhook endpoint)                              | File-HTTP plan implícito               | Media-baja — HttpConsumer no implementado, solo producer                       |        |
| Idempotent file consumer (seen-file tracking)                        | File-HTTP postmortem \#6               | Media-baja — noop=true re-envía duplicados cada poll, necesita idempotent repo |        |
| Pipeline concurrency (Option E typestate)                            | Pipeline concurrency analysis          | Media-baja — struct-based consumers con concurrency compile-time               |        |
| HttpOperationFailed: añadir URL y método                             | File-HTTP postmortem \#7               | Baja — mejora de debugging en logs                                             |        |
| Recipient List EIP                                                   | Splitter design Future EIPs            | Baja — Multicast dinámico                                                      |        |
| StepAccumulator trait (DRY SplitBuilder/RouteBuilder)                | Splitter postmortem Task 4             | Baja — refactoring puro, sin impacto funcional; ahora afecta 4 builders:       | DONE   |
|                                                                      |                                        | RouteBuilder, SplitBuilder,                                                    |        |
|                                                                      |                                        | y cualquier builder futuro que añada `.aggregate()`                            |        |
| Parallel stop_on_exception con future cancellation                   | Splitter design Non-Goals + postmortem | Baja — fallback funcional existe (join_all, retorna primer error)              |        |
| Nested splits                                                        | Splitter design Non-Goals              | Baja — YAGNI                                                                   |        |
| Streaming split (async Stream en vez de Vec)                         | Splitter design Non-Goals              | Baja — para splits muy grandes que no caben en memoria                         |        |
| Adaptive circuit breaker                                             | Resilience design Non-Goals            | Baja — sliding windows, latency thresholds                                     |        |
| Per-exchange retry state persistence                                 | Resilience design Non-Goals            | Baja — estado vive solo en memoria                                             |        |
| Concurrency limiter / tower::Buffer                                  | Resilience design Non-Goals            | Baja — YAGNI con single pipeline task/route                                    |        |
| Full pull-based pipeline (reemplazar ch 256)                         | Resilience design Non-Goals            | Baja — backpressure implícito es suficiente                                    |        |
| XML/YAML DSL para circuit breaker config                             | Resilience design Non-Goals            | Baja — diferido a camel-dsl                                                    |        |
| Type converters                                                      | Core design Future Extensions          | Baja                                                                           |        |
| camel-dsl (YAML/JSON routes)                                         | Core design + Error handling Non-Goals | Baja — crate ya existe vacío                                                   |        |
| Más components (kafka, etc.)                                         | Core design Future Extensions          | Baja — depende de casos de uso                                                 |        |
| Management API (health, metrics)                                     | Core design Future Extensions          | Baja — camel-health ya existe vacío                                            |        |
