# Step verbs reference

Every YAML step verb and field, derived from the authoritative source
`crates/camel-dsl/src/route_ast.rs`. Each verb maps to a struct that
[Route structure](route-structure.md).

Where a verb takes a predicate or value expression, the standard language fields
apply: `simple`, `rhai`, `jsonpath`, `xpath`, or `language` paired with
`source`. The tables below list them in full.

## Step verbs

### `to`

Send the exchange to an endpoint URI.

| Field | Type | Required | Description |
|---|---|---|---|
| `to` | string | yes | Target endpoint URI |

```yaml
- to: "log:info"
```

### `log`

Log the exchange state.

| Form | Syntax |
|---|---|
| Short | `log: "message"` |
| Full | `log: { message: "...", level: "DEBUG" }` |

The `message` field accepts a bare string or an expression object (`simple`,
`rhai`, `jsonpath`, `xpath`, or `language`+`source`). `level` is optional.

```yaml
- log: "Processing exchange"
- log:
    message: "Body is ${body}"
    level: "DEBUG"
```

### `set_header`

Set a message header.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Header name |
| `value` | any | no | Literal value |
| `simple` | string | no | Simple expression |
| `rhai` | string | no | Rhai expression |
| `jsonpath` | string | no | JSONPath expression |
| `xpath` | string | no | XPath expression |
| `language` | string | no | Named expression language |
| `source` | string | no | Expression source for `language` |

```yaml
- set_header:
    key: "MyHeader"
    value: "hello"
```

### `set_property`

Set an exchange property. Same expression fields as `set_header` but keyed by
`name`.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Property name |
| `value` | any | no | Literal value |
| `simple` | string | no | Simple expression |
| `rhai` | string | no | Rhai expression |
| `jsonpath` | string | no | JSONPath expression |
| `xpath` | string | no | XPath expression |
| `language` | string | no | Named expression language |
| `source` | string | no | Expression source for `language` |

```yaml
- set_property:
    name: "MyProperty"
    value: 42
```

### `set_body`

Set the exchange body.

| Form | Syntax |
|---|---|
| Literal | `set_body: "value"` |
| Config | `set_body: { value: ... }` or `set_body: { simple: "..." }` |

The config form accepts `value` plus any expression field (`simple`, `rhai`,
`jsonpath`, `xpath`, `language`+`source`).

```yaml
- set_body: "static value"
- set_body:
    value: "Hello World!"
- set_body:
    simple: "${header.foo}"
```

### `transform`

Alias for `set_body`. Same forms and fields.

```yaml
- transform:
    simple: "${body.field}"
```

### `filter`

Conditionally run child steps. When the predicate is false, the exchange skips
the `steps` list and continues down the pipeline.

| Field | Type | Required | Description |
|---|---|---|---|
| `simple` | string | no | Simple predicate |
| `rhai` | string | no | Rhai predicate |
| `jsonpath` | string | no | JSONPath predicate |
| `xpath` | string | no | XPath predicate |
| `language` | string | no | Named expression language |
| `source` | string | no | Expression source for `language` |
| `steps` | list | no | Child steps when the predicate holds |

```yaml
- filter:
    simple: "${header.type} == 'important'"
    steps:
      - to: "log:important"
```

### `choice`

Content-based router. Evaluates `when` clauses in order and runs the first that
matches. `otherwise` runs when no clause matches.

| Field | Type | Required | Description |
|---|---|---|---|
| `when` | list | no | Predicate blocks (expression fields + `steps`) |
| `otherwise` | list | no | Fallback steps |

```yaml
- choice:
    when:
      - simple: "${header.type} == 'a'"
        steps:
          - to: "log:a"
      - simple: "${header.type} == 'b'"
        steps:
          - to: "log:b"
    otherwise:
      - to: "log:other"
```

### `do_try`

Protected block with catch and finally clauses.

| Field | Type | Required | Description |
|---|---|---|---|
| `steps` | list | yes | Protected steps |
| `catch` | list | no | Catch clauses |
| `finally` | object | no | Finally clause |

Each `catch` entry accepts `exception` (list of error kinds), `when` and
`on_when` predicates, `disposition` (defaults to `handled`), and `steps`. The
`finally` object carries an optional `on_when` and a required `steps` list.

```yaml
- do_try:
    steps:
      - to: "direct:fragile"
    catch:
      - exception: ["ProcessorError"]
        steps:
          - to: "log:error"
    finally:
      steps:
        - to: "log:cleanup"
```

### `delay`

Pause processing.

| Form | Syntax |
|---|---|
| Short | `delay: 500` (milliseconds) |
| Full | `delay: { delay_ms: 500, dynamic_header: "X-Delay" }` |

```yaml
- delay: 500
- delay:
    delay_ms: 200
    dynamic_header: "X-Delay"
```

### `loop`

Repeat child steps.

| Form | Syntax |
|---|---|
| Count | `loop: 3` |
| Full | `loop: { count: 3, steps: [...] }` |
| While | `loop: { while: { simple: "..." }, steps: [...] }` |

Full-form fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `count` | integer | no | Fixed iteration count (exclusive with `while`) |
| `while` | object | no | Predicate block; loops while it holds |
| `steps` | list | no | Child steps per iteration |
| `max_iterations` | integer | no | Safety cap on iterations |

The `while` block accepts the standard predicate fields (`simple`, `rhai`,
`jsonpath`, `xpath`, `language`+`source`).

```yaml
- loop: 3
- loop:
    count: 5
    steps:
      - to: "log:iteration"
```

### `split`

Split the body into fragments and process each.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `expression` | string/object | no | — | Split expression (string or language block) |
| `aggregation` | string | no | `last_wins` | Aggregation strategy |
| `parallel` | bool | no | `false` | Process fragments in parallel |
| `parallel_limit` | integer | no | — | Max parallel fragments |
| `stop_on_exception` | bool | no | `true` | Stop on first error |
| `streaming` | bool | no | `false` | Stream the split |
| `stream` | object | no | — | Stream config (`format`, `max_record_bytes`, `batch_size`, `chunk_size`) |
| `steps` | list | no | `[]` | Per-fragment steps |

```yaml
- split:
    expression: "body_lines"
    aggregation: "last_wins"
    steps:
      - log: "Split item: ${body}"
      - to: "log:split-item"
- split:
    expression:
      simple: "${header.items}"
    aggregation: "collect_all"
    steps:
      - to: "log:fragment"
```

### `aggregate`

Group exchanges by correlation key. Emits one combined exchange when a
completion condition fires. The combined exchange then continues down the
pipeline, so `aggregate` has no nested `steps` block.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `header` | string | yes | — | Header used as the correlation key |
| `correlation_key` | string | no | — | Alternative correlation expression |
| `completion_size` | integer | no | — | Complete after N exchanges |
| `completion_timeout_ms` | integer | no | — | Complete after timeout |
| `completion_predicate` | object | no | — | Predicate-block completion trigger |
| `strategy` | string | no | `collect_all` | Aggregation strategy |
| `max_buckets` | integer | no | — | Max concurrent buckets |
| `bucket_ttl_ms` | integer | no | — | Bucket time-to-live |
| `force_completion_on_stop` | bool | no | — | Emit pending buckets on route stop |
| `discard_on_timeout` | bool | no | — | Drop buckets that time out |

```yaml
- aggregate:
    header: "CorrelationId"
    completion_size: 10
```

### `marshal`

Serialize the body to a data format.

| Field | Type | Required | Description |
|---|---|---|---|
| `marshal` | string | yes | Format name (`json`, `protobuf`, ...) |
| `config` | object | no | Format-specific config |

```yaml
- marshal: "json"
```

### `unmarshal`

Parse the body from a data format. An optional `schema` validates the parsed
JSON and rejects mismatches.

| Field | Type | Required | Description |
|---|---|---|---|
| `unmarshal` | string | yes | Format name |
| `schema` | object | no | JSON Schema for validation |
| `config` | object | no | Format-specific config |

```yaml
- unmarshal: "json"
```

### `convert_body_to`

Convert the body type.

| Field | Type | Required | Description |
|---|---|---|---|
| `convert_body_to` | string | yes | Target type (`json`, ...) |

```yaml
- convert_body_to: json
```

### `bean`

Invoke a registered bean method.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Bean name |
| `method` | string | yes | Method name |

```yaml
- bean:
    name: "myBean"
    method: "handle"
```

### `script`

Run a script inline.

| Field | Type | Required | Description |
|---|---|---|---|
| `language` | string | yes | Script language (`rhai`, ...) |
| `source` | string | yes | Script source |

```yaml
- script:
    language: "rhai"
    source: "1 + 1"
```

### `function`

Run a function in an external runtime.

| Field | Type | Required | Description |
|---|---|---|---|
| `runtime` | string | yes | Runtime name (`deno`, ...) |
| `source` | string | yes | Function source |
| `timeout_ms` | integer | no | Execution timeout |

```yaml
- function:
    runtime: "deno"
    source: "export default (ctx) => ctx.body = { processed: true }"
```

### `stop`

Stop route processing. The exchange returns to the consumer as a successful
response.

```yaml
- stop: true
```

### `stream_cache`

Materialize a stream body into bytes.

| Form | Syntax |
|---|---|
| Bool | `stream_cache: true` |
| Config | `stream_cache: { threshold: 65536 }` |

```yaml
- stream_cache: true
- stream_cache:
    threshold: 65536
```

### `wire_tap`

Send a fire-and-forget copy of the exchange to another endpoint.

```yaml
- wire_tap: "log:tap"
```

### `multicast`

Fan the exchange out to multiple endpoints.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `parallel` | bool | no | `false` | Send in parallel |
| `parallel_limit` | integer | no | — | Max parallel sends |
| `stop_on_exception` | bool | no | `false` | Stop on first error |
| `timeout_ms` | integer | no | — | Per-endpoint timeout |
| `aggregation` | string | no | `last_wins` | Aggregation strategy |
| `steps` | list | no | `[]` | Target endpoints as steps |

```yaml
- multicast:
    steps:
      - to: "log:a"
      - to: "log:b"
```

### `scatter_gather`

Fan out to a fixed set of endpoints and aggregate the results.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `endpoints` | list | no | `[]` | Target endpoint URIs |
| `aggregation` | string | no | `last_wins` | Aggregation strategy |

```yaml
- scatter_gather:
    endpoints:
      - "log:a"
      - "log:b"
```

### `recipient_list`

Resolve recipients from an expression and send to each.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `simple` | string | no | — | Simple expression for the recipient list |
| `rhai` | string | no | — | Rhai expression |
| `language` | string | no | — | Named expression language |
| `source` | string | no | — | Expression source for `language` |
| `delimiter` | string | no | `,` | URI delimiter |
| `parallel` | bool | no | `false` | Send in parallel |
| `parallel_limit` | integer | no | — | Max parallel sends |
| `stop_on_exception` | bool | no | `false` | Stop on first error |
| `strategy` | string | no | — | Aggregation strategy |

```yaml
- recipient_list:
    simple: "${header.recipients}"
```

### `routing_slip`

Route through a list of endpoints carried on the exchange.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `simple` | string | no | — | Simple expression for the slip |
| `rhai` | string | no | — | Rhai expression |
| `language` | string | no | — | Named expression language |
| `source` | string | no | — | Expression source for `language` |
| `uri_delimiter` | string | no | `,` | URI delimiter |
| `cache_size` | integer | no | `1000` | Endpoint cache size |
| `ignore_invalid_endpoints` | bool | no | `false` | Skip invalid endpoints |

```yaml
- routing_slip:
    simple: "${header.routeSlip}"
```

### `dynamic_router`

Resolve the next endpoint at each step until the expression returns empty.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `simple` | string | no | — | Simple expression |
| `rhai` | string | no | — | Rhai expression |
| `language` | string | no | — | Named expression language |
| `source` | string | no | — | Expression source for `language` |
| `uri_delimiter` | string | no | `,` | URI delimiter |
| `cache_size` | integer | no | `1000` | Endpoint cache size |
| `ignore_invalid_endpoints` | bool | no | `false` | Skip invalid endpoints |
| `max_iterations` | integer | no | `1000` | Max routing iterations |

```yaml
- dynamic_router:
    simple: "${header.nextEndpoint}"
```

### `throttle`

Rate-limit the exchange flow.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `max_requests` | integer | yes | — | Max requests per period |
| `period_secs` | integer | no | `1` | Time period in seconds |
| `strategy` | string | no | — | Throttle strategy |
| `steps` | list | no | `[]` | Child steps |

```yaml
- throttle:
    max_requests: 10
    steps:
      - to: "log:throttled"
```

### `load_balance`

Distribute exchanges across target endpoints.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `strategy` | string | no | `round_robin` | Load balance strategy |
| `distribution_ratio` | string | no | — | Weighted distribution |
| `steps` | list | no | `[]` | Target endpoints |

```yaml
- load_balance:
    strategy: "round_robin"
    steps:
      - to: "log:a"
      - to: "log:b"
```

### `enrich`

Enrich the exchange by requesting data from an endpoint.

| Form | Syntax |
|---|---|
| Short | `enrich: "http:..."` |
| Full | `enrich: { uri: "...", strategy: "...", timeout: 5000 }` |

The full form takes `uri` (required), `strategy`, and `timeout`.

```yaml
- enrich: "http:my-service/api/data"
- enrich:
    uri: "http:my-service/api/data"
    strategy: "use_enriched_body"
    timeout: 5000
```

### `poll_enrich`

Enrich the exchange by polling an endpoint. Same fields as `enrich`.

```yaml
- poll_enrich: "file:data"
```

### `validate`

Assert a predicate over the exchange. A failed assertion fails the exchange.

```yaml
- validate: "${body.field} != null"
```

### `idempotent_consumer`

Deduplicate exchanges by message ID.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | yes | — | Repository name |
| `expression` | string | yes | — | Message ID expression |
| `steps` | list | no | `[]` | Steps for first-time exchanges |
| `eager` | bool | no | — | Reserve the key before processing |
| `remove_on_failure` | bool | no | — | Remove the key if the child fails |

```yaml
- idempotent_consumer:
    repository: "memory"
    expression: "${header.messageId}"
    steps:
      - to: "log:first-time"
```

### `claim_check`

Stash or retrieve the message body in a claim check repository.

| Field | Type | Required | Description |
|---|---|---|---|
| `repository` | string | yes | Repository name |
| `operation` | string | yes | `set`, `get`, `get_and_remove`, `push`, or `pop` |
| `key` | string | yes | Claim check key expression |
| `filter` | string | no | Selective merge-back filter |

```yaml
- claim_check:
    repository: "memory"
    operation: "set"
    key: "${header.claimKey}"
```

### `cache`

Cache a computed body by key with TTL. On hit, serves the cached body. On miss, runs the `on_miss` sub-pipeline and stores the result.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | no | `"memory"` | Repository name |
| `key` | string | yes | — | Cache key expression (None = bypass cache) |
| `ttl` | duration | no | — | Time-to-live for the cached entry |
| `max_entry_bytes` | integer | no | 10 MiB | Maximum body size to cache |
| `on_miss` | list | yes | — | Sub-pipeline to run on cache miss |

```yaml
- cache:
    key: "${header.cacheKey}"
    ttl: "5s"
    on_miss:
      - set_body: "computed"
```

### `cache_invalidate`

Remove a single key from the cache repository.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Cache key expression |

```yaml
- cache_invalidate:
    key: "${header.cacheKey}"
```

### `cache_peek_stale`

Serve a cached entry, ignoring its in-band expiry. Used as a stale-read fallback.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Cache key expression |

```yaml
- cache_peek_stale:
    key: "${header.cacheKey}"
```

### `sampling`

Process one exchange out of every N.

| Form | Syntax |
|---|---|
| Short | `sampling: 5` (period) |
| Full | `sampling: { period: 5 }` |

```yaml
- sampling: 5
- sampling:
    period: 10
```

### `sort`

Sort the body array by a key expression.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `expression` | string | yes | — | Sort key expression |
| `reverse` | bool | no | `false` | Descending sort |
| `language` | string | no | — | Expression language |

```yaml
- sort:
    expression: "${body.field}"
    reverse: true
```

### `resequence`

Reorder exchanges by sequence number. Batch mode collects and sorts; stream mode
is reserved for future use.

| Field | Type | Required | Description |
|---|---|---|---|
| `batch` | object | no | Batch config: `correlation`, `sort`, `completion` |
| `stream` | object | no | Stream config (not yet implemented) |

The `completion` object accepts `size`, `timeout`, and `size_or_timeout`.

```yaml
- resequence:
    batch:
      correlation: "${header.seq}"
      sort: "asc"
      completion:
        size: 100
        timeout: 5000
```

## Route-level config

These objects attach to a route. See [Route structure](route-structure.md) for
where each one goes.

### Error handler config

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `dead_letter_channel` | string | no | — | DLC endpoint URI |
| `retry` | object | no | — | Redelivery policy |
| `on_exceptions` | list | no | — | Per-exception clauses |
| `use_original_message` | bool | no | `false` | Use original message in DLC |

### Redelivery policy

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `max_attempts` | integer | yes | — | Max retry attempts |
| `initial_delay_ms` | integer | no | `100` | Initial delay in ms |
| `multiplier` | float | no | `2.0` | Backoff multiplier |
| `max_delay_ms` | integer | no | `10000` | Max delay in ms |
| `jitter_factor` | float | no | `0.0` | Jitter factor (0.0-1.0) |
| `handled_by` | string | no | — | Route here after retries are exhausted |

### OnException clause

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | no | Error variant name to match |
| `message_contains` | string | no | Substring match on error message |
| `retry` | object | no | Per-clause redelivery policy |
| `steps` | list | no | Handler steps |
| `handled` | bool | no | Absorb the error |
| `continued` | bool | no | Clear error and continue the pipeline |

### Circuit breaker config

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `failure_threshold` | integer | no | `5` | Failures before opening |
| `open_duration_ms` | integer | no | `30000` | Duration in the open state |

### Security policy config

Choose exactly one form: `roles`, `scopes`, `ref`, `wasm`, or `permission`.

| Field | Type | Required | Description |
|---|---|---|---|
| `roles` | list | no | Required roles |
| `scopes` | list | no | Required scopes |
| `all_required` | bool | no | All roles/scopes required |
| `trust_upstream_principal` | bool | no | Accept a pre-populated principal with no token |
| `ref` | string | no | Reference to a policy |
| `wasm` | string | no | WASM policy source |
| `config` | map | no | Policy-specific config |
| `permission` | object | no | Permission-based policy |

## Top-level blocks

A route file also accepts these top-level keys alongside `routes`.

### REST DSL

The `rest` key defines REST API blocks.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `host` | string | no | `0.0.0.0` | Listen host |
| `port` | integer | no | `8080` | Listen port |
| `path` | string | no | `""` | Base path |
| `operations` | list | no | `[]` | HTTP operations |

REST operation:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `method` | string | yes | — | HTTP method (GET, POST, ...) |
| `path` | string | no | `/` | Sub-path |
| `operation_id` | string | no | — | Unique operation ID |
| `to` | string | no | — | Target endpoint URI |
| `steps` | list | no | `[]` | Child steps |
| `consumes` | string | no | `application/json` | Request content type |
| `produces` | string | no | `application/json` | Response content type |
| `success_status` | integer | no | — | Success HTTP status |
| `request_schema` | object | no | — | Request body schema |
| `response` | object | no | — | Response definition |
| `description` | string | no | — | Operation description |
| `parameters` | map | no | `{}` | Additional parameters |

### Template declaration

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | string | yes | — | Template identifier |
| `parameters` | list | no | `[]` | Template parameters |
| `routes` | list | no | `[]` | Route definitions with `{{param}}` placeholders |

### Templated route instantiation

| Field | Type | Required | Description |
|---|---|---|---|
| `route_template_ref` | string | yes | Template ID to instantiate |
| `route_id` | string | no | Override route ID |
| `parameters` | map | no | Concrete parameter values |

**Reference**: [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
