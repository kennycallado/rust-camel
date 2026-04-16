# kubernetes-platform

Demonstrates Kubernetes leader election end-to-end with **no external Kubernetes cluster**.

The example starts a local K3s cluster in Docker via testcontainers, launches two simulated pods (`pod-alpha` and `pod-beta`) against the same Lease, then shows:

1. One pod becomes leader
2. Current leader steps down
3. Remaining pod takes over leadership (failover)

## Requirements

- Docker running locally

## Run

```bash
cargo run -p camel-platform-kubernetes-example
```

## Expected output

You should see human-readable progress messages similar to:

- K3s startup
- Initial leader selected (`pod-alpha` or `pod-beta`)
- Step-down triggered for current leader
- New leader selected after failover

This confirms Lease-based leader election and takeover behavior is working using only Docker.
