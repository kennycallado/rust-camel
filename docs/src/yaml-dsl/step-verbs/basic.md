# Basic steps

Send to endpoints, log, set message fields, and call beans.

## `to`
Send the exchange to an endpoint URI.

| Field | Type | Required | Description |
|---|---|---|---|
| `to` | string | yes | Target endpoint URI |
| `parameters` | map | no | Per-endpoint parameters merged into the URI query (`{}` default) |

```yaml
- to: "log:info"
```

## `log`
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

## `set_header`
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

## `remove_header`
Remove a message header from the input message. If the header is absent,
the step does nothing (no error). Removal is input-only: output message
headers are not changed.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Header name to remove |

```yaml
- remove_header:
    key: "CamelHttpPath"
```

## `set_property`
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

## `set_body`
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

## `stop`
Stop route processing. The exchange returns to the consumer as a successful
response.

```yaml
- stop: true
```

## `stream_cache`
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

## `bean`
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

## `function`
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
