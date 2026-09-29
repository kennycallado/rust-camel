# Route-level config
These objects attach to a route. See [Route structure](route-structure.md) for
where each one goes.

## Error handler config
| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `dead_letter_channel` | string | no | — | DLC endpoint URI |
| `retry` | object | no | — | Redelivery policy |
| `on_exceptions` | list | no | — | Per-exception clauses |
| `use_original_message` | bool | no | `false` | Use original message in DLC |

## Redelivery policy
| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `max_attempts` | integer | yes | — | Max retry attempts |
| `initial_delay_ms` | integer | no | `100` | Initial delay in ms |
| `multiplier` | float | no | `2.0` | Backoff multiplier |
| `max_delay_ms` | integer | no | `10000` | Max delay in ms |
| `jitter_factor` | float | no | `0.0` | Jitter factor (0.0-1.0) |

The retry block carries redelivery parameters only. It rejects unknown
fields. `handled_by` is not a retry field; it lives on the clause.

## OnException clause
| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | no | Error variant name to match |
| `message_contains` | string | no | Substring match on error message |
| `retry` | object | no | Per-clause redelivery policy |
| `steps` | list | no | Handler steps |
| `handled` | bool | no | Absorb the error |
| `continued` | bool | no | Clear error and continue the pipeline |
| `handled_by` | string | no | Delegate endpoint URI. Runs after retries are exhausted |

`retry` and `handled_by` compose: the failing step is retried first, and
the delegate runs once retries are exhausted. With no `retry` block, the
step runs once and then delegates; no `CamelRedelivered` header is set.
`handled_by` without `handled` or `continued` is a tap: the delegate
receives the failed exchange, and the original error propagates.

Two clause shapes are load errors:

- `handled_by` inside the `retry` block. Unknown fields are rejected, so
  the legacy `retry: {handled_by: ...}` layout fails to load. Use
  `dead_letter_channel` for a route-level catch-all delegate.
- `steps` and `handled_by` on one clause. Loading fails with the typed
  `ConfigValidationError::OnExceptionStepsHandledByConflict`.

## Circuit breaker config
| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `failure_threshold` | integer | no | `5` | Failures before opening |
| `open_duration_ms` | integer | no | `30000` | Duration in the open state |
| `fallback` | list | no | — | Sub-pipeline executed while the circuit is open |

The `fallback` list holds a sub-pipeline of steps. The breaker runs it instead of
rejecting the exchange while the circuit is open. See
[Circuit breaker](route-structure.md#circuit-breaker) for the full surface.

## Security policy config
Choose exactly one form: `roles`, `scopes`, `ref`, `wasm`, or `permission`.

| Field | Type | Required | Description |
|---|---|---|---|
| `roles` | list | no | Required roles |
| `scopes` | list | no | Required scopes |
| `all_required` | bool | no | All roles/scopes required |
| `audiences` | list | no | Accepted audience values. See [Authentication and authorization](../services/auth.md). |
| `credential_sources` | list | no | Where the credential is read from. Same forms as route-level `credential_sources`. |
| `provider` | string | no | Auth provider selection |
| `ref` | string | no | Reference to a policy |
| `wasm` | string | no | WASM policy source |
| `config` | map | no | Policy-specific config |
| `permission` | object | no | Permission-based policy |

