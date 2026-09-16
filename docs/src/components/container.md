# Container

The container component manages Docker containers from a route. It runs container operations as a producer. It subscribes to Docker daemon events or container logs as a consumer.

```yaml
routes:
  - id: container-run
    from: "timer:tick?period=60000&repeatCount=1"
    steps:
      - to: "container:run?image=alpine:3.20"
      - to: "log:container?showBody=true"
```

The producer handles `list`, `run`, `start`, `stop`, `remove`, `exec`, and network management (`create`, `connect`, `disconnect`, `list`, `remove`). A `run` operation pulls an image that is not present locally before it creates the container. Consumer mode emits one Exchange per daemon event or log line.

The component tracks every container it creates. On shutdown it removes orphaned containers. The tracker is process-global and safe for hot-reload.

**Reference**: [camel-container CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-container/CONTEXT.md).