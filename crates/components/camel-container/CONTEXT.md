# camel-container

Docker container lifecycle component for rust-camel. Producer operations: list, run, start, stop, remove, exec, and network management (create/connect/disconnect/list/remove). Consumer mode: subscribes to Docker daemon events or container logs and emits one Exchange per event/log line.

## Language

**ContainerEndpoint**:
Endpoint for `container:<operation>` URIs; routes the operation through the Docker daemon.
_Avoid_: docker address, container address

**ContainerProducer**:
Producer that performs container operations (list/run/start/stop/remove/exec/network-create/network-connect/network-disconnect/network-list/network-remove).
_Avoid_: container operator, docker sender

**ContainerConsumer**:
Consumer that subscribes to Docker daemon events and emits one Exchange per event.
_Avoid_: event subscriber, docker listener

**ContainerTracker**:
Internal state that records created containers for cleanup on shutdown; safe for hot-reload.
`CONTAINER_TRACKER` is one process-global static shared by all `CamelContext` instances and tests in the process. This accepted trade-off preserves cleanup state across route hot-reload. It does not provide the multi-context or test isolation of the component-owned `ProviderMap` in ADR-0020.
_Avoid_: container registry, container state

**Auto-pull**:
When a `run` operation references an image not present locally, the producer pulls it before creating the container.
_Avoid_: image fetch, automatic pull

## Log-level policy

Per ADR-0012.

**Labels wired in Phase B (commit 5dcb80e1):**
All 4 sites are category (e) outside-contract: the Docker daemon operation failed and the route cannot meaningfully retry (daemon down, image not found, container ID invalid). Each site calls `runtime.metrics().increment_errors(route_id, label)` then logs at `error!` with `// log-policy: outside-contract`:
- `e:container:events-connect` (`ContainerConsumer::start_events_consumer`, initial connection failure)
- `e:container:events-stream` (`ContainerConsumer::start_events_consumer`, event stream broken mid-flight)
- `e:container:logs-connect` (`ContainerConsumer::start_logs_consumer`, initial connection failure)
- `e:container:logs-stream` (`ContainerConsumer::start_logs_consumer`, log stream broken mid-flight)

## Example dialogue

> "What happens if the Docker daemon goes down while consuming events?"
> "The container consumer's reconnect loop retries with exponential backoff. If all retries are exhausted, it records an error via `increment_errors` with label `e:container:events-connect`, logs at `error!` with `// log-policy: outside-contract`, and returns the error."

> "Can I track created containers for cleanup?"
> "Yes — the ContainerTracker records every container created by a `run` operation. On shutdown, it removes orphaned containers. The tracker is safe for hot-reload: stopping and restarting a route does not lose tracked containers."
> "Is ContainerTracker isolated per CamelContext?"
> "No. `CONTAINER_TRACKER` is process-global, so cleanup state is shared across contexts and tests in the same process. This is an accepted trade-off for retaining cleanup state across route hot-reload."
