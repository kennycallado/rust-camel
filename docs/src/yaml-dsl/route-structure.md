# Route structure

A YAML route file defines one or more routes. Each route has an identifier, a
source endpoint, and an ordered list of processing steps.

## Minimal route

The smallest useful route reads from a source and writes to a destination.

```yaml
{{#include ../../../examples/config-basic/routes/hello.yaml:first-route}}
```

The file opens with a `routes` list. Each list entry is one route object. The
route above, `hello-timer`, reads from a timer endpoint, logs a greeting, and
forwards the exchange to the `log:info` endpoint.

## Route fields

Each route object accepts these top-level fields. Only `id` and `from` are
required. The `error_handler` and `circuit_breaker` objects are documented
below. The `security_policy` object is documented in
[Authorization](../services/auth.md).

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | string | yes | — | Unique route identifier |
| `from` | string | yes | — | Source endpoint URI |
| `steps` | list | no | `[]` | Ordered step verbs |
| `auto_startup` | bool | no | `true` | Start the route when the context starts |
| `startup_order` | integer | no | `1000` | Ascending start order; shutdown reverses it |
| `sequential` | bool | no | `false` | Process exchanges one at a time |
| `concurrent` | integer | no | — | Maximum concurrent exchanges |
| `error_handler` | object | no | — | Per-route error handler |
| `circuit_breaker` | object | no | — | Route-level circuit breaker with optional fallback sub-pipeline |
| `security_policy` | object | no | — | Route-level authorization |
| `on_complete` | string | no | — | Producer URI for the success hook |
| `on_failure` | string | no | — | Producer URI for the failure hook |

`auto_startup: false` registers the route but does not start its consumer. Start
it later through the route controller or control bus. `concurrent` caps how many
exchanges the pipeline processes in parallel; omit it to let the runtime decide.
`on_complete` and `on_failure` fire when an exchange exits the pipeline, on
success or on error respectively.

## Error handling

Set `error_handler` to retry failed exchanges and send them to a dead letter
channel when retries run out. The handler holds a redelivery policy and optional
per-exception clauses. The full field set lives on the
[step verbs reference](step-verbs.md).

## Circuit breaker

Set `circuit_breaker` to protect a route from a failing downstream service. The
breaker opens after `failure_threshold` consecutive failures. While open, it
rejects exchanges for `open_duration_ms`. See [Circuit
breaker](../eip/circuit-breaker.md) for the breaker states.

The optional `fallback` list holds a sub-pipeline. The breaker runs the
sub-pipeline instead of rejecting the exchange while the circuit is open. An
absent or empty `fallback` keeps the existing behavior: the breaker returns
`CircuitOpen`.

```yaml
{{#include ../../../examples/cache-example/routes.yaml:cb-fallback-stale-route}}
```

The fallback runs on routes with and without an `error_handler`. A fallback
step that stops cleanly (for example, a `cache_peek_stale` MISS with the
default `on_miss: stop`) surfaces `Ok(exchange)` with the exchange state
intact. No `CircuitOpen` escapes.

A failing fallback step follows the route's error handling. A route with an
`error_handler` routes the failure through the handler. A route without one
surfaces the raw error to the caller.

The [Cache](../eip/cache.md) page shows the stale-on-error composition with
`cache_peek_stale`.

## List-form variant

The hot-reload subsystem consumes a flatter form. When a file holds only routes
and omits the top-level `routes` wrapper, the parser reads the file as a bare
list of route objects:

```yaml
{{#include ../../../examples/hot-reload/routes/route.yaml:hot-reload-route}}
```

Each entry uses `route_id` instead of `id` and carries `from` and `steps`. This
list form maps to the canonical route contract the runtime exchanges over the
control bus, not the full authoring model.

## Next

- [Step verbs](step-verbs.md): every verb and its fields

**Reference**: [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
