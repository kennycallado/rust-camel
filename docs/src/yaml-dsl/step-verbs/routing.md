# Routing verbs

Filter, branch, and copy the exchange to one or more targets.

## `filter`
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

## `choice`
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

## `wire_tap`
Send a fire-and-forget copy of the exchange to another endpoint.

The full form takes `uri` and an optional `parameters` map merged into the URI query.

```yaml
- wire_tap: "log:tap"
```

## `multicast`
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

## `scatter_gather`
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

## `recipient_list`
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

## `routing_slip`
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

## `dynamic_router`
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

## `load_balance`
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
