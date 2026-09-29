# Resilience and control

Handle errors, pace traffic, repeat steps, and validate content.

## `do_try`
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

## `delay`
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

## `loop`
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

## `throttle`
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

## `idempotent_consumer`
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

## `validate`
Assert a predicate over the exchange. A failed assertion fails the exchange.

```yaml
- validate: "${body.field} != null"
```
