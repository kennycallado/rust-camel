# Messaging verbs

Split, aggregate, reorder, sample, and offload message state.

## `split`
Split the body into fragments and process each.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `expression` | string/object | no | `body_lines` | Split expression. String form: `body_lines`, `lines`, `body_json_array`, or `json_array`. Language-block form also accepted. |
| `aggregation` | string | no | `last_wins` | Aggregation strategy |
| `parallel` | bool | no | `false` | Process fragments in parallel |
| `parallel_limit` | integer | no | — | Max parallel fragments |
| `trace_item_threshold` | integer | no | `100` | Above this fragment count, each fragment starts a new trace root span linked to the split segment span. `0` disables (legacy always-nested). |
| `stop_on_exception` | bool | no | `true` | Stop on first error |
| `streaming` | bool | no | `false` | Stream the split. `stream` applies only when this is `true`. |
| `stream.format` | string | no | `auto` | Stream record format: `ndjson`, `lines`, `chunks`, `zip`, `tar`, `tar.gz`, or `auto`. |
| `stream.max_record_bytes` | integer | no | 1 MiB | Per-record byte cap in streaming mode |
| `stream.batch_size` | integer | no | `1` | Records per streaming batch |
| `stream.chunk_size` | integer | no | — | Chunk byte size for `chunks` format |
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

## `aggregate`
Group exchanges by correlation key. Emits one combined exchange when a
completion condition fires. The combined exchange then continues down the
pipeline, so `aggregate` has no nested `steps` block.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `header` | string | no | `""` | Header used as the correlation key (header-based source; used when `correlation_key` is absent) |
| `correlation_key` | string | no | — | Expression correlation source (simple language); overrides `header` when both are present |
| `completion_size` | integer | no | — | Complete after N exchanges |
| `completion_timeout_ms` | integer | no | — | Complete after timeout |
| `completion_predicate` | object | no | — | Predicate-block completion trigger |
| `strategy` | string | no | `collect_all` | Aggregation strategy |
| `max_buckets` | integer | no | — | Max concurrent buckets |
| `max_bucket_size` | integer | no | builder default | Max exchanges held in one bucket before forced completion |
| `bucket_ttl_ms` | integer | no | — | Bucket time-to-live |
| `force_completion_on_stop` | bool | no | — | Emit pending buckets on route stop |
| `discard_on_timeout` | bool | no | — | Drop buckets that time out |

At least one non-empty source is required; when both are present, `correlation_key` overrides `header`, and an empty `correlation_key` string is rejected.

```yaml
- aggregate:
    header: "CorrelationId"
    completion_size: 10
- aggregate:
    correlation_key: "${header.orderId}"
    completion_timeout_ms: 5000
```

## `sort`
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

## `sampling`
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

## `resequence`
Reorder exchanges by sequence number. Batch mode collects and sorts a bounded
group. Stream mode reorders a continuous flow with gap detection.

| Field | Type | Required | Description |
|---|---|---|---|
| `batch` | object | no | Batch config. `correlation`, `sort`, and `completion` are all required inside it. |
| `stream` | object | no | Stream config. `sequence` is required inside it. |

The batch `completion` object accepts `size`, `timeout`, and `size_or_timeout`.

The `stream` object fields:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `sequence` | string | yes | — | Sequence number expression |
| `capacity` | integer | no | `1000` | Max in-flight sequence slots |
| `dedup` | bool | no | `false` | Drop duplicate sequence numbers |
| `gap_timeout` | integer (ms) | no | `5000` | Wait for a missing sequence number before the gap policy fires |
| `on_capacity_exceeded` | string | no | `log_and_drop` | Capacity policy: `log_and_drop` or `drop_oldest` |
| `on_gap` | string | no | `emit_partial` | Gap policy: `emit_partial` or `drop_and_log` |

```yaml
- resequence:
    batch:
      correlation: "${header.seq}"
      sort: "asc"
      completion:
        size: 100
        timeout: 5000
```

```yaml
- resequence:
    stream:
      sequence: "${header.seq}"
      capacity: 500
      on_gap: drop_and_log
```

## `claim_check`
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
