# Master

The Master component runs a delegate Consumer only on the node that holds a leadership lock. Other nodes join the same lock name and stand by until they win. ADR-0035 establishes the leader-epoch fencing token that the bridge stamps on every emitted envelope.

```rust,ignore
{{#include ../../../examples/master-leader/src/main.rs:master-route}}
```

The example drives a `timer:tick` source through a `master:mylock` lock. Only the elected leader consumes ticks. A second route uses [ControlBus](../components/controlbus.md) to poll the leader's status every five seconds.

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: master-route
    from: "master:mylock:timer:tick?period=1000"
    steps:
      - log: "DEBUG: tick"
```

The route definition runs on every node. Only the leader fires the `log` step. The YAML example in `examples/master-leader-yaml` registers the same lock from two route IDs to demonstrate the same behavior from config.

</details>

## URI

```
master:<lockname>:<delegate-uri>
```

| Segment | Required | Description |
| --- | --- | --- |
| `lockname` | yes | Leadership lock name. Nodes with the same lock name compete for one leader. |
| `delegate-uri` | yes | Full URI for any consumer Component (`timer:`, `kafka:`, `http:`, etc.). The Master wraps the Consumer this URI creates. |

The Master has no URI query parameters of its own. Query parameters belong to the delegate Component.

## Consumer

`master:<lockname>:<delegate-uri>` gates a delegate Consumer on leadership. When the route starts, the node joins leader election under the lock name. The delegate Consumer does not start until the node wins. On leadership loss, the delegate stops and drains within `drain_timeout_ms`. If the node wins again, the delegate restarts with the same delegate URI.

Each ExchangeEnvelope the delegate emits carries an `x-camel-leader-epoch` Exchange property. The Master stamps the property with a monotonic fencing token at bridge start. A stale bridge retains the epoch from its own spawn, not the live epoch. Downstream sinks that need split-brain safety compare the property against the current leader's epoch and reject older envelopes.

## Producer

`to("master:mylock:http://api.example.com")` passes through to the delegate Producer without leader gating. The Master's job is to gate Consumers, not Producers. A write that needs exclusive access should serialize through a queue that the leader Consumer drains.

## Leadership backends

The leader election backend comes from the configured `PlatformService`. The default `NoopPlatformService` always elects the local node, so a single-node deployment works without external infrastructure. A Kubernetes deployment uses `KubernetesPlatformService` and Lease objects for distributed election across pods.

Every acquired term increments the leader epoch. The bridge stamps the new epoch on each envelope. A node that loses leadership stops the delegate within `drain_timeout_ms` and steps down. The route stays alive; only delegate intake pauses until the node wins again.

## Configuration

The Master reads from `[components.master]` in `Camel.toml`:

| Key | Default | Description |
| --- | --- | --- |
| `drain_timeout_ms` | `5000` | Max time to wait for the delegate Consumer to shut down on leadership loss |
| `delegate_retry_max_attempts` | unlimited | Backward-compat alias for `reconnect.max_attempts`. `0` means unlimited |
| `reconnect.max_attempts` | `0` | Bounded retry attempts on delegate start failure. `0` means unlimited |
| `reconnect.enabled` | `true` | Enable bounded reconnect retries on delegate start failure |

When both `reconnect` and `delegate_retry_max_attempts` are set, the explicit `reconnect` value wins. The `delegate_retry_max_attempts` field stays for backward compatibility with earlier configs.

**Reference**: [Master component CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md). Example source: [`examples/master-leader`](https://github.com/kennycallado/rust-camel/tree/main/examples/master-leader) and [`examples/master-leader-yaml`](https://github.com/kennycallado/rust-camel/tree/main/examples/master-leader-yaml).
