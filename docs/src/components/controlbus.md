# ControlBus

The ControlBus component sends Route lifecycle commands through the RuntimeBus. It is a Producer-only Endpoint. It does not consume. It exposes no network API.

The controlbus-example uses a timer to suspend and resume a target route:

```rust,ignore
{{#include ../../../examples/controlbus/src/main.rs:controlbus-suspend-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: target-route
    from: "timer:target?period=500"
    steps:
      - to: "log:target?showBody=true"
  - id: suspend-controller
    from: "timer:suspend?delay=5000&repeatCount=1"
    steps:
      - to: "controlbus:route?routeId=target-route&action=suspend&authorizedRoutes=target-route"
      - to: "log:control?showBody=true"
```

The ControlBus URI declares the target `routeId` and the `authorizedRoutes` allowlist at config time. Exchange headers cannot set the target.

</details>

## URI

```text
controlbus:route?routeId=<id>&action=<action>&authorizedRoutes=<csv>
```

| Parameter | Required | Description |
| --- | --- | --- |
| `routeId` | yes | Target Route ID. Must differ from the calling Route |
| `action` | yes | Lifecycle command. One of `start`, `stop`, `suspend`, `resume`, `restart`, or `status` |
| `authorizedRoutes` | yes | Comma-separated allowlist. Endpoint fails closed when absent |

## Actions

| Action | Runtime command | Response body |
| --- | --- | --- |
| `start` | `StartRoute` | empty |
| `stop` | `StopRoute` | empty |
| `suspend` | `SuspendRoute` | empty |
| `resume` | `ResumeRoute` | empty |
| `restart` | `ReloadRoute` | empty |
| `status` | `GetRouteStatus` | `Body::Text` with the lifecycle status string |

`restart` performs an atomic Pipeline swap without drain semantics ([ADR-0004](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0004-hot-reload-atomic-pipeline-swap.md)). Suspend and resume support varies by component. `status` returns only the lifecycle string, not Route statistics.

## Authorization

The Producer enforces three gates on every call ([ADR-0034](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0034-controlbus-capability-authz.md)):

1. The URI declares the target `routeId`.
2. `authorizedRoutes` exists and contains that target.
3. The target differs from the calling Route ID.

The `CamelRouteId` Exchange header cannot select or override the target. Exchange data is untrusted ([ADR-0032](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0032-exchange-data-trust-boundary.md)). Only operator configuration drives the control plane. Authorization failures return `CamelError::Unauthorized`.

## Errors

| Failure | Result |
| --- | --- |
| Missing or unauthorized target | `CamelError::Unauthorized` |
| Unknown action or unexpected status response | `CamelError::ProcessorError` |
| RuntimeHandle error | passes through unchanged |

The component declares no public enums. `camel-api` provides `RouteAction`, `RuntimeCommand`, and `CamelError`. Future variants use fallback match arms ([ADR-0049](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md)).

**Reference**: [ControlBus crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-controlbus/CONTEXT.md). Example source: [`examples/controlbus`](https://github.com/kennycallado/rust-camel/tree/main/examples/controlbus).
